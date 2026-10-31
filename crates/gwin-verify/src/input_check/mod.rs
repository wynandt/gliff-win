//! `gwin-verify keys` and `pointer`: drive the real
//! client from outside and judge what the remote machine saw.
//!
//! The pure parts (scancodes, the remote log, the cases, the judge, the
//! lock-step rows) run on the build machine against fakes. Only `win32`
//! touches a real machine, and only on Windows.

// Only Windows drives a real machine; elsewhere these run in tests.
#[cfg_attr(not(windows), allow(dead_code))]
pub mod cases;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod countdown;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod fresh;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod judge;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod lock;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod manual;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod placement;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod remote_log;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod run;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod scan;
#[cfg(windows)]
pub mod win32;

use cases::Case;

/// The committed Super-chord table the gate sends.
#[cfg_attr(not(windows), allow(dead_code))]
const CHORD_TABLE: &str = include_str!("../../../../docs/phase4/super-chords.tsv");

/// A group of `keys` cases (`--only`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum KeyGroup {
    /// The 20 Super chords.
    Chords,
    /// Alt+Tab, Alt+F4 and the other system chords.
    System,
    /// Layout sending: German, and AltGr.
    Layout,
    /// Focus mode and hover mode capture.
    Capture,
    /// The release chords.
    Release,
    /// The hook stall and recovery.
    Stall,
    /// `--injected-keys local`.
    Injected,
    /// The Super+L stand-in chord.
    Standin,
    /// The lock-setting step (never part of the default run).
    Lock,
    /// The prompts for the chords only a person can press (as `--manual`,
    /// but alone: no case runs unless other groups are named too).
    Manual,
}

impl KeyGroup {
    /// What a run without `--only` includes.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub const DEFAULT: [Self; 8] = [
        Self::Chords,
        Self::System,
        Self::Layout,
        Self::Capture,
        Self::Release,
        Self::Stall,
        Self::Injected,
        Self::Standin,
    ];
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The cases whose name contains one of `needles` (any case of letters), in
/// plan order; all of them when there are no needles. An error when nothing
/// matches, so a mistyped name is not a run of zero cases that "passes".
pub fn select_cases(plan: Vec<Case>, needles: &[String]) -> Result<Vec<Case>, String> {
    if needles.is_empty() {
        return Ok(plan);
    }
    let needles: Vec<String> = needles.iter().map(|n| n.to_lowercase()).collect();
    let chosen: Vec<Case> = plan
        .into_iter()
        .filter(|c| {
            let name = c.name.to_lowercase();
            needles.iter().any(|n| name.contains(n.as_str()))
        })
        .collect();
    if chosen.is_empty() {
        return Err(format!("no case name contains any of {needles:?}"));
    }
    Ok(chosen)
}

/// The cases of the chosen groups, in a fixed order. The lock group has no
/// cases: it is its own driver.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn keys_plan(groups: &[KeyGroup]) -> Result<Vec<Case>, String> {
    let groups = if groups.is_empty() {
        &KeyGroup::DEFAULT[..]
    } else {
        groups
    };
    let mut plan = Vec::new();
    for group in [
        KeyGroup::Chords,
        KeyGroup::System,
        KeyGroup::Layout,
        KeyGroup::Capture,
        KeyGroup::Release,
        KeyGroup::Stall,
        KeyGroup::Injected,
        KeyGroup::Standin,
    ] {
        if !groups.contains(&group) {
            continue;
        }
        match group {
            KeyGroup::Chords => {
                let rows = cases::parse_chord_table(CHORD_TABLE)?;
                plan.extend(cases::super_chords(&rows)?);
            }
            KeyGroup::System => plan.extend(cases::system_chords()),
            KeyGroup::Layout => plan.extend(cases::layouts()),
            KeyGroup::Capture => {
                plan.extend(cases::capture_follows_focus());
                plan.extend(cases::capture_follows_hover());
            }
            KeyGroup::Release => plan.extend(cases::release_chords()),
            KeyGroup::Stall => plan.extend(cases::hook_stall()),
            KeyGroup::Injected => plan.extend(cases::injected_local()),
            KeyGroup::Standin => plan.extend(cases::standin_chord()),
            KeyGroup::Lock | KeyGroup::Manual => {}
        }
    }
    Ok(plan)
}

/// The pointer cases for a remote output of `size`: nine clicks, the wheel
/// and the four buttons.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn pointer_plan(size: (u32, u32)) -> Vec<Case> {
    let mut plan = cases::clicks(size);
    plan.extend(cases::wheel());
    plan.extend(cases::buttons());
    plan
}

#[cfg(test)]
mod tests;
