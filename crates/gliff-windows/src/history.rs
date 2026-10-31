//! The recent hosts: the `user@host` targets that were connected to, most
//! recent first, kept in `recent-hosts.toml`.
//!
//! Pure of Win32, like `config`. The file holds one list and nothing else:
//! no password, key, port or time. It is separate from `config.toml` so that
//! a damaged file in one cannot lose the other.

use std::fmt;
use std::net::Ipv6Addr;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use crate::config::FileError;
use crate::config::FileGuard;
use crate::config::Read;
use crate::config::read_file;

/// The most hosts the list keeps.
pub const MAX_RECENT: usize = 10;

/// The longest target kept, in bytes (the longest DNS name).
const MAX_TARGET_BYTES: usize = 253;

/// Why a text is not a host target.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostTargetError {
    #[error("the target is empty")]
    Empty,
    #[error("the target starts with '-', which ssh would read as an option")]
    LeadingDash,
    #[error("the target has a space or a control character")]
    Unprintable,
    #[error("the target is longer than {MAX_TARGET_BYTES} bytes")]
    TooLong,
    #[error("the target has an empty user or host")]
    EmptyPart,
    #[error("a URL is not accepted: leave out the scheme and write user@host")]
    UrlScheme,
    #[error("the user or host starts with '-', which ssh would read as an option")]
    PartLeadingDash,
    #[error("the target has {0:?}, which is not allowed in a user or host name")]
    BadCharacter(char),
    #[error(
        "a host with ':' must be an IPv6 address (such as fe80::1), without a port or brackets"
    )]
    BadAddress,
    #[error("the zone after '%' must be one name of letters, digits, '.', '_' and '-'")]
    BadZone,
}

/// A target for ssh, `user@host` or a bare host or alias, checked once where
/// it enters. It is the text ssh takes as a destination, with what could
/// turn into an option or into shell syntax left out:
///
/// - The user is everything before the last `@` (as ssh reads it, so a user
///   can hold an `@`, as in `user@domain.com@host`) and may have `\` and
///   non-ASCII letters (`DOMAIN\user@host`). It may not hold a space, a
///   control or invisible character, or a shell character or other
///   character of `USER_REFUSED` (the quotes, `$`, the backtick, `;`, `|`,
///   `&`, `<`, `>`, `:` and `=` among them), and may not start with `-`.
/// - The host is a name or alias of letters (any script), digits and `._-`,
///   or else an IPv6 literal (a `:` is allowed only there) with at most one
///   `%` and a zone of `A-Za-z0-9._-` after it. It may not start with `-`.
/// - At most 253 bytes in all.
///
/// A port (`host:22`), a bracket, a quote, a space, a `%` outside an IPv6
/// literal, a shell character and a URL (`ssh://user@host`) are refused. ssh
/// itself reads a URL, with a port; this client does not, on purpose: the
/// text is one `user@host` that is compared, kept in lists and shown, and a
/// port or other option is for ssh's own configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostTarget(String);

impl HostTarget {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The host part: what follows the last `@`, or the whole target.
    pub fn host(&self) -> &str {
        self.split().1
    }

    /// True when both name the same machine for the same user: the user part
    /// as written, the host part without regard to case.
    pub fn same(&self, other: &Self) -> bool {
        let (user, host) = self.split();
        let (other_user, other_host) = other.split();
        user == other_user && same_host(host, other_host)
    }

    /// `(user including the @ or empty, host)`.
    fn split(&self) -> (&str, &str) {
        match self.0.rfind('@') {
            Some(at) => self.0.split_at(at + 1),
            None => ("", &self.0),
        }
    }
}

/// An address without regard to case, and an IPv6 zone as written (a zone
/// name is the name of an interface, which can differ by case).
fn same_host(host: &str, other: &str) -> bool {
    let (address, zone) = host.split_once('%').unwrap_or((host, ""));
    let (other_address, other_zone) = other.split_once('%').unwrap_or((other, ""));
    address.eq_ignore_ascii_case(other_address) && zone == other_zone
}

/// The characters a user part may not have: shell syntax, globs, and the
/// characters that make a destination look like something else (a port, a
/// URL, an assignment, a home folder).
const USER_REFUSED: &str = "$`;|&<>\"'(){}[]*?!#^%:=/~";

/// Characters that show as nothing or turn the text round, so that a target
/// in a log line or a list reads as another one.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{61c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// Whether `c` may be in the user part.
fn user_char(c: char) -> bool {
    !(USER_REFUSED.contains(c) || is_invisible(c))
}

/// A host name or alias, or an IPv6 literal with an optional zone.
fn check_host(host: &str) -> Result<(), HostTargetError> {
    if !host.contains(':') {
        let bad = host
            .chars()
            .find(|&c| !(c.is_alphanumeric() || "._-".contains(c)));
        return match bad {
            Some(c) => Err(HostTargetError::BadCharacter(c)),
            None => Ok(()),
        };
    }
    let (address, zone) = match host.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (host, None),
    };
    if address.parse::<Ipv6Addr>().is_err() {
        return Err(HostTargetError::BadAddress);
    }
    let zone_ok = |zone: &str| {
        !zone.is_empty()
            && zone
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    if zone.is_some_and(|zone| !zone_ok(zone)) {
        return Err(HostTargetError::BadZone);
    }
    Ok(())
}

impl FromStr for HostTarget {
    type Err = HostTargetError;

    fn from_str(text: &str) -> Result<Self, HostTargetError> {
        if text.is_empty() {
            return Err(HostTargetError::Empty);
        }
        if text.starts_with('-') {
            return Err(HostTargetError::LeadingDash);
        }
        if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(HostTargetError::Unprintable);
        }
        if text.len() > MAX_TARGET_BYTES {
            return Err(HostTargetError::TooLong);
        }
        if text.contains("://") {
            return Err(HostTargetError::UrlScheme);
        }
        let (user, host) = match text.rsplit_once('@') {
            Some((user, host)) => (Some(user), host),
            None => (None, text),
        };
        if host.is_empty() || user.is_some_and(str::is_empty) {
            return Err(HostTargetError::EmptyPart);
        }
        if host.starts_with('-') || user.is_some_and(|user| user.starts_with('-')) {
            return Err(HostTargetError::PartLeadingDash);
        }
        if let Some(c) = user
            .into_iter()
            .flat_map(str::chars)
            .find(|&c| !user_char(c))
        {
            return Err(HostTargetError::BadCharacter(c));
        }
        check_host(host)?;
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Display for HostTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The recent hosts, most recent first, one per machine, at most
/// [`MAX_RECENT`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentHosts(Vec<HostTarget>);

impl RecentHosts {
    /// Puts `host` first. A repeat moves instead of being added, and keeps
    /// its newest spelling; the oldest host past the cap is dropped.
    pub fn record(&mut self, host: HostTarget) {
        self.0.retain(|known| !known.same(&host));
        self.0.insert(0, host);
        self.0.truncate(MAX_RECENT);
    }

    /// Removes one host. Returns whether it was there.
    pub fn forget(&mut self, host: &HostTarget) -> bool {
        let before = self.0.len();
        self.0.retain(|known| !known.same(host));
        self.0.len() != before
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn hosts(&self) -> &[HostTarget] {
        &self.0
    }

    /// The hosts of `lines`, in order (a file's lines are most recent first),
    /// without the lines that are not targets, and how many were dropped.
    fn from_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> (Self, usize) {
        let mut hosts: Vec<HostTarget> = Vec::new();
        let mut dropped = 0;
        for line in lines {
            match line.parse::<HostTarget>() {
                Ok(host) if hosts.iter().any(|known| known.same(&host)) => dropped += 1,
                Ok(host) if hosts.len() < MAX_RECENT => hosts.push(host),
                Ok(_) | Err(_) => dropped += 1,
            }
        }
        (Self(hosts), dropped)
    }
}

/// A table whose `hosts` is not a list cannot be the history: it is damaged.
fn hosts_is_usable(table: &toml::Table) -> bool {
    matches!(table.get("hosts"), None | Some(toml::Value::Array(_)))
}

/// `recent-hosts.toml`: the list and the rules for its file. A save follows
/// the rule of [`FileGuard`], and writes the `hosts` list only: another key
/// the file had is in the copy a save keeps of the text it replaces.
#[derive(Debug)]
pub struct HistoryFile {
    path: PathBuf,
    /// `remember_recent_hosts`: when false nothing is recorded.
    remember: bool,
    hosts: RecentHosts,
    guard: FileGuard,
    /// The list in memory has a change the file does not: a save failed.
    unsaved: bool,
}

impl HistoryFile {
    /// Reads the file in `path`, without changing anything on disk. A missing
    /// file is an empty list. A damaged file is an empty list and a note; a
    /// file that cannot be read is an empty list and a note. What a save
    /// does with the file is the rule of [`FileGuard`]. Lines that are not
    /// targets are dropped, with a note. With `remember` false the list is
    /// empty whatever the file holds, and the file is not read.
    pub fn load(path: &Path, remember: bool) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let mut file = Self {
            path: path.to_owned(),
            remember,
            hosts: RecentHosts::default(),
            guard: FileGuard::unread(path),
            unsaved: false,
        };
        if !remember {
            return (file, notes);
        }
        let name = path.display();
        let (read, guard) = read_file(path);
        file.guard = guard;
        match read {
            Read::Missing => {}
            Read::Damaged(why) => notes.push(format!("{name}: {why}; using an empty list")),
            Read::Unreadable(why) => notes.push(format!(
                "{name}: cannot be read ({why}); using an empty list, and the file is left alone"
            )),
            Read::Parsed(table) => match table.get("hosts") {
                None => {
                    file.guard.repaired(!table.is_empty());
                    file.set_canonical();
                }
                Some(toml::Value::Array(items)) => {
                    let lines: Vec<&str> = items.iter().filter_map(toml::Value::as_str).collect();
                    let not_text = items.len() - lines.len();
                    let (hosts, dropped) = RecentHosts::from_lines(lines);
                    file.hosts = hosts;
                    if dropped + not_text > 0 {
                        notes.push(format!(
                            "{name}: dropped {} entries that are not usable hosts",
                            dropped + not_text
                        ));
                    }
                    file.guard
                        .repaired(dropped + not_text > 0 || table.len() > 1);
                    file.set_canonical();
                }
                Some(_) => {
                    notes.push(format!("{name}: hosts is not a list; using an empty list"));
                }
            },
        }
        (file, notes)
    }

    pub fn hosts(&self) -> &RecentHosts {
        &self.hosts
    }

    pub fn remember(&self) -> bool {
        self.remember
    }

    /// Records a connection. Does nothing when remembering is off.
    pub fn record(&mut self, host: HostTarget) -> Result<(), FileError> {
        if !self.remember {
            return Ok(());
        }
        self.hosts.record(host);
        self.unsaved = true;
        self.save()
    }

    /// Forgets one host. After a save that failed, calling this again writes
    /// the list, whether or not the host was still in it.
    pub fn forget(&mut self, host: &HostTarget) -> Result<(), FileError> {
        if self.hosts.forget(host) {
            self.unsaved = true;
        }
        if !self.unsaved {
            return Ok(());
        }
        self.save()
    }

    /// Forgets every host.
    pub fn clear(&mut self) -> Result<(), FileError> {
        self.hosts.clear();
        self.unsaved = true;
        self.save()
    }

    /// Turns remembering on or off. Off forgets the list and deletes the
    /// file, after the rule of [`FileGuard`]: a file that is damaged, or is
    /// not what was loaded, is kept as a copy first, and one that cannot be
    /// read is not deleted, and this returns an error. Remembering is off
    /// then all the same, and calling this again retries the delete.
    pub fn set_remember(&mut self, remember: bool) -> Result<(), FileError> {
        self.remember = remember;
        if remember {
            return Ok(());
        }
        self.hosts.clear();
        self.guard.remove(hosts_is_usable)?;
        self.unsaved = false;
        Ok(())
    }

    fn save(&mut self) -> Result<(), FileError> {
        let text = self.render()?;
        self.guard.write(&text, hosts_is_usable)?;
        self.unsaved = false;
        Ok(())
    }

    /// The text a save writes for the list in memory.
    fn render(&self) -> Result<String, FileError> {
        let items = self
            .hosts
            .hosts()
            .iter()
            .map(|host| toml::Value::String(host.as_str().to_owned()))
            .collect();
        let mut table = toml::Table::new();
        table.insert("hosts".to_owned(), toml::Value::Array(items));
        toml::to_string(&table).map_err(|e| {
            FileError::new(
                &self.path,
                std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            )
        })
    }

    /// Tells the guard what a save writes for the list as loaded, so that a
    /// file in another layout (comments, spacing) is kept before the first
    /// save replaces it.
    fn set_canonical(&mut self) {
        if let Ok(text) = self.render() {
            self.guard.canonical(&text);
        }
    }
}

#[cfg(test)]
mod tests;
