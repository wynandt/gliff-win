//! The AltGr filter. On a layout with an AltGr key, Windows reports AltGr as
//! a fake left Ctrl followed within a few milliseconds by the right Alt. The
//! remote machine's layout has its own AltGr (level 3), so the fake Ctrl must
//! not be forwarded or every AltGr character would arrive as Ctrl+Alt+key.
//!
//! The filter holds a left-Ctrl Down for up to [`DEADLINE_MS`]. If the right
//! Alt Down follows inside that time, the Ctrl is dropped (and so is its
//! matching Up). Otherwise the Ctrl is forwarded late. Pure and clocked by
//! the key times, so it is tested without a clock.

use crate::keymap::Prefix;
use crate::keymap::Press;

/// How long a left-Ctrl Down waits for the right Alt that follows it.
pub const DEADLINE_MS: u32 = 10;

const VK_LCONTROL: u16 = 0xa2;
const VK_MENU: u16 = 0x12;
const VK_RMENU: u16 = 0xa5;
const SCAN_CTRL: u16 = 0x1d;

/// Where a key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The low-level hook.
    Hook,
    /// A window message, used when the hook is silent (the watchdog).
    LegacyMessage,
}

/// One key as Windows reports it, before any mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawKey {
    pub vk: u16,
    pub scan: u16,
    pub prefix: Prefix,
    pub press: Press,
    pub time_ms: u32,
    pub origin: Origin,
    /// `LLKHF_INJECTED`: another program sent this key.
    pub injected: bool,
}

impl RawKey {
    fn is_left_ctrl(&self) -> bool {
        self.vk == VK_LCONTROL && self.scan == SCAN_CTRL && self.prefix == Prefix::None
    }

    fn is_right_alt(&self) -> bool {
        (self.vk == VK_RMENU || self.vk == VK_MENU)
            && self.prefix == Prefix::E0
            && self.scan == 0x38
    }
}

/// What to do with a pushed key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filtered {
    /// Forward these keys, in order (possibly none: the key was the fake).
    Forward(Vec<RawKey>),
    /// The key is held back until [`AltGrFilter::expire`] or the next key.
    Hold,
}

/// See the module documentation.
#[derive(Debug, Default)]
pub struct AltGrFilter {
    held: Option<RawKey>,
    /// A fake Ctrl Down was dropped, so its Up is dropped too.
    fake_ctrl: bool,
    right_alt_down: bool,
    right_alt_up_ms: Option<u32>,
}

impl AltGrFilter {
    pub fn push(&mut self, key: RawKey) -> Filtered {
        let mut out = Vec::new();
        if let Some(held) = self.held.take() {
            let alt_follows = key.press == Press::Down
                && key.is_right_alt()
                && key.time_ms.wrapping_sub(held.time_ms) <= DEADLINE_MS;
            if alt_follows {
                self.fake_ctrl = true;
            } else {
                self.fake_ctrl = false;
                out.push(held);
            }
        }
        if key.press == Press::Down && key.is_left_ctrl() {
            self.held = Some(key);
            return if out.is_empty() {
                Filtered::Hold
            } else {
                Filtered::Forward(out)
            };
        }
        if key.is_right_alt() {
            match key.press {
                Press::Down => self.right_alt_down = true,
                Press::Up => {
                    self.right_alt_down = false;
                    self.right_alt_up_ms = Some(key.time_ms);
                }
            }
        }
        if key.press == Press::Up && key.is_left_ctrl() && self.fake_ctrl && self.drops_up(&key) {
            self.fake_ctrl = false;
            return Filtered::Forward(out);
        }
        out.push(key);
        Filtered::Forward(out)
    }

    /// The fake Ctrl's Up arrives with AltGr still held or just released.
    fn drops_up(&self, key: &RawKey) -> bool {
        self.right_alt_down
            || self
                .right_alt_up_ms
                .is_some_and(|t| key.time_ms.wrapping_sub(t) <= DEADLINE_MS)
    }

    /// The held Ctrl, when its deadline has passed at `now_ms`.
    pub fn expire(&mut self, now_ms: u32) -> Option<RawKey> {
        let held = self.held?;
        if now_ms.wrapping_sub(held.time_ms) > DEADLINE_MS {
            self.held = None;
            self.fake_ctrl = false;
            return Some(held);
        }
        None
    }

    /// The held Ctrl at once, whatever the time: a click or a wheel step
    /// must not get ahead of a key that came before it.
    pub fn flush(&mut self) -> Option<RawKey> {
        self.fake_ctrl = false;
        self.held.take()
    }

    /// True while a key is held back (the caller should call `expire`).
    pub fn is_holding(&self) -> bool {
        self.held.is_some()
    }

    /// Forget everything: after focus loss, nothing is in flight.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests;
