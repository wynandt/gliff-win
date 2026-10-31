//! The driving half of `gwin-verify present`: launch the client, act on
//! its window like a user, and collect what [`judge`] needs. Generic over
//! [`Harness`] so the build machine runs it against a fake; Windows uses
//! `win32::Win32Harness`.
//!
//! [`judge`]: super::judge::judge

use std::time::Duration;

use gliff_sw::BgraFrame;
use gliff_windows::stats_out::ClientStats;

use super::judge::Capture;
use super::judge::ClientExit;
use super::judge::Expect;
use super::judge::Observed;
use super::judge::judge;
use crate::line::Line;
use crate::pattern;

/// What the run does to the window, and what it reads back.
pub trait Harness {
    /// Starts the client.
    fn launch(&mut self) -> anyhow::Result<()>;
    /// True once the client's window exists (and the harness holds it).
    fn find_window(&mut self) -> bool;
    /// The client's latest stats file, if it has written one.
    fn stats(&mut self) -> Option<ClientStats>;
    /// Moves the window onto its monitor with this client area (pixels).
    fn set_client_size(&mut self, size: (u32, u32)) -> anyhow::Result<()>;
    /// Presses F11 in the window.
    fn press_f11(&mut self) -> anyhow::Result<()>;
    /// The window rect on the screen, in pixels.
    fn window_rect(&mut self) -> anyhow::Result<(i32, i32, u32, u32)>;
    /// The window's area, read back from the screen.
    fn capture(&mut self) -> anyhow::Result<BgraFrame>;
    /// Asks the window to close (`WM_CLOSE`).
    fn close(&mut self) -> anyhow::Result<()>;
    /// Waits up to `timeout` for the client to exit; kills it after.
    fn wait_exit(&mut self, timeout: Duration) -> ClientExit;
    fn sleep(&mut self, d: Duration);
    /// Whether the remote machine's pattern window covers the session's output,
    /// the source of the damage every measured frame needs.
    fn damage_source(&mut self) -> DamageSource;
}

/// What [`Harness::damage_source`] found.
#[derive(Debug, Clone, PartialEq)]
pub enum DamageSource {
    /// The pattern window covers the output: what hyprctl showed.
    Covers(String),
    /// It does not, or the probe failed: why.
    Missing(String),
    /// `--placement skip`: not checked.
    Unchecked,
}

/// Timings and outputs of one run.
#[derive(Debug, Clone)]
pub struct Plan {
    pub expect: Expect,
    /// Each measured stage.
    pub seconds: Duration,
    /// After a resize landed, before measuring (the client's own
    /// `MEASURE_AFTER` skips the same).
    pub settle: Duration,
    pub first_picture_timeout: Duration,
    /// How long a resize or the fullscreen switch may take to show.
    pub resize_timeout: Duration,
    pub exit_timeout: Duration,
    pub poll: Duration,
    pub tolerance: f64,
    /// Where the fullscreen screenshot goes.
    pub png: std::path::PathBuf,
}

impl Plan {
    pub fn panel(seconds: u64, png: std::path::PathBuf, tolerance: f64) -> Self {
        Self {
            expect: Expect::panel(seconds),
            seconds: Duration::from_secs(seconds),
            settle: Duration::from_secs(5),
            first_picture_timeout: Duration::from_secs(15),
            resize_timeout: Duration::from_secs(10),
            exit_timeout: Duration::from_secs(5),
            poll: Duration::from_millis(250),
            tolerance,
            png,
        }
    }
}

/// A run that stopped before judging: the step and why.
fn stopped(reason: &str, error: impl std::fmt::Display) -> Line {
    Line::new("present", false)
        .with("reasons", vec![reason.to_owned()])
        .with("error", error.to_string())
}

/// Runs the whole §6 sequence and judges it.
pub fn run(h: &mut impl Harness, plan: &Plan) -> Line {
    if let Err(e) = h.launch() {
        return stopped("launch", format!("{e:#}"));
    }
    let first = poll_until(h, plan, plan.first_picture_timeout, |h| {
        h.find_window() && h.stats().is_some_and(|s| s.present.shown > 0)
    });
    if !first {
        let exit = h.wait_exit(Duration::ZERO);
        return stopped(
            "no_first_picture",
            format!(
                "no window with a shown picture within {:?} (client {exit:?})",
                plan.first_picture_timeout
            ),
        );
    }

    let windowed = plan.expect.windowed;
    if let Err(e) = h.set_client_size(windowed) {
        return abort(h, plan, "set_client_size", e);
    }
    // The judge decides whether 3 s was met; here only "it showed".
    poll_until(h, plan, plan.resize_timeout, |h| {
        h.stats().is_some_and(|s| shown_at(&s, windowed))
    });
    h.sleep(plan.settle);
    let windowed_source = match source(h, Stage::Windowed) {
        Ok(seen) => seen,
        Err(why) => return abort(h, plan, "no_damage_source", why),
    };
    h.sleep(plan.seconds);

    if let Err(e) = h.press_f11() {
        return abort(h, plan, "press_f11", e);
    }
    let panel = plan.expect.panel;
    poll_until(h, plan, plan.resize_timeout, |h| {
        h.stats()
            .is_some_and(|s| s.window.fullscreen && shown_at(&s, panel))
    });
    h.sleep(plan.settle);
    let fullscreen_source = match source(h, Stage::Fullscreen) {
        Ok(seen) => seen,
        Err(why) => return abort(h, plan, "no_damage_source", why),
    };
    h.sleep(plan.seconds);
    let Some(fullscreen) = h.stats() else {
        return abort(h, plan, "stats", "no stats file while fullscreen");
    };
    let window_rect = match h.window_rect() {
        Ok(r) => r,
        Err(e) => return abort(h, plan, "window_rect", e),
    };
    let capture = match h.capture() {
        Ok(frame) => {
            if let Err(e) = crate::png_out::write_png(&plan.png, &frame) {
                eprintln!("present: writing {} failed: {e:#}", plan.png.display());
            }
            if all_black(&frame) {
                Capture::Black
            } else {
                Capture::Scored(pattern::compare(&frame, plan.tolerance))
            }
        }
        Err(e) => Capture::Failed(format!("{e:#}")),
    };

    let exit = match h.close() {
        Ok(()) => h.wait_exit(plan.exit_timeout),
        Err(_) => h.wait_exit(Duration::ZERO),
    };
    let last = h.stats().filter(|s| s.complete);
    judge(
        &Observed {
            fullscreen,
            last,
            window_rect,
            capture,
            exit,
        },
        &plan.expect,
    )
    .with("png", plan.png.display().to_string())
    .with(
        "damage_source",
        serde_json::json!({"windowed": windowed_source, "fullscreen": fullscreen_source}),
    )
}

/// A measured stage, for messages.
#[derive(Debug, Clone, Copy)]
enum Stage {
    Windowed,
    Fullscreen,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Windowed => "windowed",
            Self::Fullscreen => "fullscreen",
        }
    }
}

/// Probes the damage source after a stage settled, before it is measured:
/// what was seen, or why it stops the run. Presented fps and the stall gap
/// measure the source as much as the client (gliff sends frames on
/// damage), so a stage without the moving pattern would fail them for the
/// setup.
fn source(h: &mut impl Harness, stage: Stage) -> Result<String, String> {
    match h.damage_source() {
        DamageSource::Covers(seen) => Ok(seen),
        DamageSource::Unchecked => Ok("unchecked".to_owned()),
        DamageSource::Missing(why) => Err(format!(
            "{} stage: {why}; run scripts/remote/gwin-pattern follow on the remote machine first",
            stage.name()
        )),
    }
}

/// A step failed mid-run: close the client and report the step.
fn abort(h: &mut impl Harness, plan: &Plan, step: &str, error: impl std::fmt::Display) -> Line {
    let exit = match h.close() {
        Ok(()) => h.wait_exit(plan.exit_timeout),
        Err(_) => h.wait_exit(Duration::ZERO),
    };
    stopped(step, format!("{error} (client {exit:?})"))
}

/// A picture at `size` was shown after a Resize for it.
fn shown_at(stats: &ClientStats, size: (u32, u32)) -> bool {
    stats
        .resizes
        .iter()
        .any(|r| r.size() == size && r.first_picture_ms.is_some())
}

/// Polls `done` every `plan.poll` until it holds or `timeout` passes.
fn poll_until<H: Harness>(
    h: &mut H,
    plan: &Plan,
    timeout: Duration,
    mut done: impl FnMut(&mut H) -> bool,
) -> bool {
    let mut waited = Duration::ZERO;
    loop {
        if done(h) {
            return true;
        }
        if waited >= timeout {
            return false;
        }
        h.sleep(plan.poll);
        waited += plan.poll;
    }
}

#[cfg(test)]
mod tests;

/// Every pixel's colour is zero (alpha is ignored).
fn all_black(frame: &BgraFrame) -> bool {
    let (pixels, _) = frame.pixels.as_chunks::<4>();
    pixels
        .iter()
        .all(|[b, g, r, _]| *b == 0 && *g == 0 && *r == 0)
}
