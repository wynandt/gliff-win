//! A stand-in for `gliff-server --stdio`, used only to prove `gwin-verify
//! ssh` and `gwin-verify stream` on a host without the real server. Not a
//! server: it models the upstream (089662fd) behaviour those checks rest
//! on, each piece cited where it is implemented (see `model.rs`).
//!
//! Order (gliff-server `session.rs:109-158`): read Hello, write HelloAck
//! for the output `gliff-<pid>`, Ping, then StreamConfig at the clamped
//! first mode, then stream. A Bye before the first Pong, or the pipe
//! closing without a Bye, exits non-zero, so `exit_after_bye` proves the
//! client answered the Ping and said Bye.
//!
//! Shutdown is upstream's too (`main.rs:110-133`): a current-thread
//! runtime and a `LocalSet`, with stdin read by a `spawn_local` task that
//! loops until EOF. On Bye the session returns, but that task has already
//! started the next read on tokio's blocking stdin thread, which cannot be
//! cancelled, and dropping the runtime waits for it. So this exits only
//! once the client closes stdin after Bye.
//!
//! Video is real H.264 from `gliff_sw::Encoder`, upstream's CPU encoder,
//! of the shared test pattern (`pattern.rs`) with its moving square.
//!
//! `GWIN_FAKE_SERVER` picks a fault: `wedge` (stop after HelloAck),
//! `wrong-mode` (report the other desktop mode), `bad-exit` (exit 3 after a
//! correct Bye), `linger` (never exit after Bye), `stall` (send nothing
//! for 7 s, 2 s after the Resize), `drop` (leave every 15th P frame off the
//! wire), `ignore-resize`, `level60` (rewrite the SPS level to 6.0),
//! `no-pattern` (a plain desktop) and `misplaced` (hyprctl shows the
//! pattern window on another monitor), `hold30` (the ladder never steps
//! up from level 1, so the cap stays 30, as seen against a real server) and
//! `cap15` (the ladder steps down to level 2, 15 fps, with its
//! StreamConfig, rate.rs:324-358, session.rs:525-547). `reload` models a
//! compositor reload: 1 s after the first Resize the output is lost, as when
//! a runtime mode is discarded. The server keeps its own record, so it sends
//! no frame and no StreamConfig (pings go on) and returns early on a Resize
//! equal to that record (session.rs:665-667); a Resize that differs sets the
//! mode again and ends the fault (session.rs:649-715). `cursor` is not a
//! fault: it sends one 4x4 `CursorShape` after the first StreamConfig, for
//! the client's session driver test.
//!
//! `--version` prints `gliff-server 0.4.1` (the number is
//! `GWIN_FAKE_SERVER_VERSION` when set), the call the client's version check
//! makes; it needs no protocol and no fault.
//!
//! `GWIN_FAKE_TINT=r,g,b` is not a fault either: it draws one flat colour in
//! place of the grid, so a test with several fake servers can tell their
//! pictures apart.
//!
//! `GWIN_FAKE_HYPR_DIR`, when set, is where it writes `monitors.json` and
//! `clients.json` for `scripts/fake-bin/hyprctl`.

mod model;

// Shared with gwin-verify; the compare half is unused here.
#[allow(dead_code)]
#[path = "../../pattern.rs"]
mod pattern;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use gliff_proto::msg::ChromaMode;
use gliff_proto::msg::ClientMsg;
use gliff_proto::msg::Codec;
use gliff_proto::msg::OutputInfo;
use gliff_proto::msg::PROTOCOL_VERSION;
use gliff_proto::msg::Rect;
use gliff_proto::msg::ServerMsg;
use gliff_proto::msg::SessionInfo;
use gliff_proto::msg::VideoPipeline;
use gliff_proto::msg::features;
use gliff_sw::Encoder;
use gliff_sw::EncoderSettings;
use gliff_transport::Framed;
use gliff_windows::h264::set_level_idc;
use tokio::io::AsyncRead;
use tokio::io::Stdout;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::pattern::Content;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Wedge,
    WrongMode,
    BadExit,
    Linger,
    Stall,
    Drop,
    IgnoreResize,
    Level60,
    NoPattern,
    Misplaced,
    Hold30,
    Cap15,
    Cursor,
    Reload,
}

impl Fault {
    fn from_env() -> anyhow::Result<Self> {
        let value = std::env::var("GWIN_FAKE_SERVER").unwrap_or_default();
        Ok(match value.as_str() {
            "" => Self::None,
            "wedge" => Self::Wedge,
            "wrong-mode" => Self::WrongMode,
            "bad-exit" => Self::BadExit,
            "linger" => Self::Linger,
            "stall" => Self::Stall,
            "drop" => Self::Drop,
            "ignore-resize" => Self::IgnoreResize,
            "level60" => Self::Level60,
            "no-pattern" => Self::NoPattern,
            "misplaced" => Self::Misplaced,
            "hold30" => Self::Hold30,
            "cap15" => Self::Cap15,
            "cursor" => Self::Cursor,
            "reload" => Self::Reload,
            other => anyhow::bail!("unknown GWIN_FAKE_SERVER={other}"),
        })
    }
}

/// The stall fault: when it starts after the Resize, and how long.
const STALL_AFTER: Duration = Duration::from_secs(2);
const STALL_FOR: Duration = Duration::from_secs(7);
/// The reload fault: how long after the Resize the output is lost.
const RELOAD_AFTER: Duration = Duration::from_secs(1);
/// The drop fault leaves every this-many-th P frame off the wire.
const DROP_EVERY: u64 = 15;
/// The `cap15` fault's ladder level 2 cap (rate.rs:334-338).
const CAP15_FPS: u32 = 15;
/// How many frame intervals the schedule may fall behind before it restarts
/// from now. It bounds the burst after a late wake-up, so a capped stream
/// stays within a frame or two of its cap over any window.
const MAX_LAG_FRAMES: u32 = 2;
/// Upstream pings about once a second; any cadence works for the check.
const PING_EVERY: Duration = Duration::from_secs(1);
/// The fake's home monitor, for hyprctl.
const HOME: (u32, u32) = (2560, 1600);

/// `GWIN_FAKE_TINT=r,g,b`: draw one flat colour instead of the grid, so
/// several fake servers in one test show different pictures.
fn tint_from_env() -> Option<[u8; 3]> {
    let text = std::env::var("GWIN_FAKE_TINT").ok()?;
    let parts: Vec<u8> = text
        .split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    <[u8; 3]>::try_from(parts).ok()
}

fn main() -> anyhow::Result<()> {
    // As upstream main.rs: the runtime outlives the LocalSet and is dropped
    // last, which is where the pending stdin read holds the exit.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, serve())
}

async fn serve() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The call the client makes to check the server's version: what the real
    // server prints, with the number from the environment when a test needs
    // an old one.
    if args.first().map(String::as_str) == Some("--version") {
        let number = std::env::var("GWIN_FAKE_SERVER_VERSION").unwrap_or_else(|_| "0.4.1".into());
        println!("gliff-server {number}");
        return Ok(());
    }
    let fault = Fault::from_env()?;
    anyhow::ensure!(
        args.first().map(String::as_str) == Some("--stdio"),
        "usage: gwin-fake-server --stdio [--headless | --output NAME]"
    );
    let headless = args.iter().any(|a| a == "--headless");
    let name = args
        .iter()
        .position(|a| a == "--output")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .map(|name| {
            // `--output auto` is the focused real output (upstream
            // session.rs:1661-1700); the fake has one.
            if name == "auto" {
                "HDMI-A-1".to_owned()
            } else {
                name
            }
        })
        .unwrap_or_else(|| format!("gliff-{}", std::process::id()));
    let mut reader = Framed::new(tokio::io::stdin());
    let writer = Framed::new(tokio::io::stdout());
    let caps_max = match reader.read_msg::<ClientMsg>().await? {
        ClientMsg::Hello { version, caps, .. } if version == PROTOCOL_VERSION => {
            (caps.max_width, caps.max_height)
        }
        other => anyhow::bail!("expected Hello v{PROTOCOL_VERSION}, got {other:?}"),
    };
    let mode = model::initial_mode(caps_max);
    let mut server = Server::new(writer, fault, name, mode, caps_max)?;
    server
        .writer
        .write_msg(&ServerMsg::HelloAck {
            version: PROTOCOL_VERSION,
            session: SessionInfo {
                headless: headless != (fault == Fault::WrongMode),
                output: server.output.name.clone(),
            },
            outputs: vec![OutputInfo {
                name: server.output.name.clone(),
                width: mode.0,
                height: mode.1,
                scale_milli: 1000,
            }],
            features: features(),
        })
        .await?;
    if fault == Fault::Wedge {
        std::future::pending::<()>().await;
    }
    server.write_hypr()?;
    let first_ping = now_ms();
    server
        .writer
        .write_msg(&ServerMsg::Ping { t: first_ping })
        .await?;
    server.send_config().await?;
    if fault == Fault::Cursor {
        server.send_cursor().await?;
    }
    let mut inbound = spawn_reader(reader);
    let mut ponged = false;
    let mut next_frame = Instant::now();
    let mut ping = tokio::time::interval(PING_EVERY);
    loop {
        tokio::select! {
            msg = inbound.recv() => match msg {
                Some(ClientMsg::Pong { t }) if t == first_ping => ponged = true,
                Some(ClientMsg::Bye) if ponged => break,
                Some(ClientMsg::Bye) => anyhow::bail!("Bye before Pong"),
                Some(msg) => server.on_client(msg).await?,
                None => anyhow::bail!("closed without Bye"),
            },
            _ = tokio::time::sleep_until(next_frame) => {
                next_frame = server.tick(next_frame).await?;
            }
            _ = ping.tick() => {
                if !server.stalled() {
                    server.writer.write_msg(&ServerMsg::Ping { t: now_ms() }).await?;
                }
            }
        }
    }
    // Upstream leaves the session on Bye (session.rs:498) and dropping
    // `SessionOutput` removes the headless output (session.rs:1477-1482),
    // so from here hyprctl no longer shows it or the pattern on it.
    server.remove_hypr()?;
    match fault {
        Fault::BadExit => std::process::exit(3),
        Fault::Linger => std::future::pending::<()>().await,
        _ => {}
    }
    Ok(())
}

struct Output {
    name: String,
    width: u32,
    height: u32,
    scale: f32,
}

struct Server {
    writer: Framed<Stdout>,
    fault: Fault,
    caps_max: (u32, u32),
    output: Output,
    /// The coded size (`base_stream`; the fake has no scale levels).
    stream: (u32, u32),
    encoder: Encoder,
    want_keyframe: bool,
    fps_cap: u32,
    /// When the ladder last changed level or the cap last blocked a frame.
    hold_from: Instant,
    frame_id: u64,
    sent_p_frames: u64,
    inflight: VecDeque<(u64, usize)>,
    largest_frame: usize,
    last_frame: usize,
    content: Content,
    background: Vec<u8>,
    scratch: Vec<u8>,
    tick: u64,
    resized_at: Option<Instant>,
    /// The `reload` fault has struck and no differing Resize has come since.
    lost: bool,
    /// The `reload` fault strikes once.
    reloaded: bool,
    hypr_dir: Option<PathBuf>,
}

impl Server {
    fn new(
        writer: Framed<Stdout>,
        fault: Fault,
        name: String,
        mode: (u32, u32),
        caps_max: (u32, u32),
    ) -> anyhow::Result<Self> {
        let stream = model::fit_extent(mode.0, mode.1, model::CPU_ENCODER_MAX);
        let content = match (fault, tint_from_env()) {
            (Fault::NoPattern, _) => Content::Desktop,
            (_, Some(rgb)) => Content::Flat(rgb),
            _ => Content::Pattern,
        };
        let mut server = Self {
            writer,
            fault,
            caps_max,
            output: Output {
                name,
                width: mode.0,
                height: mode.1,
                scale: 1.0,
            },
            stream,
            encoder: encoder(stream, model::START_FPS)?,
            want_keyframe: true,
            fps_cap: model::START_FPS,
            hold_from: Instant::now(),
            frame_id: 0,
            sent_p_frames: 0,
            inflight: VecDeque::new(),
            largest_frame: 0,
            last_frame: 0,
            content,
            background: Vec::new(),
            scratch: Vec::new(),
            tick: 0,
            resized_at: None,
            lost: false,
            reloaded: false,
            hypr_dir: std::env::var_os("GWIN_FAKE_HYPR_DIR").map(PathBuf::from),
        };
        pattern::background(stream.0, stream.1, content, &mut server.background);
        Ok(server)
    }

    /// A 4x4 BGRA arrow-ish shape: one opaque black pixel in a clear
    /// square, hot spot (1, 2).
    async fn send_cursor(&mut self) -> anyhow::Result<()> {
        let mut argb = vec![0u8; 4 * 4 * 4];
        argb[..4].copy_from_slice(&[0, 0, 0, 255]);
        let msg = ServerMsg::CursorShape {
            id: 7,
            width: 4,
            height: 4,
            hot_x: 1,
            hot_y: 2,
            argb_len: argb.len() as u32,
        };
        Ok(self.writer.write_msg_with_payloads(&msg, &[&argb]).await?)
    }

    /// session.rs:1485-1511: view = base_stream, empty extradata
    /// (parameter sets in-band), scale_milli from the output.
    async fn send_config(&mut self) -> anyhow::Result<()> {
        let msg = ServerMsg::StreamConfig {
            codec: Codec::H264,
            chroma: ChromaMode::Single420,
            pipeline: Some(VideoPipeline::Cpu),
            width: self.stream.0,
            height: self.stream.1,
            scale_milli: model::scale_milli(self.output.scale, self.stream.0, self.output.width),
            extradata: Vec::new(),
            aux_extradata: None,
            view_width: self.stream.0,
            view_height: self.stream.1,
            fps_cap: self.fps_cap,
        };
        Ok(self.writer.write_msg(&msg).await?)
    }

    async fn on_client(&mut self, msg: ClientMsg) -> anyhow::Result<()> {
        match msg {
            // Cumulative; an id not in flight is ignored
            // (session.rs:499-503, rate.rs:111-140).
            ClientMsg::FrameAck { frame_id, .. } => {
                while self.inflight.front().is_some_and(|(id, _)| *id <= frame_id) {
                    self.inflight.pop_front();
                }
            }
            // session.rs:550-558.
            ClientMsg::RequestKeyframe => self.want_keyframe = true,
            ClientMsg::Resize {
                width,
                height,
                scale,
            } if self.fault != Fault::IgnoreResize => self.resize(width, height, scale).await?,
            // Upstream answers a client Ping (session.rs:589-599).
            ClientMsg::Ping { t } => {
                self.writer
                    .write_msg(&ServerMsg::Pong {
                        t,
                        server_now_ms: now_ms(),
                    })
                    .await?
            }
            _ => {}
        }
        Ok(())
    }

    /// session.rs:625-683, headless branch.
    async fn resize(&mut self, width: u32, height: u32, scale: f32) -> anyhow::Result<()> {
        let Some(((width, height), scale)) =
            model::resize_mode((width, height), scale, self.caps_max)
        else {
            return Ok(());
        };
        let same_size = (width, height) == (self.output.width, self.output.height);
        if same_size && (scale - self.output.scale).abs() < 0.01 {
            return Ok(());
        }
        if !same_size {
            // A geometry change rebuilds the encoder and forces a keyframe
            // (session.rs:982-1037).
            let base = model::fit_extent(width, height, model::CPU_ENCODER_MAX);
            self.encoder = encoder(base, self.fps_cap)?;
            self.want_keyframe = true;
            self.stream = base;
            self.output.width = width;
            self.output.height = height;
            pattern::background(base.0, base.1, self.content, &mut self.background);
        }
        self.output.scale = scale;
        self.resized_at = Some(Instant::now());
        self.lost = false;
        self.write_hypr()?;
        self.send_config().await
    }

    /// True while the `reload` fault has the output lost; strikes once.
    fn lost(&mut self) -> bool {
        if self.fault == Fault::Reload
            && !self.reloaded
            && self
                .resized_at
                .is_some_and(|at| at.elapsed() >= RELOAD_AFTER)
        {
            self.reloaded = true;
            self.lost = true;
        }
        self.lost
    }

    fn stalled(&self) -> bool {
        match (self.fault, self.resized_at) {
            (Fault::Stall, Some(at)) => {
                let since = at.elapsed();
                since >= STALL_AFTER && since < STALL_AFTER + STALL_FOR
            }
            _ => false,
        }
    }

    /// One pacing tick: send a frame if the byte cap has room, step the
    /// ladder, and say when to come back.
    /// Sends the frame due at `due` (or waits); returns when the next is
    /// due. Pacing from `due`, not from now, keeps the encode time from
    /// lowering the rate.
    async fn tick(&mut self, due: Instant) -> anyhow::Result<Instant> {
        let now = Instant::now();
        let interval = Duration::from_secs_f64(1.0 / f64::from(self.fps_cap.max(1)));
        if self.stalled() || self.lost() {
            return Ok(now + Duration::from_millis(20));
        }
        let inflight: u64 = self.inflight.iter().map(|(_, n)| *n as u64).sum();
        let cap = model::cap_bytes(self.largest_frame);
        if !model::has_room(inflight, self.last_frame as u64, cap) {
            // Blocked on acks: no frame, and no step up (rate.rs:324-376).
            self.hold_from = now;
            return Ok(now + Duration::from_millis(2));
        }
        self.send_frame().await?;
        if self.fps_cap == model::START_FPS
            && self.fault != Fault::Hold30
            && now.duration_since(self.hold_from) >= Duration::from_millis(model::HOLD_UP_MS)
        {
            // Level 0 (or level 2 for `cap15`): fps-only, no encoder
            // rebuild (session.rs:982-1037); either step sends a
            // StreamConfig (session.rs:525-547).
            self.fps_cap = if self.fault == Fault::Cap15 {
                CAP15_FPS
            } else {
                model::MAX_FPS
            };
            self.encoder.set_rate(bitrate(self.stream), self.fps_cap);
            self.hold_from = now;
            self.send_config().await?;
        }
        // Pace from the schedule, so a late wake-up is made up by the next
        // ticks and the average rate is the cap however busy the machine is;
        // only a lag past MAX_LAG_FRAMES starts again from now rather than burst.
        let next = due + interval;
        Ok(
            if now.saturating_duration_since(next) > interval * MAX_LAG_FRAMES {
                now
            } else {
                next
            },
        )
    }

    async fn send_frame(&mut self) -> anyhow::Result<()> {
        self.scratch.clone_from(&self.background);
        pattern::square(self.stream.0, self.stream.1, self.tick, &mut self.scratch);
        self.tick += 1;
        let force = std::mem::take(&mut self.want_keyframe);
        let mut encoded = self.encoder.encode_bgra(&self.scratch, force)?;
        if encoded.main.is_empty() {
            return Ok(());
        }
        if encoded.keyframe && self.fault == Fault::Level60 {
            set_level_idc(&mut encoded.main, 60);
        }
        // session.rs:1151-1193: frame_id from 0, +1 per frame sent.
        let frame_id = self.frame_id;
        self.frame_id += 1;
        if !encoded.keyframe {
            self.sent_p_frames += 1;
            if self.fault == Fault::Drop && self.sent_p_frames.is_multiple_of(DROP_EVERY) {
                return Ok(());
            }
        }
        let main = encoded.main;
        let msg = ServerMsg::VideoFrame {
            frame_id,
            pts_us: now_ms() * 1000,
            keyframe: encoded.keyframe,
            damage: vec![Rect {
                x: 0,
                y: 0,
                width: self.stream.0 as i32,
                height: self.stream.1 as i32,
            }],
            data_len: main.len() as u32,
            aux_len: 0,
        };
        self.writer.write_msg_with_payloads(&msg, &[&main]).await?;
        self.inflight.push_back((frame_id, main.len()));
        self.largest_frame = self.largest_frame.max(main.len());
        self.last_frame = main.len();
        Ok(())
    }

    /// The output is gone: hyprctl falls back to the home monitor alone.
    fn remove_hypr(&self) -> anyhow::Result<()> {
        let Some(dir) = &self.hypr_dir else {
            return Ok(());
        };
        for name in ["monitors.json", "clients.json"] {
            match std::fs::remove_file(dir.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// hyprctl-shaped state: the home monitor, the session output to its
    /// right, and the pattern window (on the output, or on the home
    /// monitor for `misplaced`).
    fn write_hypr(&self) -> anyhow::Result<()> {
        let Some(dir) = &self.hypr_dir else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let output_x = HOME.0;
        let (lw, lh) = (
            (self.output.width as f32 / self.output.scale).round() as u32,
            (self.output.height as f32 / self.output.scale).round() as u32,
        );
        let monitors = serde_json::json!([
            {"id": 0, "name": "eDP-1", "width": HOME.0, "height": HOME.1, "x": 0, "y": 0, "scale": 1.0},
            {"id": 1, "name": self.output.name, "width": self.output.width, "height": self.output.height,
             "x": output_x, "y": 0, "scale": self.output.scale},
        ]);
        let window = match self.fault {
            Fault::Misplaced => serde_json::json!({"title": "gwin-pattern", "monitor": 0,
                "at": [0, 0], "size": [HOME.0, HOME.1], "fullscreen": 2}),
            _ => serde_json::json!({"title": "gwin-pattern", "monitor": 1,
                "at": [output_x, 0], "size": [lw, lh], "fullscreen": 2}),
        };
        let clients = serde_json::json!([
            {"title": "Alacritty", "monitor": 0, "at": [0, 0], "size": [1280, 1600], "fullscreen": 0},
            window,
        ]);
        std::fs::write(dir.join("monitors.json"), monitors.to_string())?;
        std::fs::write(dir.join("clients.json"), clients.to_string())?;
        Ok(())
    }
}

fn bitrate(stream: (u32, u32)) -> u32 {
    (stream.0 * stream.1 * 8).clamp(2_000_000, 40_000_000)
}

fn encoder(stream: (u32, u32), fps: u32) -> anyhow::Result<Encoder> {
    Ok(Encoder::new(
        EncoderSettings {
            width: stream.0,
            height: stream.1,
            bitrate: bitrate(stream),
            framerate: fps,
        },
        false,
    )?)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Upstream's `spawn_reader`: read until EOF or an error on a local task
/// and hand each message over a channel. Nothing stops it on Bye.
fn spawn_reader<R>(mut reader: Framed<R>) -> mpsc::UnboundedReceiver<ClientMsg>
where
    R: AsyncRead + Unpin + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::task::spawn_local(async move {
        while let Ok(msg) = reader.read_msg::<ClientMsg>().await {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    rx
}
