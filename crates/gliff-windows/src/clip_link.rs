//! The clipboard engine's thread and the channels around it.
//!
//! Three threads touch the clipboard: the session thread (the ssh link),
//! the engine thread (this module: the transfer engine on a current-thread
//! runtime) and the Windows clipboard thread (`clip_win`). Whole bodies and
//! spool paths cross between them, never half-finished transfers, so the
//! Windows thread blocks on one answer per render request.
//!
//! This module has no Windows code: the Windows thread is a [`Host`], and
//! the session end is a closure, so both are replaced by fakes in tests.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::JoinHandle;

use bytes::Bytes;
use gliff_proto::ClipboardMsg;
use gliff_transport::clipboard::Side;
use gliff_transport::clipboard::Transfers;
use gliff_transport::clipboard::outbound_channel;
use tokio::sync::mpsc;

use crate::clip_engine::Command;
use crate::clip_engine::Engine;
use crate::clip_engine::HostEvent;
use crate::clip_files::Spool;
use crate::clip_log;
use crate::clip_log::Event;
use crate::clip_log::Notice;

/// What the engine thread receives.
pub enum EngineIn {
    /// A clipboard message from the server, with its `Data` payload.
    Peer(ClipboardMsg, Bytes),
    /// The Windows thread's request.
    Command(Command),
    /// Stop: the session is over.
    Stop,
}

/// The Windows thread, as the engine sees it. `event` runs on the engine
/// thread and must not block.
pub trait Host: Send + Sync + 'static {
    fn event(&self, event: HostEvent);
}

/// Everything the session needs to run the clipboard. Built by the app
/// before the session starts; the Windows thread holds a clone of `input`.
pub struct Port {
    pub input: mpsc::UnboundedSender<EngineIn>,
    output: mpsc::UnboundedReceiver<EngineIn>,
    host: Arc<dyn Host>,
    spool_base: PathBuf,
    /// The number the log tags this session's lines with.
    session: Option<u32>,
}

impl Port {
    /// A fresh channel pair: give the sender to the Windows thread, then
    /// build the port from the receiver and that thread's [`Host`].
    pub fn channel() -> (
        mpsc::UnboundedSender<EngineIn>,
        mpsc::UnboundedReceiver<EngineIn>,
    ) {
        mpsc::unbounded_channel()
    }

    pub fn new(
        input: mpsc::UnboundedSender<EngineIn>,
        output: mpsc::UnboundedReceiver<EngineIn>,
        host: Arc<dyn Host>,
        spool_base: PathBuf,
    ) -> Self {
        Self {
            input,
            output,
            host,
            spool_base,
            session: None,
        }
    }

    /// Tags the engine's log lines with the session's number.
    pub fn with_session(mut self, session: u32) -> Self {
        self.session = Some(session);
        self
    }
}

/// Sends one clipboard message (and its payload) to the server.
pub type SendToServer = Arc<dyn Fn(ClipboardMsg, Bytes) + Send + Sync>;

/// A running engine thread.
pub struct EngineThread {
    input: mpsc::UnboundedSender<EngineIn>,
    thread: Option<JoinHandle<()>>,
}

impl EngineThread {
    /// Forward a server message to the engine.
    pub fn peer(&self, msg: ClipboardMsg, payload: Bytes) {
        // A stopped engine has nobody to tell.
        let _ = self.input.send(EngineIn::Peer(msg, payload));
    }

    /// Stop the engine and wait for it. This process's spool is removed.
    pub fn stop(mut self) {
        let _ = self.input.send(EngineIn::Stop);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            clip_log::emit(Event::Notice(Notice::EngineThreadPanicked));
        }
    }
}

/// Starts the engine thread. It removes the spool directories of ended runs first.
pub fn spawn(port: Port, send: SendToServer) -> std::io::Result<EngineThread> {
    let input = port.input.clone();
    let thread = std::thread::Builder::new()
        .name("gliff-clipboard".to_owned())
        .spawn(move || run(port, send))?;
    Ok(EngineThread {
        input,
        thread: Some(thread),
    })
}

fn run(port: Port, send: SendToServer) {
    let Port {
        input,
        mut output,
        host,
        spool_base,
        session,
    } = port;
    if let Some(session) = session {
        clip_log::tag_thread(session);
    }
    // Only the senders held elsewhere keep this channel open.
    drop(input);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            clip_log::emit(Event::Notice(Notice::EngineRuntimeFailed));
            return;
        }
    };
    Spool::sweep_stale(&spool_base, crate::clip_files::process_alive);
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let (out, mut out_rx) = outbound_channel();
        let engine = Rc::new(Engine::new(
            Transfers::new(out, Side::Client),
            spool_base.clone(),
            Rc::new(move |event| host.event(event)),
        ));
        tokio::task::spawn_local(async move {
            while let Some((msg, payload)) = out_rx.recv().await {
                clip_log::outbound(&msg);
                send(msg, payload);
            }
        });
        while let Some(item) = output.recv().await {
            match item {
                EngineIn::Peer(msg, payload) => engine.on_peer_msg(msg, payload),
                EngineIn::Command(command) => engine.on_command(command),
                EngineIn::Stop => break,
            }
        }
    });
    // Dropping the runtime ends every transfer task, which drops the
    // answers the Windows thread may still be waiting for.
    drop(local);
    drop(runtime);
    Spool::remove_own(&spool_base);
}

#[cfg(test)]
mod tests;
