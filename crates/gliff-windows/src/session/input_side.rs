//! The session thread's input state: the key pipeline, the held buttons and
//! the wheel remainders, turned into protocol messages. Pure.
//!
//! The session thread is the only writer, so the held-key and held-button
//! sets are never shared; focus loss, the release chord, lock, the desktop
//! poll and Bye all end in [`InputSide::release_all`].

use gliff_proto::msg::Axis;
use gliff_proto::msg::ClientMsg;

use crate::altgr::RawKey;
use crate::input::KeyOutcome;
use crate::input::KeyPipeline;
use crate::keymap::KeyEvent;
use crate::keymap::Press;
use crate::pointer::ButtonState;
use crate::pointer::PointerEvent;
use crate::pointer::WheelAccumulator;

/// Messages to send, and whether the release chord completed.
#[derive(Debug, Default, PartialEq)]
pub struct KeyOut {
    pub msgs: Vec<ClientMsg>,
    /// The release chord completed: the UI must give the keyboard back.
    pub released: bool,
}

/// See the module documentation.
#[derive(Debug)]
pub struct InputSide {
    pipeline: KeyPipeline,
    buttons: ButtonState,
    wheel: WheelAccumulator,
}

impl InputSide {
    pub fn new(pipeline: KeyPipeline) -> Self {
        Self {
            pipeline,
            buttons: ButtonState::default(),
            wheel: WheelAccumulator::default(),
        }
    }

    /// One key from the hook (or the legacy-message watchdog).
    pub fn key(&mut self, key: RawKey) -> KeyOut {
        outcome(self.pipeline.push(key))
    }

    /// Call on a timer while [`Self::holding`]: the AltGr filter's deadline.
    pub fn tick(&mut self, now_ms: u32) -> KeyOut {
        outcome(self.pipeline.tick(now_ms))
    }

    /// Keys and buttons held now, as the server has been told.
    pub fn held(&self) -> (u32, u32) {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        (count(self.pipeline.keys_down()), count(self.buttons.len()))
    }

    /// True while a left Ctrl waits for the AltGr that may follow.
    pub fn holding(&self) -> bool {
        self.pipeline.is_holding()
    }

    /// Events already in evdev terms (Super+L from the hover hook); they
    /// bypass the held-key state because each set is balanced.
    pub fn keys(&mut self, events: Vec<KeyEvent>) -> Vec<ClientMsg> {
        events.into_iter().map(key_msg).collect()
    }

    /// A button press or release first forwards a Ctrl the AltGr filter still
    /// holds, so a Ctrl+click reaches the remote in that order. Motion does
    /// not: it carries no key meaning, and a mouse moving while AltGr is
    /// typed must not turn the AltGr's fake Ctrl real.
    pub fn pointer(&mut self, event: PointerEvent) -> Vec<ClientMsg> {
        match event {
            PointerEvent::Button { button, press } => {
                let mut msgs = self.flush_held();
                msgs.extend(self.buttons.apply(button, press).map(pointer_msg));
                msgs
            }
            other => vec![pointer_msg(other)],
        }
    }

    /// A raw wheel delta, in Windows units. Like a button, it first forwards
    /// a held Ctrl (Ctrl+wheel zooms).
    pub fn wheel(&mut self, axis: Axis, delta: i16) -> Vec<ClientMsg> {
        let mut msgs = self.flush_held();
        msgs.extend(self.wheel.push(axis, delta).into_iter().map(pointer_msg));
        msgs
    }

    fn flush_held(&mut self) -> Vec<ClientMsg> {
        self.pipeline
            .flush_held()
            .into_iter()
            .map(key_msg)
            .collect()
    }

    /// An Up for every modifier the user does not hold (see
    /// [`KeyPipeline::unheld_modifier_ups`]).
    pub fn clear_modifiers(&self) -> Vec<ClientMsg> {
        self.pipeline
            .unheld_modifier_ups()
            .into_iter()
            .map(key_msg)
            .collect()
    }

    /// An Up for every held key and button; the wheel remainders restart.
    pub fn release_all(&mut self) -> Vec<ClientMsg> {
        self.wheel.reset();
        let mut msgs: Vec<ClientMsg> = self
            .pipeline
            .release_all()
            .into_iter()
            .map(key_msg)
            .collect();
        msgs.extend(self.buttons.release_all().into_iter().map(pointer_msg));
        msgs
    }
}

fn outcome(outcome: KeyOutcome) -> KeyOut {
    match outcome {
        KeyOutcome::Send(events) => KeyOut {
            msgs: events.into_iter().map(key_msg).collect(),
            released: false,
        },
        KeyOutcome::ReleaseGrab(events) => KeyOut {
            msgs: events.into_iter().map(key_msg).collect(),
            released: true,
        },
        KeyOutcome::Nothing => KeyOut::default(),
    }
}

fn key_msg(event: KeyEvent) -> ClientMsg {
    ClientMsg::Key {
        keycode: u32::from(event.key.0),
        pressed: event.press == Press::Down,
    }
}

fn pointer_msg(event: PointerEvent) -> ClientMsg {
    match event {
        PointerEvent::Motion { x, y } => ClientMsg::PointerMotion { x, y },
        PointerEvent::Button { button, press } => ClientMsg::PointerButton {
            button,
            pressed: press == Press::Down,
        },
        PointerEvent::Axis {
            axis,
            value,
            discrete,
            stop,
        } => ClientMsg::PointerAxis {
            axis,
            value,
            discrete,
            stop,
        },
    }
}

#[cfg(test)]
mod tests;
