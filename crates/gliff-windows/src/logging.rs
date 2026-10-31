//! The client's log: one file per UTC day, `gliff-windows-YYYY-MM-DD.log`,
//! under `--log-dir` (default `%LOCALAPPDATA%\gliff-windows\logs`). A plain
//! appending writer, not `tracing-appender`: the client logs a few lines per
//! session and per window event, never per frame.

use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Log files kept, today's included.
pub const KEEP_FILES: usize = 14;

/// Size at which a day's file stops growing.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

static LOG: OnceLock<Mutex<Sink>> = OnceLock::new();

struct Sink {
    file: File,
    budget: Budget,
}

/// How much of the size limit one file has used.
#[derive(Debug)]
struct Budget {
    used: u64,
    told: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Fate {
    Write,
    /// The first line over the limit: write the notice instead.
    Notice,
    Drop,
}

impl Budget {
    fn admit(&mut self, len: u64) -> Fate {
        if self.used.saturating_add(len) <= MAX_FILE_BYTES {
            self.used += len;
            Fate::Write
        } else if !self.told {
            self.told = true;
            Fate::Notice
        } else {
            Fate::Drop
        }
    }
}

/// Opens (appends to) today's file in `dir`, creating `dir`, and removes the
/// log files beyond [`KEEP_FILES`]. Returns its path. Later calls keep the
/// first file.
pub fn init(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let now = unix_secs(SystemTime::now());
    let path = dir.join(file_name(now));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let used = file.metadata().map_or(0, |m| m.len());
    prune(dir, KEEP_FILES);
    // A second init keeps the first file; nothing to do with the new one.
    let _ = LOG.set(Mutex::new(Sink {
        file,
        budget: Budget { used, told: false },
    }));
    Ok(path)
}

/// Removes all but the newest `keep` files in `dir` that are named like the
/// client's log (`gliff-windows-YYYY-MM-DD.log`). The dates sort as text.
/// Other files are never touched; a file that cannot be removed is skipped.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| is_log_name(name))
        .collect();
    logs.sort_unstable_by(|a, b| b.cmp(a));
    for old in logs.iter().skip(keep) {
        let _ = std::fs::remove_file(dir.join(old));
    }
}

fn is_log_name(name: &str) -> bool {
    let Some(date) = name
        .strip_prefix("gliff-windows-")
        .and_then(|rest| rest.strip_suffix(".log"))
    else {
        return false;
    };
    date.len() == 10
        && date.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

/// One line, `<UTC timestamp> <message>`. Silent when [`init`] was not
/// called or the write fails: logging never stops the client.
pub fn line(message: &str) {
    let Some(log) = LOG.get() else {
        return;
    };
    let stamp = timestamp(unix_secs(SystemTime::now()));
    let text = format!("{stamp} {message}\n");
    if let Ok(mut sink) = log.lock() {
        let text = match sink.budget.admit(text.len() as u64) {
            Fate::Write => text,
            Fate::Notice => format!(
                "{stamp} log size limit reached; the rest of this file's lines are dropped\n"
            ),
            Fate::Drop => return,
        };
        let _ = sink.file.write_all(text.as_bytes());
    }
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `gliff-windows-YYYY-MM-DD.log` for the UTC day of `secs`.
pub fn file_name(secs: u64) -> String {
    let (y, m, d) = civil(secs / 86_400);
    format!("gliff-windows-{y:04}-{m:02}-{d:02}.log")
}

/// `YYYY-MM-DDTHH:MM:SSZ`.
pub fn timestamp(secs: u64) -> String {
    let (y, m, d) = civil(secs / 86_400);
    let s = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Days since 1970-01-01 to (year, month, day): Howard Hinnant's
/// `civil_from_days`, for days after the epoch.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests;
