//! `gwin-verify stream`: a headless session through [`Core`], measured.
//!
//! The session is driven exactly as the client will drive it: `Core` takes
//! every server message, decode result and command, and this module
//! performs the actions it returns (write, decode, reset, present). On top
//! it watches the wire: when the Resize took effect, frame arrival gaps,
//! frame id holes, the SPS, and the last picture, which must be the test
//! pattern. A second ssh asks hyprctl where the pattern window is, at the
//! start and end of the window, so a pattern shown on the wrong monitor
//! fails as `placement`, not as a vague pattern miss.

pub mod measure;

use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use gliff_proto::msg::ClientMsg;
use gliff_proto::msg::ServerMsg;
use gliff_transport::Framed;
use gliff_windows::decode::CpuDecoder;
use gliff_windows::frame_slot::FrameSlot;
use gliff_windows::h264::LevelVerdict;
use gliff_windows::h264::OPENH264_MAX_LEVEL_IDC;
use gliff_windows::h264::SpsInfo;
use gliff_windows::h264::find_sps;
use gliff_windows::link::Inbound;
use gliff_windows::link::read_server;
use gliff_windows::session::Action;
use gliff_windows::session::Core;
use gliff_windows::session::Decoded;
use gliff_windows::session::EndReason;
use gliff_windows::session::Picture;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::UiEvent;
use gliff_windows::session::client_caps;
use gliff_windows::session::hello;
use gliff_windows::ssh::SshSpec;
use gliff_windows::ssh::diagnose;
use gliff_windows::ssh::exec;
use gliff_windows::ssh::spawn;
use serde_json::Value;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::line::Line;
use crate::line::round2;
use crate::pattern;
use crate::pattern::Verdict;
use crate::placement;
use crate::placement::PLACEMENT_COMMAND;
use crate::png_out::write_png;
use crate::stream_check::measure::FpsProblem;
use crate::stream_check::measure::IdGaps;
use crate::stream_check::measure::ResizeVerdict;

/// Upstream's idle drain fires after 150 ms without a frame
/// (net.rs:525-561).
const IDLE_DRAIN: Duration = Duration::from_millis(150);
/// How long the server gets to exit once stdin is closed after Bye.
const EXIT_WAIT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the session stays up (still acking) for the end probe; past
/// its own timeout, so `exec` always answers first.
const END_PROBE_WAIT: Duration = Duration::from_secs(20);

/// Whether to ask hyprctl where the pattern window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PlacementProbe {
    /// A second ssh runs `hyprctl` at the window's start and end.
    Ssh,
    /// No probe (a host without Hyprland); the line says `"skipped"`.
    Skip,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub spec: SshSpec,
    pub size: (u32, u32),
    pub window: Duration,
    /// The floor for the server's fps cap.
    pub min_fps: f64,
    /// The share of the cap the decoded fps must reach.
    pub fps_of_cap: f64,
    pub stall: Duration,
    pub settle: Duration,
    pub resize_timeout: Duration,
    pub png: PathBuf,
    pub tolerance: f64,
    pub placement: PlacementProbe,
    /// Present (take from the slot) at this period instead of every
    /// picture, so the slot drops; acks must still equal frames.
    pub present_every: Option<Duration>,
}

/// Why the check failed; `code` is what tests and the report key on.
#[derive(Debug, Clone, PartialEq)]
enum Failure {
    ResizeIgnored {
        implied_output: (u32, u32),
    },
    /// A StreamConfig after the Resize took effect implied another output.
    ResizeReverted {
        implied_output: (u32, u32),
    },
    EncoderClamped {
        stream: (u32, u32),
    },
    /// Decoded fps under `FPS_OF_CAP` of the server's cap.
    Fps {
        fps: f64,
        expected: f64,
        fps_of_cap: f64,
    },
    /// The server capped the stream below `--min-fps`.
    FpsCap {
        cap: u32,
        floor: f64,
    },
    Stall {
        max_gap_ms: f64,
    },
    Acks {
        frames: u64,
        acks: u64,
        written: u64,
    },
    DecodeErrors(u64),
    FramesMissing(u64),
    LevelRejected {
        level: String,
    },
    NoSps,
    Placement(String),
    Pattern(String),
    Png(String),
    Exit(String),
    Ended(String),
    Link(String),
}

impl Failure {
    fn code(&self) -> &'static str {
        match self {
            Self::ResizeIgnored { .. } => "resize_ignored",
            Self::ResizeReverted { .. } => "resize_reverted",
            Self::EncoderClamped { .. } => "encoder_clamped",
            Self::Fps { .. } => "fps",
            Self::FpsCap { .. } => "fps_cap",
            Self::Stall { .. } => "stall",
            Self::Acks { .. } => "acks",
            Self::DecodeErrors(_) => "decode_errors",
            Self::FramesMissing(_) => "frames_missing",
            Self::LevelRejected { .. } => "level_rejected",
            Self::NoSps => "no_sps",
            Self::Placement(_) => "placement",
            Self::Pattern(_) => "pattern",
            Self::Png(_) => "png",
            Self::Exit(_) => "exit",
            Self::Ended(_) => "ended",
            Self::Link(_) => "link",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::ResizeIgnored { implied_output } => format!(
                "no StreamConfig for the requested size; last implied output {}x{}",
                implied_output.0, implied_output.1
            ),
            Self::ResizeReverted { implied_output } => format!(
                "a later StreamConfig implied a {}x{} output",
                implied_output.0, implied_output.1
            ),
            Self::EncoderClamped { stream } => format!(
                "output resized, but the encoder codes {}x{} (its maximum); the server \
                 is not on the Vulkan encoder",
                stream.0, stream.1
            ),
            Self::Fps {
                fps,
                expected,
                fps_of_cap,
            } => format!(
                "{fps:.2} fps decoded; the server's fps cap over the window allows {expected:.2} \
                 ({:.0}% of it)",
                fps_of_cap * 100.0
            ),
            Self::FpsCap { cap, floor } => format!(
                "the server capped the stream at {cap} fps, below the {floor:.0} fps floor \
                 (its rate ladder stepped down)"
            ),
            Self::Stall { max_gap_ms } => format!("no frame for {max_gap_ms:.0} ms"),
            Self::Acks {
                frames,
                acks,
                written,
            } => format!("{frames} frames, {acks} acks, {written} written"),
            Self::DecodeErrors(n) => format!("{n} decode errors"),
            Self::FramesMissing(n) => format!("{n} frame ids never arrived"),
            Self::LevelRejected { level } => format!(
                "SPS level {level} is above {}.{}: the OpenH264 decoder rejects it",
                OPENH264_MAX_LEVEL_IDC / 10,
                OPENH264_MAX_LEVEL_IDC % 10
            ),
            Self::NoSps => "no SPS after the Resize".to_owned(),
            Self::Placement(s)
            | Self::Pattern(s)
            | Self::Png(s)
            | Self::Exit(s)
            | Self::Ended(s)
            | Self::Link(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Until a StreamConfig implies the requested output.
    WaitResize,
    /// Letting the rate ladder and the encoder settle.
    Settle {
        until: Instant,
    },
    Measure {
        start: Instant,
    },
    /// The window is over; the session stays up, acking, until the end
    /// placement probe answers, because Bye removes the output
    /// (session.rs:498, :1477-1482).
    Closing {
        until: Instant,
    },
    Done,
}

/// Everything measured in the window.
#[derive(Default)]
struct Window {
    arrivals: Vec<Duration>,
    frames: u64,
    pictures: u64,
    decode_ms: Vec<f64>,
    ids: IdGaps,
    /// (offset from the window start, fps cap) of each StreamConfig in
    /// the window.
    fps_caps: Vec<(Duration, u32)>,
    /// The fps cap in force when the window started.
    cap_at_start: u32,
    /// Decoder resets (StreamConfigs with a new coded size) before the
    /// window started.
    resets_at_start: u64,
}

type InboundRx = mpsc::UnboundedReceiver<(Instant, Result<Inbound, String>)>;

struct Run {
    opts: Options,
    core: Core,
    decoder: CpuDecoder,
    phase: Phase,
    connected_at: Option<Instant>,
    output: Option<String>,
    headless: Option<bool>,
    /// (stream w, stream h, scale_milli, fps_cap) of the last StreamConfig.
    last_config: Option<(u32, u32, u32, u32)>,
    verdict: Option<ResizeVerdict>,
    sps: Option<SpsInfo>,
    window: Window,
    /// The measurement window started.
    measured: bool,
    last_picture: Arc<Mutex<Option<Picture>>>,
    slot: Option<Arc<FrameSlot>>,
    tx: Option<mpsc::UnboundedSender<ClientMsg>>,
    probes: Vec<JoinHandle<Result<String, String>>>,
    end_probe: Option<JoinHandle<Result<String, String>>>,
    end_result: Option<Result<String, String>>,
    failures: Vec<Failure>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub async fn run(opts: Options) -> Line {
    let budget = opts.resize_timeout + opts.settle + opts.window + Duration::from_secs(45);
    let base = Line::new("stream", false)
        .with("target", opts.spec.target.to_string())
        .with("size", format!("{}x{}", opts.size.0, opts.size.1))
        .with("seconds", opts.window.as_secs_f64());
    let decoder = match CpuDecoder::new() {
        Ok(d) => d,
        Err(e) => return base.with("error", format!("decoder: {e}")),
    };
    let link = match spawn(&opts.spec) {
        Ok(link) => link,
        Err(e) => return base.with("error", format!("spawn: {e}")),
    };
    let stderr_log = link.stderr_log.clone();
    let mut child = link.child;
    let acks_written = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::unbounded_channel();
    let writer = tokio::spawn(write_loop(
        Framed::new(link.stdin),
        rx,
        acks_written.clone(),
    ));
    let (in_tx, mut in_rx) = mpsc::unbounded_channel();
    let reader = tokio::spawn(read_loop(Framed::new(link.stdout), in_tx));
    let stop = Arc::new(AtomicBool::new(false));
    let last_picture = Arc::new(Mutex::new(None));
    let (slot, consumer) = match opts.present_every {
        Some(every) => {
            let slot = Arc::new(FrameSlot::default());
            let handle = std::thread::spawn({
                let (slot, stop, last) = (slot.clone(), stop.clone(), last_picture.clone());
                move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(every);
                        if let Some(p) = slot.take() {
                            *last.lock().expect("picture lock") = Some(p);
                        }
                    }
                }
            });
            (Some(slot), Some(handle))
        }
        None => (None, None),
    };
    let started = Instant::now();
    let mut run = Run {
        core: Core::new(ResizeReq {
            width: opts.size.0,
            height: opts.size.1,
            scale: 1.0,
        }),
        opts,
        decoder,
        phase: Phase::WaitResize,
        connected_at: None,
        output: None,
        headless: None,
        last_config: None,
        verdict: None,
        sps: None,
        window: Window::default(),
        measured: false,
        last_picture,
        slot,
        tx: Some(tx),
        probes: Vec::new(),
        end_probe: None,
        end_result: None,
        failures: Vec::new(),
    };
    let timed_out = tokio::time::timeout(budget, run.session(&mut in_rx))
        .await
        .is_err();
    // Bye was sent (or the session broke): dropping the sender ends the
    // writer, which closes stdin.
    run.tx = None;
    stop.store(true, Ordering::Relaxed);
    if let Some(handle) = consumer {
        // The consumer only sleeps and takes; a panic there is a bug in
        // this file, and the picture it held is simply missing.
        let _ = handle.join();
    }
    let written = match writer.await {
        Ok(Ok(())) => acks_written.load(Ordering::Relaxed),
        Ok(Err(e)) => {
            run.failures.push(Failure::Link(format!("write: {e}")));
            acks_written.load(Ordering::Relaxed)
        }
        Err(e) => {
            run.failures.push(Failure::Link(format!("writer: {e}")));
            acks_written.load(Ordering::Relaxed)
        }
    };
    let mut exit_code = None;
    let exit = if timed_out {
        run.failures.push(Failure::Link(format!(
            "timed out after {}s",
            budget.as_secs()
        )));
        // tokio's kill also waits, so ssh has exited and its log is whole.
        let _ = child.kill().await;
        None
    } else {
        // Drain to EOF so a server still writing is never blocked, then
        // wait for the exit.
        let drain = async {
            while in_rx.recv().await.is_some() {}
            child.wait().await
        };
        match tokio::time::timeout(EXIT_WAIT, drain).await {
            Ok(Ok(status)) => {
                exit_code = status.code();
                Some(status.success())
            }
            Ok(Err(e)) => {
                run.failures.push(Failure::Exit(format!("wait: {e}")));
                None
            }
            Err(_) => {
                run.failures.push(Failure::Exit(format!(
                    "still running {}s after Bye and stdin EOF",
                    EXIT_WAIT.as_secs()
                )));
                let _ = child.kill().await;
                None
            }
        }
    };
    reader.abort();
    if exit == Some(false) {
        run.failures
            .push(Failure::Exit("exited non-zero after Bye".to_owned()));
    }
    let probes = run.collect_probes().await;
    run.report(
        base,
        written,
        (exit, exit_code),
        probes,
        started.elapsed(),
        &stderr_log,
    )
}

async fn write_loop(
    mut writer: Framed<tokio::process::ChildStdin>,
    mut rx: mpsc::UnboundedReceiver<ClientMsg>,
    acks: Arc<AtomicU64>,
) -> Result<(), gliff_transport::Error> {
    while let Some(msg) = rx.recv().await {
        writer.write_msg(&msg).await?;
        if let ClientMsg::FrameAck { .. } = msg {
            acks.fetch_add(1, Ordering::Relaxed);
        }
    }
    // Dropping `writer` closes stdin: ssh forwards EOF, and upstream's
    // server exits only then (see ssh_check).
    Ok(())
}

/// `read_server` is not cancel-safe, so it runs whole on its own task and
/// hands over each message with its arrival time.
async fn read_loop(
    mut reader: Framed<tokio::process::ChildStdout>,
    tx: mpsc::UnboundedSender<(Instant, Result<Inbound, String>)>,
) {
    loop {
        let item = read_server(&mut reader).await;
        let end = item.is_err();
        let item = item.map_err(|e| match e {
            gliff_transport::Error::Closed => "closed".to_owned(),
            e => e.to_string(),
        });
        if tx.send((Instant::now(), item)).is_err() || end {
            break;
        }
    }
}

impl Run {
    async fn session(&mut self, inbound: &mut InboundRx) {
        self.send(hello(client_caps(self.opts.size), String::new()));
        let mut last_frame = Instant::now();
        loop {
            if self.phase == Phase::Done || self.core.is_ended() {
                return;
            }
            let idle = self.decoder.has_pending();
            let deadline = self.deadline();
            let mut end = self.end_probe.take();
            let mut end_result = None;
            tokio::select! {
                item = inbound.recv() => match item {
                    Some((at, Ok(inbound))) => {
                        if let ServerMsg::VideoFrame { .. } = inbound.msg {
                            last_frame = at;
                        }
                        self.on_inbound(at, inbound);
                    }
                    Some((_, Err(e))) => {
                        self.failures.push(Failure::Link(format!("read: {e}")));
                        return;
                    }
                    None => {
                        self.failures.push(Failure::Link("reader ended".to_owned()));
                        return;
                    }
                },
                () = tokio::time::sleep_until((last_frame + IDLE_DRAIN).into()), if idle => {
                    let result = self.decoder.flush();
                    let actions = self.core.drained(result);
                    self.perform(actions);
                    last_frame = Instant::now();
                }
                () = tokio::time::sleep_until(deadline.into()) => self.on_deadline(),
                r = async { end.as_mut().expect("guarded by is_some").await }, if end.is_some() => {
                    end_result = Some(r);
                }
            }
            match end_result {
                Some(r) => {
                    self.end_result = Some(r.unwrap_or_else(|e| Err(format!("probe task: {e}"))));
                    self.bye();
                }
                None => {
                    if end.is_some() {
                        self.end_probe = end;
                    }
                }
            }
        }
    }

    fn deadline(&self) -> Instant {
        let far = Instant::now() + Duration::from_secs(3600);
        match self.phase {
            Phase::WaitResize => self
                .connected_at
                .map_or(far, |at| at + self.opts.resize_timeout),
            Phase::Settle { until } => until,
            Phase::Measure { start } => start + self.opts.window,
            Phase::Closing { until } => until,
            Phase::Done => far,
        }
    }

    fn on_deadline(&mut self) {
        match self.phase {
            Phase::WaitResize => {
                let implied_output = self
                    .last_config
                    .map_or((0, 0), |(w, h, m, _)| measure::implied_output((w, h), m));
                self.failures
                    .push(Failure::ResizeIgnored { implied_output });
                self.bye();
            }
            Phase::Settle { .. } => {
                self.phase = Phase::Measure {
                    start: Instant::now(),
                };
                self.measured = true;
                self.window.cap_at_start = self.last_config.map_or(0, |(_, _, _, cap)| cap);
                self.window.resets_at_start = self.core.stats().resets;
                if let Some(probe) = self.probe() {
                    self.probes.push(probe);
                }
            }
            Phase::Measure { .. } => match self.probe() {
                Some(probe) => {
                    self.end_probe = Some(probe);
                    self.phase = Phase::Closing {
                        until: Instant::now() + END_PROBE_WAIT,
                    };
                }
                None => self.bye(),
            },
            // The end probe never answered; collect_probes reports it.
            Phase::Closing { .. } => self.bye(),
            Phase::Done => {}
        }
    }

    fn bye(&mut self) {
        let actions = self.core.on_ui(UiCmd::Bye);
        self.perform(actions);
        self.phase = Phase::Done;
    }

    fn probe(&self) -> Option<JoinHandle<Result<String, String>>> {
        if self.opts.placement == PlacementProbe::Skip {
            return None;
        }
        let spec = self.opts.spec.clone();
        let output = self.output.clone().unwrap_or_default();
        Some(tokio::spawn(async move {
            match exec(&spec, PLACEMENT_COMMAND, PROBE_TIMEOUT).await {
                Ok(out) => {
                    let placement = placement::judge(&out.stdout, &output);
                    if placement.is_ok() {
                        Ok(placement.describe())
                    } else {
                        Err(placement.describe())
                    }
                }
                Err(e) => Err(format!("hyprctl over ssh: {e}")),
            }
        }))
    }

    async fn collect_probes(&mut self) -> Vec<Result<String, String>> {
        let mut results = Vec::new();
        for probe in self.probes.drain(..) {
            results.push(match probe.await {
                Ok(r) => r,
                Err(e) => Err(format!("probe task: {e}")),
            });
        }
        if let Some(r) = self.end_result.take() {
            results.push(r);
        }
        if let Some(probe) = self.end_probe.take() {
            // Still running at Bye: whatever it sees now is after the
            // output was removed, so it proves nothing.
            probe.abort();
            results.push(Err("end probe did not answer before Bye".to_owned()));
        }
        results
    }

    fn send(&self, msg: ClientMsg) {
        if let Some(tx) = &self.tx {
            // The writer only stops on a write error, reported at the end.
            let _ = tx.send(msg);
        }
    }

    fn on_inbound(&mut self, at: Instant, inbound: Inbound) {
        if let ServerMsg::VideoFrame {
            frame_id, keyframe, ..
        } = inbound.msg
        {
            // Before the Resize the stream is the clamped first mode; the
            // SPS that counts is the one after it.
            if keyframe
                && let Some(Ok(sps)) = find_sps(&inbound.main)
                && (self.sps.is_none() || self.phase != Phase::WaitResize)
            {
                self.sps = Some(sps);
            }
            // `at` is stamped by the reader before the channel, so a frame
            // queued during a decode can predate the window's start; it
            // belongs to the settle phase.
            if let Phase::Measure { start } = self.phase
                && let Some(offset) = at.checked_duration_since(start)
            {
                self.window.arrivals.push(offset);
                self.window.frames += 1;
                self.window.ids.see(frame_id);
            }
        }
        let actions = self.core.on_server(inbound.msg, inbound.main, inbound.aux);
        self.perform(actions);
    }

    fn perform(&mut self, actions: Vec<Action>) {
        let mut queue: VecDeque<Action> = actions.into();
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Send(msg, _) => self.send(msg),
                Action::Decode { frame_id, main } => {
                    let t = Instant::now();
                    let result = self.decoder.decode(&main);
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    if let Phase::Measure { .. } = self.phase {
                        self.window.decode_ms.push(ms);
                    }
                    queue.extend(self.core.decoded(Decoded {
                        frame_id,
                        result,
                        decode_ms: ms as f32,
                        now_ms: now_ms(),
                    }));
                }
                Action::ResetDecoder => {
                    if let Err(e) = self.decoder.reset() {
                        self.failures
                            .push(Failure::Link(format!("decoder reset: {e}")));
                    }
                }
                Action::Present(picture) => self.present(picture),
                Action::Ui(event) => self.on_ui_event(event),
                Action::Clipboard(..) => {}
            }
        }
    }

    fn present(&mut self, picture: Picture) {
        if let Phase::Measure { .. } = self.phase {
            self.window.pictures += 1;
        }
        match &self.slot {
            Some(slot) => {
                slot.put(picture);
            }
            None => *self.last_picture.lock().expect("picture lock") = Some(picture),
        }
    }

    fn on_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Connected(session) => {
                self.connected_at = Some(Instant::now());
                self.headless = Some(session.headless);
                self.output = Some(session.output);
            }
            UiEvent::StreamConfig(info) => {
                let g = info.geometry;
                self.last_config = Some((g.stream.0, g.stream.1, g.scale_milli, info.fps_cap));
                if let Phase::Measure { start } = self.phase {
                    self.window.fps_caps.push((
                        Instant::now().saturating_duration_since(start),
                        info.fps_cap,
                    ));
                }
                let verdict = measure::judge_config(self.opts.size, g.stream, g.scale_milli);
                let took = !matches!(verdict, ResizeVerdict::Other { .. });
                let reverted = self.verdict.is_some() && !took;
                if reverted
                    && !self
                        .failures
                        .iter()
                        .any(|f| matches!(f, Failure::ResizeReverted { .. }))
                {
                    self.failures.push(Failure::ResizeReverted {
                        implied_output: measure::implied_output(g.stream, g.scale_milli),
                    });
                }
                if self.phase == Phase::WaitResize && took {
                    self.verdict = Some(verdict);
                    self.sps = None;
                    self.phase = Phase::Settle {
                        until: Instant::now() + self.opts.settle,
                    };
                }
            }
            UiEvent::Cursor(_)
            | UiEvent::GrabReleased
            | UiEvent::Resyncing
            | UiEvent::Resynced
            | UiEvent::Ended(EndReason::Bye) => {}
            UiEvent::Ended(reason) => {
                self.failures.push(Failure::Ended(format!("{reason:?}")));
                self.phase = Phase::Done;
            }
        }
    }

    fn report(
        mut self,
        base: Line,
        written: u64,
        (exit, exit_code): (Option<bool>, Option<i32>),
        probes: Vec<Result<String, String>>,
        elapsed: Duration,
        stderr_log: &Path,
    ) -> Line {
        let stats = self.core.stats().clone();
        let window = std::mem::take(&mut self.window);
        let fps = window.pictures as f64 / self.opts.window.as_secs_f64();
        let mut decode = window.decode_ms.clone();
        let p50 = measure::quantile(&mut decode, 0.5);
        let p95 = measure::quantile(&mut decode, 0.95);
        let max = decode.last().copied().unwrap_or(0.0);
        let max_gap = measure::max_gap(&window.arrivals, self.opts.window);
        let stalls = measure::stalls(&window.arrivals, self.opts.window, self.opts.stall);
        let measured = self.measured;

        if let Some(ResizeVerdict::Clamped { stream }) = self.verdict {
            self.failures.push(Failure::EncoderClamped { stream });
        }
        if measured {
            for problem in measure::judge_fps(
                fps,
                window.cap_at_start,
                &window.fps_caps,
                self.opts.window,
                self.opts.min_fps,
                self.opts.fps_of_cap,
            ) {
                self.failures.push(match problem {
                    FpsProblem::CapBelowFloor { cap } => Failure::FpsCap {
                        cap,
                        floor: self.opts.min_fps,
                    },
                    FpsProblem::BelowCap { fps, expected } => Failure::Fps {
                        fps,
                        expected,
                        fps_of_cap: self.opts.fps_of_cap,
                    },
                });
            }
            if stalls > 0 {
                self.failures.push(Failure::Stall {
                    max_gap_ms: max_gap.as_secs_f64() * 1000.0,
                });
            }
            if window.ids.missing > 0 {
                self.failures
                    .push(Failure::FramesMissing(window.ids.missing));
            }
        }
        if stats.acks != stats.frames || written != stats.acks {
            self.failures.push(Failure::Acks {
                frames: stats.frames,
                acks: stats.acks,
                written,
            });
        }
        if stats.decode_errors > 0 {
            self.failures
                .push(Failure::DecodeErrors(stats.decode_errors));
        }
        match (&self.sps, measured) {
            (Some(sps), _) if sps.decoder_accepts() == LevelVerdict::Rejected => {
                self.failures.push(Failure::LevelRejected {
                    level: sps.level_name(),
                });
            }
            (None, true) => self.failures.push(Failure::NoSps),
            _ => {}
        }
        for why in probes.iter().filter_map(|p| p.as_ref().err()) {
            self.failures.push(Failure::Placement(why.clone()));
        }
        let picture = self.last_picture.lock().expect("picture lock").take();
        let pattern_json = match (&picture, measured) {
            (Some(p), _) => {
                // Upstream's CPU conversion: the same pixels the pattern check scores.
                let bgra = p.frame.to_bgra();
                let score = pattern::compare(&bgra, self.opts.tolerance);
                let png = match write_png(&self.opts.png, &bgra) {
                    Ok(()) => Value::from(self.opts.png.display().to_string()),
                    Err(e) => {
                        self.failures.push(Failure::Png(format!(
                            "{} not written: {e:#}",
                            self.opts.png.display()
                        )));
                        Value::Null
                    }
                };
                if score.verdict() == Verdict::Mismatch {
                    self.failures.push(Failure::Pattern(format!(
                        "picture is not the test pattern: mae_mean {:.1}, {}/32 cells \
                         within {}",
                        score.mae_mean, score.cells_matched, score.tolerance
                    )));
                }
                json!({
                    "mae_mean": round2(score.mae_mean),
                    "mae_max": round2(score.mae_max),
                    "worst_cell": score.worst_cell,
                    "cells_matched": score.cells_matched,
                    "tolerance": score.tolerance,
                    "frame_id": p.frame_id,
                    "size": format!("{}x{}", p.frame.width(), p.frame.height()),
                    "png": png,
                })
            }
            (None, true) => {
                self.failures
                    .push(Failure::Pattern("no picture was decoded".to_owned()));
                Value::Null
            }
            (None, false) => Value::Null,
        };
        let (w, h, m, cap) = self.last_config.unwrap_or_default();
        let implied = measure::implied_output((w, h), m);
        let resize = match self.verdict {
            Some(ResizeVerdict::Applied) => "applied",
            Some(ResizeVerdict::Clamped { .. }) => "clamped",
            Some(ResizeVerdict::Scaled { .. }) => "ladder_scaled",
            Some(ResizeVerdict::Other { .. }) | None => "ignored",
        };
        let sps = self.sps.map_or(Value::Null, |s| {
            json!({
                "profile": s.profile_name(),
                "profile_idc": s.profile_idc,
                "level": s.level_name(),
                "level_idc": s.level_idc,
                "size": format!("{}x{}", s.width, s.height),
                "decoder": match s.decoder_accepts() {
                    LevelVerdict::Accepted => "accepted",
                    LevelVerdict::Rejected => "rejected",
                },
            })
        });
        let placement = match self.opts.placement {
            PlacementProbe::Skip => Value::from("skipped"),
            PlacementProbe::Ssh => Value::from(
                probes
                    .iter()
                    .map(|p| match p {
                        Ok(s) => format!("ok: {s}"),
                        Err(s) => format!("FAILED: {s}"),
                    })
                    .collect::<Vec<_>>(),
            ),
        };
        let failures: Vec<&'static str> = self.failures.iter().map(Failure::code).collect();
        let errors: Vec<String> = self.failures.iter().map(Failure::message).collect();
        let mut line = base
            .with("headless", self.headless.map_or(Value::Null, Value::from))
            .with(
                "output",
                self.output.clone().map_or(Value::Null, Value::from),
            )
            .with("resize", resize)
            .with(
                "stream_config",
                json!({
                    "stream": format!("{w}x{h}"),
                    "scale_milli": m,
                    "implied_output": format!("{}x{}", implied.0, implied.1),
                    "fps_cap": cap,
                    "fps_cap_at_window_start": window.cap_at_start,
                    "fps_caps_in_window": window.fps_caps.iter().map(|(_, c)| *c).collect::<Vec<u32>>(),
                }),
            )
            .with(
                "fps_cap_effective",
                round2(measure::effective_cap(
                    window.cap_at_start,
                    &window.fps_caps,
                    self.opts.window,
                )),
            )
            .with("fps_of_cap_min", self.opts.fps_of_cap)
            .with("measured", measured)
            .with("fps", round2(fps))
            .with("min_fps", round2(self.opts.min_fps))
            .with(
                "decode_ms",
                json!({"p50": round2(p50), "p95": round2(p95), "max": round2(max),
                       "samples": window.decode_ms.len()}),
            )
            .with("max_gap_ms", round2(max_gap.as_secs_f64() * 1000.0))
            .with("stalls", stalls)
            .with("stall_limit_s", self.opts.stall.as_secs_f64())
            .with("window_frames", window.frames)
            .with("window_pictures", window.pictures)
            .with("frame_ids_missing", window.ids.missing)
            .with("frames", stats.frames)
            .with("acks", stats.acks)
            .with("acks_written", written)
            .with("presented", stats.presented)
            .with(
                "slot_dropped",
                self.slot
                    .as_ref()
                    .map_or(Value::Null, |s| Value::from(s.dropped())),
            )
            .with("decode_errors", stats.decode_errors)
            .with("keyframe_requests", stats.keyframe_requests)
            .with("decoder_resets", stats.resets)
            .with(
                "decoder_resets_in_window",
                if measured {
                    Value::from(stats.resets.saturating_sub(window.resets_at_start))
                } else {
                    Value::Null
                },
            )
            .with("sps", sps)
            .with("placement", placement)
            .with("pattern", pattern_json)
            .with("exit_after_bye", exit.map_or(Value::Null, Value::from))
            .with("elapsed_s", round2(elapsed.as_secs_f64()))
            .with("failures", failures)
            .with("errors", errors);
        line.ok = self.failures.is_empty() && measured && exit == Some(true);
        if self.failures.iter().any(|f| matches!(f, Failure::Link(_))) {
            line = line
                .with(
                    "ssh_failure",
                    format!("{:?}", diagnose(stderr_log, exit_code)),
                )
                .with("stderr_log", stderr_log.display().to_string());
        }
        line
    }
}
