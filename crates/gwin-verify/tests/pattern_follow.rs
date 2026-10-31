//! `scripts/remote/gwin-pattern` end to end on the build machine, against the same
//! output sizing a real present run produced: the client's session
//! driver through `scripts/fake-ssh` to `gwin-fake-server`, which sizes the
//! headless output as upstream does (physical mode plus scale; a Resize of
//! 2560x1600 at 2.0 is a 2560x1600 mode at scale 2, 1280x800 logical), and
//! the script on `scripts/fake-bin` (fake hyprctl with its window world,
//! fake ffplay). An early real run never placed
//! the pattern: the script waited for a 3840x2400 logical output that a
//! scale-2 session never has.

// Bash scripts and fakes: build machine only.
#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gliff_windows::cli::DesktopMode;
use gliff_windows::input::InputOptions;
use gliff_windows::session::Colour;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::driver::Options;
use gliff_windows::session::driver::SessionHandle;
use gliff_windows::session::driver::Waker;
use gliff_windows::session::driver::start;
use gliff_windows::ssh::SshSpec;
use serde_json::Value;

/// What is run on the remote machine before a present run.
const REMOTE_ARGS: &[&str] = &["follow"];

const SERVER: &str = env!("CARGO_BIN_EXE_gwin-fake-server");

/// The present run's two client sizes, both at 200%.
const WINDOWED: ResizeReq = ResizeReq {
    width: 2560,
    height: 1600,
    scale: 2.0,
};
const FULLSCREEN: ResizeReq = ResizeReq {
    width: 3840,
    height: 2400,
    scale: 2.0,
};

struct Quiet;

impl Waker for Quiet {
    fn frame(&self) {}
    fn event(&self) {}
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write wrapper");
    let mut perms = std::fs::metadata(path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

/// The pattern window's title for the test that owns `dir`. The tests run at
/// the same time, and the script's `cleanup` kills every process on the
/// machine whose command line holds its title, so each test needs its own or
/// one test's start kills another's ffplay. It does not start with the
/// script's default title either, so a checkout whose cleanup matches by
/// prefix leaves these windows alone.
fn title(dir: &Path) -> String {
    format!(
        "test-{}",
        dir.file_name().expect("dir name").to_string_lossy()
    )
}

/// The remote machine side: the script with the fakes first on PATH and an
/// interactive session's Hyprland environment.
fn remote(dir: &Path, hypr: &Path) -> Child {
    remote_with(dir, hypr, REMOTE_ARGS)
}

fn remote_with(dir: &Path, hypr: &Path, args: &[&str]) -> Child {
    let title = title(dir);
    let run = dir.join("run");
    let sig = "a1b2c3_1761250000_200";
    std::fs::create_dir_all(run.join("hypr").join(sig)).expect("instance dir");
    std::fs::write(run.join("hypr").join(sig).join(".socket.sock"), "").expect("socket");
    std::fs::create_dir_all(hypr.join("windows")).expect("window world");
    let path = format!(
        "{}:{}",
        repo().join("scripts/fake-bin").display(),
        std::env::var("PATH").expect("PATH")
    );
    Command::new("bash")
        .arg(repo().join("scripts/remote/gwin-pattern"))
        .args(args)
        .env("PATH", path)
        .env("HOME", dir)
        .env("XDG_RUNTIME_DIR", &run)
        .env("HYPRLAND_INSTANCE_SIGNATURE", sig)
        .env("GWIN_FAKE_HYPR_DIR", hypr)
        .env("GWIN_PATTERN_SETTLE_S", "0.2")
        .env("GWIN_PATTERN_TITLE", &title)
        .stdout(std::fs::File::create(dir.join("remote.out")).expect("remote.out"))
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start gwin-pattern")
}

/// The Windows side: a session whose server writes the output into `hypr`.
fn session(dir: &Path, hypr: &Path) -> SessionHandle {
    let ssh = dir.join("ssh");
    executable(
        &ssh,
        &format!(
            "#!/usr/bin/env bash\nGWIN_FAKE_HYPR_DIR={} exec {} \"$@\"\n",
            hypr.display(),
            repo().join("scripts/fake-ssh").display()
        ),
    );
    let opts = Options {
        spec: SshSpec {
            ssh,
            target: "user@host".parse().expect("a target"),
            server_path: SERVER.to_owned(),
            mode: DesktopMode::Headless,
            log_dir: dir.to_path_buf(),
            log_tag: None,
        },
        panel: (3840, 2400),
        initial: WINDOWED,
        colour: Colour::Standard,
        input: InputOptions::default(),
    };
    start(opts, Arc::new(Quiet)).expect("start the session thread")
}

/// The session output as hyprctl reports it: (id, mode, scale).
fn output(hypr: &Path) -> Option<(i64, (i64, i64), f64)> {
    let text = std::fs::read_to_string(hypr.join("monitors.json")).ok()?;
    let monitors: Value = serde_json::from_str(&text).ok()?;
    monitors.as_array()?.iter().find_map(|m| {
        m["name"].as_str()?.starts_with("gliff-").then_some(())?;
        Some((
            m["id"].as_i64()?,
            (m["width"].as_i64()?, m["height"].as_i64()?),
            m["scale"].as_f64()?,
        ))
    })
}

/// Why the pattern is not ready on the output at `mode`, or None: one
/// fullscreen pattern window on it, drawn at the output's aspect (ffplay
/// letterboxes any other) by a source that moves at 60 fps.
fn not_ready(dir: &Path, hypr: &Path, mode: (i64, i64)) -> Option<String> {
    let Some((id, now, scale)) = output(hypr) else {
        return Some("no gliff-* output".to_owned());
    };
    if now != mode {
        return Some(format!("output at {now:?}, want {mode:?}"));
    }
    let windows: Vec<Value> = std::fs::read_dir(hypr.join("windows"))
        .expect("window world")
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .filter(|w: &Value| w["title"] == title(dir).as_str())
        .collect();
    let [window] = windows.as_slice() else {
        return Some(format!("want one pattern window, got {windows:?}"));
    };
    if window["monitor"].as_i64() != Some(id) || window["fullscreen"].as_i64() != Some(2) {
        return Some(format!("pattern window {window} not fullscreen on {id}"));
    }
    let log = std::fs::read_to_string(hypr.join("ffplay.log")).unwrap_or_default();
    let Some(last) = log.lines().rfind(|l| l.starts_with("start ")) else {
        return Some("ffplay never started".to_owned());
    };
    let fields: Vec<&str> = last.split(' ').collect();
    let (w, h) = fields[2].split_once('x').expect("start <pid> <WxH>");
    let (w, h): (i64, i64) = (w.parse().expect("w"), h.parse().expect("h"));
    if w * mode.1 != h * mode.0 {
        return Some(format!(
            "source {w}x{h} is not the output's aspect {mode:?} at scale {scale}"
        ));
    }
    if fields[3] != "r=60" {
        return Some(format!("source frame rate {}, want r=60", fields[3]));
    }
    if !last.contains("overlay=x='mod(n*") {
        return Some(format!("source has no moving square: {last}"));
    }
    None
}

/// Waits until `ready` (None) or `within` passes, taking pictures so the
/// session keeps flowing; the last reason on a timeout.
fn wait_ready(
    handle: &SessionHandle,
    dir: &Path,
    hypr: &Path,
    mode: (i64, i64),
    within: Duration,
) -> Option<String> {
    let until = Instant::now() + within;
    loop {
        let _ = handle.slot().take();
        while handle.try_event().is_some() {}
        let why = not_ready(dir, hypr, mode);
        if why.is_none() || Instant::now() > until {
            return why;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The script, killed if the test fails first so cargo does not wait on
/// its inherited stderr.
struct Script(Child);

impl Drop for Script {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_exit(child: &mut Child, within: Duration) -> Option<std::process::ExitStatus> {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[test]
fn the_pattern_follows_a_scale_2_output_through_the_present_run() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pattern-follow");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let hypr = dir.join("hypr");
    std::fs::create_dir_all(&hypr).expect("hypr dir");
    let mut script = Script(remote(&dir, &hypr));
    let out = || std::fs::read_to_string(dir.join("remote.out")).unwrap_or_default();
    let handle = session(&dir, &hypr);

    // Upstream: the mode is the client's size, the scale divides it
    // (session.rs:625-650, hypr-ipc lib.rs:197-209); hyprctl reports the
    // mode with scale 2, so the windows are 1280x800 then 1920x1200.
    let windowed = wait_ready(&handle, &dir, &hypr, (2560, 1600), Duration::from_secs(20));
    assert_eq!(
        windowed,
        None,
        "windowed (2560x1600 @2): remote machine said:\n{}",
        out()
    );
    assert_eq!(output(&hypr).map(|o| o.2), Some(2.0), "scale 2 output");

    handle.send(UiCmd::Resize(FULLSCREEN));
    let fullscreen = wait_ready(&handle, &dir, &hypr, (3840, 2400), Duration::from_secs(20));
    assert_eq!(
        fullscreen,
        None,
        "fullscreen (3840x2400 @2): remote machine said:\n{}",
        out()
    );
    // The script saw the change and checked the window again at the new
    // logical size, not only Hyprland keeping it fullscreen.
    let until = Instant::now() + Duration::from_secs(10);
    let rechecked = |said: &str| {
        said.split_once("mode 3840x2400 scale 2.0 logical 1920x1200")
            .is_some_and(|(_, after)| {
                after.contains("PLACEMENT OK: monitor 1 at 2560,0 size 1920x1200")
            })
    };
    while !rechecked(&out()) && Instant::now() < until {
        let _ = handle.slot().take();
        while handle.try_event().is_some() {}
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        rechecked(&out()),
        "re-placed at 1920x1200 logical:\n{}",
        out()
    );

    handle.send(UiCmd::Bye);
    let _ = handle.join();
    let status = wait_exit(&mut script.0, Duration::from_secs(20));
    let said = out();
    assert_eq!(
        status.map(|s| s.code()),
        Some(Some(0)),
        "gwin-pattern exits 0 when the output goes; it said:\n{said}"
    );
    assert!(
        said.contains("RESULT: PLACEMENT OK throughout"),
        "every check passed:\n{said}"
    );
    let left = std::fs::read_dir(hypr.join("windows"))
        .expect("world")
        .count();
    assert_eq!(left, 0, "ffplay stopped with the output:\n{said}");
}

/// A move Hyprland ignores (as `movetoworkspacesilent` was in a real
/// run) leaves the window on the home monitor: still exit 1.
#[test]
fn follow_still_fails_when_the_window_does_not_land() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pattern-follow-stuck");
    let _ = std::fs::remove_dir_all(&dir);
    let hypr = dir.join("hypr");
    std::fs::create_dir_all(&hypr).expect("hypr dir");
    std::fs::write(hypr.join("stuck"), "").expect("stuck marker");
    std::fs::write(
        hypr.join("monitors.json"),
        r#"[{"id":0,"name":"eDP-1","width":2560,"height":1600,"x":0,"y":0,"scale":1.0},
            {"id":1,"name":"gliff-4242","width":2560,"height":1600,"x":2560,"y":0,"scale":2.0}]"#,
    )
    .expect("monitors");
    let mut script = Script(remote_with(&dir, &hypr, &["follow"]));
    let out = || std::fs::read_to_string(dir.join("remote.out")).unwrap_or_default();
    let until = Instant::now() + Duration::from_secs(10);
    while !out().contains("PLACEMENT FAILED") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::remove_file(hypr.join("monitors.json")).expect("the session ends");
    let status = wait_exit(&mut script.0, Duration::from_secs(10));
    let said = out();
    assert_eq!(
        status.map(|s| s.code()),
        Some(Some(1)),
        "exit 1; it said:\n{said}"
    );
    assert!(
        said.contains("PLACEMENT FAILED: window monitor 0")
            && said.contains("RESULT: PLACEMENT FAILED"),
        "names the misplacement:\n{said}"
    );
}

fn monitors(hypr: &Path, gliff: &str) {
    let tmp = hypr.join("monitors.json.tmp");
    std::fs::write(
        &tmp,
        format!(r#"[{{"id":0,"name":"eDP-1","width":2560,"height":1600,"x":0,"y":0,"scale":1.0}},{gliff}]"#),
    )
    .expect("monitors");
    std::fs::rename(tmp, hypr.join("monitors.json")).expect("monitors in place");
}

/// Upstream's first mode is clamped to 1920x1080 at 1.0 (16:9) until the
/// client's Resize (16:10): the source restarts at the new aspect, so
/// ffplay never letterboxes the grid.
#[test]
fn follow_restarts_the_source_when_the_aspect_changes() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pattern-follow-aspect");
    let _ = std::fs::remove_dir_all(&dir);
    let hypr = dir.join("hypr");
    std::fs::create_dir_all(&hypr).expect("hypr dir");
    monitors(
        &hypr,
        r#"{"id":1,"name":"gliff-77","width":1920,"height":1080,"x":2560,"y":0,"scale":1.0}"#,
    );
    let mut script = Script(remote_with(&dir, &hypr, &["follow"]));
    let out = || std::fs::read_to_string(dir.join("remote.out")).unwrap_or_default();
    let wait = |what: &str| {
        let until = Instant::now() + Duration::from_secs(10);
        while !out().contains(what) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(out().contains(what), "want {what:?}:\n{}", out());
    };
    wait("PLACEMENT OK: monitor 1 at 2560,0 size 1920x1080");
    monitors(
        &hypr,
        r#"{"id":1,"name":"gliff-77","width":2560,"height":1600,"x":2560,"y":0,"scale":2.0}"#,
    );
    wait("PLACEMENT OK: monitor 1 at 2560,0 size 1280x800");
    assert_eq!(
        not_ready(&dir, &hypr, (2560, 1600)),
        None,
        "one 16:10 window:\n{}",
        out()
    );
    let log = std::fs::read_to_string(hypr.join("ffplay.log")).expect("ffplay.log");
    let sizes: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("start "))
        .map(|l| l.split(' ').nth(2).expect("size"))
        .collect();
    assert_eq!(sizes, ["960x540", "960x600"], "ffplay starts:\n{log}");
    std::fs::remove_file(hypr.join("monitors.json")).expect("the session ends");
    let status = wait_exit(&mut script.0, Duration::from_secs(10));
    assert_eq!(
        status.map(|s| s.code()),
        Some(Some(0)),
        "exit 0:\n{}",
        out()
    );
}
