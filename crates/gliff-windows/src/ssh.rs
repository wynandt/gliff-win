//! The ssh child that carries the protocol on its stdin and stdout.
//!
//! Argument building is pure; `spawn` runs on any host (tokio) and adds
//! `CREATE_NO_WINDOW` on Windows. Why a run failed is in [`crate::connect`].

use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;

use crate::cli::DesktopMode;
use crate::connect::Phase;
use crate::connect::SshFailure;
use crate::connect::classify;
use crate::history::HostTarget;

/// Everything needed to start the server over ssh.
#[derive(Debug, Clone)]
pub struct SshSpec {
    pub ssh: PathBuf,
    pub target: HostTarget,
    pub server_path: String,
    pub mode: DesktopMode,
    /// ssh's stderr goes to `<log_dir>/ssh-stderr.log`, or to
    /// `ssh-stderr-<tag>.log` when a tag is given (one file per session of a
    /// window, so tabs do not overwrite each other's), never to a console.
    pub log_dir: PathBuf,
    /// The session number that names this session's log, if there are several.
    pub log_tag: Option<u32>,
}

impl SshSpec {
    /// The file name of this session's ssh stderr log.
    pub fn log_name(&self) -> String {
        match self.log_tag {
            Some(tag) => format!("ssh-stderr-{tag}.log"),
            None => "ssh-stderr.log".to_owned(),
        }
    }
}

/// Why a spec is refused before anything runs.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpecError {
    #[error(
        "server path {0:?} must be an absolute path or a bare command name, \
         and use only [A-Za-z0-9/._+-]"
    )]
    ServerPath(String),
    #[error("output name {0:?} must use only [A-Za-z0-9._-]")]
    Output(String),
}

/// The spawned ssh process and its piped stdio.
pub struct SshLink {
    pub child: Child,
    /// Drop it after writing Bye: upstream gliff-server exits only at
    /// stdin EOF, so a session that keeps it open never sees the exit.
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr_log: PathBuf,
}

/// The remote command words are joined by the remote shell, so every word
/// is checked here rather than quoted.
pub fn validate(spec: &SshSpec) -> Result<(), SpecError> {
    let path = &spec.server_path;
    let chars_ok = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._+-".contains(c));
    // An absolute path, or one word that the remote shell looks up on its
    // `PATH` (so no `/` and no leading `-`, which it would read as an option).
    let path_ok = chars_ok
        && !path.is_empty()
        && (path.starts_with('/') || !(path.contains('/') || path.starts_with('-')));
    if !path_ok {
        return Err(SpecError::ServerPath(spec.server_path.clone()));
    }
    if let DesktopMode::Mirror { output } = &spec.mode {
        let output_ok = !output.is_empty()
            && !output.starts_with('-')
            && output
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
        if !output_ok {
            return Err(SpecError::Output(output.clone()));
        }
    }
    Ok(())
}

/// `-T -o Compression=no -o IPQoS=lowdelay -o BatchMode=yes
/// -o ConnectTimeout=10 -o ServerAliveInterval=10 -- <target> <server_path>
/// --stdio (--headless | --output X)`: upstream `ssh_command`'s options plus
/// BatchMode (never prompt; there is no console to prompt on), a connect
/// timeout (so "did not answer in 10 seconds" is true) and a keepalive. The
/// target follows `--` so it cannot be read as an option.
pub fn ssh_args(spec: &SshSpec) -> Vec<OsString> {
    let mut remote = vec![spec.server_path.as_str(), "--stdio"];
    match &spec.mode {
        DesktopMode::Headless => remote.push("--headless"),
        DesktopMode::Mirror { output } => remote.extend(["--output", output]),
    }
    invocation(spec, &remote)
}

/// The session's own ssh options, `--`, the target, then the remote command
/// words: the one builder behind the session and the version check, so a
/// check never runs with other options than the session.
pub(crate) fn invocation(spec: &SshSpec, remote: &[&str]) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-T",
        "-o",
        "Compression=no",
        "-o",
        "IPQoS=lowdelay",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=10",
        "--",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(OsString::from(spec.target.as_str()));
    args.extend(remote.iter().map(OsString::from));
    args
}

/// Validates the spec, then starts ssh with piped stdio, stderr to the
/// log file, no console window on Windows, and kill-on-drop.
pub fn spawn(spec: &SshSpec) -> std::io::Result<SshLink> {
    validate(spec).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    std::fs::create_dir_all(&spec.log_dir)?;
    let stderr_log = spec.log_dir.join(spec.log_name());
    let stderr = std::fs::File::create(&stderr_log)?;
    let mut cmd = Command::new(&spec.ssh);
    cmd.args(ssh_args(spec))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        /// `CREATE_NO_WINDOW` from WinBase.h: no console flashes up.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("stdin was set to piped");
    let stdout = child.stdout.take().expect("stdout was set to piped");
    Ok(SshLink {
        child,
        stdin,
        stdout,
        stderr_log,
    })
}

/// Arguments for a second, short ssh that runs one command: the same
/// options as [`ssh_args`] plus a 10 s connect timeout, then
/// `-- <target> <command>`. The remote shell parses `command`, which is
/// why [`exec`] only takes a `&'static str`.
pub fn exec_args(spec: &SshSpec, command: &'static str) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "--"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push(OsString::from(spec.target.as_str()));
    args.push(OsString::from(command));
    args
}

/// What a finished [`exec`] printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Why an [`exec`] gave no usable output.
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("ssh could not start: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("timed out after {0:?}")]
    Timeout(std::time::Duration),
    /// Both streams are shown: hyprctl, for one, prints its errors on
    /// stdout (hyprctl/src/main.cpp `log`).
    #[error("exited with {code:?}: {}", printed(.stdout, .stderr))]
    Failed {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
}

/// Longest stream text an [`ExecError`] quotes.
const PRINTED_MAX: usize = 600;

/// What a failed command printed, for its error: each non-empty stream
/// trimmed and labelled, or that it printed nothing.
fn printed(stdout: &str, stderr: &str) -> String {
    let quote = |s: &str| -> String {
        let s = s.trim();
        match s.char_indices().nth(PRINTED_MAX) {
            Some((cut, _)) => format!("{}...", &s[..cut]),
            None => s.to_owned(),
        }
    };
    let parts: Vec<String> = [("stdout", stdout), ("stderr", stderr)]
        .into_iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(name, text)| format!("{name}: {}", quote(text)))
        .collect();
    if parts.is_empty() {
        "no output on stdout or stderr".to_owned()
    } else {
        parts.join("; ")
    }
}

/// Runs a fixed command on the target over its own ssh, with stdin closed
/// and no console window; for `hyprctl` queries next to a live session.
/// The child is killed if `timeout` passes first.
pub async fn exec(
    spec: &SshSpec,
    command: &'static str,
    timeout: std::time::Duration,
) -> Result<ExecOutput, ExecError> {
    validate(spec)
        .map_err(|e| ExecError::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;
    let mut cmd = Command::new(&spec.ssh);
    cmd.args(exec_args(spec, command))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        /// `CREATE_NO_WINDOW` from WinBase.h: no console flashes up.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let child = cmd.spawn().map_err(ExecError::Spawn)?;
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result.map_err(ExecError::Spawn)?,
        // Dropping the future drops the child: kill_on_drop.
        Err(_) => return Err(ExecError::Timeout(timeout)),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Ok(ExecOutput { stdout, stderr })
    } else {
        Err(ExecError::Failed {
            code: output.status.code(),
            stdout,
            stderr,
        })
    }
}

/// [`classify`] on the stderr log and ssh's exit code. An unreadable log is
/// `Other`; an empty one is `Other` with no last line (the message says ssh
/// gave no reason), not "ssh exited", since a timeout kills a running ssh.
pub fn diagnose(stderr_log: &Path, exit: Option<i32>) -> SshFailure {
    match std::fs::read(stderr_log) {
        Ok(bytes) => classify(&String::from_utf8_lossy(&bytes), exit, Phase::Session),
        Err(e) => SshFailure::Other {
            last_line: format!("no ssh log: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::ExecError;
    use super::SpecError;
    use super::SshSpec;
    use super::ssh_args;
    use super::validate;
    use crate::cli::DesktopMode;
    use crate::history::HostTarget;

    fn spec(target: &str, mode: DesktopMode) -> SshSpec {
        SshSpec {
            ssh: PathBuf::from("ssh.exe"),
            target: target.parse().expect("a target"),
            server_path: "/home/user/.local/bin/gliff-server".into(),
            mode,
            log_dir: PathBuf::from("logs"),
            log_tag: None,
        }
    }

    #[test]
    fn each_tagged_session_logs_to_its_own_file() {
        let mut one = spec("user@host", DesktopMode::Headless);
        assert_eq!(one.log_name(), "ssh-stderr.log");
        one.log_tag = Some(3);
        assert_eq!(one.log_name(), "ssh-stderr-3.log");
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|a| a.into_string().expect("ASCII args"))
            .collect()
    }

    #[test]
    fn headless_args_match_the_contract() {
        let args = strings(ssh_args(&spec("user@host", DesktopMode::Headless)));
        assert_eq!(
            args,
            [
                "-T",
                "-o",
                "Compression=no",
                "-o",
                "IPQoS=lowdelay",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=10",
                "--",
                "user@host",
                "/home/user/.local/bin/gliff-server",
                "--stdio",
                "--headless",
            ]
        );
    }

    #[test]
    fn mirror_passes_output_name() {
        let mode = DesktopMode::Mirror {
            output: "eDP-1".into(),
        };
        let args = strings(ssh_args(&spec("h", mode)));
        assert_eq!(args[args.len() - 3..], ["--stdio", "--output", "eDP-1"]);
    }

    #[test]
    fn the_focused_output_is_output_auto() {
        let args = strings(ssh_args(&spec("h", DesktopMode::mirror_focused())));
        assert_eq!(args[args.len() - 3..], ["--stdio", "--output", "auto"]);
        assert_eq!(validate(&spec("h", DesktopMode::mirror_focused())), Ok(()));
    }

    #[test]
    fn target_follows_double_dash() {
        let args = strings(ssh_args(&spec("me@host", DesktopMode::Headless)));
        let at = args
            .iter()
            .position(|a| a == "me@host")
            .expect("target present");
        assert_eq!(args[at - 1], "--");
    }

    #[test]
    fn an_unusual_destination_is_one_argument_after_the_double_dash_on_every_path() {
        for target in [
            "user@domain.com@host",
            "DOMAIN\\user@host",
            "M\u{fc}ller@host",
            "me@m\u{fc}nchen.de",
        ] {
            let s = spec(target, DesktopMode::Headless);
            for args in [strings(ssh_args(&s)), strings(super::exec_args(&s, "true"))] {
                let at = args
                    .iter()
                    .position(|a| a == target)
                    .expect("target present");
                assert_eq!(args[at - 1], "--", "{target:?}");
                assert_eq!(args.iter().filter(|a| *a == "--").count(), 1);
            }
        }
    }

    #[test]
    fn a_spec_cannot_hold_a_target_ssh_would_misread() {
        // The target is a `HostTarget`, so what `validate` runs has passed the
        // one parser; these cannot be built at all.
        for bad in [
            "-oProxyCommand=x",
            "me@-oProxyCommand=x",
            "me@host:22",
            "me@%APPDATA%",
            "ssh://me@host",
            "me@host;id",
        ] {
            assert!(bad.parse::<HostTarget>().is_err(), "{bad:?}");
        }
        for good in ["me@host", "host", "me@fe80::1%eth0"] {
            let s = spec(good, DesktopMode::Headless);
            assert_eq!(validate(&s), Ok(()), "{good:?}");
        }
    }

    #[test]
    fn validate_refuses_relative_or_shell_server_path() {
        for bad in [
            "bin/gliff-server",
            "./gliff-server",
            "-gliff-server",
            "",
            "/bin/x;rm -rf ~",
            "/a b",
            "/$(id)",
            "gliff server",
            "gliff-server;id",
        ] {
            let mut s = spec("h", DesktopMode::Headless);
            s.server_path = bad.into();
            assert_eq!(
                validate(&s),
                Err(SpecError::ServerPath(bad.into())),
                "{bad}"
            );
        }
    }

    #[test]
    fn validate_refuses_shell_output_name() {
        let s = spec(
            "h",
            DesktopMode::Mirror {
                output: "eDP-1;id".into(),
            },
        );
        assert_eq!(validate(&s), Err(SpecError::Output("eDP-1;id".into())));
    }

    #[test]
    fn validate_accepts_a_bare_command_name_and_an_absolute_path() {
        for good in ["gliff-server", "/opt/gliff/bin/gliff-server"] {
            let mut s = spec("h", DesktopMode::Headless);
            s.server_path = good.into();
            assert_eq!(validate(&s), Ok(()), "{good}");
        }
    }

    #[test]
    fn validate_accepts_defaults() {
        assert_eq!(validate(&spec("user@host", DesktopMode::Headless)), Ok(()));
    }

    #[test]
    fn an_empty_log_says_ssh_gave_no_reason_not_that_ssh_exited() {
        // A timeout kills a still-running ssh, so "exited" would mislead.
        let log = std::env::temp_dir().join(format!("gwin-empty-{}.log", std::process::id()));
        std::fs::write(&log, "").expect("write log");
        let failure = super::diagnose(&log, None);
        std::fs::remove_file(&log).ok();
        assert_eq!(
            failure,
            super::SshFailure::Other {
                last_line: String::new()
            }
        );
    }

    #[test]
    fn exec_args_put_the_command_after_the_target() {
        let args = strings(super::exec_args(
            &spec("user@host", DesktopMode::Headless),
            "hyprctl -j monitors all",
        ));
        assert_eq!(
            args,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                "user@host",
                "hyprctl -j monitors all"
            ],
            "exec contract"
        );
    }

    #[test]
    fn a_failed_exec_says_what_the_command_printed_on_both_streams() {
        // hyprctl without HYPRLAND_INSTANCE_SIGNATURE: exit 1, the reason
        // on stdout, nothing on stderr (an empty error message).
        let quiet = ExecError::Failed {
            code: Some(1),
            stdout: "HYPRLAND_INSTANCE_SIGNATURE not set! (is hyprland running?)\n\n".to_owned(),
            stderr: String::new(),
        }
        .to_string();
        assert!(
            quiet.contains("HYPRLAND_INSTANCE_SIGNATURE not set!"),
            "stdout is in the message: {quiet:?}"
        );
        assert!(!quiet.ends_with(": "), "never an empty reason: {quiet:?}");
        let silent = ExecError::Failed {
            code: Some(1),
            stdout: String::new(),
            stderr: String::new(),
        }
        .to_string();
        assert!(
            silent.contains("no output"),
            "a silent failure says so: {silent:?}"
        );
    }
}
