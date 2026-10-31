//! The clipboard switchboard: one Windows clipboard thread in front of one
//! engine per session.
//!
//! The Windows thread (`clip_win`) talks to a single channel and a single
//! [`Host`]; it neither knows nor needs to know that several sessions exist.
//! The hub sits between them and applies [`ClipRouter`]'s rules:
//!
//! - Commands from the Windows thread are sent on to the right engines
//!   (`RevokeLocal` to all, `LocalOffer` to the shown one, a paste to the
//!   owner of the offer on the clipboard, a cancel to the engine that numbered
//!   the transfer).
//! - Events from an engine pass through a per-session [`Host`] that drops a
//!   background session's remote offer, refuses its read of the local
//!   clipboard, and gives its transfers ids that do not clash.
//!
//! The echo guard of the Windows thread is untouched: what a remote offer
//! wrote to the clipboard is never sent back out, so a copy made in one tab is
//! not offered to another on a switch.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::PoisonError;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::clip_engine::Command;
use crate::clip_engine::HostEvent;
use crate::clip_link::EngineIn;
use crate::clip_link::Host;
use crate::clip_link::Port;
use crate::clip_log::Cause;
use crate::clip_log::RevokeWhy;
use crate::route::ClipRouter;
use crate::route::Gone;
use crate::tabs::SessionId;

struct State {
    router: ClipRouter,
    engines: HashMap<SessionId, UnboundedSender<EngineIn>>,
}

struct Shared {
    state: Mutex<State>,
    windows: OnceLock<Arc<dyn Host>>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The switchboard. Cheap to clone; the UI thread drives `show` and
/// `session_gone`.
#[derive(Clone)]
pub struct ClipHub {
    shared: Arc<Shared>,
}

impl ClipHub {
    /// A hub and the sender to give the Windows clipboard thread. A thread
    /// reads that channel until every sender is dropped.
    pub fn new() -> std::io::Result<(Self, UnboundedSender<EngineIn>)> {
        let (input, output) = Port::channel();
        let hub = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    router: ClipRouter::new(),
                    engines: HashMap::new(),
                }),
                windows: OnceLock::new(),
            }),
        };
        let shared = Arc::clone(&hub.shared);
        std::thread::Builder::new()
            .name("gliff-clip-router".to_owned())
            .spawn(move || dispatch(&shared, output))?;
        Ok((hub, input))
    }

    /// The Windows thread's host, known once that thread has started.
    pub fn attach_windows(&self, host: Arc<dyn Host>) {
        // A second call keeps the first host.
        let _ = self.shared.windows.set(host);
    }

    /// The port for a new session's engine.
    pub fn add_session(&self, session: SessionId, spool_base: PathBuf) -> Port {
        let (input, output) = Port::channel();
        let host = self.register(session, input.clone());
        Port::new(input, output, host, spool_base).with_session(session.get())
    }

    /// Registers an engine's input and returns the host to give it.
    fn register(&self, session: SessionId, engine: UnboundedSender<EngineIn>) -> Arc<dyn Host> {
        let mut state = self.shared.state();
        state.router.session_started(session);
        state.engines.insert(session, engine);
        Arc::new(SessionHost {
            session,
            shared: Arc::clone(&self.shared),
        })
    }

    /// The shown session changed.
    pub fn show(&self, session: Option<SessionId>) {
        self.shared.state().router.show(session);
    }

    /// Withdraws the local offer this session's engine holds, without
    /// touching the others (the session was just left for another).
    pub fn revoke(&self, session: SessionId) {
        self.send(session, Command::RevokeLocal(RevokeWhy::TabLeft));
    }

    /// Cancels every file transfer this session's engine has numbered.
    pub fn cancel_transfers(&self, session: SessionId) {
        let ids = self.shared.state().router.transfers_of(session);
        for id in ids {
            self.send(session, Command::CancelTransfer(id));
        }
    }

    fn send(&self, session: SessionId, command: Command) {
        if let Some(engine) = self.shared.state().engines.get(&session) {
            let _ = engine.send(EngineIn::Command(command));
        }
    }

    /// A session is leaving: its engine is told to stop (which fails a paste
    /// waiting on it at once), and the Windows clipboard gives up its data.
    pub fn session_gone(&self, session: SessionId) {
        let (engine, gone) = {
            let mut state = self.shared.state();
            let gone = state.router.session_gone(session);
            (state.engines.remove(&session), gone)
        };
        if let Some(engine) = engine {
            let _ = engine.send(EngineIn::Stop);
        }
        self.withdraw(gone);
    }

    /// Sharing is turned off or back on for a running session. Off withdraws
    /// its local offer, cancels its transfers, and takes its data back from
    /// the Windows clipboard if that is still ours; until it is back on,
    /// nothing passes either way.
    pub fn set_sharing(&self, session: SessionId, share: bool) {
        let gone = self.shared.state().router.set_sharing(session, share);
        if !share {
            self.send(session, Command::RevokeLocal(RevokeWhy::SharingOff));
            self.cancel_transfers(session);
        }
        self.withdraw(gone);
    }

    /// An empty offer: the Windows thread clears the data it promised, only
    /// while the clipboard is still its own.
    fn withdraw(&self, gone: Gone) {
        if gone.withdraw_remote
            && let Some(windows) = self.shared.windows.get()
        {
            windows.event(HostEvent::RemoteOffer {
                serial: 0,
                mime_types: Vec::new(),
                files: Vec::new(),
            });
        }
    }
}

/// Sends the Windows thread's commands where the rules say.
fn dispatch(shared: &Shared, mut output: UnboundedReceiver<EngineIn>) {
    while let Some(message) = output.blocking_recv() {
        let EngineIn::Command(command) = message else {
            // Peer data and Stop come from a session, never from the Windows
            // thread.
            continue;
        };
        let state = shared.state();
        let send = |session: SessionId, command: Command| {
            if let Some(engine) = state.engines.get(&session) {
                let _ = engine.send(EngineIn::Command(command));
            }
        };
        match command {
            Command::RevokeLocal(why) => {
                for session in state.router.revoke_targets() {
                    send(session, Command::RevokeLocal(why));
                }
            }
            offer @ Command::LocalOffer { .. } => {
                if let Some(session) = state.router.offer_target() {
                    send(session, offer);
                }
            }
            fetch @ (Command::FetchMime { .. } | Command::FetchFiles { .. }) => {
                // With no owner the reply channel is dropped: the paste
                // fails at once.
                if let Some(session) = state.router.fetch_target() {
                    send(session, fetch);
                }
            }
            Command::CancelTransfer(global) => {
                if let Some((session, local)) = state.router.cancel_target(global) {
                    send(session, Command::CancelTransfer(local));
                }
            }
        }
    }
}

/// One session's engine, as the Windows thread's host sees it.
struct SessionHost {
    session: SessionId,
    shared: Arc<Shared>,
}

impl Host for SessionHost {
    fn event(&self, event: HostEvent) {
        let Some(windows) = self.shared.windows.get() else {
            if let HostEvent::ReadLocal { reply, .. } = event {
                let _ = reply.send(Err(Cause::NotReady));
            }
            return;
        };
        match event {
            HostEvent::RemoteOffer {
                serial,
                mime_types,
                files,
            } => {
                let empty = mime_types.is_empty() && files.is_empty();
                let passed = self.shared.state().router.remote_offer(self.session, empty);
                if passed {
                    windows.event(HostEvent::RemoteOffer {
                        serial,
                        mime_types,
                        files,
                    });
                }
            }
            HostEvent::ReadLocal {
                mime_type,
                generation,
                reply,
            } => {
                let allowed = self.shared.state().router.read_local_allowed(self.session);
                if allowed {
                    windows.event(HostEvent::ReadLocal {
                        mime_type,
                        generation,
                        reply,
                    });
                } else {
                    let _ = reply.send(Err(Cause::NotShown));
                }
            }
            HostEvent::Transfer { id, progress } => {
                let id = self.shared.state().router.global_transfer(self.session, id);
                windows.event(HostEvent::Transfer { id, progress });
            }
        }
    }
}

#[cfg(test)]
mod tests;
