//! `Core` behaviour, one upstream rule per test.

use gliff_proto::msg::OutputInfo;

use super::Action;
use super::Bytes;
use super::ChromaMode;
use super::ClientMsg;
use super::ClipboardMsg;
use super::Codec;
use super::Colour;
use super::Core;
use super::DecodeError;
use super::Decoded;
use super::DecoderChange;
use super::EndReason;
use super::Geometry;
use super::PROTOCOL_VERSION;
use super::Picture;
use super::ResizeReq;
use super::ServerMsg;
use super::SessionInfo;
use super::StreamInfo;
use super::UiCmd;
use super::UiEvent;
use super::VideoPipeline;
use super::YuvFrame;

const PANEL: ResizeReq = ResizeReq {
    width: 3840,
    height: 2400,
    scale: 1.0,
};

fn hello_ack(version: u16) -> ServerMsg {
    hello_ack_of(version, true, "gliff-1234")
}

fn hello_ack_of(version: u16, headless: bool, output: &str) -> ServerMsg {
    ServerMsg::HelloAck {
        version,
        session: SessionInfo {
            headless,
            output: output.to_owned(),
        },
        outputs: vec![OutputInfo {
            name: output.to_owned(),
            width: 1920,
            height: 1080,
            scale_milli: 1000,
        }],
        features: Vec::new(),
    }
}

fn config(stream: (u32, u32), view: (u32, u32), scale_milli: u32, fps_cap: u32) -> ServerMsg {
    ServerMsg::StreamConfig {
        codec: Codec::H264,
        chroma: ChromaMode::Single420,
        pipeline: Some(VideoPipeline::Gpu),
        width: stream.0,
        height: stream.1,
        scale_milli,
        extradata: Vec::new(),
        aux_extradata: None,
        view_width: view.0,
        view_height: view.1,
        fps_cap,
    }
}

fn frame(frame_id: u64) -> ServerMsg {
    ServerMsg::VideoFrame {
        frame_id,
        pts_us: frame_id * 16_667,
        keyframe: frame_id == 0,
        damage: Vec::new(),
        data_len: 4,
        aux_len: 0,
    }
}

fn au() -> Bytes {
    Bytes::from_static(&[0, 0, 0, 1])
}

fn picture(width: u32) -> YuvFrame {
    YuvFrame::solid((width, 2), [16, 128, 128]).expect("an even test picture")
}

fn sw_error() -> DecodeError {
    DecodeError::OpenH264(openh264::Error::msg("test"))
}

fn done(frame_id: u64, result: Result<Option<YuvFrame>, DecodeError>) -> Decoded {
    Decoded {
        frame_id,
        result,
        decode_ms: 2.0,
        now_ms: 1_000 + frame_id,
    }
}

fn ack(frame_id: u64) -> Action {
    Action::Send(
        ClientMsg::FrameAck {
            frame_id,
            decoded_at_ms: 1_000 + frame_id,
        },
        Bytes::new(),
    )
}

fn send(msg: ClientMsg) -> Action {
    Action::Send(msg, Bytes::new())
}

const G1080: Geometry = Geometry {
    stream: (1920, 1080),
    view: (1920, 1080),
    scale_milli: 1000,
};

/// A core past HelloAck and the first 1920x1080 config.
fn streaming() -> Core {
    let mut core = Core::new(PANEL);
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    let _ = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    core
}

fn ended(actions: &[Action]) -> Option<&EndReason> {
    actions.iter().find_map(|a| match a {
        Action::Ui(UiEvent::Ended(reason)) => Some(reason),
        _ => None,
    })
}

#[test]
fn hello_ack_reports_connected_then_sends_the_initial_resize() {
    let mut core = Core::new(PANEL);
    let actions = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    assert_eq!(
        actions,
        vec![
            Action::Ui(UiEvent::Connected(SessionInfo {
                headless: true,
                output: "gliff-1234".to_owned(),
            })),
            send(ClientMsg::Resize {
                width: 3840,
                height: 2400,
                scale: 1.0
            }),
        ],
        "HelloAck must surface the session and ask for the panel size"
    );
    assert_eq!(core.stats().resizes_sent, 1, "one Resize counted");
}

#[test]
fn hello_ack_with_another_version_ends_incompatible() {
    let mut core = Core::new(PANEL);
    let actions = core.on_server(hello_ack(PROTOCOL_VERSION + 1), Bytes::new(), Bytes::new());
    assert_eq!(
        ended(&actions),
        Some(&EndReason::Incompatible(
            "Client version is too old".to_owned()
        )),
        "a newer server must end the session as incompatible: {actions:?}"
    );
    assert!(core.is_ended(), "core must be ended");
    assert!(
        !actions.iter().any(|a| matches!(a, Action::Send(..))),
        "nothing may be sent to an incompatible server: {actions:?}"
    );
}

#[test]
fn ping_is_answered_in_every_phase() {
    let mut core = Core::new(PANEL);
    let pong = |t| vec![send(ClientMsg::Pong { t })];
    assert_eq!(
        core.on_server(ServerMsg::Ping { t: 1 }, Bytes::new(), Bytes::new()),
        pong(1),
        "before HelloAck"
    );
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    assert_eq!(
        core.on_server(ServerMsg::Ping { t: 2 }, Bytes::new(), Bytes::new()),
        pong(2),
        "between HelloAck and StreamConfig (net.rs:304-327)"
    );
    let _ = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    assert_eq!(
        core.on_server(ServerMsg::Ping { t: 3 }, Bytes::new(), Bytes::new()),
        pong(3),
        "while streaming"
    );
}

#[test]
fn stream_config_before_hello_ack_ends_the_link() {
    let mut core = Core::new(PANEL);
    let actions = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    assert!(
        matches!(ended(&actions), Some(EndReason::Link(m)) if m.contains("before StreamConfig")),
        "upstream bails on a config before HelloAck (net.rs:328-355): {actions:?}"
    );
}

#[test]
fn first_config_resets_the_decoder_and_reports_geometry() {
    let mut core = Core::new(PANEL);
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    let actions = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    assert_eq!(
        actions,
        vec![
            Action::ResetDecoder(ChromaMode::Single420),
            Action::Ui(UiEvent::StreamConfig(StreamInfo {
                geometry: G1080,
                fps_cap: 30,
                chroma: ChromaMode::Single420,
                pipeline: Some(VideoPipeline::Gpu),
                decoder: DecoderChange::Reset,
            })),
        ],
        "first config"
    );
    assert_eq!(
        (core.stats().configs, core.stats().resets),
        (1, 1),
        "counted"
    );
}

#[test]
fn pace_only_and_scale_only_configs_keep_the_decoder() {
    let mut core = streaming();
    let pace = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 60),
        Bytes::new(),
        Bytes::new(),
    );
    assert!(
        !pace.contains(&Action::ResetDecoder(ChromaMode::Single420)),
        "an fps-only config must not reset (net.rs:676-683): {pace:?}"
    );
    let scale = core.on_server(
        config((1920, 1080), (3840, 2160), 2000, 60),
        Bytes::new(),
        Bytes::new(),
    );
    assert!(
        !scale.contains(&Action::ResetDecoder(ChromaMode::Single420)),
        "a scale-only config must not reset: {scale:?}"
    );
    assert!(
        matches!(
            scale.as_slice(),
            [Action::Ui(UiEvent::StreamConfig(StreamInfo {
                decoder: DecoderChange::Kept,
                fps_cap: 60,
                ..
            }))]
        ),
        "scale-only config reported as kept: {scale:?}"
    );
    assert_eq!(core.stats().resets, 1, "only the first config reset");
}

#[test]
fn a_new_stream_size_resets_and_clears_the_fifo() {
    let mut core = streaming();
    let _ = core.on_server(frame(0), au(), Bytes::new());
    let _ = core.decoded(done(0, Ok(None)));
    let actions = core.on_server(
        config((3840, 2400), (3840, 2400), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    assert_eq!(
        actions.first(),
        Some(&Action::ResetDecoder(ChromaMode::Single420)),
        "size change resets: {actions:?}"
    );
    // The held 1080p unit is gone with the old decoder, so a picture
    // now carries the new geometry.
    let _ = core.on_server(frame(1), au(), Bytes::new());
    let out = core.decoded(done(1, Ok(Some(picture(3840)))));
    assert!(
        matches!(out.as_slice(), [_, Action::Present(Picture { frame_id: 1, geometry, .. })] if geometry.stream == (3840, 2400)),
        "picture after reset must be the new unit's: {out:?}"
    );
}

#[test]
fn invalid_or_unsupported_configs_end_the_link() {
    for (name, msg) in [
        ("odd width", config((1921, 1080), (1921, 1080), 1000, 30)),
        ("too tall", config((1920, 4098), (1920, 4098), 1000, 30)),
        ("zero", config((0, 1080), (0, 1080), 1000, 30)),
        (
            "Dual420 was not advertised",
            ServerMsg::StreamConfig {
                codec: Codec::H264,
                chroma: ChromaMode::Dual420,
                pipeline: None,
                width: 1920,
                height: 1080,
                scale_milli: 1000,
                extradata: Vec::new(),
                aux_extradata: None,
                view_width: 1920,
                view_height: 1080,
                fps_cap: 30,
            },
        ),
        (
            "H265 was not advertised",
            ServerMsg::StreamConfig {
                codec: Codec::H265,
                chroma: ChromaMode::Single420,
                pipeline: None,
                width: 1920,
                height: 1080,
                scale_milli: 1000,
                extradata: Vec::new(),
                aux_extradata: None,
                view_width: 1920,
                view_height: 1080,
                fps_cap: 30,
            },
        ),
    ] {
        let mut core = streaming();
        let actions = core.on_server(msg, Bytes::new(), Bytes::new());
        assert!(
            matches!(ended(&actions), Some(EndReason::Link(_))),
            "{name}: must end the link: {actions:?}"
        );
        assert!(
            !actions.contains(&Action::ResetDecoder(ChromaMode::Single420)),
            "{name}: no reset: {actions:?}"
        );
    }
}

#[test]
fn sharp_colour_decodes_dual420_pairs_and_falls_back_to_single420() {
    let mut core = Core::new(PANEL).with_colour(Colour::Sharp);
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    let dual = ServerMsg::StreamConfig {
        codec: Codec::H264,
        chroma: ChromaMode::Dual420,
        pipeline: Some(VideoPipeline::Gpu),
        width: 1920,
        height: 1080,
        scale_milli: 1000,
        extradata: Vec::new(),
        aux_extradata: Some(Vec::new()),
        view_width: 1920,
        view_height: 1080,
        fps_cap: 30,
    };
    let actions = core.on_server(dual, Bytes::new(), Bytes::new());
    assert!(
        actions.contains(&Action::ResetDecoder(ChromaMode::Dual420)),
        "an offered Dual420 stream gets a Dual420 decoder: {actions:?}"
    );
    assert_eq!(
        core.on_server(frame(1), au(), au()),
        vec![Action::Decode {
            frame_id: 1,
            main: au(),
            aux: au(),
        }],
        "both halves go to the decoder together"
    );
    let half = core.on_server(frame(2), au(), Bytes::new());
    assert!(
        matches!(
            half.as_slice(),
            [
                Action::Send(ClientMsg::FrameAck { frame_id: 2, .. }, _),
                Action::Send(ClientMsg::RequestKeyframe, _)
            ]
        ),
        "half a pair is acked, not decoded, and a keyframe asked for: {half:?}"
    );
    let single = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    assert!(
        single.contains(&Action::ResetDecoder(ChromaMode::Single420)),
        "the server's fall back to Single420 swaps the decoder: {single:?}"
    );
    assert_eq!(ended(&single), None, "falling back keeps the session");
}

#[test]
fn video_frame_is_decoded_and_counted() {
    let mut core = streaming();
    let actions = core.on_server(frame(7), au(), Bytes::new());
    assert_eq!(
        actions,
        vec![Action::Decode {
            frame_id: 7,
            main: au(),
            aux: Bytes::new(),
        }],
        "a Single420 unit goes to the decoder"
    );
    assert_eq!(core.stats().frames, 1, "frame counted");
}

#[test]
fn aux_frame_is_acked_and_a_keyframe_requested_without_decoding() {
    let mut core = streaming();
    let actions = core.on_server(frame(3), au(), au());
    assert!(
        matches!(
            actions.as_slice(),
            [
                Action::Send(ClientMsg::FrameAck { frame_id: 3, .. }, _),
                Action::Send(ClientMsg::RequestKeyframe, _)
            ]
        ),
        "Dual420 aux was not advertised: ack it and ask for a keyframe: {actions:?}"
    );
    assert_eq!(
        (core.stats().frames, core.stats().acks),
        (1, 1),
        "acked once"
    );
}

#[test]
fn decoded_always_acks_first() {
    let mut core = streaming();
    for (id, result) in [
        (0, Ok(None)),
        (1, Ok(Some(picture(1920)))),
        (2, Err(sw_error())),
    ] {
        let _ = core.on_server(frame(id), au(), Bytes::new());
        let out = core.decoded(done(id, result));
        assert_eq!(
            out.first(),
            Some(&ack(id)),
            "frame {id}: the ack must lead: {out:?}"
        );
    }
    assert_eq!(
        (core.stats().frames, core.stats().acks),
        (3, 3),
        "acks == frames"
    );
    assert_eq!(core.stats().decode_ms.count(), 3, "every decode timed");
}

#[test]
fn a_picture_carries_the_geometry_of_its_own_unit() {
    let mut core = streaming();
    let _ = core.on_server(frame(0), au(), Bytes::new());
    // OpenH264 holds the first High-profile picture one unit.
    let first = core.decoded(done(0, Ok(None)));
    assert_eq!(first, vec![ack(0)], "held picture: ack only");
    // A scale-only config lands between the two units.
    let _ = core.on_server(
        config((1920, 1080), (3840, 2160), 2000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    let _ = core.on_server(frame(1), au(), Bytes::new());
    let out = core.decoded(done(1, Ok(Some(picture(1920)))));
    assert_eq!(
        out,
        vec![
            ack(1),
            Action::Present(Picture {
                frame: picture(1920),
                geometry: G1080,
                frame_id: 0,
            })
        ],
        "the released picture is unit 0's, under the old scale (net.rs:463-467)"
    );
    assert_eq!(core.stats().presented, 1, "presented counted");
}

#[test]
fn decode_error_clears_the_fifo_and_requests_a_keyframe() {
    let mut core = streaming();
    let _ = core.on_server(frame(0), au(), Bytes::new());
    let _ = core.decoded(done(0, Ok(None)));
    let _ = core.on_server(frame(1), au(), Bytes::new());
    let out = core.decoded(done(1, Err(sw_error())));
    assert_eq!(
        out,
        vec![ack(1), send(ClientMsg::RequestKeyframe)],
        "ack then keyframe request (net.rs:657-661)"
    );
    assert_eq!(
        (core.stats().decode_errors, core.stats().keyframe_requests),
        (1, 1),
        "counted"
    );
    // FIFO cleared: the next picture takes the current geometry and
    // the id of the unit just fed.
    let _ = core.on_server(frame(2), au(), Bytes::new());
    let next = core.decoded(done(2, Ok(Some(picture(1920)))));
    assert!(
        matches!(
            next.as_slice(),
            [_, Action::Present(Picture { frame_id: 2, .. })]
        ),
        "after an error the FIFO restarts: {next:?}"
    );
}

#[test]
fn drained_pictures_are_presented_but_never_acked() {
    let mut core = streaming();
    let _ = core.on_server(frame(0), au(), Bytes::new());
    let _ = core.decoded(done(0, Ok(None)));
    let out = core.drained(Ok(Some(picture(1920))));
    assert_eq!(
        out,
        vec![Action::Present(Picture {
            frame: picture(1920),
            geometry: G1080,
            frame_id: 0
        })],
        "drain: present only"
    );
    assert_eq!(
        core.drained(Ok(None)),
        Vec::new(),
        "nothing held: nothing to do"
    );
    assert_eq!(
        core.drained(Err(sw_error())),
        Vec::new(),
        "drain error is only counted (net.rs:549-552)"
    );
    assert_eq!(core.stats().drain_errors, 1, "drain error counted");
    assert_eq!(core.stats().acks, 1, "only the decode acked");
}

#[test]
fn resize_is_deduplicated_and_early_requests_replace_the_initial() {
    let mut core = Core::new(PANEL);
    let small = ResizeReq {
        width: 2560,
        height: 1600,
        scale: 1.0,
    };
    assert_eq!(
        core.on_ui(UiCmd::Resize(small)),
        Vec::new(),
        "nothing is sent before HelloAck"
    );
    let actions = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    assert!(
        actions.contains(&send(small.to_msg())),
        "the latest request is the initial one: {actions:?}"
    );
    assert_eq!(
        core.on_ui(UiCmd::Resize(small)),
        Vec::new(),
        "same size: deduped"
    );
    assert_eq!(
        core.on_ui(UiCmd::Resize(PANEL)),
        vec![send(PANEL.to_msg())],
        "new size sent"
    );
    assert_eq!(core.stats().resizes_sent, 2, "two Resizes");
}

#[test]
fn a_resync_sends_new_sizes_and_clears_modifiers_the_user_does_not_hold() {
    let mut core = streaming();
    let narrower = ResizeReq {
        width: PANEL.width - ResizeReq::NUDGE,
        ..PANEL
    };
    // The user holds Left Shift (evdev 42); the remote may have lost the Up of
    // any other modifier while it stalled.
    core.on_ui(UiCmd::Key(crate::altgr::RawKey {
        vk: 0xa0,
        scan: 0x2a,
        prefix: crate::keymap::Prefix::None,
        press: crate::keymap::Press::Down,
        time_ms: 0,
        origin: crate::altgr::Origin::Hook,
        injected: false,
    }));
    let up = |code| {
        send(ClientMsg::Key {
            keycode: code,
            pressed: false,
        })
    };
    let ups = || -> Vec<Action> { [29, 97, 54, 56, 100, 125, 126].map(up).into() };
    let mut expected = vec![
        Action::Ui(UiEvent::Resyncing),
        send(narrower.to_msg()),
        send(PANEL.to_msg()),
    ];
    expected.extend(ups());
    assert_eq!(core.resync(), expected);
    assert_eq!(core.stats().resyncs, 1);
    assert_eq!(
        core.stats().resizes_sent,
        3,
        "initial plus the two of the resync"
    );
    assert_eq!(
        core.on_ui(UiCmd::Resize(PANEL)),
        Vec::new(),
        "the real size is still the one on record, so a window re-send is deduped"
    );
    assert_eq!(
        core.on_ui(UiCmd::ClearModifiers),
        ups(),
        "the same Ups after a hook reinstall; the held Left Shift gets none"
    );
}

/// A mirrored session: the server's output is a real one.
fn mirroring() -> Core {
    let mut core = Core::new(PANEL);
    let _ = core.on_server(
        hello_ack_of(PROTOCOL_VERSION, false, "HDMI-A-1"),
        Bytes::new(),
        Bytes::new(),
    );
    let _ = core.on_server(
        config((1920, 1080), (1920, 1080), 1000, 30),
        Bytes::new(),
        Bytes::new(),
    );
    core
}

#[test]
fn a_mirrored_screen_has_nothing_to_resync() {
    let mut core = mirroring();
    assert_eq!(core.resync(), Vec::new(), "no nudged Resize for a mirror");
    assert_eq!(core.stats().resyncs, 0);
    assert_eq!(core.stats().resizes_sent, 1, "only the initial Resize");
    assert_eq!(
        core.on_ui(UiCmd::Resize(PANEL)),
        Vec::new(),
        "the record of the last Resize is untouched"
    );
}

#[test]
fn a_mirrored_session_still_sends_every_new_window_size() {
    let mut core = mirroring();
    let smaller = ResizeReq {
        width: PANEL.width / 2,
        height: PANEL.height / 2,
        ..PANEL
    };
    assert_eq!(
        core.on_ui(UiCmd::Resize(smaller)),
        vec![send(smaller.to_msg())],
        "the server fits the mirrored screen into the new size"
    );
}

#[test]
fn the_first_picture_after_a_resync_reports_resynced_once() {
    let mut core = streaming();
    let _ = core.resync();
    let first = core.on_server(frame(1), au(), Bytes::new());
    assert_eq!(
        first,
        vec![
            Action::Ui(UiEvent::Resynced),
            Action::Decode {
                frame_id: 1,
                main: au(),
                aux: Bytes::new(),
            }
        ]
    );
    let second = core.on_server(frame(2), au(), Bytes::new());
    assert_eq!(
        second,
        vec![Action::Decode {
            frame_id: 2,
            main: au(),
            aux: Bytes::new(),
        }]
    );
}

#[test]
fn there_is_nothing_to_resync_before_a_stream_or_after_the_end() {
    let mut core = Core::new(PANEL);
    assert_eq!(core.resync(), Vec::new(), "before HelloAck");
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    assert_eq!(core.resync(), Vec::new(), "before the first StreamConfig");
    let mut core = streaming();
    let _ = core.on_ui(UiCmd::Bye);
    assert_eq!(core.resync(), Vec::new(), "after the end");
    assert_eq!(core.stats().resyncs, 0);
}

#[test]
fn a_nudge_always_differs_and_stays_wide_enough() {
    let wide = ResizeReq {
        width: 2880,
        height: 1800,
        scale: 2.0,
    };
    assert_eq!(wide.nudged().width, 2848);
    let narrow = ResizeReq { width: 340, ..wide };
    assert_eq!(narrow.nudged().width, 372, "never under 320 px");
    assert_eq!(narrow.nudged().height, 1800, "only the width changes");
    assert_eq!(narrow.nudged().scale, 2.0);
}

#[test]
fn bye_sends_bye_ends_and_ignores_everything_after() {
    let mut core = streaming();
    let actions = core.on_ui(UiCmd::Bye);
    assert_eq!(
        actions,
        vec![
            send(ClientMsg::Bye),
            Action::Ui(UiEvent::Ended(EndReason::Bye))
        ],
        "Bye"
    );
    assert!(core.is_ended(), "ended");
    assert_eq!(
        core.on_server(frame(9), au(), Bytes::new()),
        Vec::new(),
        "late frame ignored"
    );
    assert_eq!(
        core.on_server(ServerMsg::Ping { t: 4 }, Bytes::new(), Bytes::new()),
        Vec::new(),
        "late ping ignored"
    );
    assert_eq!(core.on_ui(UiCmd::Bye), Vec::new(), "second Bye ignored");
}

#[test]
fn server_errors_end_the_session() {
    let mut core = streaming();
    let actions = core.on_server(
        ServerMsg::Error {
            code: 300,
            message: "capture died".to_owned(),
            version: Some(PROTOCOL_VERSION),
        },
        Bytes::new(),
        Bytes::new(),
    );
    assert_eq!(
        ended(&actions),
        Some(&EndReason::ServerError {
            code: 300,
            message: "capture died".to_owned()
        }),
        "server error"
    );
    // The text is bounded and cannot fake a log line.
    let mut verbose = Core::new(PANEL);
    let actions = verbose.on_server(
        ServerMsg::Error {
            code: 300,
            message: format!("a\nb{}", "x".repeat(1 << 20)),
            version: Some(PROTOCOL_VERSION),
        },
        Bytes::new(),
        Bytes::new(),
    );
    let Some(EndReason::ServerError { message, .. }) = ended(&actions) else {
        panic!("server error");
    };
    assert!(message.starts_with("a?bxx") && message.chars().count() <= 513);
    let mut early = Core::new(PANEL);
    let actions = early.on_server(
        ServerMsg::Error {
            code: 2,
            message: "no chroma".to_owned(),
            version: Some(PROTOCOL_VERSION),
        },
        Bytes::new(),
        Bytes::new(),
    );
    assert_eq!(
        ended(&actions),
        Some(&EndReason::Incompatible("no chroma".to_owned())),
        "a code below 256 is incompatible"
    );
}

#[test]
fn hello_ack_mid_stream_is_ignored() {
    let mut core = streaming();
    assert_eq!(
        core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new()),
        Vec::new(),
        "net.rs:722"
    );
}

#[test]
fn video_frame_before_stream_config_ends_the_link() {
    let mut core = Core::new(PANEL);
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    let actions = core.on_server(frame(0), au(), Bytes::new());
    assert!(
        matches!(ended(&actions), Some(EndReason::Link(_))),
        "net.rs:355: {actions:?}"
    );
}

#[test]
fn acks_equal_frames_over_a_mixed_run() {
    let mut core = streaming();
    for round in 0..50u64 {
        let id = round;
        let aux = if round % 17 == 5 { au() } else { Bytes::new() };
        for action in core.on_server(frame(id), au(), aux) {
            if let Action::Decode { frame_id, .. } = action {
                let result = match round % 7 {
                    0 => Ok(None),
                    3 => Err(sw_error()),
                    _ => Ok(Some(picture(1920))),
                };
                let _ = core.decoded(done(frame_id, result));
            }
        }
        if round % 11 == 0 {
            let _ = core.on_server(
                config(
                    (1920, 1080),
                    (1920, 1080),
                    1000,
                    30 + (round as u32 % 2) * 30,
                ),
                Bytes::new(),
                Bytes::new(),
            );
            let _ = core.drained(Ok(Some(picture(1920))));
        }
    }
    let stats = core.stats();
    assert_eq!(stats.frames, 50, "frames");
    assert_eq!(
        stats.acks, stats.frames,
        "every frame acked exactly once: {stats:?}"
    );
}

#[test]
fn a_cursor_shape_while_streaming_reaches_the_ui_with_its_pixels() {
    let mut core = streaming();
    let argb = Bytes::from_static(&[0, 0, 0, 255, 1, 2, 3, 0]);
    let msg = ServerMsg::CursorShape {
        id: 7,
        width: 2,
        height: 1,
        hot_x: 1,
        hot_y: 0,
        argb_len: 8,
    };
    assert_eq!(
        core.on_server(msg, argb.clone(), Bytes::new()),
        vec![Action::Ui(UiEvent::Cursor(
            crate::cursor_image::CursorImage {
                id: 7,
                width: 2,
                height: 1,
                hot: (1, 0),
                argb,
            }
        ))],
        "main.rs:1270: the shape and hot spot go to the window"
    );
}

#[test]
fn cursor_pos_is_ignored() {
    let mut core = streaming();
    let msg = ServerMsg::CursorPos {
        x: 1.0,
        y: 2.0,
        shape_id: 7,
        visible: true,
    };
    assert_eq!(core.on_server(msg, Bytes::new(), Bytes::new()), Vec::new());
}

mod input {
    use gliff_proto::msg::Axis;

    use super::super::KeyPipeline;
    use super::*;
    use crate::altgr::Origin;
    use crate::altgr::RawKey;
    use crate::input::KeyPolicy;
    use crate::keymap::Chord;
    use crate::keymap::Prefix;
    use crate::keymap::Press;
    use crate::pointer::BTN_LEFT;
    use crate::pointer::PointerEvent;

    fn raw(vk: u16, scan: u16, prefix: Prefix, press: Press) -> RawKey {
        RawKey {
            vk,
            scan,
            prefix,
            press,
            time_ms: 0,
            origin: Origin::Hook,
            injected: false,
        }
    }

    fn key_a(press: Press) -> UiCmd {
        UiCmd::Key(raw(0x41, 0x1e, Prefix::None, press))
    }

    fn key(code: u32, pressed: bool) -> Action {
        send(ClientMsg::Key {
            keycode: code,
            pressed,
        })
    }

    #[test]
    fn keys_before_the_hello_ack_are_not_sent() {
        let mut core = Core::new(PANEL);
        assert_eq!(core.on_ui(key_a(Press::Down)), Vec::new(), "not sent");
        assert_eq!(core.on_ui(key_a(Press::Up)), Vec::new(), "not sent");
    }

    #[test]
    fn the_release_chord_works_before_the_hello_ack() {
        let mut core = Core::new(PANEL);
        core.on_ui(UiCmd::Key(raw(0xa2, 0x1d, Prefix::None, Press::Down)));
        core.on_ui(UiCmd::Tick(50));
        core.on_ui(UiCmd::Key(raw(0xa4, 0x38, Prefix::None, Press::Down)));
        let actions = core.on_ui(UiCmd::Key(raw(0x24, 0x47, Prefix::E0, Press::Down)));
        assert_eq!(
            actions,
            vec![Action::Ui(UiEvent::GrabReleased)],
            "the UI is told and nothing is sent to a server that has not answered"
        );
    }

    #[test]
    fn the_release_chord_works_after_the_hello_ack_without_a_stream_config() {
        let mut core = Core::new(PANEL);
        core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
        core.on_ui(UiCmd::Key(raw(0xa2, 0x1d, Prefix::None, Press::Down)));
        core.on_ui(UiCmd::Tick(50));
        core.on_ui(UiCmd::Key(raw(0xa4, 0x38, Prefix::None, Press::Down)));
        let actions = core.on_ui(UiCmd::Key(raw(0x24, 0x47, Prefix::E0, Press::Down)));
        assert_eq!(actions.last(), Some(&Action::Ui(UiEvent::GrabReleased)));
    }

    #[test]
    fn a_release_from_the_hook_releases_every_key_and_tells_the_ui() {
        let mut core = streaming();
        core.on_ui(key_a(Press::Down));
        let actions = core.on_ui(UiCmd::ReleaseGrab);
        assert_eq!(
            actions,
            vec![key(30, false), Action::Ui(UiEvent::GrabReleased)],
            "the Up first, then the UI hears of it"
        );
        assert_eq!(core.on_ui(key_a(Press::Up)), Vec::new(), "nothing is held");
    }

    /// What the hook does with each key of the grab for a local chord, as
    /// `hook::Ctx::grab` does it: the key goes to the core, to Windows, or
    /// nowhere; a taken chord sends `ReleaseInput` first. Returns the keys
    /// Windows gets, in order, as `(vk, pressed)` (replayed modifiers and the
    /// trigger included, passed real keys too).
    fn through_the_hook(
        core: &mut Core,
        chords: &mut crate::local_chords::LocalChords,
        local: &mut crate::key_ledger::KeyLedger,
        keys: &[(u16, u16, Prefix, Press)],
    ) -> (Vec<(u16, bool)>, Vec<Action>) {
        use crate::local_chords::ChordDecision;
        let mut windows = Vec::new();
        let mut sent = Vec::new();
        for &(vk, scan, prefix, press) in keys {
            let taken = local.route(vk, press) != crate::key_ledger::Route::Remote;
            match chords.push(vk, press, taken) {
                _ if taken => windows.push((vk, press == Press::Down)),
                ChordDecision::Forward => {
                    sent.extend(core.on_ui(UiCmd::Key(raw(vk, scan, prefix, press))));
                }
                ChordDecision::Swallow => {}
                ChordDecision::Take { modifiers, trigger } => {
                    sent.extend(core.on_ui(UiCmd::ReleaseInput));
                    for m in modifiers {
                        local.windows_saw(m, Press::Down);
                        windows.push((m, true));
                    }
                    windows.push((trigger, true));
                    windows.push((trigger, false));
                }
            }
        }
        (windows, sent)
    }

    #[test]
    fn a_local_chord_stays_on_windows_and_leaves_nothing_held_on_the_remote() {
        const LWIN: (u16, u16, Prefix) = (0x5b, 0x5b, Prefix::E0);
        const LSHIFT: (u16, u16, Prefix) = (0xa0, 0x2a, Prefix::None);
        const S: (u16, u16, Prefix) = (0x53, 0x1f, Prefix::None);
        let k = |(vk, scan, prefix): (u16, u16, Prefix), press| (vk, scan, prefix, press);
        let mut core = streaming();
        let mut chords =
            crate::local_chords::LocalChords::new(vec![crate::local_chords::LocalChord::snip()]);
        let mut local = crate::key_ledger::KeyLedger::default();
        let (windows, sent) = through_the_hook(
            &mut core,
            &mut chords,
            &mut local,
            &[
                k(LWIN, Press::Down),
                k(LSHIFT, Press::Down),
                k(S, Press::Down),
                k(S, Press::Down),
                k(S, Press::Up),
                k(LSHIFT, Press::Up),
                k(LWIN, Press::Up),
            ],
        );
        // The remote saw the two modifiers go down, then let go of both when
        // the chord completed, and never saw S or the real Ups.
        assert_eq!(
            sent,
            vec![
                key(125, true),
                key(42, true),
                key(42, false),
                key(125, false)
            ]
        );
        assert!(
            !sent
                .iter()
                .any(|a| matches!(a, Action::Send(ClientMsg::Key { keycode: 31, .. }, _))),
            "S never reaches the remote"
        );
        // Windows got the chord replayed once, then the real Ups.
        assert_eq!(
            windows,
            vec![
                (0xa0, true),
                (0x5b, true),
                (0x53, true),
                (0x53, false),
                (0xa0, false),
                (0x5b, false)
            ]
        );
        assert_eq!(
            core.on_ui(UiCmd::ReleaseInput),
            Vec::new(),
            "nothing is held"
        );
    }

    #[test]
    fn a_chord_that_is_not_local_still_reaches_the_remote() {
        let mut core = streaming();
        let mut chords =
            crate::local_chords::LocalChords::new(vec![crate::local_chords::LocalChord::snip()]);
        let mut local = crate::key_ledger::KeyLedger::default();
        let (windows, sent) = through_the_hook(
            &mut core,
            &mut chords,
            &mut local,
            &[
                (0x5b, 0x5b, Prefix::E0, Press::Down),
                (0x45, 0x12, Prefix::None, Press::Down),
                (0x45, 0x12, Prefix::None, Press::Up),
                (0x5b, 0x5b, Prefix::E0, Press::Up),
            ],
        );
        assert_eq!(windows, Vec::new());
        assert_eq!(
            sent,
            vec![
                key(125, true),
                key(18, true),
                key(18, false),
                key(125, false)
            ]
        );
    }

    #[test]
    fn a_release_from_the_hook_before_the_hello_ack_only_tells_the_ui() {
        let mut core = Core::new(PANEL);
        core.on_ui(key_a(Press::Down));
        assert_eq!(
            core.on_ui(UiCmd::ReleaseGrab),
            vec![Action::Ui(UiEvent::GrabReleased)]
        );
    }

    #[test]
    fn a_release_of_input_before_the_hello_ack_forgets_the_held_keys_and_sends_nothing() {
        let mut core = Core::new(PANEL);
        core.on_ui(key_a(Press::Down));
        assert_eq!(core.on_ui(UiCmd::ReleaseInput), Vec::new());
        let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
        let _ = core.on_server(
            config((1920, 1080), (1920, 1080), 1000, 30),
            Bytes::new(),
            Bytes::new(),
        );
        assert_eq!(
            core.on_ui(key_a(Press::Up)),
            Vec::new(),
            "no Up for a key the server never saw go down"
        );
    }

    #[test]
    fn a_key_goes_out_as_an_evdev_key_message() {
        let mut core = streaming();
        assert_eq!(core.on_ui(key_a(Press::Down)), vec![key(30, true)], "down");
        assert_eq!(core.on_ui(key_a(Press::Up)), vec![key(30, false)], "up");
    }

    #[test]
    fn the_release_chord_sends_ups_then_tells_the_ui() {
        let mut core = streaming();
        core.on_ui(UiCmd::Key(raw(0xa2, 0x1d, Prefix::None, Press::Down)));
        core.on_ui(UiCmd::Tick(50));
        core.on_ui(UiCmd::Key(raw(0xa4, 0x38, Prefix::None, Press::Down)));
        let actions = core.on_ui(UiCmd::Key(raw(0x24, 0x47, Prefix::E0, Press::Down)));
        assert_eq!(
            actions.last(),
            Some(&Action::Ui(UiEvent::GrabReleased)),
            "the UI hears of it last: {actions:?}"
        );
        assert!(
            actions.contains(&key(29, false)) && actions.contains(&key(56, false)),
            "Ctrl and Alt are released: {actions:?}"
        );
    }

    #[test]
    fn the_chord_set_by_with_pipeline_is_the_one_used() {
        let mut core =
            streaming().with_pipeline(KeyPipeline::new(Chord::ctrl_alt_l(), KeyPolicy::default()));
        core.on_ui(UiCmd::Key(raw(0xa2, 0x1d, Prefix::None, Press::Down)));
        core.on_ui(UiCmd::Tick(50));
        core.on_ui(UiCmd::Key(raw(0xa4, 0x38, Prefix::None, Press::Down)));
        let actions = core.on_ui(UiCmd::Key(raw(0x4c, 0x26, Prefix::None, Press::Down)));
        assert!(
            actions.contains(&Action::Ui(UiEvent::GrabReleased)),
            "Ctrl+Alt+L is now the release chord: {actions:?}"
        );
    }

    #[test]
    fn pointer_events_and_wheel_notches_are_sent() {
        let mut core = streaming();
        assert_eq!(
            core.on_ui(UiCmd::Pointer(PointerEvent::Motion { x: 4.0, y: 5.0 })),
            vec![send(ClientMsg::PointerMotion { x: 4.0, y: 5.0 })],
            "motion"
        );
        assert_eq!(
            core.on_ui(UiCmd::Wheel {
                axis: Axis::Horizontal,
                delta: 120
            }),
            vec![send(ClientMsg::PointerAxis {
                axis: Axis::Horizontal,
                value: 15.0,
                discrete: Some(1),
                stop: false,
            })],
            "one notch right"
        );
    }

    #[test]
    fn wheel_steps_and_button_presses_are_counted_as_sent() {
        let mut core = streaming();
        for delta in [-120, -120, 120] {
            core.on_ui(UiCmd::Wheel {
                axis: Axis::Vertical,
                delta,
            });
        }
        core.on_ui(UiCmd::Wheel {
            axis: Axis::Horizontal,
            delta: -120,
        });
        // A part of a notch is no step.
        core.on_ui(UiCmd::Wheel {
            axis: Axis::Horizontal,
            delta: 40,
        });
        for press in [Press::Down, Press::Up, Press::Down] {
            core.on_ui(UiCmd::Pointer(PointerEvent::Button { button: 278, press }));
        }
        let stats = core.stats();
        let steps: Vec<_> = stats
            .wheel_steps_sent
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        assert_eq!(
            steps,
            [("horizontal-", 1), ("vertical+", 2), ("vertical-", 1)]
        );
        let presses: Vec<_> = stats
            .button_presses_sent
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        assert_eq!(presses, [(278, 2)], "two presses, the release is not one");
    }

    #[test]
    fn release_input_lets_go_of_keys_and_buttons() {
        let mut core = streaming();
        core.on_ui(key_a(Press::Down));
        core.on_ui(UiCmd::Pointer(PointerEvent::Button {
            button: BTN_LEFT,
            press: Press::Down,
        }));
        assert_eq!(
            core.on_ui(UiCmd::ReleaseInput),
            vec![
                key(30, false),
                send(ClientMsg::PointerButton {
                    button: BTN_LEFT,
                    pressed: false
                }),
            ],
            "A and the left button"
        );
        assert_eq!(core.on_ui(UiCmd::ReleaseInput), Vec::new(), "idempotent");
    }

    #[test]
    fn the_stats_count_what_is_held_and_a_release_brings_it_to_zero() {
        let mut core = streaming();
        assert_eq!((core.stats().keys_down, core.stats().buttons_down), (0, 0));
        core.on_ui(key_a(Press::Down));
        core.on_ui(UiCmd::Pointer(PointerEvent::Button {
            button: BTN_LEFT,
            press: Press::Down,
        }));
        assert_eq!((core.stats().keys_down, core.stats().buttons_down), (1, 1));
        core.on_ui(UiCmd::ReleaseInput);
        assert_eq!((core.stats().keys_down, core.stats().buttons_down), (0, 0));
    }

    #[test]
    fn a_release_while_connecting_also_brings_the_count_to_zero() {
        let mut core = Core::new(PANEL);
        core.on_ui(key_a(Press::Down));
        core.on_ui(UiCmd::ReleaseInput);
        assert_eq!(core.stats().keys_down, 0);
    }

    #[test]
    fn bye_releases_what_is_held_before_it_says_goodbye() {
        let mut core = streaming();
        core.on_ui(key_a(Press::Down));
        assert_eq!(
            core.on_ui(UiCmd::Bye),
            vec![
                key(30, false),
                send(ClientMsg::Bye),
                Action::Ui(UiEvent::Ended(EndReason::Bye)),
            ],
            "Up, Bye, Ended"
        );
    }

    #[test]
    fn a_new_layout_is_sent_as_a_keymap_message() {
        let mut core = streaming();
        assert_eq!(
            core.on_ui(UiCmd::Keymap("xkb text".to_owned())),
            vec![send(ClientMsg::Keymap {
                keymap: "xkb text".to_owned()
            })],
            "keymap"
        );
    }

    fn keymap(text: &str) -> ClientMsg {
        ClientMsg::Keymap {
            keymap: text.to_owned(),
        }
    }

    #[test]
    fn a_layout_sent_before_the_hello_ack_goes_out_with_it() {
        let mut core = Core::new(PANEL);
        assert_eq!(
            core.on_ui(UiCmd::Keymap("first".to_owned())),
            Vec::new(),
            "the server has not accepted the session yet"
        );
        let actions = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
        assert!(
            actions.contains(&send(keymap("first"))),
            "the layout is sent once the session is accepted: {actions:?}"
        );
    }

    #[test]
    fn only_the_latest_layout_from_before_the_hello_ack_is_sent() {
        let mut core = Core::new(PANEL);
        core.on_ui(UiCmd::Keymap("first".to_owned()));
        core.on_ui(UiCmd::Keymap("second".to_owned()));
        let actions = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
        let sent: Vec<&Action> = actions
            .iter()
            .filter(|a| matches!(a, Action::Send(ClientMsg::Keymap { .. }, _)))
            .collect();
        assert_eq!(sent, vec![&send(keymap("second"))], "{actions:?}");
    }

    #[test]
    fn no_layout_before_the_hello_ack_sends_none_after_it() {
        let mut core = Core::new(PANEL);
        let actions = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, Action::Send(ClientMsg::Keymap { .. }, _))),
            "{actions:?}"
        );
    }
}

#[test]
fn clipboard_messages_go_to_the_engine_with_their_payload() {
    let mut core = streaming();
    let data = ServerMsg::ClipboardData {
        id: 3,
        offset: 0,
        data_len: 5,
        done: true,
    };
    assert_eq!(
        core.on_server(data, Bytes::from_static(b"hello"), Bytes::new()),
        vec![Action::Clipboard(
            ClipboardMsg::Data {
                id: 3,
                offset: 0,
                data_len: 5,
                done: true
            },
            Bytes::from_static(b"hello")
        )],
        "data keeps its payload"
    );
    let offer = ServerMsg::ClipboardOffer {
        serial: 7,
        mime_types: vec!["text/plain".to_owned()],
        files: Vec::new(),
    };
    assert_eq!(
        core.on_server(offer, Bytes::new(), Bytes::new()),
        vec![Action::Clipboard(
            ClipboardMsg::Offer {
                serial: 7,
                mime_types: vec!["text/plain".to_owned()],
                files: Vec::new()
            },
            Bytes::new()
        )],
        "an offer has no payload"
    );
}

#[test]
fn a_clipboard_offer_before_the_first_config_does_not_end_the_link() {
    let mut core = Core::new(PANEL);
    let _ = core.on_server(hello_ack(PROTOCOL_VERSION), Bytes::new(), Bytes::new());
    let actions = core.on_server(
        ServerMsg::ClipboardAbort { id: 1 },
        Bytes::new(),
        Bytes::new(),
    );
    assert_eq!(
        actions,
        vec![Action::Clipboard(
            ClipboardMsg::Abort { id: 1 },
            Bytes::new()
        )],
        "taken after HelloAck"
    );
    let mut fresh = Core::new(PANEL);
    let actions = fresh.on_server(
        ServerMsg::ClipboardAbort { id: 1 },
        Bytes::new(),
        Bytes::new(),
    );
    assert!(
        ended(&actions).is_some(),
        "before HelloAck is still a protocol error"
    );
}
