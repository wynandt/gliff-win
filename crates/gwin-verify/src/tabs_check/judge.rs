//! The judging half of `gwin-verify tabs`: the snapshots of the client's
//! per-session stats to one JSON line. Pure.

use gliff_windows::stats_out::SessionEntry;
use serde_json::json;

use crate::line::Line;
pub use crate::present_check::judge::ClientExit;

/// One switch the run makes and where the keyboard must be after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The chord, spelled as `scan::chord_presses` reads it.
    pub chord: &'static str,
    /// For the report: what the chord is.
    pub name: &'static str,
    /// The index (in command-line order) of the target that must be shown.
    pub shown: usize,
}

/// What the run expects: the targets in command-line order and the switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expect {
    pub targets: Vec<String>,
    pub steps: Vec<Step>,
}

impl Expect {
    /// Switches over three or more targets, ending on the last. Next and
    /// previous wrap, so the plan stays inside the tabs to say the same of
    /// every count of three or more.
    pub fn standard(targets: Vec<String>) -> Self {
        let last = targets.len().saturating_sub(1);
        let steps = vec![
            Step {
                chord: "ctrl+alt+pagedown",
                name: "next",
                shown: 1,
            },
            Step {
                chord: "ctrl+alt+pagedown",
                name: "next again",
                shown: 2,
            },
            Step {
                chord: "ctrl+alt+pageup",
                name: "previous",
                shown: 1,
            },
            Step {
                chord: "ctrl+alt+1",
                name: "jump to the first",
                shown: 0,
            },
            Step {
                chord: "ctrl+alt+3",
                name: "jump to the third",
                shown: 2.min(last),
            },
        ];
        Self { targets, steps }
    }
}

/// What the run saw after each step (and at the start): every session's line.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    /// With every session streaming, before the first chord.
    pub start: Vec<SessionEntry>,
    /// One per step, taken after the chord settled.
    pub steps: Vec<Vec<SessionEntry>>,
    /// The recent hosts the client wrote, as target text.
    pub history: Vec<String>,
    pub exit: ClientExit,
}

/// The `tabs` line: every gate, each failure a named reason.
pub fn judge(observed: &Observed, expect: &Expect) -> Line {
    let mut reasons: Vec<String> = Vec::new();
    check_start(&observed.start, expect, &mut reasons);
    if observed.steps.len() != expect.steps.len() {
        reasons.push(format!(
            "steps_missing: {} of {} switches were made",
            observed.steps.len(),
            expect.steps.len()
        ));
    }
    let mut before = &observed.start;
    let mut step_lines = Vec::new();
    for (step, entries) in expect.steps.iter().zip(&observed.steps) {
        check_step(step, entries, before, expect, &mut reasons);
        step_lines.push(json!({
            "step": step.name,
            "expected_shown": step.shown,
            "shown": shown_index(entries, expect),
            "keys_down": entries.iter().map(|e| e.keys_down).sum::<u32>(),
        }));
        before = entries;
    }
    for target in &expect.targets {
        if !observed.history.iter().any(|h| h == target) {
            reasons.push(format!("history_missing: {target} was not recorded"));
        }
    }
    match observed.exit {
        ClientExit::Exited(Some(0)) => {}
        ClientExit::Exited(code) => reasons.push(format!("exit: code {code:?}")),
        ClientExit::Killed => reasons.push("exit: the client did not exit and was killed".into()),
    }
    Line::new("tabs", reasons.is_empty())
        .with("sessions", expect.targets.len())
        .with("steps", step_lines)
        .with("history", observed.history.clone())
        .with("reasons", reasons)
}

fn check_start(start: &[SessionEntry], expect: &Expect, reasons: &mut Vec<String>) {
    if start.len() != expect.targets.len() {
        reasons.push(format!(
            "sessions: {} listed, {} expected",
            start.len(),
            expect.targets.len()
        ));
    }
    for (entry, target) in start.iter().zip(&expect.targets) {
        if entry.target != *target {
            reasons.push(format!(
                "target_order: session {} is {}, expected {target}",
                entry.id, entry.target
            ));
        }
        if entry.status != "streaming" {
            reasons.push(format!(
                "not_streaming: session {} is {}",
                entry.id, entry.status
            ));
        }
    }
}

fn check_step(
    step: &Step,
    entries: &[SessionEntry],
    before: &[SessionEntry],
    expect: &Expect,
    reasons: &mut Vec<String>,
) {
    let shown: Vec<&SessionEntry> = entries.iter().filter(|e| e.shown).collect();
    if shown.len() != 1 {
        reasons.push(format!(
            "shown_count: after {} {} sessions are shown",
            step.name,
            shown.len()
        ));
    } else if expect.targets.get(step.shown) != Some(&shown[0].target) {
        reasons.push(format!(
            "wrong_shown: after {} session {} ({}) is shown, expected target {}",
            step.name, shown[0].id, shown[0].target, step.shown
        ));
    }
    for entry in entries {
        if entry.keys_down != 0 || entry.buttons_down != 0 {
            reasons.push(format!(
                "held_input: after {} session {} has {} keys and {} buttons down",
                step.name, entry.id, entry.keys_down, entry.buttons_down
            ));
        }
        // Every session decodes and acks while hidden, so a switch is
        // instant: its acks must have grown since the last snapshot.
        if let Some(was) = before.iter().find(|b| b.id == entry.id)
            && entry.acks <= was.acks
        {
            reasons.push(format!(
                "not_acked: after {} session {} acked {} then {}",
                step.name, entry.id, was.acks, entry.acks
            ));
        }
    }
}

fn shown_index(entries: &[SessionEntry], expect: &Expect) -> Option<usize> {
    let shown = entries.iter().find(|e| e.shown)?;
    expect.targets.iter().position(|t| *t == shown.target)
}
