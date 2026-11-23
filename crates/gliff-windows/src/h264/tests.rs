use gliff_sw::Encoder;
use gliff_sw::EncoderSettings;

use super::BitReader;
use super::BitWriter;
use super::BitstreamRestriction;
use super::LevelVerdict;
use super::SpsInfo;
use super::VuiParameters;
use super::escape;
use super::find_sps;
use super::nal_units;
use super::parse_sps;
use super::parse_sps_rbsp;
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
    assert_eq!(without_reorder_delay(&[0, 0, 1, 0x65, 0x88, 0x84]), None);
    let with_vui = [&[0, 0, 1][..], &restricted].concat();
    assert_eq!(without_reorder_delay(&with_vui), None);
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedVuiTail {
    vui_parameters_present_flag: bool,
    vui: VuiParameters,
    rbsp_stop_one_bit: bool,
    rbsp_alignment_zero_bits: usize,
}

fn decode_vui_after_sps(nal: &[u8]) -> DecodedVuiTail {
    let rbsp = unescape(nal.get(1..).expect("SPS NAL has header byte"));
    let parsed = parse_sps_rbsp(&rbsp).expect("rewritten SPS parses");
    let mut r = BitReader::new(&rbsp);
    for _ in 0..parsed.vui_parameters_present_flag_bit {
        r.bit().expect("SPS prefix bit");
    }
    let vui_parameters_present_flag = r.bit().expect("vui_parameters_present_flag") == 1;
    let aspect_ratio_info_present_flag = r.bit().expect("aspect_ratio_info_present_flag") == 1;
    let overscan_info_present_flag = r.bit().expect("overscan_info_present_flag") == 1;
    let video_signal_type_present_flag = r.bit().expect("video_signal_type_present_flag") == 1;
    let chroma_loc_info_present_flag = r.bit().expect("chroma_loc_info_present_flag") == 1;
    let timing_info_present_flag = r.bit().expect("timing_info_present_flag") == 1;
    let nal_hrd_parameters_present_flag = r.bit().expect("nal_hrd_parameters_present_flag") == 1;
    let vcl_hrd_parameters_present_flag = r.bit().expect("vcl_hrd_parameters_present_flag") == 1;
    let pic_struct_present_flag = r.bit().expect("pic_struct_present_flag") == 1;
    let bitstream_restriction_flag = r.bit().expect("bitstream_restriction_flag") == 1;
    let bitstream_restriction = if bitstream_restriction_flag {
        Some(BitstreamRestriction {
            motion_vectors_over_pic_boundaries_flag: r
                .bit()
                .expect("motion_vectors_over_pic_boundaries_flag")
                == 1,
            max_bytes_per_pic_denom: r.ue().expect("max_bytes_per_pic_denom"),
            max_bits_per_mb_denom: r.ue().expect("max_bits_per_mb_denom"),
            log2_max_mv_length_horizontal: r.ue().expect("log2_max_mv_length_horizontal"),
            log2_max_mv_length_vertical: r.ue().expect("log2_max_mv_length_vertical"),
            max_num_reorder_frames: r.ue().expect("max_num_reorder_frames"),
            max_dec_frame_buffering: r.ue().expect("max_dec_frame_buffering"),
        })
    } else {
        None
    };
    let rbsp_stop_one_bit = r.bit().expect("rbsp_stop_one_bit") == 1;
    let total_bits = rbsp.len() * 8;
    let mut rbsp_alignment_zero_bits = 0;
    while r.pos < total_bits {
        assert_eq!(
            r.bit().expect("rbsp_alignment_zero_bit"),
            0,
            "trailing alignment bit must be zero"
        );
        rbsp_alignment_zero_bits += 1;
    }
    assert!(
        rbsp_alignment_zero_bits < 8,
        "RBSP must not have extra trailing bytes: got {rbsp_alignment_zero_bits} alignment bits"
    );
    DecodedVuiTail {
        vui_parameters_present_flag,
        vui: VuiParameters {
            aspect_ratio_info_present_flag,
            overscan_info_present_flag,
            video_signal_type_present_flag,
            chroma_loc_info_present_flag,
            timing_info_present_flag,
            nal_hrd_parameters_present_flag,
            vcl_hrd_parameters_present_flag,
            pic_struct_present_flag,
            bitstream_restriction,
        },
        rbsp_stop_one_bit,
        rbsp_alignment_zero_bits,
    }
}

fn write_bits(w: &mut BitWriter, value: u32, count: u32) {
    for shift in (0..count).rev() {
        w.bit((value >> shift) & 1 == 1);
    }
}

fn build_baseline_sps(
    pic_order_cnt_type: u32,
    max_num_ref_frames: u32,
    pic_width_in_mbs_minus1: u32,
    pic_height_in_map_units_minus1: u32,
) -> Vec<u8> {
    let mut w = BitWriter::default();
    let profile_idc = 66u32;
    let constraint_set_flags = 0xc0u32;
    let level_idc = 31u32;
    let seq_parameter_set_id = 0u32;
    let log2_max_frame_num_minus4 = 0u32;
    let log2_max_pic_order_cnt_lsb_minus4 = 0u32;
    let gaps_in_frame_num_value_allowed_flag = false;
    let frame_mbs_only_flag = true;
    let direct_8x8_inference_flag = false;
    let frame_cropping_flag = false;
    let vui_parameters_present_flag = false;

    write_bits(&mut w, profile_idc, 8);
    write_bits(&mut w, constraint_set_flags, 8);
    write_bits(&mut w, level_idc, 8);
    w.ue(seq_parameter_set_id);
    w.ue(log2_max_frame_num_minus4);
    w.ue(pic_order_cnt_type);
    if pic_order_cnt_type == 0 {
        w.ue(log2_max_pic_order_cnt_lsb_minus4);
    }
    w.ue(max_num_ref_frames);
    w.bit(gaps_in_frame_num_value_allowed_flag);
    w.ue(pic_width_in_mbs_minus1);
    w.ue(pic_height_in_map_units_minus1);
    w.bit(frame_mbs_only_flag);
    w.bit(direct_8x8_inference_flag);
    w.bit(frame_cropping_flag);
    w.bit(vui_parameters_present_flag);
    let mut nal = vec![0x67];
    nal.extend(escape(&w.finish_rbsp()));
    nal
}

#[test]
fn rewritten_sps_vui_fields_and_bytes_are_pinned() {
    let server_sps = [
        0x67, 0x64, 0x00, 0x34, 0xac, 0x1b, 0x1a, 0x80, 0x28, 0x00, 0xb5, 0x90, 0x00,
    ];
    let expected_server_restricted = [
        0x67, 0x64, 0x00, 0x34, 0xac, 0x1b, 0x1a, 0x80, 0x28, 0x00, 0xb5, 0xa0, 0x1b, 0x41, 0x10,
        0x8d, 0x40,
    ];
    let expected_vui = VuiParameters {
        aspect_ratio_info_present_flag: false,
        overscan_info_present_flag: false,
        video_signal_type_present_flag: false,
        chroma_loc_info_present_flag: false,
        timing_info_present_flag: false,
        nal_hrd_parameters_present_flag: false,
        vcl_hrd_parameters_present_flag: false,
        pic_struct_present_flag: false,
        bitstream_restriction: Some(BitstreamRestriction {
            motion_vectors_over_pic_boundaries_flag: true,
            max_bytes_per_pic_denom: 2,
            max_bits_per_mb_denom: 1,
            log2_max_mv_length_horizontal: 16,
            log2_max_mv_length_vertical: 16,
            max_num_reorder_frames: 0,
            max_dec_frame_buffering: 1,
        }),
    };
    assert_eq!(
        VuiParameters::bitstream_restriction_only(BitstreamRestriction::no_reorder(1)),
        expected_vui,
        "named VUI and bitstream_restriction defaults"
    );

    let au = [&[0, 0, 0, 1][..], &server_sps].concat();
    let rewritten_au = without_reorder_delay(&au).expect("server SPS needs VUI");
    let rewritten_nals = nal_units(&rewritten_au);
    assert_eq!(
        rewritten_nals.as_slice(),
        &[&expected_server_restricted[..]],
        "rewritten server SPS bytes must remain byte-for-byte identical"
    );
    let decoded = decode_vui_after_sps(rewritten_nals[0]);
    assert_eq!(
        decoded,
        DecodedVuiTail {
            vui_parameters_present_flag: true,
            vui: expected_vui,
            rbsp_stop_one_bit: true,
            rbsp_alignment_zero_bits: 6,
        },
        "decoded VUI syntax elements for server SPS"
    );
}

#[test]
fn rewritten_sps_vui_pins_ref_frame_clamping_and_stop_bit_alignments() {
    struct RewriteCase {
        label: &'static str,
        sps: Vec<u8>,
        expected_bytes: &'static [u8],
        expected_buffering: u32,
        expected_align_bits: usize,
    }

    let cases = [
        RewriteCase {
            label: "zero refs clamps max_dec_frame_buffering to 1",
            sps: build_baseline_sps(0, 0, 39, 22),
            expected_bytes: &[
                0x67, 0x42, 0xc0, 0x1f, 0xf8, 0x14, 0x05, 0xe4, 0x03, 0x68, 0x22, 0x11, 0xa8,
            ],
            expected_buffering: 1,
            expected_align_bits: 3,
        },
        RewriteCase {
            label: "four refs preserves max_dec_frame_buffering = 4",
            sps: build_baseline_sps(0, 4, 39, 22),
            expected_bytes: &[
                0x67, 0x42, 0xc0, 0x1f, 0xf2, 0x81, 0x40, 0x5e, 0x40, 0x36, 0x82, 0x21, 0x19, 0x60,
            ],
            expected_buffering: 4,
            expected_align_bits: 5,
        },
        RewriteCase {
            label: "byte-aligned vui_parameters_present_flag leaves zero alignment bits",
            sps: build_baseline_sps(2, 1, 127, 63),
            expected_bytes: &[
                0x67, 0x42, 0xc0, 0x1f, 0xda, 0x00, 0x80, 0x02, 0x04, 0x80, 0x6d, 0x04, 0x42, 0x35,
            ],
            expected_buffering: 1,
            expected_align_bits: 0,
        },
        RewriteCase {
            label: "bit-offset 1 starts a new byte for rbsp_stop_one_bit",
            sps: build_baseline_sps(0, 1, 7, 7),
            expected_bytes: &[
                0x67, 0x42, 0xc0, 0x1f, 0xf4, 0x10, 0x22, 0x40, 0x36, 0x82, 0x21, 0x1a, 0x80,
            ],
            expected_buffering: 1,
            expected_align_bits: 7,
        },
    ];

    for case in cases {
        let au = [&[0, 0, 0, 1][..], case.sps.as_slice()].concat();
        let rewritten_au = without_reorder_delay(&au).expect("SPS without VUI must be rewritten");
        let rewritten_nal = nal_units(&rewritten_au)[0];
        assert_eq!(
            rewritten_nal, case.expected_bytes,
            "{}: rewritten SPS bytes must remain byte-for-byte identical",
            case.label
        );
        let decoded = decode_vui_after_sps(rewritten_nal);
        assert_eq!(
            decoded,
            DecodedVuiTail {
                vui_parameters_present_flag: true,
                vui: VuiParameters {
                    aspect_ratio_info_present_flag: false,
                    overscan_info_present_flag: false,
                    video_signal_type_present_flag: false,
                    chroma_loc_info_present_flag: false,
                    timing_info_present_flag: false,
                    nal_hrd_parameters_present_flag: false,
                    vcl_hrd_parameters_present_flag: false,
                    pic_struct_present_flag: false,
                    bitstream_restriction: Some(BitstreamRestriction {
                        motion_vectors_over_pic_boundaries_flag: true,
                        max_bytes_per_pic_denom: 2,
                        max_bits_per_mb_denom: 1,
                        log2_max_mv_length_horizontal: 16,
                        log2_max_mv_length_vertical: 16,
                        max_num_reorder_frames: 0,
                        max_dec_frame_buffering: case.expected_buffering,
                    }),
                },
                rbsp_stop_one_bit: true,
                rbsp_alignment_zero_bits: case.expected_align_bits,
            },
            "{}: decoded VUI syntax elements",
            case.label
        );
        assert_eq!(
            parse_sps(rewritten_nal),
            parse_sps(&case.sps),
            "{}: SPS header fields must be unchanged",
            case.label
        );
    }
}
