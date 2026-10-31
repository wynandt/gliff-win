//! Super+L in the hover-only hook mode: with a Win key
//! held, the `L` press and release are swallowed and Super+L is sent to the
//! remote machine. Every other key, a Win key alone included, goes to
//! Windows untouched. Pure.

use crate::keymap::Evdev;
use crate::keymap::KeyEvent;
use crate::keymap::Press;

const VK_LWIN: u16 = 0x5b;
const VK_RWIN: u16 = 0x5c;
const VK_L: u16 = 0x4c;

/// What the hook does with a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Let Windows have it.
    Pass,
    /// Swallow it; nothing to send.
    Swallow,
    /// Swallow it, send these events, and inject the masking tap so the Win
    /// key's release does not open the Start menu.
    SendSuperL(Vec<KeyEvent>),
}

/// Tracks the Win keys and the swallowed `L`.
#[derive(Debug, Default)]
pub struct SuperLTracker {
    left_win: bool,
    right_win: bool,
    /// True from the swallowed L Down to its Up, so the auto-repeats are
    /// swallowed without sending again and the Up is swallowed too.
    swallowing_l: bool,
}

impl SuperLTracker {
    pub fn push(&mut self, vk: u16, press: Press) -> Decision {
        match (vk, press) {
            (VK_LWIN, _) => self.left_win = press == Press::Down,
            (VK_RWIN, _) => self.right_win = press == Press::Down,
            (VK_L, Press::Down) if self.swallowing_l => return Decision::Swallow,
            (VK_L, Press::Down) if self.left_win || self.right_win => {
                self.swallowing_l = true;
                return Decision::SendSuperL(super_l_events());
            }
            (VK_L, Press::Up) if self.swallowing_l => {
                self.swallowing_l = false;
                return Decision::Swallow;
            }
            _ => {}
        }
        Decision::Pass
    }

    /// Forget everything, for a hook reinstall or a desktop switch.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Whether `vk` with a Win key held is the Super+L chord.
pub fn is_super_l(win_held: WinHeld, vk: u16) -> bool {
    win_held == WinHeld::Yes && vk == VK_L
}

/// Whether a Win key is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinHeld {
    Yes,
    No,
}

fn super_l_events() -> Vec<KeyEvent> {
    let event = |key, press| KeyEvent { key, press };
    vec![
        event(Evdev::LEFT_META, Press::Down),
        event(Evdev::KEY_L, Press::Down),
        event(Evdev::KEY_L, Press::Up),
        event(Evdev::LEFT_META, Press::Up),
    ]
}

#[cfg(test)]
mod tests;
