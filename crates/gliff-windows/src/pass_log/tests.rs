use super::OWN_TAG;
use super::PassLog;
use crate::keymap::Press;

#[test]
fn a_noted_key_is_taken_once() {
    let mut log = PassLog::default();
    log.record(0xad, Press::Down, 1000);
    assert!(log.take(0xad, Press::Down, 1002), "first message");
    assert!(!log.take(0xad, Press::Down, 1003), "the note is used up");
}

#[test]
fn an_unnoted_key_is_not_taken() {
    let mut log = PassLog::default();
    log.record(0xad, Press::Down, 1000);
    assert!(!log.take(0xae, Press::Down, 1001), "another key");
    assert!(!log.take(0xad, Press::Up, 1001), "another edge");
}

#[test]
fn a_note_expires() {
    let mut log = PassLog::default();
    log.record(0xad, Press::Down, 1000);
    assert!(!log.take(0xad, Press::Down, 2001), "over a second later");
}

#[test]
fn the_tick_count_may_wrap() {
    let mut log = PassLog::default();
    log.record(0xad, Press::Down, u32::MAX - 1);
    assert!(
        log.take(0xad, Press::Down, 3),
        "wrapped but within a second"
    );
}

#[test]
fn a_message_time_slightly_before_the_hook_time_still_matches() {
    let mut log = PassLog::default();
    log.record(0xad, Press::Down, 1005);
    assert!(log.take(0xad, Press::Down, 1004), "clock rounding");
}

#[test]
fn left_and_right_modifiers_match_the_generic_key() {
    let mut log = PassLog::default();
    log.record(0xa0, Press::Down, 10);
    log.record(0xa3, Press::Down, 10);
    log.record(0xa5, Press::Up, 10);
    assert!(log.take(0x10, Press::Down, 11), "shift");
    assert!(log.take(0x11, Press::Down, 11), "control");
    assert!(log.take(0x12, Press::Up, 11), "alt");
}

#[test]
fn the_oldest_notes_are_dropped_at_capacity() {
    let mut log = PassLog::default();
    for i in 0..40u32 {
        log.record(0x41 + (i % 2) as u16, Press::Down, 5);
    }
    assert_eq!(log.notes.len(), 32, "bounded");
}

const FOREIGN: usize = 0;

#[test]
fn a_key_this_client_injected_is_noted_and_its_message_is_not_a_silent_hook() {
    let mut log = PassLog::default();
    // The hook sees the injected Win+L (left Win, L, L, left Win)...
    let keys = [
        (0x5b, Press::Down, 5000),
        (0x4c, Press::Down, 5000),
        (0x4c, Press::Up, 5001),
        (0x5b, Press::Up, 5001),
    ];
    for (vk, press, at) in keys {
        assert!(log.note_if_own(OWN_TAG, vk, press, at), "{vk:#x} passes");
    }
    // ...and each one reaches the window as a message, which is ours: the
    // window sends nothing and installs nothing.
    for (vk, press, at) in keys {
        assert!(log.take(vk, press, at + 2), "{vk:#x} {press:?} was noted");
    }
}

#[test]
fn an_own_key_message_is_only_passed_once() {
    let mut log = PassLog::default();
    assert!(log.note_if_own(OWN_TAG, 0xa2, Press::Down, 7000));
    // The message names the generic Ctrl, not the left one.
    assert!(log.take(0x11, Press::Down, 7001));
    assert!(
        !log.take(0x11, Press::Down, 7002),
        "a second message for one injected key is not covered: it is a real key"
    );
}

#[test]
fn a_foreign_key_is_not_noted_so_a_silent_hook_still_shows() {
    let mut log = PassLog::default();
    assert!(!log.note_if_own(FOREIGN, 0x41, Press::Down, 9000));
    assert!(
        !log.take(0x41, Press::Down, 9001),
        "a typed key whose message arrives with no hook call is a silent hook"
    );
}

#[test]
fn an_own_key_does_not_cover_another_key_or_a_later_one() {
    let mut log = PassLog::default();
    assert!(log.note_if_own(OWN_TAG, 0x4c, Press::Down, 1000));
    assert!(!log.take(0x4d, Press::Down, 1001), "another key");
    assert!(!log.take(0x4c, Press::Down, 4000), "long after: expired");
}

#[test]
fn a_note_too_old_to_cover_a_key_still_shows_in_the_wider_look() {
    let mut log = PassLog::default();
    log.record(0xa2, Press::Down, 1000);
    // A key message 3 s later is not covered, but the log did see this key
    // family (left Ctrl is Ctrl) a moment before; another key it did not.
    assert!(!log.take(0x11, Press::Down, 4000));
    assert!(log.noted_near(0x11, 4000));
    assert!(log.noted_near(0x11, 500), "a note after the message too");
    assert!(!log.noted_near(0x41, 4000), "another key");
    assert!(!log.noted_near(0x11, 7000), "past the wider look");
    assert!(!log.take(0x11, Press::Down, 7000));
    assert!(!log.noted_near(0x11, 7000), "and dropped by then");
}

mod silent {
    use crate::grab_state::HookMode;
    use crate::pass_log::HookTiming;
    use crate::pass_log::SilentDetail;
    use crate::pass_log::silent_hook_judge;
    use crate::pass_log::silent_line;

    const MODES: [HookMode; 3] = [HookMode::Off, HookMode::SuperLOnly, HookMode::Grab];

    #[test]
    fn a_key_that_the_hook_did_not_see_while_grabbing_is_silent() {
        assert!(silent_hook_judge(HookMode::Grab, HookMode::Grab, false));
    }

    #[test]
    fn a_key_the_hook_noted_is_not_silent() {
        for ui in MODES {
            for gate in MODES {
                assert!(!silent_hook_judge(ui, gate, true), "{ui:?} {gate:?}");
            }
        }
    }

    #[test]
    fn a_key_passed_while_the_hook_gate_is_off_is_not_silent() {
        // The release chord fired: the hook is Off, the UI thread has not
        // caught up yet, and the hook passes every key on purpose.
        assert!(!silent_hook_judge(HookMode::Grab, HookMode::Off, false));
        assert!(!silent_hook_judge(
            HookMode::Grab,
            HookMode::SuperLOnly,
            false
        ));
    }

    #[test]
    fn only_a_window_that_wants_a_grab_can_find_the_hook_silent() {
        for ui in [HookMode::Off, HookMode::SuperLOnly] {
            for gate in MODES {
                assert!(!silent_hook_judge(ui, gate, false), "{ui:?} {gate:?}");
            }
        }
    }

    #[test]
    fn the_silent_line_has_a_count_and_modifier_groups_and_nothing_else() {
        let detail = SilentDetail {
            extra_info: true,
            hook: HookTiming {
                calls: 12,
                last_us: 85,
                slowest_us: 310,
                injected: 1,
                ago_ms: Some(412),
            },
            noted_near: false,
        };
        let base = silent_line(1, [0xa2u16, 0x5b], detail);
        assert_eq!(
            base,
            "keyboard hook silent: key sent from the window (1 so far), hook installed again; \
             2 keys down (Win, Ctrl); message extra info: set; hook calls 12, last 85 us, \
             slowest 310 us, last ran 412 ms ago, injected keys seen 1; \
             hook noted this key within 5 s: no"
        );
        let quiet = SilentDetail {
            extra_info: false,
            hook: HookTiming::default(),
            noted_near: true,
        };
        assert!(silent_line(1, [], quiet).ends_with(
            "0 keys down; message extra info: none; hook never called since installed; \
                 hook noted this key within 5 s: yes"
        ));
        // No other key can change the line, only how many there are.
        let one = silent_line(1, [0x41u16], detail);
        for vk in (0u16..=0xff).filter(|vk| ![0x10, 0x11, 0x12, 0x5b, 0x5c].contains(vk)) {
            if (0xa0..=0xa5).contains(&vk) {
                continue;
            }
            assert_eq!(
                silent_line(1, [vk], detail),
                one,
                "vk {vk:#04x} shows in the line"
            );
        }
        assert!(one.contains("; 1 key down; "));
    }
}
