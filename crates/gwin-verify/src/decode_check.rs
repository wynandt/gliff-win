//! `gwin-verify decode`: CPU decode time per access unit through the
//! client's own `CpuDecoder`, the copy to tight I420 included. The display
//! converts on the GPU, so BGRA conversion is not in the client's
//! per-frame CPU cost and is not timed here.

use std::time::Instant;

use gliff_windows::decode::CpuDecoder;

use crate::clip::Clip;
use crate::line::Line;
use crate::line::round2;
use crate::size::Size;

/// Per-line limits; every size is judged against them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub max_p95_ms: f64,
    pub min_fps: f64,
}

/// The decode kill rule. A line is `kill_rule: true`
/// only at [`PANEL`] and exactly these limits.
pub const KILL_RULE: Limits = Limits {
    max_p95_ms: 33.3,
    min_fps: 30.0,
};

/// The size the kill rule is about: the target machine's panel.
pub const PANEL: Size = Size {
    width: 3840,
    height: 2400,
};

/// What one clip measured.
#[derive(Debug)]
pub struct Measured {
    /// Wall time of every decode call (each access unit, then the final
    /// flush if a picture was held), in ms. Kept whole so quantiles are
    /// exact: the session's 0.5 ms histogram would move a 33.1 ms p95 to
    /// 33.5 ms, across the kill-rule line.
    pub samples_ms: Vec<f64>,
    pub pictures: u64,
    pub errors: u64,
    pub wrong_size: u64,
    /// The first decode error and the unit it came from.
    pub first_error: Option<(usize, String)>,
    pub wall_ms: f64,
}

pub fn measure(clip: &Clip) -> anyhow::Result<Measured> {
    let mut decoder = CpuDecoder::new()?;
    let mut samples_ms = Vec::with_capacity(clip.units.len() + 1);
    let (mut pictures, mut errors, mut wrong_size) = (0u64, 0u64, 0u64);
    let mut first_error = None;
    let start = Instant::now();
    for (index, unit) in clip.units.iter().enumerate() {
        let t = Instant::now();
        let result = decoder.decode(unit, &[]);
        samples_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        match result {
            Ok(Some(picture)) => {
                pictures += 1;
                if picture.size() != (clip.size.width, clip.size.height) {
                    wrong_size += 1;
                }
            }
            Ok(None) => {}
            Err(e) => {
                errors += 1;
                first_error.get_or_insert_with(|| (index, e.to_string()));
            }
        }
    }
    // A High-profile stream holds one picture until flushed; it is judged
    // like any other.
    if decoder.has_pending() {
        let t = Instant::now();
        let result = decoder.flush();
        samples_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        match result {
            Ok(Some(picture)) => {
                pictures += 1;
                if picture.size() != (clip.size.width, clip.size.height) {
                    wrong_size += 1;
                }
            }
            Ok(None) => {}
            Err(e) => {
                errors += 1;
                first_error.get_or_insert_with(|| (clip.units.len(), format!("flush: {e}")));
            }
        }
    }
    Ok(Measured {
        samples_ms,
        pictures,
        errors,
        wrong_size,
        first_error,
        wall_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Nearest-rank quantile of `samples` (the smallest value with at least
/// `q` of the samples at or below it); 0 when empty.
pub fn quantile(samples: &[f64], q: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (q.clamp(0.0, 1.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// One result line for one clip.
pub fn judge(clip: &Clip, m: &Measured, limits: Limits) -> Line {
    let frames = clip.units.len() as u64;
    let p50 = quantile(&m.samples_ms, 0.5);
    let p95 = quantile(&m.samples_ms, 0.95);
    let max = m.samples_ms.iter().copied().fold(0.0, f64::max);
    let fps = if m.wall_ms > 0.0 {
        frames as f64 * 1000.0 / m.wall_ms
    } else {
        0.0
    };
    // Every unit must have been timed, or p95 describes fewer frames.
    let ok = m.samples_ms.len() as u64 >= frames
        && m.errors == 0
        && m.wrong_size == 0
        && m.pictures == frames
        && p95 <= limits.max_p95_ms
        && fps >= limits.min_fps;
    let line = Line::new("decode", ok)
        .with("size", clip.size.to_string())
        .with("kill_rule", clip.size == PANEL && limits == KILL_RULE)
        .with("source", clip.source.clone())
        .with("frames", frames)
        .with("pictures", m.pictures)
        .with("errors", m.errors)
        .with("wrong_size", m.wrong_size)
        .with("bytes", clip.bytes())
        .with("decode_p50_ms", round2(p50))
        .with("decode_p95_ms", round2(p95))
        .with("decode_max_ms", round2(max))
        .with("decode_fps", round2(fps))
        .with("limit_p95_ms", limits.max_p95_ms)
        .with("limit_fps", limits.min_fps);
    match &m.first_error {
        Some((unit, error)) => line
            .with("first_error_unit", *unit)
            .with("first_error", error.clone()),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::KILL_RULE;
    use super::Limits;
    use super::Measured;
    use super::judge;
    use super::measure;
    use crate::clip::Clip;
    use crate::clip::synthetic;
    use crate::size::Size;

    const LIMITS: Limits = KILL_RULE;

    fn small_clip() -> Clip {
        clip_of(10)
    }

    fn clip_of(frames: u32) -> Clip {
        synthetic(
            Size {
                width: 320,
                height: 240,
            },
            frames,
            1_000_000,
        )
        .expect("encode")
    }

    #[test]
    fn every_unit_decodes_to_a_picture_of_the_clip_size() {
        let clip = small_clip();
        let m = measure(&clip).expect("measure");
        assert_eq!(m.pictures, 10);
        assert_eq!((m.errors, m.wrong_size), (0, 0));
        assert!(m.samples_ms.len() >= 10);
        assert!(judge(&clip, &m, LIMITS).ok);
    }

    #[test]
    fn slow_p95_fails_the_line() {
        let clip = small_clip();
        let m = measured(&[40.0; 10], 100.0);
        let line = judge(&clip, &m, LIMITS);
        assert!(!line.ok);
        assert_eq!(line.metrics["decode_p95_ms"], 40.0);
    }

    #[test]
    fn low_fps_fails_the_line() {
        let clip = small_clip();
        let m = measured(&[1.0; 10], 1000.0); // 10 frames in 1 s
        assert!(!judge(&clip, &m, LIMITS).ok);
    }

    #[test]
    fn a_corrupt_unit_fails_the_line() {
        let mut clip = small_clip();
        clip.units[5] = vec![0, 0, 0, 1, 0x65, 0xff, 0xff];
        let m = measure(&clip).expect("measure");
        assert!(!judge(&clip, &m, LIMITS).ok);
    }

    fn measured(samples: &[f64], wall_ms: f64) -> Measured {
        Measured {
            samples_ms: samples.to_vec(),
            pictures: samples.len().max(10) as u64,
            errors: 0,
            first_error: None,
            wrong_size: 0,
            wall_ms,
        }
    }

    #[test]
    fn p95_is_the_measured_time_not_a_bucket_edge() {
        // 19 units at 33.1 ms and one slow IDR: the 95th of 20 is 33.1.
        let clip = clip_of(20);
        let mut samples = [33.1; 20];
        samples[0] = 50.0;
        let line = judge(&clip, &measured(&samples, 100.0), LIMITS);
        assert_eq!(line.metrics["decode_p95_ms"], 33.1);
        assert!(line.ok, "33.1 ms is inside the 33.3 ms limit");
    }

    #[test]
    fn fewer_timings_than_frames_fails_the_line() {
        let clip = small_clip();
        let line = judge(&clip, &measured(&[], 100.0), LIMITS);
        assert!(!line.ok, "no timings must not pass as p95 0");
    }

    #[test]
    fn loosened_limits_are_not_the_kill_rule() {
        let mut clip = small_clip();
        clip.size = super::PANEL;
        let loose = Limits {
            max_p95_ms: 999.0,
            min_fps: 1.0,
        };
        let line = judge(&clip, &measured(&[40.0; 10], 100.0), loose);
        assert_eq!(line.metrics["kill_rule"], false);
        let line = judge(&clip, &measured(&[40.0; 10], 100.0), LIMITS);
        assert_eq!(line.metrics["kill_rule"], true);
    }

    #[test]
    fn quantile_is_nearest_rank() {
        let samples = [5.0, 1.0, 4.0, 2.0, 3.0];
        assert_eq!(super::quantile(&samples, 0.5), 3.0);
        assert_eq!(super::quantile(&samples, 0.95), 5.0);
        assert_eq!(super::quantile(&samples, 0.0), 1.0);
        assert_eq!(super::quantile(&[], 0.95), 0.0);
    }

    /// Build-machine-only parity on a High-profile clip (`scripts/make-clips`): the
    /// I420 decoder releases pictures exactly when `gliff_sw::Decoder` does,
    /// flush included, and `to_bgra` is byte-identical. Point
    /// `GWV_PARITY_CLIP` at a `.gwclip`.
    #[test]
    #[ignore = "needs a clip from scripts/make-clips"]
    fn the_i420_decoder_matches_gliff_sw_on_a_high_profile_clip() {
        let path = std::env::var("GWV_PARITY_CLIP").expect("GWV_PARITY_CLIP=path.gwclip");
        let clip = Clip::load(std::path::Path::new(&path)).expect("load clip");
        let mut ours = gliff_windows::decode::CpuDecoder::new().expect("our decoder");
        let mut theirs = gliff_sw::Decoder::new(false, clip.size.width, clip.size.height)
            .expect("gliff_sw decoder");
        let mut compared = 0;
        let mut late = 0;
        for (i, unit) in clip.units.iter().enumerate() {
            let a = ours.decode(unit, &[]).expect("our decode");
            let b = theirs.decode(unit, &[]).expect("gliff_sw decode");
            assert_eq!(a.is_some(), b.is_some(), "unit {i}: release timing");
            assert_eq!(ours.has_pending(), theirs.has_pending(), "unit {i}: held");
            if a.is_none() {
                late += 1;
            }
            if let (Some(a), Some(b)) = (a, b) {
                assert!(a.to_bgra().pixels == b.pixels, "unit {i}: BGRA differs");
                compared += 1;
            }
        }
        let a = ours.flush().expect("our flush");
        let b = theirs.flush().expect("gliff_sw flush");
        assert_eq!(a.is_some(), b.is_some(), "flush release");
        if let (Some(a), Some(b)) = (a, b) {
            assert!(a.to_bgra().pixels == b.pixels, "flushed picture differs");
            compared += 1;
        }
        println!(
            "{path}: {compared} pictures identical, {late} released late, {} units",
            clip.units.len()
        );
        assert_eq!(compared, clip.units.len(), "every unit's picture compared");
    }
}
