use std::time::Duration;

use gliff_sw::BgraFrame;
use gliff_windows::stats_out::ClientStats;
use gliff_windows::stats_out::ResizeRecord;

use super::DamageSource;
use super::Harness;
use super::Plan;
use super::run;
use crate::pattern::Content;
use crate::present_check::fixtures::PANEL;
use crate::present_check::fixtures::good_stats;
use crate::present_check::judge::ClientExit;

/// How the fake client behaves.
#[derive(Clone, Copy, PartialEq)]
enum Behaviour {
    Good,
    /// Never opens a window.
    NoWindow,
    /// Ignores `WM_CLOSE`.
    HangsOnClose,
    /// The screen read-back fails.
    CaptureFails,
    /// The read-back works but sees only black (GDI missing a flip-model
    /// scanout).
    CaptureBlack,
    /// No pattern window on the output (the remote machine
    /// script never placed it).
    NoDamageSource,
    /// The pattern window is there windowed but gone by fullscreen.
    DamageSourceLost,
    /// `--placement skip`.
    Unprobed,
}

/// A client on a fake clock: the window shows a picture 1 s after
/// launch, a resize shows 600 ms after the call, and the final stats file
/// is [`good_stats`].
struct Fake {
    behaviour: Behaviour,
    now: Duration,
    calls: Vec<String>,
    launched_at: Option<Duration>,
    resized_at: Option<((u32, u32), Duration)>,
    fullscreen_at: Option<Duration>,
    closed: bool,
}

impl Fake {
    fn new(behaviour: Behaviour) -> Self {
        Self {
            behaviour,
            now: Duration::ZERO,
            calls: Vec::new(),
            launched_at: None,
            resized_at: None,
            fullscreen_at: None,
            closed: false,
        }
    }

    fn shown(&self, since: Option<Duration>, after: Duration) -> bool {
        since.is_some_and(|t| self.now >= t + after)
    }
}

const SHOW_AFTER: Duration = Duration::from_millis(600);

impl Harness for Fake {
    fn launch(&mut self) -> anyhow::Result<()> {
        self.calls.push("launch".to_owned());
        self.launched_at = Some(self.now);
        Ok(())
    }

    fn find_window(&mut self) -> bool {
        self.behaviour != Behaviour::NoWindow && self.launched_at.is_some()
    }

    fn stats(&mut self) -> Option<ClientStats> {
        if self.behaviour == Behaviour::NoWindow
            || !self.shown(self.launched_at, Duration::from_secs(1))
        {
            return None;
        }
        if self.closed && self.behaviour != Behaviour::HangsOnClose {
            return Some(good_stats());
        }
        let mut stats = good_stats();
        stats.complete = false;
        stats.end = String::new();
        stats.ssh_exit = None;
        stats.window.fullscreen = self.fullscreen_at.is_some();
        stats.resizes.clear();
        if let Some((size, at)) = self.resized_at
            && self.shown(Some(at), SHOW_AFTER)
        {
            stats.resizes.push(ResizeRecord {
                width: size.0,
                height: size.1,
                scale: 2.0,
                sent_ms: 0,
                applied_ms: Some(300),
                first_picture_ms: Some(600),
            });
        }
        if self.shown(self.fullscreen_at, SHOW_AFTER) {
            stats.resizes.push(ResizeRecord {
                width: 3840,
                height: 2400,
                scale: 2.0,
                sent_ms: 0,
                applied_ms: Some(300),
                first_picture_ms: Some(600),
            });
        }
        Some(stats)
    }

    fn set_client_size(&mut self, size: (u32, u32)) -> anyhow::Result<()> {
        self.calls
            .push(format!("set_client_size {}x{}", size.0, size.1));
        self.resized_at = Some((size, self.now));
        Ok(())
    }

    fn press_f11(&mut self) -> anyhow::Result<()> {
        self.calls.push("press_f11".to_owned());
        self.fullscreen_at = Some(self.now);
        Ok(())
    }

    fn window_rect(&mut self) -> anyhow::Result<(i32, i32, u32, u32)> {
        self.calls.push("window_rect".to_owned());
        Ok(PANEL)
    }

    fn capture(&mut self) -> anyhow::Result<BgraFrame> {
        self.calls.push("capture".to_owned());
        if self.behaviour == Behaviour::CaptureFails {
            anyhow::bail!("BitBlt failed");
        }
        let (width, height) = (384, 240);
        let mut pixels = Vec::new();
        crate::pattern::render(width, height, 7, Content::Pattern, &mut pixels);
        if self.behaviour == Behaviour::CaptureBlack {
            let (black, _) = pixels.as_chunks_mut::<4>();
            black.iter_mut().for_each(|px| *px = [0, 0, 0, 255]);
        }
        Ok(BgraFrame {
            width,
            height,
            pixels,
        })
    }

    fn close(&mut self) -> anyhow::Result<()> {
        self.calls.push("close".to_owned());
        self.closed = true;
        Ok(())
    }

    fn wait_exit(&mut self, timeout: Duration) -> ClientExit {
        self.calls.push(format!("wait_exit {}s", timeout.as_secs()));
        if self.closed && self.behaviour != Behaviour::HangsOnClose {
            ClientExit::Exited(Some(0))
        } else {
            self.now += timeout;
            ClientExit::Killed
        }
    }

    fn sleep(&mut self, d: Duration) {
        self.now += d;
    }

    fn damage_source(&mut self) -> DamageSource {
        self.calls.push("damage_source".to_owned());
        let covers = || {
            DamageSource::Covers(
                "gwin-pattern covers gliff-1 (monitor 1) at (2560, 0) size (1280, 800) fullscreen=2"
                    .to_owned(),
            )
        };
        let missing = || {
            DamageSource::Missing(
                "no gwin-pattern window; want one on gliff-1 (monitor 1) at (2560, 0) size (1280, 800)"
                    .to_owned(),
            )
        };
        match self.behaviour {
            Behaviour::NoDamageSource => missing(),
            Behaviour::DamageSourceLost if self.fullscreen_at.is_some() => missing(),
            Behaviour::Unprobed => DamageSource::Unchecked,
            _ => covers(),
        }
    }
}

/// A plan whose screenshot is removed when the test ends.
struct TestPlan(Plan);

impl Drop for TestPlan {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0.png);
    }
}

fn plan(name: &str) -> TestPlan {
    TestPlan(Plan::panel(
        20,
        std::env::temp_dir().join(format!("gwv-present-{}-{name}.png", std::process::id())),
        crate::pattern::DEFAULT_TOLERANCE,
    ))
}

fn reasons(line: &crate::line::Line) -> Vec<String> {
    line.metrics["reasons"]
        .as_array()
        .expect("reasons is an array")
        .iter()
        .map(|v| v.as_str().expect("reasons are strings").to_owned())
        .collect()
}

#[test]
fn a_good_client_passes_in_the_section_6_order() {
    let mut fake = Fake::new(Behaviour::Good);
    let plan = plan("good");
    let line = run(&mut fake, &plan.0);
    assert!(line.ok, "a good client must pass: {}", line.to_json());
    assert_eq!(
        fake.calls,
        [
            "launch",
            "set_client_size 2560x1600",
            "damage_source",
            "press_f11",
            "damage_source",
            "window_rect",
            "capture",
            "close",
            "wait_exit 5s",
        ],
        "the steps of a present run"
    );
    // 1 s to the first picture, then per stage the resize poll (600 ms,
    // in 250 ms steps), the 5 s settle and the 20 s measurement.
    assert_eq!(
        fake.now,
        Duration::from_millis(1000 + 2 * (750 + 25_000)),
        "fake time"
    );
    assert!(plan.0.png.exists(), "the screenshot is written");
    for stage in ["windowed", "fullscreen"] {
        let seen = line.metrics["damage_source"][stage]
            .as_str()
            .unwrap_or_default();
        assert!(
            seen.starts_with("gwin-pattern covers gliff-1"),
            "{stage} damage source recorded: {}",
            line.to_json()
        );
    }
}

/// A real failure: no pattern window on the output, so a static
/// desktop gave the damage-driven server almost nothing to send and the
/// stall and pattern gates failed for the setup. The run now stops before
/// measuring and says so.
#[test]
fn no_damage_source_stops_before_measuring() {
    let mut fake = Fake::new(Behaviour::NoDamageSource);
    let line = run(&mut fake, &plan("no-damage").0);
    assert_eq!(reasons(&line), ["no_damage_source"], "{}", line.to_json());
    let error = line.metrics["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("windowed") && error.contains("no gwin-pattern window"),
        "names the stage and what hyprctl showed: {error}"
    );
    assert_eq!(
        fake.calls,
        [
            "launch",
            "set_client_size 2560x1600",
            "damage_source",
            "close",
            "wait_exit 5s"
        ],
        "probed after the settle, then closed without measuring"
    );
    assert_eq!(
        fake.now,
        Duration::from_millis(1000 + 750 + 5000),
        "first picture, resize poll and settle only"
    );
}

#[test]
fn a_damage_source_lost_by_fullscreen_stops_there() {
    let mut fake = Fake::new(Behaviour::DamageSourceLost);
    let line = run(&mut fake, &plan("lost").0);
    assert_eq!(reasons(&line), ["no_damage_source"], "{}", line.to_json());
    let error = line.metrics["error"].as_str().unwrap_or_default();
    assert!(error.contains("fullscreen"), "names the stage: {error}");
    assert!(
        !fake.calls.iter().any(|c| c == "capture"),
        "no capture without a source: {:?}",
        fake.calls
    );
}

#[test]
fn an_unprobed_run_is_judged_and_says_so() {
    let mut fake = Fake::new(Behaviour::Unprobed);
    let line = run(&mut fake, &plan("unprobed").0);
    assert!(line.ok, "skip only skips the probe: {}", line.to_json());
    assert_eq!(
        line.metrics["damage_source"]["windowed"],
        "unchecked",
        "{}",
        line.to_json()
    );
}

#[test]
fn no_window_fails_after_fifteen_seconds() {
    let mut fake = Fake::new(Behaviour::NoWindow);
    let line = run(&mut fake, &plan("no-window").0);
    assert_eq!(reasons(&line), ["no_first_picture"], "{}", line.to_json());
    assert_eq!(fake.now, Duration::from_secs(15), "gave up at the timeout");
    assert_eq!(
        fake.calls,
        ["launch", "wait_exit 0s"],
        "and killed the client"
    );
}

#[test]
fn a_client_that_ignores_close_is_killed_and_fails() {
    let mut fake = Fake::new(Behaviour::HangsOnClose);
    let line = run(&mut fake, &plan("hang").0);
    assert_eq!(reasons(&line), ["exit_after_bye"], "{}", line.to_json());
    assert_eq!(line.metrics["exit"]["killed"], true, "killed");
}

#[test]
fn a_failed_capture_fails_pattern_and_still_closes() {
    let mut fake = Fake::new(Behaviour::CaptureFails);
    let line = run(&mut fake, &plan("capture").0);
    assert_eq!(reasons(&line), ["pattern"], "{}", line.to_json());
    assert_eq!(
        fake.calls.last().map(String::as_str),
        Some("wait_exit 5s"),
        "the client was still closed"
    );
}

#[test]
fn an_all_black_capture_is_named_apart_from_a_wrong_pattern() {
    // GDI may not see an independent-flip
    // scanout. The gate must say so, not report a colour failure.
    let mut fake = Fake::new(Behaviour::CaptureBlack);
    let line = run(&mut fake, &plan("black").0);
    assert_eq!(reasons(&line), ["capture_black"], "{}", line.to_json());
}
