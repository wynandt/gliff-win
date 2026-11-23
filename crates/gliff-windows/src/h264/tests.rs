use gliff_sw::Encoder;
use gliff_sw::EncoderSettings;

use super::BitReader;
use super::LevelVerdict;
use super::SeqParameterSet;
use super::SpsInfo;
use super::VuiParameters;
use super::find_sps;
use super::nal_units;
use super::parse_seq_parameter_set_data;
use super::parse_sps;
use super::set_level_idc;
use super::unescape;
use super::without_reorder_delay;

fn openh264_keyframe(width: u32, height: u32) -> Vec<u8> {
    let mut encoder = Encoder::new(
        EncoderSettings {
            width,
            height,
            bitrate: 4_000_000,
            framerate: 30,
        },
        false,
    )
    .expect("OpenH264 encoder");
    let bgra = vec![128u8; (width * height * 4) as usize];
    encoder.encode_bgra(&bgra, true).expect("encode").main
}

#[test]
fn openh264_sps_reads_baseline_and_the_cropped_size() {
    let au = openh264_keyframe(1920, 1080);
    let sps = find_sps(&au)
        .expect("keyframe carries an SPS")
        .expect("parses");
    assert_eq!(
        (sps.width, sps.height),
        (1920, 1080),
        "cropped size: {sps:?}"
    );
    assert_eq!(sps.profile_idc, 66, "gliff-sw encodes Baseline: {sps:?}");
    assert_eq!(sps.decoder_accepts(), LevelVerdict::Accepted, "{sps:?}");
}

#[test]
fn rewritten_level_is_read_back_and_rejected() {
    let mut au = openh264_keyframe(640, 360);
    assert_eq!(set_level_idc(&mut au, 60), 1, "one SPS rewritten");
    let sps = find_sps(&au).expect("SPS").expect("parses");
    assert_eq!(
        (sps.level_idc, sps.level_name()),
        (60, "6.0".to_owned()),
        "{sps:?}"
    );
    assert_eq!(
        sps.decoder_accepts(),
        LevelVerdict::Rejected,
        "6.0 is above OpenH264's 5.2"
    );
    assert_eq!(
        (sps.width, sps.height),
        (640, 360),
        "rest of the SPS intact"
    );
}

#[test]
fn libx264_high_level_6_sps_parses() {
    let nal = [
        0x67, 0x64, 0x00, 0x3c, 0xac, 0xd9, 0x40, 0x3c, 0x00, 0x4b, 0x6c, 0x04, 0x40, 0x00, 0x00,
        0x03, 0x00, 0x40, 0x00, 0x00, 0x0c, 0x83, 0xc2, 0x01, 0x06, 0x58,
    ];
    let sps = parse_sps(&nal).expect("parses");
    assert_eq!(
        (sps.profile_name(), sps.level_name(), sps.width, sps.height),
        ("High".to_owned(), "6.0".to_owned(), 3840, 2400),
        "{sps:?}"
    );
}

#[test]
fn names_cover_constrained_baseline_and_1b() {
    let base = SpsInfo {
        profile_idc: 66,
        constraint_flags: 0xc0,
        level_idc: 11,
        width: 16,
        height: 16,
    };
    assert_eq!(
        base.profile_name(),
        "Constrained Baseline",
        "constraint_set1"
    );
    assert_eq!(base.level_name(), "1.1", "no constraint_set3");
    let one_b = SpsInfo {
        constraint_flags: 0xd0,
        ..base
    };
    assert_eq!(one_b.level_name(), "1b", "constraint_set3 at 11");
}

#[test]
fn nal_units_strip_start_codes_and_trailing_zeros() {
    let au = [
        0, 0, 0, 1, 9, 0xf0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 0, 1, 0x65, 7,
    ];
    assert_eq!(
        nal_units(&au),
        vec![&[9, 0xf0][..], &[0x67, 1, 2][..], &[0x65, 7][..]],
        "three units"
    );
    assert!(
        find_sps(&[0, 0, 1, 0x65, 1]).is_none(),
        "no SPS in a P unit"
    );
}

#[test]
fn truncated_sps_is_an_error_not_a_panic() {
    for len in 1..8 {
        let nal = &[0x67, 0x64, 0x00, 0x34, 0xac, 0xd9, 0x40][..len];
        assert!(parse_sps(nal).is_err(), "length {len} must fail cleanly");
    }
}

#[test]
fn sps_without_vui_gains_no_reorder_restriction() {
    let server_sps = [
        0x67, 0x64, 0x00, 0x34, 0xac, 0x1b, 0x1a, 0x80, 0x28, 0x00, 0xb5, 0x90, 0x00,
    ];
    let restricted = [
        0x67, 0x64, 0x00, 0x34, 0xac, 0x1b, 0x1a, 0x80, 0x28, 0x00, 0xb5, 0xa0, 0x1b, 0x41, 0x10,
        0x8d, 0x40,
    ];
    let slice = [0x65, 0x88, 0x84];
    let au: Vec<u8> = [&[0, 0, 0, 1][..], &server_sps, &[0, 0, 1], &slice].concat();
    let out = without_reorder_delay(&au).expect("the SPS needs restrictions");
    assert_eq!(nal_units(&out), [&restricted[..], &slice[..]]);
    assert_eq!(parse_sps(&restricted), parse_sps(&server_sps));
    assert_eq!(parse_sps(&restricted).expect("parses").width, 2560);
    assert_named_syntax("high, 2560x1440", &server_sps, &restricted);
    assert_eq!(without_reorder_delay(&[0, 0, 1, 0x65, 0x88, 0x84]), None);
    let with_vui = [&[0, 0, 1][..], &restricted].concat();
    assert_eq!(without_reorder_delay(&with_vui), None);
}

#[test]
fn restriction_bytes_cover_the_sps_shapes_the_writer_rebuilds() {
    let cases: &[(&str, &[u8], &[u8])] = &[
        (
            "baseline, cropped",
            &[0x67, 0x42, 0xe0, 0x1f, 0xf4, 0x03, 0xc0, 0x11, 0x3f, 0x2a],
            &[
                0x67, 0x42, 0xe0, 0x1f, 0xf4, 0x03, 0xc0, 0x11, 0x3f, 0x2c, 0x03, 0x68, 0x22, 0x11,
                0xa8,
            ],
        ),
        (
            "baseline, pic_order_cnt_type 2, no reference frames",
            &[0x67, 0x42, 0xe0, 0x1f, 0xdc, 0x07, 0x80, 0x22, 0x64],
            &[
                0x67, 0x42, 0xe0, 0x1f, 0xdc, 0x07, 0x80, 0x22, 0x68, 0x06, 0xd0, 0x44, 0x23, 0x50,
            ],
        ),
        (
            "baseline, pic_order_cnt_type 1, reserved constraint bits",
            &[
                0x67, 0x42, 0x03, 0x1f, 0xd1, 0x91, 0xa3, 0x96, 0x07, 0x80, 0x22, 0x64,
            ],
            &[
                0x67, 0x42, 0x03, 0x1f, 0xd1, 0x91, 0xa3, 0x96, 0x07, 0x80, 0x22, 0x68, 0x06, 0xd0,
                0x44, 0x23, 0x2c,
            ],
        ),
        (
            "baseline, field picture, cropping",
            &[0x67, 0x42, 0xe0, 0x1f, 0xf4, 0x0a, 0x0f, 0x54, 0x92, 0x40],
            &[
                0x67, 0x42, 0xe0, 0x1f, 0xf4, 0x0a, 0x0f, 0x54, 0x92, 0x80, 0x6d, 0x04, 0x42, 0x35,
            ],
        ),
        (
            "high, 4:2:0",
            &[0x67, 0x64, 0x00, 0x34, 0xac, 0xe8, 0x02, 0x80, 0x0b, 0x59],
            &[
                0x67, 0x64, 0x00, 0x34, 0xac, 0xe8, 0x02, 0x80, 0x0b, 0x5a, 0x01, 0xb4, 0x11, 0x08,
                0xd4,
            ],
        ),
        (
            "high, monochrome",
            &[0x67, 0x64, 0x00, 0x34, 0xf3, 0xa0, 0x40, 0x21, 0x90],
            &[
                0x67, 0x64, 0x00, 0x34, 0xf3, 0xa0, 0x40, 0x21, 0xa0, 0x1b, 0x41, 0x10, 0x8d, 0x40,
            ],
        ),
        (
            "high, 4:2:2, scaling matrix present with every list absent",
            &[
                0x67, 0x64, 0x00, 0x34, 0xb6, 0xf0, 0x0e, 0x80, 0x28, 0x00, 0xb5, 0x90,
            ],
            &[
                0x67, 0x64, 0x00, 0x34, 0xb6, 0xf0, 0x0e, 0x80, 0x28, 0x00, 0xb5, 0xa0, 0x1b, 0x41,
                0x10, 0x8d, 0x40,
            ],
        ),
        (
            "high 4:4:4, separate colour plane, scaling lists",
            &[
                0x67, 0xf4, 0x00, 0x28, 0x21, 0x3b, 0x08, 0xff, 0xff, 0xc2, 0x20, 0x40, 0x21, 0x01,
                0x6c, 0xe0, 0x14, 0x00, 0x5a, 0xc8,
            ],
            &[
                0x67, 0xf4, 0x00, 0x28, 0x21, 0x3b, 0x08, 0xff, 0xff, 0xc2, 0x20, 0x40, 0x21, 0x01,
                0x6c, 0xe0, 0x14, 0x00, 0x5a, 0xd0, 0x0d, 0xa0, 0x88, 0x46, 0x78,
            ],
        ),
        (
            "emulation prevention in the SPS",
            &[0x67, 0x42, 0x00, 0x00, 0x03, 0x00, 0x80, 0x74, 0xe2],
            &[
                0x67, 0x42, 0x00, 0x00, 0x03, 0x00, 0x80, 0x74, 0xe4, 0x03, 0x68, 0x22, 0x11, 0xa8,
            ],
        ),
    ];
    for (name, nal, restricted) in cases {
        let au = [&[0, 0, 0, 1][..], nal].concat();
        let out = without_reorder_delay(&au).expect(name);
        assert_eq!(nal_units(&out), vec![*restricted], "{name}");
        assert_eq!(
            parse_sps(restricted).expect(name),
            parse_sps(nal).expect(name),
            "{name}: SPS info"
        );
        assert_named_syntax(name, nal, restricted);
    }
    let already_has_vui = [
        0x67, 0x64, 0x00, 0x3c, 0xac, 0xd9, 0x40, 0x3c, 0x00, 0x4b, 0x6c, 0x04, 0x40, 0x00, 0x00,
        0x03, 0x00, 0x40, 0x00, 0x00, 0x0c, 0x83, 0xc2, 0x01, 0x06, 0x58,
    ];
    let with_vui = [&[0, 0, 0, 1][..], &already_has_vui[..]].concat();
    assert_eq!(
        without_reorder_delay(&with_vui),
        None,
        "an SPS that already has a VUI is left alone"
    );
}

#[test]
fn openh264_sps_with_a_vui_is_left_alone() {
    let au = openh264_keyframe(320, 240);
    assert_eq!(
        without_reorder_delay(&au),
        None,
        "the OpenH264 SPS already carries a VUI"
    );
}

fn assert_named_syntax(name: &str, nal: &[u8], restricted: &[u8]) {
    let input = read_named(nal);
    let output = read_named(restricted);
    assert_eq!(
        (
            output.profile_idc,
            output.constraint_set0_flag,
            output.constraint_set1_flag,
            output.constraint_set2_flag,
            output.constraint_set3_flag,
            output.constraint_set4_flag,
            output.constraint_set5_flag,
            output.reserved_zero_2bits,
            output.level_idc,
            output.seq_parameter_set_id,
        ),
        (
            input.profile_idc,
            input.constraint_set0_flag,
            input.constraint_set1_flag,
            input.constraint_set2_flag,
            input.constraint_set3_flag,
            input.constraint_set4_flag,
            input.constraint_set5_flag,
            input.reserved_zero_2bits,
            input.level_idc,
            input.seq_parameter_set_id,
        ),
        "{name}: profile_idc, constraint_set flags, level_idc, seq_parameter_set_id"
    );
    assert!(
        !input.vui_parameters_present_flag,
        "{name}: input vui_parameters_present_flag"
    );
    assert!(
        output.vui_parameters_present_flag,
        "{name}: vui_parameters_present_flag"
    );
    assert_eq!(
        sps_without_vui(&output),
        sps_without_vui(&input),
        "{name}: SPS syntax elements"
    );
    let vui = output.vui.as_ref().expect("vui_parameters");
    assert!(
        !vui.aspect_ratio_info_present_flag
            && !vui.overscan_info_present_flag
            && !vui.video_signal_type_present_flag
            && !vui.chroma_loc_info_present_flag
            && !vui.timing_info_present_flag
            && !vui.nal_hrd_parameters_present_flag
            && !vui.vcl_hrd_parameters_present_flag
            && !vui.pic_struct_present_flag,
        "{name}: vui present-flags"
    );
    assert!(
        vui.bitstream_restriction_flag,
        "{name}: bitstream_restriction_flag"
    );
    assert_eq!(
        vui.max_num_reorder_frames, 0,
        "{name}: max_num_reorder_frames"
    );
    assert_eq!(
        vui.max_dec_frame_buffering,
        input.max_num_ref_frames.max(1),
        "{name}: max_dec_frame_buffering"
    );
    assert_eq!(
        vui,
        &VuiParameters {
            aspect_ratio_info_present_flag: false,
            overscan_info_present_flag: false,
            video_signal_type_present_flag: false,
            chroma_loc_info_present_flag: false,
            timing_info_present_flag: false,
            nal_hrd_parameters_present_flag: false,
            vcl_hrd_parameters_present_flag: false,
            pic_struct_present_flag: false,
            bitstream_restriction_flag: true,
            motion_vectors_over_pic_boundaries_flag: true,
            max_bytes_per_pic_denom: 2,
            max_bits_per_mb_denom: 1,
            log2_max_mv_length_horizontal: 16,
            log2_max_mv_length_vertical: 16,
            max_num_reorder_frames: 0,
            max_dec_frame_buffering: input.max_num_ref_frames.max(1),
        },
        "{name}: vui syntax elements"
    );
}

fn sps_without_vui(sps: &SeqParameterSet) -> SeqParameterSet {
    let mut copy = sps.clone();
    copy.vui_parameters_present_flag = false;
    copy.vui = None;
    copy
}

fn read_named(nal: &[u8]) -> SeqParameterSet {
    let rbsp = unescape(&nal[1..]);
    let mut reader = BitReader::new(&rbsp);
    let mut sps = parse_seq_parameter_set_data(&mut reader).expect("seq_parameter_set_data");
    if sps.vui_parameters_present_flag {
        sps.vui = Some(read_vui(&mut reader));
        let rbsp_stop_one_bit = reader.flag().expect("rbsp_stop_one_bit");
        assert!(rbsp_stop_one_bit, "rbsp_stop_one_bit");
        while !reader.pos.is_multiple_of(8) {
            let rbsp_alignment_zero_bit = reader.flag().expect("rbsp_alignment_zero_bit");
            assert!(!rbsp_alignment_zero_bit, "rbsp_alignment_zero_bit");
        }
        assert_eq!(reader.pos, rbsp.len() * 8, "rbsp_trailing_bits");
    }
    sps
}

fn read_vui(reader: &mut BitReader<'_>) -> VuiParameters {
    VuiParameters {
        aspect_ratio_info_present_flag: reader.flag().expect("aspect_ratio_info_present_flag"),
        overscan_info_present_flag: reader.flag().expect("overscan_info_present_flag"),
        video_signal_type_present_flag: reader.flag().expect("video_signal_type_present_flag"),
        chroma_loc_info_present_flag: reader.flag().expect("chroma_loc_info_present_flag"),
        timing_info_present_flag: reader.flag().expect("timing_info_present_flag"),
        nal_hrd_parameters_present_flag: reader.flag().expect("nal_hrd_parameters_present_flag"),
        vcl_hrd_parameters_present_flag: reader.flag().expect("vcl_hrd_parameters_present_flag"),
        pic_struct_present_flag: reader.flag().expect("pic_struct_present_flag"),
        bitstream_restriction_flag: reader.flag().expect("bitstream_restriction_flag"),
        motion_vectors_over_pic_boundaries_flag: reader
            .flag()
            .expect("motion_vectors_over_pic_boundaries_flag"),
        max_bytes_per_pic_denom: reader.ue().expect("max_bytes_per_pic_denom"),
        max_bits_per_mb_denom: reader.ue().expect("max_bits_per_mb_denom"),
        log2_max_mv_length_horizontal: reader.ue().expect("log2_max_mv_length_horizontal"),
        log2_max_mv_length_vertical: reader.ue().expect("log2_max_mv_length_vertical"),
        max_num_reorder_frames: reader.ue().expect("max_num_reorder_frames"),
        max_dec_frame_buffering: reader.ue().expect("max_dec_frame_buffering"),
    }
}
