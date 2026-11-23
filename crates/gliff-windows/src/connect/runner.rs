//! The one place the connection checks touch the outside world: running a
//! short ssh and collecting what it printed.
//!
//! [`Tools`] is the seam. Production uses [`RealTools`]; tests use a fake
//! that returns canned [`Output`]. [`Cmd`] cannot hold an arbitrary command:
//! it has one constructor per call the client makes, and each is built from
//! the session's own options, so no check can run with weaker options than
//! the session.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use crate::ssh::SpecError;
use crate::ssh::SshSpec;
use crate::ssh::invocation;
use crate::ssh::validate;

/// How long one call may take: ssh's own 10 second connect timeout, and a
/// few seconds for the remote command to run.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// One ssh call the client may make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    ssh: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
}

impl Cmd {
    /// `<server path> --version` on the session's target, with the session's
    /// ssh options (still `BatchMode=yes`).
    pub fn version(spec: &SshSpec) -> Result<Self, SpecError> {
        validate(spec)?;
        Ok(Self {
            ssh: spec.ssh.clone(),
            args: invocation(spec, &[&spec.server_path, "--version"]),
            timeout: CALL_TIMEOUT,
        })
    }

    pub fn program(&self) -> &PathBuf {
        &self.ssh
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    #[cfg(all(test, unix))]
    fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }
}

/// What a finished call printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// `None` when the process ended without an exit code.
    pub exit: Option<i32>,
}

/// Why a call gave no [`Output`].
#[derive(Debug)]
pub enum ToolError {
    /// The program could not start.
    Spawn(std::io::Error),
    /// It did not finish in time and was killed.
    Timeout,
    /// `try_wait` failed. The child is killed and reaped when it is still
    /// there. This is a failed wait, including when the deadline has passed.
    Wait(std::io::Error),
}

/// A child the runner can poll, kill, and reap.
trait ChildWait {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus>;
}

impl ChildWait for std::process::Child {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        std::process::Child::try_wait(self)
    }

    fn kill(&mut self) -> std::io::Result<()> {
        std::process::Child::kill(self)
    }

    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        std::process::Child::wait(self)
    }
}

fn reap(child: &mut impl ChildWait) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Polls until the child exits, the deadline passes, or `try_wait` fails.
///
/// A `try_wait` error is [`ToolError::Wait`], including after the deadline.
/// A child still running at the deadline is killed and reaped, and that is
/// [`ToolError::Timeout`].
fn wait_until(
    child: &mut impl ChildWait,
    deadline: Instant,
) -> Result<std::process::ExitStatus, ToolError> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                reap(child);
                return Err(ToolError::Timeout);
            }
            Err(error) => {
                reap(child);
                return Err(ToolError::Wait(error));
            }
        }
    }
}

/// Runs the client's ssh calls.
pub trait Tools {
    fn run(&self, cmd: &Cmd) -> Result<Output, ToolError>;
}

/// Runs the call as a child process with no console window.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealTools;

impl Tools for RealTools {
    fn run(&self, cmd: &Cmd) -> Result<Output, ToolError> {
        let mut command = Command::new(cmd.program());
        command
            .args(cmd.args())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::win32::CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(ToolError::Spawn)?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let deadline = Instant::now() + cmd.timeout();
        // Platform: a helper the killed ssh started can keep the pipes open, so a
        // failed wait returns without joining the readers.
        let status = wait_until(&mut child, deadline)?;
        let until = Instant::now() + PIPE_GRACE;
        Ok(Output {
            stdout: collect(stdout, until),
            stderr: collect(stderr, until),
            exit: status.code(),
        })
    }
}

/// The most text kept from one pipe. A version or an ssh error is a few
/// lines; the rest of a chatty child is read and thrown away so it never
/// blocks on a full pipe.
const KEEP_BYTES: u64 = 64 * 1024;

/// How long to wait for the pipes after ssh itself has exited. A helper it
/// started (a proxy command, a shared connection) can hold them open.
const PIPE_GRACE: Duration = Duration::from_secs(2);

/// A pipe being read: what has arrived so far, and a signal for "ended".
struct Reader {
    kept: Arc<Mutex<Vec<u8>>>,
    ended: mpsc::Receiver<()>,
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Reader {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (send, ended) = mpsc::channel();
    let sink = Arc::clone(&kept);
    std::thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut chunk = [0_u8; 4096];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut kept = sink.lock().unwrap_or_else(|e| e.into_inner());
                let room = (KEEP_BYTES as usize).saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
        let _ = send.send(());
    });
    Reader { kept, ended }
}

/// The text. When the pipes stay open past the grace, what arrived so far.
fn collect(reader: Reader, until: Instant) -> String {
    let _ = reader
        .ended
        .recv_timeout(until.saturating_duration_since(Instant::now()));
    let kept = reader.kept.lock().unwrap_or_else(|e| e.into_inner());
    String::from_utf8_lossy(&kept).into_owned()
}

#[cfg(test)]
mod tests;
