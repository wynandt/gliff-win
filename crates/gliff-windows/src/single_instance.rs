//! One client per configuration folder: who runs, and what a second launch
//! hands to the first. The decisions and the wire text are pure; the Windows
//! mutex and the `WM_COPYDATA` call are in [`win`].
//!
//! Two clients would fight over the keyboard hook and the clipboard, so a
//! second launch must not start a second client. It forwards what it was asked
//! for to the first one and exits: a target opens a tab there, and a launch
//! with no target brings the window back (it may be hidden in the tray).
//!
//! The instance name comes from the configuration folder, so a test with its
//! own folder runs beside a real client. The mutex is taken by the client
//! only, after `main` has dispatched the helper commands (`--lock-watchdog`
//! and `--restore-lock-policy`): those run the same executable and must never
//! meet it, or the watchdog would exit at once.

use std::path::Path;

use crate::history::HostTarget;

#[cfg(windows)]
pub mod win;

/// What a second launch asks the running client to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Forward {
    /// Bring the window back and to the front.
    Show,
    /// Open a tab for each target, and show the window.
    Open(Vec<HostTarget>),
}

/// What this launch does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// Nobody else runs: become the client.
    Run,
    /// A client runs: hand it this and exit.
    HandOver(Forward),
    /// A client runs and there is nothing to hand it: exit quietly.
    Quit,
}

/// The decision, from whether a client already runs, the targets on the
/// command line, and whether this is the logon start (`--autostart`, kept for a start at logon).
///
/// A logon start never pops the window of a client that is already running,
/// and a launch with no target is "show".
pub fn decide(already_running: bool, targets: &[HostTarget], autostart: bool) -> Launch {
    if !already_running {
        Launch::Run
    } else if autostart {
        Launch::Quit
    } else if targets.is_empty() {
        Launch::HandOver(Forward::Show)
    } else {
        Launch::HandOver(Forward::Open(targets.to_vec()))
    }
}

/// The name of the mutex and of the window class's owner, for a
/// configuration folder: the same folder always gives the same name, however
/// it is spelled in case or with a trailing separator, and two folders give
/// two names (barring a 64-bit hash collision).
pub fn instance_name(config_dir: &Path) -> String {
    let text = config_dir
        .to_string_lossy()
        .to_lowercase()
        .replace('/', "\\");
    let text = text.trim_end_matches('\\');
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("Local\\gliff-windows-{hash:016x}")
}

/// The tag of the `WM_COPYDATA` message, so the first client ignores a message
/// from anything else.
pub const COPY_DATA_TAG: usize = 0x6c69_6666;

/// The largest message the first client accepts: a target is at most 253
/// bytes, so this is well above any real request and well below a hostile one.
pub const MAX_MESSAGE: usize = 16 * 1024;
/// The most targets in one message.
pub const MAX_TARGETS: usize = 32;

const HEADER: &str = "gliff-forward 1";

/// A message from another process that is not a forward request.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("the message is {0} bytes; the most accepted is {MAX_MESSAGE}")]
    TooLong(usize),
    #[error("the message is not text")]
    NotText,
    #[error("the message is not a forward request")]
    BadHeader,
    #[error("unknown request {0:?}")]
    UnknownRequest(String),
    #[error("an open request has no targets")]
    NoTargets,
    #[error("an open request has {0} targets; the most accepted is {MAX_TARGETS}")]
    TooManyTargets(usize),
    #[error("a target is refused: {0}")]
    BadTarget(String),
}

/// The text sent with `WM_COPYDATA`.
pub fn encode(forward: &Forward) -> Vec<u8> {
    let mut text = String::from(HEADER);
    match forward {
        Forward::Show => text.push_str("\nshow"),
        Forward::Open(targets) => {
            text.push_str("\nopen");
            for target in targets {
                text.push('\n');
                text.push_str(target.as_str());
            }
        }
    }
    text.into_bytes()
}

/// Reads a message from another process. Every target is checked as the
/// command line checks it, so a hostile process cannot get a host with an ssh
/// option in it started.
pub fn decode(bytes: &[u8]) -> Result<Forward, DecodeError> {
    if bytes.len() > MAX_MESSAGE {
        return Err(DecodeError::TooLong(bytes.len()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| DecodeError::NotText)?;
    let mut lines = text.split('\n');
    if lines.next() != Some(HEADER) {
        return Err(DecodeError::BadHeader);
    }
    match lines.next() {
        Some("show") if lines.next().is_none() => Ok(Forward::Show),
        Some("show") => Err(DecodeError::UnknownRequest("show with more".to_owned())),
        Some("open") => {
            let targets = lines
                .map(|line| {
                    line.parse::<HostTarget>()
                        .map_err(|e| DecodeError::BadTarget(e.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            match targets.len() {
                0 => Err(DecodeError::NoTargets),
                n if n > MAX_TARGETS => Err(DecodeError::TooManyTargets(n)),
                _ => Ok(Forward::Open(targets)),
            }
        }
        other => Err(DecodeError::UnknownRequest(
            other.unwrap_or_default().chars().take(32).collect(),
        )),
    }
}

#[cfg(test)]
mod tests;
