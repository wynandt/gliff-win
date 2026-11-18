//! A stand-in for `ssh.exe` that runs `gwin-fake-server` on this machine
//! (verification scaffolding only). Pass it to the client with `--ssh`, and
//! the real client runs a real session with no network, no sshd, no keys and
//! no host trust: the loopback the native verification skill drives.
//!
//! It is the portable twin of `scripts/fake-ssh`, which needs bash. It takes
//! the argument shape `gliff-windows` builds (`ssh::invocation`):
//!
//! ```text
//! [ssh options...] -- <target> <server_path> [args...]
//! ```
//!
//! drops the options and the target, and runs the server with the remaining
//! arguments on the same stdin, stdout and stderr. The server is
//! `<server_path>` when it is an absolute path to an existing file, or else
//! (a bare name such as the default `gliff-server`) the `gwin-fake-server`
//! next to this program. A one-word remote command (`ssh::exec`) has no shell
//! here, so it fails with 127 as a missing command would.
//!
//! As in `scripts/fake-ssh`, `GWIN_FAKE_SSH_STDERR=<file>` prints that file on
//! stderr and exits with `GWIN_FAKE_SSH_EXIT` (default 255) before anything
//! else: a connection failure on demand.

use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;

/// What to run for one invocation.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Serve {
        server: Server,
        args: Vec<OsString>,
    },
    /// Not the shape the client builds; the message goes to stderr.
    Refuse(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Server {
    /// An absolute path that was given.
    Given(PathBuf),
    /// A bare name: the fake server next to this program.
    Sibling,
}

fn plan(args: &[OsString]) -> Plan {
    let Some(dashes) = args.iter().position(|a| a == "--") else {
        return Plan::Refuse("expected '-- <target> <server_path> ...'".into());
    };
    let remote = &args[dashes + 1..];
    let [_target, server, rest @ ..] = remote else {
        return Plan::Refuse("expected '-- <target> <server_path> ...'".into());
    };
    if rest.is_empty() && server.to_string_lossy().contains(' ') {
        return Plan::Refuse(format!(
            "no remote shell to run {:?}",
            server.to_string_lossy()
        ));
    }
    let path = Path::new(server);
    let server = if server.to_string_lossy().starts_with('/') && path.is_file() {
        Server::Given(path.to_owned())
    } else {
        Server::Sibling
    };
    Plan::Serve {
        server,
        args: rest.to_vec(),
    }
}

fn sibling() -> std::io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let name = if cfg!(windows) {
        "gwin-fake-server.exe"
    } else {
        "gwin-fake-server"
    };
    Ok(me.with_file_name(name))
}

fn main() -> ExitCode {
    if let Some(file) = std::env::var_os("GWIN_FAKE_SSH_STDERR") {
        if let Ok(text) = std::fs::read(&file) {
            use std::io::Write;
            let _ = std::io::stderr().write_all(&text);
        }
        let code = std::env::var("GWIN_FAKE_SSH_EXIT")
            .ok()
            .and_then(|c| c.parse::<u8>().ok())
            .unwrap_or(255);
        return ExitCode::from(code);
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (server, args) = match plan(&args) {
        Plan::Serve { server, args } => (server, args),
        Plan::Refuse(why) => {
            eprintln!("gwin-fake-ssh: {why}");
            return ExitCode::from(127);
        }
    };
    let server = match server {
        Server::Given(path) => path,
        Server::Sibling => match sibling() {
            Ok(path) => path,
            Err(e) => {
                eprintln!("gwin-fake-ssh: cannot find myself: {e}");
                return ExitCode::from(255);
            }
        },
    };
    if !server.is_file() {
        eprintln!(
            "gwin-fake-ssh: {}: No such file or directory",
            server.display()
        );
        return ExitCode::from(127);
    }
    match Command::new(&server).args(&args).status() {
        Ok(status) => match status.code() {
            Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(255)),
            None => ExitCode::from(255),
        },
        Err(e) => {
            eprintln!("gwin-fake-ssh: {}: {e}", server.display());
            ExitCode::from(255)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<OsString> {
        line.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn the_session_shape_runs_the_sibling_with_the_server_words() {
        let plan = plan(&words(
            "-T -o Compression=no -o BatchMode=yes -- verify@loopback gliff-server --stdio --headless",
        ));
        assert_eq!(
            plan,
            Plan::Serve {
                server: Server::Sibling,
                args: words("--stdio --headless"),
            }
        );
    }

    #[test]
    fn the_version_check_shape_runs_the_sibling() {
        let plan = plan(&words("-T -- verify@loopback gliff-server --version"));
        assert_eq!(
            plan,
            Plan::Serve {
                server: Server::Sibling,
                args: words("--version"),
            }
        );
    }

    #[test]
    fn a_missing_target_or_dashes_is_refused() {
        assert!(matches!(
            plan(&words("-T verify@loopback")),
            Plan::Refuse(_)
        ));
        assert!(matches!(
            plan(&words("-T -- verify@loopback")),
            Plan::Refuse(_)
        ));
    }

    #[test]
    fn a_shell_command_is_refused() {
        let args = vec![
            OsString::from("--"),
            OsString::from("verify@loopback"),
            OsString::from("hyprctl -j monitors all"),
        ];
        assert!(matches!(plan(&args), Plan::Refuse(_)));
    }
}
