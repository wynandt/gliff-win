//! SPS parsing against real encoder output.

use gliff_sw::Encoder;
use gliff_sw::EncoderSettings;

use super::LevelVerdict;
use super::SpsInfo;
use super::find_sps;
use super::nal_units;
use super::parse_sps;
use super::set_level_idc;

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
    // 1080 is not a multiple of 16, so this exercises frame cropping.
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
    // ffmpeg -f lavfi -i testsrc2=s=3840x2400 -frames:v 1 -c:v libx264
    // -profile:v high -level 6.0 (build machine ffmpeg).
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
