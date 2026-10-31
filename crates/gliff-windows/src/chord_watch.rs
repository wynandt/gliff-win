//! The release chord and the tab-switch chords, decided on the hook thread.
//! Pure.
//!
//! The release chord used to be seen only by the session, after the hook had
//! sent it the key: hook thread, session task, UI event, UI thread. While the
//! session was busy or not yet connected (a slow ssh), the hook already
//! swallowed every key and the chord did nothing. So the hook watches for it
//! itself and takes the keyboard back at once ([`HookGate::chord_fired`]); the
//! session and the UI catch up afterwards. The switch chords are decided in
//! the same place, by the same key tracking, so that they too work whichever
//! way the modifiers were pressed and do not wait for a session.
//!
//! [`ChordWatch`] sees every key the hook sees, including the ones that were
//! already down when the grab began (which the session never sees). It returns
//! one [`Decision`]: **Release** or **Switch**. They have different aftermaths:
//! a release turns the grab off, a switch leaves it on. [`HookGate`] is the
//! mode the hook acts on, with the one rule that a chord that fired is not
//! undone by a window event that is still on its way to the UI thread.

use crate::altgr::DEADLINE_MS;
use crate::altgr::RawKey;
use crate::grab_state::HookMode;
use crate::keymap::Chord;
use crate::keymap::Evdev;
use crate::keymap::KeyEvent;
use crate::keymap::KeyState;
use crate::keymap::ModifierSet;
use crate::keymap::Prefix;
use crate::keymap::Press;
use crate::keymap::evdev;
use crate::tabs::SwitchTo;

const VK_LCONTROL: u16 = 0xa2;
const VK_MENU: u16 = 0x12;
const VK_RMENU: u16 = 0xa5;
const SCAN_CTRL: u16 = 0x1d;
const SCAN_ALT: u16 = 0x38;

/// The modifier keys `GetAsyncKeyState` can seed, by virtual key.
const SEEDABLE: [(u16, Evdev); 8] = [
    (0xa0, Evdev::LEFT_SHIFT),
    (0xa1, Evdev::RIGHT_SHIFT),
    (0xa2, Evdev::LEFT_CTRL),
    (0xa3, Evdev::RIGHT_CTRL),
    (0xa4, Evdev::LEFT_ALT),
    (0xa5, Evdev::RIGHT_ALT),
    (0x5b, Evdev::LEFT_META),
    (0x5c, Evdev::RIGHT_META),
];

/// The switch chords: each can be off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchChords {
    pub next: Option<Chord>,
    pub previous: Option<Chord>,
    /// The modifiers of the jump chords; the triggers are the main-row digits
    /// 1 to 9 (not the keypad).
    pub jump: Option<ModifierSet>,
}

impl SwitchChords {
    /// No switch chord at all.
    pub const NONE: Self = Self {
        next: None,
        previous: None,
        jump: None,
    };

    /// The defaults: Ctrl+Alt+PageDown, Ctrl+Alt+PageUp, Ctrl+Alt+1 to 9.
    pub fn standard() -> Self {
        Self {
            next: Some(Chord::ctrl_alt_page_down()),
            previous: Some(Chord::ctrl_alt_page_up()),
            jump: Some(ModifierSet::CTRL.with(ModifierSet::ALT)),
        }
    }
}

/// What a completed chord asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Take the keyboard back: the grab goes off.
    Release,
    /// Show another tab: the grab stays on.
    Switch(SwitchTo),
}

/// A jump chord: the modifiers and the position it selects.
#[derive(Debug)]
struct Jump {
    chord: Chord,
    place: u8,
}

fn jump_chords(modifiers: ModifierSet) -> Vec<Jump> {
    if modifiers.is_empty() {
        // A bare digit would take every digit key.
        return Vec::new();
    }
    (1..=9u8)
        .filter_map(|place| {
            let text = format!("{modifiers}+{place}");
            Some(Jump {
                chord: text.parse().ok()?,
                place,
            })
        })
        .collect()
}

/// Tracks the keys that are down and says when a chord completes.
#[derive(Debug)]
pub struct ChordWatch {
    release: Chord,
    next: Option<Chord>,
    previous: Option<Chord>,
    jump: Vec<Jump>,
    keys: KeyState,
    /// When the last left Ctrl Down came, for the fake Ctrl of AltGr.
    ctrl_down_ms: Option<u32>,
}

impl ChordWatch {
    pub fn new(release: Chord, switch: SwitchChords) -> Self {
        let mut watch = Self {
            release,
            next: None,
            previous: None,
            jump: Vec::new(),
            keys: KeyState::default(),
            ctrl_down_ms: None,
        };
        watch.set_chords(watch.release.clone(), switch);
        watch
    }

    /// The watch for a hook installed again in the middle of a grab: starts
    /// from the modifiers Windows holds down, unless the hook is off.
    pub fn after_reinstall(
        release: Chord,
        switch: SwitchChords,
        mode: HookMode,
        held: impl IntoIterator<Item = u16>,
    ) -> Self {
        let mut watch = Self::new(release, switch);
        if mode != HookMode::Off {
            watch.seed(held);
        }
        watch
    }

    /// Changes the chords and keeps the keys that are down.
    pub fn set_chords(&mut self, release: Chord, switch: SwitchChords) {
        self.release = release;
        self.next = switch.next;
        self.previous = switch.previous;
        self.jump = switch.jump.map(jump_chords).unwrap_or_default();
    }

    /// Forgets every key. Call when the hook stops looking.
    pub fn clear(&mut self) {
        self.keys = KeyState::default();
        self.ctrl_down_ms = None;
    }

    /// Starts again from the modifiers Windows holds down now, given by
    /// virtual key (the left and right ones; other keys are not seeded).
    ///
    /// A left Ctrl held together with the right Alt is AltGr, whose Ctrl is
    /// Windows' fake one, and is dropped: `seed` has no timing to tell it from
    /// a real one, and a digit typed with AltGr must not switch tabs.
    pub fn seed(&mut self, held: impl IntoIterator<Item = u16>) {
        self.clear();
        for vk in held {
            if let Some((_, key)) = SEEDABLE.iter().find(|(v, _)| *v == vk) {
                self.keys.apply(KeyEvent {
                    key: *key,
                    press: Press::Down,
                });
            }
        }
        if self.keys.is_held(Evdev::RIGHT_ALT) {
            self.keys.apply(KeyEvent {
                key: Evdev::LEFT_CTRL,
                press: Press::Up,
            });
        }
    }

    /// Applies one key. `Some` when this key completed a chord; the trigger
    /// is then not a key to forward. The release chord is tested first.
    pub fn push(&mut self, key: &RawKey) -> Option<Decision> {
        let code = evdev(key.vk, key.scan, key.prefix)?;
        let event = KeyEvent {
            key: code,
            press: key.press,
        };
        if is_left_ctrl(key) && key.press == Press::Down {
            self.ctrl_down_ms = Some(key.time_ms);
        }
        if is_right_alt(key)
            && key.press == Press::Down
            && self
                .ctrl_down_ms
                .is_some_and(|t| key.time_ms.wrapping_sub(t) <= DEADLINE_MS)
        {
            // AltGr: the Ctrl that came just before is Windows' fake one.
            self.keys.apply(KeyEvent {
                key: Evdev::LEFT_CTRL,
                press: Press::Up,
            });
            self.ctrl_down_ms = None;
        }
        let decision = self.decide(event);
        self.keys.apply(event);
        decision
    }

    fn decide(&self, event: KeyEvent) -> Option<Decision> {
        if self.release.completes(&self.keys, event) {
            return Some(Decision::Release);
        }
        let switch = |chord: &Option<Chord>, to: SwitchTo| {
            chord
                .as_ref()
                .is_some_and(|chord| chord.completes(&self.keys, event))
                .then_some(Decision::Switch(to))
        };
        switch(&self.next, SwitchTo::Next)
            .or_else(|| switch(&self.previous, SwitchTo::Previous))
            .or_else(|| {
                self.jump
                    .iter()
                    .find(|jump| jump.chord.completes(&self.keys, event))
                    .map(|jump| Decision::Switch(SwitchTo::Position(jump.place)))
            })
    }
}

fn is_left_ctrl(key: &RawKey) -> bool {
    key.vk == VK_LCONTROL && key.scan == SCAN_CTRL && key.prefix == Prefix::None
}

fn is_right_alt(key: &RawKey) -> bool {
    (key.vk == VK_RMENU || key.vk == VK_MENU) && key.prefix == Prefix::E0 && key.scan == SCAN_ALT
}

/// A tab switch as the word of the window message the hook posts to the UI
/// thread: 1 next, 2 previous, 0x100 + place for a jump.
pub fn switch_word(to: SwitchTo) -> usize {
    match to {
        SwitchTo::Next => 1,
        SwitchTo::Previous => 2,
        SwitchTo::Position(place) => 0x100 + usize::from(place),
    }
}

/// The switch a posted word asks for; `None` for a word that is none of
/// them (another program cannot post to the window, but the answer is still
/// total).
pub fn switch_of_word(word: usize) -> Option<SwitchTo> {
    match word {
        1 => Some(SwitchTo::Next),
        2 => Some(SwitchTo::Previous),
        0x101..=0x1ff => u8::try_from(word - 0x100).ok().map(SwitchTo::Position),
        _ => None,
    }
}

/// What the hook does with a key, once the watch has said whether a chord
/// completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// Go on with the mode's own rules (forward the key while grabbing).
    Normal,
    /// The release chord: the keyboard goes back to Windows, from the hook.
    Release,
    /// A switch chord: post the switch and swallow the key.
    Switch(SwitchTo),
    /// The rest of a switch chord's trigger (auto-repeat and its Up): the
    /// key is taken, so the tab just switched to does not see a stray key.
    Swallow,
    /// No session to send keys to: Windows keeps the keyboard.
    Pass,
}

/// Remembers the trigger of a switch chord until its Up, and applies the
/// no-session rule. Locked with the hook's other state.
#[derive(Debug, Default)]
pub struct Taker {
    taken: Option<u16>,
}

impl Taker {
    pub const fn new() -> Self {
        Self { taken: None }
    }

    /// `has_target`: the input route points at a session. A release with no
    /// session is not taken: nothing would tell the UI thread, and the
    /// hook's gate would stay shut. A switch is taken whether or not there is
    /// a session: that is how a tab whose session ended is left.
    pub fn fate(
        &mut self,
        mode: HookMode,
        has_target: bool,
        decision: Option<Decision>,
        vk: u16,
        press: Press,
    ) -> Fate {
        if mode != HookMode::Grab {
            // The grab ended with a trigger held: its Up goes to Windows, and
            // the next Down must not be taken.
            self.taken = None;
        }
        if self.taken == Some(vk) {
            if press == Press::Up {
                self.taken = None;
            }
            return Fate::Swallow;
        }
        match decision {
            Some(Decision::Release) if mode == HookMode::Grab && has_target => {
                return Fate::Release;
            }
            Some(Decision::Switch(to)) if mode == HookMode::Grab => {
                self.taken = Some(vk);
                return Fate::Switch(to);
            }
            _ => {}
        }
        if has_target { Fate::Normal } else { Fate::Pass }
    }
}

/// How many UI ticks a fired release chord waits for its session's answer
/// before the UI thread takes it in without one.
pub const OVERDUE_TICKS: u8 = 3;

/// The mode the hook acts on.
#[derive(Debug)]
pub struct HookGate {
    mode: HookMode,
    /// The hook fired the release chord and the UI thread has not caught up
    /// yet.
    fired: bool,
    /// UI ticks seen since the chord fired without an acknowledgement.
    waited: u8,
}

impl HookGate {
    pub const fn new() -> Self {
        Self {
            mode: HookMode::Off,
            fired: false,
            waited: 0,
        }
    }

    pub fn mode(&self) -> HookMode {
        self.mode
    }

    /// The UI thread wants `mode`. Returns the mode the hook now has: after a
    /// chord that fired, a request for a grab is refused until
    /// [`Self::acknowledge`], so a focus or hover event that was already on
    /// its way cannot undo the release.
    pub fn request(&mut self, mode: HookMode) -> HookMode {
        self.mode = if self.fired && mode == HookMode::Grab {
            HookMode::Off
        } else {
            mode
        };
        self.mode
    }

    /// The hook saw the release chord: the keyboard goes back to Windows now.
    pub fn chord_fired(&mut self) {
        self.mode = HookMode::Off;
        self.fired = true;
        self.waited = 0;
    }

    /// The UI thread has handled the chord, so its own state no longer wants
    /// a grab.
    pub fn acknowledge(&mut self) {
        self.fired = false;
        self.waited = 0;
    }

    /// One UI tick. True once a fired chord has gone [`OVERDUE_TICKS`] ticks
    /// without [`Self::acknowledge`]. The session normally answers the
    /// chord within a tick or two, and the UI thread acknowledges then. A
    /// session that cannot answer (it has ended, or its tab is no longer
    /// shown) would leave the gate refusing every grab for good, so the UI
    /// thread takes the chord in itself when this says so.
    pub fn overdue(&mut self) -> bool {
        if !self.fired {
            return false;
        }
        self.waited = self.waited.saturating_add(1);
        self.waited >= OVERDUE_TICKS
    }
}

impl Default for HookGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
