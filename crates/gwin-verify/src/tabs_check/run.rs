//! The driving half of `gwin-verify tabs`, generic over [`Harness`] so the
//! build machine runs it against a fake.

use std::time::Duration;
use std::time::Instant;

use gliff_windows::stats_out::ClientStats;
use gliff_windows::stats_out::SessionEntry;

use super::judge::ClientExit;
use super::judge::Expect;
use super::judge::Observed;
use super::judge::judge;
use crate::line::Line;

/// What the run does to the client, and what it reads back.
pub trait Harness {
    /// Starts the client with every target and waits for its window.
    fn launch(&mut self) -> anyhow::Result<()>;
    /// Clicks the picture so the window is in the foreground and grabbing.
    fn focus(&mut self) -> anyhow::Result<()>;
    /// Injects a chord (`ctrl+alt+pagedown`) and lets go of it.
    fn chord(&mut self, chord: &str) -> anyhow::Result<()>;
    /// The client's latest stats file.
    fn stats(&mut self) -> Option<ClientStats>;
    /// The targets of the recent hosts file.
    fn recent_hosts(&mut self) -> Vec<String>;
    /// Asks the window to close.
    fn close(&mut self) -> anyhow::Result<()>;
    /// Waits up to `timeout` for the client to exit; kills it after.
    fn wait_exit(&mut self, timeout: Duration) -> ClientExit;
    fn sleep(&mut self, d: Duration);
}

/// Timings of one run.
#[derive(Debug, Clone)]
pub struct Plan {
    pub expect: Expect,
    /// How long every session gets to stream.
    pub start_timeout: Duration,
    /// How long a chord gets to move the shown session.
    pub switch_timeout: Duration,
    /// After a switch, for the counters to move on every session.
    pub settle: Duration,
    pub exit_timeout: Duration,
    pub poll: Duration,
}

impl Plan {
    pub fn new(expect: Expect) -> Self {
        Self {
            expect,
            start_timeout: Duration::from_secs(60),
            switch_timeout: Duration::from_secs(5),
            settle: Duration::from_secs(3),
            exit_timeout: Duration::from_secs(10),
            poll: Duration::from_millis(250),
        }
    }
}

fn stopped(reason: &str, error: impl std::fmt::Display) -> Line {
    Line::new("tabs", false)
        .with("reasons", vec![reason.to_owned()])
        .with("error", error.to_string())
}

fn sessions(h: &mut impl Harness) -> Vec<SessionEntry> {
    h.stats().map(|s| s.sessions).unwrap_or_default()
}

fn all_streaming(entries: &[SessionEntry], count: usize) -> bool {
    entries.len() == count
        && entries
            .iter()
            .all(|e| e.status == "streaming" && e.frames > 0)
}

/// Waits until `done` holds for the sessions or `timeout` passes; returns
/// the last sessions read either way.
fn wait_for(
    h: &mut impl Harness,
    plan: &Plan,
    timeout: Duration,
    done: impl Fn(&[SessionEntry]) -> bool,
) -> (bool, Vec<SessionEntry>) {
    let deadline = Instant::now() + timeout;
    loop {
        let entries = sessions(h);
        if done(&entries) {
            return (true, entries);
        }
        if Instant::now() >= deadline {
            return (false, entries);
        }
        h.sleep(plan.poll);
    }
}

/// Runs the switches and judges them.
pub fn run(h: &mut impl Harness, plan: &Plan) -> Line {
    if let Err(e) = h.launch() {
        return stopped("launch", format!("{e:#}"));
    }
    let count = plan.expect.targets.len();
    let (streaming, start) = wait_for(h, plan, plan.start_timeout, |e| all_streaming(e, count));
    if !streaming {
        let _ = h.close();
        let exit = h.wait_exit(Duration::ZERO);
        return stopped(
            "not_streaming",
            format!(
                "{} of {count} sessions streaming within {:?} (client {exit:?})",
                start.iter().filter(|e| e.status == "streaming").count(),
                plan.start_timeout
            ),
        );
    }
    if let Err(e) = h.focus() {
        let _ = h.close();
        return stopped("focus", format!("{e:#}"));
    }
    h.sleep(plan.settle);
    let start = sessions(h);
    let mut steps = Vec::new();
    for step in &plan.expect.steps {
        if let Err(e) = h.chord(step.chord) {
            let _ = h.close();
            return stopped("chord", format!("{}: {e:#}", step.name));
        }
        let want = plan.expect.targets.get(step.shown).cloned();
        wait_for(h, plan, plan.switch_timeout, |entries| {
            entries
                .iter()
                .any(|e| e.shown && Some(&e.target) == want.as_ref())
        });
        h.sleep(plan.settle);
        steps.push(sessions(h));
    }
    let _ = h.close();
    let exit = h.wait_exit(plan.exit_timeout);
    let observed = Observed {
        start,
        steps,
        history: h.recent_hosts(),
        exit,
    };
    judge(&observed, &plan.expect)
}
