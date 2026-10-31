use super::InjectedKeys;
use super::KeyOutcome;
use super::KeyPipeline;
use super::KeyPolicy;
use super::MediaKeys;
use crate::altgr::Origin;
use crate::altgr::RawKey;
use crate::keymap::Chord;
use crate::keymap::Evdev;
use crate::keymap::KeyEvent;
use crate::keymap::Prefix;
use crate::keymap::Press;

fn raw(vk: u16, scan: u16, prefix: Prefix, press: Press, t: u32) -> RawKey {
    RawKey {
        vk,
        scan,
        prefix,
        press,
        time_ms: t,
        origin: Origin::Hook,
        injected: false,
    }
}

fn ev(code: u16, press: Press) -> KeyEvent {
    KeyEvent {
        key: Evdev(code),
        press,
    }
}

fn pipeline() -> KeyPipeline {
    KeyPipeline::new(Chord::ctrl_alt_home(), KeyPolicy::default())
}

fn a(press: Press, t: u32) -> RawKey {
    raw(0x41, 0x1e, Prefix::None, press, t)
}

fn lalt(press: Press, t: u32) -> RawKey {
    raw(0xa4, 0x38, Prefix::None, press, t)
}

fn lctrl(press: Press, t: u32) -> RawKey {
    raw(0xa2, 0x1d, Prefix::None, press, t)
}

fn home(press: Press, t: u32) -> RawKey {
    raw(0x24, 0x47, Prefix::E0, press, t)
}

#[test]
fn a_plain_key_goes_through() {
    let mut p = pipeline();
    assert_eq!(
        p.push(a(Press::Down, 1)),
        KeyOutcome::Send(vec![ev(30, Press::Down)])
    );
    assert_eq!(
        p.push(a(Press::Up, 2)),
        KeyOutcome::Send(vec![ev(30, Press::Up)])
    );
}

#[test]
fn auto_repeat_is_not_sent_twice() {
    let mut p = pipeline();
    p.push(a(Press::Down, 1));
    assert_eq!(
        p.push(a(Press::Down, 40)),
        KeyOutcome::Nothing,
        "a repeated Down"
    );
}

#[test]
fn unmapped_keys_are_dropped() {
    let mut p = pipeline();
    assert_eq!(
        p.push(raw(0xa0, 0x2a, Prefix::E0, Press::Down, 1)),
        KeyOutcome::Nothing,
        "the fake shift"
    );
}

#[test]
fn release_chord_sends_every_up_and_not_the_trigger() {
    let mut p = pipeline();
    assert_eq!(
        p.push(lctrl(Press::Down, 1)),
        KeyOutcome::Nothing,
        "Ctrl waits for a possible AltGr"
    );
    assert_eq!(
        p.push(lalt(Press::Down, 20)),
        KeyOutcome::Send(vec![ev(29, Press::Down), ev(56, Press::Down)]),
        "a left Alt is no AltGr, so the Ctrl goes first"
    );
    let out = p.push(home(Press::Down, 200));
    assert_eq!(
        out,
        KeyOutcome::ReleaseGrab(vec![ev(29, Press::Up), ev(56, Press::Up)]),
        "Ctrl and Alt are released and Home is never sent"
    );
    assert_eq!(
        p.push(home(Press::Up, 210)),
        KeyOutcome::Nothing,
        "Home's Up is dropped too"
    );
}

#[test]
fn shift_escape_chord_works_when_chosen() {
    let mut p = KeyPipeline::new("shift+esc".parse().expect("chord"), KeyPolicy::default());
    p.push(raw(0xa0, 0x2a, Prefix::None, Press::Down, 1));
    let out = p.push(raw(0x1b, 0x01, Prefix::None, Press::Down, 2));
    assert_eq!(
        out,
        KeyOutcome::ReleaseGrab(vec![ev(42, Press::Up)]),
        "Shift released, Esc held back"
    );
}

#[test]
fn with_the_shift_esc_option_ctrl_alt_home_is_an_ordinary_chord() {
    let mut p = KeyPipeline::new("shift+esc".parse().expect("chord"), KeyPolicy::default());
    p.push(lctrl(Press::Down, 1));
    p.push(lalt(Press::Down, 30));
    p.tick(50);
    let out = p.push(home(Press::Down, 60));
    assert!(
        matches!(out, KeyOutcome::Send(_)),
        "Home is forwarded: {out:?}"
    );
}

#[test]
fn altgr_sends_only_the_right_alt() {
    let mut p = pipeline();
    assert_eq!(p.push(lctrl(Press::Down, 100)), KeyOutcome::Nothing);
    let out = p.push(raw(0xa5, 0x38, Prefix::E0, Press::Down, 100));
    assert_eq!(
        out,
        KeyOutcome::Send(vec![ev(100, Press::Down)]),
        "KEY_RIGHTALT only"
    );
}

#[test]
fn a_lone_ctrl_arrives_after_the_tick() {
    let mut p = pipeline();
    p.push(lctrl(Press::Down, 100));
    assert!(p.is_holding(), "waiting for the AltGr that may follow");
    assert_eq!(p.tick(105), KeyOutcome::Nothing, "too early");
    assert_eq!(p.tick(111), KeyOutcome::Send(vec![ev(29, Press::Down)]));
    assert!(!p.is_holding(), "no longer waiting");
}

#[test]
fn release_all_sends_ups_for_what_was_sent_and_drops_a_held_ctrl() {
    let mut p = pipeline();
    p.push(a(Press::Down, 1));
    p.push(lctrl(Press::Down, 5));
    assert_eq!(
        p.release_all(),
        vec![ev(30, Press::Up)],
        "the held Ctrl was never sent, so no Up"
    );
    assert!(!p.is_holding(), "reset");
    assert!(p.release_all().is_empty(), "nothing left");
}

#[test]
fn injected_keys_follow_the_option() {
    let mut injected = a(Press::Down, 1);
    injected.injected = true;
    let mut p = pipeline();
    assert_eq!(
        p.push(injected),
        KeyOutcome::Send(vec![ev(30, Press::Down)]),
        "forward is the default"
    );
    let local = KeyPolicy {
        injected: InjectedKeys::Local,
        media: MediaKeys::Local,
    };
    assert!(
        !local.forwards(&injected),
        "local leaves injected keys to Windows"
    );
    assert!(
        local.forwards(&a(Press::Down, 1)),
        "typed keys are still forwarded"
    );
    let mut p = KeyPipeline::new(Chord::ctrl_alt_home(), local);
    assert_eq!(
        p.push(injected),
        KeyOutcome::Nothing,
        "and the pipeline ignores them"
    );
}

#[test]
fn media_keys_stay_local_unless_asked() {
    let volume_up = raw(0xaf, 0x30, Prefix::E0, Press::Down, 1);
    assert!(
        !KeyPolicy::default().forwards(&volume_up),
        "local by default"
    );
    let forward = KeyPolicy {
        injected: InjectedKeys::Forward,
        media: MediaKeys::Forward,
    };
    assert!(forward.forwards(&volume_up), "--forward-media-keys");
    for vk in [0xad, 0xae, 0xb0, 0xb1, 0xb2, 0xb3] {
        assert!(
            !KeyPolicy::default().forwards(&raw(vk, 0, Prefix::E0, Press::Down, 1)),
            "vk {vk:#x}"
        );
    }
    assert!(
        KeyPolicy::default().forwards(&raw(0xb4, 0, Prefix::E0, Press::Down, 1)),
        "VK_LAUNCH_MAIL is not a media key here"
    );
}

mod stand_in {
    use super::*;

    fn with_chord() -> KeyPipeline {
        pipeline().with_stand_in(Chord::ctrl_alt_l())
    }

    fn l(press: Press, t: u32) -> RawKey {
        raw(0x4c, 0x26, Prefix::None, press, t)
    }

    fn lshift(press: Press, t: u32) -> RawKey {
        raw(0xa0, 0x2a, Prefix::None, press, t)
    }

    fn lwin(press: Press, t: u32) -> RawKey {
        raw(0x5b, 0x5b, Prefix::E0, press, t)
    }

    /// Ctrl and Alt down, as the remote sees them.
    fn held(p: &mut KeyPipeline) {
        p.push(lctrl(Press::Down, 1));
        p.push(lalt(Press::Down, 2));
    }

    fn send(events: &[(u16, Press)]) -> KeyOutcome {
        KeyOutcome::Send(events.iter().map(|&(c, p)| ev(c, p)).collect())
    }

    #[test]
    fn the_chord_sends_super_l_with_the_modifiers_released_and_pressed_again() {
        let mut p = with_chord();
        held(&mut p);
        assert_eq!(
            p.push(l(Press::Down, 3)),
            send(&[
                (29, Press::Up),
                (56, Press::Up),
                (125, Press::Down),
                (38, Press::Down),
                (38, Press::Up),
                (125, Press::Up),
                (29, Press::Down),
                (56, Press::Down),
            ])
        );
    }

    #[test]
    fn a_release_of_all_keys_forgets_the_trigger_the_remote_never_saw() {
        let mut p = with_chord();
        held(&mut p);
        p.push(l(Press::Down, 3));
        // Focus is lost with L still down, and its Up is seen elsewhere.
        p.release_all();
        assert_eq!(
            p.push(l(Press::Down, 100)),
            send(&[(38, Press::Down)]),
            "the next L is an ordinary key, not the old trigger's repeat"
        );
    }

    #[test]
    fn shift_is_released_and_pressed_again_with_the_rest() {
        let mut p = with_chord();
        p.push(lshift(Press::Down, 0));
        held(&mut p);
        let KeyOutcome::Send(events) = p.push(l(Press::Down, 3)) else {
            panic!("the chord sends");
        };
        let first_four: Vec<_> = events.iter().take(3).map(|e| (e.key.0, e.press)).collect();
        assert_eq!(
            first_four,
            vec![(29, Press::Up), (42, Press::Up), (56, Press::Up)],
            "shift 42 is released with ctrl and alt: {events:?}"
        );
        let last: Vec<_> = events.iter().rev().take(3).map(|e| e.press).collect();
        assert_eq!(
            last,
            vec![Press::Down; 3],
            "all three pressed again: {events:?}"
        );
    }

    #[test]
    fn the_remote_state_matches_the_keyboard_afterwards() {
        let mut p = with_chord();
        held(&mut p);
        p.push(l(Press::Down, 3));
        assert_eq!(
            p.push(l(Press::Up, 4)),
            KeyOutcome::Nothing,
            "the trigger was never held"
        );
        assert_eq!(
            p.push(lalt(Press::Up, 5)),
            send(&[(56, Press::Up)]),
            "the pressed-again Alt is released with the real one"
        );
        assert_eq!(p.push(lctrl(Press::Up, 6)), send(&[(29, Press::Up)]));
        assert!(p.release_all().is_empty(), "nothing left held");
    }

    #[test]
    fn an_auto_repeat_of_l_does_not_send_again() {
        let mut p = with_chord();
        held(&mut p);
        assert!(matches!(p.push(l(Press::Down, 3)), KeyOutcome::Send(_)));
        assert_eq!(p.push(l(Press::Down, 4)), KeyOutcome::Nothing, "repeat");
        assert_eq!(p.push(l(Press::Down, 5)), KeyOutcome::Nothing, "repeat");
        assert_eq!(p.push(l(Press::Up, 6)), KeyOutcome::Nothing);
        assert!(
            matches!(p.push(l(Press::Down, 7)), KeyOutcome::Send(ref e) if e.len() == 8),
            "a new press after the Up works again"
        );
    }

    #[test]
    fn a_held_win_key_is_released_and_pressed_again_too() {
        let mut p = with_chord();
        p.push(lwin(Press::Down, 0));
        held(&mut p);
        let KeyOutcome::Send(events) = p.push(l(Press::Down, 3)) else {
            panic!("the chord sends");
        };
        let codes: Vec<_> = events.iter().map(|e| (e.key.0, e.press)).collect();
        assert_eq!(
            codes,
            vec![
                (29, Press::Up),
                (56, Press::Up),
                (125, Press::Up),
                (125, Press::Down),
                (38, Press::Down),
                (38, Press::Up),
                (125, Press::Up),
                (29, Press::Down),
                (56, Press::Down),
                (125, Press::Down),
            ]
        );
    }

    #[test]
    fn l_alone_or_with_only_ctrl_is_a_plain_key() {
        let mut p = with_chord();
        assert_eq!(p.push(l(Press::Down, 0)), send(&[(38, Press::Down)]));
        p.push(l(Press::Up, 1));
        p.push(lctrl(Press::Down, 2));
        assert_eq!(
            p.push(l(Press::Down, 3)),
            send(&[(29, Press::Down), (38, Press::Down)]),
            "the held Ctrl is forwarded with the L"
        );
    }

    #[test]
    fn altgr_l_is_not_the_chord() {
        // AltGr is Right Alt plus a fake Left Ctrl, which the filter drops.
        let mut p = with_chord();
        p.push(lctrl(Press::Down, 0));
        p.push(raw(0xa5, 0x38, Prefix::E0, Press::Down, 1));
        let out = p.push(l(Press::Down, 2));
        assert_eq!(
            out,
            send(&[(38, Press::Down)]),
            "AltGr+L types its character"
        );
    }

    #[test]
    fn the_release_chord_wins_over_the_stand_in() {
        let mut p = KeyPipeline::new("ctrl+alt+l".parse().expect("chord"), KeyPolicy::default())
            .with_stand_in(Chord::ctrl_alt_l());
        held(&mut p);
        assert!(matches!(
            p.push(l(Press::Down, 3)),
            KeyOutcome::ReleaseGrab(_)
        ));
    }

    #[test]
    fn another_stand_in_chord_can_be_configured() {
        let mut p = pipeline().with_stand_in("ctrl+shift+f12".parse().expect("chord"));
        p.push(lctrl(Press::Down, 0));
        p.push(lshift(Press::Down, 1));
        let out = p.push(raw(0x7b, 0x58, Prefix::None, Press::Down, 2));
        assert!(
            matches!(out, KeyOutcome::Send(ref e) if e.len() == 8),
            "{out:?}"
        );
    }
}

#[test]
fn release_all_ends_every_left_and_right_modifier_that_was_sent() {
    let mut p = pipeline();
    let mods = [
        (0xa0, 0x2a, Prefix::None, 42), // left Shift
        (0xa1, 0x36, Prefix::E0, 54),   // right Shift (extended, as a probe measured)
        (0xa2, 0x1d, Prefix::None, 29), // left Ctrl
        (0xa3, 0x1d, Prefix::E0, 97),   // right Ctrl
        (0xa4, 0x38, Prefix::None, 56), // left Alt
        (0xa5, 0x38, Prefix::E0, 100),  // right Alt
        (0x5b, 0x5b, Prefix::E0, 125),  // left Super
        (0x5c, 0x5c, Prefix::E0, 126),  // right Super
    ];
    let mut t = 100;
    for (vk, scan, prefix, _) in mods {
        p.push(raw(vk, scan, prefix, Press::Down, t));
        t += 50;
        // Lets a lone Ctrl through, so none is waiting for an AltGr.
        p.tick(t);
        t += 50;
    }
    let mut ups: Vec<u16> = p
        .release_all()
        .into_iter()
        .map(|e| {
            assert_eq!(e.press, Press::Up);
            e.key.0
        })
        .collect();
    ups.sort_unstable();
    let mut want: Vec<u16> = mods.iter().map(|m| m.3).collect();
    want.sort_unstable();
    assert_eq!(ups, want, "an Up for each of the eight modifiers");
}
