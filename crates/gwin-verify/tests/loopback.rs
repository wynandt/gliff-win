//! The loopback the native verification skill drives, end to end on the
//! build machine: the real session driver, spawning `gwin-fake-ssh` (the
//! portable ssh stand-in), which runs the `gwin-fake-server` next to it with
//! `GWIN_FAKE_INPUT_LOG` set. The test reads that log as the remote side's
//! record of what arrived.
//!
//! What it proves: keys and a Resize sent through the session reach the
//! remote side in order; a key left down is let go when the client
//! releases input (before any Bye); a key still down at the end is let go
//! before the Bye, so the Bye line reports nothing still held. On Windows
//! the skill's `gwin-native.ps1` reads the same file after real key presses.

// The ssh wrapper that sets the log path is a bash script: build machine only.
#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gliff_windows::altgr::Origin;
use gliff_windows::altgr::RawKey;
use gliff_windows::cli::DesktopMode;
use gliff_windows::input::InputOptions;
use gliff_windows::keymap::Prefix;
use gliff_windows::keymap::Press;
use gliff_windows::session::Colour;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::driver::Options;
use gliff_windows::session::driver::SessionHandle;
use gliff_windows::session::driver::Waker;
use gliff_windows::session::driver::start;
use gliff_windows::ssh::SshSpec;
use serde_json::Value;
use serde_json::json;

const FAKE_SSH: &str = env!("CARGO_BIN_EXE_gwin-fake-ssh");

struct Quiet;

impl Waker for Quiet {
    fn frame(&self) {}
    fn event(&self) {}
}

/// An ssh that is `gwin-fake-ssh` with the input log set, as
/// `gwin-native.ps1` sets it in the client's environment on Windows.
fn ssh_with_log(dir: &Path, log: &Path) -> PathBuf {
    let wrapper = dir.join("ssh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/usr/bin/env bash\nGWIN_FAKE_INPUT_LOG={} exec {FAKE_SSH} \"$@\"\n",
            log.display()
        ),
    )
    .expect("write ssh wrapper");
    let mut perms = std::fs::metadata(&wrapper).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&wrapper, perms).expect("chmod");
    wrapper
}

fn session(name: &str) -> (SessionHandle, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("loopback-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("log dir");
    let log = dir.join("input.jsonl");
    let opts = Options {
        spec: SshSpec {
            ssh: ssh_with_log(&dir, &log),
            target: "verify@loopback".parse().expect("a target"),
            // A bare name, as on Windows: gwin-fake-ssh runs its sibling.
            server_path: "gliff-server".to_owned(),
            mode: DesktopMode::Headless,
            log_dir: dir,
            log_tag: None,
        },
        panel: (1920, 1080),
        initial: ResizeReq {
            width: 640,
            height: 400,
            scale: 1.0,
        },
        colour: Colour::Standard,
        input: InputOptions::default(),
        gpu_decode: false,
    };
    (start(opts, Arc::new(Quiet)).expect("start"), log)
}

/// Waits until the session has decoded a few pictures, taking them as the
/// UI thread would.
fn streaming(handle: &SessionHandle) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while handle.stats().frames < 3 {
        assert!(
            Instant::now() < deadline,
            "no pictures: {:?}",
            handle.stats()
        );
        let _ = handle.slot().take();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn lines(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .expect("the remote input log")
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// A key as the low-level hook reports it: virtual key and scan code.
fn key(vk: u16, scan: u16, press: Press, time_ms: u32) -> UiCmd {
    UiCmd::Key(RawKey {
        vk,
        scan,
        prefix: Prefix::None,
        press,
        time_ms,
        origin: Origin::Hook,
        injected: false,
    })
}

/// Lines of one kind, in order.
fn of(lines: &[Value], ev: &str) -> Vec<Value> {
    lines.iter().filter(|l| l["ev"] == ev).cloned().collect()
}

#[test]
fn keys_and_a_resize_reach_the_remote_side_and_nothing_is_held_at_bye() {
    let (handle, log) = session("keys");
    streaming(&handle);
    // g, l, i, f, f through the hook's path: (virtual key, scan code), which
    // reach the remote as evdev 34, 38, 23, 33, 33.
    let mut t = 1000;
    for (vk, scan) in [
        (0x47, 0x22),
        (0x4c, 0x26),
        (0x49, 0x17),
        (0x46, 0x21),
        (0x46, 0x21),
    ] {
        for press in [Press::Down, Press::Up] {
            handle.send(key(vk, scan, press, t));
            t += 20;
        }
    }
    handle.send(UiCmd::Resize(ResizeReq {
        width: 800,
        height: 600,
        scale: 1.0,
    }));
    // A left Shift (evdev 42) left down, then the client lets go of the
    // input, as on focus loss.
    handle.send(key(0xa0, 0x2a, Press::Down, t));
    handle.send(UiCmd::ReleaseInput);
    // The Up comes from the release, not from the Bye (which also lets go).
    let released = Instant::now() + Duration::from_secs(5);
    while !lines(&log)
        .iter()
        .any(|l| l["ev"] == "key" && l["code"] == 42 && l["down"] == false)
    {
        assert!(
            Instant::now() < released,
            "no Shift Up before Bye: {:?}",
            lines(&log)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // A key still down when the session ends: the Bye line must show that
    // the client let go of it first.
    handle.send(key(0x41, 0x1e, Press::Down, t + 20));
    std::thread::sleep(Duration::from_millis(200));
    handle.send(UiCmd::Bye);
    let end = handle.join();
    assert_eq!(end.ssh_exit, Some(0), "{end:?}");

    let seen = lines(&log);
    assert_eq!(seen[0]["ev"], "hello", "{seen:?}");
    let keys: Vec<(u64, bool)> = of(&seen, "key")
        .iter()
        .map(|l| (l["code"].as_u64().expect("code"), l["down"] == true))
        .collect();
    let mut want = Vec::new();
    for code in [34, 38, 23, 33, 33] {
        want.push((code, true));
        want.push((code, false));
    }
    want.push((42, true));
    want.push((42, false));
    want.push((30, true));
    want.push((30, false));
    assert_eq!(keys, want, "{seen:?}");
    assert!(
        of(&seen, "resize")
            .iter()
            .any(|l| l["width"] == 800 && l["height"] == 600),
        "the Resize reached the remote side: {seen:?}"
    );
    let bye = seen.last().expect("a last line");
    assert_eq!(bye["ev"], "bye", "{seen:?}");
    assert_eq!(bye["held_keys"], json!([]));
    assert_eq!(bye["held_buttons"], json!([]));
}

#[test]
fn the_version_check_runs_through_the_loopback() {
    let out = std::process::Command::new(FAKE_SSH)
        .args(["-T", "--", "verify@loopback", "gliff-server", "--version"])
        .output()
        .expect("run gwin-fake-ssh");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "gliff-server 0.4.1"
    );
}
