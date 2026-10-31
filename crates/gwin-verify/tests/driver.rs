//! The client's session driver (`gliff_windows::session::driver`) end to
//! end on the build machine: the real loop through `scripts/fake-ssh` to
//! `gwin-fake-server`, with a counting [`Waker`] standing in for
//! `PostMessageW` and the test thread standing in for the UI thread.

// The loop runs through bash wrappers (`scripts/fake-ssh`): build machine only.
#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use gliff_windows::cli::DesktopMode;
use gliff_windows::connect::SshFailure;
use gliff_windows::input::InputOptions;
use gliff_windows::keymap::Press;
use gliff_windows::pointer::PointerEvent;
use gliff_windows::session::EndReason;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::UiEvent;
use gliff_windows::session::driver::Options;
use gliff_windows::session::driver::SessionEnd;
use gliff_windows::session::driver::SessionHandle;
use gliff_windows::session::driver::Waker;
use gliff_windows::session::driver::start;
use gliff_windows::ssh::SshSpec;

#[derive(Default)]
struct Counting {
    frames: AtomicU64,
    events: AtomicU64,
}

impl Waker for Counting {
    fn frame(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
    fn event(&self) {
        self.events.fetch_add(1, Ordering::Relaxed);
    }
}

/// fake-ssh behind a one-line wrapper that sets `env` (shell assignments such
/// as `GWIN_FAKE_SERVER=wedge`), so no test changes the process environment.
fn ssh_for(dir: &Path, env: &str) -> PathBuf {
    let fake_ssh = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh");
    let wrapper = dir.join("ssh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/usr/bin/env bash\n{env} exec {} \"$@\"\n",
            fake_ssh.display()
        ),
    )
    .expect("write ssh wrapper");
    let mut perms = std::fs::metadata(&wrapper).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&wrapper, perms).expect("chmod");
    wrapper
}

const SIZE: ResizeReq = ResizeReq {
    width: 640,
    height: 400,
    scale: 1.0,
};

fn session(name: &str, fake: &str, server_path: &str) -> (SessionHandle, Arc<Counting>) {
    session_with(name, &format!("GWIN_FAKE_SERVER={fake}"), server_path)
}

/// A session whose ssh is fake-ssh run with `env`.
fn session_with(name: &str, env: &str, server_path: &str) -> (SessionHandle, Arc<Counting>) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("driver-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("log dir");
    let opts = Options {
        spec: SshSpec {
            ssh: ssh_for(&dir, env),
            target: "user@host".parse().expect("a target"),
            server_path: server_path.to_owned(),
            mode: DesktopMode::Headless,
            log_dir: dir,
            log_tag: None,
        },
        panel: (3840, 2400),
        initial: SIZE,
        input: InputOptions::default(),
    };
    let waker = Arc::new(Counting::default());
    let handle = start(opts, waker.clone()).expect("start the session thread");
    (handle, waker)
}

const SERVER: &str = env!("CARGO_BIN_EXE_gwin-fake-server");

/// Polls events like the UI thread would, taking pictures as they come,
/// until `want` matches one or `within` passes.
fn wait_for(
    handle: &SessionHandle,
    seen: &mut Vec<UiEvent>,
    pictures: &mut u64,
    within: Duration,
    want: impl Fn(&UiEvent) -> bool,
) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if handle.slot().take().is_some() {
            *pictures += 1;
        }
        while let Some(event) = handle.try_event() {
            let hit = want(&event);
            seen.push(event);
            if hit {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn pictures_for(handle: &SessionHandle, pictures: &mut u64, n: u64, within: Duration) {
    let until = Instant::now() + within;
    while *pictures < n && Instant::now() < until {
        if handle.slot().take().is_some() {
            *pictures += 1;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn bye(handle: SessionHandle) -> SessionEnd {
    handle.send(UiCmd::Bye);
    handle.join()
}

#[test]
fn a_session_streams_pictures_and_ends_cleanly_on_bye() {
    let (handle, waker) = session("clean", "", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(15),
            |e| { matches!(e, UiEvent::StreamConfig(info) if info.geometry.view == (640, 400)) }
        ),
        "a StreamConfig for the requested 640x400 must arrive: {seen:?}"
    );
    assert!(
        matches!(seen.first(), Some(UiEvent::Connected(s)) if s.headless),
        "Connected (headless) comes first: {seen:?}"
    );
    pictures_for(&handle, &mut pictures, 20, Duration::from_secs(15));
    assert!(
        pictures >= 20,
        "20 pictures must reach the slot, got {pictures}"
    );
    let end = bye(handle);
    assert_eq!(end.reason, Some(EndReason::Bye), "{end:?}");
    assert_eq!(
        end.ssh_exit,
        Some(0),
        "the server exits 0 after Bye and EOF: {end:?}"
    );
    assert!(end.errors.is_empty(), "no errors: {end:?}");
    assert!(end.stats.frames >= 20, "{:?}", end.stats);
    assert_eq!(
        end.stats.frames, end.stats.acks,
        "every frame acked: {:?}",
        end.stats
    );
    assert_eq!(end.stats.decode_errors, 0, "{:?}", end.stats);
    assert!(
        waker.frames.load(Ordering::Relaxed) >= pictures,
        "the waker is told of every picture"
    );
    assert!(
        waker.events.load(Ordering::Relaxed) >= 2,
        "and of the events"
    );
}

#[test]
fn a_resize_mid_session_brings_a_new_stream_config() {
    let (handle, _) = session("resize", "", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(15),
            |e| { matches!(e, UiEvent::StreamConfig(info) if info.geometry.view == (640, 400)) }
        ),
        "first config: {seen:?}"
    );
    handle.send(UiCmd::Resize(ResizeReq {
        width: 800,
        height: 480,
        scale: 2.0,
    }));
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(10),
            |e| {
                matches!(e, UiEvent::StreamConfig(info)
                if info.geometry.view == (800, 480) && info.geometry.scale_milli == 2000)
            }
        ),
        "the 800x480 @ 2.0 config must follow the Resize: {seen:?}"
    );
    let end = bye(handle);
    assert_eq!(
        end.stats.resizes_sent, 2,
        "initial and the new one: {:?}",
        end.stats
    );
    assert_eq!(end.ssh_exit, Some(0), "{end:?}");
}

#[test]
fn a_cursor_shape_reaches_the_ui() {
    let (handle, _) = session("cursor", "cursor", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(15),
            |e| { matches!(e, UiEvent::Cursor(_)) }
        ),
        "the fake's cursor must arrive: {seen:?}"
    );
    let Some(UiEvent::Cursor(image)) = seen.last() else {
        panic!("last event is the cursor: {seen:?}");
    };
    assert_eq!((image.width, image.height, image.hot), (4, 4, (1, 2)));
    assert_eq!(image.argb.len(), 64, "the payload travels with it");
    assert!(
        image.usable(),
        "one opaque pixel in a clear square is a shape"
    );
    assert_eq!(bye(handle).ssh_exit, Some(0));
}

#[test]
fn a_missing_server_is_named_as_such_with_ssh_127() {
    let (handle, waker) = session("missing", "", "/nonexistent/gliff-server");
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(10),
            |e| { matches!(e, UiEvent::Ended(EndReason::Ssh(SshFailure::ServerMissing))) }
        ),
        "the link ends without any server: {seen:?}"
    );
    let end = handle.join();
    assert!(
        matches!(end.reason, Some(EndReason::Ssh(SshFailure::ServerMissing))),
        "{end:?}"
    );
    assert_eq!(
        end.ssh_exit,
        Some(127),
        "fake-ssh's 'No such file': {end:?}"
    );
    let log = std::fs::read_to_string(end.stderr_log.as_ref().expect("stderr log"))
        .expect("read stderr log");
    assert!(
        log.contains("No such file"),
        "the reason is in the log: {log}"
    );
    assert!(waker.events.load(Ordering::Relaxed) >= 1, "the UI is woken");
}

#[test]
fn a_server_that_lingers_after_bye_is_killed_and_reported() {
    let (handle, _) = session("linger", "linger", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(15),
            |e| { matches!(e, UiEvent::StreamConfig(_)) }
        ),
        "{seen:?}"
    );
    let started = Instant::now();
    let end = bye(handle);
    assert_eq!(end.reason, Some(EndReason::Bye), "{end:?}");
    assert_eq!(end.ssh_exit, None, "killed: {end:?}");
    assert!(
        end.errors.iter().any(|e| e.contains("still running")),
        "the linger is reported: {end:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "the kill comes after EXIT_WAIT, not never: {:?}",
        started.elapsed()
    );
}

#[test]
fn dropping_the_handle_says_bye() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("driver-drop");
    let (handle, waker) = session("drop", "", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(15),
            |e| { matches!(e, UiEvent::StreamConfig(_)) }
        ),
        "{seen:?}"
    );
    let before = waker.events.load(Ordering::Relaxed);
    drop(handle);
    // The thread is detached now; its last act is the final wake, after
    // ssh exited.
    let until = Instant::now() + Duration::from_secs(10);
    while waker.events.load(Ordering::Relaxed) < before + 2 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        waker.events.load(Ordering::Relaxed) >= before + 2,
        "Ended(Bye) and the final wake must follow the drop"
    );
    let log = std::fs::read_to_string(dir.join("ssh-stderr.log")).expect("stderr log");
    assert!(
        !log.contains("closed without Bye"),
        "the server saw a Bye: {log}"
    );
}

#[test]
fn the_end_reports_what_the_slot_replaced() {
    // Regression: the exit stats read the slot's drop
    // counter after the handle was gone and always wrote 0.
    let (handle, _) = session("drops", "", SERVER);
    let until = Instant::now() + Duration::from_secs(15);
    // A UI that never takes: every picture after the first is replaced.
    while handle.stats().frames < 10 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    let seen = handle.slot().dropped();
    assert!(seen >= 5, "the slot replaced pictures: {seen}");
    let end = bye(handle);
    assert!(
        end.slot_dropped >= seen,
        "the end carries the slot's counter ({seen} seen before Bye): {end:?}"
    );
}

/// Three button presses: input a working server answers with a picture.
fn click_thrice(handle: &SessionHandle) {
    for _ in 0..3 {
        for press in [Press::Down, Press::Up] {
            handle.send(UiCmd::Pointer(PointerEvent::Button {
                button: 0x110,
                press,
            }));
        }
    }
}

#[test]
fn a_server_that_loses_its_output_is_resynced_when_input_goes_unanswered() {
    let (handle, _) = session("reload", "reload", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    pictures_for(&handle, &mut pictures, 5, Duration::from_secs(15));
    assert!(pictures >= 5, "pictures before the fault: {seen:?}");
    // The fake loses its output 1 s after the Resize; wait for the silence.
    std::thread::sleep(Duration::from_millis(1500));
    while handle.slot().take().is_some() {}
    click_thrice(&handle);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(10),
            |e| matches!(e, UiEvent::Resyncing)
        ),
        "unanswered input must start a resync: {seen:?}"
    );
    let before = pictures;
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(10),
            |e| matches!(e, UiEvent::Resynced)
        ),
        "the first picture after the resync must say so: {seen:?}"
    );
    pictures_for(&handle, &mut pictures, before + 5, Duration::from_secs(10));
    assert!(pictures >= before + 5, "pictures flow again: {pictures}");
    let end = bye(handle);
    assert_eq!(end.stats.resyncs, 1, "{:?}", end.stats);
    assert_eq!(
        end.stats.resizes_sent, 3,
        "the initial Resize and the resync's two: {:?}",
        end.stats
    );
    assert_eq!(end.ssh_exit, Some(0), "{end:?}");
}

#[test]
fn a_server_that_keeps_answering_is_never_resynced() {
    let (handle, _) = session("no-resync", "", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    pictures_for(&handle, &mut pictures, 5, Duration::from_secs(15));
    click_thrice(&handle);
    assert!(
        !wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(4),
            |e| matches!(e, UiEvent::Resyncing)
        ),
        "pictures kept coming, so there is nothing to resync: {seen:?}"
    );
    let end = bye(handle);
    assert_eq!(end.stats.resyncs, 0, "{:?}", end.stats);
}

/// A real ssh failure (a file of the corpus) played by fake-ssh, and what the
/// session says about it.
fn fails_with(name: &str, file: &str, exit: i32) -> (EndReason, SessionEnd) {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gliff-windows/tests/ssh_corpus/openssh-10.0p2-linux")
        .join(format!("{file}.stderr"));
    let env = format!(
        "GWIN_FAKE_SSH_STDERR={} GWIN_FAKE_SSH_EXIT={exit}",
        corpus.display()
    );
    let (handle, _) = session_with(name, &env, "gliff-server");
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(
        wait_for(
            &handle,
            &mut seen,
            &mut pictures,
            Duration::from_secs(10),
            |e| matches!(e, UiEvent::Ended(_))
        ),
        "{file}: the session ends: {seen:?}"
    );
    let end = handle.join();
    let reason = end.reason.clone().expect("a reason");
    (reason, end)
}

#[test]
fn each_way_ssh_fails_ends_the_session_with_its_class_and_the_log_says_so() {
    let log_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("driver-client-log");
    let _ = std::fs::remove_dir_all(&log_dir);
    let log = gliff_windows::logging::init(&log_dir).expect("client log");
    // (corpus file, ssh exit, class, what the log must say)
    let table = [
        (
            "host_unknown",
            255,
            "host_unknown",
            "not known to this computer yet",
        ),
        (
            "host_changed",
            255,
            "host_key_changed",
            "different identity",
        ),
        ("key_refused", 255, "key_refused", "did not accept a key"),
        (
            "refused",
            255,
            "refused",
            "Nothing is listening for ssh on host",
        ),
        (
            "timeout",
            255,
            "unreachable",
            "did not answer in 10 seconds",
        ),
        (
            "name_not_found_unknown",
            255,
            "name_not_found",
            "Can't find \"host\"",
        ),
        ("reset", 255, "other", "ssh said: Connection reset by"),
    ];
    for (file, exit, class, says) in table {
        let (reason, end) = fails_with(file, file, exit);
        let EndReason::Ssh(failure) = reason else {
            panic!("{file}: {reason:?}");
        };
        assert_eq!(failure.name(), class, "{file}");
        assert_eq!(end.ssh_exit, Some(exit), "{file}");
        let text = std::fs::read_to_string(&log).expect("read client log");
        let line = text
            .lines()
            .find(|l| {
                l.contains(&format!("ssh failed: {class} (ssh exit {exit}): ")) && l.contains(says)
            })
            .unwrap_or_else(|| {
                panic!("{file}: no log line for {class} saying {says:?} in:\n{text}")
            });
        assert!(line.contains("ssh-stderr"), "names ssh's own text: {line}");
    }
}

#[test]
fn a_server_that_exits_badly_after_the_stream_started_is_not_a_connect_failure() {
    let (handle, _) = session("worked-then-bad-exit", "bad-exit", SERVER);
    let (mut seen, mut pictures) = (Vec::new(), 0);
    assert!(wait_for(
        &handle,
        &mut seen,
        &mut pictures,
        Duration::from_secs(15),
        |e| matches!(e, UiEvent::StreamConfig(_))
    ));
    handle.send(UiCmd::Bye);
    let end = handle.join();
    assert!(
        !matches!(end.reason, Some(EndReason::Ssh(_))),
        "a session that worked is not a connect failure: {end:?}"
    );
}

/// The version check through the real runner and fake-ssh to the fake server.
fn check_through_fake_ssh(
    name: &str,
    env: &str,
    server_path: &str,
) -> Result<gliff_windows::connect::ServerVersion, SshFailure> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("driver-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let spec = SshSpec {
        ssh: ssh_for(&dir, env),
        target: "user@host".parse().expect("a target"),
        server_path: server_path.to_owned(),
        mode: DesktopMode::Headless,
        log_dir: dir,
        log_tag: None,
    };
    gliff_windows::connect::check_server(&gliff_windows::connect::runner::RealTools, &spec)
}

#[test]
fn the_version_check_tells_a_good_server_an_old_one_and_a_missing_one_apart() {
    let ok = check_through_fake_ssh("v-ok", "GWIN_FAKE_SERVER=", SERVER).expect("current server");
    assert_eq!(ok.to_string(), "0.4.1");
    assert_eq!(
        check_through_fake_ssh(
            "v-old",
            "GWIN_FAKE_SERVER_VERSION=0.3.2 GWIN_FAKE_SERVER=",
            SERVER
        )
        .expect_err("old")
        .name(),
        "server_old"
    );
    assert_eq!(
        check_through_fake_ssh(
            "v-missing",
            "GWIN_FAKE_SERVER=",
            "/nonexistent/gliff-server"
        ),
        Err(SshFailure::ServerMissing)
    );
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gliff-windows/tests/ssh_corpus/openssh-10.0p2-linux/key_refused.stderr");
    let refused = format!("GWIN_FAKE_SSH_STDERR={}", corpus.display());
    assert_eq!(
        check_through_fake_ssh("v-refused", &refused, SERVER),
        Err(SshFailure::KeyRefused)
    );
}
