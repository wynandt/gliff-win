//! The tabs: which machines the client knows, which have a session, which is
//! shown, and what to do when that changes.
//!
//! Pure of Win32 and of sessions. `Tabs` is a state machine: the UI thread
//! calls one method per event (a click, a session that connected or ended)
//! and runs the returned [`Effect`]s in order. The model never starts a
//! session or touches a route; it says what to do, so every rule here is a
//! test of a list.
//!
//! Two ids are kept apart on purpose. A [`TabId`] is a place in the strip and
//! lives as long as the tab. A [`SessionId`] names one connection attempt and
//! is never reused, so an event from a session that was stopped (a late
//! `ended`, a late `connected`) finds no tab and does nothing.
//!
//! The order of the effects of a switch is part of the contract. The input
//! route moves to the new session *first*, so no key reaches the old session
//! after that; the keys the old session already holds are released next. See
//! `route.rs` for why that order is what makes the release complete.
//!
//! A session that ends is retried by the tab itself, by the rules of
//! [`crate::reconnect`]: `ended` says when the next try is due (the tab is
//! `Reconnecting`), and `due` starts the ones whose time has come. The model
//! has no clock and no timer: callers pass `now`, and the window arms a
//! timer from `next_due`.

use std::time::Instant;

use crate::config::TabSet;
use crate::connect;
use crate::connect::SshFailure;
use crate::history::HostTarget;
use crate::reconnect;
use crate::reconnect::Backoff;
use crate::reconnect::Next;
use crate::reconnect::Policy;

/// A tab; stable while the tab exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u32);

impl TabId {
    /// The number, for a window message or a stats line.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// One connection attempt of a tab; never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u32);

impl SessionId {
    /// The number, for a window message (`wparam`) or a file name.
    pub fn get(self) -> u32 {
        self.0
    }

    /// The id a number stands for. A number that names no session is
    /// harmless: no tab has it, so an event carrying it is dropped.
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// What a tab is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No session, and none wanted: a remembered machine, or one stopped by
    /// hand.
    Stopped,
    /// A session was started and has not reported `connected`.
    Connecting,
    /// The session is up.
    Streaming,
    /// The session ended for a reason that can pass by itself, and the tab
    /// is trying again: retry number `attempt` of `of`. The text says why
    /// the last session ended.
    Reconnecting {
        attempt: u8,
        of: u8,
        why: String,
        /// When the retry starts; `None` once it has started and is
        /// connecting.
        at: Option<Instant>,
    },
    /// The session ended on its own and the tab stopped trying (the reason
    /// cannot pass by itself, or the retries ran out); the text says why.
    /// Clicking the tab starts again.
    GaveUp(String),
}

/// The window title for the shown tab. A tab that gave up says why, and
/// that wins over a re-sync (a session that is over is not re-syncing); so
/// does one that is reconnecting; a shown session that is re-syncing its
/// display says so; anything else, including no tab at all, is the plain
/// name.
pub fn window_title(shown: Option<&Status>, resyncing: bool) -> String {
    match shown {
        Some(Status::Reconnecting {
            attempt, of, why, ..
        }) => format!("gliff - reconnecting, attempt {attempt} of {of} ({why})"),
        Some(Status::GaveUp(why)) => format!("gliff - session ended ({why})"),
        Some(_) if resyncing => "gliff - re-syncing the display".to_owned(),
        _ => "gliff".to_owned(),
    }
}

/// Why a session started. Only a session the user asked for puts its host
/// in the recent hosts: a machine reconnected at start is not "recent".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A click, a chord, or a target on the command line.
    User,
    /// Bare start, reconnecting what was open.
    Restore,
    /// The tab's own retry after a session ended. It is not a new request
    /// of the user, so it records a host only if the attempt it replaces
    /// would have.
    Retry,
}

/// How a session ended, as the session reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The session was closed in order (an orderly `Bye`). The shipped session
    /// reports it only for the client's own `Bye`, which has already unbound
    /// the tab, so it is dropped; if a server `Bye` is ever mapped here it is
    /// retried like a link that dropped.
    Closed,
    /// The link failed or the server refused; the text is for the user.
    Error(String),
    /// The server speaks another protocol version. Starting again cannot
    /// help, so the text says to update.
    Incompatible,
    /// ssh failed, and its own words said why ([`crate::connect::classify`]).
    Ssh(SshFailure),
}

/// Records `failure` on the tab and returns the status text for it: the
/// message's first sentence, which names the machine. The whole message goes
/// to the log, and the failure panel shows it with its button.
fn explain(tab: &mut Tab, failure: SshFailure) -> String {
    let what = connect::message(&failure, &tab.target).what;
    tab.failure = Some(failure);
    what.trim_end_matches('.').to_owned()
}

/// Where a chord or a command moves the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchTo {
    /// The tab to the right, the first one after the last.
    Next,
    /// The tab to the left, the last one before the first.
    Previous,
    /// The tab at a place, counted from 1; a place past the last tab does
    /// nothing.
    Position(u8),
}

/// One thing for the UI thread to do, in the order returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Point the keyboard and pointer route at this session, or at none.
    /// Always before any `ReleaseInput` of the same call.
    InputTo(Option<SessionId>),
    /// Tell the session to release every key and button it holds.
    ReleaseInput(SessionId),
    /// Withdraw the local clipboard offer from this session's engine.
    ClipRevoke(SessionId),
    /// Cancel this session's file transfers and pending pastes.
    ClipCancel(SessionId),
    /// Offer the local clipboard to this session's engine.
    ClipOffer(SessionId),
    /// Show this session's view: its last picture (or its status), sent the
    /// window's size and zoom.
    Present(SessionId),
    /// Show no session: an empty picture area and a status.
    PresentNothing,
    /// Start a session for a tab. Comes before the `InputTo` of the same
    /// call, because the route needs the new session's sender.
    Start {
        tab: TabId,
        session: SessionId,
        target: HostTarget,
        cause: Cause,
    },
    /// Put this host first in the recent hosts. Once per user-started
    /// session, at its first `connected`.
    RecordHost(HostTarget),
    /// The session is leaving: drop its clipboard offer from Windows, fail a
    /// paste waiting on it.
    ClipGone(SessionId),
    /// End the session and release its handle (idempotent).
    Stop(SessionId),
    /// Save the tab set (`machines`, `open`, `shown`) from [`Tabs::tab_set`].
    Persist,
}

#[derive(Clone, Debug)]
struct Tab {
    id: TabId,
    target: HostTarget,
    session: Option<SessionId>,
    status: Status,
    /// Retries so far and uptime ([`crate::reconnect`]).
    backoff: Backoff,
    /// Connected at least once, or came from the file: the machine is
    /// remembered.
    known: bool,
    /// Wanted at the next start: not stopped by hand.
    open: bool,
    /// The user started this session and it has not connected yet.
    record_pending: bool,
    /// Why the last session ended, when ssh said ([`End::Ssh`], or a version
    /// mismatch). Meaningful only while the status is `GaveUp`.
    failure: Option<SshFailure>,
}

impl Tab {
    fn is_running(&self) -> bool {
        self.session.is_some()
    }

    /// The tab gave up for a reason that only the person can fix, so
    /// starting it again changes nothing ([`reconnect::retryable`]).
    fn needs_the_person(&self) -> bool {
        match (&self.status, &self.failure) {
            (Status::GaveUp(_), Some(failure)) => !reconnect::retryable(&End::Ssh(failure.clone())),
            _ => false,
        }
    }
}

/// A read-only view of one tab.
#[derive(Clone, Copy, Debug)]
pub struct TabView<'a>(&'a Tab);

impl<'a> TabView<'a> {
    pub fn id(&self) -> TabId {
        self.0.id
    }
    pub fn target(&self) -> &'a HostTarget {
        &self.0.target
    }
    /// The live session, if the tab has one.
    pub fn session(&self) -> Option<SessionId> {
        self.0.session
    }
    pub fn status(&self) -> &'a Status {
        &self.0.status
    }
    pub fn retries(&self) -> u8 {
        self.0.backoff.retries()
    }
    /// The class of the failure behind a `GaveUp` tab, for the panel's
    /// message and button; `None` for any other state or an end that was
    /// not an ssh failure.
    pub fn failure(&self) -> Option<&'a SshFailure> {
        match self.0.status {
            Status::GaveUp(_) => self.0.failure.as_ref(),
            _ => None,
        }
    }
}

/// The tabs, their order, and the shown one.
#[derive(Clone, Debug, Default)]
pub struct Tabs {
    tabs: Vec<Tab>,
    shown: Option<TabId>,
    next_tab: u32,
    next_session: u32,
    policy: Policy,
}

impl Tabs {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same set with other retry numbers (tests use short ones).
    #[must_use]
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Bare start: a tab for every remembered machine; the machines that
    /// were open start (cause `Restore`, in tab order), the one that was
    /// shown is shown, else the first open one. Then each command-line
    /// target opens as the user would open it; the first of them is shown.
    pub fn start_up(saved: &TabSet, targets: &[HostTarget]) -> (Self, Vec<Effect>) {
        let mut tabs = Self::new();
        let mut effects = Vec::new();
        for machine in saved.machines() {
            tabs.push_tab(machine.clone(), true);
        }
        let mut first_open = None;
        let mut shown = None;
        for index in 0..tabs.tabs.len() {
            let target = tabs.tabs[index].target.clone();
            if !saved.open().iter().any(|open| open.same(&target)) {
                continue;
            }
            let id = tabs.tabs[index].id;
            first_open.get_or_insert(id);
            if saved.shown().is_some_and(|name| name.same(&target)) {
                shown = Some(id);
            }
            effects.push(tabs.begin(index, Cause::Restore));
        }
        match targets.split_first() {
            Some((first, rest)) => {
                tabs.open_one(first, &mut effects);
                for target in rest {
                    tabs.open_in_background(target, &mut effects);
                }
            }
            None => {
                if let Some(id) = shown.or(first_open) {
                    tabs.show(None, id, &mut effects);
                    effects.push(Effect::Persist);
                }
            }
        }
        (tabs, effects)
    }

    /// The tabs in order.
    pub fn iter(&self) -> impl Iterator<Item = TabView<'_>> {
        self.tabs.iter().map(TabView)
    }

    /// One tab.
    pub fn tab(&self, id: TabId) -> Option<TabView<'_>> {
        self.find(id).map(|index| TabView(&self.tabs[index]))
    }

    /// The tab holding a session, if the session is still the tab's own.
    pub fn tab_of(&self, session: SessionId) -> Option<TabView<'_>> {
        self.tabs
            .iter()
            .find(|tab| tab.session == Some(session))
            .map(TabView)
    }

    /// The shown tab.
    pub fn shown(&self) -> Option<TabId> {
        self.shown
    }

    /// The session of the shown tab, if it has one.
    pub fn shown_session(&self) -> Option<SessionId> {
        self.shown
            .and_then(|id| self.find(id))
            .and_then(|index| self.tabs[index].session)
    }

    /// The number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// There are no tabs.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The machines as the file keeps them: the remembered machines in tab
    /// order, those wanted at the next start as `open`, and the shown one.
    pub fn tab_set(&self) -> TabSet {
        let mut set = TabSet::default();
        for tab in self.tabs.iter().filter(|tab| tab.known) {
            set.remember(tab.target.clone());
            if tab.open {
                set.open_machine(&tab.target);
            }
        }
        if let Some(tab) = self
            .shown
            .and_then(|id| self.find(id))
            .map(|index| &self.tabs[index])
            .filter(|tab| tab.known && tab.open)
        {
            set.set_shown(&tab.target);
        }
        set
    }

    /// A click on a tab: show it, starting it first when it has no session
    /// (stopped, gave up, or waiting to retry). A click on the shown, running tab does
    /// nothing. A tab that failed for a reason the person has to fix (a key,
    /// a host key, a missing or old server) is only shown: starting it would
    /// fail the same way and hide the explanation behind "connecting". The
    /// failure panel's "Try again" is [`Tabs::reconnect`].
    pub fn click(&mut self, id: TabId) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.click_into(id, &mut effects);
        effects
    }

    /// Starts a new session for a tab that has none, at once, and shows it:
    /// "Try again" on the failure panel and "Reconnect" in the tab menu.
    pub fn reconnect(&mut self, id: TabId) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.start_into(id, &mut effects);
        effects
    }

    /// A host the user typed or named: the tab with that target is clicked;
    /// with none, a tab is added at the end and clicked.
    pub fn open(&mut self, target: &HostTarget) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.open_one(target, &mut effects);
        effects
    }

    /// A switch chord or menu command; same as clicking the tab it lands
    /// on.
    pub fn switch(&mut self, to: SwitchTo) -> Vec<Effect> {
        let count = self.tabs.len();
        if count == 0 {
            return Vec::new();
        }
        let current = self.shown.and_then(|id| self.find(id));
        let index = match to {
            SwitchTo::Next => current.map_or(0, |at| (at + 1) % count),
            SwitchTo::Previous => current.map_or(count - 1, |at| (at + count - 1) % count),
            SwitchTo::Position(place) => match usize::from(place).checked_sub(1) {
                Some(index) if index < count => index,
                _ => return Vec::new(),
            },
        };
        let id = self.tabs[index].id;
        self.click(id)
    }

    /// The session reports it is up. Repeats are ignored (the session
    /// reports on every resize).
    pub fn connected(&mut self, session: SessionId, now: Instant) -> Vec<Effect> {
        let Some(index) = self.index_of(session) else {
            return Vec::new();
        };
        if self.tabs[index].status == Status::Streaming {
            return Vec::new();
        }
        let mut effects = Vec::new();
        let tab = &mut self.tabs[index];
        tab.status = Status::Streaming;
        tab.backoff.up(now);
        if tab.record_pending {
            tab.record_pending = false;
            effects.push(Effect::RecordHost(tab.target.clone()));
        }
        if !tab.known {
            tab.known = true;
            effects.push(Effect::Persist);
        }
        if self.shown_session() == Some(session) {
            effects.push(Effect::ClipOffer(session));
        }
        effects
    }

    /// The session ended on its own at `now`. An end from a session that is
    /// no longer the tab's (stopped, replaced) is dropped.
    ///
    /// A reason that can pass by itself puts the tab in `Reconnecting`,
    /// due after the policy's wait (see [`Tabs::due`]); any other reason, or
    /// the last retry used up, leaves it `GaveUp`. Either way the shown tab
    /// stays shown and the route points at nothing until the retry starts.
    pub fn ended(&mut self, session: SessionId, why: End, now: Instant) -> Vec<Effect> {
        let Some(index) = self.index_of(session) else {
            return Vec::new();
        };
        let retryable = reconnect::retryable(&why);
        let policy = self.policy;
        let tab = &mut self.tabs[index];
        tab.session = None;
        tab.failure = None;
        let text = match why {
            End::Closed => "the remote machine closed the session".to_owned(),
            End::Error(text) => text,
            End::Incompatible => explain(tab, SshFailure::ProtocolMismatch),
            End::Ssh(failure) => explain(tab, failure),
        };
        tab.status = match tab.backoff.ended(&policy, now, retryable) {
            Next::Retry { attempt, after } => Status::Reconnecting {
                attempt,
                of: policy.max_retries,
                why: text,
                at: Some(now + after),
            },
            Next::GiveUp { exhausted } => {
                tab.record_pending = false;
                Status::GaveUp(if exhausted {
                    format!("{text}; gave up after {} retries", policy.max_retries)
                } else {
                    text
                })
            }
        };
        let mut effects = Vec::new();
        if self.shown == Some(self.tabs[index].id) {
            effects.push(Effect::InputTo(None));
            effects.push(Effect::PresentNothing);
        }
        effects.push(Effect::ClipGone(session));
        effects.push(Effect::Stop(session));
        effects
    }

    /// Starts every retry whose time has come. A hidden tab only starts; a
    /// shown one also takes the route and the picture, as a click would.
    /// Calling it early, or twice, does nothing.
    pub fn due(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        for index in 0..self.tabs.len() {
            let ripe = matches!(
                &self.tabs[index].status,
                Status::Reconnecting { at: Some(at), .. } if *at <= now
            );
            if !ripe {
                continue;
            }
            let id = self.tabs[index].id;
            effects.push(self.begin(index, Cause::Retry));
            if self.shown == Some(id) {
                self.show(None, id, &mut effects);
            }
        }
        effects
    }

    /// When the earliest waiting retry is due, for the window's timer.
    pub fn next_due(&self) -> Option<Instant> {
        self.tabs
            .iter()
            .filter_map(|tab| match &tab.status {
                Status::Reconnecting { at, .. } => *at,
                _ => None,
            })
            .min()
    }

    /// The button on a running tab, or "stop" from a menu: end the session
    /// and keep the tab. Stopped by hand stays stopped at the next start.
    /// Stopping the shown tab shows the nearest running tab (right first,
    /// then left), or nothing.
    pub fn stop(&mut self, id: TabId) -> Vec<Effect> {
        let Some(index) = self.find(id) else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        self.retire(index, &mut effects);
        let tab = &mut self.tabs[index];
        tab.open = false;
        tab.status = Status::Stopped;
        tab.backoff.reset();
        effects.push(Effect::Persist);
        effects
    }

    /// The button on a stopped tab: remove the tab (stopping it first).
    pub fn forget(&mut self, id: TabId) -> Vec<Effect> {
        let Some(index) = self.find(id) else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        self.retire(index, &mut effects);
        if self.shown == Some(id) {
            // A shown tab that had no session (a session was handed on by
            // `retire`): the neighbour rule still picks the nearest running
            // tab.
            match self.neighbour(index) {
                Some(next) => self.show(None, next, &mut effects),
                None => {
                    self.shown = None;
                    effects.push(Effect::InputTo(None));
                    effects.push(Effect::PresentNothing);
                }
            }
        }
        self.tabs.remove(index);
        effects.push(Effect::Persist);
        effects
    }

    /// Drag a tab before another, or to the end when `before` is `None`.
    /// A tab dropped on itself, or an unknown tab, does nothing.
    pub fn move_tab(&mut self, id: TabId, before: Option<TabId>) -> Vec<Effect> {
        let Some(from) = self.find(id) else {
            return Vec::new();
        };
        let to = match before {
            Some(other) if other == id => return Vec::new(),
            Some(other) => match self.find(other) {
                Some(at) => at,
                None => return Vec::new(),
            },
            None => self.tabs.len(),
        };
        let tab = self.tabs.remove(from);
        let to = if to > from { to - 1 } else { to };
        self.tabs.insert(to, tab);
        vec![Effect::Persist]
    }

    fn find(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    fn index_of(&self, session: SessionId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.session == Some(session))
    }

    fn push_tab(&mut self, target: HostTarget, known: bool) -> usize {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        self.tabs.push(Tab {
            id,
            target,
            session: None,
            status: Status::Stopped,
            backoff: Backoff::default(),
            known,
            open: false,
            record_pending: false,
            failure: None,
        });
        self.tabs.len() - 1
    }

    /// Gives the tab a new session and returns the effect that starts it.
    fn begin(&mut self, index: usize, cause: Cause) -> Effect {
        let session = SessionId(self.next_session);
        self.next_session += 1;
        let tab = &mut self.tabs[index];
        tab.session = Some(session);
        tab.open = true;
        match cause {
            Cause::User => tab.record_pending = true,
            Cause::Restore => tab.record_pending = false,
            // The retry is the same wish as the attempt it replaces.
            Cause::Retry => {}
        }
        tab.status = match (
            cause,
            std::mem::replace(&mut tab.status, Status::Connecting),
        ) {
            (
                Cause::Retry,
                Status::Reconnecting {
                    attempt, of, why, ..
                },
            ) => Status::Reconnecting {
                attempt,
                of,
                why,
                at: None,
            },
            _ => Status::Connecting,
        };
        if cause != Cause::Retry {
            tab.backoff.reset();
        }
        Effect::Start {
            tab: tab.id,
            session,
            target: tab.target.clone(),
            cause,
        }
    }

    fn click_into(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(index) = self.find(id) else {
            return;
        };
        let old = self.shown_session();
        if self.tabs[index].is_running() || self.tabs[index].needs_the_person() {
            if self.shown != Some(id) {
                self.show(old, id, effects);
                effects.push(Effect::Persist);
            }
            return;
        }
        self.start_into(id, effects);
    }

    /// Starts a tab that has no session and shows it. A tab with a session
    /// is left as it is.
    fn start_into(&mut self, id: TabId, effects: &mut Vec<Effect>) {
        let Some(index) = self.find(id) else {
            return;
        };
        if self.tabs[index].is_running() {
            return;
        }
        let old = self.shown_session();
        effects.push(self.begin(index, Cause::User));
        self.show(old, id, effects);
        effects.push(Effect::Persist);
    }

    /// Clicks the tab with this target, adding the tab first if there is
    /// none.
    fn open_one(&mut self, target: &HostTarget, effects: &mut Vec<Effect>) {
        let id = self.tab_for(target);
        self.click_into(id, effects);
    }

    /// Starts the tab with this target without showing it.
    fn open_in_background(&mut self, target: &HostTarget, effects: &mut Vec<Effect>) {
        let id = self.tab_for(target);
        let Some(index) = self.find(id) else {
            return;
        };
        if !self.tabs[index].is_running() {
            effects.push(self.begin(index, Cause::User));
            effects.push(Effect::Persist);
        }
    }

    fn tab_for(&mut self, target: &HostTarget) -> TabId {
        match self.tabs.iter().find(|tab| tab.target.same(target)) {
            Some(tab) => tab.id,
            None => {
                let index = self.push_tab(target.clone(), false);
                self.tabs[index].id
            }
        }
    }

    /// Makes `id` the shown tab. `old` is the session that was shown (taken
    /// before a start changed the tab). The input route moves first.
    fn show(&mut self, old: Option<SessionId>, id: TabId, effects: &mut Vec<Effect>) {
        let Some(index) = self.find(id) else {
            return;
        };
        let new = self.tabs[index].session;
        self.shown = Some(id);
        effects.push(Effect::InputTo(new));
        if let Some(old) = old.filter(|old| Some(*old) != new) {
            effects.push(Effect::ReleaseInput(old));
            effects.push(Effect::ClipRevoke(old));
            effects.push(Effect::ClipCancel(old));
        }
        match new {
            Some(session) => {
                if self.tabs[index].status == Status::Streaming {
                    effects.push(Effect::ClipOffer(session));
                }
                effects.push(Effect::Present(session));
            }
            None => effects.push(Effect::PresentNothing),
        }
    }

    /// Ends the session of the tab at `index`, if it has one: when it is
    /// shown, first moves the show to the neighbour rule's tab.
    fn retire(&mut self, index: usize, effects: &mut Vec<Effect>) {
        let Some(session) = self.tabs[index].session else {
            return;
        };
        let id = self.tabs[index].id;
        if self.shown == Some(id) {
            match self.neighbour(index) {
                Some(next) => self.show(Some(session), next, effects),
                None => {
                    self.shown = None;
                    effects.push(Effect::InputTo(None));
                    effects.push(Effect::ReleaseInput(session));
                    effects.push(Effect::ClipRevoke(session));
                    effects.push(Effect::ClipCancel(session));
                    effects.push(Effect::PresentNothing);
                }
            }
        }
        let tab = &mut self.tabs[index];
        tab.session = None;
        tab.record_pending = false;
        effects.push(Effect::ClipGone(session));
        effects.push(Effect::Stop(session));
    }

    /// The nearest tab with a session: right of `index` first, then left.
    fn neighbour(&self, index: usize) -> Option<TabId> {
        let right = self.tabs.iter().skip(index + 1);
        let left = self.tabs.iter().take(index).rev();
        right
            .chain(left)
            .find(|tab| tab.is_running())
            .map(|tab| tab.id)
    }
}

#[cfg(test)]
mod tests;
