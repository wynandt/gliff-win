//! `Core`: the client's protocol state machine, with no I/O.
//!
//! A port of the upstream client's session loop (`gliff/src/net.rs` at
//! 089662fd). Each server message, decode result and UI command goes in,
//! and the [`Action`]s the driver must perform come out, in order. The
//! driver owns the socket, the decoder and the clock; `Core` owns the
//! protocol rules, the pending-geometry FIFO and the counters.

use std::collections::BTreeMap;
use std::collections::VecDeque;

use bytes::Bytes;
use gliff_proto::msg::Axis;
use gliff_proto::msg::ChromaMode;
use gliff_proto::msg::ClientMsg;
use gliff_proto::msg::ClipboardMsg;
use gliff_proto::msg::Codec;
use gliff_proto::msg::PROTOCOL_VERSION;
use gliff_proto::msg::ServerMsg;
use gliff_proto::msg::SessionInfo;
use gliff_proto::msg::VideoPipeline;
use gliff_proto::msg::is_incompatible;
use gliff_proto::msg::version_mismatch;

use crate::altgr::RawKey;
use crate::cursor_image::CursorImage;
use crate::decode::DecodeError;
use crate::input::KeyPipeline;
use crate::input::KeyPolicy;
use crate::keymap::Chord;
use crate::keymap::KeyEvent;
use crate::pointer::PointerEvent;
use crate::session::Colour;
use crate::session::Histogram;
use crate::session::input_side::InputSide;
use crate::session::input_side::KeyOut;
use crate::yuv::YuvFrame;

/// The largest stream the client accepts (upstream `MAX_DECODE`,
/// net.rs:769-774).
pub const MAX_DECODE: (u32, u32) = (4096, 4096);

/// A window-size request, in physical pixels at the protocol scale
/// (dpi / 96, never raw DPI).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeReq {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl ResizeReq {
    /// The width a resync steps by, in pixels.
    pub const NUDGE: u32 = 32;

    /// A request the server cannot take for the one it already holds: the
    /// width changes by [`Self::NUDGE`] (narrower, unless that leaves under
    /// 320 px).
    pub fn nudged(self) -> Self {
        let width = if self.width >= 320 + Self::NUDGE {
            self.width - Self::NUDGE
        } else {
            self.width + Self::NUDGE
        };
        Self { width, ..self }
    }

    fn to_msg(self) -> ClientMsg {
        ClientMsg::Resize {
            width: self.width,
            height: self.height,
            scale: self.scale,
        }
    }
}

/// The geometry an access unit was encoded under: the coded stream size,
/// the view (the output region it shows) and the server's effective scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub stream: (u32, u32),
    pub view: (u32, u32),
    pub scale_milli: u32,
}

/// Whether a StreamConfig replaced the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderChange {
    /// The coded stream (width, height, chroma) changed, or this is the
    /// first config.
    Reset,
    /// A pace-only or scale-only change; the decoder keeps its state.
    Kept,
}

/// What a StreamConfig told the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub geometry: Geometry,
    pub fps_cap: u32,
    /// The colour the server picked (Dual420 only when Sharp colour offered it).
    pub chroma: ChromaMode,
    pub pipeline: Option<VideoPipeline>,
    pub decoder: DecoderChange,
}

/// A decoded picture with the geometry of the access unit that made it.
#[derive(Debug)]
pub struct Picture {
    /// Tight I420; it also carries when the decoder released it.
    pub frame: YuvFrame,
    pub geometry: Geometry,
    /// The `frame_id` of the access unit this picture came from.
    pub frame_id: u64,
}

// By picture content; `YuvFrame`'s own equality ignores `decoded_at`.
impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        self.geometry == other.geometry
            && self.frame_id == other.frame_id
            && self.frame == other.frame
    }
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The client said Bye.
    Bye,
    /// A version or capability mismatch (an error code below 256, or a
    /// HelloAck with another protocol version).
    Incompatible(String),
    /// Any other server Error.
    ServerError { code: u16, message: String },
    /// The server broke the protocol.
    Link(String),
    /// ssh failed before the session began, and its own words said why
    /// ([`crate::connect::classify`]).
    Ssh(crate::connect::SshFailure),
}

/// Events for the UI (or, in `gwin-verify`, for the check).
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Connected(SessionInfo),
    StreamConfig(StreamInfo),
    /// A `CursorShape` while streaming (`CursorPos` is ignored, as
    /// upstream: the local pointer is the cursor).
    Cursor(CursorImage),
    Ended(EndReason),
    /// The release chord completed: the UI gives the keyboard back to
    /// Windows.
    GrabReleased,
    /// A resync started: the server stopped answering and the client asked
    /// it for its output again.
    Resyncing,
    /// The first picture after a resync: the server is answering again.
    Resynced,
}

/// Commands from the UI (and, for keys, from the hook thread).
#[derive(Debug, Clone, PartialEq)]
pub enum UiCmd {
    /// Deduplicated: the same request twice sends one Resize.
    Resize(ResizeReq),
    Bye,
    /// A key from the hook or the legacy-message watchdog.
    Key(RawKey),
    /// Events already in evdev terms (Super+L from the hover hook).
    Keys(Vec<KeyEvent>),
    /// The AltGr filter's timer.
    Tick(u32),
    /// A pointer event, in remote coordinates.
    Pointer(PointerEvent),
    /// A raw wheel delta, in Windows units.
    Wheel {
        axis: Axis,
        delta: i16,
    },
    /// An Up for every held key and button (focus loss, lock, desktop
    /// switch).
    ReleaseInput,
    /// The Windows layout changed: new XKB text.
    Keymap(String),
    /// The hook saw the release chord and has already taken the keyboard
    /// back: release every held key and tell the UI. Handled in every phase
    /// until the session ends.
    ReleaseGrab,
    /// The keyboard hook was installed again in the middle of a grab: an Up
    /// for every modifier the user is not holding, in case the remote missed
    /// one.
    ClearModifiers,
}

/// What the driver must do, in the order given.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Write this message, then this payload (empty for every message
    /// the client sends so far), through the writer task.
    Send(ClientMsg, Bytes),
    /// Decode `main` (and `aux`, which is empty unless the stream is
    /// Dual420), then call [`Core::decoded`] with the result.
    Decode {
        frame_id: u64,
        main: Bytes,
        aux: Bytes,
    },
    /// Replace the decoder with one for this chroma before the next
    /// [`Action::Decode`].
    ResetDecoder(ChromaMode),
    /// Hand the picture to the frame slot.
    Present(Picture),
    Ui(UiEvent),
    /// A clipboard message from the server, with the payload a `Data`
    /// message declared (empty otherwise), for the clipboard engine.
    Clipboard(ClipboardMsg, Bytes),
}

/// One finished decode, for [`Core::decoded`].
#[derive(Debug)]
pub struct Decoded {
    pub frame_id: u64,
    pub result: Result<Option<YuvFrame>, DecodeError>,
    /// How long the decode call took.
    pub decode_ms: f32,
    /// Wall clock in ms since the Unix epoch, for `FrameAck::decoded_at_ms`.
    pub now_ms: u64,
}

/// Counters for the session.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    /// VideoFrames received.
    pub frames: u64,
    /// FrameAcks emitted.
    pub acks: u64,
    pub decode_errors: u64,
    /// Idle drains that failed (upstream only logs these, net.rs:549-552).
    pub drain_errors: u64,
    pub keyframe_requests: u64,
    /// Pictures handed to the frame slot.
    pub presented: u64,
    pub configs: u64,
    pub resets: u64,
    pub resizes_sent: u64,
    /// Resyncs started (see [`Core::resync`]).
    pub resyncs: u64,
    pub decode_ms: Histogram,
    /// Wheel steps sent, by axis and the sign of the step as sent, as
    /// Wayland counts: "vertical+" (down), "vertical-" (up), "horizontal+"
    /// (right), "horizontal-" (left).
    pub wheel_steps_sent: BTreeMap<String, u64>,
    /// Button presses sent, by Linux button code.
    pub button_presses_sent: BTreeMap<u32, u64>,
    /// Keys held down now (sent Down, no Up yet): zero in a session that is
    /// not shown, whatever was held when it was switched away from.
    pub keys_down: u32,
    /// Buttons held down now, likewise.
    pub buttons_down: u32,
}

impl SessionStats {
    /// Counts the wheel steps and button presses in a message sent.
    fn count_sent(&mut self, msg: &ClientMsg) {
        match msg {
            ClientMsg::PointerAxis {
                axis,
                discrete: Some(steps),
                ..
            } if *steps != 0 => {
                let name = match axis {
                    Axis::Vertical => "vertical",
                    Axis::Horizontal => "horizontal",
                };
                let sign = if *steps > 0 { '+' } else { '-' };
                *self
                    .wheel_steps_sent
                    .entry(format!("{name}{sign}"))
                    .or_default() += u64::from(steps.unsigned_abs());
            }
            ClientMsg::PointerButton {
                button,
                pressed: true,
            } => *self.button_presses_sent.entry(*button).or_default() += 1,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitHelloAck,
    AwaitStreamConfig,
    Streaming,
    Ended,
}

/// The current coded stream, for deciding whether a StreamConfig resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Coded {
    size: (u32, u32),
    chroma: ChromaMode,
}

/// The client's protocol state machine; see the module docs.
#[derive(Debug)]
pub struct Core {
    phase: Phase,
    /// The Resize sent right after HelloAck (`--headless` starts clamped to
    /// 1920x1080, so the client must ask for its size).
    initial: ResizeReq,
    /// The newest layout the window sent before the HelloAck: the window
    /// sends each layout once, so one dropped here would never be resent.
    pending_keymap: Option<String>,
    last_resize: Option<ResizeReq>,
    /// A resync was sent and no picture has arrived since.
    resyncing: bool,
    /// The server's output is a private headless one (HelloAck's `headless`);
    /// true until the HelloAck says otherwise, as before there was a mirror.
    headless: bool,
    /// What `client_caps` offered; a StreamConfig for anything else ends
    /// the session.
    colour: Colour,
    coded: Option<Coded>,
    geometry: Option<Geometry>,
    /// (frame_id, geometry) of each unit fed to the decoder and not yet
    /// released, oldest first (upstream `in_decoder`, net.rs:463-467).
    in_decoder: VecDeque<(u64, Geometry)>,
    stats: SessionStats,
    input: InputSide,
}

impl Core {
    /// A core that will ask for `initial` once the server's HelloAck is in.
    pub fn new(initial: ResizeReq) -> Self {
        Self {
            phase: Phase::AwaitHelloAck,
            initial,
            pending_keymap: None,
            last_resize: None,
            resyncing: false,
            headless: true,
            colour: Colour::Standard,
            coded: None,
            geometry: None,
            in_decoder: VecDeque::new(),
            stats: SessionStats::default(),
            input: InputSide::new(KeyPipeline::new(
                Chord::ctrl_alt_home(),
                KeyPolicy::default(),
            )),
        }
    }

    /// The colour the Hello offered (`Colour::Standard` unless set).
    pub fn with_colour(mut self, colour: Colour) -> Self {
        self.colour = colour;
        self
    }

    /// Replaces the key pipeline (the release chord, the stand-in chord and
    /// the key policy come from the command line).
    pub fn with_pipeline(mut self, pipeline: KeyPipeline) -> Self {
        self.input = InputSide::new(pipeline);
        self
    }

    /// True while the AltGr filter waits for a key, so the driver arms its
    /// timer.
    pub fn input_holding(&self) -> bool {
        self.input.holding()
    }

    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// Asks a server that stopped sending pictures for its output again: a
    /// Resize [`ResizeReq::nudged`] from the last one, then the last one. A
    /// server that reset its output behind its own record ignores a Resize
    /// equal to the record, and a different size makes it set the mode and send
    /// a `StreamConfig` (see windows-quirks.md, "The remote compositor
    /// reloads"). `Resynced` follows with the first picture. Nothing happens
    /// before the first Resize or once the session has ended, and never for a
    /// mirrored screen: only a headless output the compositor reset has a
    /// mode behind the server's record, and a real output keeps its own.
    ///
    /// The Resizes are followed by an Up for every modifier the user does not
    /// hold: the remote may have missed one while it stalled.
    pub fn resync(&mut self) -> Vec<Action> {
        let (Phase::Streaming, Some(real), true) = (self.phase, self.last_resize, self.headless)
        else {
            return Vec::new();
        };
        self.resyncing = true;
        self.stats.resyncs += 1;
        self.stats.resizes_sent += 2;
        // The remote may have missed a modifier's Up while it stalled.
        let mut actions = vec![
            Action::Ui(UiEvent::Resyncing),
            send(real.nudged().to_msg()),
            send(real.to_msg()),
        ];
        actions.extend(self.input.clear_modifiers().into_iter().map(send));
        actions
    }

    /// True once the session has ended; every later input is ignored.
    pub fn is_ended(&self) -> bool {
        self.phase == Phase::Ended
    }

    /// One server message with its payloads (see `link::read_server`).
    pub fn on_server(&mut self, msg: ServerMsg, main: Bytes, aux: Bytes) -> Vec<Action> {
        match (self.phase, msg) {
            (Phase::Ended, _) => Vec::new(),
            // Ping first in every phase, so the server's RTT estimate is
            // seeded before the first ack (net.rs:304-306, :326, :712-714).
            (_, ServerMsg::Ping { t }) => vec![send(ClientMsg::Pong { t })],
            (_, ServerMsg::Error { code, message, .. }) => {
                let message = bounded_message(&message);
                let reason = if is_incompatible(code) {
                    EndReason::Incompatible(message)
                } else {
                    EndReason::ServerError { code, message }
                };
                self.end(reason)
            }
            (
                Phase::AwaitHelloAck,
                ServerMsg::HelloAck {
                    version, session, ..
                },
            ) => self.on_hello_ack(version, session),
            (Phase::AwaitStreamConfig | Phase::Streaming, msg @ ServerMsg::StreamConfig { .. }) => {
                self.on_stream_config(msg)
            }
            (Phase::Streaming, ServerMsg::VideoFrame { frame_id, .. }) => {
                self.on_video_frame(frame_id, main, aux)
            }
            (
                Phase::Streaming,
                ServerMsg::CursorShape {
                    id,
                    width,
                    height,
                    hot_x,
                    hot_y,
                    ..
                },
            ) => vec![Action::Ui(UiEvent::Cursor(CursorImage {
                id,
                width,
                height,
                hot: (hot_x, hot_y),
                argb: main,
            }))],
            // The server may offer its selection as soon as the session
            // is up, so clipboard traffic is also taken before the first
            // StreamConfig.
            (Phase::Streaming | Phase::AwaitStreamConfig, msg) if is_clipboard(&msg) => msg
                .into_clipboard()
                .map(|clip| vec![Action::Clipboard(clip, main)])
                .unwrap_or_default(),
            // Mid-stream HelloAck and CursorPos: ignored (net.rs:715-722).
            (Phase::Streaming | Phase::AwaitStreamConfig, ServerMsg::HelloAck { .. }) => Vec::new(),
            (Phase::Streaming, _) => Vec::new(),
            (Phase::AwaitHelloAck | Phase::AwaitStreamConfig, other) => self.end(EndReason::Link(
                format!("unexpected message before StreamConfig: {}", name(&other)),
            )),
        }
    }

    /// The result of an [`Action::Decode`]. The FrameAck comes first,
    /// whatever the result (net.rs:604-611).
    pub fn decoded(&mut self, done: Decoded) -> Vec<Action> {
        if self.phase == Phase::Ended {
            return Vec::new();
        }
        self.stats.decode_ms.record(done.decode_ms);
        let mut actions = vec![self.ack(done.frame_id, done.now_ms)];
        match done.result {
            Ok(Some(frame)) => actions.push(self.present(frame, done.frame_id)),
            // The CPU decoder holds the picture; its geometry stays queued.
            Ok(None) => {}
            Err(_) => {
                self.stats.decode_errors += 1;
                self.in_decoder.clear();
                actions.push(self.request_keyframe());
            }
        }
        actions
    }

    /// The result of an idle drain (`CpuDecoder::flush`). Never acks: the
    /// unit was acked when it was fed (net.rs:525-561).
    pub fn drained(&mut self, result: Result<Option<YuvFrame>, DecodeError>) -> Vec<Action> {
        match (self.phase, result) {
            (Phase::Ended, _) | (_, Ok(None)) => Vec::new(),
            (_, Ok(Some(frame))) => {
                let fallback = self.in_decoder.back().map_or(0, |(id, _)| *id);
                vec![self.present(frame, fallback)]
            }
            (_, Err(_)) => {
                self.stats.drain_errors += 1;
                Vec::new()
            }
        }
    }

    pub fn on_ui(&mut self, cmd: UiCmd) -> Vec<Action> {
        let actions = self.on_ui_cmd(cmd);
        (self.stats.keys_down, self.stats.buttons_down) = self.input.held();
        actions
    }

    fn on_ui_cmd(&mut self, cmd: UiCmd) -> Vec<Action> {
        match (self.phase, cmd) {
            (Phase::Ended, _) => Vec::new(),
            (Phase::AwaitHelloAck, UiCmd::Resize(req)) => {
                self.initial = req;
                Vec::new()
            }
            (_, UiCmd::Resize(req)) => self.resize(req).into_iter().collect(),
            (_, UiCmd::Bye) => {
                // Nothing may stay held on the remote after the session.
                let mut actions: Vec<Action> =
                    self.input.release_all().into_iter().map(send).collect();
                actions.push(send(ClientMsg::Bye));
                actions.extend(self.end(EndReason::Bye));
                actions
            }
            (phase, UiCmd::ReleaseGrab) => {
                // Before the HelloAck the server has seen no key.
                let mut actions = if phase == Phase::AwaitHelloAck {
                    self.input.release_all();
                    Vec::new()
                } else {
                    self.on_input(UiCmd::ReleaseInput)
                };
                actions.push(Action::Ui(UiEvent::GrabReleased));
                actions
            }
            // Before the HelloAck keys go through the pipeline, so the
            // release chord works while the hook already holds the keyboard
            // (a slow or unreachable host), but nothing is sent: the server
            // has not accepted the session yet. Everything else has nowhere
            // to go.
            (Phase::AwaitHelloAck, cmd @ (UiCmd::Key(_) | UiCmd::Tick(_))) => self
                .on_input(cmd)
                .into_iter()
                .filter(|action| matches!(action, Action::Ui(_)))
                .collect(),
            // A switch away from a session that is still connecting: the
            // server has seen no key, but the pipeline counts the held ones,
            // and they must not stay held for the next time it is shown.
            (Phase::AwaitHelloAck, UiCmd::ReleaseInput) => {
                self.input.release_all();
                Vec::new()
            }
            (Phase::AwaitHelloAck, UiCmd::Keymap(keymap)) => {
                self.pending_keymap = Some(keymap);
                Vec::new()
            }
            (Phase::AwaitHelloAck, _) => Vec::new(),
            (_, cmd) => self.on_input(cmd),
        }
    }

    fn on_input(&mut self, cmd: UiCmd) -> Vec<Action> {
        let msgs = match cmd {
            UiCmd::Key(key) => return key_actions(self.input.key(key)),
            UiCmd::Tick(now_ms) => return key_actions(self.input.tick(now_ms)),
            UiCmd::Keys(events) => self.input.keys(events),
            UiCmd::Pointer(event) => self.input.pointer(event),
            UiCmd::Wheel { axis, delta } => self.input.wheel(axis, delta),
            UiCmd::ReleaseInput => self.input.release_all(),
            UiCmd::Keymap(keymap) => vec![ClientMsg::Keymap { keymap }],
            UiCmd::ClearModifiers => self.input.clear_modifiers(),
            // Handled by `on_ui`.
            UiCmd::Resize(_) | UiCmd::Bye | UiCmd::ReleaseGrab => Vec::new(),
        };
        for msg in &msgs {
            self.stats.count_sent(msg);
        }
        msgs.into_iter().map(send).collect()
    }

    fn on_hello_ack(&mut self, version: u16, session: SessionInfo) -> Vec<Action> {
        if version != PROTOCOL_VERSION {
            return self.end(EndReason::Incompatible(
                version_mismatch(PROTOCOL_VERSION, version).to_owned(),
            ));
        }
        self.phase = Phase::AwaitStreamConfig;
        self.headless = session.headless;
        let mut actions = vec![Action::Ui(UiEvent::Connected(session))];
        actions.extend(self.resize(self.initial));
        if let Some(keymap) = self.pending_keymap.take() {
            actions.extend(self.on_input(UiCmd::Keymap(keymap)));
        }
        actions
    }

    fn on_stream_config(&mut self, msg: ServerMsg) -> Vec<Action> {
        let ServerMsg::StreamConfig {
            codec,
            chroma,
            pipeline,
            width,
            height,
            scale_milli,
            view_width,
            view_height,
            fps_cap,
            ..
        } = msg
        else {
            return Vec::new();
        };
        if let Err(problem) = validate_stream(codec, chroma, (width, height), self.colour) {
            return self.end(EndReason::Link(problem));
        }
        let coded = Coded {
            size: (width, height),
            chroma,
        };
        let geometry = Geometry {
            stream: (width, height),
            view: (view_width, view_height),
            scale_milli,
        };
        self.stats.configs += 1;
        self.phase = Phase::Streaming;
        self.geometry = Some(geometry);
        let mut actions = Vec::new();
        // Reset only when the coded stream changes (net.rs:676-683).
        let decoder = if self.coded == Some(coded) {
            DecoderChange::Kept
        } else {
            self.coded = Some(coded);
            self.in_decoder.clear();
            self.stats.resets += 1;
            actions.push(Action::ResetDecoder(chroma));
            DecoderChange::Reset
        };
        actions.push(Action::Ui(UiEvent::StreamConfig(StreamInfo {
            geometry,
            fps_cap,
            chroma,
            pipeline,
            decoder,
        })));
        actions
    }

    fn on_video_frame(&mut self, frame_id: u64, main: Bytes, aux: Bytes) -> Vec<Action> {
        self.stats.frames += 1;
        let dual = self.coded.is_some_and(|c| c.chroma == ChromaMode::Dual420);
        if dual == aux.is_empty() {
            // An aux payload on a Single420 stream, which no decoder takes,
            // or a Dual420 frame without its aux half. Neither decoder is
            // fed, so a Dual420 pair stays in step until the keyframe.
            // decoded_at_ms is 0 (never decoded); gliff-server never reads
            // the field (only frame_id, session.rs:499-503).
            return vec![self.ack(frame_id, 0), self.request_keyframe()];
        }
        let geometry = self.current_geometry();
        self.in_decoder.push_back((frame_id, geometry));
        let mut actions = Vec::new();
        if std::mem::take(&mut self.resyncing) {
            actions.push(Action::Ui(UiEvent::Resynced));
        }
        actions.push(Action::Decode {
            frame_id,
            main,
            aux,
        });
        actions
    }

    fn resize(&mut self, req: ResizeReq) -> Option<Action> {
        if self.last_resize == Some(req) {
            return None;
        }
        self.last_resize = Some(req);
        self.stats.resizes_sent += 1;
        Some(send(req.to_msg()))
    }

    fn ack(&mut self, frame_id: u64, now_ms: u64) -> Action {
        self.stats.acks += 1;
        send(ClientMsg::FrameAck {
            frame_id,
            decoded_at_ms: now_ms,
        })
    }

    fn request_keyframe(&mut self) -> Action {
        self.stats.keyframe_requests += 1;
        send(ClientMsg::RequestKeyframe)
    }

    /// Pops the oldest queued unit for a released picture; with nothing
    /// queued it takes the current geometry and `fallback_id`
    /// (net.rs:618-628).
    fn present(&mut self, frame: YuvFrame, fallback_id: u64) -> Action {
        let (frame_id, geometry) = self
            .in_decoder
            .pop_front()
            .unwrap_or_else(|| (fallback_id, self.current_geometry()));
        self.stats.presented += 1;
        Action::Present(Picture {
            frame,
            geometry,
            frame_id,
        })
    }

    fn current_geometry(&self) -> Geometry {
        // Streaming implies a config; the zero geometry is unreachable.
        self.geometry.unwrap_or(Geometry {
            stream: (0, 0),
            view: (0, 0),
            scale_milli: 0,
        })
    }

    fn end(&mut self, reason: EndReason) -> Vec<Action> {
        self.phase = Phase::Ended;
        vec![Action::Ui(UiEvent::Ended(reason))]
    }
}

fn key_actions(out: KeyOut) -> Vec<Action> {
    let mut actions: Vec<Action> = out.msgs.into_iter().map(send).collect();
    if out.released {
        actions.push(Action::Ui(UiEvent::GrabReleased));
    }
    actions
}

fn send(msg: ClientMsg) -> Action {
    Action::Send(msg, Bytes::new())
}

/// Only what `client_caps` advertised, at a size the decoder takes
/// (net.rs:769-774).
fn validate_stream(
    codec: Codec,
    chroma: ChromaMode,
    (width, height): (u32, u32),
    colour: Colour,
) -> Result<(), String> {
    if codec != Codec::H264 || !colour.offered().contains(&chroma) {
        let offered = match colour {
            Colour::Standard => "Single420",
            Colour::Sharp => "Dual420 and Single420",
        };
        return Err(format!(
            "unsupported stream {codec:?}/{chroma:?}; the client advertised H264/{offered} only"
        ));
    }
    let fits = width > 0
        && height > 0
        && width <= MAX_DECODE.0
        && height <= MAX_DECODE.1
        && width.is_multiple_of(2)
        && height.is_multiple_of(2);
    if fits {
        Ok(())
    } else {
        Err(format!(
            "invalid stream dimensions {width}x{height}; expected even dimensions within {}x{}",
            MAX_DECODE.0, MAX_DECODE.1
        ))
    }
}

/// The variant name, for errors (the payload can be large).
/// The most characters of a server's error text that are kept: the text goes
/// to the log and the screen, and a server may send a megabyte of it.
const MAX_ERROR_CHARS: usize = 512;

/// The server's error text cut to [`MAX_ERROR_CHARS`], with control
/// characters (a line break could fake a log line) shown as `?`.
fn bounded_message(message: &str) -> String {
    let mut out: String = message
        .chars()
        .take(MAX_ERROR_CHARS)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if message.chars().nth(MAX_ERROR_CHARS).is_some() {
        out.push('…');
    }
    out
}

/// Whether the server message is one of the five clipboard messages.
fn is_clipboard(msg: &ServerMsg) -> bool {
    matches!(
        msg,
        ServerMsg::ClipboardOffer { .. }
            | ServerMsg::ClipboardRequest { .. }
            | ServerMsg::ClipboardData { .. }
            | ServerMsg::ClipboardAck { .. }
            | ServerMsg::ClipboardAbort { .. }
    )
}

fn name(msg: &ServerMsg) -> &'static str {
    match msg {
        ServerMsg::HelloAck { .. } => "HelloAck",
        ServerMsg::StreamConfig { .. } => "StreamConfig",
        ServerMsg::VideoFrame { .. } => "VideoFrame",
        ServerMsg::CursorShape { .. } => "CursorShape",
        ServerMsg::CursorPos { .. } => "CursorPos",
        ServerMsg::Pong { .. } => "Pong",
        ServerMsg::Ping { .. } => "Ping",
        ServerMsg::Error { .. } => "Error",
        _ => "a clipboard message",
    }
}

#[cfg(test)]
mod tests;
