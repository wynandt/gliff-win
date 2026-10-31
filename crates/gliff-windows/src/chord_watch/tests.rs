use super::ChordWatch;
use super::Decision;
use super::Fate;
use super::HookGate;
use super::OVERDUE_TICKS;
use super::SwitchChords;
use super::Taker;
use super::switch_of_word;
use super::switch_word;
use crate::altgr::Origin;
use crate::altgr::RawKey;
use crate::grab_state::HookMode;
use crate::keymap::Chord;
use crate::keymap::ModifierSet;
use crate::keymap::Prefix;
use crate::keymap::Press;
use crate::tabs::SwitchTo;

fn key(vk: u16, scan: u16, prefix: Prefix, press: Press, time_ms: u32) -> RawKey {
    RawKey {
        vk,
        scan,
        prefix,
        press,
        time_ms,
        origin: Origin::Hook,
        injected: false,
    }
}

fn ctrl(press: Press, t: u32) -> RawKey {
    key(0xa2, 0x1d, Prefix::None, press, t)
}

fn alt(press: Press, t: u32) -> RawKey {
    key(0xa4, 0x38, Prefix::None, press, t)
}

fn right_alt(press: Press, t: u32) -> RawKey {
    key(0xa5, 0x38, Prefix::E0, press, t)
}

fn home(press: Press, t: u32) -> RawKey {
    key(0x24, 0x47, Prefix::E0, press, t)
}

fn watch() -> ChordWatch {
    ChordWatch::new(Chord::ctrl_alt_home(), SwitchChords::NONE)
}

/// The watch with the default switch chords too.
fn switching() -> ChordWatch {
    ChordWatch::new(Chord::ctrl_alt_home(), SwitchChords::standard())
}

/// True when `key` completed the release chord.
fn released(watch: &mut ChordWatch, key: &RawKey) -> bool {
    watch.push(key) == Some(Decision::Release)
}

#[test]
fn home_alone_or_with_one_modifier_does_not_complete() {
    let mut w = watch();
    assert!(!released(&mut w, &home(Press::Down, 0)));
    assert!(!released(&mut w, &home(Press::Up, 10)));
    w.push(&ctrl(Press::Down, 20));
    assert!(!released(&mut w, &home(Press::Down, 30)));
}

#[test]
fn a_modifier_that_came_up_no_longer_counts() {
    let mut w = watch();
    w.push(&ctrl(Press::Down, 0));
    w.push(&alt(Press::Down, 100));
    w.push(&alt(Press::Up, 200));
    assert!(!released(&mut w, &home(Press::Down, 300)));
}

#[test]
fn modifiers_held_before_the_hook_began_count_once_seeded() {
    let mut w = watch();
    w.seed([0xa2, 0xa4]);
    assert!(
        released(&mut w, &home(Press::Down, 0)),
        "Ctrl and Alt were already down"
    );
}

#[test]
fn seeding_replaces_and_clearing_forgets_what_was_tracked() {
    for forget in [|w: &mut ChordWatch| w.seed([]), ChordWatch::clear] {
        let mut w = watch();
        w.push(&ctrl(Press::Down, 0));
        w.push(&alt(Press::Down, 100));
        forget(&mut w);
        assert!(!released(&mut w, &home(Press::Down, 200)));
    }
}

#[test]
fn ctrl_alt_home_completes_on_the_home_down_and_an_auto_repeat_does_not_again() {
    let mut w = watch();
    assert!(!released(&mut w, &ctrl(Press::Down, 0)));
    assert!(!released(&mut w, &alt(Press::Down, 100)));
    assert!(released(&mut w, &home(Press::Down, 200)));
    assert!(
        !released(&mut w, &home(Press::Down, 230)),
        "Home is held now"
    );
}

#[test]
fn altgr_is_not_ctrl_and_alt() {
    let mut w = watch();
    // Windows sends a fake left Ctrl, then the right Alt, within a few ms.
    w.push(&ctrl(Press::Down, 1000));
    w.push(&right_alt(Press::Down, 1002));
    assert!(
        !released(&mut w, &home(Press::Down, 1100)),
        "AltGr+Home stays a key"
    );
}

#[test]
fn a_real_ctrl_with_a_right_alt_pressed_later_completes() {
    let mut w = watch();
    w.push(&ctrl(Press::Down, 1000));
    w.push(&right_alt(Press::Down, 1500));
    assert!(released(&mut w, &home(Press::Down, 1600)));
}

#[test]
fn only_a_ctrl_and_a_right_alt_with_their_own_codes_make_the_altgr_pair() {
    // A key with Ctrl's virtual key but another scan code, or Ctrl's scan
    // code under another virtual key, is not the fake Ctrl of AltGr: the
    // real Ctrl held before it stays in the chord.
    for (vk, scan) in [(0xa2, 0x1e), (0x11, 0x1d)] {
        let mut w = watch();
        w.push(&ctrl(Press::Down, 1000));
        w.push(&key(vk, scan, Prefix::None, Press::Down, 1500));
        w.push(&right_alt(Press::Down, 1502));
        assert!(
            released(&mut w, &home(Press::Down, 1600)),
            "vk {vk:#x} scan {scan:#x}"
        );
    }
    // A key that is not the right Alt, right after the Ctrl, is no AltGr.
    for (vk, prefix) in [(0xa4, Prefix::None), (0xa4, Prefix::E0)] {
        let mut w = watch();
        w.push(&ctrl(Press::Down, 1000));
        w.push(&key(vk, 0x38, prefix, Press::Down, 1002));
        assert!(
            released(&mut w, &home(Press::Down, 1100)),
            "vk {vk:#x} {prefix:?}"
        );
    }
}

#[test]
fn a_custom_chord_is_the_one_watched() {
    let mut w = ChordWatch::new("shift+esc".parse().expect("a chord"), SwitchChords::NONE);
    w.push(&key(0xa0, 0x2a, Prefix::None, Press::Down, 0));
    assert!(released(
        &mut w,
        &key(0x1b, 0x01, Prefix::None, Press::Down, 10)
    ));
}

#[test]
fn a_real_right_shift_completes_a_shift_chord() {
    let mut w = ChordWatch::new("shift+esc".parse().expect("a chord"), SwitchChords::NONE);
    w.push(&key(0xa1, 0x36, Prefix::E0, Press::Down, 0));
    assert!(
        released(&mut w, &key(0x1b, 0x01, Prefix::None, Press::Down, 10)),
        "right Shift arrives extended"
    );
}

#[test]
fn keys_without_an_evdev_code_are_ignored() {
    let mut w = watch();
    w.push(&ctrl(Press::Down, 0));
    w.push(&alt(Press::Down, 100));
    assert!(!released(
        &mut w,
        &key(0xe8, 0, Prefix::None, Press::Down, 150)
    ));
    assert!(released(&mut w, &home(Press::Down, 200)));
}

fn page_down(press: Press, t: u32) -> RawKey {
    key(0x22, 0x51, Prefix::E0, press, t)
}

fn page_up(press: Press, t: u32) -> RawKey {
    key(0x21, 0x49, Prefix::E0, press, t)
}

/// A main-row digit: virtual key 0x30 + n, scan code 1 + n.
fn digit(n: u16, press: Press, t: u32) -> RawKey {
    let scan = if n == 0 { 11 } else { 1 + n };
    key(0x30 + n, scan, Prefix::None, press, t)
}

/// A keypad digit: virtual key 0x60 + n.
fn keypad_digit(n: u16, press: Press, t: u32) -> RawKey {
    key(0x60 + n, 0x4f, Prefix::None, press, t)
}

fn hold_ctrl_alt(w: &mut ChordWatch) {
    w.push(&ctrl(Press::Down, 0));
    w.push(&alt(Press::Down, 100));
}

#[test]
fn page_down_and_page_up_switch_to_the_next_and_the_previous_tab() {
    let mut w = switching();
    hold_ctrl_alt(&mut w);
    assert_eq!(
        w.push(&page_down(Press::Down, 200)),
        Some(Decision::Switch(SwitchTo::Next))
    );
    w.push(&page_down(Press::Up, 210));
    assert_eq!(
        w.push(&page_up(Press::Down, 300)),
        Some(Decision::Switch(SwitchTo::Previous))
    );
}

#[test]
fn the_digits_one_to_nine_switch_to_that_position() {
    for n in 1..=9u16 {
        let mut w = switching();
        hold_ctrl_alt(&mut w);
        assert_eq!(
            w.push(&digit(n, Press::Down, 200)),
            Some(Decision::Switch(SwitchTo::Position(n as u8))),
            "digit {n}"
        );
    }
}

#[test]
fn zero_and_the_keypad_digits_do_not_switch() {
    let mut w = switching();
    hold_ctrl_alt(&mut w);
    assert_eq!(w.push(&digit(0, Press::Down, 200)), None);
    for n in 0..=9 {
        assert_eq!(
            w.push(&keypad_digit(n, Press::Down, 210)),
            None,
            "keypad {n}"
        );
    }
}

#[test]
fn a_switch_chord_needs_its_modifiers() {
    let mut w = switching();
    assert_eq!(w.push(&page_down(Press::Down, 0)), None);
    w.push(&page_down(Press::Up, 5));
    w.push(&ctrl(Press::Down, 10));
    assert_eq!(w.push(&page_down(Press::Down, 20)), None, "Ctrl alone");
    w.push(&page_down(Press::Up, 25));
    w.push(&ctrl(Press::Up, 30));
    w.push(&alt(Press::Down, 40));
    assert_eq!(w.push(&digit(2, Press::Down, 50)), None, "Alt alone");
}

#[test]
fn a_switch_leaves_the_modifiers_tracked_so_the_next_switch_works() {
    let mut w = switching();
    hold_ctrl_alt(&mut w);
    w.push(&page_down(Press::Down, 200));
    w.push(&page_down(Press::Up, 210));
    assert_eq!(
        w.push(&page_down(Press::Down, 400)),
        Some(Decision::Switch(SwitchTo::Next)),
        "Ctrl and Alt still held: a second press switches again"
    );
}

#[test]
fn the_auto_repeat_of_a_switch_trigger_does_not_switch_again() {
    let mut w = switching();
    hold_ctrl_alt(&mut w);
    assert!(w.push(&page_down(Press::Down, 200)).is_some());
    assert_eq!(w.push(&page_down(Press::Down, 230)), None, "held");
}

#[test]
fn the_release_chord_is_tested_before_a_switch_chord() {
    // A release chord that shares its modifiers with a switch chord and is
    // the same key would be refused by settings; here the same trigger is
    // forced to show the order.
    let both = SwitchChords {
        next: Some(Chord::ctrl_alt_home()),
        ..SwitchChords::NONE
    };
    let mut w = ChordWatch::new(Chord::ctrl_alt_home(), both);
    hold_ctrl_alt(&mut w);
    assert_eq!(w.push(&home(Press::Down, 200)), Some(Decision::Release));
}

#[test]
fn switch_chords_that_are_off_do_nothing() {
    let mut w = ChordWatch::new(
        Chord::ctrl_alt_home(),
        SwitchChords {
            next: None,
            previous: Some(Chord::ctrl_alt_page_up()),
            jump: None,
        },
    );
    hold_ctrl_alt(&mut w);
    assert_eq!(w.push(&page_down(Press::Down, 200)), None);
    assert_eq!(w.push(&digit(1, Press::Down, 210)), None);
    assert_eq!(
        w.push(&page_up(Press::Down, 220)),
        Some(Decision::Switch(SwitchTo::Previous))
    );
}

#[test]
fn the_jump_modifiers_can_be_changed() {
    let mut w = ChordWatch::new(
        Chord::ctrl_alt_home(),
        SwitchChords {
            jump: Some(ModifierSet::ALT),
            ..SwitchChords::NONE
        },
    );
    w.push(&alt(Press::Down, 0));
    assert_eq!(
        w.push(&digit(3, Press::Down, 10)),
        Some(Decision::Switch(SwitchTo::Position(3)))
    );
}

#[test]
fn an_empty_jump_modifier_set_would_take_bare_digits_so_it_takes_none() {
    let mut w = ChordWatch::new(
        Chord::ctrl_alt_home(),
        SwitchChords {
            jump: Some(ModifierSet::default()),
            ..SwitchChords::NONE
        },
    );
    assert_eq!(w.push(&digit(1, Press::Down, 0)), None);
}

#[test]
fn changing_the_chords_keeps_the_keys_that_are_down() {
    let mut w = watch();
    hold_ctrl_alt(&mut w);
    w.set_chords(Chord::ctrl_alt_home(), SwitchChords::standard());
    assert_eq!(
        w.push(&page_down(Press::Down, 200)),
        Some(Decision::Switch(SwitchTo::Next))
    );
}

#[test]
fn altgr_and_a_digit_does_not_switch_as_a_pushed_sequence() {
    // Windows sends a fake left Ctrl, then the right Alt, within a few ms.
    let mut w = switching();
    w.push(&ctrl(Press::Down, 1000));
    w.push(&right_alt(Press::Down, 1002));
    assert_eq!(
        w.push(&digit(4, Press::Down, 1100)),
        None,
        "AltGr+4 types a character"
    );
    assert_eq!(w.push(&page_down(Press::Down, 1200)), None);
}

#[test]
fn altgr_and_a_digit_does_not_switch_as_a_seed() {
    // A grab that starts while AltGr is held: Windows holds left Ctrl and
    // right Alt and there is no timing to tell the Ctrl is fake.
    let mut w = switching();
    w.seed([0xa2, 0xa5]);
    assert_eq!(w.push(&digit(4, Press::Down, 0)), None);
    assert_eq!(w.push(&page_down(Press::Down, 10)), None);
}

#[test]
fn a_seed_of_the_left_alt_with_ctrl_still_switches() {
    let mut w = switching();
    w.seed([0xa2, 0xa4]);
    assert_eq!(
        w.push(&digit(4, Press::Down, 0)),
        Some(Decision::Switch(SwitchTo::Position(4)))
    );
}

#[test]
fn a_seed_of_the_right_alt_alone_keeps_no_fake_ctrl() {
    let mut w = switching();
    w.seed([0xa5]);
    w.push(&ctrl(Press::Down, 5000));
    assert_eq!(
        w.push(&digit(2, Press::Down, 5010)),
        Some(Decision::Switch(SwitchTo::Position(2))),
        "a real Ctrl pressed later, with the right Alt held, is Ctrl+Alt"
    );
}

#[test]
fn the_gate_starts_off_and_takes_requests() {
    let mut gate = HookGate::new();
    assert_eq!(gate.mode(), HookMode::Off);
    assert_eq!(gate.request(HookMode::Grab), HookMode::Grab);
    assert_eq!(gate.mode(), HookMode::Grab);
}

#[test]
fn a_fired_chord_turns_the_grab_off_at_once() {
    let mut gate = HookGate::new();
    gate.request(HookMode::Grab);
    gate.chord_fired();
    assert_eq!(gate.mode(), HookMode::Off);
}

#[test]
fn a_grab_request_that_was_already_on_its_way_is_refused_after_the_chord() {
    let mut gate = HookGate::new();
    gate.request(HookMode::Grab);
    gate.chord_fired();
    assert_eq!(
        gate.request(HookMode::Grab),
        HookMode::Off,
        "a late focus event"
    );
    assert_eq!(
        gate.request(HookMode::SuperLOnly),
        HookMode::SuperLOnly,
        "only the grab is held back"
    );
}

#[test]
fn after_the_ui_acknowledges_a_grab_is_possible_again() {
    let mut gate = HookGate::new();
    gate.request(HookMode::Grab);
    gate.chord_fired();
    gate.acknowledge();
    assert_eq!(
        gate.request(HookMode::Grab),
        HookMode::Grab,
        "a click re-grabs"
    );
}

mod taking {
    use super::*;

    const PAGE_DOWN: u16 = 0x22;
    const NEXT: Option<Decision> = Some(Decision::Switch(SwitchTo::Next));

    #[test]
    fn every_switch_survives_the_trip_through_a_window_message() {
        let all = [SwitchTo::Next, SwitchTo::Previous]
            .into_iter()
            .chain((1..=9).map(SwitchTo::Position));
        for to in all {
            assert_eq!(switch_of_word(switch_word(to)), Some(to));
        }
    }

    #[test]
    fn a_word_that_is_no_switch_is_none() {
        for word in [0, 3, 0xff, 0x100, 0x200, usize::MAX] {
            assert_eq!(switch_of_word(word), None, "{word:#x}");
        }
    }

    #[test]
    fn a_switch_while_grabbing_is_taken_and_so_are_the_repeats_and_the_up() {
        let mut taker = Taker::new();
        let fate = taker.fate(HookMode::Grab, true, NEXT, PAGE_DOWN, Press::Down);
        assert_eq!(fate, Fate::Switch(SwitchTo::Next));
        for _ in 0..3 {
            let fate = taker.fate(HookMode::Grab, true, None, PAGE_DOWN, Press::Down);
            assert_eq!(fate, Fate::Swallow, "an auto-repeat");
        }
        assert_eq!(
            taker.fate(HookMode::Grab, true, None, PAGE_DOWN, Press::Up),
            Fate::Swallow
        );
        assert_eq!(
            taker.fate(HookMode::Grab, true, None, PAGE_DOWN, Press::Down),
            Fate::Normal,
            "after the Up, the key is an ordinary key again"
        );
    }

    #[test]
    fn another_key_during_a_taken_trigger_is_not_taken() {
        let mut taker = Taker::new();
        taker.fate(HookMode::Grab, true, NEXT, PAGE_DOWN, Press::Down);
        assert_eq!(
            taker.fate(HookMode::Grab, true, None, 0x41, Press::Down),
            Fate::Normal
        );
    }

    #[test]
    fn a_switch_is_taken_with_no_session_so_a_dead_tab_can_be_left() {
        let mut taker = Taker::new();
        let fate = taker.fate(HookMode::Grab, false, NEXT, PAGE_DOWN, Press::Down);
        assert_eq!(fate, Fate::Switch(SwitchTo::Next));
    }

    #[test]
    fn a_release_with_no_session_is_not_taken() {
        let mut taker = Taker::new();
        let fate = taker.fate(
            HookMode::Grab,
            false,
            Some(Decision::Release),
            0x24,
            Press::Down,
        );
        assert_eq!(fate, Fate::Pass);
    }

    #[test]
    fn a_release_with_a_session_is_taken() {
        let mut taker = Taker::new();
        let fate = taker.fate(
            HookMode::Grab,
            true,
            Some(Decision::Release),
            0x24,
            Press::Down,
        );
        assert_eq!(fate, Fate::Release);
    }

    #[test]
    fn with_no_session_every_other_key_passes_to_windows() {
        let mut taker = Taker::new();
        for mode in [HookMode::Off, HookMode::SuperLOnly, HookMode::Grab] {
            assert_eq!(
                taker.fate(mode, false, None, 0x41, Press::Down),
                Fate::Pass,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_switch_chord_outside_a_grab_is_left_alone() {
        let mut taker = Taker::new();
        for mode in [HookMode::Off, HookMode::SuperLOnly] {
            assert_eq!(
                taker.fate(mode, true, NEXT, PAGE_DOWN, Press::Down),
                Fate::Normal,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn a_grab_that_ends_with_a_trigger_held_does_not_take_its_next_press() {
        let mut taker = Taker::new();
        taker.fate(HookMode::Grab, true, NEXT, PAGE_DOWN, Press::Down);
        // The grab ended; the Up goes to Windows.
        assert_eq!(
            taker.fate(HookMode::Off, true, None, PAGE_DOWN, Press::Up),
            Fate::Normal
        );
        assert_eq!(
            taker.fate(HookMode::Grab, true, None, PAGE_DOWN, Press::Down),
            Fate::Normal
        );
    }
}

#[test]
fn reinstall_seeds_chord_watch() {
    use crate::grab_state::HookMode;
    // Ctrl and Alt went down while the hook was not looking (the gap of a
    // reinstall). The new watch must know, or Home does not release the
    // grab, and the key a user presses to get out does nothing.
    let held = [0xa2u16, 0xa4];
    let mut fresh = ChordWatch::new(Chord::ctrl_alt_home(), SwitchChords::NONE);
    assert!(
        !released(&mut fresh, &home(Press::Down, 10)),
        "an unseeded watch has not seen Ctrl and Alt"
    );
    let mut watch = ChordWatch::after_reinstall(
        Chord::ctrl_alt_home(),
        SwitchChords::NONE,
        HookMode::Grab,
        held,
    );
    assert!(released(&mut watch, &home(Press::Down, 20)));

    let mut off = ChordWatch::after_reinstall(
        Chord::ctrl_alt_home(),
        SwitchChords::NONE,
        HookMode::Off,
        held,
    );
    assert!(
        !released(&mut off, &home(Press::Down, 30)),
        "an off hook looks at nothing"
    );
}

#[test]
fn a_chord_no_session_answers_is_taken_in_by_the_ui_tick() {
    let mut gate = HookGate::new();
    gate.request(HookMode::Grab);
    gate.chord_fired();
    // The session answers within a tick or two: the tick stays out of it.
    for _ in 1..OVERDUE_TICKS {
        assert!(!gate.overdue(), "the answer may still come");
    }
    assert_eq!(gate.request(HookMode::Grab), HookMode::Off, "still refused");
    // No answer (an ended session, a tab that is not shown): the UI acts.
    assert!(gate.overdue(), "overdue");
    gate.acknowledge();
    assert!(!gate.overdue(), "acknowledged");
    assert_eq!(
        gate.request(HookMode::Grab),
        HookMode::Grab,
        "a click re-grabs"
    );
    // An answered chord never turns overdue.
    gate.chord_fired();
    gate.acknowledge();
    assert!(!(0..OVERDUE_TICKS).any(|_| gate.overdue()));
}
