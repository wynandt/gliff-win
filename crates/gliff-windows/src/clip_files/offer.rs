//! Checking the file list of a peer's offer, the same on every OS.
//!
//! Upstream's `validate_files` reads each path with the host's path parser.
//! On Windows that parser sees `a:b?.txt` as a drive prefix and a name, so
//! one legal Linux name made the client refuse a whole offer. The paths of an
//! offer are `/`-separated text from another machine, so they are read here
//! as text and never as host paths. What Windows cannot hold in a name is
//! not refused: [`super::sanitize_component`] renames it on disk.
//!
//! This file must not use `std::path`; a test checks that.

use std::collections::HashSet;

use gliff_proto::ClipboardFile;
use gliff_proto::clipboard::FilesError;
use gliff_proto::clipboard::MAX_FILE_ENTRIES;
use gliff_proto::clipboard::MAX_FILE_PATH_BYTES;
use gliff_proto::clipboard::MAX_FILES_TOTAL;

/// Whether the entries of an offer are acceptable: every path made of plain
/// `/`-separated parts (none empty, `.` or `..`, no NUL, not starting with
/// `/`) and distinct, a folder with no size, and the counts and sizes within
/// upstream's caps. A backslash or a colon is part of a name here, as it is
/// on Linux; the sanitizer turns both into `_` before anything is written.
pub fn validate_offer(files: &[ClipboardFile]) -> Result<(), FilesError> {
    if files.len() > MAX_FILE_ENTRIES {
        return Err(FilesError::TooMany(files.len()));
    }
    let path_bytes: usize = files.iter().map(|f| f.path.len()).sum();
    if path_bytes > MAX_FILE_PATH_BYTES {
        return Err(FilesError::PathsTooLong(path_bytes));
    }
    let mut total: u64 = 0;
    let mut seen = HashSet::new();
    for f in files {
        if !is_plain_relative(&f.path) || !seen.insert(f.path.as_str()) || (f.dir && f.size != 0) {
            return Err(FilesError::BadPath(f.path.clone()));
        }
        total = total.saturating_add(f.size);
    }
    if total > MAX_FILES_TOTAL {
        return Err(FilesError::TooLarge(total));
    }
    Ok(())
}

fn is_plain_relative(path: &str) -> bool {
    !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
