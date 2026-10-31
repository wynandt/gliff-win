//! `gwin-verify ssh`: start the server exactly as the client does, run the
//! opening of a session the way upstream's server orders it (HelloAck,
//! Ping, StreamConfig), say Bye, close stdin, and require a clean exit.
//!
//! Closing stdin is part of ending the session, not a courtesy: upstream's
//! server returns from its session on Bye but only exits at stdin EOF (see
//! [`session`]), so a client that keeps stdin open waits forever.

use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use gliff_proto::msg::ClientMsg;
use gliff_proto::msg::PROTOCOL_VERSION;
use gliff_proto::msg::ServerMsg;
use gliff_transport::Framed;
use gliff_windows::cli::DesktopMode;
use gliff_windows::cli::FOCUSED_OUTPUT;
use gliff_windows::link::read_server;
use gliff_windows::session::Colour;
use gliff_windows::session::client_caps;
use gliff_windows::session::hello;
use gliff_windows::ssh::SshSpec;
use gliff_windows::ssh::diagnose;
use gliff_windows::ssh::spawn;
use gliff_windows::ssh::ssh_args;
use serde_json::Value;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;

use crate::decode_check::PANEL;
use crate::line::Line;
use crate::line::round2;

/// What the session told us, filled in as it arrives so a timeout can
/// still report how far it got.
#[derive(Default)]
struct Progress {
    version: Option<u16>,
    headless: Option<bool>,
    output: Option<String>,
    features: Vec<String>,
    handshake_ms: Option<f64>,
    ponged: bool,
    stream: Option<String>,
    bye_sent: bool,
    exit_success: Option<bool>,
    exit_code: Option<i32>,
}

impl Progress {
    fn stage(&self) -> &'static str {
        if self.exit_success.is_some() {
            "exited"
        } else if self.bye_sent {
            "waiting for exit after Bye"
        } else if self.stream.is_some() {
            "StreamConfig"
        } else if self.ponged {
            "Pong sent"
        } else if self.version.is_some() {
            "HelloAck"
        } else {
            "Hello sent"
        }
    }

    fn mode_matches(&self, mode: &DesktopMode) -> bool {
        match mode {
            DesktopMode::Headless => self.headless == Some(true),
            // `auto` is the server's choice of a real output: any name that is
            // not a private screen matches.
            DesktopMode::Mirror { output } if output == FOCUSED_OUTPUT => {
                self.headless == Some(false)
            }
            DesktopMode::Mirror { output } => {
                self.headless == Some(false) && self.output.as_deref() == Some(output.as_str())
            }
        }
    }
}

pub async fn run(spec: SshSpec, timeout: Duration) -> Line {
    let args: Vec<String> = ssh_args(&spec)
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let line = Line::new("ssh", false)
        .with("target", spec.target.to_string())
        .with("server_path", spec.server_path.clone())
        .with("ssh", spec.ssh.display().to_string())
        .with("args", args)
        .with("batch_mode", true)
        .with("no_console", cfg!(windows));
    let started = Instant::now();
    let link = match spawn(&spec) {
        Ok(link) => link,
        Err(e) => return line.with("error", format!("spawn: {e}")),
    };
    let mut child = link.child;
    let mut progress = Progress::default();
    let result = tokio::time::timeout(
        timeout,
        session(link.stdin, link.stdout, &mut child, &mut progress, started),
    )
    .await;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let error = match result {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(_) => Some(format!("timed out after {}s", timeout.as_secs())),
    };
    if error.is_some() {
        // tokio's kill also waits, so ssh has exited and its log is whole.
        let _ = child.kill().await;
        // An ssh that had already exited (a missing server, exit 127) keeps
        // its code; one the kill ended has none.
        if progress.exit_code.is_none() {
            progress.exit_code = child.try_wait().ok().flatten().and_then(|s| s.code());
        }
    }
    let mode_ok = progress.mode_matches(&spec.mode);
    let ok = error.is_none()
        && progress.version == Some(PROTOCOL_VERSION)
        && mode_ok
        && progress.ponged
        && progress.stream.is_some()
        && progress.exit_success == Some(true);
    let mut line = line
        .with("mode_ok", mode_ok)
        .with("stage", progress.stage())
        .with(
            "handshake_ms",
            progress.handshake_ms.map_or(Value::Null, round2),
        )
        .with(
            "server_version",
            progress.version.map_or(Value::Null, Value::from),
        )
        .with(
            "headless",
            progress.headless.map_or(Value::Null, Value::from),
        )
        .with("output", progress.output.map_or(Value::Null, Value::from))
        .with("features", progress.features)
        .with("pong", progress.ponged)
        .with("stream", progress.stream.map_or(Value::Null, Value::from))
        .with(
            "exit_after_bye",
            progress.exit_success.map_or(Value::Null, Value::from),
        )
        .with("elapsed_ms", round2(elapsed_ms));
    line.ok = ok;
    match error {
        Some(error) => fail(line, &link.stderr_log, progress.exit_code, error),
        None => line,
    }
}

fn fail(line: Line, stderr_log: &Path, exit: Option<i32>, error: String) -> Line {
    line.with("error", error)
        .with("failure", format!("{:?}", diagnose(stderr_log, exit)))
        .with("stderr_log", stderr_log.display().to_string())
}

/// Hello, then read until HelloAck, Ping (answered with Pong at once, as
/// upstream asks) and StreamConfig have all arrived; Bye; close stdin;
/// drain the pipe to EOF so a server still writing is never blocked; wait
/// for the exit.
///
/// Takes `stdin` by value so it can be closed: upstream gliff-server
/// (089662fd) breaks its loop on Bye (`session.rs:498`, `:320`) and tears
/// down capture, input and the output (`:326-339`), but its `spawn_local`
/// reader (`session.rs:376-413`) has by then started another read on
/// `tokio::io::stdin()` (`main.rs:128`), a blocking read tokio cannot
/// cancel, and dropping the runtime (`main.rs:110-133`) waits for it. With
/// stdin open the server never exits and never closes stdout.
async fn session(
    stdin: ChildStdin,
    stdout: ChildStdout,
    child: &mut Child,
    progress: &mut Progress,
    started: Instant,
) -> anyhow::Result<()> {
    let mut writer = Framed::new(stdin);
    let mut reader = Framed::new(stdout);
    writer
        .write_msg(&hello(
            client_caps((PANEL.width, PANEL.height), Colour::Standard),
            String::new(),
        ))
        .await?;
    while !(progress.version.is_some() && progress.ponged && progress.stream.is_some()) {
        let inbound = read_server(&mut reader).await?;
        match inbound.msg {
            ServerMsg::HelloAck {
                version,
                session,
                features,
                ..
            } => {
                progress.handshake_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
                progress.version = Some(version);
                progress.headless = Some(session.headless);
                progress.output = Some(session.output);
                progress.features = features;
            }
            ServerMsg::Ping { t } => {
                writer.write_msg(&ClientMsg::Pong { t }).await?;
                progress.ponged = true;
            }
            ServerMsg::StreamConfig { width, height, .. } => {
                progress.stream = Some(format!("{width}x{height}"));
            }
            ServerMsg::Error { code, message, .. } => {
                anyhow::bail!("server error {code}: {message}")
            }
            _ => {}
        }
    }
    writer.write_msg(&ClientMsg::Bye).await?;
    progress.bye_sent = true;
    // EOF on the server's stdin: ssh forwards it as channel EOF.
    drop(writer);
    loop {
        match read_server(&mut reader).await {
            Ok(_) => {}
            Err(gliff_transport::Error::Closed) => break,
            Err(e) => return Err(e.into()),
        }
    }
    let status = child.wait().await?;
    progress.exit_success = Some(status.success());
    progress.exit_code = status.code();
    Ok(())
}
