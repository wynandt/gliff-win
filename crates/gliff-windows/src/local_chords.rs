//! Chords that stay on Windows while the keyboard is grabbed.
//!
//! A grab forwards every key, so Win+Shift+S (the snipping overlay) would
//! reach the remote machine and Windows would never see it. A *local chord*
//! is a key combination the hook keeps for Windows: when its trigger key goes
//! down with exactly its modifiers held, the remote machine is told to let go
//! of everything it holds, and the hook replays the chord to Windows, which
//! has not seen the modifiers go down because the grab swallowed them. Pure;
//! the Win32 calls are in `hook`.

use std::fmt;
use std::str::FromStr;

use crate::keymap::ModifierSet;
use crate::keymap::Press;
use crate::modifiers::MODIFIERS;
use crate::modifiers::VK_LCONTROL;
use crate::modifiers::VK_LMENU;
use crate::modifiers::VK_LSHIFT;
use crate::modifiers::VK_LWIN;
use crate::modifiers::VK_RCONTROL;
use crate::modifiers::VK_RMENU;
use crate::modifiers::VK_RSHIFT;
use crate::modifiers::VK_RWIN;

const CTRL: u8 = ModifierSet::CTRL.bits();
const ALT: u8 = ModifierSet::ALT.bits();
const SHIFT: u8 = ModifierSet::SHIFT.bits();
const WIN: u8 = ModifierSet::WIN.bits();

/// The modifier family of a key, as a bit, or 0 for any other key.
fn family(vk: u16) -> u8 {
    match vk {
        VK_LCONTROL | VK_RCONTROL => CTRL,
        VK_LMENU | VK_RMENU => ALT,
        VK_LSHIFT | VK_RSHIFT => SHIFT,
        VK_LWIN | VK_RWIN => WIN,
        _ => 0,
    }
}

/// Why a chord list did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocalChordError {
    #[error("the chord is empty")]
    Empty,
    #[error("a chord needs at least one modifier (ctrl, alt, shift, win) before its key")]
    NoModifier,
    #[error("a chord ends in one key, not a modifier: {0:?}")]
    ModifierTrigger(String),
    #[error("unknown key {0:?} in the chord")]
    UnknownKey(String),
}

/// A chord such as `Win+Shift+S`: modifiers of either side, then one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalChord {
    modifiers: u8,
    trigger: u16,
}

impl LocalChord {
    /// Win+Shift+S, the snipping overlay.
    pub fn snip() -> Self {
        "win+shift+s".parse().expect("the default chord parses")
    }

    /// A chord with any trigger key, for tests of code that must not trust
    /// that the parser only makes chords it can name.
    #[cfg(test)]
    pub(crate) fn from_parts(modifiers: ModifierSet, trigger: u16) -> Self {
        Self {
            modifiers: modifiers.bits(),
            trigger,
        }
    }

    /// The modifier families that must be held, and no others.
    pub fn modifier_set(&self) -> ModifierSet {
        ModifierSet::from_bits(self.modifiers)
    }

    /// The word of the trigger key, as [`vk_by_name`] reads it.
    pub fn trigger_name(&self) -> String {
        match self.trigger {
            vk @ (0x41..=0x5a) => char::from(u8::try_from(vk).unwrap_or(b'?'))
                .to_ascii_lowercase()
                .to_string(),
            vk @ (0x30..=0x39) => char::from(u8::try_from(vk).unwrap_or(b'?')).to_string(),
            vk @ (0x70..=0x87) => format!("f{}", vk - 0x70 + 1),
            0x09 => "tab".to_owned(),
            0x0d => "enter".to_owned(),
            0x1b => "esc".to_owned(),
            0x20 => "space".to_owned(),
            vk => format!("vk{vk:#04x}"),
        }
    }
}

impl fmt::Display for LocalChord {
    /// `win+shift+s`: the canonical spelling, which parses back to this chord.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}+{}", self.modifier_set(), self.trigger_name())
    }
}

impl FromStr for LocalChord {
    type Err = LocalChordError;

    fn from_str(text: &str) -> Result<Self, LocalChordError> {
        let words: Vec<String> = text
            .split('+')
            .map(|w| w.trim().to_ascii_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        let Some((last, rest)) = words.split_last() else {
            return Err(LocalChordError::Empty);
        };
        let mut modifiers = 0;
        for word in rest {
            modifiers |= match word.as_str() {
                "ctrl" | "control" => CTRL,
                "alt" => ALT,
                "shift" => SHIFT,
                "win" | "super" | "meta" => WIN,
                other => return Err(LocalChordError::UnknownKey(other.to_owned())),
            };
        }
        if modifiers == 0 {
            return Err(LocalChordError::NoModifier);
        }
        if matches!(
            last.as_str(),
            "ctrl" | "control" | "alt" | "shift" | "win" | "super" | "meta"
        ) {
            return Err(LocalChordError::ModifierTrigger(last.clone()));
        }
        let trigger =
            vk_by_name(last).ok_or_else(|| LocalChordError::UnknownKey(last.to_owned()))?;
        Ok(Self { modifiers, trigger })
    }
}

/// The virtual key behind a trigger word: `a`..`z`, `0`..`9`, `f1`..`f24`,
/// `tab`, `space`, `enter`, `esc`.
fn vk_by_name(name: &str) -> Option<u16> {
    match name.as_bytes() {
        [c @ b'a'..=b'z'] => Some(u16::from(c.to_ascii_uppercase())),
        [c @ b'0'..=b'9'] => Some(u16::from(*c)),
        _ => match name {
            "tab" => Some(0x09),
            "enter" | "return" => Some(0x0d),
            "esc" | "escape" => Some(0x1b),
            "space" => Some(0x20),
            _ => {
                let number: u16 = name.strip_prefix('f')?.parse().ok()?;
                (1..=24).contains(&number).then(|| 0x70 + number - 1)
            }
        },
    }
}

/// The `--local-chords` value: a comma-separated list, or `none`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalChordList(pub Vec<LocalChord>);

impl Default for LocalChordList {
    fn default() -> Self {
        Self(vec![LocalChord::snip()])
    }
}

impl fmt::Display for LocalChordList {
    /// The comma-separated chords, or `none` for the empty list.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("none");
        }
        let chords: Vec<String> = self.0.iter().map(ToString::to_string).collect();
        f.write_str(&chords.join(","))
    }
}

impl FromStr for LocalChordList {
    type Err = LocalChordError;

    fn from_str(text: &str) -> Result<Self, LocalChordError> {
        if text.trim().eq_ignore_ascii_case("none") {
            return Ok(Self(Vec::new()));
        }
        text.split(',')
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }
}

/// What the hook does with a key of the grab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChordDecision {
    /// Not a local chord: the normal path (send it to the remote machine).
    Forward,
    /// The trigger of a local chord went down. The caller releases the remote
    /// machine's keys, replays `modifiers` (virtual keys, in [`MODIFIERS`]
    /// order) and the trigger to Windows, and swallows the real key.
    Take { modifiers: Vec<u16>, trigger: u16 },
    /// The auto-repeat or the Up of a taken trigger: swallow it. Windows saw
    /// a complete press of the trigger already, and the remote never saw it.
    Swallow,
}

/// Tracks the modifiers held in the grab and finds local chords.
#[derive(Debug, Default)]
pub struct LocalChords {
    chords: Vec<LocalChord>,
    held: [bool; MODIFIERS.len()],
    /// The trigger taken and not yet released.
    taking: Option<u16>,
}

impl LocalChords {
    pub fn new(chords: Vec<LocalChord>) -> Self {
        Self {
            chords,
            ..Self::default()
        }
    }

    /// Looks at one key of the grab. `local` is true for a key that was down
    /// in Windows before the grab (its life goes to Windows and the remote
    /// never sees it): it only updates the modifier state.
    pub fn push(&mut self, vk: u16, press: Press, local: bool) -> ChordDecision {
        if family(vk) != 0 {
            if let Some(slot) = MODIFIERS.iter().position(|m| m.vk == vk) {
                self.held[slot] = press == Press::Down;
            }
            return ChordDecision::Forward;
        }
        if self.taking == Some(vk) {
            if press == Press::Up {
                self.taking = None;
            }
            // A local Up (no session to send it to) ends the take too, or
            // the next press of the key would be swallowed.
            if !local {
                return ChordDecision::Swallow;
            }
        }
        if local || self.chords.is_empty() {
            return ChordDecision::Forward;
        }
        if press != Press::Down {
            return ChordDecision::Forward;
        }
        let held = self.held_modifiers();
        let bits = held.iter().fold(0, |bits, vk| bits | family(*vk));
        if self
            .chords
            .iter()
            .any(|c| c.trigger == vk && c.modifiers == bits)
        {
            self.taking = Some(vk);
            return ChordDecision::Take {
                modifiers: held,
                trigger: vk,
            };
        }
        ChordDecision::Forward
    }

    /// The modifier virtual keys held now, in [`MODIFIERS`] order.
    pub fn held_modifiers(&self) -> Vec<u16> {
        MODIFIERS
            .iter()
            .zip(self.held)
            .filter_map(|(m, held)| held.then_some(m.vk))
            .collect()
    }

    /// The chord just taken was not done: its trigger is a key like any
    /// other.
    pub fn cancel_take(&mut self) {
        self.taking = None;
    }

    /// Forget everything, for a mode change or a hook reinstall.
    pub fn reset(&mut self) {
        self.held = [false; MODIFIERS.len()];
        self.taking = None;
    }
}

#[cfg(test)]
mod tests;
