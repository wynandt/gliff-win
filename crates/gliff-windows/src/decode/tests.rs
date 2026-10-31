//! The I420 decoder against `gliff_sw::Decoder`, upstream's BGRA one.

use gliff_sw::Decoder;
use gliff_sw::Encoder;
use gliff_sw::EncoderSettings;

use super::CpuDecoder;

/// A moving gradient with sharp edges, so chroma siting and the crop show.
fn source(width: u32, height: u32, t: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let edge = ((x + 3 * t) / 7 + y / 5).is_multiple_of(2);
            px.extend_from_slice(&[
                (x * 255 / width) as u8,
                if edge { 230 } else { 20 },
                ((y + t) * 255 / height) as u8,
                255,
            ]);
        }
    }
    px
}

fn clip(width: u32, height: u32, frames: u32) -> Vec<Vec<u8>> {
    let mut encoder = Encoder::new(
        EncoderSettings {
            width,
            height,
            bitrate: 6_000_000,
            framerate: 30,
        },
        false,
    )
    .expect("OpenH264 encoder");
    (0..frames)
        .map(|t| {
            encoder
                .encode_bgra(&source(width, height, t), t == 0)
                .expect("encode")
                .main
        })
        .collect()
}

#[test]
fn to_bgra_matches_gliff_sw_byte_for_byte() {
    // 1278x718 is neither a multiple of 16 nor of 4 (cropping and
    // odd chroma rows in the coded size).
    for (w, h) in [(640, 400), (1278, 718)] {
        let mut ours = CpuDecoder::new().expect("our decoder");
        let mut theirs = Decoder::new(false, w, h).expect("gliff_sw decoder");
        let mut compared = 0;
        for (i, au) in clip(w, h, 12).iter().enumerate() {
            let a = ours.decode(au).expect("our decode");
            let b = theirs.decode(au, &[]).expect("gliff_sw decode");
            assert_eq!(a.is_some(), b.is_some(), "{w}x{h} unit {i}: release timing");
            assert_eq!(ours.has_pending(), theirs.has_pending(), "{w}x{h} unit {i}");
            if let (Some(a), Some(b)) = (a, b) {
                let a = a.to_bgra();
                assert_eq!((a.width, a.height), (b.width, b.height), "{w}x{h} unit {i}");
                assert!(a.pixels == b.pixels, "{w}x{h} unit {i}: BGRA differs");
                compared += 1;
            }
        }
        assert_eq!(compared, 12, "{w}x{h}: Baseline releases every picture");
    }
}

#[test]
fn planes_are_tight_and_even() {
    let mut decoder = CpuDecoder::new().expect("decoder");
    let au = &clip(1278, 718, 1)[0];
    let frame = decoder.decode(au).expect("decode").expect("a Baseline IDR");
    assert_eq!(frame.size(), (1278, 718));
    assert_eq!(frame.y().len(), 1278 * 718);
    assert_eq!(frame.u().len(), 639 * 359);
    assert_eq!(frame.v().len(), 639 * 359);
}

#[test]
fn reset_decodes_the_next_idr() {
    let units = clip(320, 240, 2);
    let mut decoder = CpuDecoder::new().expect("decoder");
    let _ = decoder.decode(&units[0]).expect("idr");
    decoder.reset().expect("reset");
    assert!(!decoder.has_pending(), "a fresh decoder holds nothing");
    let again = decoder.decode(&units[0]).expect("idr after reset");
    assert!(again.is_some(), "the IDR decodes on the fresh decoder");
}

#[test]
fn flush_with_nothing_held_returns_none() {
    let mut decoder = CpuDecoder::new().expect("decoder");
    assert!(decoder.flush().expect("flush").is_none());
    assert!(!decoder.has_pending());
}
