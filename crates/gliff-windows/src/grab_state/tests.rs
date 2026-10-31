use super::Desktop;
use super::Event;
use super::Focus;
use super::Follow;
use super::GrabState;
use super::HookMode;
use super::Output;
use super::Pointer;
use super::PointerWatch;
use super::Release;
use super::Session;
use super::Typing;
use crate::lock_policy::Want;

fn run(state: &mut GrabState, events: &[Event]) -> Output {
    let mut last = None;
    for e in events {
        last = Some(state.handle(*e));
    }
    last.expect("at least one event")
}

const GAIN: Event = Event::Focus(Focus::Gained);
const LOSE: Event = Event::Focus(Focus::Lost);
const ENTER: Event = Event::Pointer(Pointer::Entered);
const LEAVE: Event = Event::Pointer(Pointer::Left);

mod focus {
    use super::*;

    fn state() -> GrabState {
        GrabState::new(Follow::Focus)
    }

    #[test]
    fn focus_grabs_and_the_lock_setting_follows() {
        let mut s = state();
        let o = run(&mut s, &[GAIN]);
        assert_eq!((o.mode, o.lock), (HookMode::Grab, Want::On));
        assert_eq!(o.release, Release::Nothing);
    }

    #[test]
    fn losing_focus_releases_every_held_key() {
        let mut s = state();
        run(&mut s, &[GAIN]);
        let o = run(&mut s, &[LOSE]);
        assert_eq!(
            (o.mode, o.release, o.lock),
            (HookMode::Off, Release::All, Want::Off)
        );
    }

    #[test]
    fn the_pointer_leaving_changes_nothing_while_focused() {
        let mut s = state();
        run(&mut s, &[GAIN, ENTER]);
        let o = run(&mut s, &[LEAVE]);
        assert_eq!(o.mode, HookMode::Grab, "keys stay with the focused window");
        assert_eq!(o.release, Release::Nothing);
    }

    #[test]
    fn hovering_without_focus_takes_only_super_l() {
        let mut s = state();
        let o = run(&mut s, &[ENTER]);
        assert_eq!((o.mode, o.lock), (HookMode::SuperLOnly, Want::On));
        let o = run(&mut s, &[LEAVE]);
        assert_eq!((o.mode, o.lock), (HookMode::Off, Want::Off));
        assert_eq!(o.release, Release::Nothing, "SuperLOnly held no keys");
    }

    #[test]
    fn focus_while_hovering_goes_from_super_l_only_to_grab() {
        let mut s = state();
        run(&mut s, &[ENTER]);
        let o = run(&mut s, &[GAIN]);
        assert_eq!(o.mode, HookMode::Grab);
        assert_eq!(o.release, Release::Nothing);
    }

    #[test]
    fn the_release_chord_ungrabs_until_a_click() {
        let mut s = state();
        run(&mut s, &[GAIN]);
        let o = run(&mut s, &[Event::ReleaseChord]);
        assert_eq!(
            (o.mode, o.release, o.lock),
            (HookMode::Off, Release::All, Want::Off)
        );
        // Focus changes do not re-arm it.
        let o = run(&mut s, &[LOSE, GAIN]);
        assert_eq!(
            o.mode,
            HookMode::Off,
            "still released after a focus round trip"
        );
        let o = run(&mut s, &[Event::Click]);
        assert_eq!(o.mode, HookMode::Grab, "a click takes the grab back");
    }

    #[test]
    fn released_but_hovering_still_forwards_super_l() {
        // "To lock the Windows machine, press the release chord and move away."
        let mut s = state();
        run(&mut s, &[GAIN, ENTER]);
        let o = run(&mut s, &[Event::ReleaseChord]);
        assert_eq!((o.mode, o.lock), (HookMode::SuperLOnly, Want::On));
        let o = run(&mut s, &[LEAVE]);
        assert_eq!(
            (o.mode, o.lock),
            (HookMode::Off, Want::Off),
            "moving away frees Win+L"
        );
    }

    #[test]
    fn a_click_while_not_released_changes_nothing() {
        let mut s = state();
        run(&mut s, &[GAIN]);
        let o = run(&mut s, &[Event::Click]);
        assert_eq!((o.mode, o.release), (HookMode::Grab, Release::Nothing));
    }
}

mod hover {
    use super::*;

    fn state() -> GrabState {
        GrabState::new(Follow::Hover)
    }

    #[test]
    fn the_pointer_over_the_window_grabs_even_without_focus() {
        let mut s = state();
        let o = run(&mut s, &[ENTER]);
        assert_eq!((o.mode, o.lock), (HookMode::Grab, Want::On));
    }

    #[test]
    fn leaving_releases_every_held_key() {
        let mut s = state();
        run(&mut s, &[ENTER]);
        let o = run(&mut s, &[LEAVE]);
        assert_eq!((o.mode, o.release), (HookMode::Off, Release::All));
    }

    #[test]
    fn the_release_chord_ungrabs_until_the_pointer_leaves_and_comes_back() {
        let mut s = state();
        run(&mut s, &[ENTER]);
        let o = run(&mut s, &[Event::ReleaseChord]);
        assert_eq!((o.mode, o.release), (HookMode::SuperLOnly, Release::All));
        assert_eq!(
            run(&mut s, &[Event::Click]).mode,
            HookMode::SuperLOnly,
            "a click does not re-arm"
        );
        assert_eq!(run(&mut s, &[LEAVE]).mode, HookMode::Off);
        assert_eq!(
            run(&mut s, &[ENTER]).mode,
            HookMode::Grab,
            "re-armed by leaving"
        );
    }
}

mod secure_desktop_and_lock {
    use super::*;

    #[test]
    fn the_secure_desktop_releases_and_turns_everything_off_in_both_modes() {
        for follow in [Follow::Focus, Follow::Hover] {
            let mut s = GrabState::new(follow);
            run(&mut s, &[GAIN, ENTER]);
            let o = run(&mut s, &[Event::Desktop(Desktop::Secure)]);
            assert_eq!(
                (o.mode, o.release, o.lock),
                (HookMode::Off, Release::All, Want::Off),
                "{follow:?}"
            );
            let o = run(&mut s, &[Event::Desktop(Desktop::Normal)]);
            assert_eq!(
                o.mode,
                HookMode::Grab,
                "back on the normal desktop, {follow:?}"
            );
        }
    }

    #[test]
    fn a_locked_session_releases_and_stays_off_until_unlocked() {
        let mut s = GrabState::new(Follow::Focus);
        run(&mut s, &[GAIN]);
        let o = run(&mut s, &[Event::Session(Session::Locked)]);
        assert_eq!((o.mode, o.release), (HookMode::Off, Release::All));
        assert_eq!(
            run(&mut s, &[ENTER]).mode,
            HookMode::Off,
            "no hover route while locked"
        );
        assert_eq!(
            run(&mut s, &[Event::Session(Session::Unlocked)]).mode,
            HookMode::Grab
        );
    }

    #[test]
    fn a_repeated_event_does_not_release_twice() {
        let mut s = GrabState::new(Follow::Focus);
        run(&mut s, &[GAIN]);
        assert_eq!(run(&mut s, &[LOSE]).release, Release::All);
        assert_eq!(run(&mut s, &[LOSE]).release, Release::Nothing);
    }
}

#[test]
fn enter_leave_focus_sequences_never_leave_the_setting_on_with_the_hook_off() {
    // Every sequence of up to four events keeps lock == (mode != Off).
    let events = [
        GAIN,
        LOSE,
        ENTER,
        LEAVE,
        Event::Click,
        Event::ReleaseChord,
        Event::Desktop(Desktop::Secure),
        Event::Desktop(Desktop::Normal),
        Event::Session(Session::Locked),
        Event::Session(Session::Unlocked),
        Event::Typing(Typing::Started),
        Event::Typing(Typing::Ended),
    ];
    for follow in [Follow::Focus, Follow::Hover] {
        for a in events {
            for b in events {
                for c in events {
                    let mut s = GrabState::new(follow);
                    for e in [a, b, c] {
                        let o = s.handle(e);
                        let want = if o.mode == HookMode::Off {
                            Want::Off
                        } else {
                            Want::On
                        };
                        assert_eq!(o.lock, want, "{follow:?} after {a:?} {b:?} {c:?}");
                    }
                }
            }
        }
    }
}

/// The address bar's field (section 4 of the tabs design): while it is open
/// and has the keyboard, keys belong to it, whatever the grab follows.
mod typing {
    use super::*;

    const START: Event = Event::Typing(Typing::Started);
    const END: Event = Event::Typing(Typing::Ended);

    #[test]
    fn typing_in_focus_mode_releases_the_grab_and_ending_it_grabs_again() {
        let mut s = GrabState::new(Follow::Focus);
        run(&mut s, &[GAIN]);
        let o = run(&mut s, &[START]);
        assert_eq!(
            (o.mode, o.release, o.lock),
            (HookMode::Off, Release::All, Want::Off)
        );
        let o = run(&mut s, &[END]);
        assert_eq!((o.mode, o.lock), (HookMode::Grab, Want::On));
    }

    #[test]
    fn typing_in_hover_mode_releases_the_grab_with_the_pointer_over_the_window() {
        let mut s = GrabState::new(Follow::Hover);
        run(&mut s, &[ENTER]);
        let o = run(&mut s, &[START]);
        // Not even Super+L is taken: a field wants the keys it is given.
        assert_eq!(
            (o.mode, o.release, o.lock),
            (HookMode::Off, Release::All, Want::Off)
        );
        // Moving about while typing changes nothing.
        assert_eq!(run(&mut s, &[LEAVE, ENTER]).mode, HookMode::Off);
        assert_eq!(run(&mut s, &[END]).mode, HookMode::Grab);
    }

    #[test]
    fn typing_that_began_before_the_pointer_arrived_holds_off_the_grab() {
        let mut s = GrabState::new(Follow::Hover);
        run(&mut s, &[START]);
        assert_eq!(run(&mut s, &[ENTER]).mode, HookMode::Off);
        assert_eq!(run(&mut s, &[END]).mode, HookMode::Grab);
    }

    #[test]
    fn ending_while_the_pointer_is_away_leaves_the_hook_off() {
        let mut s = GrabState::new(Follow::Hover);
        run(&mut s, &[ENTER, START, LEAVE]);
        assert_eq!(run(&mut s, &[END]).mode, HookMode::Off);
    }

    #[test]
    fn the_release_chord_before_typing_stays_in_force_after_it() {
        let mut s = GrabState::new(Follow::Focus);
        run(&mut s, &[GAIN, Event::ReleaseChord, START]);
        assert_eq!(run(&mut s, &[END]).mode, HookMode::Off);
    }

    #[test]
    fn a_repeated_start_does_not_release_twice() {
        let mut s = GrabState::new(Follow::Focus);
        run(&mut s, &[GAIN, START]);
        assert_eq!(run(&mut s, &[START]).release, Release::Nothing);
    }

    #[test]
    fn the_secure_desktop_still_wins_when_typing_ends() {
        let mut s = GrabState::new(Follow::Focus);
        run(&mut s, &[GAIN, START, Event::Desktop(Desktop::Secure)]);
        assert_eq!(run(&mut s, &[END]).mode, HookMode::Off);
    }
}

mod timeout {
    use super::*;

    const SECS: u32 = 30;

    fn state(follow: Follow) -> GrabState {
        GrabState::with_timeout(follow, Some(SECS))
    }

    fn tick(s: &mut GrabState, ms: u32) -> Output {
        s.handle(Event::Tick(ms))
    }

    #[test]
    fn focus_grab_is_let_go_when_the_time_is_up_and_not_before() {
        let mut s = state(Follow::Focus);
        tick(&mut s, 1_000);
        s.handle(GAIN);
        let o = tick(&mut s, 30_999);
        assert_eq!((o.mode, o.timed_out), (HookMode::Grab, false), "1 ms short");
        let o = tick(&mut s, 31_000);
        assert!(o.timed_out, "30 s after the grab began");
        assert_eq!(o.release, Release::All, "held keys are let go");
        assert_ne!(o.mode, HookMode::Grab, "the hook stops swallowing");
        assert_eq!(o.lock, Want::Off, "not hovered: the lock setting too");
    }

    #[test]
    fn a_click_grabs_again_for_a_full_period() {
        let mut s = state(Follow::Focus);
        s.handle(GAIN);
        assert!(tick(&mut s, 30_000).timed_out);
        let o = tick(&mut s, 60_000);
        assert!(!o.timed_out, "ungrabbed: nothing to time out");
        let o = s.handle(Event::Click);
        assert_eq!(o.mode, HookMode::Grab, "re-armed by the click");
        assert!(!tick(&mut s, 89_875).timed_out);
        assert!(
            tick(&mut s, 90_000).timed_out,
            "30 s after the click's tick"
        );
    }

    #[test]
    fn a_hover_grab_times_out_and_leave_then_enter_grabs_again() {
        let mut s = state(Follow::Hover);
        s.handle(ENTER);
        let o = tick(&mut s, 30_000);
        assert!(o.timed_out);
        assert_eq!(o.mode, HookMode::SuperLOnly, "still hovered");
        assert_eq!(o.release, Release::All);
        let o = tick(&mut s, 45_000);
        assert_eq!(
            o.mode,
            HookMode::SuperLOnly,
            "stays ungrabbed while hovered"
        );
        s.handle(LEAVE);
        let o = s.handle(ENTER);
        assert_eq!(o.mode, HookMode::Grab, "re-armed by leave and enter");
    }

    #[test]
    fn time_off_the_grab_does_not_count() {
        let mut s = state(Follow::Focus);
        tick(&mut s, 0);
        s.handle(GAIN);
        tick(&mut s, 20_000);
        s.handle(LOSE);
        tick(&mut s, 25_000);
        s.handle(GAIN);
        assert!(!tick(&mut s, 50_000).timed_out, "25 s into the second grab");
        assert!(tick(&mut s, 55_000).timed_out, "30 s into the second grab");
    }

    #[test]
    fn the_release_chord_comes_first_and_the_timer_then_has_nothing_to_do() {
        let mut s = state(Follow::Focus);
        s.handle(GAIN);
        let o = s.handle(Event::ReleaseChord);
        assert_eq!(o.release, Release::All);
        assert!(!tick(&mut s, 60_000).timed_out);
    }

    #[test]
    fn without_the_option_a_grab_has_no_limit() {
        let mut s = GrabState::new(Follow::Focus);
        s.handle(GAIN);
        let o = tick(&mut s, u32::MAX / 2);
        assert_eq!((o.mode, o.timed_out), (HookMode::Grab, false));
    }
}

#[test]
fn a_pointer_poll_that_agrees_changes_nothing() {
    let mut w = PointerWatch::default();
    for _ in 0..5 {
        assert_eq!(w.observe(true, Some(true)), None);
        assert_eq!(w.observe(false, Some(false)), None);
    }
}

#[test]
fn a_leave_that_never_came_is_made_up_after_two_ticks() {
    let mut w = PointerWatch::default();
    assert_eq!(w.observe(true, Some(false)), None, "one tick may be a race");
    assert_eq!(w.observe(true, Some(false)), Some(Pointer::Left));
}

#[test]
fn an_enter_that_never_came_is_made_up_after_two_ticks() {
    let mut w = PointerWatch::default();
    assert_eq!(w.observe(false, Some(true)), None);
    assert_eq!(w.observe(false, Some(true)), Some(Pointer::Entered));
}

#[test]
fn a_disagreement_that_goes_away_starts_the_count_again() {
    let mut w = PointerWatch::default();
    assert_eq!(w.observe(true, Some(false)), None);
    assert_eq!(w.observe(true, Some(true)), None, "the message came");
    assert_eq!(w.observe(true, Some(false)), None, "one tick again");
    assert_eq!(w.observe(true, None), None, "no answer: start again");
    assert_eq!(w.observe(true, Some(false)), None);
}

#[test]
fn the_made_up_leave_releases_a_hover_grab_like_the_real_one() {
    let mut s = GrabState::new(Follow::Hover);
    let o = run(&mut s, &[ENTER]);
    assert_eq!(o.mode, HookMode::Grab);
    let mut w = PointerWatch::default();
    let _ = w.observe(true, Some(false));
    let made_up = w.observe(true, Some(false)).expect("a leave");
    let o = s.handle(Event::Pointer(made_up));
    assert_eq!(o.mode, HookMode::Off);
    assert_eq!(o.release, Release::All);
    assert_eq!(o.lock, Want::Off);
}

#[test]
fn describe_names_every_flag_the_mode_is_made_of() {
    let mut state = GrabState::new(Follow::Hover);
    state.handle(Event::Pointer(Pointer::Entered));
    let text = state.describe();
    for want in [
        "follow=Hover",
        "focused=false",
        "hovered=true",
        "armed=true",
    ] {
        assert!(text.contains(want), "{want} in {text}");
    }
    assert!(state.hovered());
    state.handle(Event::Pointer(Pointer::Left));
    assert!(!state.hovered());
}

#[test]
fn a_poll_enter_that_a_leave_message_undoes_is_not_made_up_again_without_a_move() {
    let mut w = PointerWatch::default();
    assert_eq!(w.observe(false, Some(true)), None);
    assert_eq!(w.observe(false, Some(true)), Some(Pointer::Entered));
    // Windows says the pointer left (it is over a child window).
    w.left_by_message();
    for _ in 0..50 {
        assert_eq!(
            w.observe(false, Some(true)),
            None,
            "no mouse message came: the poll does not enter again"
        );
    }
    // A real move comes, and the poll may be believed again.
    w.moved_by_message();
    assert_eq!(w.observe(false, Some(true)), None);
    assert_eq!(w.observe(false, Some(true)), Some(Pointer::Entered));
}

#[test]
fn a_leave_message_after_a_real_move_does_not_stop_the_poll() {
    let mut w = PointerWatch::default();
    w.moved_by_message();
    w.left_by_message();
    assert_eq!(w.observe(false, Some(true)), None);
    assert_eq!(
        w.observe(false, Some(true)),
        Some(Pointer::Entered),
        "the pointer was moved from outside and no move message came"
    );
}

#[test]
fn a_pointer_over_a_child_window_is_not_over_the_window() {
    use super::Hit;
    use super::pointer_over;
    // Windows sends WM_MOUSELEAVE for a child, so the poll must agree.
    assert_eq!(pointer_over(Hit::Window, true), Some(true));
    assert_eq!(pointer_over(Hit::Window, false), Some(false));
    assert_eq!(pointer_over(Hit::OwnChild, true), Some(false));
    assert_eq!(pointer_over(Hit::Other, true), Some(false));
}
