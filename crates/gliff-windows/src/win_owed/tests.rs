use super::WinOwed;
use crate::key_ledger::Physical;
use crate::key_ledger::Seen;
use crate::keymap::Press;
use crate::modifiers::VK_LWIN;
use crate::modifiers::VK_RWIN;
use crate::modifiers::names;
use crate::modifiers::to_release_where;

const L: u16 = 0x4c;
const F: u16 = 0x46;

/// Issue #35, the second report: with the lock switched off, Win+L in a grab.
/// The hook swallows Win's Down and Up (they go to the remote); Windows took
/// Win as down ahead of the hook, and `GetAsyncKeyState` says no key is down.
/// The grab-end release must still give Windows the Win key's Up.
#[test]
fn win_l_in_a_grab_leaves_windows_owed_the_win_up_at_the_grab_end() {
    let mut real = Physical::new();
    let mut owed = WinOwed::new();
    for (vk, press) in [
        (VK_LWIN, Press::Down),
        (L, Press::Down),
        (L, Press::Up),
        (VK_LWIN, Press::Up),
    ] {
        real.saw(vk, press);
        owed.swallowed(vk);
    }
    let windows_says_down = |_| false;
    let due = owed.due(|vk| real.seen(vk) == Seen::Down);
    assert_eq!(due, [VK_LWIN]);
    let released = to_release_where(
        windows_says_down,
        |down, vk| real.stuck_at_grab_end(down, vk),
        &due,
    );
    assert_eq!(names(&released), "LWin", "released though Windows says up");
    let gated_on_windows = to_release_where(
        windows_says_down,
        |down, vk| real.stuck_at_grab_end(down, vk),
        &[],
    );
    assert!(
        gated_on_windows.is_empty(),
        "the release before this fix, gated on GetAsyncKeyState, saw nothing"
    );
}

#[test]
fn a_win_key_the_user_still_holds_is_not_due() {
    let mut owed = WinOwed::new();
    owed.swallowed(VK_LWIN);
    assert!(owed.due(|vk| vk == VK_LWIN).is_empty());
    assert_eq!(owed.owed(), [VK_LWIN], "still owed, for the log");
}

#[test]
fn an_up_that_reaches_windows_pays_what_is_owed() {
    let mut owed = WinOwed::new();
    owed.swallowed(VK_LWIN);
    owed.swallowed(VK_RWIN);
    owed.windows_saw_up(VK_RWIN);
    assert_eq!(owed.owed(), [VK_LWIN]);
    owed.settle(&[VK_LWIN]);
    assert!(owed.owed().is_empty());
}

#[test]
fn only_win_keys_are_owed() {
    let mut owed = WinOwed::new();
    owed.swallowed(F);
    owed.swallowed(0xa2);
    assert!(owed.owed().is_empty());
}

#[test]
fn a_grab_end_forgets_what_it_did_not_give() {
    let mut owed = WinOwed::new();
    owed.swallowed(VK_LWIN);
    owed.swallowed(VK_RWIN);
    owed.clear();
    assert_eq!(owed, WinOwed::new());
}
