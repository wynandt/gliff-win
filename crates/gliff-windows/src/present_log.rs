//! What the UI thread presented, per stage of the session, for
//! `--stats-out` and the `gwin-verify present` gate.
//!
//! A stage (windowed, resizing, fullscreen) starts when the window enters
//! it. Its first [`MEASURE_AFTER`] are settling time; the rest is the
//! measured window the gate judges. "Decoded" for a stage is what reached
//! the UI: pictures presented plus pictures the frame slot replaced
//! (decoded and acked, never shown).

use std::time::Duration;
use std::time::Instant;

use crate::session::Histogram;

/// Settling time at the start of each stage, outside the measured window.
pub const MEASURE_AFTER: Duration = Duration::from_secs(5);

/// Where the window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Before the first picture is shown.
    Starting,
    Windowed,
    /// From a client-area change until a picture of the requested size is
    /// shown (or no Resize was needed).
    Resizing,
    Fullscreen,
    /// From `WM_CLOSE` (or the end of the session) on: teardown, never
    /// judged.
    Closing,
}

/// What one `Present` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentOutcome {
    Shown,
    /// `DXGI_STATUS_OCCLUDED`: the window is hidden; nothing was shown.
    Occluded,
    /// `DXGI_ERROR_DEVICE_REMOVED` or `_RESET`: the presenter recreates.
    DeviceLost,
}

#[derive(Debug, Clone)]
struct Segment {
    stage: Stage,
    start: Instant,
    end: Option<Instant>,
    presented: u64,
    measured: Measured,
}

#[derive(Debug, Clone, Default)]
struct Measured {
    presented: u64,
    occluded: u64,
    device_lost: u64,
    /// The slot's drop counter at the first record in the window.
    dropped_base: Option<u64>,
    dropped_last: u64,
    latency: Histogram,
    last_shown: Option<Instant>,
    max_gap: Duration,
}

/// The log. Pure: the caller passes the clock and the slot's drop counter.
#[derive(Debug, Clone)]
pub struct PresentLog {
    origin: Instant,
    segments: Vec<Segment>,
    shown: u64,
    occluded: u64,
    device_lost: u64,
}

impl PresentLog {
    /// A log in [`Stage::Starting`] from `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            origin: now,
            segments: vec![Segment::new(Stage::Starting, now)],
            shown: 0,
            occluded: 0,
            device_lost: 0,
        }
    }

    /// The current stage.
    pub fn current(&self) -> Stage {
        self.segments
            .last()
            .expect("the log always has a segment")
            .stage
    }

    /// Enter `stage`; staying in the same stage is not a new segment.
    pub fn stage(&mut self, stage: Stage, now: Instant) {
        if self.current() == stage {
            return;
        }
        if let Some(last) = self.segments.last_mut() {
            last.end = Some(now);
        }
        self.segments.push(Segment::new(stage, now));
    }

    /// One `Present`. `dropped` is [`FrameSlot::dropped`] at this moment.
    ///
    /// [`FrameSlot::dropped`]: crate::frame_slot::FrameSlot::dropped
    pub fn record(
        &mut self,
        decoded_at: Instant,
        presented_at: Instant,
        outcome: PresentOutcome,
        dropped: u64,
    ) {
        match outcome {
            PresentOutcome::Shown => self.shown += 1,
            PresentOutcome::Occluded => self.occluded += 1,
            PresentOutcome::DeviceLost => self.device_lost += 1,
        }
        let segment = self
            .segments
            .last_mut()
            .expect("the log always has a segment");
        if outcome == PresentOutcome::Shown {
            segment.presented += 1;
        }
        let from = segment.start + MEASURE_AFTER;
        if presented_at < from {
            return;
        }
        let m = &mut segment.measured;
        m.dropped_base.get_or_insert(dropped);
        m.dropped_last = dropped;
        match outcome {
            PresentOutcome::Shown => {
                m.presented += 1;
                let latency = presented_at.saturating_duration_since(decoded_at);
                m.latency.record(latency.as_secs_f32() * 1000.0);
                let since = m.last_shown.unwrap_or(from);
                m.max_gap = m.max_gap.max(presented_at.saturating_duration_since(since));
                m.last_shown = Some(presented_at);
            }
            PresentOutcome::Occluded => m.occluded += 1,
            PresentOutcome::DeviceLost => m.device_lost += 1,
        }
    }

    /// Pictures shown so far, in every stage.
    pub fn shown(&self) -> u64 {
        self.shown
    }

    /// Every stage up to `now`; the open one ends at `now`.
    pub fn summary(&self, now: Instant) -> PresentSummary {
        PresentSummary {
            shown: self.shown,
            occluded: self.occluded,
            device_lost: self.device_lost,
            stages: self
                .segments
                .iter()
                .map(|s| s.summary(self.origin, now))
                .collect(),
        }
    }
}

impl Segment {
    fn new(stage: Stage, start: Instant) -> Self {
        Self {
            stage,
            start,
            end: None,
            presented: 0,
            measured: Measured::default(),
        }
    }

    fn summary(&self, origin: Instant, now: Instant) -> StageSummary {
        let end = self.end.unwrap_or(now);
        let from = self.start + MEASURE_AFTER;
        let span = end.saturating_duration_since(from);
        let m = &self.measured;
        let dropped = m.dropped_last - m.dropped_base.unwrap_or(m.dropped_last);
        let decoded = m.presented + dropped;
        let per_second = |n: u64| {
            if span.is_zero() {
                0.0
            } else {
                n as f64 / span.as_secs_f64()
            }
        };
        // A freeze at the end of the window is a gap too.
        let tail = if span.is_zero() {
            Duration::ZERO
        } else {
            end.saturating_duration_since(m.last_shown.unwrap_or(from))
        };
        StageSummary {
            stage: self.stage,
            start_ms: ms(self.start.saturating_duration_since(origin)),
            end_ms: ms(end.saturating_duration_since(origin)),
            measured_ms: ms(span),
            presented_total: self.presented,
            presented: m.presented,
            decoded,
            slot_dropped: dropped,
            presented_fps: per_second(m.presented),
            decoded_fps: per_second(decoded),
            latency_p50_ms: m.latency.quantile(0.5),
            latency_p95_ms: m.latency.quantile(0.95),
            latency_max_ms: m.latency.max(),
            max_gap_ms: ms(m.max_gap.max(tail)),
            occluded: m.occluded,
            device_lost: m.device_lost,
        }
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The log as written to `--stats-out`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PresentSummary {
    pub shown: u64,
    pub occluded: u64,
    pub device_lost: u64,
    pub stages: Vec<StageSummary>,
}

/// One stage. Everything but `presented_total` and the times covers the
/// measured window only (the stage after its first [`MEASURE_AFTER`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StageSummary {
    pub stage: Stage,
    /// Since the log started.
    pub start_ms: u64,
    pub end_ms: u64,
    pub measured_ms: u64,
    /// Shown in the whole stage, settling included.
    pub presented_total: u64,
    pub presented: u64,
    /// Presented plus replaced in the slot.
    pub decoded: u64,
    pub slot_dropped: u64,
    pub presented_fps: f64,
    pub decoded_fps: f64,
    /// Decoded (`Picture::frame.decoded_at`) to `Present` returned.
    pub latency_p50_ms: f32,
    pub latency_p95_ms: f32,
    pub latency_max_ms: f32,
    /// The longest time without a shown picture, edges of the window
    /// included.
    pub max_gap_ms: u64,
    pub occluded: u64,
    pub device_lost: u64,
}

#[cfg(test)]
mod tests;
