use super::AltGrFilter;
use super::DEADLINE_MS;
use super::Filtered;
use super::Origin;
use super::RawKey;
use crate::keymap::Prefix;
use crate::keymap::Press;

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

fn lctrl(press: Press, t: u32) -> RawKey {
    key(0xa2, 0x1d, Prefix::None, press, t)
}

fn ralt(press: Press, t: u32) -> RawKey {
    key(0xa5, 0x38, Prefix::E0, press, t)
}

fn letter(press: Press, t: u32) -> RawKey {
    key(0x51, 0x10, Prefix::None, press, t)
}

fn forwarded(f: Filtered) -> Vec<RawKey> {
    match f {
        Filtered::Forward(keys) => keys,
        Filtered::Hold => panic!("expected Forward, got Hold"),
    }
}

#[test]
fn altgr_press_drops_the_fake_ctrl() {
    let mut f = AltGrFilter::default();
    assert_eq!(
        f.push(lctrl(Press::Down, 100)),
        Filtered::Hold,
        "Ctrl waits"
    );
    let out = forwarded(f.push(ralt(Press::Down, 100)));
    assert_eq!(out, vec![ralt(Press::Down, 100)], "only the Alt goes on");
    assert!(!f.is_holding(), "nothing is held back any more");
}

#[test]
fn altgr_release_drops_the_fake_ctrl_up_in_either_order() {
    for ctrl_first in [true, false] {
        let mut f = AltGrFilter::default();
        f.push(lctrl(Press::Down, 100));
        f.push(ralt(Press::Down, 101));
        let ups = if ctrl_first {
            [lctrl(Press::Up, 900), ralt(Press::Up, 900)]
        } else {
            [ralt(Press::Up, 900), lctrl(Press::Up, 901)]
        };
        let mut sent = Vec::new();
        for up in ups {
            sent.extend(forwarded(f.push(up)));
        }
        assert_eq!(
            sent,
            vec![ralt(Press::Up, 900)],
            "ctrl_first={ctrl_first}: only the Alt Up is sent"
        );
    }
}

#[test]
fn a_real_ctrl_chord_is_forwarded_in_order() {
    let mut f = AltGrFilter::default();
    assert_eq!(f.push(lctrl(Press::Down, 100)), Filtered::Hold);
    let out = forwarded(f.push(letter(Press::Down, 105)));
    assert_eq!(
        out,
        vec![lctrl(Press::Down, 100), letter(Press::Down, 105)],
        "Ctrl then the letter"
    );
    let ctrl_up = forwarded(f.push(lctrl(Press::Up, 200)));
    assert_eq!(
        ctrl_up,
        vec![lctrl(Press::Up, 200)],
        "a real Ctrl Up is kept"
    );
}

#[test]
fn a_lone_ctrl_is_forwarded_when_the_deadline_passes() {
    let mut f = AltGrFilter::default();
    f.push(lctrl(Press::Down, 100));
    assert_eq!(
        f.expire(100 + DEADLINE_MS),
        None,
        "not yet: the deadline is inclusive"
    );
    assert_eq!(
        f.expire(100 + DEADLINE_MS + 1),
        Some(lctrl(Press::Down, 100)),
        "late, but forwarded"
    );
    assert_eq!(f.expire(10_000), None, "once");
}

#[test]
fn right_alt_after_the_deadline_keeps_the_ctrl() {
    let mut f = AltGrFilter::default();
    f.push(lctrl(Press::Down, 100));
    let out = forwarded(f.push(ralt(Press::Down, 100 + DEADLINE_MS + 1)));
    assert_eq!(
        out,
        vec![lctrl(Press::Down, 100), ralt(Press::Down, 111)],
        "a real Ctrl then a real Alt"
    );
}

#[test]
fn real_ctrl_up_after_a_late_ctrl_is_forwarded() {
    let mut f = AltGrFilter::default();
    f.push(lctrl(Press::Down, 100));
    f.push(ralt(Press::Down, 100));
    f.push(lctrl(Press::Down, 500));
    assert_eq!(f.expire(520), Some(lctrl(Press::Down, 500)));
    let up = forwarded(f.push(lctrl(Press::Up, 900)));
    assert_eq!(
        up,
        vec![lctrl(Press::Up, 900)],
        "a Ctrl that was forwarded gets its Up even while AltGr is held"
    );
}

#[test]
fn a_stray_ctrl_up_is_forwarded_when_no_fake_was_dropped() {
    let mut f = AltGrFilter::default();
    let out = forwarded(f.push(lctrl(Press::Up, 50)));
    assert_eq!(out, vec![lctrl(Press::Up, 50)]);
}

#[test]
fn right_ctrl_is_never_held() {
    let mut f = AltGrFilter::default();
    let rctrl = key(0xa3, 0x1d, Prefix::E0, Press::Down, 10);
    assert_eq!(
        forwarded(f.push(rctrl)),
        vec![rctrl],
        "right Ctrl is not the fake"
    );
}

#[test]
fn reset_forgets_a_held_key() {
    let mut f = AltGrFilter::default();
    f.push(lctrl(Press::Down, 1));
    f.reset();
    assert!(!f.is_holding(), "reset clears the held Ctrl");
    assert_eq!(f.expire(1000), None);
}

#[test]
fn time_wraps_without_a_panic() {
    let mut f = AltGrFilter::default();
    f.push(lctrl(Press::Down, u32::MAX - 2));
    let out = forwarded(f.push(ralt(Press::Down, 3)));
    assert_eq!(
        out,
        vec![ralt(Press::Down, 3)],
        "6 ms apart across the wrap"
    );
}
