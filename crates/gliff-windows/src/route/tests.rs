//! The input route's ordering guarantee and the clipboard rules.

use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use super::ClipRouter;
use super::InputRoute;
use crate::tabs::SessionId;

fn id(number: u32) -> SessionId {
    SessionId::from_raw(number)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Cmd {
    Key(u32),
    Release,
}

/// What a session holds after replaying its channel: a down key stays until
/// a release or its own up.
fn held_after(rx: &mut mpsc::UnboundedReceiver<Cmd>) -> usize {
    let mut held = 0usize;
    while let Ok(command) = rx.try_recv() {
        match command {
            Cmd::Key(_) => held += 1,
            Cmd::Release => held = 0,
        }
    }
    held
}

#[test]
fn a_command_goes_to_the_current_session_and_nowhere_when_there_is_none() {
    let route = InputRoute::new();
    assert!(
        !route.send(Cmd::Key(1)),
        "no session: the event is not taken"
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    assert_eq!(route.switch(Some((id(1), tx))), None);
    assert!(route.send(Cmd::Key(2)));
    assert_eq!(rx.try_recv(), Ok(Cmd::Key(2)));
    assert_eq!(route.target(), Some(id(1)));
}

#[test]
fn a_closed_channel_means_the_event_is_not_taken() {
    let route = InputRoute::new();
    let (tx, rx) = mpsc::unbounded_channel();
    route.switch(Some((id(1), tx)));
    drop(rx);
    assert!(!route.send(Cmd::Key(1)));
}

#[test]
fn a_switch_returns_the_old_session_and_sends_follow_the_new_one() {
    let route = InputRoute::new();
    let (tx1, mut rx1) = mpsc::unbounded_channel();
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    route.switch(Some((id(1), tx1)));
    assert_eq!(route.switch(Some((id(2), tx2))), Some(id(1)));
    route.send(Cmd::Key(7));
    assert_eq!(rx1.try_recv().ok(), None);
    assert_eq!(rx2.try_recv(), Ok(Cmd::Key(7)));
    assert_eq!(route.switch(None), Some(id(2)));
    assert!(!route.send(Cmd::Key(8)));
}

/// One round: a thread sends keys while the UI thread switches the route and
/// then releases the old session. Returns what the old session still holds.
fn round_with(send: impl Fn(&InputRoute<Cmd>, Cmd) + Sync + Send + 'static) -> usize {
    let route = Arc::new(InputRoute::new());
    let (old_tx, mut old_rx) = mpsc::unbounded_channel();
    let (new_tx, _new_rx) = mpsc::unbounded_channel();
    route.switch(Some((id(1), old_tx.clone())));
    let stop = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(2));
    let sender = {
        let (route, stop, start) = (route.clone(), stop.clone(), start.clone());
        std::thread::spawn(move || {
            start.wait();
            let mut key = 0;
            while !stop.load(Ordering::Relaxed) {
                send(&route, Cmd::Key(key));
                key += 1;
            }
        })
    };
    start.wait();
    std::thread::yield_now();
    // What the UI thread does for a switch: move the route, then release.
    route.switch(Some((id(2), new_tx)));
    let _ = old_tx.send(Cmd::Release);
    stop.store(true, Ordering::Relaxed);
    sender.join().expect("the sender thread");
    held_after(&mut old_rx)
}

#[test]
fn the_old_session_ends_every_key_up_whatever_the_timing() {
    for round in 0..200 {
        let held = round_with(|route, command| {
            route.send(command);
        });
        assert_eq!(held, 0, "round {round}: keys arrived behind the release");
    }
}

/// The wrong design: the sender is copied out of the lock and used after it.
/// The test above must be able to tell it apart, so this shows it can.
fn send_outside_the_lock(route: &InputRoute<Cmd>, gap: &dyn Fn(), command: Cmd) -> bool {
    let sender = {
        let target = route.target.lock().expect("lock");
        target.as_ref().map(|(_, sender)| sender.clone())
    };
    gap();
    sender.is_some_and(|sender| sender.send(command).is_ok())
}

#[test]
fn a_sender_copied_out_of_the_lock_would_leave_a_key_down() {
    let route = InputRoute::new();
    let (old_tx, mut old_rx) = mpsc::unbounded_channel();
    let (new_tx, _new_rx) = mpsc::unbounded_channel();
    route.switch(Some((id(1), old_tx.clone())));
    // The hook copies the sender, the UI thread switches and releases, and
    // only then does the hook send: the key lands behind the release.
    let gap = || {
        route.switch(Some((id(2), new_tx.clone())));
        let _ = old_tx.send(Cmd::Release);
    };
    assert!(send_outside_the_lock(&route, &gap, Cmd::Key(1)));
    assert_eq!(held_after(&mut old_rx), 1, "the wrong design leaks a key");
}

#[test]
fn the_real_send_cannot_be_interleaved_that_way() {
    // The same sequence with the real `send`: the send finishes inside the
    // lock before `switch` can run, so the release is last.
    let route = InputRoute::new();
    let (old_tx, mut old_rx) = mpsc::unbounded_channel();
    let (new_tx, _new_rx) = mpsc::unbounded_channel();
    route.switch(Some((id(1), old_tx.clone())));
    route.send(Cmd::Key(1));
    route.switch(Some((id(2), new_tx)));
    let _ = old_tx.send(Cmd::Release);
    assert_eq!(held_after(&mut old_rx), 0);
}

fn router(running: &[u32], shown: Option<u32>) -> ClipRouter {
    let mut router = ClipRouter::new();
    for number in running {
        router.session_started(id(*number));
    }
    router.show(shown.map(id));
    router
}

#[test]
fn a_local_change_revokes_every_running_session_and_offers_the_shown_one() {
    let router = router(&[1, 2, 3], Some(2));
    assert_eq!(router.revoke_targets(), [id(1), id(2), id(3)]);
    assert_eq!(router.offer_target(), Some(id(2)));
}

#[test]
fn nothing_is_offered_when_nothing_is_shown_or_the_shown_one_is_gone() {
    let mut router = router(&[1], None);
    assert_eq!(router.offer_target(), None);
    router.show(Some(id(9)));
    assert_eq!(router.offer_target(), None, "no engine for it");
}

#[test]
fn a_background_sessions_remote_offer_is_ignored() {
    let mut router = router(&[1, 2], Some(1));
    assert!(!router.remote_offer(id(2), false));
    assert_eq!(router.fetch_target(), None, "it took nothing");
    assert!(router.remote_offer(id(1), false));
    assert_eq!(router.fetch_target(), Some(id(1)));
}

#[test]
fn a_paste_goes_to_the_owner_even_after_a_switch() {
    let mut router = router(&[1, 2], Some(1));
    router.remote_offer(id(1), false);
    router.show(Some(id(2)));
    assert_eq!(router.fetch_target(), Some(id(1)));
    assert!(
        router.remote_offer(id(2), false),
        "the shown one now owns it"
    );
    assert_eq!(router.fetch_target(), Some(id(2)));
}

#[test]
fn an_empty_offer_from_the_owner_clears_ownership_and_one_from_another_does_not() {
    let mut router = router(&[1, 2], Some(1));
    router.remote_offer(id(1), false);
    router.show(Some(id(2)));
    assert!(
        router.remote_offer(id(2), true),
        "passed on: it clears Windows too"
    );
    assert_eq!(
        router.fetch_target(),
        Some(id(1)),
        "id 1 still owns the clipboard"
    );
    router.show(Some(id(1)));
    router.remote_offer(id(1), true);
    assert_eq!(router.fetch_target(), None);
}

#[test]
fn only_the_shown_session_may_read_the_local_clipboard() {
    let router = router(&[1, 2], Some(1));
    assert!(router.read_local_allowed(id(1)));
    assert!(!router.read_local_allowed(id(2)));
}

#[test]
fn when_the_owner_goes_its_data_is_withdrawn_and_pastes_have_no_target() {
    let mut router = router(&[1, 2], Some(1));
    router.remote_offer(id(1), false);
    let gone = router.session_gone(id(1));
    assert!(gone.withdraw_remote);
    assert_eq!(router.fetch_target(), None);
    assert_eq!(router.revoke_targets(), [id(2)]);
    assert!(
        !router.session_gone(id(2)).withdraw_remote,
        "a non-owner leaves quietly"
    );
}

#[test]
fn a_session_that_is_not_the_owner_leaving_keeps_the_owners_data() {
    let mut router = router(&[1, 2], Some(1));
    router.remote_offer(id(1), false);
    assert!(!router.session_gone(id(2)).withdraw_remote);
    assert_eq!(router.fetch_target(), Some(id(1)));
}

#[test]
fn transfer_ids_of_two_engines_stay_apart_and_cancel_goes_to_the_right_one() {
    let mut router = router(&[1, 2], Some(1));
    let a = router.global_transfer(id(1), 1);
    let b = router.global_transfer(id(2), 1);
    assert_ne!(a, b, "both engines count from 1");
    assert_eq!(router.global_transfer(id(1), 1), a, "stable");
    assert_eq!(router.cancel_target(a), Some((id(1), 1)));
    assert_eq!(router.cancel_target(b), Some((id(2), 1)));
    router.session_gone(id(1));
    assert_eq!(router.cancel_target(a), None, "the engine is gone");
    assert_eq!(router.cancel_target(b), Some((id(2), 1)));
    assert_eq!(router.cancel_target(4242), None);
}

#[test]
fn a_sessions_transfers_are_listed_by_the_engines_own_ids() {
    let mut router = router(&[1, 2], Some(1));
    router.global_transfer(id(1), 3);
    router.global_transfer(id(1), 1);
    router.global_transfer(id(2), 1);
    assert_eq!(router.transfers_of(id(1)), [1, 3]);
    assert_eq!(router.transfers_of(id(2)), [1]);
    router.session_gone(id(1));
    assert_eq!(router.transfers_of(id(1)), Vec::<u32>::new());
}
