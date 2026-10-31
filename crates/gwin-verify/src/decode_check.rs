//! `gwin-verify decode`: decode time per access unit through the client's
//! own `VideoDecoder` (the CPU decoder, or the GPU decoder with
//! `--decoder gpu`), the copy to tight I420 included, and the process CPU
//! time it took. The display converts on the GPU, so BGRA conversion is
//! not in the client's per-frame CPU cost and is not timed here.

use std::time::Duration;
use std::time::Instant;

use gliff_proto::msg::ChromaMode;
use gliff_windows::decode::DecoderKind;
use gliff_windows::decode::VideoDecoder;
use gliff_windows::yuv::YuvFrame;

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
    /// The decoder that ran to the end ("cpu" or "gpu").
    pub decoder: &'static str,
    /// Process CPU time over the run, ms; `None` where it is not read.
    pub cpu_ms: Option<f64>,
    /// The decoder's own log lines (a start, a fallback).
    pub notes: Vec<String>,
}

/// How one clip is decoded.
#[derive(Debug, Clone, Copy)]
pub struct Run<'a> {
    /// The aux stream of a Dual420 pair, unit for unit with the clip.
    pub aux: Option<&'a Clip>,
    pub decoder: DecoderKind,
    /// Feed units at this rate (0: as fast as they decode), so the CPU
    /// share is the share a stream at that rate costs.
    pub pace_fps: f64,
}

impl Default for Run<'_> {
    fn default() -> Self {
        Self {
            aux: None,
            decoder: DecoderKind::Cpu,
            pace_fps: 0.0,
        }
    }
}

#[cfg(test)]
fn measure(clip: &Clip) -> anyhow::Result<Measured> {
    measure_with(clip, Run::default(), |_, _| {})
}

/// Decodes `clip` as `run` says, handing each released picture (with the
/// index of the unit that released it) to `seen`.
pub fn measure_with(
    clip: &Clip,
    run: Run<'_>,
    mut seen: impl FnMut(usize, &YuvFrame),
) -> anyhow::Result<Measured> {
    let mut decoder = VideoDecoder::new(run.decoder == DecoderKind::Gpu)?;
    let mut notes = decoder.take_notes();
    if let Some(aux) = run.aux {
        anyhow::ensure!(
            aux.units.len() == clip.units.len(),
            "aux clip has {} units, main has {}",
            aux.units.len(),
            clip.units.len()
        );
        decoder.reset(ChromaMode::Dual420)?;
    }
    let want = (clip.size.width & !1, clip.size.height & !1);
    let period = (run.pace_fps > 0.0).then(|| Duration::from_secs_f64(1.0 / run.pace_fps));
    let mut samples_ms = Vec::with_capacity(clip.units.len() + 1);
    let (mut pictures, mut errors, mut wrong_size) = (0u64, 0u64, 0u64);
    let mut first_error = None;
    let cpu_start = process_cpu_ms();
    let start = Instant::now();
    for (index, unit) in clip.units.iter().enumerate() {
        if let Some(period) = period {
            let due = start + period * index as u32;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        let aux = run.aux.map_or(&[][..], |a| &a.units[index][..]);
        let t = Instant::now();
        let result = decoder.decode(unit, aux);
        samples_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        notes.extend(decoder.take_notes());
        match result {
            Ok(Some(picture)) => {
                pictures += 1;
                if picture.size() != want {
                    wrong_size += 1;
                }
                seen(index, &picture);
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
        notes.extend(decoder.take_notes());
        match result {
            Ok(Some(picture)) => {
                pictures += 1;
                if picture.size() != want {
                    wrong_size += 1;
                }
                seen(clip.units.len(), &picture);
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
        decoder: decoder.kind().name(),
        cpu_ms: cpu_start.and_then(|a| process_cpu_ms().map(|b| b - a)),
        notes,
    })
}

/// This process's user plus kernel CPU time, ms.
#[cfg(windows)]
fn process_cpu_ms() -> Option<f64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::Win32::System::Threading::GetProcessTimes;
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    // SAFETY: four live FILETIME out-parameters for the current process.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ms = |t: FILETIME| {
        ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64 / 10_000.0
    };
    Some(ms(kernel) + ms(user))
}

#[cfg(not(windows))]
fn process_cpu_ms() -> Option<f64> {
    None
}

/// What `--decoder both` found: the GPU's pictures against the CPU's.
#[derive(Debug, Default)]
pub struct Compared {
    pub pictures: u64,
    pub differ: u64,
    /// How many units earlier the GPU released a picture than the CPU,
    /// smallest and largest.
    pub gpu_lead_units: (i64, i64),
    pub first_difference: Option<String>,
}

/// Decodes `clip` with both decoders and compares every picture byte for
/// byte: hardware H.264 decode is bit-exact, so any difference is a bug.
pub fn compare(clip: &Clip, aux: Option<&Clip>) -> anyhow::Result<(Compared, Measured, Measured)> {
    let mut cpu = Vec::new();
    let run = Run {
        aux,
        ..Run::default()
    };
    let cpu_m = measure_with(clip, run, |i, p| cpu.push((i, fingerprint(p))))?;
    let mut gpu = Vec::new();
    let run = Run {
        decoder: DecoderKind::Gpu,
        ..run
    };
    let gpu_m = measure_with(clip, run, |i, p| gpu.push((i, fingerprint(p))))?;
    let mut c = Compared {
        gpu_lead_units: (i64::MAX, i64::MIN),
        ..Compared::default()
    };
    // Pictures are compared in release order. OpenH264 holds a High-profile
    // picture for one unit, while the low-latency hardware decoder releases
    // it at once, so the same picture can come out at different units.
    for ((ci, f), (gi, g)) in cpu.iter().zip(&gpu) {
        c.pictures += 1;
        let lead = *ci as i64 - *gi as i64;
        c.gpu_lead_units = (c.gpu_lead_units.0.min(lead), c.gpu_lead_units.1.max(lead));
        if g != f {
            c.differ += 1;
            c.first_difference
                .get_or_insert_with(|| format!("picture {}: {}", c.pictures - 1, f.diff(g)));
        }
    }
    if c.pictures == 0 {
        c.gpu_lead_units = (0, 0);
    }
    Ok((c, cpu_m, gpu_m))
}

/// A picture's size and planes, kept whole for the compare.
#[derive(PartialEq, Eq)]
struct Fingerprint {
    size: (u32, u32),
    planes: [Vec<u8>; 3],
}

impl Fingerprint {
    fn diff(&self, other: &Self) -> String {
        if self.size != other.size {
            return format!("size {:?} vs {:?}", self.size, other.size);
        }
        for (name, (a, b)) in ["y", "u", "v"]
            .iter()
            .zip(self.planes.iter().zip(&other.planes))
        {
            if let Some(at) = a.iter().zip(b).position(|(x, y)| x != y) {
                let n = a.iter().zip(b).filter(|(x, y)| x != y).count();
                return format!("plane {name}: {n} bytes differ, first at {at}");
            }
            if a.len() != b.len() {
                return format!("plane {name}: {} vs {} bytes", a.len(), b.len());
            }
        }
        "equal".to_owned()
    }
}

fn fingerprint(p: &YuvFrame) -> Fingerprint {
    Fingerprint {
        size: p.size(),
        planes: [p.y().to_vec(), p.u().to_vec(), p.v().to_vec()],
    }
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

/// The result lines for one clip (and its Dual420 aux): one per decoder
/// run, and with `Both` the compare.
pub fn lines(
    clip: &Clip,
    aux: Option<&Clip>,
    choice: crate::DecoderChoice,
    pace_fps: f64,
    limits: Limits,
) -> Vec<Line> {
    let timed = |kind: DecoderKind| -> Line {
        let run = Run {
            aux,
            decoder: kind,
            pace_fps,
        };
        match measure_with(clip, run, |_, _| {}) {
            Ok(m) => {
                let mut line = judge(clip, &m, limits)
                    .with("dual420", aux.is_some())
                    .with("pace_fps", pace_fps);
                if kind == DecoderKind::Gpu && m.decoder != "gpu" {
                    line.ok = false;
                }
                if !m.notes.is_empty() {
                    line = line.with("decoder_notes", m.notes.join(" | "));
                }
                line
            }
            Err(e) => Line::new("decode", false).with("error", format!("{e:#}")),
        }
    };
    match choice {
        crate::DecoderChoice::Cpu => vec![timed(DecoderKind::Cpu)],
        crate::DecoderChoice::Gpu => vec![timed(DecoderKind::Gpu)],
        crate::DecoderChoice::Both => {
            let mut out = vec![timed(DecoderKind::Cpu), timed(DecoderKind::Gpu)];
            out.push(match compare(clip, aux) {
                Ok((c, cpu, gpu)) => {
                    let ok = c.differ == 0
                        && c.pictures == cpu.pictures
                        && c.pictures == gpu.pictures
                        && cpu.errors == 0
                        && gpu.errors == 0
                        && gpu.decoder == "gpu"
                        && c.pictures > 0;
                    let line = Line::new("decode-compare", ok)
                        .with("size", clip.size.to_string())
                        .with("source", clip.source.clone())
                        .with("dual420", aux.is_some())
                        .with("pictures_compared", c.pictures)
                        .with("pictures_differ", c.differ)
                        .with("gpu_lead_units_min", c.gpu_lead_units.0)
                        .with("gpu_lead_units_max", c.gpu_lead_units.1)
                        .with("cpu_pictures", cpu.pictures)
                        .with("gpu_pictures", gpu.pictures)
                        .with("gpu_decoder", gpu.decoder);
                    match c.first_difference {
                        Some(d) => line.with("first_difference", d),
                        None => line,
                    }
                }
                Err(e) => Line::new("decode-compare", false).with("error", format!("{e:#}")),
            });
            out
        }
    }
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
        .with("limit_fps", limits.min_fps)
        .with("decoder", m.decoder);
    let line = match m.cpu_ms {
        Some(cpu) if m.wall_ms > 0.0 => line
            .with("cpu_pct", round2(cpu * 100.0 / m.wall_ms))
            .with("cpu_ms_per_frame", round2(cpu / frames.max(1) as f64)),
        _ => line,
    };
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
            decoder: "cpu",
            cpu_ms: None,
            notes: Vec::new(),
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
