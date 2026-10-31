//! Reading the server's messages off the ssh pipe, payloads included.

use bytes::Bytes;
use gliff_proto::clipboard::CHUNK;
use gliff_proto::msg::ServerMsg;
use gliff_transport::Framed;
use tokio::io::AsyncRead;

/// One server message with the payload bytes it declared.
#[derive(Debug)]
pub struct Inbound {
    pub msg: ServerMsg,
    /// `VideoFrame` main stream, `CursorShape` ARGB, or `ClipboardData`.
    pub main: Bytes,
    /// `VideoFrame` aux stream; empty unless `aux_len > 0`.
    pub aux: Bytes,
}

/// Reads one message and the payload it declares, as upstream
/// `gliff/src/net.rs` does inline. A message that declares no payload gets
/// empty `main` and `aux`. Not cancel-safe once the body is read: race it
/// in a `select!` only as a whole task.
pub async fn read_server<S: AsyncRead + Unpin>(
    reader: &mut Framed<S>,
) -> Result<Inbound, gliff_transport::Error> {
    let msg = reader.read_msg::<ServerMsg>().await?;
    let (main_len, aux_len) = payload_lens(&msg);
    // The transport lets a payload be 64 MiB; a message declaring more than
    // it can carry is refused before its payload is read.
    let limit = payload_limit(&msg);
    if let Some(over) = [main_len, aux_len].into_iter().find(|&n| n > limit) {
        return Err(gliff_transport::Error::PayloadTooLarge(over));
    }
    let main = read_payload(reader, main_len).await?;
    let aux = read_payload(reader, aux_len).await?;
    Ok(Inbound { msg, main, aux })
}

/// The payload lengths a message declares: (main, aux).
fn payload_lens(msg: &ServerMsg) -> (u32, u32) {
    match msg {
        ServerMsg::VideoFrame {
            data_len, aux_len, ..
        } => (*data_len, *aux_len),
        ServerMsg::CursorShape { argb_len, .. } => (*argb_len, 0),
        ServerMsg::ClipboardData { data_len, .. } => (*data_len, 0),
        _ => (0, 0),
    }
}

/// The largest payload one message of this kind may declare: a clipboard
/// chunk, a cursor picture, or a video frame (a large keyframe is some
/// megabytes).
fn payload_limit(msg: &ServerMsg) -> u32 {
    match msg {
        ServerMsg::VideoFrame { .. } => 16 << 20,
        ServerMsg::CursorShape { .. } => 4 << 20,
        ServerMsg::ClipboardData { .. } => u32::try_from(CHUNK).unwrap_or(u32::MAX),
        _ => 0,
    }
}

async fn read_payload<S: AsyncRead + Unpin>(
    reader: &mut Framed<S>,
    len: u32,
) -> Result<Bytes, gliff_transport::Error> {
    if len == 0 {
        return Ok(Bytes::new());
    }
    reader.read_payload(len).await
}

#[cfg(test)]
mod tests {
    use gliff_proto::msg::ServerMsg;
    use gliff_transport::Framed;

    use super::read_server;

    async fn round_trip(msg: ServerMsg, payloads: &[&[u8]]) -> super::Inbound {
        let (a, b) = tokio::io::duplex(1 << 16);
        let mut server = Framed::new(a);
        server
            .write_msg_with_payloads(&msg, payloads)
            .await
            .expect("write");
        let mut client = Framed::new(b);
        let got = read_server(&mut client).await.expect("read");
        assert_eq!(got.msg, msg);
        got
    }

    #[tokio::test]
    async fn dual420_frame_reads_main_then_aux() {
        let msg = ServerMsg::VideoFrame {
            frame_id: 1,
            pts_us: 0,
            keyframe: false,
            damage: Vec::new(),
            data_len: 3,
            aux_len: 2,
        };
        let got = round_trip(msg, &[b"abc", b"de"]).await;
        assert_eq!(&got.main[..], b"abc");
        assert_eq!(&got.aux[..], b"de");
    }

    #[tokio::test]
    async fn a_payload_over_the_limit_is_refused_before_it_is_read() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let msg = ServerMsg::CursorShape {
            id: 4,
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            argb_len: 8 << 20,
        };
        Framed::new(a).write_msg(&msg).await.expect("write");
        let err = read_server(&mut Framed::new(b)).await.expect_err("refused");
        assert!(matches!(
            err,
            gliff_transport::Error::PayloadTooLarge(n) if n == 8 << 20
        ));
    }

    #[tokio::test]
    async fn cursor_shape_reads_argb() {
        let msg = ServerMsg::CursorShape {
            id: 4,
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            argb_len: 4,
        };
        let got = round_trip(msg, &[&[1, 2, 3, 4]]).await;
        assert_eq!(&got.main[..], &[1, 2, 3, 4]);
        assert!(got.aux.is_empty());
    }

    #[tokio::test]
    async fn clipboard_data_reads_chunk() {
        let msg = ServerMsg::ClipboardData {
            id: 9,
            offset: 0,
            data_len: 5,
            done: true,
        };
        let got = round_trip(msg, &[b"hello"]).await;
        assert_eq!(&got.main[..], b"hello");
    }

    #[tokio::test]
    async fn payload_free_message_reads_nothing() {
        let got = round_trip(ServerMsg::Ping { t: 3 }, &[]).await;
        assert!(got.main.is_empty() && got.aux.is_empty());
    }
}
