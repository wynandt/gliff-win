//! Keys the hook let through on purpose. While the grab is on, a key the hook
//! does not take (a media key, or an injected key under `--injected-keys
//! local`) still reaches the window as a key message. The window must not
//! mistake that for a silent hook, so the hook notes each such key here and
//! the window asks before it treats a message as "the hook was removed".

use std::collections::VecDeque;

use crate::grab_state::HookMode;
use crate::keymap::Press;

/// `dwExtraInfo` on keys this client injects, so the hook lets them through.
pub const OWN_TAG: usize = 0x676c_6966;

/// How long a note stays valid, in milliseconds (a key message follows its
/// hook call within a few milliseconds).
const LIFETIME_MS: i32 = 1000;
/// How long a note is kept for [`PassLog::noted_near`], which the silent-hook
/// line uses to tell a key whose time did not match its note from a key the
/// hook never saw.
const KEEP_MS: i32 = 5000;
/// Notes kept; older ones are dropped first.
const CAPACITY: usize = 32;

const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_LSHIFT: u16 = 0xa0;
const VK_RSHIFT: u16 = 0xa1;
const VK_LCONTROL: u16 = 0xa2;
const VK_RCONTROL: u16 = 0xa3;
const VK_LMENU: u16 = 0xa4;
const VK_RMENU: u16 = 0xa5;

/// The hook reports left and right modifiers, a key message the generic one;
/// both map to the generic key here.
fn family(vk: u16) -> u16 {
    match vk {
        VK_LSHIFT | VK_RSHIFT => VK_SHIFT,
        VK_LCONTROL | VK_RCONTROL => VK_CONTROL,
        VK_LMENU | VK_RMENU => VK_MENU,
        other => other,
    }
}

#[derive(Debug, Default)]
pub struct PassLog {
    notes: VecDeque<(u16, Press, u32)>,
}

impl PassLog {
    /// An empty log, usable in a `static`.
    pub const fn new() -> Self {
        Self {
            notes: VecDeque::new(),
        }
    }

    /// The hook let this key through at `time_ms` (the hook's key time).
    pub fn record(&mut self, vk: u16, press: Press, time_ms: u32) {
        if self.notes.len() == CAPACITY {
            self.notes.pop_front();
        }
        self.notes.push_back((family(vk), press, time_ms));
    }

    /// The hook's look at a key's `dwExtraInfo`: a key this client injected
    /// (it carries [`OWN_TAG`]) passes untouched, is noted so the key
    /// message that follows it is not taken for a silent hook, and is never
    /// forwarded to the remote. True for such a key, which the hook then
    /// lets through; false for any other, which is none of this log's
    /// business.
    pub fn note_if_own(&mut self, extra_info: usize, vk: u16, press: Press, time_ms: u32) -> bool {
        if extra_info != OWN_TAG {
            return false;
        }
        self.record(vk, press, time_ms);
        true
    }

    /// True (and the note is used up) if the hook let this key through
    /// shortly before `time_ms`, the key message's time.
    pub fn take(&mut self, vk: u16, press: Press, time_ms: u32) -> bool {
        self.notes
            .retain(|&(_, _, at)| (time_ms.wrapping_sub(at) as i32) <= KEEP_MS);
        let wanted = (family(vk), press);
        let valid = |at: u32| (time_ms.wrapping_sub(at) as i32) <= LIFETIME_MS;
        match self
            .notes
            .iter()
            .position(|&(v, p, at)| (v, p) == wanted && valid(at))
        {
            Some(index) => {
                self.notes.remove(index);
                true
            }
            None => false,
        }
    }
}

impl PassLog {
    /// True if the hook noted any press of this key's family within
    /// [`KEEP_MS`] of `time_ms`, before or after, that a message has not yet used. For the log
    /// only; it answers yes or no and names no key.
    pub fn noted_near(&self, vk: u16, time_ms: u32) -> bool {
        self.notes.iter().any(|&(v, _, at)| {
            v == family(vk) && (time_ms.wrapping_sub(at) as i32).unsigned_abs() <= KEEP_MS as u32
        })
    }
}

/// True when a key message that reached the window shows a silent hook: the
/// window believes it is grabbing, the hook itself is grabbing, and the hook
/// did not note passing this key.
///
/// `ui` is the mode the UI thread wants; `gate` is the mode the hook is in
/// (the two differ for a moment after a release chord, when the hook has
/// already given the keyboard back and passes every key on purpose);
/// `was_passed` is whether the hook noted passing this key, which it does for
/// a key it passes with no session to send it to as well.
pub fn silent_hook_judge(ui: HookMode, gate: HookMode, was_passed: bool) -> bool {
    ui == HookMode::Grab && gate == HookMode::Grab && !was_passed
}

/// What the hook did before the window found it silent: counts and times
/// only.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HookTiming {
    /// Callbacks since the hook was installed.
    pub calls: u32,
    /// Microseconds the last callback took, and the slowest one.
    pub last_us: u32,
    pub slowest_us: u32,
    /// Keys the hook saw flagged as injected by another program.
    pub injected: u32,
    /// Milliseconds since the last callback ended; none if it never ran.
    pub ago_ms: Option<u64>,
}

/// What the log adds to the silent-hook line to tell the causes apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilentDetail {
    /// The key message carried extra info, which a program that sends keys
    /// usually sets and a keyboard does not.
    pub extra_info: bool,
    pub hook: HookTiming,
    /// The pass log held a note of this key's family within [`KEEP_MS`].
    pub noted_near: bool,
}

/// The log line for a silent hook: how many times so far, how many keys are
/// down with which modifier groups among them (see
/// [`crate::key_ledger::describe_down`]), and the [`SilentDetail`]. It names
/// no key.
pub fn silent_line(
    so_far: u32,
    held: impl IntoIterator<Item = u16>,
    detail: SilentDetail,
) -> String {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let hook = match detail.hook.ago_ms {
        Some(ago) => format!(
            "hook calls {}, last {} us, slowest {} us, last ran {ago} ms ago, injected keys seen {}",
            detail.hook.calls, detail.hook.last_us, detail.hook.slowest_us, detail.hook.injected
        ),
        None => "hook never called since installed".to_owned(),
    };
    format!(
        "keyboard hook silent: key sent from the window ({so_far} so far), hook installed again; \
         {}; message extra info: {}; {hook}; hook noted this key within {} s: {}",
        crate::key_ledger::describe_down(held),
        if detail.extra_info { "set" } else { "none" },
        KEEP_MS / 1000,
        yes(detail.noted_near),
    )
}

#[cfg(test)]
mod tests;
