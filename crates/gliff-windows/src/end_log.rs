//! The two lines that say why a session or the client ended.
//!
//! [`session_end_line`] and [`client_exit_line`] build those lines. Neither
//! opens a window. [`read_stderr_tail`] is the only I/O: at most the last
//! 16 KiB of one ssh stderr file.

use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

use crate::connect::diag::hide_paths;
use crate::connect::diag::sanitize;
use crate::session::EndReason;
use crate::session::driver::SessionEnd;
use crate::session::driver::session_name;

/// Why the process is exiting, in the words [`client_exit_line`] prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientExit {
    /// The window was closed. `waited_out` is when a session did not say
    /// Bye within 3 s.
    WindowClosed { waited_out: bool },
    /// The window was destroyed without a close request.
    WindowGone,
    /// Windows is signing out or shutting down.
    WindowsEnding,
    /// Another client was running and the machines were handed to it.
    HandedOver,
    /// Another client was running and there was nothing to hand over.
    AlreadyRunning,
    /// An error stopped the client. The string is the message, before it
    /// is made one line.
    Failed(String),
}

impl std::fmt::Display for ClientExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WindowClosed { waited_out: false } => f.write_str("the window was closed"),
            Self::WindowClosed { waited_out: true } => {
                f.write_str("the window was closed; a session did not say Bye within 3 s")
            }
            Self::WindowGone => f.write_str("the window was destroyed without a close request"),
            Self::WindowsEnding => f.write_str("Windows is signing out or shutting down"),
            Self::HandedOver => {
                f.write_str("another client was running; the machines were handed to it")
            }
            Self::AlreadyRunning => f.write_str("another client was running; nothing to hand over"),
            Self::Failed(message) => write!(f, "error: {}", one_line(message)),
        }
    }
}

/// How the sessions of one run ended.
///
/// `clean` is a Bye whose ssh exited 0. `unclean` is every other session
/// that finished. `start_failures` is sessions that never started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTally {
    pub clean: u32,
    pub unclean: u32,
    pub start_failures: u32,
}

/// Counts [`RunTally`] from the sessions that finished and those that never
/// started.
pub fn run_tally(start_failures: u32, ends: &[SessionEnd]) -> RunTally {
    let clean = ends.iter().filter(|end| clean_end(end)).count();
    let clean = u32::try_from(clean).unwrap_or(u32::MAX);
    let total = u32::try_from(ends.len()).unwrap_or(u32::MAX);
    RunTally {
        clean,
        unclean: total.saturating_sub(clean),
        start_failures,
    }
}

fn clean_end(end: &SessionEnd) -> bool {
    matches!(end.reason, Some(EndReason::Bye)) && end.ssh_exit == Some(0)
}

/// `client exit: <reason>; exit code <n|not known>[; sessions: ...]`.
///
/// The sessions clause is present only when `tally` is `Some`.
pub fn client_exit_line(exit: &ClientExit, code: Option<u8>, tally: Option<RunTally>) -> String {
    let code = match code {
        Some(n) => n.to_string(),
        None => "not known".to_owned(),
    };
    let mut line = format!("client exit: {exit}; exit code {code}");
    if let Some(tally) = tally {
        line.push_str(&format!(
            "; sessions: {} clean, {} not clean, {} failed to start",
            tally.clean, tally.unclean, tally.start_failures
        ));
    }
    line
}

/// `<session>: ended: <reason>; ssh exit <n|none>[; errors: ...][; ssh stderr tail: ...]`.
///
/// Every embedded string is one line: control characters are removed.
/// `tail` is appended only when it is `Some` and not empty after that.
pub fn session_end_line(tag: Option<u32>, end: &SessionEnd, tail: Option<&str>) -> String {
    let mut line = format!(
        "{}: ended: {}; ssh exit {}",
        session_name(tag),
        reason_text(end.reason.as_ref()),
        ssh_exit_text(end.ssh_exit),
    );
    let errors = end
        .errors
        .iter()
        .map(|error| one_line(error))
        .filter(|error| !error.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if !errors.is_empty() {
        line.push_str("; errors: ");
        line.push_str(&errors);
    }
    if let Some(tail) = tail
        .map(one_line)
        .as_deref()
        .map(str::trim)
        .filter(|tail| !tail.is_empty())
    {
        line.push_str("; ssh stderr tail: ");
        line.push_str(tail);
    }
    line
}

fn ssh_exit_text(code: Option<i32>) -> String {
    match code {
        Some(n) => n.to_string(),
        None => "none".to_owned(),
    }
}

fn reason_text(reason: Option<&EndReason>) -> String {
    match reason {
        Some(EndReason::Bye) => "the client said Bye".to_owned(),
        Some(EndReason::Incompatible(text)) => {
            format!("the server is incompatible: {}", one_line(text))
        }
        Some(EndReason::ServerError { code, message }) => {
            format!("the server sent error {code}: {}", one_line(message))
        }
        Some(EndReason::Link(text)) => format!("the link was lost: {}", one_line(text)),
        Some(EndReason::Ssh(failure)) => format!("ssh failed: {}", failure.name()),
        None => "the session thread ended before the protocol ran".to_owned(),
    }
}

/// `text` with control characters removed, so it cannot break a log line.
fn one_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Non-blank lines of an ssh stderr file kept on one session line.
const TAIL_LINES: usize = 6;
/// Bytes read from the end of an ssh stderr file.
const STDERR_TAIL_BYTES: u64 = 16 * 1024;
/// A run of base64 characters this long is key text and is not logged.
const HIDDEN_RUN: usize = 40;

/// The last [`TAIL_LINES`] non-blank lines of `text`, each at most 200
/// characters, joined by ` | `.
///
/// File paths become `(a file)`. A `SHA256:` or `MD5:` fingerprint becomes
/// `SHA256:(hidden)` or `MD5:(hidden)`. A run of 40 or more base64
/// characters becomes `(hidden)`. `None` when nothing is left.
pub fn stderr_tail(text: &str) -> Option<String> {
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() > TAIL_LINES {
        lines.drain(..lines.len() - TAIL_LINES);
    }
    let kept: Vec<String> = lines
        .into_iter()
        .map(redact_stderr_line)
        .filter(|line| !line.is_empty())
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" | "))
    }
}

fn redact_stderr_line(line: &str) -> String {
    let clean = sanitize(line);
    let marked = redact_prefix(&clean, "SHA256:", b64_body_len);
    let marked = redact_prefix(&marked, "MD5:", md5_body_len);
    hide_paths(&redact_base64_runs(&marked))
}

fn is_b64(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')
}

/// How many leading base64 characters `chars` has, when it has at least one.
fn b64_body_len(chars: &[char]) -> Option<usize> {
    let n = chars.iter().take_while(|c| is_b64(**c)).count();
    (n >= 1).then_some(n)
}

fn hex2(chars: &[char], at: usize) -> bool {
    chars.get(at).is_some_and(|c| c.is_ascii_hexdigit())
        && chars.get(at + 1).is_some_and(|c| c.is_ascii_hexdigit())
}

/// Length of `hh(:hh)+` at the start of `chars`.
fn md5_body_len(chars: &[char]) -> Option<usize> {
    if !hex2(chars, 0) {
        return None;
    }
    let mut i = 2;
    let mut pairs = 1;
    while chars.get(i) == Some(&':') && hex2(chars, i + 1) {
        i += 3;
        pairs += 1;
    }
    (pairs >= 2).then_some(i)
}

fn redact_prefix(line: &str, prefix: &str, body_len: fn(&[char]) -> Option<usize>) -> String {
    let chars: Vec<char> = line.chars().collect();
    let prefix: Vec<char> = prefix.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i..].starts_with(&prefix)
            && let Some(n) = body_len(&chars[i + prefix.len()..])
        {
            out.extend(prefix.iter().copied());
            out.push_str("(hidden)");
            i += prefix.len() + n;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_base64_runs(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if is_b64(chars[i]) {
            let start = i;
            while i < chars.len() && is_b64(chars[i]) {
                i += 1;
            }
            if i - start >= HIDDEN_RUN {
                out.push_str("(hidden)");
            } else {
                out.extend(chars[start..i].iter().copied());
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// [`stderr_tail`] of at most the last [`STDERR_TAIL_BYTES`] of `path`.
///
/// The read seeks from the end. Bytes are lossy UTF-8. When the seek did
/// not start at the beginning and it landed in the middle of a line, that
/// partial first line is dropped. `None` when the file cannot be read or
/// nothing is left.
pub fn read_stderr_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(STDERR_TAIL_BYTES);
    let drop_partial = start > 0 && !previous_byte_is_newline(&mut file, start)?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(STDERR_TAIL_BYTES).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let text = if drop_partial {
        match text.find('\n') {
            Some(i) => &text[i + 1..],
            None => "",
        }
    } else {
        text.as_ref()
    };
    stderr_tail(text)
}

fn previous_byte_is_newline(file: &mut File, start: u64) -> Option<bool> {
    let mut prev = [0u8; 1];
    file.seek(SeekFrom::Start(start - 1)).ok()?;
    (file.read(&mut prev).ok()? == 1).then_some(prev[0] == b'\n')
}

#[cfg(test)]
mod tests;
