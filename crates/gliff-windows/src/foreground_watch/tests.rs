use super::Foreground;
use super::ForegroundWatch;
use super::Relation;
use super::classify;
use super::grabbing_by_focus;
use crate::grab_state::Desktop;
use crate::grab_state::Event;
use crate::grab_state::Focus;
use crate::grab_state::Follow;
use crate::grab_state::GrabState;
use crate::grab_state::HookMode;
use crate::grab_state::Release;
use crate::grab_state::Session;
use crate::grab_state::Typing;

const OURS: Foreground = Foreground::Ours;
const OTHER: Foreground = Foreground::Other;
const UNKNOWN: Foreground = Foreground::Unknown;

mod classify {
    use super::*;

    #[test]
    fn our_own_window_is_ours() {
        assert_eq!(classify(Relation::Is), OURS);
    }

    #[test]
    fn a_child_or_an_owned_popup_of_ours_is_ours() {
        // The address bar's field is a child; a context menu, a tooltip and
        // a dialog are owned by the window.
        assert_eq!(classify(Relation::OwnedOrChild), OURS);
    }

    #[test]
    fn any_other_window_of_our_process_is_ours() {
        // A menu window or an IME window made on our thread, owned by no
        // window of ours.
        assert_eq!(classify(Relation::SameProcess), OURS);
    }

    #[test]
    fn a_window_of_another_program_is_not_ours() {
        assert_eq!(classify(Relation::Foreign), OTHER);
    }

    #[test]
    fn no_foreground_window_and_an_unreadable_one_prove_nothing() {
        // `GetForegroundWindow` gives null while the foreground changes and
        // on the secure desktop; a window that cannot be asked about is no
        // evidence either way.
        assert_eq!(classify(Relation::None), UNKNOWN);
        assert_eq!(classify(Relation::Unreadable), UNKNOWN);
    }
}

mod when_it_looks {
    use super::*;

    #[test]
    fn only_a_focus_grab_is_checked() {
        assert!(grabbing_by_focus(Follow::Focus, HookMode::Grab));
        // Hover: the keyboard follows the pointer, so another program in
        // front is the point, not a fault.
        assert!(!grabbing_by_focus(Follow::Hover, HookMode::Grab));
    }

    #[test]
    fn only_while_grabbing() {
        assert!(!grabbing_by_focus(Follow::Focus, HookMode::Off));
        assert!(!grabbing_by_focus(Follow::Focus, HookMode::SuperLOnly));
    }

    #[test]
    fn a_watch_that_has_nothing_to_do_does_not_ask_windows() {
        let w = ForegroundWatch::default();
        assert!(!w.active(false));
        assert!(w.active(true));
    }
}

mod losing {
    use super::*;

    #[test]
    fn one_look_at_another_window_is_not_enough() {
        let mut w = ForegroundWatch::default();
        assert_eq!(w.look(true, OTHER), None);
    }

    #[test]
    fn two_looks_in_a_row_are_a_lost_focus() {
        let mut w = ForegroundWatch::default();
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(true, OTHER), Some(Focus::Lost));
    }

    #[test]
    fn our_own_window_in_between_starts_the_count_again() {
        let mut w = ForegroundWatch::default();
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(true, OURS), None);
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(true, OTHER), Some(Focus::Lost));
    }

    #[test]
    fn an_unknown_look_starts_the_count_again() {
        let mut w = ForegroundWatch::default();
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(true, UNKNOWN), None);
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(true, OTHER), Some(Focus::Lost));
    }

    #[test]
    fn looks_while_not_grabbing_do_not_count() {
        let mut w = ForegroundWatch::default();
        assert_eq!(w.look(false, OTHER), None);
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(false, OTHER), None);
        assert_eq!(w.look(true, OTHER), None);
    }

    #[test]
    fn it_says_lost_once() {
        let mut w = ForegroundWatch::default();
        w.look(true, OTHER);
        assert_eq!(w.look(true, OTHER), Some(Focus::Lost));
        // The grab has ended by now, so the caller says it is not grabbing.
        for _ in 0..10 {
            assert_eq!(w.look(false, OTHER), None);
        }
    }
}

mod regaining {
    use super::*;

    fn lost() -> ForegroundWatch {
        let mut w = ForegroundWatch::default();
        w.look(true, OTHER);
        assert_eq!(w.look(true, OTHER), Some(Focus::Lost));
        w
    }

    #[test]
    fn after_a_made_up_loss_the_watch_keeps_looking_until_it_is_settled() {
        let w = lost();
        assert!(w.active(false));
    }

    #[test]
    fn our_window_in_front_again_with_no_message_gives_the_focus_back() {
        let mut w = lost();
        assert_eq!(w.look(false, OURS), None);
        assert_eq!(w.look(false, OURS), Some(Focus::Gained));
        assert!(!w.active(false));
    }

    #[test]
    fn another_window_in_between_starts_that_count_again() {
        let mut w = lost();
        assert_eq!(w.look(false, OURS), None);
        assert_eq!(w.look(false, OTHER), None);
        assert_eq!(w.look(false, OURS), None);
        assert_eq!(w.look(false, OURS), Some(Focus::Gained));
    }

    #[test]
    fn a_real_focus_message_settles_it_and_the_watch_goes_quiet() {
        let mut w = lost();
        w.focus_message();
        assert!(!w.active(false));
        assert_eq!(w.look(false, OURS), None);
        assert_eq!(w.look(false, OURS), None);
    }

    #[test]
    fn a_real_focus_message_starts_the_loss_count_again() {
        let mut w = ForegroundWatch::default();
        w.look(true, OTHER);
        w.focus_message();
        assert_eq!(w.look(true, OTHER), None);
        assert_eq!(w.look(true, OTHER), Some(Focus::Lost));
    }
}

/// The watch and the grab state together, as the window drives them.
mod with_the_grab_state {
    use super::*;

    fn grabbing_state() -> GrabState {
        let mut s = GrabState::new(Follow::Focus);
        s.handle(Event::Focus(Focus::Gained));
        assert_eq!(s.mode(), HookMode::Grab);
        s
    }

    /// One look, as `Grab::tick` makes it: the watch decides, the state hears.
    fn look(
        w: &mut ForegroundWatch,
        s: &mut GrabState,
        follow: Follow,
        seen: Foreground,
    ) -> Option<crate::grab_state::Output> {
        let grabbing = grabbing_by_focus(follow, s.mode());
        if !w.active(grabbing) {
            return None;
        }
        w.look(grabbing, seen)
            .map(|focus| s.handle(Event::Focus(focus)))
    }

    #[test]
    fn a_game_in_front_ends_the_grab_the_normal_way_and_releases_held_keys() {
        let mut w = ForegroundWatch::default();
        let mut s = grabbing_state();
        assert!(look(&mut w, &mut s, Follow::Focus, OTHER).is_none());
        let out = look(&mut w, &mut s, Follow::Focus, OTHER).expect("ended");
        assert_eq!(out.mode, HookMode::Off);
        // The release is what sends the remote machine its `ReleaseInput`
        // and lets go of Windows' modifiers.
        assert_eq!(out.release, Release::All);
    }

    #[test]
    fn coming_back_without_a_message_grabs_again() {
        let mut w = ForegroundWatch::default();
        let mut s = grabbing_state();
        look(&mut w, &mut s, Follow::Focus, OTHER);
        look(&mut w, &mut s, Follow::Focus, OTHER);
        assert_eq!(s.mode(), HookMode::Off);
        assert!(look(&mut w, &mut s, Follow::Focus, OURS).is_none());
        let out = look(&mut w, &mut s, Follow::Focus, OURS).expect("grabbed");
        assert_eq!(out.mode, HookMode::Grab);
    }

    #[test]
    fn hover_mode_is_left_alone() {
        let mut w = ForegroundWatch::default();
        let mut s = GrabState::new(Follow::Hover);
        s.handle(Event::Pointer(crate::grab_state::Pointer::Entered));
        assert_eq!(s.mode(), HookMode::Grab);
        for _ in 0..10 {
            assert!(look(&mut w, &mut s, Follow::Hover, OTHER).is_none());
        }
        assert_eq!(s.mode(), HookMode::Grab);
    }

    #[test]
    fn a_locked_session_is_not_looked_at() {
        // Win+L: the session locks, the state turns the hook off, and the
        // foreground is another program (the lock screen) for a long time.
        let mut w = ForegroundWatch::default();
        let mut s = grabbing_state();
        s.handle(Event::Session(Session::Locked));
        assert_eq!(s.mode(), HookMode::Off);
        for _ in 0..10 {
            assert!(look(&mut w, &mut s, Follow::Focus, OTHER).is_none());
        }
    }

    #[test]
    fn the_secure_desktop_is_not_looked_at() {
        let mut w = ForegroundWatch::default();
        let mut s = grabbing_state();
        s.handle(Event::Desktop(Desktop::Secure));
        for _ in 0..10 {
            assert!(look(&mut w, &mut s, Follow::Focus, UNKNOWN).is_none());
            assert!(look(&mut w, &mut s, Follow::Focus, OTHER).is_none());
        }
    }

    #[test]
    fn typing_in_the_address_bar_is_not_looked_at() {
        let mut w = ForegroundWatch::default();
        let mut s = grabbing_state();
        s.handle(Event::Typing(Typing::Started));
        for _ in 0..10 {
            assert!(look(&mut w, &mut s, Follow::Focus, OTHER).is_none());
        }
    }

    #[test]
    fn a_menu_or_the_address_bar_of_ours_in_front_keeps_the_grab_state_as_it_is() {
        let mut w = ForegroundWatch::default();
        let mut s = grabbing_state();
        for _ in 0..10 {
            assert!(
                look(
                    &mut w,
                    &mut s,
                    Follow::Focus,
                    classify(Relation::OwnedOrChild)
                )
                .is_none()
            );
        }
        assert_eq!(s.mode(), HookMode::Grab);
    }
}
