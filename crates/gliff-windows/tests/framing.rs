//! A `VideoFrame` with a real H.264 access unit goes through upstream
//! `Framed` and comes back out of the client's reader byte for byte, then
//! decodes to a picture of the encoded size.

use gliff_proto::msg::Rect;
use gliff_proto::msg::ServerMsg;
use gliff_sw::Encoder;
use gliff_sw::EncoderSettings;
use gliff_transport::Framed;
use gliff_windows::decode::CpuDecoder;
use gliff_windows::link::Inbound;
use gliff_windows::link::read_server;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

/// One keyframe from the same OpenH264 encoder the server's CPU path uses.
fn real_access_unit() -> Vec<u8> {
    let mut encoder = Encoder::new(
        EncoderSettings {
            width: WIDTH,
            height: HEIGHT,
            bitrate: 2_000_000,
            framerate: 30,
        },
        false,
    )
    .expect("OpenH264 encoder for 640x360");
    let bgra: Vec<u8> = (0..WIDTH * HEIGHT)
        .flat_map(|i| {
            let (x, y) = (i % WIDTH, i / WIDTH);
            [(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]
        })
        .collect();
    let frame = encoder.encode_bgra(&bgra, true).expect("encode one frame");
    assert!(frame.keyframe, "a forced keyframe is an IDR");
    assert!(frame.aux.is_none(), "Single420 has no aux stream");
    frame.main
}

#[tokio::test]
async fn video_frame_with_real_payload_round_trips_and_decodes() {
    let au = real_access_unit();
    let sent = ServerMsg::VideoFrame {
        frame_id: 7,
        pts_us: 233_333,
        keyframe: true,
        damage: vec![Rect {
            x: 0,
            y: 0,
            width: WIDTH as i32,
            height: HEIGHT as i32,
        }],
        data_len: u32::try_from(au.len()).expect("one AU fits in u32"),
        aux_len: 0,
    };
    // A small pipe forces the reader to see the AU in several pieces.
    let (server_end, client_end) = tokio::io::duplex(4096);
    let writer = tokio::spawn({
        let sent = sent.clone();
        let au = au.clone();
        async move {
            let mut server = Framed::new(server_end);
            server
                .write_msg_with_payloads(&sent, &[&au])
                .await
                .expect("write VideoFrame");
            server
                .write_msg(&ServerMsg::Ping { t: 1 })
                .await
                .expect("write Ping");
        }
    });

    let mut client = Framed::new(client_end);
    let Inbound { msg, main, aux } = read_server(&mut client).await.expect("read VideoFrame");
    assert_eq!(msg, sent);
    assert_eq!(main.len(), au.len(), "the whole AU is read");
    assert!(main[..] == au[..], "AU bytes are unchanged");
    assert!(aux.is_empty(), "aux_len 0 reads no aux payload");

    // The next message starts exactly after the payload.
    let next = read_server(&mut client).await.expect("read Ping");
    assert_eq!(next.msg, ServerMsg::Ping { t: 1 });
    assert!(next.main.is_empty());
    writer.await.expect("writer task");

    let mut decoder = CpuDecoder::new().expect("OpenH264 decoder");
    let picture = decoder
        .decode(&main)
        .expect("decode the AU")
        .expect("a Baseline IDR is released at once");
    assert_eq!(picture.size(), (WIDTH, HEIGHT));
    assert_eq!(picture.y().len(), (WIDTH * HEIGHT) as usize, "tight luma");
}
