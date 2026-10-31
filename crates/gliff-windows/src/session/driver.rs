//! The client's session loop on its own thread: ssh, the protocol
//! [`Core`], the decoder, and the hand-off to the UI.
//!
//! `gwin-verify stream` keeps its own instrumented copy of this loop, so
//! the stream check is not changed under it; the shape is the same (a
//! writer task, a whole-message reader task, the idle drain, and
//! Bye → stdin EOF → wait for ssh).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use gliff_proto::msg::ClientMsg;
use gliff_proto::msg::ServerMsg;
use gliff_transport::Framed;
use tokio::process::Child;
use tokio::sync::mpsc;

use super::Action;
use super::Colour;
use super::Core;
use super::Decoded;
use super::EndReason;
use super::ResizeReq;
use super::SessionStats;
use super::UiCmd;
use super::UiEvent;
use super::client_caps;
use super::hello;
use super::resync::Watch;
use crate::altgr::DEADLINE_MS;
use crate::clip_link;
use crate::clip_link::EngineThread;
use crate::clip_link::Port;
use crate::connect::SshFailure;
use crate::connect::from_spawn_error;
use crate::connect::log_line;
use crate::decode::CpuDecoder;
use crate::frame_slot::FrameSlot;
use crate::input::InputOptions;
use crate::keymap::Press;
use crate::link::Inbound;
use crate::link::read_server;
use crate::pointer::PointerEvent;
use crate::ssh::SshSpec;
use crate::ssh::diagnose;
use crate::ssh::spawn;

/// With a held frame and no input for this long, flush the decoder (as
/// upstream's `IDLE_DRAIN`, net.rs).
pub const IDLE_DRAIN: Duration = Duration::from_millis(150);
/// How long after a key arrives the AltGr filter's deadline has passed: the
/// filter waits [`DEADLINE_MS`] and gives up when more than that has gone.
const KEY_HOLD: Duration = Duration::from_millis(DEADLINE_MS as u64 + 1);
/// The most queued commands one coalescing pass reads, so a flood of mouse
/// motion cannot starve the socket.
const COALESCE_LIMIT: usize = 256;
/// How long ssh may take to exit after Bye and stdin EOF.
pub const EXIT_WAIT: Duration = Duration::from_secs(5);

/// How long a lost link waits for ssh to exit before it stops looking for
/// ssh's reason: a connection that ssh failed to make exits at once.
const LINK_EXIT_WAIT: Duration = Duration::from_secs(1);

/// Wakes the UI thread from the session thread. Both calls must be cheap
/// and never block (the Win32 one is a `PostMessageW`).
pub trait Waker: Send + Sync + 'static {
    /// A picture is in the slot.
    fn frame(&self);
    /// An event is in the queue, or the session ended.
    fn event(&self);
}

/// What the session needs to start.
#[derive(Debug, Clone)]
pub struct Options {
    pub spec: SshSpec,
    /// The size the client advertises (`client_caps`).
    pub panel: (u32, u32),
    /// The first Resize, sent after HelloAck.
    pub initial: ResizeReq,
    /// The chroma modes the client offers (the machine's Sharp colour).
    pub colour: Colour,
    /// Keys: the layout, the chords and the policy.
    pub input: InputOptions,
}

/// How the session ended, from [`SessionHandle::join`].
#[derive(Debug, Clone)]
pub struct SessionEnd {
    /// `None` only if the session thread failed before the protocol ran.
    pub reason: Option<EndReason>,
    /// ssh's exit code; `None` if it was killed or never started.
    pub ssh_exit: Option<i32>,
    /// Problems outside the protocol (spawn, write, wait), in order.
    pub errors: Vec<String>,
    pub stats: SessionStats,
    /// Pictures the frame slot replaced before the UI took them, read by
    /// [`SessionHandle::join`] (the session thread leaves it at 0).
    pub slot_dropped: u64,
    pub stderr_log: Option<PathBuf>,
}

/// Whether a whole run ended cleanly: every session said Bye and its ssh
/// exited 0, and none failed to start. A run that never opened a machine
/// (the window started empty and was closed) is clean.
pub fn run_was_clean(start_failures: u32, ends: &[SessionEnd]) -> bool {
    start_failures == 0
        && ends
            .iter()
            .all(|end| matches!(end.reason, Some(EndReason::Bye)) && end.ssh_exit == Some(0))
}

/// The UI's side of a running session.
pub struct SessionHandle {
    cmds: mpsc::UnboundedSender<UiCmd>,
    events: std::sync::mpsc::Receiver<UiEvent>,
    slot: Arc<FrameSlot>,
    stats: Arc<Mutex<SessionStats>>,
    thread: Option<std::thread::JoinHandle<SessionEnd>>,
}

impl SessionHandle {
    /// Sends a command; after the session ended it is dropped.
    pub fn send(&self, cmd: UiCmd) {
        // A closed channel means the session already ended, which the
        // event queue reports.
        let _ = self.cmds.send(cmd);
    }

    /// The next queued event, without blocking.
    pub fn try_event(&self) -> Option<UiEvent> {
        self.events.try_recv().ok()
    }

    /// A sender for the hook thread, which feeds keys straight to the
    /// session.
    pub fn commands(&self) -> mpsc::UnboundedSender<UiCmd> {
        self.cmds.clone()
    }

    pub fn slot(&self) -> &Arc<FrameSlot> {
        &self.slot
    }

    /// A snapshot of the counters.
    pub fn stats(&self) -> SessionStats {
        self.stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// Waits for the thread: send [`UiCmd::Bye`] first, or this waits for
    /// the server to end the session.
    pub fn join(mut self) -> SessionEnd {
        let stats = self.stats();
        let end = match self.thread.take().map(std::thread::JoinHandle::join) {
            Some(Ok(end)) => end,
            Some(Err(_)) => SessionEnd {
                reason: None,
                ssh_exit: None,
                errors: vec!["session thread panicked".to_owned()],
                stats,
                slot_dropped: 0,
                stderr_log: None,
            },
            None => SessionEnd {
                reason: None,
                ssh_exit: None,
                errors: vec!["session already joined".to_owned()],
                stats,
                slot_dropped: 0,
                stderr_log: None,
            },
        };
        // The slot lives on this side; the thread leaves the count at 0.
        SessionEnd {
            slot_dropped: self.slot.dropped(),
            ..end
        }
    }
}

/// Starts the session thread. Dropping every clone of the handle's
/// command sender (dropping the handle) says Bye.
pub fn start(opts: Options, waker: Arc<dyn Waker>) -> std::io::Result<SessionHandle> {
    start_with_clipboard(opts, waker, None)
}

/// [`start`], with the clipboard engine run on the session's link when a
/// [`Port`] is given. Without one the server's clipboard traffic is dropped
/// and nothing is offered.
pub fn start_with_clipboard(
    opts: Options,
    waker: Arc<dyn Waker>,
    clipboard: Option<Port>,
) -> std::io::Result<SessionHandle> {
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, events) = std::sync::mpsc::channel();
    let slot = Arc::new(FrameSlot::default());
    let stats = Arc::new(Mutex::new(SessionStats::default()));
    let thread = std::thread::Builder::new()
        .name("gliff-session".to_owned())
        .spawn({
            let (slot, stats) = (slot.clone(), stats.clone());
            move || {
                let out = Out {
                    events: event_tx,
                    slot,
                    stats,
                    waker,
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(run(opts, cmd_rx, out, clipboard)),
                    Err(e) => out.fail(format!("runtime: {e}")),
                }
            }
        })?;
    Ok(SessionHandle {
        cmds,
        events,
        slot,
        stats,
        thread: Some(thread),
    })
}

/// Where the loop's results go.
struct Out {
    events: std::sync::mpsc::Sender<UiEvent>,
    slot: Arc<FrameSlot>,
    stats: Arc<Mutex<SessionStats>>,
    waker: Arc<dyn Waker>,
}

impl Out {
    fn event(&self, event: UiEvent) {
        // The UI dropped its receiver: nobody is left to tell.
        let _ = self.events.send(event);
        self.waker.event();
    }

    fn publish(&self, stats: &SessionStats) {
        *self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stats.clone();
    }

    /// A session that never reached the protocol: tell the UI it ended.
    fn fail(&self, error: String) -> SessionEnd {
        self.fail_with(EndReason::Link(error.clone()), error)
    }

    /// [`Out::fail`] with the reason the UI is told.
    fn fail_with(&self, reason: EndReason, error: String) -> SessionEnd {
        self.event(UiEvent::Ended(reason.clone()));
        SessionEnd {
            reason: Some(reason),
            ssh_exit: None,
            errors: vec![error],
            stats: SessionStats::default(),
            slot_dropped: 0,
            stderr_log: None,
        }
    }
}

/// How many server messages wait for the session at most. The reader waits
/// when the queue is full, so a server that sends faster than the session
/// consumes is slowed by the pipe instead of filling memory.
const INBOUND_QUEUE: usize = 32;

type InboundRx = mpsc::Receiver<(Instant, Result<Inbound, String>)>;

async fn run(
    opts: Options,
    mut cmds: mpsc::UnboundedReceiver<UiCmd>,
    out: Out,
    clipboard: Option<Port>,
) -> SessionEnd {
    let decoder = match CpuDecoder::new() {
        Ok(d) => d,
        Err(e) => return out.fail(format!("decoder: {e}")),
    };
    let link = match spawn(&opts.spec) {
        Ok(link) => link,
        Err(e) => {
            let error = format!("spawn ssh: {e}");
            // A program that is not there is the one start failure with a
            // plain-words answer; the rest stay as they were.
            return match from_spawn_error(&e) {
                SshFailure::ClientMissing => {
                    let failure = SshFailure::ClientMissing;
                    crate::logging::line(&log_line(&failure, &opts.spec.target, None, &error));
                    out.fail_with(EndReason::Ssh(failure), error)
                }
                _ => out.fail(error),
            };
        }
    };
    let stderr_log = link.stderr_log.clone();
    let mut child = link.child;
    let (tx, rx) = mpsc::unbounded_channel();
    let engine = clipboard.and_then(|port| {
        let writer_tx = tx.clone();
        let send: clip_link::SendToServer = Arc::new(move |msg, payload| {
            // The writer only stops on a write error, reported at the end.
            let _ = writer_tx.send((ClientMsg::from(msg), payload));
        });
        match clip_link::spawn(port, send) {
            Ok(engine) => Some(engine),
            Err(e) => {
                crate::logging::line(&format!("clipboard: engine thread: io_{:?}", e.kind()));
                None
            }
        }
    });
    let writer = tokio::spawn(write_loop(Framed::new(link.stdin), rx));
    let (in_tx, mut in_rx) = mpsc::channel(INBOUND_QUEUE);
    let reader = tokio::spawn(read_loop(Framed::new(link.stdout), in_tx));
    let mut session = Session {
        core: Core::new(opts.initial)
            .with_colour(opts.colour)
            .with_pipeline(opts.input.pipeline()),
        decoder,
        tx: Some(tx),
        engine,
        out,
        spec: opts.spec.clone(),
        stderr_log: stderr_log.clone(),
        reason: None,
        errors: Vec::new(),
        key_clock: None,
        watch: Watch::default(),
    };
    session.send(
        hello(
            client_caps(opts.panel, opts.colour),
            opts.input.keymap.clone(),
        ),
        Bytes::new(),
    );
    session.run(&mut in_rx, &mut cmds, &mut child).await;
    // Bye was sent (or the link broke): dropping the sender ends the
    // writer, which closes stdin; upstream's server exits only then.
    session.tx = None;
    // The engine holds a writer sender too: stop it first, or stdin
    // stays open and the server never exits.
    if let Some(engine) = session.engine.take() {
        let _ = tokio::task::spawn_blocking(move || engine.stop()).await;
    }
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => session.errors.push(format!("write: {e}")),
        Err(e) => session.errors.push(format!("writer: {e}")),
    }
    let ssh_exit = finish(&mut child, &mut in_rx, &mut session.errors).await;
    reader.abort();
    let stats = session.core.stats().clone();
    session.out.publish(&stats);
    // The UI learns of the end only after ssh is gone, so it may exit.
    session.out.waker.event();
    SessionEnd {
        reason: session.reason,
        ssh_exit,
        errors: session.errors,
        stats,
        slot_dropped: 0,
        stderr_log: Some(stderr_log),
    }
}

/// Drains the server's output to EOF, so a server still writing is never
/// blocked, then waits for ssh; kills it after [`EXIT_WAIT`].
async fn finish(
    child: &mut Child,
    inbound: &mut InboundRx,
    errors: &mut Vec<String>,
) -> Option<i32> {
    let drain = async {
        while inbound.recv().await.is_some() {}
        child.wait().await
    };
    match tokio::time::timeout(EXIT_WAIT, drain).await {
        Ok(Ok(status)) => status.code(),
        Ok(Err(e)) => {
            errors.push(format!("wait: {e}"));
            None
        }
        Err(_) => {
            errors.push(format!(
                "ssh still running {}s after Bye and stdin EOF",
                EXIT_WAIT.as_secs()
            ));
            // tokio's kill also waits, so ssh has exited and its log is whole.
            let _ = child.kill().await;
            None
        }
    }
}

async fn write_loop(
    mut writer: Framed<tokio::process::ChildStdin>,
    mut rx: mpsc::UnboundedReceiver<(ClientMsg, Bytes)>,
) -> Result<(), gliff_transport::Error> {
    while let Some((msg, payload)) = rx.recv().await {
        writer.write_msg_with_payloads(&msg, &[&payload]).await?;
    }
    Ok(())
}

/// `read_server` is not cancel-safe, so it runs whole on its own task.
async fn read_loop(
    mut reader: Framed<tokio::process::ChildStdout>,
    tx: mpsc::Sender<(Instant, Result<Inbound, String>)>,
) {
    loop {
        let item = read_server(&mut reader).await;
        let end = item.is_err();
        let item = item.map_err(|e| match e {
            gliff_transport::Error::Closed => "closed".to_owned(),
            e => e.to_string(),
        });
        if tx.send((Instant::now(), item)).await.is_err() || end {
            break;
        }
    }
}

struct Session {
    core: Core,
    decoder: CpuDecoder,
    tx: Option<mpsc::UnboundedSender<(ClientMsg, Bytes)>>,
    /// The clipboard engine, when the clipboard is on.
    engine: Option<EngineThread>,
    out: Out,
    /// What ssh was asked to do, and where its stderr went, to say why a
    /// link that never came up did not.
    spec: SshSpec,
    stderr_log: PathBuf,
    reason: Option<EndReason>,
    errors: Vec<String>,
    /// The newest key's clocks, so the AltGr deadline can be checked in the
    /// hook's clock without reading it.
    key_clock: Option<KeyClock>,
    /// Input the server has not answered with a picture, for resyncing a
    /// server that went quiet.
    watch: Watch,
}

/// The hook time of a key and when it arrived here.
#[derive(Clone, Copy, Debug)]
struct KeyClock {
    time_ms: u32,
    arrived: Instant,
}

impl KeyClock {
    /// When the AltGr filter gives up on the key. It is fixed by the key's
    /// arrival, so waking for something else does not move it.
    fn deadline(self) -> Instant {
        self.arrived + KEY_HOLD
    }

    /// The hook's clock at `now`.
    fn hook_ms(self, now: Instant) -> u32 {
        let elapsed = now.saturating_duration_since(self.arrived);
        self.time_ms.wrapping_add(elapsed.as_millis() as u32)
    }
}

impl Session {
    async fn run(
        &mut self,
        inbound: &mut InboundRx,
        cmds: &mut mpsc::UnboundedReceiver<UiCmd>,
        ssh: &mut Child,
    ) {
        let mut last_frame = Instant::now();
        while !self.core.is_ended() {
            // Whatever woke the loop, a deadline that has passed is served
            // first: a flood of mouse motion must not push it back.
            self.tick_if_due();
            self.resync_if_due();
            let key_due = self.key_due();
            let resync_due = self.watch.due();
            let idle = self.decoder.has_pending();
            tokio::select! {
                item = inbound.recv() => match item {
                    Some((at, Ok(inbound))) => {
                        match inbound.msg {
                            ServerMsg::VideoFrame { .. } => {
                                last_frame = at;
                                self.watch.answered();
                            }
                            ServerMsg::StreamConfig { .. } => self.watch.answered(),
                            _ => {}
                        }
                        let actions = self.core.on_server(inbound.msg, inbound.main, inbound.aux);
                        self.perform(actions);
                    }
                    Some((_, Err(e))) => self.link_lost(ssh, format!("read: {e}")).await,
                    None => self.link_lost(ssh, "reader ended".to_owned()).await,
                },
                cmd = cmds.recv() => {
                    // The handle was dropped: the UI is gone, so say Bye.
                    let cmd = cmd.unwrap_or(UiCmd::Bye);
                    for cmd in coalesce_motion(cmd, cmds) {
                        self.on_cmd(cmd);
                    }
                }
                () = tokio::time::sleep_until(key_due.unwrap_or_else(Instant::now).into()), if key_due.is_some() => {
                    self.tick_if_due();
                }
                () = tokio::time::sleep_until(resync_due.unwrap_or_else(Instant::now).into()), if resync_due.is_some() => {
                    self.resync_if_due();
                }
                () = tokio::time::sleep_until((last_frame + IDLE_DRAIN).into()), if idle => {
                    let result = self.decoder.flush();
                    let actions = self.core.drained(result);
                    self.perform(actions);
                    last_frame = Instant::now();
                }
            }
            self.out.publish(self.core.stats());
        }
    }

    fn on_cmd(&mut self, cmd: UiCmd) {
        if let UiCmd::Key(key) = &cmd {
            self.key_clock = Some(KeyClock {
                time_ms: key.time_ms,
                arrived: Instant::now(),
            });
        }
        if is_stimulus(&cmd) && self.core.stats().configs > 0 {
            self.watch.input(Instant::now());
        }
        let actions = self.core.on_ui(cmd);
        self.perform(actions);
    }

    /// Resyncs when the watch says unanswered input has waited long enough,
    /// and logs the counters that tell a dead server from a slow one.
    fn resync_if_due(&mut self) {
        let unanswered = self.watch.unanswered();
        if !self.watch.take(Instant::now()) {
            return;
        }
        let actions = self.core.resync();
        let stats = self.core.stats();
        let what = if actions.is_empty() {
            "no resync on a mirrored screen"
        } else {
            "resync"
        };
        crate::logging::line(&format!(
            "{what}: {unanswered} inputs unanswered; frames {}, presented {}, acks {}, configs {}",
            stats.frames, stats.presented, stats.acks, stats.configs
        ));
        if !actions.is_empty() {
            crate::logging::line(&format!(
                "resync: told the remote Win, Ctrl, Alt and Shift are up, except the {} key(s) \
                 the user holds",
                stats.keys_down
            ));
        }
        self.perform(actions);
    }

    /// When the AltGr filter's deadline falls, while it holds a key.
    fn key_due(&self) -> Option<Instant> {
        self.key_clock
            .filter(|_| self.core.input_holding())
            .map(KeyClock::deadline)
    }

    fn tick_if_due(&mut self) {
        let now = Instant::now();
        if let (Some(due), Some(clock)) = (self.key_due(), self.key_clock)
            && now >= due
        {
            self.on_cmd(UiCmd::Tick(clock.hook_ms(now)));
        }
    }

    /// The server's side closed or broke without an Error: end the Core
    /// with Bye (nothing more is sent) and report the link.
    async fn link_lost(&mut self, ssh: &mut Child, error: String) {
        self.tx = None;
        let _ = self.core.on_ui(UiCmd::Bye);
        let reason = self.explain_link(ssh, error).await;
        self.reason = Some(reason.clone());
        self.out.event(UiEvent::Ended(reason));
    }

    /// Why the link is gone. A link that never reached StreamConfig, whose
    /// ssh has exited and said something, gets ssh's own reason in plain
    /// words ([`classify`](crate::connect::classify)), and the log gets the
    /// whole message. Otherwise the reason is what the loop saw: ssh is
    /// still running (it is not the cause), the session had been working (a
    /// failure now is a lost connection, not a failed one to connect), or
    /// ssh printed nothing to add.
    async fn explain_link(&mut self, ssh: &mut Child, error: String) -> EndReason {
        if self.core.stats().configs > 0 {
            return EndReason::Link(error);
        }
        // ssh closes its stdout as it exits; give it a moment to be gone.
        let exit = match tokio::time::timeout(LINK_EXIT_WAIT, ssh.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => return EndReason::Link(error),
        };
        match diagnose(&self.stderr_log, exit) {
            SshFailure::Other { last_line } if last_line.is_empty() => EndReason::Link(error),
            failure => {
                crate::logging::line(&log_line(
                    &failure,
                    &self.spec.target,
                    exit,
                    &self.spec.log_name(),
                ));
                EndReason::Ssh(failure)
            }
        }
    }

    fn send(&self, msg: ClientMsg, payload: Bytes) {
        if let Some(tx) = &self.tx {
            // The writer only stops on a write error, reported at the end.
            let _ = tx.send((msg, payload));
        }
    }

    fn perform(&mut self, actions: Vec<Action>) {
        let mut queue: VecDeque<Action> = actions.into();
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Send(msg, payload) => self.send(msg, payload),
                Action::Clipboard(msg, payload) => {
                    if let Some(engine) = &self.engine {
                        engine.peer(msg, payload);
                    }
                }
                Action::Decode {
                    frame_id,
                    main,
                    aux,
                } => {
                    let t = Instant::now();
                    let result = self.decoder.decode(&main, &aux);
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    queue.extend(self.core.decoded(Decoded {
                        frame_id,
                        result,
                        decode_ms: ms as f32,
                        now_ms: now_ms(),
                    }));
                }
                Action::ResetDecoder(chroma) => {
                    if let Err(e) = self.decoder.reset(chroma) {
                        self.errors.push(format!("decoder reset: {e}"));
                    }
                }
                Action::Present(picture) => {
                    let _ = self.out.slot.put(picture);
                    self.out.waker.frame();
                }
                Action::Ui(event) => {
                    if let UiEvent::Ended(reason) = &event {
                        self.reason.get_or_insert_with(|| reason.clone());
                    }
                    self.out.event(event);
                }
            }
        }
    }
}

/// Whether the user did something a working server answers with a picture:
/// a press (not a release or a motion) or a wheel step.
fn is_stimulus(cmd: &UiCmd) -> bool {
    match cmd {
        UiCmd::Key(key) => key.press == Press::Down,
        UiCmd::Keys(keys) => keys.iter().any(|k| k.press == Press::Down),
        UiCmd::Pointer(PointerEvent::Button { press, .. }) => *press == Press::Down,
        UiCmd::Wheel { .. } => true,
        _ => false,
    }
}

/// `first` and what is already queued behind it, with each run of pointer
/// motions reduced to its last (the server only needs where the pointer is).
/// Everything else keeps its order.
fn coalesce_motion(first: UiCmd, rx: &mut mpsc::UnboundedReceiver<UiCmd>) -> Vec<UiCmd> {
    let is_motion = |cmd: &UiCmd| matches!(cmd, UiCmd::Pointer(PointerEvent::Motion { .. }));
    let mut out = Vec::new();
    let mut current = first;
    for _ in 0..COALESCE_LIMIT {
        let Ok(next) = rx.try_recv() else {
            break;
        };
        if !(is_motion(&current) && is_motion(&next)) {
            out.push(current);
        }
        current = next;
    }
    out.push(current);
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
