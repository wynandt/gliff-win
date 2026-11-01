use gliff_proto::msg::Axis;
use gliff_proto::msg::ClientMsg;

use super::InputSide;
use crate::altgr::Origin;
use crate::altgr::RawKey;
use crate::input::KeyPipeline;
use crate::input::KeyPolicy;
use crate::keymap::Chord;
use crate::keymap::Evdev;
use crate::keymap::KeyEvent;
use crate::keymap::Prefix;
use crate::keymap::Press;
use crate::pointer::BTN_LEFT;
use crate::pointer::PointerEvent;

fn side() -> InputSide {
    InputSide::new(KeyPipeline::new(
        Chord::ctrl_alt_home(),
        KeyPolicy::default(),
    ))
}

fn raw(vk: u16, scan: u16, prefix: Prefix, press: Press, time_ms: u32) -> RawKey {
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

fn key(code: u16, pressed: bool) -> ClientMsg {
    ClientMsg::Key {
        keycode: u32::from(code),
        pressed,
    }
}

#[test]
fn a_letter_goes_out_as_its_evdev_code() {
    let mut s = side();
    let down = s.key(raw(0x41, 0x1e, Prefix::None, Press::Down, 0));
    let up = s.key(raw(0x41, 0x1e, Prefix::None, Press::Up, 5));
    assert_eq!(down.msgs, vec![key(30, true)], "A down is KEY_A");
    assert_eq!(up.msgs, vec![key(30, false)], "A up is KEY_A");
    assert!(!down.released && !up.released);
}

#[test]
fn the_release_chord_sends_ups_and_asks_the_ui_to_let_go() {
    let mut s = side();
    s.key(raw(0xa2, 0x1d, Prefix::None, Press::Down, 0));
    s.key(raw(0xa4, 0x38, Prefix::None, Press::Down, 1));
    let out = s.tick(50);
    assert!(
        out.msgs.is_empty(),
        "nothing is pending after the tick: {out:?}"
    );
    let out = s.key(raw(0x24, 0x47, Prefix::E0, Press::Down, 60));
    assert!(out.released, "Ctrl+Alt+Home completes the chord");
    assert!(
        out.msgs.contains(&key(29, false)) && out.msgs.contains(&key(56, false)),
        "Ctrl and Alt are released on the remote: {:?}",
        out.msgs
    );
}

#[test]
fn super_l_events_pass_through_as_keys() {
    let mut s = side();
    let msgs = s.keys(vec![
        KeyEvent {
            key: Evdev::LEFT_META,
            press: Press::Down,
        },
        KeyEvent {
            key: Evdev::KEY_L,
            press: Press::Down,
        },
        KeyEvent {
            key: Evdev::KEY_L,
            press: Press::Up,
        },
        KeyEvent {
            key: Evdev::LEFT_META,
            press: Press::Up,
        },
    ]);
    assert_eq!(
        msgs,
        vec![
            key(125, true),
            key(38, true),
            key(38, false),
            key(125, false)
        ]
    );
}

#[test]
fn a_button_press_is_sent_once_and_released_on_release_all() {
    let mut s = side();
    let down = PointerEvent::Button {
        button: BTN_LEFT,
        press: Press::Down,
    };
    assert_eq!(
        s.pointer(down),
        vec![ClientMsg::PointerButton {
            button: BTN_LEFT,
            pressed: true
        }]
    );
    assert!(s.pointer(down).is_empty(), "a repeated Down is dropped");
    assert_eq!(
        s.release_all(),
        vec![ClientMsg::PointerButton {
            button: BTN_LEFT,
            pressed: false
        }]
    );
    assert!(s.release_all().is_empty(), "nothing is held now");
}

#[test]
fn motion_is_sent_as_is() {
    let mut s = side();
    assert_eq!(
        s.pointer(PointerEvent::Motion { x: 10.5, y: 20.0 }),
        vec![ClientMsg::PointerMotion { x: 10.5, y: 20.0 }]
    );
}

#[test]
fn touchpad_deltas_scroll_smoothly_until_the_scroll_ends() {
    let mut s = side();
    let axis = |value, discrete, stop| ClientMsg::PointerAxis {
        axis: Axis::Vertical,
        value,
        discrete,
        stop,
    };
    let mut msgs = Vec::new();
    for delta in [-8, -8, -120] {
        msgs.extend(s.wheel(Axis::Vertical, delta));
    }
    msgs.extend(s.release_all());
    msgs.extend(s.wheel(Axis::Vertical, -120));
    assert_eq!(
        msgs,
        vec![
            axis(1.0, None, false),
            axis(1.0, None, false),
            axis(15.0, None, false),
            axis(0.0, None, true),
            axis(15.0, Some(1), false),
        ],
        "8 units is 1 pixel at once, a notch inside the gesture stays smooth, \
         the end is a stop, and after it a notch is a wheel step again"
    );
}

#[test]
fn release_all_ends_held_keys_buttons_and_the_smooth_scroll() {
    let mut s = side();
    s.key(raw(0x41, 0x1e, Prefix::None, Press::Down, 0));
    s.pointer(PointerEvent::Button {
        button: BTN_LEFT,
        press: Press::Down,
    });
    s.wheel(Axis::Vertical, 60);
    let msgs = s.release_all();
    assert_eq!(
        msgs,
        vec![
            key(30, false),
            ClientMsg::PointerButton {
                button: BTN_LEFT,
                pressed: false
            },
            ClientMsg::PointerAxis {
                axis: Axis::Vertical,
                value: 0.0,
                discrete: None,
                stop: true,
            },
        ]
    );
    assert!(!s.scrolling(), "the scroll is closed");
}

#[test]
fn a_lone_left_ctrl_waits_for_altgr_then_the_tick_forwards_it() {
    let mut s = side();
    let down = s.key(raw(0xa2, 0x1d, Prefix::None, Press::Down, 100));
    assert!(down.msgs.is_empty(), "Ctrl Down waits");
    assert!(s.holding());
    let out = s.tick(120);
    assert_eq!(out.msgs, vec![key(29, true)]);
    assert!(!s.holding());
}

fn left_click(press: Press) -> PointerEvent {
    PointerEvent::Button {
        button: BTN_LEFT,
        press,
    }
}

fn left_button(pressed: bool) -> ClientMsg {
    ClientMsg::PointerButton {
        button: BTN_LEFT,
        pressed,
    }
}

#[test]
fn a_click_right_after_a_ctrl_goes_out_after_the_ctrl() {
    let mut s = side();
    s.key(raw(0xa2, 0x1d, Prefix::None, Press::Down, 100));
    assert_eq!(
        s.pointer(left_click(Press::Down)),
        vec![key(29, true), left_button(true)],
        "the Ctrl is not left waiting behind the click"
    );
    assert!(!s.holding());
    assert_eq!(s.release_all(), vec![key(29, false), left_button(false)]);
}

#[test]
fn a_wheel_step_right_after_a_ctrl_goes_out_after_the_ctrl() {
    let mut s = side();
    s.key(raw(0xa2, 0x1d, Prefix::None, Press::Down, 100));
    let msgs = s.wheel(Axis::Vertical, 120);
    assert_eq!(msgs.first(), Some(&key(29, true)));
    assert_eq!(msgs.len(), 2, "the Ctrl, then the notch: {msgs:?}");
    assert!(!s.holding());
}

#[test]
fn motion_leaves_the_held_ctrl_for_the_altgr_filter() {
    let mut s = side();
    s.key(raw(0xa2, 0x1d, Prefix::None, Press::Down, 100));
    s.pointer(PointerEvent::Motion { x: 1.0, y: 2.0 });
    assert!(s.holding(), "a moving mouse does not decide it");
    // The AltGr that follows still removes the fake Ctrl.
    let alt = s.key(raw(0xa5, 0x38, Prefix::E0, Press::Down, 103));
    assert_eq!(alt.msgs, vec![key(100, true)], "right Alt alone, no Ctrl");
}

#[test]
fn a_click_during_or_after_a_whole_altgr_press_never_sends_its_fake_ctrl() {
    let mut s = side();
    // Windows reports AltGr as the fake Ctrl and the right Alt with the same
    // time: once both are in, the fake Ctrl is gone and nothing is waiting.
    s.key(raw(0xa2, 0x1d, Prefix::None, Press::Down, 100));
    let alt = s.key(raw(0xa5, 0x38, Prefix::E0, Press::Down, 100));
    assert_eq!(alt.msgs, vec![key(100, true)], "right Alt alone");
    assert!(!s.holding());
    assert_eq!(
        s.pointer(left_click(Press::Down)),
        vec![left_button(true)],
        "a click while AltGr is held gets no Ctrl"
    );
    assert!(
        s.wheel(Axis::Vertical, 120).len() == 1,
        "nor does a wheel step"
    );
    // The fake Ctrl's Up is dropped, too: the remote never saw it go down.
    let up = s.key(raw(0xa2, 0x1d, Prefix::None, Press::Up, 140));
    assert!(up.msgs.is_empty(), "{:?}", up.msgs);
}

#[test]
fn a_click_without_a_waiting_ctrl_sends_only_the_click() {
    let mut s = side();
    assert_eq!(s.pointer(left_click(Press::Down)), vec![left_button(true)]);
}

const RSHIFT: u16 = 0xa1;
const SLASH: u16 = 0xbf;

/// A real right Shift as a built-in keyboard's hook reports it: scan 0x36 with
/// the extended flag (measured with a probe).
fn rshift(press: Press, t: u32) -> RawKey {
    raw(RSHIFT, 0x36, Prefix::E0, press, t)
}

fn slash(press: Press, t: u32) -> RawKey {
    raw(SLASH, 0x35, Prefix::None, press, t)
}

#[test]
fn right_shift_and_slash_go_out_as_shift_down_slash_shift_up_in_order() {
    let mut s = side();
    let wire: Vec<ClientMsg> = [
        rshift(Press::Down, 0),
        slash(Press::Down, 10),
        slash(Press::Up, 20),
        rshift(Press::Up, 30),
    ]
    .into_iter()
    .flat_map(|k| s.key(k).msgs)
    .collect();
    assert_eq!(
        wire,
        vec![key(54, true), key(53, true), key(53, false), key(54, false)],
        "KEY_RIGHTSHIFT down, KEY_SLASH down and up, KEY_RIGHTSHIFT up"
    );
}

#[test]
fn a_held_right_shift_is_released_on_the_remote_when_the_grab_ends() {
    let mut s = side();
    s.key(rshift(Press::Down, 0));
    let out = s.release_all();
    assert!(
        out.contains(&key(54, false)),
        "right Shift is let go: {out:?}"
    );
}

#[test]
fn right_shift_and_escape_release_a_shift_escape_chord() {
    let mut s = InputSide::new(KeyPipeline::new(
        "shift+esc".parse().expect("a chord"),
        KeyPolicy::default(),
    ));
    s.key(rshift(Press::Down, 0));
    let out = s.key(raw(0x1b, 0x01, Prefix::None, Press::Down, 10));
    assert!(out.released, "either Shift completes Shift+Esc");
    assert!(
        out.msgs.contains(&key(54, false)) && !out.msgs.contains(&key(1, true)),
        "Shift is let go and Esc is not sent: {:?}",
        out.msgs
    );
}

/// Modelled, not measured: what Windows is documented to send with Num Lock
/// on when Shift is held and a keypad key is pressed. Windows takes the
/// Shift up (scan 0x22A), sends the key (as the navigation key, here Up from
/// keypad 8: vk 0x26, scan 0x48, not extended), then puts the Shift back
/// with its normal scan code. The fake Up is dropped, the keypad key goes
/// out as keypad 8, and the repeated Shift Down adds nothing.
#[test]
fn the_fake_shifts_around_a_keypad_key_are_not_sent_modelled() {
    for (vk, real_scan, fake_scan, prefix, code) in [
        (0xa0, 0x2a, 0x22a, Prefix::None, 42),
        (RSHIFT, 0x36, 0x236, Prefix::E0, 54),
    ] {
        let mut s = side();
        let wire: Vec<ClientMsg> = [
            raw(vk, real_scan, prefix, Press::Down, 0),
            raw(vk, fake_scan, prefix, Press::Up, 100),
            raw(0x26, 0x48, Prefix::None, Press::Down, 100),
            raw(0x26, 0x48, Prefix::None, Press::Up, 150),
            raw(vk, real_scan, prefix, Press::Down, 150),
            raw(vk, real_scan, prefix, Press::Up, 400),
        ]
        .into_iter()
        .flat_map(|k| s.key(k).msgs)
        .collect();
        assert_eq!(
            wire,
            vec![
                key(code, true),
                key(72, true),
                key(72, false),
                key(code, false)
            ],
            "Shift {code} stays down through the fake Up and its restoring Down"
        );
    }
}
