use std::time::Duration;
use std::time::Instant;

use gliff_proto::msg::ServerMsg;
use gliff_transport::Framed;
use gliff_windows::link::read_server;
use gliff_windows::session::Colour;
use gliff_windows::session::client_caps;
use gliff_windows::session::hello;

const SERVER: &str = env!("CARGO_BIN_EXE_gwin-fake-server");
const AFTER_FIRST: Duration = Duration::from_millis(1100);

#[tokio::test]
async fn the_fake_sends_one_ping_at_startup_and_none_on_a_timer() {
    let mut child = tokio::process::Command::new(SERVER)
        .args(["--stdio", "--headless"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start the fake server");
    let mut writer = Framed::new(child.stdin.take().expect("stdin"));
    let mut reader = Framed::new(child.stdout.take().expect("stdout"));
    writer
        .write_msg(&hello(
            client_caps((640, 400), Colour::Standard),
            String::new(),
        ))
        .await
        .expect("Hello");

    let mut pings = 0u32;
    let mut hello_ack = false;
    let mut stream = false;
    let mut window: Option<Instant> = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let slice = match window {
            Some(end) => end.saturating_duration_since(Instant::now()),
            None => deadline.saturating_duration_since(Instant::now()),
        };
        if slice.is_zero() {
            break;
        }
        let inbound = match tokio::time::timeout(slice, read_server(&mut reader)).await {
            Ok(Ok(inbound)) => inbound,
            Ok(Err(e)) => panic!("the fake closed before the window ended: {e}"),
            Err(_) => break,
        };
        match inbound.msg {
            ServerMsg::HelloAck { .. } => hello_ack = true,
            ServerMsg::Ping { .. } => {
                pings += 1;
                if window.is_none() {
                    window = Some(Instant::now() + AFTER_FIRST);
                }
            }
            ServerMsg::StreamConfig { .. } => stream = true,
            _ => {}
        }
    }
    assert!(hello_ack, "HelloAck");
    assert!(stream, "StreamConfig");
    assert_eq!(
        pings,
        1,
        "one startup Ping, and none in the next {} ms",
        AFTER_FIRST.as_millis()
    );
}
