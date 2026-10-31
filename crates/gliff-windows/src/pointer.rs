//! Windows mouse input to the protocol's pointer events. Pure.

use gliff_proto::msg::Axis;

use crate::keymap::Press;
use std::collections::BTreeSet;

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// What a wheel notch is worth in Windows units (`WHEEL_DELTA`).
pub const WHEEL_DELTA: i32 = 120;
/// What one step is worth in the protocol, as upstream sends for one wheel
/// click (`value = 15 * direction`).
const STEP_VALUE: f64 = 15.0;

const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_XBUTTONDOWN: u32 = 0x020b;
const WM_XBUTTONUP: u32 = 0x020c;

/// Which evdev codes the two thumb buttons send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SideButtons {
    /// Upstream's: back is `BTN_BACK` 0x116, forward is `BTN_FORWARD` 0x115.
    Upstream,
    /// What Linux mice send for the thumb buttons: `BTN_SIDE` 0x113 and
    /// `BTN_EXTRA` 0x114.
    LinuxMouse,
}

/// One pointer event for the protocol.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        press: Press,
    },
    Axis {
        axis: Axis,
        value: f64,
        discrete: Option<i32>,
        stop: bool,
    },
}

/// A mouse-button message to an evdev button. `None` for unknown buttons
/// (upstream would send them as left clicks).
pub fn button(msg: u32, xbutton: u16, codes: SideButtons) -> Option<(u32, Press)> {
    let side = |first: u32, second: u32| match xbutton {
        1 => Some(first),
        2 => Some(second),
        _ => None,
    };
    let (code, press) = match msg {
        WM_LBUTTONDOWN => (Some(BTN_LEFT), Press::Down),
        WM_LBUTTONUP => (Some(BTN_LEFT), Press::Up),
        WM_RBUTTONDOWN => (Some(BTN_RIGHT), Press::Down),
        WM_RBUTTONUP => (Some(BTN_RIGHT), Press::Up),
        WM_MBUTTONDOWN => (Some(BTN_MIDDLE), Press::Down),
        WM_MBUTTONUP => (Some(BTN_MIDDLE), Press::Up),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let code = match codes {
                SideButtons::Upstream => side(0x116, 0x115),
                SideButtons::LinuxMouse => side(0x113, 0x114),
            };
            let press = if msg == WM_XBUTTONDOWN {
                Press::Down
            } else {
                Press::Up
            };
            (code, press)
        }
        _ => return None,
    };
    code.map(|code| (code, press))
}

/// The buttons held, so a release can be sent for each on focus loss and the
/// mouse capture can be taken on the first Down and dropped on the last Up.
#[derive(Debug, Default)]
pub struct ButtonState {
    held: BTreeSet<u32>,
}

impl ButtonState {
    /// `None` for a Down of a held button and an Up of a button not held.
    pub fn apply(&mut self, button: u32, press: Press) -> Option<PointerEvent> {
        let changed = match press {
            Press::Down => self.held.insert(button),
            Press::Up => self.held.remove(&button),
        };
        changed.then_some(PointerEvent::Button { button, press })
    }

    /// How many buttons are held.
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// An Up for every held button.
    pub fn release_all(&mut self) -> Vec<PointerEvent> {
        std::mem::take(&mut self.held)
            .into_iter()
            .map(|button| PointerEvent::Button {
                button,
                press: Press::Up,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

/// Wheel deltas to whole steps, with the remainder carried over (decision 10).
/// One notch (120) is one step, the same as upstream sends for one GTK wheel
/// click. A touchpad's small deltas add up to a step per full notch.
#[derive(Debug, Default)]
pub struct WheelAccumulator {
    rest: [i32; 2],
}

impl WheelAccumulator {
    /// Adds `delta` (Windows units; positive is away from the user for the
    /// vertical wheel and right for the horizontal one) and returns one event
    /// for every whole notch crossed. The vertical direction is negated,
    /// since Wayland's positive vertical is down.
    pub fn push(&mut self, axis: Axis, delta: i16) -> Vec<PointerEvent> {
        let (slot, sign) = match axis {
            Axis::Vertical => (0, -1),
            Axis::Horizontal => (1, 1),
        };
        self.rest[slot] += i32::from(delta);
        let steps = self.rest[slot] / WHEEL_DELTA;
        self.rest[slot] -= steps * WHEEL_DELTA;
        let direction = steps.signum() * sign;
        (0..steps.abs())
            .map(|_| PointerEvent::Axis {
                axis,
                value: STEP_VALUE * f64::from(direction),
                discrete: Some(direction),
                stop: false,
            })
            .collect()
    }

    /// The remainder of an axis, in Windows units (tests and the log).
    pub fn rest(&self, axis: Axis) -> i32 {
        match axis {
            Axis::Vertical => self.rest[0],
            Axis::Horizontal => self.rest[1],
        }
    }

    /// Forget the remainders, for a focus change.
    pub fn reset(&mut self) {
        self.rest = [0; 2];
    }
}

#[cfg(test)]
mod tests;
