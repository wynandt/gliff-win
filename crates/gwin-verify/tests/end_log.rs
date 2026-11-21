#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use gliff_windows::cli::DesktopMode;
use gliff_windows::input::InputOptions;
use gliff_windows::session::Colour;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::UiEvent;
use gliff_windows::session::driver::Options;
use gliff_windows::session::driver::SessionHandle;
use gliff_windows::session::driver::Waker;
use gliff_windows::session::driver::start;
use gliff_windows::ssh::SshSpec;

struct Quiet;

impl Waker for Quiet {
    fn frame(&self) {}
    fn event(&self) {}
}

fn client_log() -> PathBuf {
    static LOG: OnceLock<PathBuf> = OnceLock::new();
    LOG.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("end-log-client");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("client log dir");
        gliff_windows::logging::init(&dir).expect("client log")
    })
    .clone()
}

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

const SERVER: &str = env!("CARGO_BIN_EXE_gwin-fake-server");

fn start_session(tag: u32, ssh: PathBuf, server_path: &str) -> SessionHandle {
    let _ = client_log();
    let dir = ssh
        .parent()
        .expect("ssh wrapper lives in the session dir")
        .to_path_buf();
    let opts = Options {
        spec: SshSpec {
            ssh,
            target: "user@host".parse().expect("a target"),
            server_path: server_path.to_owned(),
            mode: DesktopMode::Headless,
            log_dir: dir,
            log_tag: Some(tag),
        },
        panel: (3840, 2400),
        initial: SIZE,
        colour: Colour::Standard,
        input: InputOptions::default(),
        gpu_decode: false,
    };
    start(opts, Arc::new(Quiet)).expect("start the session thread")
}

fn session(tag: u32, fake: &str) -> SessionHandle {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("end-log-session-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("session dir");
    let env = format!("GWIN_FAKE_SERVER={fake}");
    start_session(tag, ssh_for(&dir, &env), SERVER)
}

fn session_env(tag: u32, env: &str, server_path: &str) -> SessionHandle {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("end-log-session-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("session dir");
    start_session(tag, ssh_for(&dir, env), server_path)
}

fn wait_for(handle: &SessionHandle, want: impl Fn(&UiEvent) -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        while let Some(event) = handle.try_event() {
            if want(&event) {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the session event did not arrive");
}

fn ended_line(tag: u32) -> String {
    let text = std::fs::read_to_string(client_log()).expect("read client log");
    let hits: Vec<String> = text
        .lines()
        .filter(|line| line.contains(&format!("session {tag}: ended:")))
        .map(|line| {
            line.split_once(' ')
                .map(|(_, rest)| rest.to_owned())
                .unwrap_or_else(|| line.to_owned())
        })
        .collect();
    assert_eq!(hits.len(), 1, "session {tag} end lines in:\n{text}");
    let line = hits.into_iter().next().expect("one end line");
    eprintln!("END_LOG_CASE {tag}: {line}");
    line
}

#[test]
fn a_clean_bye_names_itself_and_ssh_exit_0_with_no_tail() {
    let handle = session(1, "");
    wait_for(&handle, |event| matches!(event, UiEvent::StreamConfig(_)));
    handle.send(UiCmd::Bye);
    let _ = handle.join();
    let line = ended_line(1);
    assert_eq!(line, "session 1: ended: the client said Bye; ssh exit 0");
    assert!(!line.contains("ssh stderr tail"), "{line}");
}

#[test]
fn a_server_error_is_named_with_its_code() {
    let handle = session(2, "error");
    wait_for(&handle, |event| matches!(event, UiEvent::Ended(_)));
    let _ = handle.join();
    let line = ended_line(2);
    assert!(
        line.contains("the server sent error 300: fake error on purpose"),
        "{line}"
    );
}

#[test]
fn an_incompatible_server_is_named_as_such() {
    let handle = session(3, "incompatible");
    wait_for(&handle, |event| matches!(event, UiEvent::Ended(_)));
    let _ = handle.join();
    let line = ended_line(3);
    assert!(line.contains("the server is incompatible:"), "{line}");
    assert!(line.contains("fake incompatible on purpose"), "{line}");
}

#[test]
fn a_crash_after_the_stream_starts_keeps_a_sanitised_stderr_tail() {
    let handle = session(4, "crash");
    wait_for(&handle, |event| matches!(event, UiEvent::Ended(_)));
    let _ = handle.join();
    let line = ended_line(4);
    assert!(line.contains("the link was lost:"), "{line}");
    assert!(line.contains("ssh exit 1"), "{line}");
    assert!(
        line.contains("ssh stderr tail: fake-server: crashed on purpose"),
        "{line}"
    );
}

#[test]
fn an_ssh_failure_hides_the_fingerprint_the_path_and_the_key() {
    let stderr = Path::new(env!("CARGO_TARGET_TMPDIR")).join("end-log-ssh-5.stderr");
    std::fs::write(
        &stderr,
        "\
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
The fingerprint for the ED25519 key sent by the remote host is
SHA256:kewFiRGj02X1n6KzFRID5mmHmKrpaXNy/Wgiqp4o4rw.
Offending ED25519 key in /home/user/.ssh/known_hosts:4
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
Host key verification failed.
",
    )
    .expect("write ssh stderr");
    let env = format!(
        "GWIN_FAKE_SSH_STDERR=\"{}\" GWIN_FAKE_SSH_EXIT=255",
        stderr.display()
    );
    let handle = session_env(5, &env, "gliff-server");
    wait_for(&handle, |event| matches!(event, UiEvent::Ended(_)));
    let _ = handle.join();
    let line = ended_line(5);
    assert!(line.contains("ssh failed: host_key_changed"), "{line}");
    assert!(line.contains("ssh exit 255"), "{line}");
    assert!(line.contains("ssh stderr tail:"), "{line}");
    assert!(line.contains("Host key verification failed."), "{line}");
    let log = std::fs::read_to_string(client_log()).expect("read client log");
    assert!(!log.contains("kewFiRGj02X1n6KzFRID5mmHmKrpaXNy"), "{log}");
    assert!(!log.contains("/home/user"), "{log}");
    assert!(!log.contains("AAAAC3Nza"), "{log}");
}

#[test]
fn a_bad_exit_after_bye_keeps_the_bye_and_the_code() {
    let handle = session(6, "bad-exit");
    wait_for(&handle, |event| matches!(event, UiEvent::StreamConfig(_)));
    handle.send(UiCmd::Bye);
    let _ = handle.join();
    let line = ended_line(6);
    assert!(line.contains("the client said Bye; ssh exit 3"), "{line}");
}

#[test]
fn a_missing_ssh_program_has_no_stderr_tail() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("end-log-session-7");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("session dir");
    let handle = start_session(7, dir.join("missing-ssh"), SERVER);
    let _ = handle.join();
    let line = ended_line(7);
    assert!(line.contains("ssh failed:"), "{line}");
    assert!(!line.contains("ssh stderr tail"), "{line}");
}
