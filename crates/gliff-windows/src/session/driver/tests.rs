use tokio::sync::mpsc;

use std::time::Duration;
use std::time::Instant;

use super::COALESCE_LIMIT;
use super::KeyClock;
use super::SessionEnd;
use super::coalesce_motion;
use super::run_was_clean;
use crate::altgr::AltGrFilter;
use crate::altgr::Filtered;
use crate::altgr::Origin;
use crate::altgr::RawKey;
use crate::keymap::Prefix;
use crate::keymap::Press;
use crate::pointer::BTN_LEFT;
use crate::pointer::PointerEvent;
use crate::session::EndReason;
use crate::session::SessionStats;
use crate::session::UiCmd;

fn motion(x: f64) -> UiCmd {
    UiCmd::Pointer(PointerEvent::Motion { x, y: 0.0 })
}

fn click(press: Press) -> UiCmd {
    UiCmd::Pointer(PointerEvent::Button {
        button: BTN_LEFT,
        press,
    })
}

fn run(cmds: Vec<UiCmd>) -> Vec<UiCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut cmds = cmds.into_iter();
    let first = cmds.next().expect("at least one command");
    for cmd in cmds {
        tx.send(cmd).expect("the receiver is alive");
    }
    coalesce_motion(first, &mut rx)
}

#[test]
fn a_run_of_motions_becomes_the_last_one() {
    assert_eq!(
        run(vec![motion(1.0), motion(2.0), motion(3.0)]),
        vec![motion(3.0)]
    );
}

#[test]
fn a_button_between_motions_keeps_its_place() {
    assert_eq!(
        run(vec![
            motion(1.0),
            motion(2.0),
            click(Press::Down),
            motion(3.0),
            motion(4.0),
            click(Press::Up),
        ]),
        vec![
            motion(2.0),
            click(Press::Down),
            motion(4.0),
            click(Press::Up)
        ],
        "the pointer is where it was when the button changed"
    );
}

#[test]
fn keys_are_never_merged_or_reordered() {
    assert_eq!(
        run(vec![UiCmd::ReleaseInput, motion(1.0), UiCmd::Bye]),
        vec![UiCmd::ReleaseInput, motion(1.0), UiCmd::Bye]
    );
}

#[test]
fn one_pass_reads_at_most_the_limit() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    for i in 0..COALESCE_LIMIT * 2 {
        tx.send(motion(i as f64)).expect("the receiver is alive");
    }
    let out = coalesce_motion(motion(-1.0), &mut rx);
    assert_eq!(out.len(), 1, "all motions merge");
    assert!(
        rx.try_recv().is_ok(),
        "the pass stopped at the limit and left the rest queued"
    );
}

#[test]
fn the_deadline_is_late_enough_for_the_filter_to_give_up() {
    // The Ctrl Down came at hook time 1000 and reached the session now.
    let arrived = Instant::now();
    let clock = KeyClock {
        time_ms: 1000,
        arrived,
    };
    let mut filter = AltGrFilter::default();
    let down = RawKey {
        vk: 0xa2,
        scan: 0x1d,
        prefix: Prefix::None,
        press: Press::Down,
        time_ms: 1000,
        origin: Origin::Hook,
        injected: false,
    };
    assert_eq!(filter.push(down), Filtered::Hold);
    assert_eq!(
        filter.expire(clock.hook_ms(clock.deadline() - Duration::from_millis(1))),
        None
    );
    assert_eq!(filter.expire(clock.hook_ms(clock.deadline())), Some(down));
}

#[test]
fn the_deadline_is_fixed_by_the_key_not_by_when_it_is_asked() {
    let arrived = Instant::now();
    let clock = KeyClock {
        time_ms: 5,
        arrived,
    };
    assert_eq!(clock.deadline(), clock.deadline());
    assert!(clock.deadline() <= arrived + Duration::from_millis(12));
    assert!(clock.deadline() > arrived + Duration::from_millis(10));
}

#[test]
fn the_hook_clock_wraps_like_the_hook_times_do() {
    let arrived = Instant::now();
    let clock = KeyClock {
        time_ms: u32::MAX - 2,
        arrived,
    };
    assert_eq!(clock.hook_ms(arrived + Duration::from_millis(5)), 2);
}

fn ended(reason: Option<EndReason>, ssh_exit: Option<i32>) -> SessionEnd {
    SessionEnd {
        reason,
        ssh_exit,
        errors: Vec::new(),
        stats: SessionStats::default(),
        slot_dropped: 0,
        stderr_log: None,
    }
}

#[test]
fn a_run_that_never_opened_a_machine_and_was_closed_is_clean() {
    // The window may start empty and be closed from there.
    assert!(run_was_clean(0, &[]));
}

#[test]
fn a_run_with_a_session_that_said_bye_and_exited_zero_is_clean() {
    assert!(run_was_clean(0, &[ended(Some(EndReason::Bye), Some(0))]));
}

#[test]
fn a_session_that_never_started_or_ended_badly_is_not_clean() {
    assert!(!run_was_clean(1, &[]));
    assert!(!run_was_clean(0, &[ended(Some(EndReason::Bye), Some(255))]));
    assert!(!run_was_clean(0, &[ended(None, None)]));
}
