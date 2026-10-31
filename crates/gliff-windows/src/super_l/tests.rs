use super::Decision;
use super::SuperLTracker;
use super::WinHeld;
use super::is_super_l;
use crate::keymap::Evdev;
use crate::keymap::Press;

const LWIN: u16 = 0x5b;
const RWIN: u16 = 0x5c;
const L: u16 = 0x4c;

fn sent(d: &Decision) -> Vec<(u16, Press)> {
    match d {
        Decision::SendSuperL(events) => events.iter().map(|e| (e.key.0, e.press)).collect(),
        other => panic!("not SendSuperL: {other:?}"),
    }
}

#[test]
fn win_l_sends_super_l_and_swallows_both_l_events() {
    let mut t = SuperLTracker::default();
    assert_eq!(
        t.push(LWIN, Press::Down),
        Decision::Pass,
        "a Win key alone goes to Windows"
    );
    let d = t.push(L, Press::Down);
    assert_eq!(
        sent(&d),
        vec![
            (Evdev::LEFT_META.0, Press::Down),
            (Evdev::KEY_L.0, Press::Down),
            (Evdev::KEY_L.0, Press::Up),
            (Evdev::LEFT_META.0, Press::Up),
        ]
    );
    assert_eq!(t.push(L, Press::Up), Decision::Swallow);
    assert_eq!(
        t.push(LWIN, Press::Up),
        Decision::Pass,
        "Windows sees the Win release"
    );
}

#[test]
fn the_right_win_key_works_too() {
    let mut t = SuperLTracker::default();
    t.push(RWIN, Press::Down);
    assert!(matches!(t.push(L, Press::Down), Decision::SendSuperL(_)));
}

#[test]
fn auto_repeats_of_l_are_swallowed_and_send_nothing() {
    let mut t = SuperLTracker::default();
    t.push(LWIN, Press::Down);
    t.push(L, Press::Down);
    assert_eq!(t.push(L, Press::Down), Decision::Swallow);
    assert_eq!(t.push(L, Press::Down), Decision::Swallow);
    assert_eq!(t.push(L, Press::Up), Decision::Swallow);
}

#[test]
fn l_without_win_is_for_windows() {
    let mut t = SuperLTracker::default();
    assert_eq!(t.push(L, Press::Down), Decision::Pass);
    assert_eq!(t.push(L, Press::Up), Decision::Pass);
}

#[test]
fn l_after_win_was_released_is_for_windows() {
    let mut t = SuperLTracker::default();
    t.push(LWIN, Press::Down);
    t.push(LWIN, Press::Up);
    assert_eq!(t.push(L, Press::Down), Decision::Pass);
}

#[test]
fn every_other_key_passes_with_win_held() {
    let mut t = SuperLTracker::default();
    t.push(LWIN, Press::Down);
    for vk in [0x41_u16, 0x0d, 0x45, 0x4b, 0x4d] {
        assert_eq!(t.push(vk, Press::Down), Decision::Pass, "{vk:#x}");
        assert_eq!(t.push(vk, Press::Up), Decision::Pass, "{vk:#x}");
    }
}

#[test]
fn win_held_across_two_chords_sends_twice() {
    let mut t = SuperLTracker::default();
    t.push(LWIN, Press::Down);
    assert!(matches!(t.push(L, Press::Down), Decision::SendSuperL(_)));
    t.push(L, Press::Up);
    assert!(matches!(t.push(L, Press::Down), Decision::SendSuperL(_)));
}

#[test]
fn the_other_win_key_keeps_the_chord_alive() {
    let mut t = SuperLTracker::default();
    t.push(LWIN, Press::Down);
    t.push(RWIN, Press::Down);
    t.push(LWIN, Press::Up);
    assert!(
        matches!(t.push(L, Press::Down), Decision::SendSuperL(_)),
        "right Win is still down"
    );
}

#[test]
fn reset_forgets_held_win_keys() {
    let mut t = SuperLTracker::default();
    t.push(LWIN, Press::Down);
    t.reset();
    assert_eq!(t.push(L, Press::Down), Decision::Pass);
}

#[test]
fn is_super_l_needs_a_win_key() {
    assert!(is_super_l(WinHeld::Yes, L));
    assert!(!is_super_l(WinHeld::No, L));
    assert!(!is_super_l(WinHeld::Yes, 0x4d));
}
