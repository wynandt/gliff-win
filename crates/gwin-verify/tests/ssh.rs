//! `gwin-verify ssh` end to end on the build machine: the real binary, spawning
//! `scripts/fake-ssh`, which runs `gwin-fake-server` locally over stdio.
//! Stands in for ssh.exe to the remote machine; see docs/development.md.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn fake_ssh() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh")
}

fn run(server_path: &str, log_dir: &Path, extra: &[&str]) -> (bool, Value) {
    run_with(server_path, log_dir, extra, "")
}

/// `fake` goes to the fake server as GWIN_FAKE_SERVER (through fake-ssh).
fn run_with(server_path: &str, log_dir: &Path, extra: &[&str], fake: &str) -> (bool, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_gwin-verify"))
        .env("GWIN_FAKE_SERVER", fake)
        .args(["ssh", "user@host", "--server-path", server_path])
        .arg("--ssh")
        .arg(fake_ssh())
        .arg("--log-dir")
        .arg(log_dir)
        .args(["--timeout-s", "5"])
        .args(extra)
        .output()
        .expect("run gwin-verify");
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let lines: Vec<&str> = stdout.lines().collect();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(lines.len(), 1, "one JSON line: {stdout} / stderr: {stderr}");
    let line: Value = serde_json::from_str(lines[0]).expect("JSON line");
    (out.status.success(), line)
}

fn log_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("log dir");
    dir
}

#[test]
fn headless_handshake_against_the_fake_server_passes() {
    let (success, line) = run(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-ok"),
        &[],
    );
    assert!(success, "{line}");
    assert_eq!(line["check"], "ssh");
    assert_eq!(line["ok"], true, "{line}");
    assert_eq!(line["headless"], true, "{line}");
    assert_eq!(line["server_version"], 1, "{line}");
    assert_eq!(line["exit_after_bye"], true, "{line}");
}

#[test]
fn mirror_mode_names_the_output() {
    let (success, line) = run(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-mirror"),
        &["--output", "DP-1"],
    );
    assert!(success, "{line}");
    assert_eq!(line["headless"], false, "{line}");
    assert_eq!(line["output"], "DP-1", "{line}");
}

#[test]
fn the_default_command_line_mirrors_the_focused_output() {
    let (success, line) = run(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-auto"),
        &["--output", "auto"],
    );
    assert!(success, "{line}");
    assert_eq!(line["headless"], false, "{line}");
    assert_eq!(
        line["output"], "HDMI-A-1",
        "the real screen it chose: {line}"
    );
    let args = line["args"].as_array().expect("args");
    assert_eq!(
        args[args.len() - 2..],
        ["--output", "auto"],
        "the client's default command line: {line}"
    );
}

#[test]
fn a_missing_server_is_classified_as_server_missing() {
    let (success, line) = run("/nonexistent/gliff-server", &log_dir("ssh-missing"), &[]);
    assert!(!success, "{line}");
    assert_eq!(line["ok"], false, "{line}");
    assert_eq!(line["failure"], "ServerMissing", "{line}");
}

#[test]
fn the_handshake_reads_stream_config() {
    let (_, line) = run(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-stream"),
        &[],
    );
    assert_eq!(line["stream"], "1920x1080", "{line}");
}

#[test]
fn a_server_that_stops_after_hello_ack_fails() {
    let (success, line) = run_with(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-wedge"),
        &[],
        "wedge",
    );
    assert!(!success, "{line}");
    assert_eq!(line["ok"], false, "{line}");
}

#[test]
fn a_server_in_the_wrong_mode_fails() {
    let (success, line) = run_with(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-wrong-mode"),
        &[],
        "wrong-mode",
    );
    assert!(!success, "{line}");
    assert_eq!(line["ok"], false, "{line}");
}

#[test]
fn a_server_that_exits_badly_after_bye_fails() {
    let (success, line) = run_with(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-bad-exit"),
        &[],
        "bad-exit",
    );
    assert!(!success, "{line}");
    assert_eq!(line["exit_after_bye"], false, "{line}");
}

#[test]
fn a_server_that_never_exits_after_bye_fails() {
    let (success, line) = run_with(
        env!("CARGO_BIN_EXE_gwin-fake-server"),
        &log_dir("ssh-linger"),
        &[],
        "linger",
    );
    assert!(!success, "{line}");
    assert_eq!(line["ok"], false, "{line}");
    assert_eq!(line["stage"], "waiting for exit after Bye", "{line}");
    assert!(line["exit_after_bye"].is_null(), "{line}");
}
