//! The running sessions of one window, and the runner of [`Effect`]s.
//!
//! [`Tabs`] decides; this module does. It owns every live session's handle,
//! the [`InputRoute`] and the clipboard side, and turns each effect of the
//! model into the matching call, in the order given. It holds no rule of its
//! own: which tab is shown and what a switch releases is the model's, and
//! which engine gets which clipboard request is `route.rs`'s.
//!
//! Sessions and the clipboard are reached through two small traits ([`Link`],
//! [`ClipSide`]), so the runner is tested with fakes and the real
//! `SessionHandle` and `ClipHub` plug in unchanged.
//!
//! # Lifetimes
//!
//! A session that [`Effect::Stop`] names is told `Bye` and moved to the
//! retiring list; the UI thread never waits for it. [`SessionSet::poll`] joins
//! it once its thread has finished, and [`SessionSet::shutdown`] joins the
//! rest. A session whose thread ends without an `Ended` event (a panic) is
//! reported to the model as an error, so its tab does not stay `Connecting`.
//!
//! # Time
//!
//! The model takes `now` as an argument. This runner reads it from a clock it
//! owns (`Instant::now`, or a test's fake through [`SessionSet::with_clock`])
//! and starts the retries that are due at the end of every [`SessionSet::poll`],
//! so the window needs only a timer that makes it poll at
//! [`Tabs::next_due`]. After [`SessionSet::close_all`] or
//! [`SessionSet::shutdown`] no retry starts.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::clip_hub::ClipHub;
use crate::clip_link::Port;
use crate::history::HostTarget;
use crate::route::InputRoute;
use crate::session::EndReason;
use crate::session::SessionStats;
use crate::session::UiCmd;
use crate::session::UiEvent;
use crate::session::driver::SessionEnd;
use crate::session::driver::SessionHandle;
use crate::tabs::Cause;
use crate::tabs::Effect;
use crate::tabs::End;
use crate::tabs::SessionId;
use crate::tabs::Status;
use crate::tabs::TabId;
use crate::tabs::Tabs;

/// One running session, as the runner sees it.
pub trait Link {
    /// A sender for this session's commands (the route holds a clone).
    fn commands(&self) -> UnboundedSender<UiCmd>;
    /// The next queued event, without blocking.
    fn try_event(&self) -> Option<UiEvent>;
    /// The counters now.
    fn stats(&self) -> SessionStats;
    /// The thread has ended; every event it sent is already queued.
    fn is_finished(&self) -> bool;
    /// Waits for the thread. Called after `Bye`, or once finished.
    fn finish(self) -> SessionEnd;
}

impl Link for SessionHandle {
    fn commands(&self) -> UnboundedSender<UiCmd> {
        SessionHandle::commands(self)
    }

    fn try_event(&self) -> Option<UiEvent> {
        SessionHandle::try_event(self)
    }

    fn stats(&self) -> SessionStats {
        SessionHandle::stats(self)
    }

    fn is_finished(&self) -> bool {
        SessionHandle::is_finished(self)
    }

    fn finish(self) -> SessionEnd {
        self.join()
    }
}

/// The clipboard side of the effects.
pub trait ClipSide {
    /// Registers a session before it starts and returns its engine's port.
    /// With `share` false the session is registered all the same, so the
    /// router's books stay whole, but it gets no port: its clipboard does
    /// not run.
    fn add_session(&mut self, session: SessionId, share: bool) -> Option<Port>;
    /// Tells the router which session is shown (or none).
    fn show(&mut self, session: Option<SessionId>);
    /// Withdraws the local offer from one session's engine.
    fn revoke(&mut self, session: SessionId);
    /// Cancels one session's transfers.
    fn cancel(&mut self, session: SessionId);
    /// Offers the current Windows clipboard to the shown session.
    fn offer(&mut self);
    /// The session is leaving.
    fn gone(&mut self, session: SessionId);
    /// Sharing is turned off or back on for a session that started with it.
    fn set_sharing(&mut self, session: SessionId, share: bool);
}

/// [`ClipSide`] over the real hub. `rescan` makes the Windows clipboard
/// thread re-read the clipboard and send it as a new local offer.
pub struct HubClip {
    pub hub: ClipHub,
    pub spool_base: std::path::PathBuf,
    pub rescan: Box<dyn Fn() + Send>,
}

impl ClipSide for HubClip {
    fn add_session(&mut self, session: SessionId, share: bool) -> Option<Port> {
        let port = self.hub.add_session(session, self.spool_base.clone());
        share.then_some(port)
    }

    fn show(&mut self, session: Option<SessionId>) {
        self.hub.show(session);
    }

    fn revoke(&mut self, session: SessionId) {
        self.hub.revoke(session);
    }

    fn cancel(&mut self, session: SessionId) {
        self.hub.cancel_transfers(session);
    }

    fn offer(&mut self) {
        (self.rescan)();
    }

    fn gone(&mut self, session: SessionId) {
        self.hub.session_gone(session);
    }

    fn set_sharing(&mut self, session: SessionId, share: bool) {
        self.hub.set_sharing(session, share);
        if share {
            (self.rescan)();
        }
    }
}

/// What the starter needs to start one session.
pub struct Launch {
    pub tab: TabId,
    pub session: SessionId,
    pub target: HostTarget,
    pub cause: Cause,
    /// The clipboard engine's port, when there is a clipboard side.
    pub clip: Option<Port>,
}

/// Starts a session. An error is a session that never ran; its tab shows the
/// text.
pub type Starter<L> = Box<dyn FnMut(Launch) -> io::Result<L>>;

/// What the UI should show after effects ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presented {
    /// This session's view.
    Session(SessionId),
    /// An empty picture area.
    Nothing,
}

/// The results of running effects that only the UI thread can act on.
#[derive(Debug, Default)]
pub struct Applied {
    /// The last `Present` or `PresentNothing`, if any ran.
    pub presented: Option<Presented>,
    /// The tab set changed: save it.
    pub persist: bool,
    /// Hosts to put first in the recent hosts, in order.
    pub recorded: Vec<HostTarget>,
    /// Sessions whose first `Start` call failed, with the reason. [`SessionSet::run`]
    /// reports these to the model; they stay here for the log.
    pub failed: Vec<(SessionId, String)>,
    /// Sessions whose handle was joined, with how they ended.
    pub ended: Vec<(SessionId, SessionEnd)>,
    /// Events the model does not own (a picture's size, a cursor, the grab
    /// released), for the view of that session.
    pub events: Vec<(SessionId, UiEvent)>,
}

impl Applied {
    fn merge(&mut self, other: Self) {
        self.presented = other.presented.or(self.presented);
        self.persist |= other.persist;
        self.recorded.extend(other.recorded);
        self.failed.extend(other.failed);
        self.ended.extend(other.ended);
        self.events.extend(other.events);
    }
}

/// One live session's line in `--stats-out`.
#[derive(Clone, Debug)]
pub struct Report {
    pub session: SessionId,
    pub target: HostTarget,
    pub stats: SessionStats,
}

struct Entry<L> {
    link: L,
    target: HostTarget,
    /// An `Ended` event was seen; the model has been told.
    ended: bool,
}

/// The sessions of a window.
pub struct SessionSet<L: Link> {
    route: Arc<InputRoute<UiCmd>>,
    clip: Option<Box<dyn ClipSide>>,
    starter: Starter<L>,
    live: BTreeMap<SessionId, Entry<L>>,
    retiring: Vec<(SessionId, L)>,
    clock: Box<dyn Fn() -> Instant>,
    /// Whether a machine's clipboard is shared, asked at each connect.
    shares: Box<dyn Fn(&HostTarget) -> bool>,
    /// The window is closing: no retry starts.
    closing: bool,
}

/// The model's words for how a session ended.
pub fn end_of(reason: &EndReason) -> End {
    match reason {
        EndReason::Bye => End::Closed,
        EndReason::Incompatible(_) => End::Incompatible,
        EndReason::ServerError { message, .. } => End::Error(message.clone()),
        EndReason::Link(text) => End::Error(text.clone()),
        EndReason::Ssh(failure) => End::Ssh(failure.clone()),
    }
}

impl<L: Link> SessionSet<L> {
    pub fn new(
        route: Arc<InputRoute<UiCmd>>,
        clip: Option<Box<dyn ClipSide>>,
        starter: Starter<L>,
    ) -> Self {
        Self {
            route,
            clip,
            starter,
            live: BTreeMap::new(),
            retiring: Vec::new(),
            clock: Box::new(Instant::now),
            shares: Box::new(|_| true),
            closing: false,
        }
    }

    /// The same set with another clock, for tests.
    #[must_use]
    pub fn with_clock(mut self, clock: Box<dyn Fn() -> Instant>) -> Self {
        self.clock = clock;
        self
    }

    /// The same set, asking `shares` at each connect whether that machine's
    /// clipboard is shared. The default is yes for all.
    #[must_use]
    pub fn with_share_policy(mut self, shares: Box<dyn Fn(&HostTarget) -> bool>) -> Self {
        self.shares = shares;
        self
    }

    /// The live session's link, e.g. for its frame slot.
    pub fn get(&self, session: SessionId) -> Option<&L> {
        self.live.get(&session).map(|entry| &entry.link)
    }

    /// The sessions that run now.
    pub fn sessions(&self) -> Vec<SessionId> {
        self.live.keys().copied().collect()
    }

    /// The counters of every live session, in session order.
    pub fn reports(&self) -> Vec<Report> {
        self.live
            .iter()
            .map(|(session, entry)| Report {
                session: *session,
                target: entry.target.clone(),
                stats: entry.link.stats(),
            })
            .collect()
    }

    /// Runs effects in order, then feeds sessions that failed to start back
    /// to the model and runs what it answers, until nothing is left.
    pub fn run(&mut self, tabs: &mut Tabs, effects: Vec<Effect>) -> Applied {
        let mut total = Applied::default();
        let mut queue = effects;
        while !queue.is_empty() {
            let mut one = self.apply(queue);
            queue = Vec::new();
            for (session, reason) in &one.failed {
                queue.extend(self.ended(tabs, *session, End::Error(reason.clone())));
            }
            one.failed.clear();
            total.merge(one);
        }
        total
    }

    /// Runs effects in order. Sessions that failed to start are in
    /// `failed`; the model does not know yet (see [`SessionSet::run`]).
    pub fn apply(&mut self, effects: Vec<Effect>) -> Applied {
        let mut out = Applied::default();
        for effect in effects {
            match effect {
                Effect::InputTo(session) => self.input_to(session),
                Effect::ReleaseInput(session) => self.send(session, UiCmd::ReleaseInput),
                Effect::ClipRevoke(session) => {
                    if let Some(clip) = &mut self.clip {
                        clip.revoke(session);
                    }
                }
                Effect::ClipCancel(session) => {
                    if let Some(clip) = &mut self.clip {
                        clip.cancel(session);
                    }
                }
                Effect::ClipOffer(session) => {
                    if let Some(clip) = &mut self.clip {
                        clip.show(Some(session));
                        clip.offer();
                    }
                }
                Effect::Present(session) => {
                    if let Some(clip) = &mut self.clip {
                        clip.show(Some(session));
                    }
                    out.presented = Some(Presented::Session(session));
                }
                Effect::PresentNothing => {
                    if let Some(clip) = &mut self.clip {
                        clip.show(None);
                    }
                    out.presented = Some(Presented::Nothing);
                }
                Effect::Start {
                    tab,
                    session,
                    target,
                    cause,
                } => self.start(tab, session, target, cause, &mut out),
                Effect::RecordHost(target) => out.recorded.push(target),
                Effect::ClipGone(session) => {
                    if let Some(clip) = &mut self.clip {
                        clip.gone(session);
                    }
                }
                Effect::Stop(session) => self.retire(session),
                Effect::Persist => out.persist = true,
            }
        }
        out
    }

    /// Takes every queued event, tells the model what it owns (a connect, an
    /// end), applies what it answers, joins sessions that finished, and
    /// returns them for the views (a connect is both the model's and the
    /// view's: it carries the machine's kind).
    pub fn poll(&mut self, tabs: &mut Tabs) -> Applied {
        let mut total = Applied::default();
        let sessions = self.sessions();
        for session in sessions {
            // Finished first: every event the thread sent is queued by then.
            let finished = self
                .live
                .get(&session)
                .is_some_and(|e| e.link.is_finished());
            let mut effects = Vec::new();
            while let Some(event) = self.live.get(&session).and_then(|e| e.link.try_event()) {
                match event {
                    UiEvent::Connected(info) => {
                        effects.extend(tabs.connected(session, (self.clock)()));
                        // The view wants the machine's kind too.
                        total.events.push((session, UiEvent::Connected(info)));
                    }
                    UiEvent::Ended(reason) => {
                        if let Some(entry) = self.live.get_mut(&session) {
                            entry.ended = true;
                        }
                        effects.extend(self.ended(tabs, session, end_of(&reason)));
                    }
                    other => total.events.push((session, other)),
                }
            }
            let silent = finished && self.live.get(&session).is_some_and(|e| !e.ended);
            if silent {
                let why = "the session stopped unexpectedly".to_owned();
                effects.extend(self.ended(tabs, session, End::Error(why)));
            }
            total.merge(self.run(tabs, effects));
        }
        if !self.closing {
            let due = tabs.due((self.clock)());
            total.merge(self.run(tabs, due));
        }
        total.ended.extend(self.reap());
        total
    }

    /// Tells the model a session ended and logs what it decided.
    fn ended(&self, tabs: &mut Tabs, session: SessionId, why: End) -> Vec<Effect> {
        let now = (self.clock)();
        let tab = tabs.tab_of(session).map(|tab| tab.id());
        let effects = tabs.ended(session, why, now);
        if let Some(view) = tab.and_then(|id| tabs.tab(id)) {
            match view.status() {
                Status::Reconnecting {
                    attempt,
                    of,
                    why,
                    at,
                } => crate::logging::line(&format!(
                    "session {}: ended ({why}); {}: retry {attempt} of {of} in {:.1} s",
                    session.get(),
                    view.target(),
                    at.map_or(0.0, |at| at.saturating_duration_since(now).as_secs_f64()),
                )),
                Status::GaveUp(why) => crate::logging::line(&format!(
                    "session {}: ended ({why}); {}: not retrying",
                    session.get(),
                    view.target(),
                )),
                _ => {}
            }
        }
        effects
    }

    /// Turns clipboard sharing off or back on for a session that started
    /// with it.
    pub fn set_sharing(&mut self, session: SessionId, share: bool) {
        if let Some(clip) = &mut self.clip {
            clip.set_sharing(session, share);
        }
    }

    /// How many sessions have a thread that has not been joined yet: the live
    /// ones and the retiring ones. The window closes when this reaches zero.
    pub fn running(&self) -> usize {
        self.live.len() + self.retiring.len()
    }

    /// Says `Bye` to every live session without waiting and without telling
    /// the model, so the tabs stay as they were for the next start. `poll`
    /// joins each thread once it has finished.
    pub fn close_all(&mut self) {
        self.closing = true;
        self.input_to(None);
        for session in self.sessions() {
            self.retire(session);
        }
    }

    /// Ends every session and waits for it. For the end of the program.
    pub fn shutdown(&mut self) -> Vec<(SessionId, SessionEnd)> {
        self.closing = true;
        self.input_to(None);
        let sessions = self.sessions();
        for session in sessions {
            self.retire(session);
        }
        let mut ended = Vec::new();
        for (session, link) in std::mem::take(&mut self.retiring) {
            ended.push((session, link.finish()));
        }
        ended
    }

    fn input_to(&mut self, session: Option<SessionId>) {
        let to = session.and_then(|id| self.live.get(&id).map(|e| (id, e.link.commands())));
        self.route.switch(to);
    }

    fn send(&self, session: SessionId, cmd: UiCmd) {
        if let Some(entry) = self.live.get(&session) {
            let _ = entry.link.commands().send(cmd);
        }
    }

    fn start(
        &mut self,
        tab: TabId,
        session: SessionId,
        target: HostTarget,
        cause: Cause,
        out: &mut Applied,
    ) {
        let share = (self.shares)(&target);
        let clip = self
            .clip
            .as_mut()
            .and_then(|clip| clip.add_session(session, share));
        let launch = Launch {
            tab,
            session,
            target: target.clone(),
            cause,
            clip,
        };
        if cause == Cause::Retry {
            crate::logging::line(&format!("session {}: retry for {target}", session.get()));
        }
        match (self.starter)(launch) {
            Ok(link) => {
                self.live.insert(
                    session,
                    Entry {
                        link,
                        target,
                        ended: false,
                    },
                );
            }
            Err(e) => {
                if let Some(clip) = &mut self.clip {
                    clip.gone(session);
                }
                out.failed.push((session, e.to_string()));
            }
        }
    }

    fn retire(&mut self, session: SessionId) {
        if self.route.target() == Some(session) {
            self.route.switch(None);
        }
        if let Some(entry) = self.live.remove(&session) {
            let _ = entry.link.commands().send(UiCmd::Bye);
            self.retiring.push((session, entry.link));
        }
    }

    fn reap(&mut self) -> Vec<(SessionId, SessionEnd)> {
        let mut ended = Vec::new();
        let mut keep = Vec::new();
        for (session, link) in std::mem::take(&mut self.retiring) {
            if link.is_finished() {
                ended.push((session, link.finish()));
            } else {
                keep.push((session, link));
            }
        }
        self.retiring = keep;
        ended
    }
}

#[cfg(test)]
mod tests;
