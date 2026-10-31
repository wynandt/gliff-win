//! The tabs' rules, as lists of effects.

use super::Cause;
use super::Effect;
use super::End;
use super::SessionId;
use super::Status;
use super::SwitchTo;
use super::TabId;
use super::Tabs;
use crate::config::TabSet;
use crate::connect::SshFailure;
use crate::history::HostTarget;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

/// A fake clock: `at(n)` is `n` milliseconds after one fixed start.
fn at(ms: u64) -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now) + Duration::from_millis(ms)
}

fn host(text: &str) -> HostTarget {
    text.parse().expect("a host")
}

/// The session a `Start` effect made.
fn started(effects: &[Effect]) -> SessionId {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Start { session, .. } => Some(*session),
            _ => None,
        })
        .expect("a Start effect")
}

fn tab_of(tabs: &Tabs, name: &str) -> TabId {
    tabs.iter()
        .find(|tab| tab.target().same(&host(name)))
        .expect("the tab")
        .id()
}

fn session_of(tabs: &Tabs, name: &str) -> SessionId {
    tabs.tab(tab_of(tabs, name))
        .and_then(|tab| tab.session())
        .expect("a live session")
}

fn status_of(tabs: &Tabs, name: &str) -> Status {
    tabs.tab(tab_of(tabs, name))
        .map(|tab| tab.status().clone())
        .expect("the tab")
}

/// Three tabs a, b and c, all streaming, a shown.
fn three() -> Tabs {
    let mut tabs = Tabs::new();
    for name in ["a@x", "b@x", "c@x"] {
        let effects = tabs.open(&host(name));
        tabs.connected(started(&effects), at(0));
    }
    let a = tab_of(&tabs, "a@x");
    tabs.click(a);
    tabs
}

#[test]
fn opening_a_new_host_adds_a_tab_starts_it_and_shows_it() {
    let mut tabs = Tabs::new();
    let effects = tabs.open(&host("me@host"));
    let session = started(&effects);
    let tab = tab_of(&tabs, "me@host");
    assert_eq!(
        effects,
        vec![
            Effect::Start {
                tab,
                session,
                target: host("me@host"),
                cause: Cause::User
            },
            Effect::InputTo(Some(session)),
            Effect::Present(session),
            Effect::Persist,
        ],
        "a connecting tab is not offered the clipboard yet"
    );
    assert_eq!(tabs.shown(), Some(tab));
    assert_eq!(status_of(&tabs, "me@host"), Status::Connecting);
}

#[test]
fn opening_a_host_that_has_a_tab_clicks_that_tab() {
    let mut tabs = three();
    let effects = tabs.open(&host("b@X"));
    assert_eq!(
        tabs.len(),
        3,
        "the user part is exact, the host part is not"
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "b@x")));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Start { .. })));
    let other_user = tabs.open(&host("B@x"));
    assert_eq!(tabs.len(), 4, "a different user is a different machine");
    assert!(other_user.iter().any(|e| matches!(e, Effect::Start { .. })));
}

#[test]
fn a_switch_moves_the_route_before_anything_is_released() {
    let mut tabs = three();
    let (a, b) = (session_of(&tabs, "a@x"), session_of(&tabs, "b@x"));
    let effects = tabs.click(tab_of(&tabs, "b@x"));
    assert_eq!(
        effects,
        vec![
            Effect::InputTo(Some(b)),
            Effect::ReleaseInput(a),
            Effect::ClipRevoke(a),
            Effect::ClipCancel(a),
            Effect::ClipOffer(b),
            Effect::Present(b),
            Effect::Persist,
        ]
    );
}

#[test]
fn a_click_on_the_shown_running_tab_does_nothing() {
    let mut tabs = three();
    assert_eq!(tabs.click(tab_of(&tabs, "a@x")), Vec::new());
    assert_eq!(tabs.click(TabId(999)), Vec::new(), "no such tab");
}

#[test]
fn a_click_on_a_stopped_tab_starts_it_before_the_route_moves() {
    let mut tabs = three();
    let c = tab_of(&tabs, "c@x");
    tabs.stop(c);
    let a = session_of(&tabs, "a@x");
    let effects = tabs.click(c);
    let c_session = started(&effects);
    assert_eq!(
        effects,
        vec![
            Effect::Start {
                tab: c,
                session: c_session,
                target: host("c@x"),
                cause: Cause::User
            },
            Effect::InputTo(Some(c_session)),
            Effect::ReleaseInput(a),
            Effect::ClipRevoke(a),
            Effect::ClipCancel(a),
            Effect::Present(c_session),
            Effect::Persist,
        ]
    );
}

#[test]
fn a_session_id_is_never_reused() {
    let mut tabs = Tabs::new();
    let first = started(&tabs.open(&host("a@x")));
    let tab = tab_of(&tabs, "a@x");
    tabs.stop(tab);
    let second = started(&tabs.click(tab));
    assert_ne!(first, second);
}

#[test]
fn a_late_event_of_a_stopped_session_does_nothing() {
    let mut tabs = Tabs::new();
    let old = started(&tabs.open(&host("a@x")));
    let tab = tab_of(&tabs, "a@x");
    tabs.stop(tab);
    let new = started(&tabs.click(tab));
    assert_eq!(
        tabs.ended(old, End::Error("late".into()), at(0)),
        Vec::new()
    );
    assert_eq!(tabs.connected(old, at(0)), Vec::new());
    assert_eq!(status_of(&tabs, "a@x"), Status::Connecting);
    assert_eq!(session_of(&tabs, "a@x"), new);
    assert_eq!(
        tabs.ended(SessionId::from_raw(4000), End::Closed, at(0)),
        Vec::new()
    );
}

#[test]
fn a_tab_stopped_while_connecting_stops_its_session() {
    let mut tabs = Tabs::new();
    let session = started(&tabs.open(&host("a@x")));
    let tab = tab_of(&tabs, "a@x");
    let effects = tabs.stop(tab);
    assert!(effects.contains(&Effect::Stop(session)));
    assert!(effects.contains(&Effect::ClipGone(session)));
    assert_eq!(status_of(&tabs, "a@x"), Status::Stopped);
    assert_eq!(
        tabs.connected(session, at(0)),
        Vec::new(),
        "it connects too late"
    );
}

#[test]
fn stopping_the_shown_tab_shows_the_nearest_running_tab_right_first() {
    let mut tabs = three();
    let (a, b) = (session_of(&tabs, "a@x"), session_of(&tabs, "b@x"));
    let effects = tabs.stop(tab_of(&tabs, "a@x"));
    assert_eq!(
        effects,
        vec![
            Effect::InputTo(Some(b)),
            Effect::ReleaseInput(a),
            Effect::ClipRevoke(a),
            Effect::ClipCancel(a),
            Effect::ClipOffer(b),
            Effect::Present(b),
            Effect::ClipGone(a),
            Effect::Stop(a),
            Effect::Persist,
        ]
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "b@x")));
}

#[test]
fn the_neighbour_rule_skips_tabs_with_no_session_and_then_goes_left() {
    let mut tabs = three();
    let (b, c) = (tab_of(&tabs, "b@x"), tab_of(&tabs, "c@x"));
    tabs.click(c);
    tabs.stop(b);
    // Shown c; its right has nothing; the left neighbour b is stopped, so a.
    tabs.stop(c);
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "a@x")));
    // Now a is alone with c and b stopped: right side has none running.
    let effects = tabs.stop(tab_of(&tabs, "a@x"));
    assert_eq!(tabs.shown(), None);
    let a = SessionId::from_raw(0);
    assert_eq!(
        effects,
        vec![
            Effect::InputTo(None),
            Effect::ReleaseInput(a),
            Effect::ClipRevoke(a),
            Effect::ClipCancel(a),
            Effect::PresentNothing,
            Effect::ClipGone(a),
            Effect::Stop(a),
            Effect::Persist,
        ]
    );
}

#[test]
fn right_is_preferred_even_when_the_left_is_closer_to_a_gap() {
    let mut tabs = three();
    let b = tab_of(&tabs, "b@x");
    tabs.click(b);
    tabs.stop(b);
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "c@x")), "right of b first");
}

#[test]
fn stopping_a_background_tab_leaves_the_shown_one_alone() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    let effects = tabs.stop(tab_of(&tabs, "c@x"));
    assert_eq!(
        effects,
        vec![Effect::ClipGone(c), Effect::Stop(c), Effect::Persist]
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "a@x")));
}

#[test]
fn a_connect_is_reported_once_and_records_a_host_the_user_started() {
    let mut tabs = Tabs::new();
    let session = started(&tabs.open(&host("a@x")));
    let first = tabs.connected(session, at(0));
    assert_eq!(
        first,
        vec![
            Effect::RecordHost(host("a@x")),
            Effect::Persist,
            Effect::ClipOffer(session)
        ]
    );
    assert_eq!(tabs.connected(session, at(0)), Vec::new(), "idempotent");
    assert_eq!(status_of(&tabs, "a@x"), Status::Streaming);
}

#[test]
fn a_hidden_session_connects_without_a_clipboard_offer() {
    let mut tabs = three();
    let effects = tabs.open(&host("d@x"));
    let d = started(&effects);
    let b = tab_of(&tabs, "a@x");
    tabs.click(b);
    assert!(
        tabs.connected(d, at(0))
            .iter()
            .all(|e| !matches!(e, Effect::ClipOffer(_)))
    );
}

#[test]
fn a_restored_session_is_not_recorded_and_a_second_start_by_click_is() {
    let mut saved = TabSet::default();
    saved.open_machine(&host("a@x"));
    let (mut tabs, effects) = Tabs::start_up(&saved, &[]);
    let session = started(&effects);
    assert!(matches!(
        effects.iter().find(|e| matches!(e, Effect::Start { .. })),
        Some(Effect::Start {
            cause: Cause::Restore,
            ..
        })
    ));
    let on_connect = tabs.connected(session, at(0));
    assert!(
        !on_connect
            .iter()
            .any(|e| matches!(e, Effect::RecordHost(_)))
    );
    let tab = tab_of(&tabs, "a@x");
    tabs.stop(tab);
    let again = started(&tabs.click(tab));
    assert!(
        tabs.connected(again, at(0))
            .contains(&Effect::RecordHost(host("a@x")))
    );
}

#[test]
fn a_session_stopped_before_it_connects_records_nothing() {
    let mut tabs = Tabs::new();
    let session = started(&tabs.open(&host("a@x")));
    tabs.stop(tab_of(&tabs, "a@x"));
    assert_eq!(tabs.connected(session, at(0)), Vec::new());
    assert_eq!(
        tabs.tab_set().machines().len(),
        0,
        "never connected: not remembered"
    );
}

#[test]
fn an_ended_shown_tab_stays_shown_and_waits_to_reconnect() {
    let mut tabs = three();
    let a = session_of(&tabs, "a@x");
    let effects = tabs.ended(a, End::Error("connection lost".into()), at(0));
    assert_eq!(
        effects,
        vec![
            Effect::InputTo(None),
            Effect::PresentNothing,
            Effect::ClipGone(a),
            Effect::Stop(a)
        ]
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "a@x")));
    assert_eq!(
        status_of(&tabs, "a@x"),
        Status::Reconnecting {
            attempt: 1,
            of: 5,
            why: "connection lost".into(),
            at: Some(at(1_500)),
        },
        "a lost link is retried, not given up"
    );
}

#[test]
fn an_ended_background_tab_only_cleans_up() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    assert_eq!(
        tabs.ended(c, End::Closed, at(0)),
        vec![Effect::ClipGone(c), Effect::Stop(c)]
    );
    assert!(matches!(
        status_of(&tabs, "c@x"),
        Status::Reconnecting { attempt: 1, .. }
    ));
}

#[test]
fn an_incompatible_server_says_to_update() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Incompatible, at(0));
    let Status::GaveUp(text) = status_of(&tabs, "c@x") else {
        panic!("gave up");
    };
    assert!(text.contains("different protocol"), "{text}");
    assert!(text.contains(" x"), "names the machine: {text}");
}

#[test]
fn an_ssh_failure_says_what_went_wrong_and_names_the_machine() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Ssh(SshFailure::KeyRefused), at(0));
    let Status::GaveUp(text) = status_of(&tabs, "c@x") else {
        panic!("gave up");
    };
    assert_eq!(text, "x did not accept a key from this computer");
    let view = tabs.tab(tab_of(&tabs, "c@x")).expect("the tab");
    assert_eq!(view.failure(), Some(&SshFailure::KeyRefused));
}

#[test]
fn a_later_plain_error_does_not_keep_the_old_ssh_failure() {
    let mut tabs = three();
    let id = tab_of(&tabs, "c@x");
    let old = session_of(&tabs, "c@x");
    tabs.ended(old, End::Ssh(SshFailure::Refused), at(0));
    let effects = tabs.click(id);
    tabs.ended(started(&effects), End::Error("link lost".into()), at(0));
    let view = tabs.tab(id).expect("the tab");
    assert_eq!(view.failure(), None);
}

/// One shown tab `n@x` that is streaming and then ends with `failure`, with
/// no retries allowed, so it gives up at once.
fn gave_up_with(failure: SshFailure) -> (Tabs, TabId, SessionId) {
    let policy = crate::reconnect::Policy {
        first_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(20),
        max_retries: 0,
        stable_after: Duration::from_secs(1),
    };
    let mut tabs = Tabs::new().with_policy(policy);
    let session = started(&tabs.open(&host("n@x")));
    tabs.connected(session, at(0));
    tabs.ended(session, End::Ssh(failure), at(0));
    assert!(matches!(status_of(&tabs, "n@x"), Status::GaveUp(_)));
    let id = tab_of(&tabs, "n@x");
    (tabs, id, session)
}

#[test]
fn a_click_retries_a_tab_that_gave_up_on_the_network() {
    for failure in [
        SshFailure::Refused,
        SshFailure::Unreachable,
        SshFailure::NameNotFound,
    ] {
        let (mut tabs, id, old) = gave_up_with(failure.clone());
        let new = started(&tabs.click(id));
        assert_ne!(new, old, "{failure:?}");
        assert_eq!(status_of(&tabs, "n@x"), Status::Connecting, "{failure:?}");
    }
}

#[test]
fn a_click_on_a_tab_that_needs_the_person_only_selects_it() {
    let failures = [
        SshFailure::ClientMissing,
        SshFailure::HostUnknown,
        SshFailure::HostKeyChanged,
        SshFailure::KeyRefused,
        SshFailure::KeyFilePermissions,
        SshFailure::TooManyKeys,
        SshFailure::ServerMissing,
        SshFailure::ProtocolMismatch,
    ];
    for failure in failures {
        let mut tabs = three();
        let c = tab_of(&tabs, "c@x");
        let session = session_of(&tabs, "c@x");
        tabs.ended(session, End::Ssh(failure.clone()), at(0));
        let effects = tabs.click(c);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Start { .. })),
            "{failure:?} must not start ssh again: {effects:?}"
        );
        assert_eq!(tabs.shown(), Some(c), "{failure:?} selects the tab");
        assert!(effects.contains(&Effect::PresentNothing), "{failure:?}");
        assert!(matches!(status_of(&tabs, "c@x"), Status::GaveUp(_)));
        assert_eq!(
            tabs.tab(c).and_then(|t| t.failure().cloned()),
            Some(failure.clone()),
            "the explanation is kept"
        );
    }
}

#[test]
fn a_chord_onto_a_tab_that_needs_the_person_only_selects_it() {
    let mut tabs = three();
    let session = session_of(&tabs, "c@x");
    tabs.ended(session, End::Ssh(SshFailure::KeyRefused), at(0));
    let effects = tabs.switch(SwitchTo::Position(3));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Start { .. })));
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "c@x")));
}

#[test]
fn a_tab_that_failed_for_a_key_is_started_by_reconnect() {
    let mut tabs = three();
    let c = tab_of(&tabs, "c@x");
    let old = session_of(&tabs, "c@x");
    tabs.ended(old, End::Ssh(SshFailure::KeyRefused), at(0));
    let effects = tabs.reconnect(c);
    let new = started(&effects);
    assert_ne!(new, old);
    assert_eq!(status_of(&tabs, "c@x"), Status::Connecting);
    assert_eq!(tabs.shown(), Some(c), "the tab is shown");
    assert_eq!(tabs.tab(c).and_then(|t| t.failure().cloned()), None);
    assert!(effects.contains(&Effect::Persist));
}

#[test]
fn reconnect_on_a_waiting_tab_starts_now_and_resets_the_count() {
    let mut tabs = three();
    let c = tab_of(&tabs, "c@x");
    let s1 = session_of(&tabs, "c@x");
    tabs.ended(s1, End::Error("lost".into()), at(0));
    let s2 = retry_started(&tabs.due(at(1_500))).expect("a retry");
    tabs.ended(s2, End::Error("lost".into()), at(1_500));
    assert_eq!(tabs.tab(c).map(|t| t.retries()), Some(2));
    let now = started(&tabs.reconnect(c));
    assert_eq!(tabs.next_due(), None);
    tabs.ended(now, End::Error("lost".into()), at(2_000));
    assert_eq!(
        tabs.tab(c).map(|t| t.retries()),
        Some(1),
        "counted from one"
    );
}

#[test]
fn reconnect_on_a_running_or_unknown_tab_does_nothing() {
    let mut tabs = three();
    let b = tab_of(&tabs, "b@x");
    assert_eq!(tabs.reconnect(b), Vec::new());
    assert_eq!(tabs.reconnect(TabId(999)), Vec::new());
    assert_eq!(status_of(&tabs, "b@x"), Status::Streaming);
}

#[test]
fn retrying_the_shown_tab_that_gave_up_releases_nothing() {
    let mut tabs = three();
    let a = session_of(&tabs, "a@x");
    tabs.ended(a, End::Closed, at(0));
    let effects = tabs.click(tab_of(&tabs, "a@x"));
    let new = started(&effects);
    assert_eq!(
        effects[1..],
        [
            Effect::InputTo(Some(new)),
            Effect::Present(new),
            Effect::Persist
        ]
    );
}

#[test]
fn forgetting_a_tab_removes_it_and_a_running_one_is_stopped_first() {
    let mut tabs = three();
    let (a, b) = (session_of(&tabs, "a@x"), session_of(&tabs, "b@x"));
    let effects = tabs.forget(tab_of(&tabs, "a@x"));
    assert!(effects.contains(&Effect::Stop(a)));
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "b@x")));
    assert_eq!(tabs.shown_session(), Some(b));
    assert_eq!(tabs.forget(TabId(999)), Vec::new());
}

#[test]
fn forgetting_the_shown_tab_with_no_session_shows_the_nearest_running_tab() {
    let mut tabs = three();
    let a = session_of(&tabs, "a@x");
    tabs.ended(a, End::Closed, at(0));
    let effects = tabs.forget(tab_of(&tabs, "a@x"));
    let b = session_of(&tabs, "b@x");
    assert_eq!(effects[0], Effect::InputTo(Some(b)));
    assert_eq!(tabs.shown_session(), Some(b));
}

#[test]
fn forgetting_the_last_tab_leaves_nothing_shown() {
    let mut tabs = Tabs::new();
    let session = started(&tabs.open(&host("a@x")));
    tabs.connected(session, at(0));
    let effects = tabs.forget(tab_of(&tabs, "a@x"));
    assert!(tabs.is_empty());
    assert_eq!(tabs.shown(), None);
    assert!(effects.contains(&Effect::PresentNothing));
    assert!(tabs.tab_set().machines().is_empty());
}

#[test]
fn chords_move_next_previous_and_by_position() {
    let mut tabs = three();
    let name_of = |tabs: &Tabs| {
        tabs.tab(tabs.shown().expect("shown"))
            .map(|tab| tab.target().as_str().to_owned())
            .expect("tab")
    };
    tabs.switch(SwitchTo::Next);
    assert_eq!(name_of(&tabs), "b@x");
    tabs.switch(SwitchTo::Next);
    tabs.switch(SwitchTo::Next);
    assert_eq!(name_of(&tabs), "a@x", "wraps to the first");
    tabs.switch(SwitchTo::Previous);
    assert_eq!(name_of(&tabs), "c@x", "wraps to the last");
    tabs.switch(SwitchTo::Position(2));
    assert_eq!(name_of(&tabs), "b@x");
    assert_eq!(
        tabs.switch(SwitchTo::Position(4)),
        Vec::new(),
        "past the last"
    );
    assert_eq!(tabs.switch(SwitchTo::Position(0)), Vec::new());
    assert_eq!(name_of(&tabs), "b@x");
}

#[test]
fn a_chord_that_lands_on_a_stopped_tab_starts_it_like_a_click() {
    let mut tabs = three();
    tabs.stop(tab_of(&tabs, "b@x"));
    let effects = tabs.switch(SwitchTo::Next);
    assert!(effects.iter().any(|e| matches!(e, Effect::Start { .. })));
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "b@x")));
}

#[test]
fn chords_do_nothing_with_no_tabs() {
    let mut tabs = Tabs::new();
    for to in [SwitchTo::Next, SwitchTo::Previous, SwitchTo::Position(1)] {
        assert_eq!(tabs.switch(to), Vec::new());
    }
}

#[test]
fn next_with_nothing_shown_picks_the_first_and_previous_the_last() {
    let mut tabs = three();
    tabs.stop(tab_of(&tabs, "a@x"));
    tabs.stop(tab_of(&tabs, "b@x"));
    tabs.stop(tab_of(&tabs, "c@x"));
    assert_eq!(tabs.shown(), None);
    tabs.switch(SwitchTo::Previous);
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "c@x")));
}

#[test]
fn moving_a_tab_reorders_and_saves() {
    let mut tabs = three();
    let (a, c) = (tab_of(&tabs, "a@x"), tab_of(&tabs, "c@x"));
    let order = |tabs: &Tabs| -> Vec<String> {
        tabs.iter()
            .map(|t| t.target().as_str().to_owned())
            .collect()
    };
    assert_eq!(tabs.move_tab(c, Some(a)), vec![Effect::Persist]);
    assert_eq!(order(&tabs), ["c@x", "a@x", "b@x"]);
    tabs.move_tab(c, None);
    assert_eq!(order(&tabs), ["a@x", "b@x", "c@x"]);
    tabs.move_tab(a, Some(c));
    assert_eq!(order(&tabs), ["b@x", "a@x", "c@x"]);
    assert_eq!(tabs.move_tab(a, Some(a)), Vec::new());
    assert_eq!(tabs.move_tab(a, Some(TabId(77))), Vec::new());
    assert_eq!(tabs.move_tab(TabId(77), None), Vec::new());
    assert_eq!(tabs.shown(), Some(a), "moving keeps the selection");
}

#[test]
fn the_tab_set_holds_what_connected_and_what_was_not_stopped_by_hand() {
    let mut tabs = three();
    tabs.open(&host("never@x")); // connecting, never connected
    tabs.click(tab_of(&tabs, "b@x"));
    tabs.stop(tab_of(&tabs, "c@x"));
    let set = tabs.tab_set();
    let names = |list: &[HostTarget]| {
        list.iter()
            .map(|h| h.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(set.machines()), ["a@x", "b@x", "c@x"]);
    assert_eq!(names(set.open()), ["a@x", "b@x"]);
    assert_eq!(set.shown().map(HostTarget::as_str), Some("b@x"));
}

#[test]
fn a_tab_that_gave_up_stays_open_for_the_next_start() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Ssh(SshFailure::KeyRefused), at(0));
    assert!(matches!(status_of(&tabs, "c@x"), Status::GaveUp(_)));
    assert!(tabs.tab_set().open().iter().any(|h| h.same(&host("c@x"))));
}

#[test]
fn bare_start_reconnects_the_open_machines_and_leaves_the_rest_idle() {
    let mut saved = TabSet::default();
    for name in ["a@x", "b@x", "c@x", "d@x"] {
        saved.remember(host(name));
    }
    saved.open_machine(&host("b@x"));
    saved.open_machine(&host("c@x"));
    saved.set_shown(&host("c@x"));
    let (tabs, effects) = Tabs::start_up(&saved, &[]);
    assert_eq!(tabs.len(), 4);
    let starts: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Start { target, cause, .. } => Some((target.as_str().to_owned(), *cause)),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        [
            ("b@x".to_owned(), Cause::Restore),
            ("c@x".to_owned(), Cause::Restore)
        ]
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "c@x")));
    assert_eq!(status_of(&tabs, "a@x"), Status::Stopped);
    assert_eq!(status_of(&tabs, "d@x"), Status::Stopped);
    assert_eq!(
        tabs.tab_set(),
        saved,
        "restoring changes nothing in the file"
    );
}

#[test]
fn bare_start_with_nothing_shown_in_the_file_shows_the_first_open_machine() {
    let mut saved = TabSet::default();
    saved.remember(host("a@x"));
    saved.open_machine(&host("b@x"));
    saved.open_machine(&host("c@x"));
    let (tabs, _) = Tabs::start_up(&saved, &[]);
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "b@x")));
}

#[test]
fn bare_start_with_no_file_has_no_tabs_and_shows_nothing() {
    let (tabs, effects) = Tabs::start_up(&TabSet::default(), &[]);
    assert!(tabs.is_empty());
    assert_eq!(effects, Vec::new());
}

#[test]
fn command_line_targets_open_after_the_restored_ones_and_the_first_is_shown() {
    let mut saved = TabSet::default();
    saved.open_machine(&host("old@x"));
    saved.set_shown(&host("old@x"));
    let targets = [host("one@x"), host("two@x"), host("old@X")];
    let (tabs, effects) = Tabs::start_up(&saved, &targets);
    let order: Vec<_> = tabs
        .iter()
        .map(|t| t.target().as_str().to_owned())
        .collect();
    assert_eq!(
        order,
        ["old@x", "one@x", "two@x"],
        "a repeat is the same tab"
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "one@x")));
    let starts = effects
        .iter()
        .filter(|e| matches!(e, Effect::Start { .. }))
        .count();
    assert_eq!(starts, 3, "old, one and two each start once");
    for name in ["old@x", "one@x", "two@x"] {
        assert_eq!(status_of(&tabs, name), Status::Connecting);
    }
    let one = session_of(&tabs, "one@x");
    assert_eq!(
        effects.iter().rfind(|e| matches!(e, Effect::InputTo(_))),
        Some(&Effect::InputTo(Some(one)))
    );
}

#[test]
fn a_command_line_target_that_is_a_restored_tab_is_shown_without_a_second_start() {
    let mut saved = TabSet::default();
    saved.open_machine(&host("a@x"));
    saved.open_machine(&host("b@x"));
    saved.set_shown(&host("a@x"));
    let (tabs, effects) = Tabs::start_up(&saved, &[host("b@x")]);
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "b@x")));
    let starts = effects
        .iter()
        .filter(|e| matches!(e, Effect::Start { .. }))
        .count();
    assert_eq!(starts, 2);
}

#[test]
fn a_stopped_machine_named_on_the_command_line_starts_as_the_user() {
    let mut saved = TabSet::default();
    saved.remember(host("a@x"));
    let (tabs, effects) = Tabs::start_up(&saved, &[host("a@x")]);
    assert!(matches!(
        effects.iter().find(|e| matches!(e, Effect::Start { .. })),
        Some(Effect::Start {
            cause: Cause::User,
            ..
        })
    ));
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "a@x")));
}

#[test]
fn the_title_names_a_resync_only_for_a_live_shown_session() {
    use super::window_title;
    assert_eq!(window_title(Some(&Status::Streaming), false), "gliff");
    assert_eq!(
        window_title(Some(&Status::Streaming), true),
        "gliff - re-syncing the display"
    );
    assert_eq!(window_title(None, true), "gliff");
}

#[test]
fn an_ended_tab_says_why_even_if_it_was_re_syncing() {
    use super::window_title;
    let gave_up = Status::GaveUp("the server closed the connection".to_owned());
    assert_eq!(
        window_title(Some(&gave_up), true),
        "gliff - session ended (the server closed the connection)"
    );
}

// Reconnect. A fake clock drives every test: nothing here sleeps.

/// The retry a `due` call started, if any.
fn retry_started(effects: &[Effect]) -> Option<SessionId> {
    effects.iter().find_map(|effect| match effect {
        Effect::Start {
            session,
            cause: Cause::Retry,
            ..
        } => Some(*session),
        _ => None,
    })
}

fn reconnecting(attempt: u8, why: &str, at: Option<Instant>) -> Status {
    Status::Reconnecting {
        attempt,
        of: 5,
        why: why.to_owned(),
        at,
    }
}

#[test]
fn a_lost_link_waits_then_retries_with_a_growing_delay() {
    let mut tabs = three();
    let mut session = session_of(&tabs, "c@x");
    let mut now = 0;
    for (attempt, wait) in [(1, 1_500), (2, 3_000), (3, 6_000)] {
        tabs.ended(session, End::Error("lost".into()), at(now));
        assert_eq!(
            status_of(&tabs, "c@x"),
            reconnecting(attempt, "lost", Some(at(now + wait)))
        );
        assert_eq!(tabs.next_due(), Some(at(now + wait)));
        assert!(
            tabs.due(at(now + wait - 1)).is_empty(),
            "not before its time"
        );
        now += wait;
        session = retry_started(&tabs.due(at(now))).expect("a retry starts");
        assert_eq!(
            status_of(&tabs, "c@x"),
            reconnecting(attempt, "lost", None),
            "the attempt shows while it runs"
        );
        assert_eq!(tabs.next_due(), None, "one retry at a time");
        assert!(tabs.due(at(now)).is_empty(), "asking again starts nothing");
    }
}

#[test]
fn a_hidden_tab_retries_without_touching_the_route_or_the_picture() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Error("lost".into()), at(0));
    let effects = tabs.due(at(1_500));
    let tab = tab_of(&tabs, "c@x");
    assert_eq!(
        effects,
        vec![Effect::Start {
            tab,
            session: retry_started(&effects).expect("a retry"),
            target: host("c@x"),
            cause: Cause::Retry
        }]
    );
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "a@x")));
}

#[test]
fn the_shown_tab_takes_the_route_and_the_picture_back_when_it_retries() {
    let mut tabs = three();
    let a = session_of(&tabs, "a@x");
    tabs.ended(a, End::Error("lost".into()), at(0));
    let effects = tabs.due(at(1_500));
    let new = retry_started(&effects).expect("a retry");
    assert!(effects.contains(&Effect::InputTo(Some(new))));
    assert!(effects.contains(&Effect::Present(new)));
    assert_eq!(tabs.shown(), Some(tab_of(&tabs, "a@x")));
}

#[test]
fn the_earliest_waiting_tab_sets_the_wake_up_time() {
    let mut tabs = three();
    let (b, c) = (session_of(&tabs, "b@x"), session_of(&tabs, "c@x"));
    assert_eq!(tabs.next_due(), None);
    tabs.ended(c, End::Error("lost".into()), at(1_000));
    tabs.ended(b, End::Error("lost".into()), at(2_000));
    assert_eq!(tabs.next_due(), Some(at(2_500)));
    let started = tabs.due(at(3_500));
    assert_eq!(started.len(), 2, "both are due by then");
}

#[test]
fn a_failure_the_person_must_fix_is_not_retried() {
    for failure in [
        SshFailure::KeyRefused,
        SshFailure::HostKeyChanged,
        SshFailure::HostUnknown,
        SshFailure::ServerMissing,
        SshFailure::ClientMissing,
    ] {
        let mut tabs = three();
        let c = session_of(&tabs, "c@x");
        tabs.ended(c, End::Ssh(failure.clone()), at(0));
        assert!(
            matches!(status_of(&tabs, "c@x"), Status::GaveUp(_)),
            "{failure:?}"
        );
        assert_eq!(tabs.next_due(), None, "{failure:?}");
        assert!(tabs.due(at(3_600_000)).is_empty(), "{failure:?}");
    }
}

#[test]
fn a_machine_that_is_down_is_retried() {
    for failure in [SshFailure::Refused, SshFailure::Unreachable] {
        let mut tabs = three();
        let c = session_of(&tabs, "c@x");
        tabs.ended(c, End::Ssh(failure.clone()), at(0));
        assert!(
            matches!(status_of(&tabs, "c@x"), Status::Reconnecting { .. }),
            "{failure:?}"
        );
    }
}

#[test]
fn five_retries_that_all_fail_give_up_and_say_so() {
    let mut tabs = three();
    let mut session = session_of(&tabs, "c@x");
    let mut now = 0;
    for _ in 0..5 {
        tabs.ended(session, End::Ssh(SshFailure::Refused), at(now));
        now = tabs
            .next_due()
            .expect("waiting")
            .duration_since(at(0))
            .as_millis() as u64;
        session = retry_started(&tabs.due(at(now))).expect("a retry");
    }
    tabs.ended(session, End::Ssh(SshFailure::Refused), at(now));
    let Status::GaveUp(text) = status_of(&tabs, "c@x") else {
        panic!("gave up");
    };
    assert!(text.ends_with("; gave up after 5 retries"), "{text}");
    assert_eq!(tabs.next_due(), None);
    assert!(tabs.due(at(now + 3_600_000)).is_empty());
}

#[test]
fn a_connect_that_drops_at_once_does_not_loop_forever() {
    let mut tabs = three();
    let mut session = session_of(&tabs, "c@x");
    let mut now = 0;
    let mut starts = 0;
    while starts < 50 {
        tabs.connected(session, at(now));
        tabs.ended(session, End::Error("dropped".into()), at(now + 100));
        let Some(due) = tabs.next_due() else { break };
        now = due.duration_since(at(0)).as_millis() as u64;
        session = retry_started(&tabs.due(at(now))).expect("a retry");
        starts += 1;
    }
    assert_eq!(starts, 5, "five retries, then it stops");
    assert!(matches!(status_of(&tabs, "c@x"), Status::GaveUp(_)));
}

#[test]
fn a_session_that_stayed_up_starts_the_count_again() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Error("lost".into()), at(0));
    let c = retry_started(&tabs.due(at(1_500))).expect("a retry");
    tabs.connected(c, at(2_000));
    tabs.ended(c, End::Error("lost again".into()), at(2_000 + 30_000));
    assert_eq!(
        status_of(&tabs, "c@x"),
        reconnecting(1, "lost again", Some(at(2_000 + 30_000 + 1_500)))
    );
}

#[test]
fn a_click_on_a_waiting_tab_connects_now_and_starts_the_count_again() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Error("lost".into()), at(0));
    let id = tab_of(&tabs, "c@x");
    let effects = tabs.click(id);
    let now = started(&effects);
    assert_eq!(status_of(&tabs, "c@x"), Status::Connecting);
    assert_eq!(tabs.next_due(), None, "the waiting retry is gone");
    assert!(tabs.due(at(60_000)).is_empty());
    tabs.ended(now, End::Error("lost".into()), at(100));
    assert_eq!(
        status_of(&tabs, "c@x"),
        reconnecting(1, "lost", Some(at(1_600)))
    );
}

#[test]
fn stopping_a_waiting_tab_cancels_its_retry() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Error("lost".into()), at(0));
    tabs.stop(tab_of(&tabs, "c@x"));
    assert_eq!(status_of(&tabs, "c@x"), Status::Stopped);
    assert_eq!(tabs.next_due(), None);
    assert!(tabs.due(at(3_600_000)).is_empty());
}

#[test]
fn forgetting_a_waiting_tab_cancels_its_retry() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Error("lost".into()), at(0));
    tabs.forget(tab_of(&tabs, "c@x"));
    assert_eq!(tabs.next_due(), None);
    assert!(tabs.due(at(3_600_000)).is_empty());
}

#[test]
fn an_event_of_a_session_that_was_replaced_is_ignored() {
    let mut tabs = three();
    let old = session_of(&tabs, "c@x");
    tabs.ended(old, End::Error("lost".into()), at(0));
    let new = retry_started(&tabs.due(at(1_500))).expect("a retry");
    assert_eq!(
        tabs.ended(old, End::Error("late".into()), at(1_600)),
        Vec::new()
    );
    assert_eq!(tabs.connected(old, at(1_600)), Vec::new());
    assert_eq!(status_of(&tabs, "c@x"), reconnecting(1, "lost", None));
    tabs.connected(new, at(1_700));
    assert_eq!(status_of(&tabs, "c@x"), Status::Streaming);
}

#[test]
fn a_host_the_user_asked_for_is_recorded_when_its_retry_connects() {
    let mut tabs = Tabs::new();
    let first = started(&tabs.open(&host("me@host")));
    tabs.ended(first, End::Error("lost".into()), at(0));
    let retry = retry_started(&tabs.due(at(1_500))).expect("a retry");
    let effects = tabs.connected(retry, at(1_600));
    assert!(effects.contains(&Effect::RecordHost(host("me@host"))));
}

#[test]
fn a_host_restored_at_start_is_not_recorded_when_its_retry_connects() {
    let mut tabs = three();
    let c = session_of(&tabs, "c@x");
    tabs.ended(c, End::Error("lost".into()), at(0));
    let retry = retry_started(&tabs.due(at(1_500))).expect("a retry");
    let effects = tabs.connected(retry, at(1_600));
    assert!(!effects.iter().any(|e| matches!(e, Effect::RecordHost(_))));
}

#[test]
fn the_title_of_a_waiting_tab_says_which_attempt() {
    use super::window_title;
    assert_eq!(
        window_title(Some(&reconnecting(2, "connection lost", None)), false),
        "gliff - reconnecting, attempt 2 of 5 (connection lost)"
    );
}

#[test]
fn a_shorter_policy_changes_the_schedule() {
    let policy = crate::reconnect::Policy {
        first_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(20),
        max_retries: 2,
        stable_after: Duration::from_secs(1),
    };
    let mut tabs = Tabs::new().with_policy(policy);
    let first = started(&tabs.open(&host("a@x")));
    tabs.ended(first, End::Error("x".into()), at(0));
    assert_eq!(tabs.next_due(), Some(at(10)));
    let second = retry_started(&tabs.due(at(10))).expect("a retry");
    tabs.ended(second, End::Error("x".into()), at(10));
    assert_eq!(tabs.next_due(), Some(at(30)));
    let third = retry_started(&tabs.due(at(30))).expect("a retry");
    tabs.ended(third, End::Error("x".into()), at(30));
    assert!(matches!(status_of(&tabs, "a@x"), Status::GaveUp(_)));
}
