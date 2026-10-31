//! The per-stage arithmetic the gate relies on.

use std::time::Duration;
use std::time::Instant;

use super::MEASURE_AFTER;
use super::PresentLog;
use super::PresentOutcome;
use super::Stage;

fn at(t0: Instant, ms: u64) -> Instant {
    t0 + Duration::from_millis(ms)
}

/// Shows a picture every `every` ms in [from, to), each decoded 4 ms
/// earlier, with the slot's drop counter at `dropped`.
fn run(log: &mut PresentLog, t0: Instant, from: u64, to: u64, every: u64, dropped: u64) {
    let mut t = from;
    while t < to {
        log.record(
            at(t0, t.saturating_sub(4)),
            at(t0, t),
            PresentOutcome::Shown,
            dropped,
        );
        t += every;
    }
}

#[test]
fn a_stage_measures_after_its_settle() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Windowed, t0);
    // 50 fps for 25 s; the first 5 s are settling.
    run(&mut log, t0, 0, 25_000, 20, 0);
    let s = log.summary(at(t0, 25_000));
    let w = &s.stages[1];
    assert_eq!(w.stage, Stage::Windowed);
    assert_eq!(w.measured_ms, 20_000, "{w:?}");
    assert_eq!(w.presented_total, 1250, "{w:?}");
    assert_eq!(w.presented, 1000, "{w:?}");
    assert!((w.presented_fps - 50.0).abs() < 1e-9, "{w:?}");
    assert_eq!(w.latency_p95_ms, 4.0, "{w:?}");
    assert_eq!(w.max_gap_ms, 20, "{w:?}");
    assert_eq!(s.shown, 1250);
}

#[test]
fn slot_drops_count_as_decoded_not_presented() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Fullscreen, t0);
    run(&mut log, t0, 0, 5_000, 20, 7);
    // In the window the slot replaces 100 pictures.
    let mut t: u64 = 5_000;
    let mut dropped = 7;
    while t < 15_000 {
        log.record(
            at(t0, t.saturating_sub(4)),
            at(t0, t),
            PresentOutcome::Shown,
            dropped,
        );
        if (t / 20).is_multiple_of(5) {
            dropped += 1;
        }
        t += 20;
    }
    let w = &log.summary(at(t0, 15_000)).stages[1];
    assert_eq!(w.presented, 500, "{w:?}");
    assert_eq!(
        w.slot_dropped, 100,
        "drops after the first record in the window: {w:?}"
    );
    assert_eq!(w.decoded, 600, "{w:?}");
    assert!((w.presented_fps / w.decoded_fps - 500.0 / 600.0).abs() < 1e-9);
}

#[test]
fn a_freeze_is_the_max_gap_even_at_the_end() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Windowed, t0);
    run(&mut log, t0, 0, 8_000, 20, 0);
    run(&mut log, t0, 8_300, 10_000, 20, 0);
    let w = log.summary(at(t0, 10_000)).stages[1].clone();
    assert_eq!(w.max_gap_ms, 320, "8000-20 to 8300: {w:?}");
    let w = log.summary(at(t0, 11_000)).stages[1].clone();
    assert_eq!(w.max_gap_ms, 1_020, "nothing shown since 9980: {w:?}");
}

#[test]
fn a_stage_with_nothing_shown_gaps_for_its_whole_window() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Windowed, t0);
    let w = log.summary(at(t0, 7_000)).stages[1].clone();
    assert_eq!(
        (w.presented, w.max_gap_ms, w.measured_ms),
        (0, 2_000, 2_000)
    );
    assert_eq!(w.presented_fps, 0.0);
}

#[test]
fn stages_close_when_the_next_starts_and_repeats_are_ignored() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Windowed, at(t0, 1_000));
    log.stage(Stage::Windowed, at(t0, 2_000));
    log.stage(Stage::Resizing, at(t0, 9_000));
    log.stage(Stage::Fullscreen, at(t0, 9_400));
    let s = log.summary(at(t0, 20_000));
    let stages: Vec<_> = s
        .stages
        .iter()
        .map(|s| (s.stage, s.start_ms, s.end_ms))
        .collect();
    assert_eq!(
        stages,
        [
            (Stage::Starting, 0, 1_000),
            (Stage::Windowed, 1_000, 9_000),
            (Stage::Resizing, 9_000, 9_400),
            (Stage::Fullscreen, 9_400, 20_000),
        ]
    );
    assert_eq!(s.stages[2].measured_ms, 0, "shorter than the settle");
    assert_eq!(log.current(), Stage::Fullscreen);
}

#[test]
fn occluded_and_lost_presents_are_counted_not_shown() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Windowed, t0);
    let late = MEASURE_AFTER.as_millis() as u64 + 100;
    log.record(at(t0, late - 4), at(t0, late), PresentOutcome::Occluded, 0);
    log.record(
        at(t0, late),
        at(t0, late + 10),
        PresentOutcome::DeviceLost,
        0,
    );
    log.record(at(t0, 1), at(t0, 2), PresentOutcome::Occluded, 0);
    let s = log.summary(at(t0, late + 20));
    assert_eq!((s.shown, s.occluded, s.device_lost), (0, 2, 1));
    let w = &s.stages[1];
    assert_eq!((w.presented, w.occluded, w.device_lost), (0, 1, 1), "{w:?}");
}

#[test]
fn the_summary_round_trips_through_json() {
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Windowed, t0);
    run(&mut log, t0, 0, 6_000, 20, 0);
    let s = log.summary(at(t0, 6_000));
    let json = serde_json::to_string(&s).expect("serialise");
    assert!(json.contains("\"stage\":\"windowed\""), "{json}");
    let back: super::PresentSummary = serde_json::from_str(&json).expect("parse");
    assert_eq!(back, s);
}

#[test]
fn closing_freezes_the_fullscreen_window_at_the_close() {
    // Regression: the exit stats file is written after
    // Bye and the ssh wait, so an open fullscreen stage used to run on to
    // that moment and fail `stall_fullscreen` with a healthy stream.
    let t0 = Instant::now();
    let mut log = PresentLog::new(t0);
    log.stage(Stage::Fullscreen, t0);
    run(&mut log, t0, 0, 25_000, 20, 0);
    log.stage(Stage::Closing, at(t0, 25_000));
    // Teardown: no pictures for 4 s, then the final file.
    let s = log.summary(at(t0, 29_000));
    let fs = &s.stages[1];
    assert_eq!(fs.stage, Stage::Fullscreen, "{s:?}");
    assert_eq!(fs.end_ms, 25_000, "the stage ends at the close: {fs:?}");
    assert_eq!(fs.measured_ms, 20_000, "{fs:?}");
    assert_eq!(fs.max_gap_ms, 20, "teardown is not a gap: {fs:?}");
    assert!((fs.presented_fps - 50.0).abs() < 1e-9, "{fs:?}");
    assert_eq!(s.stages[2].stage, Stage::Closing, "{s:?}");
}
