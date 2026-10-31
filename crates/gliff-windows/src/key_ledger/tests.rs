use super::KeyLedger;
use super::Physical;
use super::Route;
use super::Seen;
use super::describe_down;
use crate::keymap::Press;

const LWIN: u16 = 0x5b;
const LCTRL: u16 = 0xa2;
const LALT: u16 = 0xa4;
const TAB: u16 = 0x09;
const F: u16 = 0x46;

/// The virtual keys that are modifiers, generic and left/right.
const MODIFIER_VKS: [u16; 11] = [
    0x10, 0x11, 0x12, 0x5b, 0x5c, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5,
];

// What a grab did before the ledger: the keys held when it started.

#[test]
fn the_up_of_a_key_that_was_down_before_the_grab_is_taken_once() {
    let mut keys = KeyLedger::default();
    keys.seed([LALT]);
    assert_eq!(
        keys.route(LALT, Press::Up),
        Route::Windows { remote: false },
        "the Up goes to Windows"
    );
    assert_eq!(
        keys.route(LALT, Press::Up),
        Route::Remote,
        "a second Up is not its"
    );
}

#[test]
fn auto_repeats_of_a_held_key_stay_on_windows_until_it_comes_up() {
    let mut keys = KeyLedger::default();
    keys.seed([LWIN]);
    assert_eq!(
        keys.route(LWIN, Press::Down),
        Route::Windows { remote: false }
    );
    assert_eq!(
        keys.route(LWIN, Press::Down),
        Route::Windows { remote: false }
    );
    assert_eq!(
        keys.route(LWIN, Press::Up),
        Route::Windows { remote: false }
    );
    assert_eq!(
        keys.route(LWIN, Press::Down),
        Route::Remote,
        "pressed again after its Up, it is a key of the grab"
    );
}

#[test]
fn a_key_that_was_up_before_the_grab_is_never_taken() {
    let mut keys = KeyLedger::default();
    keys.seed([LALT]);
    assert_eq!(keys.route(TAB, Press::Down), Route::Remote);
    assert_eq!(keys.route(TAB, Press::Up), Route::Remote);
}

#[test]
fn seeding_again_replaces_the_ledger_and_clear_empties_it() {
    let mut keys = KeyLedger::default();
    keys.seed([LALT, LWIN]);
    assert_eq!(keys.windows_len(), 2);
    keys.seed([TAB]);
    assert_eq!(keys.windows_len(), 1);
    assert_eq!(keys.route(LALT, Press::Up), Route::Remote);
    keys.clear();
    assert_eq!(keys.windows_len(), 0);
    assert_eq!(keys.route(TAB, Press::Up), Route::Remote);
}

#[test]
fn a_code_past_the_table_is_ignored() {
    let mut keys = KeyLedger::default();
    keys.seed([0x1ff]);
    keys.windows_saw(0x1ff, Press::Down);
    keys.remote_saw(0x1ff, Press::Down);
    assert_eq!(keys.windows_len(), 0);
    assert_eq!(keys.route(0x1ff, Press::Up), Route::Remote);
    assert_eq!(keys.take_remote_held(), Vec::<u16>::new());
}

#[test]
fn a_replayed_key_is_taken_once_on_its_up() {
    let mut keys = KeyLedger::new();
    keys.windows_saw(LWIN, Press::Down);
    assert_eq!(keys.windows_len(), 1);
    assert_eq!(
        keys.route(LWIN, Press::Down),
        Route::Windows { remote: false },
        "a repeat stays local"
    );
    assert_eq!(
        keys.route(LWIN, Press::Up),
        Route::Windows { remote: false },
        "the Up is local"
    );
    assert_eq!(
        keys.route(LWIN, Press::Down),
        Route::Remote,
        "then the key is the remote's"
    );
}

// What the ledger adds: every Down Windows saw, across a reinstall.

#[test]
fn up_of_key_windows_saw_down_passes_after_reinstall() {
    let mut keys = KeyLedger::new();
    keys.seed([]);
    // Win went down while the hook was not looking, in the reinstall gap.
    // The new hook starts with the keys Windows holds down.
    keys.reinstall([LWIN]);
    assert_eq!(
        keys.route(LWIN, Press::Up),
        Route::Windows { remote: false },
        "the Up reaches Windows, which counted the Down"
    );
    assert_eq!(keys.route(LWIN, Press::Down), Route::Remote);

    // The same when Windows' own list of held keys missed it: the Down was
    // recorded when the hook passed it, and the ledger outlives the hook.
    keys.windows_saw(LCTRL, Press::Down);
    keys.reinstall([]);
    assert_eq!(
        keys.route(LCTRL, Press::Up),
        Route::Windows { remote: false }
    );
}

#[test]
fn a_reinstall_keeps_what_the_remote_saw() {
    let mut keys = KeyLedger::new();
    keys.seed([]);
    keys.remote_saw(LCTRL, Press::Down);
    keys.reinstall([LCTRL, LWIN]);
    assert_eq!(
        keys.route(LCTRL, Press::Up),
        Route::Windows { remote: true },
        "Windows holds Ctrl too, and the remote saw its Down: both get the Up"
    );
    assert_eq!(
        keys.route(LWIN, Press::Up),
        Route::Windows { remote: false },
        "the remote never saw Win"
    );
}

#[test]
fn pass_without_target_records_down() {
    let mut keys = KeyLedger::new();
    keys.seed([]);
    // No session was there: the hook let the Down through to Windows.
    keys.windows_saw(LCTRL, Press::Down);
    // The session is back by the time of the Up, which would otherwise be
    // swallowed and sent to the remote.
    assert_eq!(
        keys.route(LCTRL, Press::Down),
        Route::Windows { remote: false },
        "a repeat stays on Windows"
    );
    assert_eq!(
        keys.route(LCTRL, Press::Up),
        Route::Windows { remote: false }
    );
    assert_eq!(keys.route(F, Press::Down), Route::Remote);
}

#[test]
fn a_passed_up_ends_the_windows_side_of_the_key() {
    let mut keys = KeyLedger::new();
    keys.windows_saw(LCTRL, Press::Down);
    keys.windows_saw(LCTRL, Press::Up);
    assert_eq!(keys.windows_len(), 0);
    assert_eq!(keys.route(LCTRL, Press::Down), Route::Remote);
}

#[test]
fn a_key_sent_from_the_window_then_reinstall_sends_its_up_to_both() {
    let mut keys = KeyLedger::new();
    keys.seed([]);
    // The window got the Down and sent it to the remote; then the hook was
    // installed again.
    keys.legacy(LWIN, Press::Down, true);
    keys.reinstall([LWIN]);
    assert_eq!(keys.route(LWIN, Press::Up), Route::Windows { remote: true });
    assert_eq!(keys.take_remote_held(), Vec::<u16>::new(), "Up ended it");

    // The window could not send it: only Windows saw the Down.
    keys.legacy(LCTRL, Press::Down, false);
    assert_eq!(
        keys.route(LCTRL, Press::Up),
        Route::Windows { remote: false }
    );
}

#[test]
fn swallowed_down_then_mode_off_emits_remote_release() {
    let mut keys = KeyLedger::new();
    keys.seed([]);
    keys.remote_saw(F, Press::Down);
    keys.remote_saw(LCTRL, Press::Down);
    keys.remote_saw(TAB, Press::Down);
    keys.remote_saw(TAB, Press::Up);
    // The grab ends.
    keys.forget_windows();
    assert_eq!(
        keys.take_remote_held(),
        vec![F, LCTRL],
        "the remote still holds these two"
    );
    assert_eq!(
        keys.take_remote_held(),
        Vec::<u16>::new(),
        "they are released once"
    );
}

#[test]
fn leaving_the_grab_forgets_the_windows_side_only() {
    let mut keys = KeyLedger::new();
    keys.seed([LWIN]);
    keys.remote_saw(F, Press::Down);
    keys.forget_windows();
    assert_eq!(keys.windows_len(), 0);
    assert_eq!(keys.take_remote_held(), vec![F]);
}

// What the log may say.

#[test]
fn the_log_line_for_keys_down_has_a_count_and_modifier_groups() {
    assert_eq!(describe_down([]), "0 keys down");
    assert_eq!(describe_down([F]), "1 key down");
    assert_eq!(
        describe_down([LWIN, F, LCTRL, 0x74]),
        "4 keys down (Win, Ctrl)"
    );
    assert_eq!(
        describe_down([0xa1, 0xa5, 0x5c, 0xa3]),
        "4 keys down (Win, Ctrl, Alt, Shift)"
    );
}

#[test]
fn the_generic_modifier_codes_add_a_group_but_no_extra_key() {
    assert_eq!(describe_down([0x10, 0xa0]), "1 key down (Shift)");
    assert_eq!(
        describe_down([0x11, 0x12, 0xa2, 0xa4]),
        "2 keys down (Ctrl, Alt)"
    );
}

#[test]
fn the_log_line_names_no_key_other_than_modifier_groups() {
    for vk in 0u16..=0xff {
        if MODIFIER_VKS.contains(&vk) {
            continue;
        }
        assert_eq!(
            describe_down([vk]),
            "1 key down",
            "vk {vk:#04x} left a trace in the line"
        );
    }
    // Whatever is held, every word is from the fixed vocabulary, and the
    // modifier groups come in a fixed order, not the order of the keys.
    let vocabulary = ["key", "keys", "down", "Win", "Ctrl", "Alt", "Shift"];
    let sets: [&[u16]; 4] = [
        &[F, LCTRL, 0x35, 0x74, LWIN, 0x1b, 0x20],
        &[0xa0, 0x41, 0x42, 0x43, 0xa4],
        &(0u16..=0xff).collect::<Vec<_>>(),
        &[0xa4, 0xa0, LWIN, LCTRL],
    ];
    for held in sets {
        let line = describe_down(held.iter().copied());
        for word in line
            .replace(['(', ')', ','], " ")
            .split_whitespace()
            .filter(|w| w.parse::<usize>().is_err())
        {
            assert!(vocabulary.contains(&word), "{word:?} in {line:?}");
        }
    }
    assert_eq!(
        describe_down([0xa0, 0xa4, LWIN, LCTRL]),
        describe_down([LCTRL, LWIN, 0xa4, 0xa0]),
        "the order of the keys does not show"
    );
}

// The last-resort net: a modifier Windows counts as down while the user's
// own key is up is stuck; one the user holds is not.

#[test]
fn a_win_key_the_hook_saw_come_up_is_stuck_if_windows_still_holds_it() {
    let mut real = Physical::new();
    real.saw(LWIN, Press::Down);
    real.saw(LWIN, Press::Up);
    assert!(real.stuck_at_grab_end(true, LWIN));
    assert!(real.stuck_at_reinstall(true, LWIN));
    assert!(
        !real.stuck_at_grab_end(false, LWIN),
        "Windows holds nothing"
    );
}

#[test]
fn a_win_key_the_user_still_holds_is_not_released() {
    let mut real = Physical::new();
    real.saw(LWIN, Press::Down);
    assert_eq!(real.seen(LWIN), Seen::Down);
    assert!(!real.stuck_at_grab_end(true, LWIN));
    assert!(!real.stuck_at_reinstall(true, LWIN));
    real.saw(F, Press::Down);
    real.saw(F, Press::Up);
    assert_eq!(
        real.down_keys(),
        vec![LWIN],
        "a key that came up is not down"
    );
}

#[test]
fn a_key_the_hook_never_saw_is_released_at_a_grab_end_but_not_at_a_reinstall() {
    let real = Physical::new();
    assert_eq!(real.seen(LWIN), Seen::Unknown);
    assert!(
        real.stuck_at_grab_end(true, LWIN),
        "as before: the grab is over, nothing stays held"
    );
    assert!(
        !real.stuck_at_reinstall(true, LWIN),
        "mid-grab it may be a key held since before the grab"
    );
}

#[test]
fn the_ledger_lists_what_windows_saw_go_down() {
    let mut keys = KeyLedger::new();
    keys.windows_saw(LWIN, Press::Down);
    keys.windows_saw(F, Press::Down);
    keys.windows_saw(F, Press::Up);
    assert_eq!(keys.windows_held(), vec![LWIN]);
}
