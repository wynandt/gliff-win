//! The pass or fail of one case, from what the remote log, the foreground
//! window, Notepad and the client's stats showed. Pure.

use gliff_windows::stats_out::InputInfo;

use super::cases::Expect;
use super::cases::Foreground;
use super::cases::Notepad;
use super::cases::Remote;
use super::remote_log::Entry;
use super::remote_log::cursor_of;

/// What the harness saw after a case.
#[derive(Clone, Debug, Default)]
pub struct Observed {
    /// The remote log entries that appeared during the case.
    pub remote: Vec<Entry>,
    /// The evdev codes of the keys the case sent (what the remote machine's key tap
    /// may see); anything else it saw is a stray.
    pub sent_keys: Vec<u16>,
    pub foreground_is_client: bool,
    pub notepad_text: Option<String>,
    pub input: Option<InputInfo>,
}

/// An entry matches a name when it is that name, or the name followed by
/// a space and detail (`mouse:273 10,20`).
fn matches(what: &str, name: &str) -> bool {
    what == name
        || what
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with(' '))
}

/// Why the case failed; empty when it passed.
pub fn judge(expect: &Expect, seen: &Observed) -> Vec<String> {
    let mut reasons = Vec::new();
    let all: Vec<&str> = seen.remote.iter().map(|e| e.what.as_str()).collect();
    let is_binds = |w: &&str| super::remote_log::is_bind(w);
    let whats: Vec<&str> = all.iter().copied().filter(is_binds).collect();
    judge_keys(expect, seen, &all, &mut reasons);
    match &expect.remote {
        Remote::Nothing => {
            if !whats.is_empty() {
                reasons.push(format!("remote logged {whats:?}, wanted nothing"));
            }
        }
        Remote::Exactly(want) => {
            let ok = whats.len() == want.len()
                && whats.iter().zip(want).all(|(w, name)| matches(w, name));
            if !ok {
                reasons.push(format!("remote logged {whats:?}, wanted exactly {want:?}"));
            }
        }
        Remote::InOrder { entries, never } => {
            let mut rest = whats.iter();
            for name in entries {
                if !rest.any(|w| matches(w, name)) {
                    reasons.push(format!("remote log {whats:?} lacks {name:?} (in order)"));
                    break;
                }
            }
            for name in never {
                if whats.iter().any(|w| matches(w, name)) {
                    reasons.push(format!("remote logged {name:?}, which must never appear"));
                }
            }
        }
        Remote::Clicks { targets, tolerance } => {
            judge_clicks(&whats, targets, *tolerance, &mut reasons);
        }
    }
    if let Some(want) = &expect.typed {
        let typed: Vec<&str> = all
            .iter()
            .filter_map(|w| w.strip_prefix("typed:"))
            .collect();
        if typed != *want {
            reasons.push(format!(
                "a remote client typed {typed:?}, wanted {want:?} \
                 (an empty list means no `wev` window had focus there)"
            ));
        }
    }
    match expect.foreground {
        Foreground::Unchecked => {}
        Foreground::Client => {
            if !seen.foreground_is_client {
                reasons.push("the client is no longer the foreground window".to_owned());
            }
        }
    }
    match (&expect.notepad, &seen.notepad_text) {
        (Notepad::Unchecked, _) => {}
        (_, None) => reasons.push("Notepad's text could not be read".to_owned()),
        (Notepad::Empty, Some(text)) if !text.is_empty() => {
            reasons.push(format!("Notepad holds {text:?}, wanted it empty"));
        }
        (Notepad::Text(want), Some(text)) if text != want => {
            reasons.push(format!("Notepad holds {text:?}, wanted {want:?}"));
        }
        _ => {}
    }
    judge_client(expect, seen, &mut reasons);
    reasons
}

/// One raw key event of the remote log: `key:down:<evdev>`.
fn raw_down(what: &str) -> Option<u16> {
    what.strip_prefix("key:down:")?.parse().ok()
}

fn raw_code(what: &str) -> Option<u16> {
    let rest = what.strip_prefix("key:")?;
    rest.split_once(':')?.1.parse().ok()
}

/// What the remote machine's key tap saw: no key the case did not send, every key
/// that had to arrive, none that must not.
fn judge_keys(expect: &Expect, seen: &Observed, all: &[&str], reasons: &mut Vec<String>) {
    let keys = &expect.keys;
    let mut stray: Vec<u16> = Vec::new();
    for code in all.iter().filter_map(|w| raw_code(w)) {
        let sent = seen.sent_keys.contains(&code) || keys.extra.contains(&code);
        if !sent && !stray.contains(&code) {
            stray.push(code);
        }
    }
    if !stray.is_empty() {
        reasons.push(format!(
            "the remote machine saw keys this case did not send (evdev {stray:?}): a stray key, or a leak"
        ));
    }
    let downs: Vec<u16> = all.iter().filter_map(|w| raw_down(w)).collect();
    for code in &keys.never {
        if downs.contains(code) {
            reasons.push(format!(
                "key {code} reached the remote machine, which must never happen"
            ));
        }
    }
    for code in &keys.need {
        if !downs.contains(code) {
            reasons.push(format!("key {code} never went down on the remote machine"));
        }
    }
    for code in &keys.need_up {
        match last_event(all, *code) {
            Some(RawEdge::Up) => {}
            Some(RawEdge::Down) => reasons.push(format!(
                "key {code} was still down on the remote machine at the end (a stuck key)"
            )),
            None => reasons.push(format!("key {code} never reached the remote machine")),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawEdge {
    Down,
    Up,
}

/// The last raw event the remote machine saw for `code`.
fn last_event(all: &[&str], code: u16) -> Option<RawEdge> {
    all.iter().rev().find_map(|w| {
        let (edge, c) = w.strip_prefix("key:")?.split_once(':')?;
        if c.parse::<u16>().ok()? != code {
            return None;
        }
        match edge {
            "down" => Some(RawEdge::Down),
            "up" => Some(RawEdge::Up),
            _ => None,
        }
    })
}

fn judge_clicks(whats: &[&str], targets: &[(f64, f64)], tolerance: f64, reasons: &mut Vec<String>) {
    let positions: Vec<Option<(f64, f64)>> = whats.iter().map(|w| cursor_of(w, 272)).collect();
    if positions.len() != targets.len() || positions.iter().any(Option::is_none) {
        reasons.push(format!(
            "remote logged {whats:?}, wanted {} left click(s)",
            targets.len()
        ));
        return;
    }
    for (got, want) in positions.iter().flatten().zip(targets) {
        let (dx, dy) = (got.0 - want.0, got.1 - want.1);
        if dx.abs() > tolerance || dy.abs() > tolerance {
            reasons.push(format!(
                "click landed at ({:.1},{:.1}), wanted ({:.1},{:.1}) within {tolerance}",
                got.0, got.1, want.0, want.1
            ));
        }
    }
}

fn judge_client(expect: &Expect, seen: &Observed, reasons: &mut Vec<String>) {
    let want = &expect.client;
    if want.grab.is_none() && want.keymap_contains.is_empty() && want.min_hook_reinstalls == 0 {
        return;
    }
    let Some(input) = &seen.input else {
        reasons.push("the client's stats file has no input section".to_owned());
        return;
    };
    if let Some(grab) = want.grab
        && input.grab != grab
    {
        reasons.push(format!("client grab is {}, wanted {grab}", input.grab));
    }
    if input.hook_reinstalls < want.min_hook_reinstalls {
        reasons.push(format!(
            "inconclusive: the client reinstalled the hook {} times, wanted at least {}; \
             Windows did not drop the hook, so the recovery was not exercised",
            input.hook_reinstalls, want.min_hook_reinstalls
        ));
    }
    for needle in &want.keymap_contains {
        if !input.keymap_sent.iter().any(|l| l.contains(needle)) {
            reasons.push(format!(
                "no keymap_sent line contains {needle:?}: {:?}",
                input.keymap_sent
            ));
        }
    }
}

#[cfg(test)]
mod tests;
