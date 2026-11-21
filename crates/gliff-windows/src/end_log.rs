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
use crate::connect::diag::is_invisible_format;
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

/// `text` as one log line that is safe to share: [`scrub`] at
/// [`FREE_TEXT_MAX`] characters. Server and link text can carry the same
/// secrets as ssh's own output, so it gets the same treatment.
pub fn one_line(text: &str) -> String {
    scrub(text, FREE_TEXT_MAX)
}

/// Non-blank lines of an ssh stderr file kept on one session line.
const TAIL_LINES: usize = 6;
/// The most characters kept of one stderr line.
const TAIL_LINE_MAX: usize = 200;
/// The most characters kept of one server, link or error text.
const FREE_TEXT_MAX: usize = 512;
/// Bytes read from the end of an ssh stderr file.
const STDERR_TAIL_BYTES: u64 = 16 * 1024;
/// A run of base64 characters this long is key text and is not logged.
const HIDDEN_RUN: usize = 40;
/// A run of hex digits this long is a digest or a token.
const HIDDEN_HEX_RUN: usize = 32;
/// Colon-separated hex pairs this many or more are a fingerprint.
const HIDDEN_HEX_PAIRS: usize = 8;
/// What a private key block becomes.
const PRIVATE_KEY_HIDDEN: &str = "(private key hidden)";
/// Fingerprint prefixes, matched without regard to case.
const DIGEST_PREFIXES: [&str; 5] = ["sha1:", "sha256:", "sha384:", "sha512:", "md5:"];
/// Words that name a secret in `name=value` or `name: value`.
const SECRET_NAMES: [&str; 8] = [
    "pass",
    "pwd",
    "token",
    "secret",
    "auth",
    "cookie",
    "credential",
    "apikey",
];

/// The last [`TAIL_LINES`] non-blank lines of `text`, each at most 200
/// characters, joined by ` | `.
///
/// A private key block becomes `(private key hidden)`. Each line is then
/// [`scrub`]bed before it is cut, so a cut never leaves part of a secret.
/// `None` when nothing is left.
pub fn stderr_tail(text: &str) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    let mut in_key = false;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let upper = line.to_ascii_uppercase();
        let marker = upper.contains("PRIVATE KEY");
        if marker && upper.contains("BEGIN") {
            in_key = true;
        }
        if in_key {
            if lines.last() != Some(&PRIVATE_KEY_HIDDEN) {
                lines.push(PRIVATE_KEY_HIDDEN);
            }
            if marker && upper.contains("END") {
                in_key = false;
            }
            continue;
        }
        lines.push(line);
    }
    if lines.len() > TAIL_LINES {
        lines.drain(..lines.len() - TAIL_LINES);
    }
    let kept: Vec<String> = lines
        .into_iter()
        .map(|line| scrub(line, TAIL_LINE_MAX))
        .filter(|line| !line.is_empty())
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" | "))
    }
}

/// One line safe to share, at most `max` characters: control and invisible
/// format characters removed, then fingerprints, key text, hex digests,
/// secret values and file paths hidden, and only then cut.
fn scrub(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_format(*c))
        .collect();
    let hidden = redact_private_keys(clean.trim());
    let hidden = redact_digest_prefixes(&hidden);
    let hidden = redact_runs(&hidden, |c| c.is_ascii_hexdigit() || c == ':', is_hex_pairs);
    let hidden = redact_runs(&hidden, is_b64, |run| {
        run.len() >= HIDDEN_RUN
            || (run.len() >= HIDDEN_HEX_RUN && run.chars().all(|c| c.is_ascii_hexdigit()))
    });
    let hidden = hide_paths(&redact_secrets(&hidden));
    match hidden.char_indices().nth(max) {
        Some((cut, _)) => format!("{}...", &hidden[..cut]),
        None => hidden,
    }
}

/// A private key block inside one line (a server text whose line breaks
/// were replaced) becomes [`PRIVATE_KEY_HIDDEN`], from its `BEGIN` marker to
/// its `END` marker, or to the end of the text when there is none.
fn redact_private_keys(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    loop {
        let upper = rest.to_ascii_uppercase();
        let Some(key) = upper.find("PRIVATE KEY") else {
            break;
        };
        let Some(begin) = upper[..key].rfind("BEGIN") else {
            out.push_str(&rest[..key + "PRIVATE KEY".len()]);
            rest = &rest[key + "PRIVATE KEY".len()..];
            continue;
        };
        let start = upper[..begin].trim_end_matches('-').len();
        out.push_str(&rest[..start]);
        out.push_str(PRIVATE_KEY_HIDDEN);
        let after = key + "PRIVATE KEY".len();
        rest = match upper[after..].find("END") {
            Some(end) => match upper[after + end..].find("PRIVATE KEY") {
                Some(close) => {
                    let close = after + end + close + "PRIVATE KEY".len();
                    let dashes = rest[close..].len() - rest[close..].trim_start_matches('-').len();
                    &rest[close + dashes..]
                }
                None => "",
            },
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn is_b64(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

/// `hh:hh:...` with at least [`HIDDEN_HEX_PAIRS`] pairs.
fn is_hex_pairs(run: &str) -> bool {
    let pairs: Vec<&str> = run.split(':').collect();
    pairs.len() >= HIDDEN_HEX_PAIRS && pairs.iter().all(|pair| pair.len() == 2)
}

/// `SHA256:<anything fingerprint-like>` and the other prefixes, any case,
/// become `<prefix>(hidden)`.
fn redact_digest_prefixes(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < line.len() {
        let hit = DIGEST_PREFIXES.iter().find(|p| lower[i..].starts_with(*p));
        if let Some(prefix) = hit {
            let body = line[i + prefix.len()..]
                .find(|c: char| !(is_b64(c) || c == ':'))
                .unwrap_or(line.len() - i - prefix.len());
            if body > 0 {
                out.push_str(&line[i..i + prefix.len()]);
                out.push_str("(hidden)");
                i += prefix.len() + body;
                continue;
            }
        }
        let c = line[i..]
            .chars()
            .next()
            .expect("i is a char boundary below len");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Every maximal run of `in_run` characters for which `hide` holds becomes
/// `(hidden)`.
fn redact_runs(line: &str, in_run: impl Fn(char) -> bool, hide: impl Fn(&str) -> bool) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if hide(run) {
            out.push_str("(hidden)");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in line.chars() {
        if in_run(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Hides the value of `name=value`, `name: value` and `name = value` when
/// the name looks like a secret, the word after `Bearer`, `Basic`,
/// `password`, `passphrase`, `token`, `secret` or `apikey`, and
/// everything after ` = ` in ssh's `setting env NAME = value`.
fn redact_secrets(line: &str) -> String {
    if let Some(at) = line.find(" env ")
        && let Some(eq) = line[at..].find(" = ")
    {
        return format!("{} = (hidden)", &line[..at + eq]);
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut hide_next = false;
    for word in words {
        let bare = word.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
        let lower = bare.to_ascii_lowercase();
        let named = lower.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        if matches!(named, "bearer" | "basic") {
            out.push(word.to_owned());
            hide_next = true;
            continue;
        }
        if hide_next && word != "=" {
            out.push("(hidden)".to_owned());
            hide_next = false;
            continue;
        }
        if matches!(
            named,
            "password" | "passphrase" | "token" | "secret" | "apikey"
        ) {
            out.push(word.to_owned());
            hide_next = true;
            continue;
        }
        match word.find(['=', ':']) {
            Some(sep) if names_a_secret(&word[..sep]) => {
                let value = &word[sep + 1..];
                if value.is_empty() {
                    hide_next = true;
                    out.push(word.to_owned());
                } else {
                    out.push(format!("{}(hidden)", &word[..=sep]));
                }
            }
            _ => out.push(word.to_owned()),
        }
        if word == "=" && out.len() >= 2 && names_a_secret(&out[out.len() - 2]) {
            hide_next = true;
        }
    }
    out.join(" ")
}

fn names_a_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase().replace(['_', '-'], "");
    SECRET_NAMES.iter().any(|secret| lower.contains(secret))
}

/// What [`read_stderr_tail`] puts in the log when the path is there but is
/// not a plain file: a link, a folder, or a file with more than one name.
pub const TAIL_NOT_READ: &str = "(not read: the file is not a plain file)";

/// [`stderr_tail`] of at most the last [`STDERR_TAIL_BYTES`] of `path`.
///
/// The read seeks from the end. Bytes are lossy UTF-8. When the seek did
/// not start at the beginning and it landed in the middle of a line, that
/// partial first line is dropped. `None` when the file is missing, cannot
/// be read or nothing is left. [`TAIL_NOT_READ`] when it is not a plain
/// file; nothing is read from it then.
pub fn read_stderr_tail(path: &Path) -> Option<String> {
    let mut file = match open_plain(path) {
        Opened::File(file) => file,
        Opened::Missing => return None,
        Opened::NotPlain => return Some(TAIL_NOT_READ.to_owned()),
    };
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

enum Opened {
    File(File),
    Missing,
    NotPlain,
}

/// Opens `path` only when it is a plain file: not a symlink, junction or
/// folder, and not a hard link (one name only). Any of those planted in
/// place of the stderr file (in a shared `--log-dir`) would copy the tail
/// of whatever it points at into the log. The name is checked first, then
/// the opened handle again, so a swap between the two is caught too.
fn open_plain(path: &Path) -> Opened {
    match std::fs::symlink_metadata(path) {
        Err(_) => return Opened::Missing,
        Ok(meta) if !meta.file_type().is_file() => return Opened::NotPlain,
        Ok(_) => {}
    }
    let Some(file) = open_no_follow(path) else {
        return Opened::Missing;
    };
    let plain = file.metadata().is_ok_and(|meta| meta.file_type().is_file())
        && link_count(&file) == Some(1);
    if plain {
        Opened::File(file)
    } else {
        Opened::NotPlain
    }
}

/// On Windows the open itself does not follow a link. Elsewhere (host
/// tests only) the name check in [`open_plain`] does that job.
fn open_no_follow(path: &Path) -> Option<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        /// `FILE_FLAG_OPEN_REPARSE_POINT`: open the link itself.
        const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(OPEN_REPARSE_POINT);
    }
    options.open(path).ok()
}

#[cfg(windows)]
fn link_count(file: &File) -> Option<u32> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for the life of `file`, and `info` is a
    // valid out-pointer for the call.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &raw mut info) }.ok()?;
    Some(info.nNumberOfLinks)
}

#[cfg(unix)]
fn link_count(file: &File) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    u32::try_from(file.metadata().ok()?.nlink()).ok()
}

fn previous_byte_is_newline(file: &mut File, start: u64) -> Option<bool> {
    let mut prev = [0u8; 1];
    file.seek(SeekFrom::Start(start - 1)).ok()?;
    (file.read(&mut prev).ok()? == 1).then_some(prev[0] == b'\n')
}

#[cfg(test)]
mod tests;
