//! What each key did, as Windows and the remote machine saw it.
//!
//! The hook swallows keys while a grab is on. A swallowed Down never reaches
//! Windows, so Windows must also never get that key's Up, or it counts a key
//! that was never pressed as released; and a key Windows *did* see go down
//! (the hook was not looking, or let it through) must get its Up, or Windows
//! counts it as held for good: a Win or Ctrl that stays "down" turns the next
//! plain key into a shortcut.
//!
//! The ledger keeps two facts per key: whether Windows saw its Down, and
//! whether the remote machine did. It generalises the old set of keys that were
//! down before a grab. It outlives a hook reinstall, so a key that went down
//! in the gap still gets its Up. Pure.
//!
//! The ledger holds key codes in memory only. [`describe_down`] is the one
//! way it is written to the log, and it writes a count and the names of the
//! four modifier groups, never which other keys are down.

use crate::keymap::Press;

/// Virtual-key codes are one byte.
const KEYS: usize = 256;

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    /// Windows saw this key's Down and has not seen its Up.
    windows: bool,
    /// The remote machine saw this key's Down and has not seen its Up.
    remote: bool,
}

/// Where the next event of a key goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// An ordinary key of the grab: swallow it and send it to the remote.
    Remote,
    /// Windows saw this key go down, so the event goes to Windows. `remote`
    /// says the remote saw the Down too, so it gets the event as well.
    Windows { remote: bool },
}

#[derive(Debug)]
pub struct KeyLedger {
    slots: [Slot; KEYS],
}

impl KeyLedger {
    /// An empty ledger, usable in a `static`.
    pub const fn new() -> Self {
        Self {
            slots: [Slot {
                windows: false,
                remote: false,
            }; KEYS],
        }
    }

    /// A grab starts: the keys Windows holds down now are the Windows-side
    /// keys, and the remote has seen nothing. Replaces everything.
    pub fn seed(&mut self, held: impl IntoIterator<Item = u16>) {
        self.clear();
        for vk in held {
            if let Some(slot) = self.slots.get_mut(usize::from(vk)) {
                slot.windows = true;
            }
        }
    }

    /// The hook was installed again in the middle of a grab: the keys Windows
    /// holds down now join the Windows-side keys, and what the remote saw
    /// stays as it was.
    pub fn reinstall(&mut self, held: impl IntoIterator<Item = u16>) {
        for vk in held {
            if let Some(slot) = self.slots.get_mut(usize::from(vk)) {
                slot.windows = true;
            }
        }
    }

    /// Windows saw this event (the hook passed it, or a window message
    /// brought it, or the hook replayed it). A Down makes the key a
    /// Windows-side key; its Up ends that.
    pub fn windows_saw(&mut self, vk: u16, press: Press) {
        if let Some(slot) = self.slots.get_mut(usize::from(vk)) {
            slot.windows = press == Press::Down;
        }
    }

    /// The hook handed this event to the remote's session. A Down is held on
    /// the remote until its Up.
    pub fn remote_saw(&mut self, vk: u16, press: Press) {
        if let Some(slot) = self.slots.get_mut(usize::from(vk)) {
            slot.remote = press == Press::Down;
        }
    }

    /// A key message reached the window while the hook was thought silent, and
    /// the window sent it to the remote (`sent`). Windows saw it either way.
    pub fn legacy(&mut self, vk: u16, press: Press, sent: bool) {
        self.windows_saw(vk, press);
        if sent || press == Press::Up {
            self.remote_saw(vk, press);
        }
    }

    /// Where this event goes. An Up of a Windows-side key ends its time on
    /// both sides.
    pub fn route(&mut self, vk: u16, press: Press) -> Route {
        let Some(slot) = self.slots.get_mut(usize::from(vk)) else {
            return Route::Remote;
        };
        if !slot.windows {
            return Route::Remote;
        }
        let remote = slot.remote;
        if press == Press::Up {
            slot.windows = false;
            slot.remote = false;
        }
        Route::Windows { remote }
    }

    /// The keys the remote still holds; forgets them. For a grab that ended:
    /// the caller tells the session to let go of them.
    pub fn take_remote_held(&mut self) -> Vec<u16> {
        let mut held = Vec::new();
        for (vk, slot) in self.slots.iter_mut().enumerate() {
            if slot.remote {
                slot.remote = false;
                held.push(vk as u16);
            }
        }
        held
    }

    /// The grab ended: Windows owns the keyboard, and what it saw before is
    /// no longer a reason to route anything. The remote's keys stay, for
    /// [`Self::take_remote_held`].
    pub fn forget_windows(&mut self) {
        for slot in &mut self.slots {
            slot.windows = false;
        }
    }

    pub fn clear(&mut self) {
        self.slots = [Slot::default(); KEYS];
    }

    /// How many keys Windows saw go down and has not seen come up.
    pub fn windows_len(&self) -> usize {
        self.slots.iter().filter(|s| s.windows).count()
    }

    /// The keys Windows saw go down and has not seen come up, for the log
    /// (through [`describe_down`], which names no key but the modifiers).
    pub fn windows_held(&self) -> Vec<u16> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.windows)
            .map(|(vk, _)| vk as u16)
            .collect()
    }
}

/// What the hook last saw of a key's real state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Seen {
    /// The hook never saw this key move.
    #[default]
    Unknown,
    Up,
    Down,
}

/// The keys as the user holds them, as far as the keyboard hook (and the
/// window's own key messages) have seen: every event but this client's own
/// injected ones, in every mode and whether or not the hook swallowed it.
/// Windows' own state (`GetAsyncKeyState`) cannot say whether a modifier is
/// held or stuck; this can, for what the hook saw. Pure, memory only.
#[derive(Debug)]
pub struct Physical {
    keys: [Seen; KEYS],
}

impl Physical {
    pub const fn new() -> Self {
        Self {
            keys: [Seen::Unknown; KEYS],
        }
    }

    pub fn saw(&mut self, vk: u16, press: Press) {
        if let Some(key) = self.keys.get_mut(usize::from(vk)) {
            *key = if press == Press::Down {
                Seen::Down
            } else {
                Seen::Up
            };
        }
    }

    /// The keys the hook saw go down and not come up, for the log.
    pub fn down_keys(&self) -> Vec<u16> {
        (0..KEYS as u16)
            .filter(|&vk| self.seen(vk) == Seen::Down)
            .collect()
    }

    pub fn seen(&self, vk: u16) -> Seen {
        self.keys.get(usize::from(vk)).copied().unwrap_or_default()
    }

    /// Which of the keys Windows says are down may be released as stuck at
    /// the end of a grab: all but those the hook saw the user press and not
    /// let go of. A key whose Down the hook never saw is let go of, as it
    /// always was.
    pub fn stuck_at_grab_end(&self, windows_down: bool, vk: u16) -> bool {
        windows_down && self.seen(vk) != Seen::Down
    }

    /// Which may be released when the hook is installed again in the middle
    /// of a grab: only those the hook saw come up while Windows still counts
    /// them as down. A key the hook never saw move may be one the user holds
    /// since before the grab.
    pub fn stuck_at_reinstall(&self, windows_down: bool, vk: u16) -> bool {
        windows_down && self.seen(vk) == Seen::Up
    }
}

impl Default for Physical {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for KeyLedger {
    fn default() -> Self {
        Self::new()
    }
}

/// What the client may write to its log about the keys that are down: how
/// many, and which of the four modifier groups (Win, Ctrl, Alt, Shift) are
/// among them. Never any other key, and never an order: the groups come in
/// a fixed order whatever order the keys went down in.
///
/// The generic Shift, Ctrl and Alt codes (Windows reports them beside the left
/// and right ones) add their group but are not counted as keys of their own.
pub fn describe_down(held: impl IntoIterator<Item = u16>) -> String {
    let mut count = 0usize;
    let mut groups = [false; 4];
    for vk in held {
        let group = match vk {
            0x5b | 0x5c => Some(0),
            0x11 | 0xa2 | 0xa3 => Some(1),
            0x12 | 0xa4 | 0xa5 => Some(2),
            0x10 | 0xa0 | 0xa1 => Some(3),
            _ => None,
        };
        if let Some(group) = group {
            groups[group] = true;
        }
        if !matches!(vk, 0x10..=0x12) {
            count += 1;
        }
    }
    let names: Vec<&str> = ["Win", "Ctrl", "Alt", "Shift"]
        .into_iter()
        .zip(groups)
        .filter_map(|(name, held)| held.then_some(name))
        .collect();
    let noun = if count == 1 { "key" } else { "keys" };
    if names.is_empty() {
        format!("{count} {noun} down")
    } else {
        format!("{count} {noun} down ({})", names.join(", "))
    }
}

#[cfg(test)]
mod tests;
