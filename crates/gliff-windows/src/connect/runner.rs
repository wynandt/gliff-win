//! The one place the connection checks touch the outside world: running a
//! short ssh and collecting what it printed.
//!
//! [`Tools`] is the seam. Production uses [`RealTools`]; tests use a fake
//! that returns canned [`Output`]. [`Cmd`] cannot hold an arbitrary command:
//! it has one constructor per call the client makes, and each is built from
//! the session's own options, so no check can run with weaker options than
//! the session (section 11.7 of the design).

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
            /// `CREATE_NO_WINDOW` from WinBase.h: no console flashes up.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(ToolError::Spawn)?;
        // Each pipe is drained on its own thread: a full pipe would block
        // the child, and the wait below would never end.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let deadline = Instant::now() + cmd.timeout();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) | Err(_) => {
                    // Killing a child that just exited is harmless.
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        match status {
            Some(status) => {
                let until = Instant::now() + PIPE_GRACE;
                Ok(Output {
                    stdout: collect(stdout, until),
                    stderr: collect(stderr, until),
                    exit: status.code(),
                })
            }
            // A helper the killed ssh started may still hold the pipes open.
            // The readers are not waited for.
            None => Err(ToolError::Timeout),
        }
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
            // A read error ends the text there; what was read is kept.
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
