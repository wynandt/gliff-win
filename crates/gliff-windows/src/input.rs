//! The session's one key owner: a raw Windows key goes in, evdev events come
//! out. Pure. The steps are the AltGr filter, the policy for keys that stay
//! on Windows (injected keys and media keys), the table, the release chord,
//! and the held-key state.

use crate::altgr::AltGrFilter;
use crate::altgr::Filtered;
use crate::altgr::RawKey;
use crate::keymap::Chord;
use crate::keymap::Evdev;
use crate::keymap::KeyEvent;
use crate::keymap::KeyState;
use crate::keymap::Press;
use crate::keymap::evdev;

/// What to do with keys that another program injected (decision 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum InjectedKeys {
    /// Forward them like typed keys (password-manager auto-type).
    Forward,
    /// Leave them to Windows.
    Local,
}

/// What to do with volume and media keys (decision 7 default: stay local).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum MediaKeys {
    /// They change the Windows volume and are not forwarded.
    Local,
    /// They go to the remote machine.
    Forward,
}

/// How Super+L reaches the remote machine (`--super-l`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SuperL {
    /// Opt in: hold Windows' lock setting off while the window is focused or
    /// hovered, so the real Win+L is forwarded. Windows locking, including the
    /// Ctrl+Alt+Del Lock entry and the Start menu, is off meanwhile.
    Registry,
    /// The stand-in chord only (`--super-l-chord`, Ctrl+Alt+L by default); the
    /// registry is not touched. The default.
    Chord,
    /// Nothing: Windows locks as usual.
    Off,
}

/// Which keys the grab forwards, shared by the hook (to decide what to
/// swallow) and the pipeline (to decide what to send).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPolicy {
    pub injected: InjectedKeys,
    pub media: MediaKeys,
}

impl Default for KeyPolicy {
    fn default() -> Self {
        Self {
            injected: InjectedKeys::Forward,
            media: MediaKeys::Local,
        }
    }
}

/// `VK_VOLUME_MUTE` to `VK_MEDIA_PLAY_PAUSE`.
const MEDIA_VKS: std::ops::RangeInclusive<u16> = 0xad..=0xb3;

impl KeyPolicy {
    /// False when `key` stays on Windows: the hook passes it on and the
    /// pipeline ignores it.
    pub fn forwards(&self, key: &RawKey) -> bool {
        if key.injected && self.injected == InjectedKeys::Local {
            return false;
        }
        if MEDIA_VKS.contains(&key.vk) && self.media == MediaKeys::Local {
            return false;
        }
        true
    }
}

/// What the command line decides about keys, handed to the session.
#[derive(Clone, Debug)]
pub struct InputOptions {
    /// The XKB keymap text for `Hello` (empty: the server keeps its own).
    pub keymap: String,
    pub release_chord: Chord,
    /// The Super+L stand-in chord, when it is on.
    pub stand_in: Option<Chord>,
    pub policy: KeyPolicy,
}

impl Default for InputOptions {
    fn default() -> Self {
        Self {
            keymap: String::new(),
            release_chord: Chord::ctrl_alt_home(),
            stand_in: None,
            policy: KeyPolicy::default(),
        }
    }
}

impl InputOptions {
    pub fn pipeline(&self) -> KeyPipeline {
        let pipeline = KeyPipeline::new(self.release_chord.clone(), self.policy);
        match &self.stand_in {
            Some(chord) => pipeline.with_stand_in(chord.clone()),
            None => pipeline,
        }
    }
}

/// What the session does with a pushed key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Send these events.
    Send(Vec<KeyEvent>),
    /// The release chord completed: send these Ups (every held key) and give
    /// the keyboard back to Windows.
    ReleaseGrab(Vec<KeyEvent>),
    /// Nothing to send.
    Nothing,
}

/// See the module documentation.
#[derive(Debug)]
pub struct KeyPipeline {
    altgr: AltGrFilter,
    keys: KeyState,
    chord: Chord,
    policy: KeyPolicy,
    stand_in: Option<Chord>,
    /// The key whose Down completed the stand-in chord and has not had its
    /// Up yet, so its auto-repeats do not send Super+L again.
    stand_in_trigger: Option<Evdev>,
}

impl KeyPipeline {
    pub fn new(chord: Chord, policy: KeyPolicy) -> Self {
        Self {
            altgr: AltGrFilter::default(),
            keys: KeyState::default(),
            chord,
            policy,
            stand_in: None,
            stand_in_trigger: None,
        }
    }

    /// Turns on the Super+L stand-in chord (see [`Self::stand_in`]).
    pub fn with_stand_in(mut self, chord: Chord) -> Self {
        self.stand_in = Some(chord);
        self
    }

    pub fn push(&mut self, key: RawKey) -> KeyOutcome {
        if !self.policy.forwards(&key) {
            return KeyOutcome::Nothing;
        }
        match self.altgr.push(key) {
            Filtered::Hold => KeyOutcome::Nothing,
            Filtered::Forward(keys) => self.run(keys),
        }
    }

    /// Call on a timer while [`Self::is_holding`]: forwards a Ctrl whose
    /// AltGr never came.
    pub fn tick(&mut self, now_ms: u32) -> KeyOutcome {
        match self.altgr.expire(now_ms) {
            Some(key) => self.run(vec![key]),
            None => KeyOutcome::Nothing,
        }
    }

    pub fn is_holding(&self) -> bool {
        self.altgr.is_holding()
    }

    /// How many keys are held down on the remote side of the pipeline.
    pub fn keys_down(&self) -> usize {
        self.keys.len()
    }

    /// Forwards a Ctrl still waiting for an AltGr, now. A pointer button or a
    /// wheel step calls this first: the remote must see the Ctrl before the
    /// click it modifies, not after the filter's deadline.
    pub fn flush_held(&mut self) -> Vec<KeyEvent> {
        match self.altgr.flush().map(|key| self.run(vec![key])) {
            Some(KeyOutcome::Send(events) | KeyOutcome::ReleaseGrab(events)) => events,
            Some(KeyOutcome::Nothing) | None => Vec::new(),
        }
    }

    /// An Up for every held key (focus loss, lock, desktop switch, Bye).
    pub fn release_all(&mut self) -> Vec<KeyEvent> {
        self.altgr.reset();
        self.stand_in_trigger = None;
        self.keys.release_all()
    }

    /// An Up for every modifier the user does not hold, for a remote that
    /// may have missed one while it stalled. Nothing here changes what the
    /// pipeline holds.
    pub fn unheld_modifier_ups(&self) -> Vec<KeyEvent> {
        MODIFIERS
            .into_iter()
            .filter(|&key| !self.keys.is_held(key))
            .map(|key| KeyEvent {
                key,
                press: Press::Up,
            })
            .collect()
    }

    fn run(&mut self, raw: Vec<RawKey>) -> KeyOutcome {
        let mut sent = Vec::new();
        for key in raw {
            let Some(code) = evdev(key.vk, key.scan, key.prefix) else {
                continue;
            };
            let event = KeyEvent {
                key: code,
                press: key.press,
            };
            if self.chord.completes(&self.keys, event) {
                sent.extend(self.keys.release_all());
                return KeyOutcome::ReleaseGrab(sent);
            }
            if self.stand_in_swallows(event) {
                continue;
            }
            if self
                .stand_in
                .as_ref()
                .is_some_and(|c| c.completes(&self.keys, event))
            {
                self.stand_in_trigger = Some(event.key);
                sent.extend(self.super_l());
                continue;
            }
            sent.extend(self.keys.apply(event));
        }
        if sent.is_empty() {
            KeyOutcome::Nothing
        } else {
            KeyOutcome::Send(sent)
        }
    }
}

impl KeyPipeline {
    /// True for the auto-repeats and the Up of the stand-in chord's trigger,
    /// which the remote never saw go down.
    fn stand_in_swallows(&mut self, event: KeyEvent) -> bool {
        if self.stand_in_trigger != Some(event.key) {
            return false;
        }
        if event.press == Press::Up {
            self.stand_in_trigger = None;
        }
        true
    }

    /// The stand-in chord: release the modifiers the remote holds, send
    /// Super+L, and press the modifiers again, so the remote's idea of the
    /// keyboard is what it was before.
    fn super_l(&mut self) -> Vec<KeyEvent> {
        let held = self.keys.held_among(&MODIFIERS);
        let event = |key, press| KeyEvent { key, press };
        let mut out: Vec<KeyEvent> = held.iter().map(|&k| event(k, Press::Up)).collect();
        out.extend([
            event(Evdev::LEFT_META, Press::Down),
            event(Evdev::KEY_L, Press::Down),
            event(Evdev::KEY_L, Press::Up),
            event(Evdev::LEFT_META, Press::Up),
        ]);
        out.extend(held.iter().map(|&k| event(k, Press::Down)));
        out
    }
}

/// Every modifier, both sides.
const MODIFIERS: [Evdev; 8] = [
    Evdev::LEFT_CTRL,
    Evdev::RIGHT_CTRL,
    Evdev::LEFT_SHIFT,
    Evdev::RIGHT_SHIFT,
    Evdev::LEFT_ALT,
    Evdev::RIGHT_ALT,
    Evdev::LEFT_META,
    Evdev::RIGHT_META,
];

#[cfg(test)]
mod tests;
