use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use super::Cmd;
use crate::cli::DesktopMode;
use crate::ssh::SshSpec;
use crate::ssh::exec_args;
use crate::ssh::ssh_args;

fn spec(ssh: &Path) -> SshSpec {
    SshSpec {
        ssh: ssh.to_owned(),
        target: "me@lab-pc".parse().expect("a target"),
        server_path: "gliff-server".into(),
        mode: DesktopMode::Headless,
        log_dir: PathBuf::from("logs"),
        log_tag: None,
    }
}

fn strings(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// A script standing in for ssh, in a fresh folder.
#[cfg(unix)]
fn script(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gwin-runner-{}-{name}", std::process::id()));
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[test]
fn no_call_the_client_makes_weakens_ssh() {
    // Section 11.7 of the design: the client never turns off host key
    // checking, never forwards the agent, never forces or allows passwords,
    // and never asks to be prompted.
    let spec = spec(Path::new("ssh"));
    let version = Cmd::version(&spec).expect("a valid spec");
    let all = [
        strings(&ssh_args(&spec)),
        strings(&exec_args(&spec, "true")),
        strings(version.args()),
    ];
    let banned = [
        "stricthostkeychecking",
        "userknownhostsfile",
        "passwordauthentication",
        "kbdinteractiveauthentication",
        "forwardagent",
        "preferredauthentications",
        "checkhostip",
        "verifyhostkeydns",
        "nohostauthenticationforlocalhost",
        "askpass",
        "proxycommand",
        "permitlocalcommand",
        "localcommand",
    ];
    for args in all {
        let joined = args.join(" ").to_lowercase();
        for word in banned {
            assert!(!joined.contains(word), "{word} in {args:?}");
        }
        assert!(
            !args.iter().any(|a| a == "-A" || a == "-oBatchMode=no"),
            "{args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"),
            "never prompts: {args:?}"
        );
    }
}

#[test]
fn the_session_and_the_version_check_share_their_ssh_options() {
    let spec = spec(Path::new("ssh"));
    let options = |args: Vec<String>| {
        let end = args.iter().position(|a| a == "--").expect("--");
        args[..end].to_vec()
    };
    let version = Cmd::version(&spec).expect("a valid spec");
    assert_eq!(
        options(strings(&ssh_args(&spec))),
        options(strings(version.args()))
    );
}

#[cfg(unix)]
mod real {
    use std::path::Path;
    use std::time::Duration;

    use super::super::Cmd;
    use super::super::RealTools;
    use super::super::ToolError;
    use super::super::Tools;
    use super::{script, spec};

    /// Writing a script and starting it must not overlap with another test
    /// starting a process: a forked child that still holds the script's
    /// write handle makes the start fail with "text file busy".
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn the_real_runner_returns_what_the_program_printed_and_its_exit() {
        let _serial = serial();
        let ssh = script(
            "prints",
            "echo out; echo 'ssh: connect to host h port 22: Connection refused' >&2; exit 255",
        );
        let cmd = Cmd::version(&spec(&ssh)).expect("a valid spec");
        let output = RealTools.run(&cmd).expect("it ran");
        assert_eq!(output.stdout, "out\n");
        assert!(output.stderr.contains("Connection refused"), "{output:?}");
        assert_eq!(output.exit, Some(255));
    }

    #[test]
    fn the_real_runner_does_not_block_on_a_chatty_program() {
        let _serial = serial();
        let ssh = script(
            "chatty",
            "head -c 2000000 /dev/zero | tr '\\0' x; echo done >&2",
        );
        let cmd = Cmd::version(&spec(&ssh)).expect("a valid spec");
        let output = RealTools.run(&cmd).expect("it ran");
        // Only the first part is kept, and the program still ran to the end.
        assert_eq!(output.stdout.len(), 64 * 1024);
        assert_eq!(output.stderr, "done\n");
        assert_eq!(output.exit, Some(0));
    }

    #[test]
    fn the_real_runner_returns_when_a_helper_keeps_the_pipes_open_after_ssh_exits() {
        let _serial = serial();
        let ssh = script(
            "helper",
            "sleep 30 &\necho ssh: connection reset >&2\nexit 255",
        );
        let cmd = Cmd::version(&spec(&ssh)).expect("a valid spec");
        let started = std::time::Instant::now();
        let output = RealTools.run(&cmd).expect("ssh itself exited");
        assert_eq!(output.exit, Some(255));
        assert!(output.stderr.contains("connection reset"), "{output:?}");
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn the_real_runner_kills_a_call_that_does_not_finish() {
        let _serial = serial();
        let ssh = script("hangs", "sleep 30");
        let cmd = Cmd::version(&spec(&ssh))
            .expect("a valid spec")
            .with_timeout(Duration::from_millis(200));
        let started = std::time::Instant::now();
        assert!(matches!(RealTools.run(&cmd), Err(ToolError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_real_runner_says_a_missing_program_is_missing() {
        let _serial = serial();
        let cmd = Cmd::version(&spec(Path::new("/nonexistent/ssh"))).expect("a valid spec");
        let Err(ToolError::Spawn(e)) = RealTools.run(&cmd) else {
            panic!("expected a spawn error");
        };
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }
}
