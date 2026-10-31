//! The judging half of `gwin-verify present`: the client's
//! stats files plus what the harness saw, to one JSON line. Pure, so the
//! build machine tests it on recorded stats.

use gliff_windows::present_log::Stage;
use gliff_windows::present_log::StageSummary;
use gliff_windows::stats_out::ClientStats;
use serde_json::Value;
use serde_json::json;

use crate::line::Line;
use crate::line::round2;
use crate::pattern::Score;
use crate::pattern::Verdict;

/// The gates; `Expect::panel` is the panel run.
#[derive(Debug, Clone, PartialEq)]
pub struct Expect {
    /// The windowed client area the harness asks for.
    pub windowed: (u32, u32),
    /// The protocol scale at the panel's DPI.
    pub scale: f32,
    /// The monitor (and so fullscreen) size.
    pub panel: (u32, u32),
    pub dpi: u32,
    /// The measured window each stage must reach.
    pub min_measured_ms: u64,
    /// Sent to applied, and sent to first picture.
    pub resize_within_ms: u64,
    pub present_of_decoded: f64,
    /// Below this share of the cap, decoded fps only warns.
    pub decoded_of_cap: f64,
    pub max_latency_p95_ms: f64,
    pub max_gap_ms: u64,
}

/// The share of `--seconds` a stage's measured window must cover (the
/// harness's sleeps and the client's stage edges differ by a heartbeat).
pub const MEASURED_SHARE: f64 = 0.8;

impl Expect {
    /// The 3840x2400 panel at 200%, measuring `seconds` per stage.
    pub fn panel(seconds: u64) -> Self {
        Self {
            windowed: (2560, 1600),
            scale: 2.0,
            panel: (3840, 2400),
            dpi: 192,
            min_measured_ms: (seconds as f64 * 1000.0 * MEASURED_SHARE) as u64,
            resize_within_ms: 3000,
            present_of_decoded: 0.95,
            decoded_of_cap: 0.95,
            max_latency_p95_ms: 16.7,
            max_gap_ms: 250,
        }
    }
}

/// How the client process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientExit {
    /// Exited by itself within the wait, with this code (`None`: no code).
    Exited(Option<i32>),
    /// Still running after the wait; the harness killed it.
    Killed,
}

/// The screenshot of the fullscreen window.
#[derive(Debug, Clone, PartialEq)]
pub enum Capture {
    Scored(Score),
    /// The read-back worked but every pixel was black: GDI did not see
    /// the window (an independent-flip scanout, a locked screen), which is
    /// not a colour failure.
    Black,
    Failed(String),
}

/// Everything the run collected.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    /// The heartbeat read while fullscreen, before the close.
    pub fullscreen: ClientStats,
    /// The file the client wrote at exit (`None` if it never did).
    pub last: Option<ClientStats>,
    /// The window rect the harness read while fullscreen.
    pub window_rect: (i32, i32, u32, u32),
    pub capture: Capture,
    pub exit: ClientExit,
}

/// One `present` line: every gate, each failure a named reason.
pub fn judge(observed: &Observed, expect: &Expect) -> Line {
    let mut reasons: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    // The final file has the whole run; the fullscreen heartbeat stands in
    // if the client never wrote it.
    let stats = observed.last.as_ref().unwrap_or(&observed.fullscreen);
    let fs = &observed.fullscreen;

    let mut stage_lines = serde_json::Map::new();
    for (stage, name) in [
        (Stage::Windowed, "windowed"),
        (Stage::Fullscreen, "fullscreen"),
    ] {
        let value = match longest(stats, stage) {
            Some(s) => judge_stage(s, name, stats.fps_cap, expect, &mut reasons, &mut warnings),
            None => {
                reasons.push(format!("no_{name}_stage"));
                Value::Null
            }
        };
        stage_lines.insert(name.to_owned(), value);
    }

    let session = &stats.session;
    if session.frames != session.acks {
        reasons.push("acks".to_owned());
    }
    if session.decode_errors != 0 {
        reasons.push("decode_errors".to_owned());
    }

    let swap = &fs.swap_chain;
    if swap.swap_effect != "flip_discard" || swap.allow_tearing || swap.sync_interval != 1 {
        reasons.push("tearing".to_owned());
    }

    let monitor_size = (fs.monitor.rect.2, fs.monitor.rect.3);
    let fullscreen_ok = fs.window.fullscreen
        && fs.window.client == expect.panel
        && swap.size == expect.panel
        && monitor_size == expect.panel
        && fs.monitor.dpi == expect.dpi
        && observed.window_rect == fs.monitor.rect;
    if !fullscreen_ok {
        reasons.push("fullscreen".to_owned());
    }

    let resize = stats
        .resizes
        .iter()
        .find(|r| r.size() == expect.windowed && (r.scale - expect.scale).abs() < 0.01);
    let resize_value = match resize {
        Some(r) => {
            let applied = r.applied_ms.map(|t| t.saturating_sub(r.sent_ms));
            let picture = r.first_picture_ms.map(|t| t.saturating_sub(r.sent_ms));
            let within = |d: Option<u64>| d.is_some_and(|d| d <= expect.resize_within_ms);
            if !(within(applied) && within(picture)) {
                reasons.push("resize".to_owned());
            }
            json!({
                "size": format!("{}x{}", r.width, r.height),
                "scale": r.scale,
                "applied_after_ms": applied,
                "picture_after_ms": picture,
            })
        }
        None => {
            reasons.push("resize".to_owned());
            Value::Null
        }
    };

    let pattern_value = match &observed.capture {
        Capture::Scored(score) => {
            if score.verdict() != Verdict::Match {
                reasons.push("pattern".to_owned());
            }
            json!({
                "cells_matched": score.cells_matched,
                "mae_mean": round2(score.mae_mean),
                "mae_max": round2(score.mae_max),
                "worst_cell": score.worst_cell,
                "tolerance": score.tolerance,
            })
        }
        Capture::Black => {
            reasons.push("capture_black".to_owned());
            json!({ "error": "the screen read back all black" })
        }
        Capture::Failed(error) => {
            reasons.push("pattern".to_owned());
            json!({ "error": error })
        }
    };

    let (exit_code, killed) = match observed.exit {
        ClientExit::Exited(code) => (code, false),
        ClientExit::Killed => (None, true),
    };
    let last = observed.last.as_ref();
    let clean_exit = !killed
        && exit_code == Some(0)
        && last.is_some_and(|l| l.complete && l.end == "bye" && l.ssh_exit == Some(0));
    if !clean_exit {
        reasons.push("exit_after_bye".to_owned());
    }

    Line::new("present", reasons.is_empty())
        .with("reasons", reasons)
        .with("warnings", warnings)
        .with(
            "windowed",
            stage_lines.remove("windowed").unwrap_or(Value::Null),
        )
        .with(
            "fullscreen",
            stage_lines.remove("fullscreen").unwrap_or(Value::Null),
        )
        .with("resize", resize_value)
        .with(
            "fullscreen_geometry",
            json!({
                "client": size_text(fs.window.client),
                "swap_chain": size_text(swap.size),
                "monitor": size_text(monitor_size),
                "dpi": fs.monitor.dpi,
                "refresh_hz": round2(fs.monitor.refresh_hz),
                "window_rect": rect_value(observed.window_rect),
                "monitor_rect": rect_value(fs.monitor.rect),
            }),
        )
        .with(
            "swap_chain",
            json!({
                "swap_effect": swap.swap_effect,
                "buffers": swap.buffers,
                "allow_tearing": swap.allow_tearing,
                "sync_interval": swap.sync_interval,
                "max_frame_latency": swap.max_frame_latency,
            }),
        )
        .with(
            "adapter",
            json!({
                "name": fs.adapter.name,
                "vendor_id": format!("0x{:04x}", fs.adapter.vendor_id),
                "device_id": format!("0x{:04x}", fs.adapter.device_id),
                "choice": fs.adapter.choice,
                "drives_monitor": fs.adapter.drives_monitor,
            }),
        )
        .with(
            "session",
            json!({
                "frames": session.frames,
                "acks": session.acks,
                "decode_errors": session.decode_errors,
                "slot_dropped": session.slot_dropped,
                "decode_p95_ms": round2(f64::from(session.decode_p95_ms)),
                "fps_cap": stats.fps_cap,
            }),
        )
        .with("pattern", pattern_value)
        .with(
            "exit",
            json!({
                "code": exit_code,
                "killed": killed,
                "end": last.map(|l| l.end.clone()),
                "ssh_exit": last.and_then(|l| l.ssh_exit),
                "complete": last.is_some_and(|l| l.complete),
            }),
        )
}

/// The stage's longest measured segment (a stage can recur).
fn longest(stats: &ClientStats, stage: Stage) -> Option<&StageSummary> {
    stats
        .present
        .stages
        .iter()
        .filter(|s| s.stage == stage)
        .max_by_key(|s| s.measured_ms)
}

fn judge_stage(
    s: &StageSummary,
    name: &str,
    fps_cap: u32,
    expect: &Expect,
    reasons: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Value {
    if s.measured_ms < expect.min_measured_ms {
        reasons.push(format!("short_{name}"));
    }
    if s.decoded == 0 {
        reasons.push(format!("no_frames_{name}"));
    }
    if s.presented_fps < expect.present_of_decoded * s.decoded_fps {
        reasons.push(format!("present_fps_{name}"));
    }
    // Compared at the 0.01 ms the line prints: 16.7f32 widens to
    // 16.700000762, which is the budget, not over it.
    if hundredths(f64::from(s.latency_p95_ms)) > hundredths(expect.max_latency_p95_ms) {
        reasons.push(format!("latency_{name}"));
    }
    if s.max_gap_ms > expect.max_gap_ms {
        reasons.push(format!("stall_{name}"));
    }
    if s.occluded + s.device_lost > 0 {
        reasons.push(format!("present_errors_{name}"));
    }
    if s.decoded_fps < expect.decoded_of_cap * f64::from(fps_cap) {
        warnings.push(format!("decoded_below_cap_{name}"));
    }
    json!({
        "measured_ms": s.measured_ms,
        "presented": s.presented,
        "decoded": s.decoded,
        "slot_dropped": s.slot_dropped,
        "presented_fps": round2(s.presented_fps),
        "decoded_fps": round2(s.decoded_fps),
        "latency_p50_ms": round2(f64::from(s.latency_p50_ms)),
        "latency_p95_ms": round2(f64::from(s.latency_p95_ms)),
        "latency_max_ms": round2(f64::from(s.latency_max_ms)),
        "max_gap_ms": s.max_gap_ms,
        "occluded": s.occluded,
        "device_lost": s.device_lost,
    })
}

fn hundredths(ms: f64) -> i64 {
    (ms * 100.0).round() as i64
}

fn size_text(size: (u32, u32)) -> String {
    format!("{}x{}", size.0, size.1)
}

fn rect_value(r: (i32, i32, u32, u32)) -> Value {
    json!([r.0, r.1, r.2, r.3])
}

#[cfg(test)]
mod tests;
