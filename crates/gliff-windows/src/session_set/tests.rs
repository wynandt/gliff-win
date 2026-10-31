//! The runner with fake sessions and a fake clipboard.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use gliff_proto::msg::SessionInfo;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use super::ClipSide;
use super::Link;
use super::Presented;
use super::SessionSet;
use super::end_of;
use crate::clip_link::Port;
use crate::config::TabSet;
use crate::history::HostTarget;
use crate::route::InputRoute;
use crate::session::EndReason;
use crate::session::SessionStats;
use crate::session::UiCmd;
use crate::session::UiEvent;
use crate::session::driver::SessionEnd;
use crate::tabs::End;
use crate::tabs::SessionId;
use crate::tabs::Status;
use crate::tabs::Tabs;

/// The fake clock's start.
fn base() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn host(text: &str) -> HostTarget {
    text.parse().expect("a host")
}

/// What the fakes saw and what the test makes them do.
#[derive(Default)]
struct World {
    commands: RefCell<BTreeMap<u32, UnboundedReceiver<UiCmd>>>,
    events: RefCell<BTreeMap<u32, mpsc::Sender<UiEvent>>>,
    finished: RefCell<BTreeSet<u32>>,
    refuse: RefCell<BTreeSet<String>>,
    clip: RefCell<Vec<String>>,
    started: RefCell<Vec<String>>,
}

impl World {
    /// The commands a session got so far, as short names.
    fn commands_of(&self, session: u32) -> Vec<String> {
        let mut out = Vec::new();
        let mut map = self.commands.borrow_mut();
        let rx = map.get_mut(&session).expect("a session");
        while let Ok(cmd) = rx.try_recv() {
            out.push(match cmd {
                UiCmd::ReleaseInput => "release".to_owned(),
                UiCmd::Bye => "bye".to_owned(),
                UiCmd::Tick(n) => format!("tick{n}"),
                other => format!("{other:?}"),
            });
        }
        out
    }

    fn event(&self, session: u32, event: UiEvent) {
        self.events.borrow()[&session].send(event).expect("send");
    }

    fn finish(&self, session: u32) {
        self.finished.borrow_mut().insert(session);
    }

    fn clip(&self) -> Vec<String> {
        std::mem::take(&mut *self.clip.borrow_mut())
    }
}

struct FakeLink {
    id: u32,
    world: Rc<World>,
    commands: UnboundedSender<UiCmd>,
    events: mpsc::Receiver<UiEvent>,
}

impl Link for FakeLink {
    fn commands(&self) -> UnboundedSender<UiCmd> {
        self.commands.clone()
    }

    fn try_event(&self) -> Option<UiEvent> {
        self.events.try_recv().ok()
    }

    fn stats(&self) -> SessionStats {
        SessionStats {
            frames: u64::from(self.id) * 10,
            ..SessionStats::default()
        }
    }

    fn is_finished(&self) -> bool {
        self.world.finished.borrow().contains(&self.id)
    }

    fn finish(self) -> SessionEnd {
        SessionEnd {
            reason: None,
            ssh_exit: Some(0),
            errors: Vec::new(),
            stats: self.stats(),
            slot_dropped: 0,
            stderr_log: None,
        }
    }
}

struct FakeClip(Rc<World>);

impl FakeClip {
    fn log(&self, line: String) {
        self.0.clip.borrow_mut().push(line);
    }
}

impl ClipSide for FakeClip {
    fn add_session(&mut self, session: SessionId, share: bool) -> Option<Port> {
        self.log(if share {
            format!("add {}", session.get())
        } else {
            format!("add {} without a clipboard", session.get())
        });
        None
    }

    fn show(&mut self, session: Option<SessionId>) {
        self.log(format!("show {}", session.map_or(0, SessionId::get)));
    }

    fn revoke(&mut self, session: SessionId) {
        self.log(format!("revoke {}", session.get()));
    }

    fn cancel(&mut self, session: SessionId) {
        self.log(format!("cancel {}", session.get()));
    }

    fn offer(&mut self) {
        self.log("offer".to_owned());
    }

    fn gone(&mut self, session: SessionId) {
        self.log(format!("gone {}", session.get()));
    }

    fn set_sharing(&mut self, session: SessionId, share: bool) {
        self.log(format!("sharing {} {share}", session.get()));
    }
}

struct Fixture {
    world: Rc<World>,
    route: Arc<InputRoute<UiCmd>>,
    set: SessionSet<FakeLink>,
    tabs: Tabs,
    clock: Rc<Cell<Instant>>,
}

impl Fixture {
    /// Moves the fake clock on.
    fn advance(&self, ms: u64) {
        self.clock.set(self.clock.get() + Duration::from_millis(ms));
    }
}

fn fixture(targets: &[&str], saved: &TabSet) -> (Fixture, super::Applied) {
    fixture_sharing(targets, saved, Box::new(|_| true))
}

fn fixture_sharing(
    targets: &[&str],
    saved: &TabSet,
    shares: Box<dyn Fn(&HostTarget) -> bool>,
) -> (Fixture, super::Applied) {
    let world = Rc::new(World::default());
    let route = Arc::new(InputRoute::new());
    let starter: super::Starter<FakeLink> = {
        let world = Rc::clone(&world);
        Box::new(move |launch| {
            world.started.borrow_mut().push(launch.target.to_string());
            if world.refuse.borrow().contains(launch.target.as_str()) {
                return Err(io::Error::other("cannot start ssh"));
            }
            let id = launch.session.get();
            let (commands, rx) = tokio::sync::mpsc::unbounded_channel();
            let (tx, events) = mpsc::channel();
            world.commands.borrow_mut().insert(id, rx);
            world.events.borrow_mut().insert(id, tx);
            Ok(FakeLink {
                id,
                world: Rc::clone(&world),
                commands,
                events,
            })
        })
    };
    let clip: Box<dyn ClipSide> = Box::new(FakeClip(Rc::clone(&world)));
    let clock = Rc::new(Cell::new(base()));
    let mut set = SessionSet::new(Arc::clone(&route), Some(clip), starter)
        .with_share_policy(shares)
        .with_clock({
            let clock = Rc::clone(&clock);
            Box::new(move || clock.get())
        });
    let targets: Vec<HostTarget> = targets.iter().map(|t| host(t)).collect();
    let (mut tabs, effects) = Tabs::start_up(saved, &targets);
    let applied = set.run(&mut tabs, effects);
    (
        Fixture {
            world,
            route,
            set,
            tabs,
            clock,
        },
        applied,
    )
}

fn connected() -> UiEvent {
    UiEvent::Connected(SessionInfo {
        headless: true,
        output: "out".to_owned(),
    })
}

fn session_of(tabs: &Tabs, name: &str) -> SessionId {
    tabs.iter()
        .find(|tab| tab.target().same(&host(name)))
        .and_then(|tab| tab.session())
        .expect("a live session")
}

fn status_of(tabs: &Tabs, name: &str) -> Status {
    tabs.iter()
        .find(|tab| tab.target().same(&host(name)))
        .expect("the tab")
        .status()
        .clone()
}

#[test]
fn two_targets_start_two_sessions_and_the_first_is_shown_and_gets_the_keys() {
    let (fx, applied) = fixture(&["a@x", "b@x"], &TabSet::default());
    assert_eq!(*fx.world.started.borrow(), ["a@x", "b@x"]);
    let a = session_of(&fx.tabs, "a@x");
    assert_eq!(fx.route.target(), Some(a));
    assert_eq!(applied.presented, Some(Presented::Session(a)));
    assert!(applied.persist);
    assert_eq!(fx.set.sessions().len(), 2);
}

#[test]
fn a_switch_moves_the_route_then_releases_the_old_session_and_never_the_new() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let (a, b) = (session_of(&fx.tabs, "a@x"), session_of(&fx.tabs, "b@x"));
    assert!(fx.route.send(UiCmd::Tick(1)));
    let next = fx.tabs.switch(crate::tabs::SwitchTo::Next);
    let applied = fx.set.run(&mut fx.tabs, next);
    assert_eq!(fx.route.target(), Some(b));
    assert!(fx.route.send(UiCmd::Tick(2)));
    assert_eq!(fx.world.commands_of(a.get()), ["tick1", "release"]);
    assert_eq!(fx.world.commands_of(b.get()), ["tick2"]);
    assert_eq!(applied.presented, Some(Presented::Session(b)));
}

#[test]
fn a_switch_revokes_the_old_offer_cancels_its_transfers_and_shows_the_new_session() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let (a, b) = (session_of(&fx.tabs, "a@x"), session_of(&fx.tabs, "b@x"));
    for session in [a, b] {
        fx.world.event(session.get(), connected());
    }
    fx.set.poll(&mut fx.tabs);
    fx.world.clip();
    let next = fx.tabs.switch(crate::tabs::SwitchTo::Next);
    fx.set.run(&mut fx.tabs, next);
    assert_eq!(
        fx.world.clip(),
        [
            format!("revoke {}", a.get()),
            format!("cancel {}", a.get()),
            format!("show {}", b.get()),
            "offer".to_owned(),
            format!("show {}", b.get()),
        ]
    );
}

#[test]
fn a_connect_of_a_session_the_user_asked_for_is_recorded_once() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let a = session_of(&fx.tabs, "a@x");
    fx.world.event(a.get(), connected());
    fx.world.event(a.get(), connected());
    let applied = fx.set.poll(&mut fx.tabs);
    assert_eq!(applied.recorded, [host("a@x")]);
    assert_eq!(status_of(&fx.tabs, "a@x"), Status::Streaming);
}

#[test]
fn a_session_that_will_not_start_leaves_its_tab_with_the_reason_and_no_route() {
    let world_refuses = |fx: &Fixture| fx.world.refuse.borrow_mut().insert("bad@x".to_owned());
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    world_refuses(&fx);
    let effects = fx.tabs.open(&host("bad@x"));
    let applied = fx.set.run(&mut fx.tabs, effects);
    assert_eq!(
        status_of(&fx.tabs, "bad@x"),
        Status::Reconnecting {
            attempt: 1,
            of: 5,
            why: "cannot start ssh".to_owned(),
            at: Some(base() + Duration::from_millis(1_500)),
        }
    );
    assert_eq!(fx.route.target(), None);
    assert_eq!(applied.presented, Some(Presented::Nothing));
    assert_eq!(
        fx.set.sessions().len(),
        1,
        "the other session keeps running"
    );
    assert!(fx.world.clip().iter().any(|line| line.starts_with("gone ")));
}

#[test]
fn an_ended_session_gives_its_tab_up_and_is_joined_once_its_thread_is_done() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let a = session_of(&fx.tabs, "a@x");
    fx.world.event(
        a.get(),
        UiEvent::Ended(EndReason::ServerError {
            code: 300,
            message: "no display".to_owned(),
        }),
    );
    let applied = fx.set.poll(&mut fx.tabs);
    assert_eq!(
        status_of(&fx.tabs, "a@x"),
        Status::Reconnecting {
            attempt: 1,
            of: 5,
            why: "no display".to_owned(),
            at: Some(base() + Duration::from_millis(1_500)),
        }
    );
    assert_eq!(fx.route.target(), None);
    assert!(fx.set.sessions().is_empty());
    assert!(applied.ended.is_empty(), "the thread has not finished yet");
    fx.world.finish(a.get());
    let applied = fx.set.poll(&mut fx.tabs);
    assert_eq!(applied.ended.len(), 1);
    assert_eq!(applied.ended[0].0, a);
    assert!(fx.world.commands_of(a.get()).contains(&"bye".to_owned()));
}

#[test]
fn a_thread_that_ends_without_saying_so_still_ends_its_tab() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let a = session_of(&fx.tabs, "a@x");
    fx.world.finish(a.get());
    fx.set.poll(&mut fx.tabs);
    assert_eq!(
        status_of(&fx.tabs, "a@x"),
        Status::Reconnecting {
            attempt: 1,
            of: 5,
            why: "the session stopped unexpectedly".to_owned(),
            at: Some(base() + Duration::from_millis(1_500)),
        }
    );
}

#[test]
fn events_the_model_does_not_own_are_passed_on_with_their_session() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let b = session_of(&fx.tabs, "b@x");
    fx.world.event(b.get(), UiEvent::GrabReleased);
    let applied = fx.set.poll(&mut fx.tabs);
    assert_eq!(applied.events, [(b, UiEvent::GrabReleased)]);
}

#[test]
fn a_connect_reaches_the_model_and_the_view() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let a = session_of(&fx.tabs, "a@x");
    fx.world.event(a.get(), connected());
    let applied = fx.set.poll(&mut fx.tabs);
    assert_eq!(applied.events, [(a, connected())]);
    assert_eq!(status_of(&fx.tabs, "a@x"), Status::Streaming);
}

#[test]
fn stopping_twice_is_harmless_and_sends_one_bye() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let a = session_of(&fx.tabs, "a@x");
    fx.set.apply(vec![crate::tabs::Effect::Stop(a)]);
    fx.set.apply(vec![crate::tabs::Effect::Stop(a)]);
    assert_eq!(fx.world.commands_of(a.get()), ["bye"]);
    assert_eq!(fx.route.target(), None, "a stopped session gets no keys");
}

#[test]
fn shutdown_says_bye_to_every_session_and_joins_them() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let (a, b) = (session_of(&fx.tabs, "a@x"), session_of(&fx.tabs, "b@x"));
    let ended = fx.set.shutdown();
    assert_eq!(ended.len(), 2);
    for session in [a, b] {
        assert!(
            fx.world
                .commands_of(session.get())
                .contains(&"bye".to_owned())
        );
    }
    assert_eq!(fx.route.target(), None);
}

#[test]
fn close_all_says_bye_at_once_and_running_falls_as_threads_finish() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let (a, b) = (session_of(&fx.tabs, "a@x"), session_of(&fx.tabs, "b@x"));
    assert_eq!(fx.set.running(), 2);
    fx.set.close_all();
    assert_eq!(fx.route.target(), None);
    assert_eq!(fx.world.commands_of(a.get()), ["bye"]);
    assert_eq!(fx.world.commands_of(b.get()), ["bye"]);
    assert_eq!(fx.set.running(), 2, "nothing is joined before it finishes");
    fx.world.finish(a.get());
    let applied = fx.set.poll(&mut fx.tabs);
    assert_eq!(applied.ended.len(), 1);
    assert_eq!(fx.set.running(), 1);
    fx.world.finish(b.get());
    fx.set.poll(&mut fx.tabs);
    assert_eq!(fx.set.running(), 0);
}

#[test]
fn close_all_leaves_the_tabs_wanted_open_for_the_next_start() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let before = fx.tabs.tab_set();
    fx.set.close_all();
    assert_eq!(fx.tabs.tab_set(), before);
}

#[test]
fn reports_list_every_live_session_with_its_target_and_counters() {
    let (fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let reports = fx.set.reports();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].target, host("a@x"));
    assert_eq!(
        reports[1].stats.frames,
        u64::from(reports[1].session.get()) * 10
    );
}

#[test]
fn the_end_a_session_reports_maps_to_the_models_words() {
    assert_eq!(end_of(&EndReason::Bye), End::Closed);
    assert_eq!(
        end_of(&EndReason::Incompatible("v".into())),
        End::Incompatible
    );
    assert_eq!(
        end_of(&EndReason::Link("broken".into())),
        End::Error("broken".into())
    );
}

#[test]
fn a_bare_start_reconnects_what_was_open_and_shows_what_was_shown() {
    let mut saved = TabSet::default();
    for name in ["a@x", "b@x", "c@x"] {
        saved.remember(host(name));
    }
    saved.open_machine(&host("a@x"));
    saved.open_machine(&host("c@x"));
    saved.set_shown(&host("c@x"));
    let (fx, applied) = fixture(&[], &saved);
    assert_eq!(*fx.world.started.borrow(), ["a@x", "c@x"]);
    assert_eq!(fx.route.target(), Some(session_of(&fx.tabs, "c@x")));
    assert!(matches!(applied.presented, Some(Presented::Session(_))));
}

/// The session ends with a lost link (the kind a retry can fix).
fn lose(fx: &Fixture, session: SessionId) {
    fx.world.event(
        session.get(),
        UiEvent::Ended(EndReason::Link("lost".to_owned())),
    );
}

#[test]
fn a_lost_session_is_started_again_when_the_clock_reaches_its_time() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let (a, b) = (session_of(&fx.tabs, "a@x"), session_of(&fx.tabs, "b@x"));
    lose(&fx, b);
    fx.set.poll(&mut fx.tabs);
    assert_eq!(fx.world.started.borrow().len(), 2);
    fx.advance(1_499);
    fx.set.poll(&mut fx.tabs);
    assert_eq!(fx.world.started.borrow().len(), 2, "not yet");
    fx.advance(1);
    fx.set.poll(&mut fx.tabs);
    assert_eq!(*fx.world.started.borrow(), ["a@x", "b@x", "b@x"]);
    let again = session_of(&fx.tabs, "b@x");
    assert!(again.get() > b.get());
    assert_eq!(fx.route.target(), Some(a), "a hidden tab leaves the route");
    fx.world.event(again.get(), connected());
    fx.set.poll(&mut fx.tabs);
    assert_eq!(status_of(&fx.tabs, "b@x"), Status::Streaming);
}

#[test]
fn the_shown_tab_gets_the_route_back_with_its_new_session() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    let a = session_of(&fx.tabs, "a@x");
    lose(&fx, a);
    fx.set.poll(&mut fx.tabs);
    assert_eq!(fx.route.target(), None, "nothing to send keys to meanwhile");
    fx.advance(1_500);
    let applied = fx.set.poll(&mut fx.tabs);
    let again = session_of(&fx.tabs, "a@x");
    assert_eq!(fx.route.target(), Some(again));
    assert_eq!(applied.presented, Some(Presented::Session(again)));
}

#[test]
fn a_start_that_fails_again_and_again_waits_each_time_and_then_stops() {
    let (mut fx, _) = fixture(&["a@x"], &TabSet::default());
    fx.world.refuse.borrow_mut().insert("a@x".to_owned());
    let a = session_of(&fx.tabs, "a@x");
    lose(&fx, a);
    fx.set.poll(&mut fx.tabs);
    // Poll every second for a long while; the starts are bounded.
    for _ in 0..600 {
        fx.advance(1_000);
        fx.set.poll(&mut fx.tabs);
    }
    assert_eq!(fx.world.started.borrow().len(), 1 + 5, "five retries only");
    assert!(matches!(status_of(&fx.tabs, "a@x"), Status::GaveUp(_)));
}

#[test]
fn polling_often_does_not_start_a_retry_early_or_twice() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let b = session_of(&fx.tabs, "b@x");
    lose(&fx, b);
    for _ in 0..100 {
        fx.set.poll(&mut fx.tabs);
    }
    assert_eq!(fx.world.started.borrow().len(), 2);
    fx.advance(1_500);
    for _ in 0..100 {
        fx.set.poll(&mut fx.tabs);
    }
    assert_eq!(fx.world.started.borrow().len(), 3);
}

#[test]
fn a_closing_window_starts_no_retry() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let b = session_of(&fx.tabs, "b@x");
    lose(&fx, b);
    fx.set.poll(&mut fx.tabs);
    fx.set.close_all();
    fx.advance(60_000);
    fx.set.poll(&mut fx.tabs);
    assert_eq!(fx.world.started.borrow().len(), 2);
}

#[test]
fn closing_a_tab_while_it_waits_starts_nothing() {
    let (mut fx, _) = fixture(&["a@x", "b@x"], &TabSet::default());
    let b = session_of(&fx.tabs, "b@x");
    lose(&fx, b);
    fx.set.poll(&mut fx.tabs);
    let id = fx
        .tabs
        .iter()
        .find(|tab| tab.target().same(&host("b@x")))
        .expect("the tab")
        .id();
    let effects = fx.tabs.stop(id);
    fx.set.run(&mut fx.tabs, effects);
    fx.advance(60_000);
    fx.set.poll(&mut fx.tabs);
    assert_eq!(fx.world.started.borrow().len(), 2);
    assert_eq!(status_of(&fx.tabs, "b@x"), Status::Stopped);
}

#[test]
fn a_machine_that_does_not_share_is_registered_without_a_clipboard() {
    let (fx, _) = fixture_sharing(
        &["a@x", "b@x"],
        &TabSet::default(),
        Box::new(|target| target.as_str() != "b@x"),
    );
    let (a, b) = (session_of(&fx.tabs, "a@x"), session_of(&fx.tabs, "b@x"));
    let log = fx.world.clip();
    assert!(log.contains(&format!("add {}", a.get())), "{log:?}");
    assert!(
        log.contains(&format!("add {} without a clipboard", b.get())),
        "{log:?}"
    );
}

#[test]
fn the_share_choice_is_asked_again_at_each_connect() {
    let shared = Rc::new(Cell::new(true));
    let (mut fx, _) = fixture_sharing(&["a@x"], &TabSet::default(), {
        let shared = Rc::clone(&shared);
        Box::new(move |_| shared.get())
    });
    let a = session_of(&fx.tabs, "a@x");
    assert_eq!(
        fx.world.clip(),
        [format!("add {}", a.get()), format!("show {}", a.get())]
    );
    shared.set(false);
    lose(&fx, a);
    fx.set.poll(&mut fx.tabs);
    fx.advance(1_500);
    fx.set.poll(&mut fx.tabs);
    let again = session_of(&fx.tabs, "a@x");
    let log = fx.world.clip();
    assert!(
        log.contains(&format!("add {} without a clipboard", again.get())),
        "{log:?}"
    );
}
