//! Runs [`Case`]s against a [`Harness`] and reports one JSON line each and a
//! summary. Generic over the harness so the build machine runs it against a
//! fake; Windows uses `win32::Win32Harness`.

use gliff_windows::stats_out::InputInfo;

use super::cases::Act;
use super::cases::Case;
use super::judge::Observed;
use super::judge::judge;
use super::remote_log::Entry;
use super::remote_log::last_n;
use super::remote_log::since;
use super::scan::evdev_of;
use crate::line::Line;

/// What the run does to the machine, and what it reads back.
pub trait Harness {
    /// Starts the client with these extra arguments and waits until its
    /// window shows a picture; stops the previous one first if still up.
    fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()>;
    fn stop_client(&mut self);
    fn act(&mut self, act: &Act) -> anyhow::Result<()>;
    /// The whole remote log, over ssh.
    fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>>;
    /// The foreground check, Notepad's text and the client's input stats.
    fn observe(&mut self) -> Seen;
    /// The remote output in logical pixels, from the client's stats.
    fn remote_size(&mut self) -> Option<(u32, u32)>;
}

/// What [`Harness::observe`] read.
#[derive(Debug, Default)]
pub struct Seen {
    pub foreground_is_client: bool,
    pub notepad_text: Option<String>,
    pub input: Option<InputInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseResult {
    pub name: String,
    /// Empty when the case passed.
    pub reasons: Vec<String>,
}

impl CaseResult {
    pub fn passed(&self) -> bool {
        self.reasons.is_empty()
    }

    pub fn line(&self, check: &'static str) -> Line {
        Line::new(check, self.passed())
            .with("case", self.name.clone())
            .with("reasons", self.reasons.clone())
    }
}

/// Runs every case in order. The client restarts only when the flags
/// change, so cases that depend on its state run back to back with the same
/// flags.
pub fn run_cases(h: &mut impl Harness, cases: &[Case]) -> Vec<CaseResult> {
    let mut results = Vec::new();
    let mut running: Option<&[String]> = None;
    let mut tripped: Option<String> = None;
    for case in cases {
        if case.risky
            && let Some(first) = &tripped
        {
            results.push(CaseResult {
                name: case.name.clone(),
                reasons: vec![format!(
                    "skipped: `{first}` did not reach the remote machine, so Windows may have acted on it; \
                     the remaining chords are not sent. Look at the Windows machine for an open window"
                )],
            });
            continue;
        }
        if running != Some(case.flags.as_slice()) {
            h.stop_client();
            if let Err(e) = h.start_client(&case.flags) {
                running = None;
                results.push(CaseResult {
                    name: case.name.clone(),
                    reasons: vec![format!("the client did not start: {e:#}")],
                });
                continue;
            }
            running = Some(case.flags.as_slice());
        }
        let (result, reached) = run_case(h, case);
        if case.risky && !reached {
            tripped = Some(case.name.clone());
        }
        results.push(result);
    }
    h.stop_client();
    results
}

/// The result, and whether the keys got to the remote machine with the client
/// still in front (false: Windows may have taken them). Anything short of
/// seeing both counts as false, errors included: a risky chord that could
/// not be followed is treated as one Windows may have acted on.
fn run_case(h: &mut impl Harness, case: &Case) -> (CaseResult, bool) {
    let (result, reached) = try_case(h, case);
    if case.risky {
        // Best effort; what it fixes shows in the next case's own checks.
        // It runs on every path, since a failed case may have left a window
        // open too.
        let _ = h.act(&Act::Dismiss);
    }
    (result, reached)
}

fn try_case(h: &mut impl Harness, case: &Case) -> (CaseResult, bool) {
    let mark = match h.remote_log() {
        Ok(log) => last_n(&log),
        Err(e) => {
            return (
                failed(case, format!("the remote log could not be read: {e:#}")),
                false,
            );
        }
    };
    for act in &case.acts {
        if let Err(e) = h.act(act) {
            return (failed(case, format!("{act:?} failed: {e:#}")), false);
        }
    }
    let log = match h.remote_log() {
        Ok(log) => log,
        Err(e) => {
            return (
                failed(case, format!("the remote log could not be read: {e:#}")),
                false,
            );
        }
    };
    let seen = h.observe();
    let observed = Observed {
        remote: since(&log, mark),
        sent_keys: sent_keys(case),
        foreground_is_client: seen.foreground_is_client,
        notepad_text: seen.notepad_text,
        input: seen.input,
    };
    let reached = !observed.remote.is_empty() && observed.foreground_is_client;
    let result = CaseResult {
        name: case.name.clone(),
        reasons: judge(&case.expect, &observed),
    };
    (result, reached)
}

/// The evdev codes of every key the case presses.
fn sent_keys(case: &Case) -> Vec<u16> {
    let mut codes: Vec<u16> = case
        .acts
        .iter()
        .filter_map(|act| match act {
            Act::Keys(presses) => Some(presses),
            _ => None,
        })
        .flatten()
        .filter_map(|(scan, _)| evdev_of(*scan))
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn failed(case: &Case, reason: String) -> CaseResult {
    CaseResult {
        name: case.name.clone(),
        reasons: vec![reason],
    }
}

/// The one line that says whether the whole check passed.
pub fn summary(check: &'static str, results: &[CaseResult]) -> Line {
    let failed: Vec<String> = results
        .iter()
        .filter(|r| !r.passed())
        .map(|r| r.name.clone())
        .collect();
    Line::new(check, failed.is_empty() && !results.is_empty())
        .with("cases", results.len())
        .with("passed", results.len() - failed.len())
        .with("failed", failed)
}

/// The flags a gate client starts with. `--super-l registry` changes a Windows
/// setting; only the lock step may do that, so every other case names `chord`
/// unless it names a mode itself, and never depends on the client's default.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn gate_flags(flags: &[String]) -> Vec<String> {
    let mut all = Vec::with_capacity(flags.len() + 2);
    if !flags.iter().any(|f| f == "--super-l") {
        all.extend(["--super-l".to_owned(), "chord".to_owned()]);
    }
    all.extend_from_slice(flags);
    all
}

#[cfg(test)]
mod tests;
