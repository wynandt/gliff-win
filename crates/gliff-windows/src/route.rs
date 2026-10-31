//! Where input and clipboard traffic goes when there are several sessions.
//!
//! [`InputRoute`] is the keyboard and pointer path: the hook thread and the
//! window read it, only the UI thread moves it. [`ClipRouter`] is the pure
//! rule set that decides which session's clipboard engine gets which request.
//!
//! # Why the route holds its lock across the send
//!
//! A switch must leave the old session with every key up. The UI thread moves
//! the route to the new session first, then sends `ReleaseInput` to the old
//! one. If `send` copied the old sender out of the lock and sent after
//! unlocking, a key could be sent after the release and arrive behind it: the
//! old session would then hold a key down for good. With the send inside the
//! lock, `switch` returns only after every key sent to the old session is
//! already in its channel, so the release that follows is behind all of them.
//! The channel is unbounded, so the send never blocks the lock.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::PoisonError;

use tokio::sync::mpsc::UnboundedSender;

use crate::tabs::SessionId;

/// The session that keys and pointer events go to, and its command channel.
pub struct InputRoute<C> {
    target: Mutex<Option<(SessionId, UnboundedSender<C>)>>,
}

impl<C> Default for InputRoute<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> InputRoute<C> {
    /// A route that goes nowhere.
    pub fn new() -> Self {
        Self {
            target: Mutex::new(None),
        }
    }

    /// Sends a command to the current session. False when there is none or
    /// its channel is closed: the caller lets the event through to Windows.
    pub fn send(&self, command: C) -> bool {
        let target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        match target.as_ref() {
            Some((_, sender)) => sender.send(command).is_ok(),
            None => false,
        }
    }

    /// Moves the route and returns the session it pointed at. Every command
    /// `send` accepted before this returns is in the old session's channel.
    pub fn switch(&self, to: Option<(SessionId, UnboundedSender<C>)>) -> Option<SessionId> {
        let mut target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::replace(&mut *target, to).map(|(session, _)| session)
    }

    /// The session the route points at.
    pub fn target(&self) -> Option<SessionId> {
        self.target
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|(session, _)| *session)
    }
}

/// The clipboard rules for several sessions, with no I/O.
///
/// There is one Windows clipboard and one engine per session. Only the shown
/// session shares the clipboard: the local clipboard is offered to it alone,
/// and only its remote offers reach Windows. Two pointers carry this:
/// `shown`, and `owner`, the session whose data the Windows clipboard
/// currently promises. A paste goes to the owner even after a switch, so a
/// copy made in one tab can still be pasted after moving to another until the
/// shown tab's own offer replaces it.
#[derive(Debug, Default)]
pub struct ClipRouter {
    running: BTreeSet<SessionId>,
    shown: Option<SessionId>,
    owner: Option<SessionId>,
    /// Running sessions whose sharing was turned off: they get nothing from
    /// the clipboard and give it nothing.
    off: BTreeSet<SessionId>,
    /// Transfer ids as engines number them, to one id for the Windows side.
    to_global: HashMap<(SessionId, u32), u32>,
    from_global: HashMap<u32, (SessionId, u32)>,
    next_global: u32,
}

/// What a session's departure asks of the clipboard.
#[derive(Debug, PartialEq, Eq)]
pub struct Gone {
    /// The Windows clipboard promised this session's data: withdraw it.
    pub withdraw_remote: bool,
}

impl ClipRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session's engine exists.
    pub fn session_started(&mut self, session: SessionId) {
        self.running.insert(session);
    }

    /// A session is leaving. Its transfers are forgotten; if the clipboard
    /// promised its data, that is to be withdrawn.
    pub fn session_gone(&mut self, session: SessionId) -> Gone {
        self.running.remove(&session);
        self.off.remove(&session);
        if self.shown == Some(session) {
            self.shown = None;
        }
        self.from_global.retain(|_, (owner, _)| *owner != session);
        self.to_global.retain(|(owner, _), _| *owner != session);
        let withdraw_remote = self.owner == Some(session);
        if withdraw_remote {
            self.owner = None;
        }
        Gone { withdraw_remote }
    }

    /// Sharing is turned off or back on for a running session. Turning it
    /// off takes back the ownership, as when the session leaves.
    pub fn set_sharing(&mut self, session: SessionId, share: bool) -> Gone {
        if share {
            self.off.remove(&session);
            return Gone {
                withdraw_remote: false,
            };
        }
        self.off.insert(session);
        let withdraw_remote = self.owner == Some(session);
        if withdraw_remote {
            self.owner = None;
        }
        Gone { withdraw_remote }
    }

    fn shares(&self, session: SessionId) -> bool {
        self.shown == Some(session) && !self.off.contains(&session)
    }

    /// The shown session changed (or nothing is shown).
    pub fn show(&mut self, session: Option<SessionId>) {
        self.shown = session;
    }

    /// Who gets `RevokeLocal` when the local clipboard changes: every
    /// running session, so none keeps serving a copy that is gone.
    pub fn revoke_targets(&self) -> Vec<SessionId> {
        self.running.iter().copied().collect()
    }

    /// Who gets the new local offer: the shown session only, if it shares.
    pub fn offer_target(&self) -> Option<SessionId> {
        self.shown
            .filter(|shown| self.running.contains(shown) && !self.off.contains(shown))
    }

    /// Who answers a paste: the session that owns the remote offer on the
    /// clipboard.
    pub fn fetch_target(&self) -> Option<SessionId> {
        self.owner.filter(|owner| self.running.contains(owner))
    }

    /// A session reports its remote clipboard. True when it goes to Windows:
    /// only the shown session's does, if it shares. An empty offer from the owner clears
    /// the ownership; a non-empty one takes it.
    pub fn remote_offer(&mut self, from: SessionId, empty: bool) -> bool {
        if !self.shares(from) {
            return false;
        }
        if empty {
            if self.owner == Some(from) {
                self.owner = None;
            }
        } else {
            self.owner = Some(from);
        }
        true
    }

    /// A session asks to read the local clipboard. Only the shown session may,
    /// if it shares (the others were revoked).
    pub fn read_local_allowed(&self, from: SessionId) -> bool {
        self.shares(from)
    }

    /// The id the Windows side sees for an engine's transfer.
    pub fn global_transfer(&mut self, session: SessionId, local: u32) -> u32 {
        if let Some(global) = self.to_global.get(&(session, local)) {
            return *global;
        }
        self.next_global = self.next_global.wrapping_add(1);
        let global = self.next_global;
        self.to_global.insert((session, local), global);
        self.from_global.insert(global, (session, local));
        global
    }

    /// The engine's own ids of this session's transfers, as numbered so far.
    pub fn transfers_of(&self, session: SessionId) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .to_global
            .keys()
            .filter(|(owner, _)| *owner == session)
            .map(|(_, local)| *local)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// The engine and its own id for a cancel of a global transfer id.
    pub fn cancel_target(&self, global: u32) -> Option<(SessionId, u32)> {
        self.from_global
            .get(&global)
            .copied()
            .filter(|(session, _)| self.running.contains(session))
    }
}

#[cfg(test)]
mod tests;
