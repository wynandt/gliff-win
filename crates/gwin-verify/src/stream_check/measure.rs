//! The pure half of `gwin-verify stream`: what counts as Resize applied,
//! a stall, a missing frame, and the quantiles.

use std::time::Duration;

/// How far the output size implied by a StreamConfig may be from the
/// request: `scale_milli` is rounded to 1/1000, worth 2-4 px at 4K.
const IMPLIED_SLACK: u32 = 4;

/// What one StreamConfig says about the requested size (upstream sends
/// view = stream = `base_stream`, and `scale_milli` = scale x stream /
/// output, session.rs:961-975, :1494-1500).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeVerdict {
    /// The output is the requested size and the stream codes all of it.
    Applied,
    /// The output is the requested size, but the encoder scaled the
    /// stream down to fit its maximum.
    Clamped { stream: (u32, u32) },
    /// The output is the requested size, but the stream is smaller than
    /// the encoder fit: the rate ladder's scale-0.5 levels
    /// (rate.rs:345-358), a symptom that `fps` reports.
    Scaled { stream: (u32, u32) },
    /// Not (yet) the requested output.
    Other { implied_output: (u32, u32) },
}

/// The output size a StreamConfig implies, at the requested scale 1.0.
pub fn implied_output(stream: (u32, u32), scale_milli: u32) -> (u32, u32) {
    let s = f64::from(scale_milli.max(1)) / 1000.0;
    (
        (f64::from(stream.0) / s).round() as u32,
        (f64::from(stream.1) / s).round() as u32,
    )
}

/// The CPU encoder's maximum (gliff-sw lib.rs:238, enforced at :330).
const CPU_ENCODER_MAX: (u32, u32) = (3840, 2160);

/// The stream the server scales an output down to when it is over the
/// CPU encoder maximum: the same aspect, inside the maximum.
fn encoder_fit(output: (u32, u32)) -> (u32, u32) {
    let s = (f64::from(CPU_ENCODER_MAX.0) / f64::from(output.0))
        .min(f64::from(CPU_ENCODER_MAX.1) / f64::from(output.1))
        .min(1.0);
    (
        (f64::from(output.0) * s).round() as u32,
        (f64::from(output.1) * s).round() as u32,
    )
}

pub fn judge_config(requested: (u32, u32), stream: (u32, u32), scale_milli: u32) -> ResizeVerdict {
    let implied = implied_output(stream, scale_milli);
    let near = |a: (u32, u32), b: (u32, u32)| {
        a.0.abs_diff(b.0) <= IMPLIED_SLACK && a.1.abs_diff(b.1) <= IMPLIED_SLACK
    };
    if !near(implied, requested) {
        return ResizeVerdict::Other {
            implied_output: implied,
        };
    }
    if near(stream, requested) {
        ResizeVerdict::Applied
    } else if encoder_fit(requested) != requested && near(stream, encoder_fit(requested)) {
        ResizeVerdict::Clamped { stream }
    } else {
        ResizeVerdict::Scaled { stream }
    }
}

/// The share of the server's fps cap the client must decode: the server
/// commands `min(cap, what its encoder sustains)` (session.rs:951-957)
/// and paces frames to it, so a healthy run sits just under the cap.
pub const FPS_OF_CAP: f64 = 0.95;

/// Why the decoded fps fails, judged against the server's cap.
#[derive(Debug, Clone, PartialEq)]
pub enum FpsProblem {
    /// The server itself capped the stream below the floor at some point
    /// in the window (a ladder step down, rate.rs:324-358).
    CapBelowFloor { cap: u32 },
    /// The client decoded fewer than `FPS_OF_CAP` of what the cap allowed.
    BelowCap { fps: f64, expected: f64 },
}

/// The fps cap in force over the window, time-weighted: `at_start` until
/// the first step, then each StreamConfig's cap from its offset
/// (upstream sends a StreamConfig on every ladder step,
/// session.rs:525-547).
pub fn effective_cap(at_start: u32, steps: &[(Duration, u32)], window: Duration) -> f64 {
    let total = window.as_secs_f64();
    if total <= 0.0 {
        return f64::from(at_start);
    }
    let mut cap = at_start;
    let mut from = Duration::ZERO;
    let mut sum = 0.0;
    for &(at, next) in steps {
        let at = at.min(window);
        sum += f64::from(cap) * at.saturating_sub(from).as_secs_f64();
        from = from.max(at);
        cap = next;
    }
    sum += f64::from(cap) * window.saturating_sub(from).as_secs_f64();
    sum / total
}

/// Judges the decoded fps against the server's cap, and the cap against
/// `floor` (`--min-fps`); empty means pass.
pub fn judge_fps(
    fps: f64,
    at_start: u32,
    steps: &[(Duration, u32)],
    window: Duration,
    floor: f64,
    fps_of_cap: f64,
) -> Vec<FpsProblem> {
    let mut problems = Vec::new();
    let lowest = steps
        .iter()
        .filter(|(at, _)| *at < window)
        .map(|(_, cap)| *cap)
        .fold(at_start, u32::min);
    if f64::from(lowest) < floor {
        problems.push(FpsProblem::CapBelowFloor { cap: lowest });
        // Under its own low cap the fps is the server's choice; the cap
        // is the failure.
        return problems;
    }
    let expected = round2(fps_of_cap * effective_cap(at_start, steps, window));
    if fps < expected {
        problems.push(FpsProblem::BelowCap { fps, expected });
    }
    problems
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The longest gap between frames in a window of length `window`, with
/// the window's start and end counted as edges, so a stream that stops
/// for good is a gap too. `arrivals` are offsets from the window start.
pub fn max_gap(arrivals: &[Duration], window: Duration) -> Duration {
    let mut last = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for at in arrivals.iter().copied().chain(std::iter::once(window)) {
        worst = worst.max(at.saturating_sub(last));
        last = last.max(at);
    }
    worst
}

/// The number of gaps longer than `limit`, same edges as [`max_gap`].
pub fn stalls(arrivals: &[Duration], window: Duration, limit: Duration) -> usize {
    let mut last = Duration::ZERO;
    let mut count = 0;
    for at in arrivals.iter().copied().chain(std::iter::once(window)) {
        if at.saturating_sub(last) > limit {
            count += 1;
        }
        last = last.max(at);
    }
    count
}

/// Counts frame ids that never arrived: upstream numbers every frame it
/// sends from 0, +1 each (session.rs:1151-1193), so a hole is a frame
/// lost on the way.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IdGaps {
    next: Option<u64>,
    pub missing: u64,
    pub out_of_order: u64,
}

impl IdGaps {
    pub fn see(&mut self, id: u64) {
        match self.next {
            Some(next) if id > next => self.missing += id - next,
            Some(next) if id < next => self.out_of_order += 1,
            _ => {}
        }
        self.next = Some(self.next.map_or(id + 1, |n| n.max(id + 1)));
    }
}

/// Nearest-rank quantile of `samples` (sorted here); 0 when empty.
pub fn quantile(samples: &mut [f64], q: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(f64::total_cmp);
    let rank = ((q.clamp(0.0, 1.0) * samples.len() as f64).ceil() as usize).max(1);
    samples[rank - 1]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::FPS_OF_CAP;
    use super::FpsProblem;
    use super::IdGaps;
    use super::ResizeVerdict;
    use super::effective_cap;
    use super::judge_config;
    use super::judge_fps;
    use super::max_gap;
    use super::quantile;
    use super::stalls;

    fn ms(v: &[u64]) -> Vec<Duration> {
        v.iter().copied().map(Duration::from_millis).collect()
    }

    #[test]
    fn resize_is_applied_clamped_or_not_from_stream_and_scale_milli() {
        assert_eq!(
            judge_config((3840, 2400), (3840, 2400), 1000),
            ResizeVerdict::Applied,
            "real server, Vulkan"
        );
        assert_eq!(
            judge_config((3840, 2400), (3456, 2160), 900),
            ResizeVerdict::Clamped {
                stream: (3456, 2160)
            },
            "CPU encoder max (fit_extent)"
        );
        assert_eq!(
            judge_config((3840, 2400), (1920, 1080), 1000),
            ResizeVerdict::Other {
                implied_output: (1920, 1080)
            },
            "the first, clamped mode"
        );
    }

    #[test]
    fn a_full_size_stream_with_the_wrong_scale_is_not_applied() {
        assert_eq!(
            judge_config((640, 400), (640, 400), 500),
            ResizeVerdict::Other {
                implied_output: (1280, 800)
            },
            "stream equals the request but implies a 1280x800 output"
        );
    }

    #[test]
    fn a_stream_within_the_slack_is_applied() {
        assert_eq!(
            judge_config((3840, 2400), (3838, 2400), 1000),
            ResizeVerdict::Applied,
            "2 px off is rounding, not a clamp"
        );
    }

    #[test]
    fn a_ladder_scaled_stream_is_not_an_encoder_clamp() {
        // rate.rs:345-358: ladder levels 5 and 6 code at scale 0.5.
        assert_eq!(
            judge_config((1280, 800), (640, 400), 500),
            ResizeVerdict::Scaled { stream: (640, 400) },
            "a request inside the encoder maximum cannot be clamped by it"
        );
        assert_eq!(
            judge_config((3840, 2400), (1728, 1080), 450),
            ResizeVerdict::Scaled {
                stream: (1728, 1080)
            },
            "half of the encoder fit is the ladder, not the fit"
        );
    }

    #[test]
    fn a_stream_that_stops_for_good_is_a_stall() {
        let window = Duration::from_secs(30);
        let arrivals = ms(&[100, 200, 300, 20_000]);
        assert_eq!(
            max_gap(&arrivals, window),
            Duration::from_millis(19_700),
            "gap"
        );
        assert_eq!(
            stalls(&arrivals, window, Duration::from_secs(5)),
            2,
            "middle and tail"
        );
        assert_eq!(
            stalls(&[], window, Duration::from_secs(5)),
            1,
            "nothing at all"
        );
        assert_eq!(
            stalls(
                &ms(&[1000, 4000, 8000]),
                Duration::from_secs(10),
                Duration::from_secs(5)
            ),
            0,
            "gaps of 4 s pass"
        );
    }

    #[test]
    fn id_holes_are_missing_frames() {
        let mut gaps = IdGaps::default();
        for id in [0, 1, 2, 4, 5, 9, 10] {
            gaps.see(id);
        }
        assert_eq!(gaps.missing, 4, "3, 6, 7, 8");
        assert_eq!(gaps.out_of_order, 0, "in order");
        let mut late = IdGaps::default();
        for id in [7, 8, 3] {
            late.see(id);
        }
        assert_eq!((late.missing, late.out_of_order), (0, 1), "starts anywhere");
    }

    #[test]
    fn quantiles_are_nearest_rank() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        v.reverse();
        assert_eq!(quantile(&mut v, 0.5), 50.0, "p50");
        assert_eq!(quantile(&mut v, 0.95), 95.0, "p95");
        assert_eq!(quantile(&mut [], 0.5), 0.0, "empty");
    }

    #[test]
    fn a_run_at_the_servers_cap_of_30_passes() {
        // Seen against a real server: cap 30 all window, 871 pictures in 30 s.
        let window = Duration::from_secs(30);
        assert_eq!(
            judge_fps(29.03, 30, &[], window, 30.0, FPS_OF_CAP),
            Vec::<FpsProblem>::new(),
            "29.03 fps is 97% of the server's own cap of 30"
        );
    }

    #[test]
    fn a_run_well_under_the_cap_fails_with_the_expected_fps() {
        let window = Duration::from_secs(30);
        assert_eq!(
            judge_fps(50.0, 60, &[], window, 30.0, FPS_OF_CAP),
            vec![FpsProblem::BelowCap {
                fps: 50.0,
                expected: 57.0
            }],
            "cap 60 asks for 57"
        );
        assert_eq!(
            judge_fps(28.0, 30, &[], window, 30.0, FPS_OF_CAP),
            vec![FpsProblem::BelowCap {
                fps: 28.0,
                expected: 28.5
            }],
            "cap 30 asks for 28.5"
        );
    }

    #[test]
    fn a_server_cap_below_the_floor_fails_by_name() {
        let window = Duration::from_secs(30);
        assert_eq!(
            judge_fps(15.0, 15, &[], window, 30.0, FPS_OF_CAP),
            vec![FpsProblem::CapBelowFloor { cap: 15 }],
            "15 fps at a cap of 15 is the server's choice, and below 30"
        );
        assert_eq!(
            judge_fps(
                40.0,
                30,
                &[(Duration::from_secs(10), 15), (Duration::from_secs(12), 60)],
                window,
                30.0,
                FPS_OF_CAP
            ),
            vec![FpsProblem::CapBelowFloor { cap: 15 }],
            "a step below the floor inside the window fails even if it recovers"
        );
    }

    #[test]
    fn a_ladder_step_inside_the_window_is_time_weighted() {
        let window = Duration::from_secs(30);
        let steps = [(Duration::from_secs(15), 60)];
        assert!(
            (effective_cap(30, &steps, window) - 45.0).abs() < 1e-9,
            "half at 30, half at 60: {}",
            effective_cap(30, &steps, window)
        );
        assert_eq!(
            judge_fps(43.0, 30, &steps, window, 30.0, FPS_OF_CAP),
            Vec::<FpsProblem>::new(),
            "43 >= 0.95 x 45"
        );
        assert_eq!(
            judge_fps(40.0, 30, &steps, window, 30.0, FPS_OF_CAP),
            vec![FpsProblem::BelowCap {
                fps: 40.0,
                expected: 42.75
            }],
            "40 < 0.95 x 45"
        );
    }
}
