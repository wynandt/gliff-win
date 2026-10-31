//! The table against the keycodemapdb oracle, the Windows quirks, and the
//! chord and held-key logic.

use super::Chord;
use super::ChordError;
use super::Evdev;
use super::KeyEvent;
use super::KeyState;
use super::ModifierSet;
use super::ModifierSetError;
use super::Prefix;
use super::Press;
use super::VK_NUMLOCK;
use super::VK_PAUSE;
use super::evdev;

const FIXTURE: &str = include_str!("../../tests/fixtures/set1-evdev.tsv");

/// (scan, prefix, evdev, name) for every fixture row.
fn oracle() -> Vec<(u16, Prefix, u16, String)> {
    FIXTURE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|line| {
            let mut cols = line.split('\t');
            let set1 = cols.next().expect("a set-1 column");
            let code: u16 = cols
                .next()
                .expect("an evdev column")
                .parse()
                .expect("a decimal evdev code");
            let name = cols.next().unwrap_or("").to_string();
            let value =
                u16::from_str_radix(set1.trim_start_matches("0x"), 16).expect("a hex set-1 code");
            let prefix = if value > 0xff {
                Prefix::E0
            } else {
                Prefix::None
            };
            (value & 0xff, prefix, code, name)
        })
        .collect()
}

/// A virtual key that never disambiguates anything: `VK_OEM_CLEAR`.
const VK_ANY: u16 = 0xfe;

#[test]
fn table_matches_the_oracle_except_the_documented_overrides() {
    let mut checked = 0;
    for (scan, prefix, code, name) in oracle() {
        let expected = match (prefix, scan) {
            // E0 2A (a fake left Shift) is dropped on purpose. E0 36 is where
            // a real right Shift arrives, so it is `KEY_RIGHTSHIFT`, not the
            // oracle's `KEY_BASSBOOST`.
            (Prefix::E0, 0x2a) => None,
            (Prefix::E0, 0x36) => Some(Evdev::RIGHT_SHIFT),
            _ => Some(Evdev(code)),
        };
        assert_eq!(
            evdev(VK_ANY, scan, prefix),
            expected,
            "scan {scan:#04x} {prefix:?} ({name})"
        );
        checked += 1;
    }
    assert_eq!(checked, 235, "every oracle row was compared");
}

/// Measured with a hook probe on a built-in keyboard: a real right Shift is
/// vk 0xA1, scan 0x36, with the extended flag; a real left Shift is vk 0xA0,
/// scan 0x2A without it.
#[test]
fn a_real_right_shift_is_extended_and_is_forwarded() {
    assert_eq!(
        evdev(0xa1, 0x36, Prefix::E0),
        Some(Evdev::RIGHT_SHIFT),
        "right Shift as the probe saw it"
    );
    assert_eq!(
        evdev(0xa1, 0x36, Prefix::None),
        Some(Evdev::RIGHT_SHIFT),
        "right Shift on a keyboard that does not flag it"
    );
    assert_eq!(
        evdev(0xa0, 0x2a, Prefix::None),
        Some(Evdev::LEFT_SHIFT),
        "left Shift as the probe saw it"
    );
    assert_eq!(Evdev::RIGHT_SHIFT, Evdev(54), "KEY_RIGHTSHIFT");
}

/// Windows tags the fake shifts it makes around a keypad key with the
/// simulated bit, 0x200, in the scan code the hook reports: 0x22A for the
/// left Shift and 0x236 for the right one (documented, and what AutoHotkey
/// tests for; not measured on the probe keyboard, where Num Lock was off).
#[test]
fn shifts_tagged_as_simulated_are_dropped_with_or_without_the_extended_flag() {
    for prefix in [Prefix::None, Prefix::E0] {
        assert_eq!(
            evdev(0xa0, 0x22a, prefix),
            None,
            "fake left Shift, {prefix:?}"
        );
        assert_eq!(
            evdev(0xa1, 0x236, prefix),
            None,
            "fake right Shift, {prefix:?}"
        );
    }
}

#[test]
fn pause_and_num_lock_share_a_scan_code_and_the_vk_decides() {
    assert_eq!(
        evdev(VK_PAUSE, 0x45, Prefix::None),
        Some(Evdev(119)),
        "Pause"
    );
    assert_eq!(
        evdev(VK_NUMLOCK, 0x45, Prefix::None),
        Some(Evdev(69)),
        "Num Lock"
    );
    assert_eq!(
        evdev(VK_ANY, 0x45, Prefix::None),
        Some(Evdev(69)),
        "oracle default"
    );
    assert_eq!(
        evdev(VK_ANY, 0x45, Prefix::E0),
        Some(Evdev(221)),
        "E0 45 is the oracle's KEY_SHOP, not Pause or Num Lock"
    );
}

#[test]
fn windows_quirks() {
    assert_eq!(
        evdev(0x2c, 0x37, Prefix::E0),
        Some(Evdev::SYSRQ),
        "Print Screen"
    );
    assert_eq!(
        evdev(0x03, 0x46, Prefix::E0),
        Some(Evdev(119)),
        "Ctrl+Break"
    );
    assert_eq!(evdev(0xa0, 0x2a, Prefix::E0), None, "E0 2A is never a key");
    assert_eq!(
        evdev(0x41, 0, Prefix::None),
        None,
        "scan code 0: VK-only injection"
    );
    assert_eq!(
        evdev(0x41, 0x1ff, Prefix::None),
        None,
        "a scan code above one byte"
    );
}

#[test]
fn copilot_key_is_f23() {
    // The Copilot key is LWin + LShift + F23. The oracle's F23 is scan 0x6d;
    // a keyboard that reports another scan code falls back to the VK.
    assert_eq!(
        evdev(0x86, 0x6d, Prefix::None),
        Some(Evdev(193)),
        "KEY_F23 by scan"
    );
    assert_eq!(
        evdev(0x86, 0x6e, Prefix::None),
        Some(Evdev(193)),
        "KEY_F23 by VK"
    );
    assert_eq!(
        evdev(0x7c, 0x6e, Prefix::None),
        Some(Evdev(183)),
        "KEY_F13 by VK"
    );
    assert_eq!(
        evdev(0x87, 0x6e, Prefix::None),
        Some(Evdev(194)),
        "KEY_F24 by VK"
    );
    assert_eq!(
        evdev(0x88, 0x6e, Prefix::None),
        None,
        "VK_NAVIGATION_VIEW is not F25"
    );
}

#[test]
fn gate_chord_keys_are_all_mapped() {
    // The scan codes of the keys in the gate's 20 chords: letters, digits,
    // modifiers, Enter, arrows, F-keys, Home/End, Tab, Escape, Space.
    for scan in [
        0x1e, 0x30, 0x2e, 0x12, 0x1c, 0x39, 0x0f, 0x01, 0x02, 0x0b, 0x3b, 0x58, 0x1d, 0x2a, 0x38,
        0x36,
    ] {
        assert!(
            evdev(VK_ANY, scan, Prefix::None).is_some(),
            "scan {scan:#04x}"
        );
    }
    for (scan, name) in [
        (0x1d, "right ctrl"),
        (0x38, "right alt"),
        (0x5b, "left win"),
        (0x5c, "right win"),
        (0x47, "home"),
        (0x4f, "end"),
        (0x48, "up"),
        (0x50, "down"),
        (0x4b, "left"),
        (0x4d, "right"),
        (0x53, "delete"),
    ] {
        assert!(
            evdev(VK_ANY, scan, Prefix::E0).is_some(),
            "E0 {scan:#04x} {name}"
        );
    }
}

#[test]
fn modifiers_map_to_the_right_evdev_codes() {
    assert_eq!(evdev(0xa2, 0x1d, Prefix::None), Some(Evdev::LEFT_CTRL));
    assert_eq!(evdev(0xa3, 0x1d, Prefix::E0), Some(Evdev::RIGHT_CTRL));
    assert_eq!(evdev(0xa4, 0x38, Prefix::None), Some(Evdev::LEFT_ALT));
    assert_eq!(evdev(0xa5, 0x38, Prefix::E0), Some(Evdev::RIGHT_ALT));
    assert_eq!(evdev(0x5b, 0x5b, Prefix::E0), Some(Evdev::LEFT_META));
    assert_eq!(evdev(0x5c, 0x5c, Prefix::E0), Some(Evdev::RIGHT_META));
    assert_eq!(
        evdev(0x4c, 0x26, Prefix::None),
        Some(Evdev::KEY_L),
        "the L key"
    );
    assert_eq!(evdev(0x24, 0x47, Prefix::E0), Some(Evdev::HOME));
}

#[test]
fn evdev_is_total_over_every_input() {
    for scan in 0..=u16::MAX {
        for prefix in [Prefix::None, Prefix::E0] {
            for vk in [0, VK_PAUSE, VK_NUMLOCK, VK_ANY, u16::MAX] {
                if let Some(code) = evdev(vk, scan, prefix) {
                    assert!(
                        (1..=0x2ff).contains(&code.0),
                        "vk {vk:#x} scan {scan:#x} {prefix:?} gave {code:?}"
                    );
                }
            }
        }
    }
}

fn down(key: Evdev) -> KeyEvent {
    KeyEvent {
        key,
        press: Press::Down,
    }
}

fn up(key: Evdev) -> KeyEvent {
    KeyEvent {
        key,
        press: Press::Up,
    }
}

#[test]
fn key_state_drops_auto_repeat_and_stray_ups() {
    let mut state = KeyState::default();
    let a = Evdev(30);
    assert_eq!(state.apply(down(a)), Some(down(a)));
    assert_eq!(state.apply(down(a)), None, "auto-repeat is not sent twice");
    assert_eq!(state.apply(up(a)), Some(up(a)));
    assert_eq!(state.apply(up(a)), None, "an Up for a key never sent");
    assert!(state.is_empty(), "nothing held at the end");
}

#[test]
fn release_all_sends_one_up_per_held_key_and_empties_the_state() {
    let mut state = KeyState::default();
    state.apply(down(Evdev::LEFT_CTRL));
    state.apply(down(Evdev(30)));
    let ups = state.release_all();
    assert_eq!(ups, vec![up(Evdev::LEFT_CTRL), up(Evdev(30))]);
    assert!(state.is_empty(), "release_all empties the state");
    assert!(
        state.release_all().is_empty(),
        "a second call sends nothing"
    );
}

#[test]
fn default_chord_completes_on_home_with_either_ctrl_and_alt() {
    let chord = Chord::ctrl_alt_home();
    for (ctrl, alt) in [
        (Evdev::LEFT_CTRL, Evdev::LEFT_ALT),
        (Evdev::RIGHT_CTRL, Evdev::RIGHT_ALT),
        (Evdev::LEFT_CTRL, Evdev::RIGHT_ALT),
    ] {
        let mut state = KeyState::default();
        state.apply(down(ctrl));
        state.apply(down(alt));
        assert!(
            chord.completes(&state, down(Evdev::HOME)),
            "{ctrl:?}+{alt:?}+Home"
        );
    }
}

#[test]
fn chord_needs_every_modifier_and_a_down_of_the_trigger() {
    let chord = Chord::ctrl_alt_home();
    let mut state = KeyState::default();
    state.apply(down(Evdev::LEFT_CTRL));
    assert!(!chord.completes(&state, down(Evdev::HOME)), "Alt missing");
    state.apply(down(Evdev::LEFT_ALT));
    assert!(
        !chord.completes(&state, up(Evdev::HOME)),
        "an Up never completes"
    );
    assert!(!chord.completes(&state, down(Evdev(30))), "another key");
    state.apply(down(Evdev::HOME));
    assert!(
        !chord.completes(&state, down(Evdev::HOME)),
        "an auto-repeat of the trigger does not complete again"
    );
}

#[test]
fn pressing_the_trigger_first_does_not_fire_when_a_modifier_arrives() {
    let chord = Chord::ctrl_alt_home();
    let mut state = KeyState::default();
    state.apply(down(Evdev::HOME));
    state.apply(down(Evdev::LEFT_ALT));
    assert!(
        !chord.completes(&state, down(Evdev::LEFT_CTRL)),
        "only the last key of the chord is the trigger"
    );
}

#[test]
fn shift_escape_chord_parses_and_completes() {
    let chord: Chord = "shift+esc".parse().expect("shift+esc parses");
    let mut state = KeyState::default();
    state.apply(down(Evdev::RIGHT_SHIFT));
    assert!(chord.completes(&state, down(Evdev::ESC)));
    assert!(
        !Chord::ctrl_alt_home().completes(&state, down(Evdev::ESC)),
        "the two chords are different"
    );
}

#[test]
fn chord_names_are_case_insensitive_and_cover_letters_digits_and_f_keys() {
    for text in [
        "Ctrl+Alt+Home",
        "CTRL+ALT+L",
        "super+f12",
        "ctrl+7",
        "alt+F24",
        "Shift+Escape",
    ] {
        assert!(text.parse::<Chord>().is_ok(), "{text}");
    }
    let chord: Chord = "ctrl+alt+l".parse().expect("parse");
    let mut state = KeyState::default();
    state.apply(down(Evdev::LEFT_CTRL));
    state.apply(down(Evdev::LEFT_ALT));
    assert!(
        chord.completes(&state, down(Evdev::KEY_L)),
        "the letter L is evdev 38"
    );
}

#[test]
fn chord_errors_name_the_problem() {
    assert_eq!("".parse::<Chord>(), Err(ChordError::Empty));
    assert_eq!("+ +".parse::<Chord>(), Err(ChordError::Empty));
    assert_eq!(
        "ctrl+banana".parse::<Chord>(),
        Err(ChordError::UnknownKey("banana".into()))
    );
    assert_eq!(
        "f25".parse::<Chord>(),
        Err(ChordError::UnknownKey("f25".into())),
        "only F1 to F24 exist"
    );
}

#[test]
fn modifier_keys_lists_both_sides_of_every_modifier() {
    let keys = Chord::ctrl_alt_home().modifier_keys();
    for key in [
        Evdev::LEFT_CTRL,
        Evdev::RIGHT_CTRL,
        Evdev::LEFT_ALT,
        Evdev::RIGHT_ALT,
    ] {
        assert!(keys.contains(&key), "{key:?}");
    }
    assert!(
        !keys.contains(&Evdev::HOME),
        "the trigger is not a modifier"
    );
}

#[test]
fn a_chord_prints_in_one_spelling_that_parses_back_to_it() {
    for text in [
        "ctrl+alt+home",
        "Control+Alt+PgDn",
        "super+shift+F01",
        "ctrl+alt+1",
        "ctrl+escape",
    ] {
        let chord: Chord = text.parse().expect("parses");
        let printed = chord.to_string();
        assert_eq!(printed.parse::<Chord>(), Ok(chord.clone()), "{text}");
        assert_eq!(printed, printed.to_lowercase(), "{text}");
    }
    let a: Chord = "Control+PgDn".parse().expect("parses");
    let b: Chord = "ctrl+pagedown".parse().expect("parses");
    assert_eq!(a, b, "aliases are one chord");
    assert_eq!(a.to_string(), "ctrl+pagedown");
    assert_eq!(
        "win+f01".parse::<Chord>().expect("parses").to_string(),
        "win+f1"
    );
}

#[test]
fn a_chord_reports_its_trigger_and_its_modifier_families() {
    let chord: Chord = "ctrl+alt+pagedown".parse().expect("parses");
    assert_eq!(chord.trigger_keys(), [Evdev(109)]);
    assert_eq!(chord.modifiers().to_string(), "ctrl+alt");
    let plain: Chord = "home".parse().expect("parses");
    assert!(plain.modifiers().is_empty());
}

#[test]
fn modifier_sets_parse_print_and_compare() {
    let both: ModifierSet = "Alt + control".parse().expect("parses");
    assert_eq!(both.to_string(), "ctrl+alt", "printed in a fixed order");
    assert!(ModifierSet::CTRL.is_subset_of(both));
    assert!(!ModifierSet::SHIFT.is_subset_of(both));
    assert!(both.is_subset_of(both));
    assert_eq!("".parse::<ModifierSet>(), Err(ModifierSetError::Empty));
    assert_eq!(
        "ctrl+home".parse::<ModifierSet>(),
        Err(ModifierSetError::NotAModifier("home".to_owned()))
    );
}
