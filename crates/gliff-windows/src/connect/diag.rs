//! Turns one failed ssh run into a failure class, and a class into words.
//!
//! [`classify`] reads ssh's stderr and exit code. It matches stable ssh
//! phrases in a fixed order, most specific first, and never fails: what it
//! does not know is [`SshFailure::Other`]. [`message`] gives each class its
//! one [`Message`] and one primary [`Button`]; the `match` is exhaustive, so
//! a new class cannot compile without both. The raw stderr goes to the
//! session log only; a message never shows a path, a stack or an error code
//! as its only content.

use super::version::MIN_SERVER;
use super::version::ServerVersion;
use crate::history::HostTarget;

/// Which ssh run failed. A version check has no host or key part to fail,
/// and a session that never printed anything can only be told by its exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The session's own ssh.
    Session,
    /// The extra `<server path> --version` call.
    VersionCheck,
}

/// Why a session or a version check failed. Matched exhaustively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshFailure {
    /// Windows' OpenSSH client is not installed.
    ClientMissing,
    /// The name did not resolve.
    NameNotFound,
    /// The machine answered and nothing listens for ssh.
    Refused,
    /// The machine did not answer (timed out, no route, no network).
    Unreachable,
    /// The machine is not in `known_hosts`, and a session cannot ask.
    HostUnknown,
    /// The machine's identity differs from the one in `known_hosts`.
    HostKeyChanged,
    /// The machine accepted no key from this computer.
    KeyRefused,
    /// ssh will not use a key file other users can read.
    KeyFilePermissions,
    /// ssh offered more keys than the machine allows.
    TooManyKeys,
    /// Signing in worked, and the shell cannot find `gliff-server`.
    ServerMissing,
    /// `gliff-server --version` is older than [`MIN_SERVER`].
    ServerOld { found: ServerVersion },
    /// The server's reply speaks another protocol version.
    ProtocolMismatch,
    /// Anything else: the last line ssh printed, kept for the message.
    Other { last_line: String },
}

/// Classifies one failed ssh run. Pure: the stderr text and the exit code
/// in, the class out.
///
/// The order is the design's match order (11.4), most specific first, with
/// two decisions: a changed host key is tested before an unknown one (ssh
/// prints "Host key verification failed" after both), and an unusable key
/// file before a refused key (ssh prints both lines, and the file is the
/// cause).
///
/// Lines that start with `Warning:` are ignored: they say nothing about why
/// ssh ended (for example, an identity file that does not exist). Windows'
/// ssh ends its lines with CRLF; [`str::lines`] drops the CR, and the text
/// is matched, never compared whole.
pub fn classify(stderr: &str, exit: Option<i32>, phase: Phase) -> SshFailure {
    let lowered: Vec<String> = stderr
        .lines()
        .filter(|line| !line.trim_start().starts_with("Warning:"))
        .map(str::to_lowercase)
        .collect();
    let has = |needle: &str| lowered.iter().any(|line| line.contains(needle));

    if has("is not recognized as an internal or external command") && has("ssh") {
        return SshFailure::ClientMissing;
    }
    if has("remote host identification has changed") || has("has changed and you have requested") {
        return SshFailure::HostKeyChanged;
    }
    if has("host key verification failed") {
        return SshFailure::HostUnknown;
    }
    if has("could not resolve hostname") {
        return SshFailure::NameNotFound;
    }
    if has("connection refused") {
        return SshFailure::Refused;
    }
    if has("connection timed out") || has("no route to host") || has("network is unreachable") {
        return SshFailure::Unreachable;
    }
    // Windows' ssh has no words for some Winsock errors, the no-route ones
    // among them (an IPv6 address with no route prints "Unknown error"). A
    // failed connect with no reason is a machine that cannot be reached.
    if lowered.iter().any(|line| {
        line.contains("connect to host") && line.trim_end().ends_with(": unknown error")
    }) {
        return SshFailure::Unreachable;
    }
    if has("unprotected private key file") || has("bad permissions") {
        return SshFailure::KeyFilePermissions;
    }
    if has("too many authentication failures") {
        return SshFailure::TooManyKeys;
    }
    if has("permission denied (") {
        return SshFailure::KeyRefused;
    }
    // A connection that failed never reached a remote shell, so these come
    // after the connection classes. Only exit 127 speaks for the remote
    // shell: ssh's own errors exit 255, a killed ssh has no exit, and any
    // other code is the server's own business, not a missing server.
    let shell_says_missing =
        has("command not found") || has("no such file or directory") || has(": not found");
    if shell_says_missing && exit == Some(127) {
        return SshFailure::ServerMissing;
    }
    // A session that printed nothing and exited 127 asked a shell for a
    // command it does not have. A version check is not trusted with that.
    if phase == Phase::Session && exit == Some(127) && lowered.iter().all(|l| l.trim().is_empty()) {
        return SshFailure::ServerMissing;
    }
    SshFailure::Other {
        last_line: last_line(stderr),
    }
}

/// The class for a failure to start ssh at all: a program that is not there
/// is a missing client.
pub fn from_spawn_error(error: &std::io::Error) -> SshFailure {
    if error.kind() == std::io::ErrorKind::NotFound {
        SshFailure::ClientMissing
    } else {
        SshFailure::Other {
            last_line: sanitize(&error.to_string()),
        }
    }
}

/// The last line ssh printed that is not a warning (or the last warning if
/// there is nothing else), without control characters, empty when ssh
/// printed nothing.
fn last_line(stderr: &str) -> String {
    let lines = || stderr.lines().rev().filter(|l| !l.trim().is_empty());
    let line = lines()
        .find(|l| !l.trim_start().starts_with("Warning:"))
        .or_else(|| lines().next());
    line.map(sanitize).unwrap_or_default()
}

/// The longest text kept from ssh's output.
const LAST_LINE_MAX: usize = 200;

/// A line safe to show: no control characters (the server can write some
/// into a disconnect reason), bounded.
fn sanitize(line: &str) -> String {
    let clean: String = line
        .trim()
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_format(*c))
        .collect();
    match clean.char_indices().nth(LAST_LINE_MAX) {
        Some((cut, _)) => format!("{}...", &clean[..cut]),
        None => clean,
    }
}

/// Characters that draw nothing but change how the text around them reads:
/// zero-width marks, direction overrides and isolates, line and paragraph
/// separators, the byte order mark.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// `line` with every file path replaced by "(a file)": a word that holds a
/// `/` or a `\` is a path. Punctuation around it stays.
fn hide_paths(line: &str) -> String {
    let is_edge = |c: char| matches!(c, ':' | ';' | ',' | '.' | '\'' | '"' | '(' | ')' | '`');
    let mut out = Vec::new();
    for word in line.split_whitespace() {
        if !word.contains(['/', '\\']) {
            out.push(word.to_owned());
            continue;
        }
        let start = word.len() - word.trim_start_matches(is_edge).len();
        let end = word.trim_end_matches(is_edge).len().max(start);
        out.push(format!("{}(a file){}", &word[..start], &word[end..]));
    }
    out.join(" ")
}

impl SshFailure {
    /// A short stable name for the log and the stats file.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ClientMissing => "client_missing",
            Self::NameNotFound => "name_not_found",
            Self::Refused => "refused",
            Self::Unreachable => "unreachable",
            Self::HostUnknown => "host_unknown",
            Self::HostKeyChanged => "host_key_changed",
            Self::KeyRefused => "key_refused",
            Self::KeyFilePermissions => "key_file_permissions",
            Self::TooManyKeys => "too_many_keys",
            Self::ServerMissing => "server_missing",
            Self::ServerOld { .. } => "server_old",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::Other { .. } => "other",
        }
    }
}

/// The client log's line for a failure: the class, ssh's exit code, the
/// words the person sees, and which file holds ssh's own text.
pub fn log_line(
    failure: &SshFailure,
    target: &HostTarget,
    exit: Option<i32>,
    ssh_log: &str,
) -> String {
    let exit = exit.map_or("none".to_owned(), |code| code.to_string());
    format!(
        "ssh failed: {} (ssh exit {exit}): {} (ssh's own text: {ssh_log})",
        failure.name(),
        message(failure, target).text(),
    )
}

/// What the primary button does. The failure panel draws the label and acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Open Windows' Settings page for optional features.
    OpenOptionalFeatures,
    /// Put the cursor in the address bar.
    EditAddress,
    /// Start the session again.
    TryAgain,
    /// Run the version check again ([`super::check_server`]).
    CheckAgain,
    /// Dismiss the panel; nothing is offered, because a changed identity must
    /// not be waved through.
    Close,
    /// Open the session's ssh log.
    Details,
}

/// A page of the key guide ([`GUIDE_FILE`]) a message links to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guide {
    /// Trust the machine once, by hand.
    TrustMachine,
    /// Create a key and put its public half on the machine.
    SetUpKey,
    /// Remove the extra rights from a key file.
    KeyFileRights,
    /// Name the one key to use.
    PickOneKey,
    /// What to check when a machine's identity changed.
    ChangedIdentity,
}

/// The key guide, relative to the repository root.
pub const GUIDE_FILE: &str = "docs/ssh-keys.md";

impl Guide {
    /// The heading anchor in [`GUIDE_FILE`].
    pub fn anchor(self) -> &'static str {
        match self {
            Self::TrustMachine => "1-trust-the-machine-once",
            Self::SetUpKey => "2-create-a-key",
            Self::KeyFileRights => "if-ssh-refuses-your-key-file",
            Self::PickOneKey => "if-ssh-says-too-many-authentication-failures",
            Self::ChangedIdentity => "if-the-machines-identity-changed",
        }
    }
}

/// What the person reads for one failure: what happened in one sentence (it
/// names the machine), what to do in one more, one primary button, and
/// where it helps a link into the guide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub what: String,
    pub to_do: String,
    pub button: Button,
    pub guide: Option<Guide>,
}

impl Message {
    /// Both sentences, for a log line or a place with one line of text.
    pub fn text(&self) -> String {
        format!("{} {}", self.what, self.to_do)
    }
}

/// The one message for a failure on `target`. Matched exhaustively.
pub fn message(failure: &SshFailure, target: &HostTarget) -> Message {
    let host = target.host();
    let make = |what: String, to_do: &str, button, guide| Message {
        what,
        to_do: to_do.to_owned(),
        button,
        guide,
    };
    match failure {
        SshFailure::ClientMissing => make(
            "Windows' OpenSSH client is missing.".to_owned(),
            "Turn it on in Settings, System, Optional features. Gliff does not change \
             Windows features itself.",
            Button::OpenOptionalFeatures,
            None,
        ),
        SshFailure::NameNotFound => make(
            format!("Can't find \"{host}\"."),
            "Check the spelling, and that you are on the same network or VPN.",
            Button::EditAddress,
            None,
        ),
        SshFailure::Refused => make(
            format!("Nothing is listening for ssh on {host}."),
            "Start the ssh service on the Linux machine.",
            Button::TryAgain,
            None,
        ),
        SshFailure::Unreachable => make(
            format!("{host} did not answer in 10 seconds."),
            "It may be off, asleep, or blocked by a firewall or VPN.",
            Button::TryAgain,
            None,
        ),
        SshFailure::HostKeyChanged => make(
            format!("{host} has a different identity than before."),
            "That can happen after a reinstall. It can also be an attack. Do not continue \
             unless you know why.",
            Button::Close,
            Some(Guide::ChangedIdentity),
        ),
        SshFailure::HostUnknown => make(
            format!("{host} is not known to this computer yet."),
            "Connect to it once with ssh in a terminal, check the fingerprint, and answer \
             yes. Then try again.",
            Button::TryAgain,
            Some(Guide::TrustMachine),
        ),
        SshFailure::KeyRefused => make(
            format!("{host} did not accept a key from this computer."),
            "Gliff signs in with your own ssh and keys; it cannot type a password. The key \
             may not be on the machine, may have a passphrase that ssh-agent does not hold, \
             or the machine may allow passwords only.",
            Button::TryAgain,
            Some(Guide::SetUpKey),
        ),
        SshFailure::KeyFilePermissions => make(
            "Other users can read your key file, so Windows' ssh will not use it.".to_owned(),
            "Change the file's rights; Gliff does not touch it.",
            Button::TryAgain,
            Some(Guide::KeyFileRights),
        ),
        SshFailure::TooManyKeys => make(
            format!("ssh-agent offered more keys than {host} allows."),
            "Name the key to use in your ssh config.",
            Button::TryAgain,
            Some(Guide::PickOneKey),
        ),
        SshFailure::ServerMissing => make(
            format!("gliff is not installed on {host}."),
            &format!(
                "Signing in works, but ssh cannot find gliff-server (version {MIN_SERVER} or \
                 newer). Install it on the Linux machine, or give its path with --server-path \
                 or server_path in config.toml."
            ),
            Button::CheckAgain,
            None,
        ),
        SshFailure::ServerOld { found } => make(
            format!("gliff on {host} is version {found}; this client needs {MIN_SERVER} or newer."),
            "Update it on the Linux machine.",
            Button::CheckAgain,
            None,
        ),
        SshFailure::ProtocolMismatch => make(
            format!("The gliff on {host} speaks a different protocol."),
            "Update it, or this client.",
            Button::CheckAgain,
            None,
        ),
        SshFailure::Other { last_line } => {
            let said = hide_paths(last_line);
            make(
                format!("Something went wrong connecting to {host}."),
                &if said.is_empty() {
                    "ssh gave no reason; Details has its log.".to_owned()
                } else {
                    format!("ssh said: {said}")
                },
                Button::Details,
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests;
