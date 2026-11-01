use gliff_proto::msg::Axis;

use super::BTN_LEFT;
use super::BTN_MIDDLE;
use super::BTN_RIGHT;
use super::ButtonState;
use super::PointerEvent;
use super::Scroll;
use super::SideButtons;
use super::button;
use crate::keymap::Press;

const LDOWN: u32 = 0x0201;
const LUP: u32 = 0x0202;
const XDOWN: u32 = 0x020b;
const XUP: u32 = 0x020c;

fn steps(events: &[PointerEvent]) -> Vec<i32> {
    events
        .iter()
        .map(|e| match e {
            PointerEvent::Axis { discrete, .. } => discrete.expect("a wheel step"),
            other => panic!("not an axis event: {other:?}"),
        })
        .collect()
}

#[test]
fn the_three_main_buttons() {
    for (down, up, code) in [
        (LDOWN, LUP, BTN_LEFT),
        (0x0204, 0x0205, BTN_RIGHT),
        (0x0207, 0x0208, BTN_MIDDLE),
    ] {
        assert_eq!(
            button(down, 0, SideButtons::Upstream),
            Some((code, Press::Down))
        );
        assert_eq!(
            button(up, 0, SideButtons::Upstream),
            Some((code, Press::Up))
        );
    }
}

#[test]
fn thumb_buttons_follow_the_enum() {
    assert_eq!(
        button(XDOWN, 1, SideButtons::Upstream),
        Some((0x116, Press::Down)),
        "back"
    );
    assert_eq!(
        button(XUP, 2, SideButtons::Upstream),
        Some((0x115, Press::Up)),
        "forward"
    );
    assert_eq!(
        button(XDOWN, 1, SideButtons::LinuxMouse),
        Some((0x113, Press::Down))
    );
    assert_eq!(
        button(XUP, 2, SideButtons::LinuxMouse),
        Some((0x114, Press::Up))
    );
}

#[test]
fn unknown_buttons_and_messages_are_dropped() {
    assert_eq!(
        button(XDOWN, 3, SideButtons::Upstream),
        None,
        "a third thumb button"
    );
    assert_eq!(button(XDOWN, 0, SideButtons::Upstream), None);
    assert_eq!(
        button(0x0200, 0, SideButtons::Upstream),
        None,
        "WM_MOUSEMOVE"
    );
    assert_eq!(
        button(0x0203, 0, SideButtons::Upstream),
        None,
        "a double click is not a button"
    );
}

#[test]
fn button_state_tracks_downs_and_ups() {
    let mut b = ButtonState::default();
    assert!(b.apply(BTN_LEFT, Press::Down).is_some());
    assert!(b.apply(BTN_LEFT, Press::Down).is_none(), "already held");
    assert!(b.apply(BTN_RIGHT, Press::Up).is_none(), "never pressed");
    assert!(!b.is_empty(), "left is held");
    assert!(b.apply(BTN_LEFT, Press::Up).is_some());
    assert!(b.is_empty(), "the capture can be released");
}

#[test]
fn release_all_sends_an_up_per_held_button() {
    let mut b = ButtonState::default();
    b.apply(BTN_LEFT, Press::Down);
    b.apply(BTN_MIDDLE, Press::Down);
    let ups = b.release_all();
    assert_eq!(
        ups,
        vec![
            PointerEvent::Button {
                button: BTN_LEFT,
                press: Press::Up
            },
            PointerEvent::Button {
                button: BTN_MIDDLE,
                press: Press::Up
            },
        ]
    );
    assert!(b.is_empty(), "empty after release_all");
    assert!(b.release_all().is_empty());
}

#[test]
fn a_mouse_notch_is_one_step_with_upstreams_value() {
    let mut w = Scroll::default();
    let out = w.push(Axis::Vertical, -120);
    assert_eq!(
        out,
        vec![PointerEvent::Axis {
            axis: Axis::Vertical,
            value: 15.0,
            discrete: Some(1),
            stop: false
        }],
        "a notch toward the user scrolls down: positive in Wayland"
    );
    let out = w.push(Axis::Vertical, 120);
    assert_eq!(steps(&out), vec![-1], "a notch away scrolls up");
}

#[test]
fn three_notches_down_then_three_up() {
    let mut w = Scroll::default();
    let mut all = Vec::new();
    for _ in 0..3 {
        all.extend(w.push(Axis::Vertical, -120));
    }
    for _ in 0..3 {
        all.extend(w.push(Axis::Vertical, 120));
    }
    assert_eq!(steps(&all), vec![1, 1, 1, -1, -1, -1]);
}

#[test]
fn horizontal_keeps_the_windows_sign() {
    let mut w = Scroll::default();
    assert_eq!(
        steps(&w.push(Axis::Horizontal, 120)),
        vec![1],
        "right is positive"
    );
    assert_eq!(
        steps(&w.push(Axis::Horizontal, -120)),
        vec![-1],
        "left is negative"
    );
}
