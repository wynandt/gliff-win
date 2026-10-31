use gliff_proto::msg::Axis;

use super::BTN_LEFT;
use super::BTN_MIDDLE;
use super::BTN_RIGHT;
use super::ButtonState;
use super::PointerEvent;
use super::SideButtons;
use super::WheelAccumulator;
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
    let mut w = WheelAccumulator::default();
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
    let mut w = WheelAccumulator::default();
    let mut all = Vec::new();
    for _ in 0..3 {
        all.extend(w.push(Axis::Vertical, -120));
    }
    for _ in 0..3 {
        all.extend(w.push(Axis::Vertical, 120));
    }
    assert_eq!(steps(&all), vec![1, 1, 1, -1, -1, -1]);
    assert_eq!(w.rest(Axis::Vertical), 0, "nothing left over");
}

#[test]
fn horizontal_keeps_the_windows_sign() {
    let mut w = WheelAccumulator::default();
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

#[test]
fn small_touchpad_deltas_add_up_to_one_step_per_notch() {
    let mut w = WheelAccumulator::default();
    let mut all = Vec::new();
    for _ in 0..30 {
        all.extend(w.push(Axis::Vertical, -8));
    }
    // 30 x 8 = 240 = two notches.
    assert_eq!(
        steps(&all),
        vec![1, 1],
        "two steps, where per-event signum would send 30"
    );
    assert_eq!(w.rest(Axis::Vertical), 0);
}

#[test]
fn the_remainder_is_carried_over_and_never_dropped() {
    let mut w = WheelAccumulator::default();
    let mut all = Vec::new();
    for _ in 0..7 {
        all.extend(w.push(Axis::Vertical, -17));
    }
    assert!(all.is_empty(), "7 x 17 = 119 is one short of a notch");
    assert_eq!(w.rest(Axis::Vertical), -119, "119 is carried");
    let out = w.push(Axis::Vertical, -1);
    assert_eq!(steps(&out), vec![1], "the last unit completes the step");
    assert_eq!(w.rest(Axis::Vertical), 0);
}

#[test]
fn a_big_event_gives_several_steps_and_keeps_the_rest() {
    let mut w = WheelAccumulator::default();
    let out = w.push(Axis::Vertical, -300);
    assert_eq!(steps(&out), vec![1, 1], "300 is two notches");
    assert_eq!(w.rest(Axis::Vertical), -60, "and 60 left");
}

#[test]
fn a_reversal_subtracts_from_the_same_sum() {
    let mut w = WheelAccumulator::default();
    let mut all = Vec::new();
    for _ in 0..4 {
        all.extend(w.push(Axis::Vertical, -30));
    }
    // 120 down reaches exactly one notch, so a step is sent; then 2 x 60 up.
    assert_eq!(steps(&all), vec![1]);
    all.clear();
    for _ in 0..2 {
        all.extend(w.push(Axis::Vertical, 60));
    }
    assert_eq!(steps(&all), vec![-1], "back up one step");
    let mut w = WheelAccumulator::default();
    w.push(Axis::Vertical, -90);
    assert!(
        w.push(Axis::Vertical, 60).is_empty(),
        "-90 + 60 is -30: no step"
    );
    assert_eq!(w.rest(Axis::Vertical), -30);
}

#[test]
fn the_axes_do_not_share_a_remainder() {
    let mut w = WheelAccumulator::default();
    w.push(Axis::Vertical, -100);
    assert!(
        w.push(Axis::Horizontal, 100).is_empty(),
        "100 right is no step"
    );
    assert_eq!(w.rest(Axis::Vertical), -100);
    assert_eq!(w.rest(Axis::Horizontal), 100);
}

#[test]
fn reset_drops_the_remainders() {
    let mut w = WheelAccumulator::default();
    w.push(Axis::Vertical, -100);
    w.reset();
    assert_eq!(w.rest(Axis::Vertical), 0);
}

#[test]
fn extreme_deltas_do_not_overflow() {
    let mut w = WheelAccumulator::default();
    for _ in 0..1000 {
        w.push(Axis::Vertical, i16::MAX);
        w.push(Axis::Vertical, i16::MIN);
    }
    assert!(
        w.rest(Axis::Vertical).abs() < 120,
        "remainder stays below a notch"
    );
}
