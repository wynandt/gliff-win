//! A Win key whose Up the grab kept from Windows.
//!
//! Windows reads Win+L (and a few other Win shortcuts) ahead of the keyboard
//! hook, from its own view of the keys. With the lock switched off by the
//! `DisableLockWorkstation` setting, that path still counts the Win key as
//! down. The hook swallows the Win key's Up, so the path never sees it come
//! up, and Windows goes on acting as if Win were held: the next plain key is a
//! Win shortcut (F opens Feedback Hub, D shows the desktop). This state is
//! not the one `GetAsyncKeyState` reads, which only follows events the hook
//! let through, so a check of that state says no key is down.
//!
//! So whenever the hook swallows a Win key event in a grab, Windows is owed
//! that key's Up. The hook gives it at once when it swallows the Up, and the
//! grab end, a hook reinstall and the exit give any Up still owed, whatever
//! `GetAsyncKeyState` says. An Up Windows did not need is harmless; the
//! Start-menu mask goes first so it never opens Start. Pure.

use crate::modifiers::VK_LWIN;
use crate::modifiers::VK_RWIN;

/// The two Win keys, left first.
const WINS: [u16; 2] = [VK_LWIN, VK_RWIN];

/// Which Win keys Windows is owed an Up for.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WinOwed {
    owed: [bool; 2],
}

fn slot(vk: u16) -> Option<usize> {
    WINS.iter().position(|&win| win == vk)
}

impl WinOwed {
    pub const fn new() -> Self {
        Self { owed: [false; 2] }
    }

    /// The hook swallowed this key's event while grabbing. Any other key is
    /// ignored.
    pub fn swallowed(&mut self, vk: u16) {
        if let Some(i) = slot(vk) {
            self.owed[i] = true;
        }
    }

    /// An Up of this key reached Windows: the hook passed it, or it was
    /// this client's own.
    pub fn windows_saw_up(&mut self, vk: u16) {
        if let Some(i) = slot(vk) {
            self.owed[i] = false;
        }
    }

    /// The Win keys whose Up Windows is owed and the user does not hold now
    /// (`holds` is the hook's view), left first. A key the user still holds
    /// gets its own Up later.
    pub fn due(&self, holds: impl Fn(u16) -> bool) -> Vec<u16> {
        WINS.iter()
            .zip(self.owed)
            .filter(|&(&vk, owed)| owed && !holds(vk))
            .map(|(&vk, _)| vk)
            .collect()
    }

    /// Every Win key still owed, for the log.
    pub fn owed(&self) -> Vec<u16> {
        self.due(|_| false)
    }

    /// The Ups for these keys were given.
    pub fn settle(&mut self, given: &[u16]) {
        for &vk in given {
            self.windows_saw_up(vk);
        }
    }

    /// Forgets every key, at a grab end: the Ups not given there are of keys
    /// the user still holds, and their own Up reaches Windows.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests;
