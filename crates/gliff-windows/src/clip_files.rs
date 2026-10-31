//! Files on the clipboard, the Windows ends: walking the copied paths into an
//! offer, naming the files of a peer's offer so Windows accepts them, and
//! the directory they are spooled to.
//!
//! Upstream's walker and spool are Unix-shaped (`\` joins, UTF-8-only names,
//! a `/proc` sweep), so these are ours; the types are upstream's.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use gliff_proto::ClipboardFile;
use gliff_proto::clipboard as proto;
use gliff_transport::clipboard::files::ListError;
use gliff_transport::clipboard::files::LocalFiles;

use crate::clip_log;
use crate::clip_log::Event;

mod offer;

pub use offer::validate_offer;

/// The longest path part Windows accepts, in UTF-16 units.
const MAX_PART_UNITS: usize = 255;
/// Paths from this many characters on get the `\\?\` prefix; the limit for
/// plain paths is 260 with the terminator.
const LONG_PATH: usize = 240;
/// Characters a path may have for a program without long path support,
/// without the terminator.
const MAX_PATH_UNITS: usize = 259;
/// The least room a top-level name gets, whatever the spool path takes.
const MIN_TOP_UNITS: usize = 32;
/// The longest extension kept when a long name is shortened, in UTF-16 units.
const MAX_EXTENSION_UNITS: usize = 32;

/// Device names Windows reserves, with or without an extension.
const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

// Naming files that arrive from Linux.

/// A path part Windows accepts, made from one a Linux program chose.
///
/// `<>:"\|?*` and control characters become `_`; a trailing dot or space is
/// removed; a reserved device name gets a `_` in front; a part over 255
/// UTF-16 units is shortened, keeping a short extension; an empty result is
/// `_`.
pub fn sanitize_component(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut part = replaced.trim_end_matches(['.', ' ']).to_owned();
    if part.is_empty() {
        return "_".to_owned();
    }
    if is_reserved(&part) {
        part.insert(0, '_');
    }
    shorten(&part)
}

/// Whether the part, up to its first dot, is a device name.
fn is_reserved(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if RESERVED.contains(&upper.as_str()) {
        return true;
    }
    let numbered = |prefix: &str| {
        upper.strip_prefix(prefix).is_some_and(|rest| {
            let mut chars = rest.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('0'..='9' | '\u{B9}' | '\u{B2}' | '\u{B3}'), None)
            )
        })
    };
    numbered("COM") || numbered("LPT")
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The part cut to [`MAX_PART_UNITS`], with a short extension kept.
fn shorten(part: &str) -> String {
    shorten_to(part, MAX_PART_UNITS)
}

/// The part cut to `limit` UTF-16 units, with a short extension kept.
fn shorten_to(part: &str, limit: usize) -> String {
    if utf16_len(part) <= limit {
        return part.to_owned();
    }
    let (stem, extension) = match part.rfind('.') {
        Some(at) if at > 0 && utf16_len(&part[at..]) <= MAX_EXTENSION_UNITS => part.split_at(at),
        _ => (part, ""),
    };
    let room = limit.saturating_sub(utf16_len(extension));
    let mut out = String::new();
    let mut units = 0;
    for c in stem.chars() {
        units += c.len_utf16();
        if units > room {
            break;
        }
        out.push(c);
    }
    // A cut can leave a trailing dot or space that Windows would drop.
    let out = out.trim_end_matches(['.', ' ']).to_owned();
    let out = if out.is_empty() { "_".to_owned() } else { out };
    out + extension
}

/// Where each file of a peer's offer goes under a spool: the sanitized
/// relative path of every entry, by index. Two parts that differ only in case
/// or that collide after sanitizing within one directory get ` (2)`, ` (3)`
/// and so on before the extension. The offer must have passed
/// `validate_offer`.
pub fn spool_paths(files: &[ClipboardFile]) -> Vec<PathBuf> {
    spool_paths_within(files, MAX_PART_UNITS)
}

/// The room a top-level name has under `spool_dir` for the whole path to stay
/// under the 260 characters a program without long path support opens: at
/// least [`MIN_TOP_UNITS`], at most [`MAX_PART_UNITS`].
pub fn top_level_units(spool_dir: &Path) -> usize {
    let used = utf16_len(&spool_dir.to_string_lossy()) + 1;
    MAX_PATH_UNITS
        .saturating_sub(used)
        .clamp(MIN_TOP_UNITS, MAX_PART_UNITS)
}

/// [`spool_paths`], with top-level names cut to `top_units` UTF-16 units.
pub fn spool_paths_within(files: &[ClipboardFile], top_units: usize) -> Vec<PathBuf> {
    // Original relative prefix to its sanitized path, and the lower-cased
    // sanitized paths in use.
    let mut mapped: HashMap<String, String> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        let mut original = String::new();
        let mut current = String::new();
        for part in file.path.split('/') {
            if !original.is_empty() {
                original.push('/');
            }
            original.push_str(part);
            if let Some(known) = mapped.get(&original) {
                current.clone_from(known);
                continue;
            }
            let limit = if current.is_empty() {
                top_units
            } else {
                MAX_PART_UNITS
            };
            let name = unique_part(
                &current,
                &shorten_to(&sanitize_component(part), limit),
                &taken,
                limit,
            );
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(&name);
            taken.insert(current.to_lowercase());
            mapped.insert(original.clone(), current.clone());
        }
        out.push(current.split('/').collect::<PathBuf>());
    }
    out
}

/// `name`, or `name (2)` and on, so that `parent/name` is free in `taken`.
fn unique_part(parent: &str, name: &str, taken: &HashSet<String>, limit: usize) -> String {
    let path = |part: &str| {
        if parent.is_empty() {
            part.to_lowercase()
        } else {
            format!("{parent}/{part}").to_lowercase()
        }
    };
    if !taken.contains(&path(name)) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(at) if at > 0 && utf16_len(&name[at..]) <= MAX_EXTENSION_UNITS => name.split_at(at),
        _ => (name, ""),
    };
    (2u32..)
        .map(|n| shorten_to(&format!("{stem} ({n}){extension}"), limit))
        .find(|candidate| !taken.contains(&path(candidate)))
        .expect("an unbounded counter finds a free name")
}

/// A path Windows takes when it is long: `C:\a` as `\\?\C:\a` and
/// `\\server\share\a` as `\\?\UNC\server\share\a`. Short paths, paths that
/// have the prefix already, and relative paths are returned as they are.
pub fn extended(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text.chars().count() < LONG_PATH || text.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return PathBuf::from(format!(r"\\?\{text}"));
    }
    if let Some(unc) = text.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{unc}"));
    }
    path.to_path_buf()
}

// Listing files that are copied on Windows.

/// Walk the copied paths into an offer. Directories are listed recursively;
/// symbolic links and junctions inside them are skipped so a loop cannot run
/// the walk forever (a cloud placeholder is a plain file). Entries are in
/// name order within a directory and have `/`-separated paths. Roots with one
/// name are offered as `name (2)` and on. Fails when a
/// path has no name (a drive root), cannot be read, or the caps are
/// exceeded; the caller then offers nothing rather than part of a copy.
pub fn list_files(roots: &[PathBuf]) -> Result<LocalFiles, ListError> {
    let mut out = LocalFiles::default();
    let mut total = 0u64;
    let mut names: HashSet<String> = HashSet::new();
    for root in roots {
        let name = root
            .file_name()
            .ok_or_else(|| ListError::NoName(root.clone()))?;
        let io_path = extended(root);
        let meta = std::fs::metadata(&io_path).map_err(|e| ListError::Io(root.clone(), e))?;
        let size = if meta.is_dir() { 0 } else { meta.len() };
        let name = free_name(&name.to_string_lossy(), meta.is_dir(), &names);
        names.insert(name.clone());
        walk(&io_path, &name, meta.is_dir(), size, &mut out, &mut total)?;
    }
    proto::validate_files(&out.entries)?;
    Ok(out)
}

/// `name`, or `name (2)` and on (before the extension of a file), so that no
/// copied top-level path repeats: the server refuses an offer that does.
fn free_name(name: &str, is_dir: bool, taken: &HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(at) if !is_dir && at > 0 => name.split_at(at),
        _ => (name, ""),
    };
    (2u32..)
        .map(|n| format!("{stem} ({n}){extension}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unbounded counter finds a free name")
}

fn walk(
    path: &Path,
    rel: &str,
    is_dir: bool,
    size: u64,
    out: &mut LocalFiles,
    total: &mut u64,
) -> Result<(), ListError> {
    if out.entries.len() >= proto::MAX_FILE_ENTRIES {
        return Err(proto::FilesError::TooMany(out.entries.len() + 1).into());
    }
    if !is_dir {
        *total = total.saturating_add(size);
        if *total > proto::MAX_FILES_TOTAL {
            return Err(proto::FilesError::TooLarge(*total).into());
        }
    }
    out.entries.push(ClipboardFile {
        path: rel.to_owned(),
        size,
        dir: is_dir,
    });
    out.paths.push(path.to_path_buf());
    if !is_dir {
        return Ok(());
    }
    let io_error = |p: &Path, e: io::Error| ListError::Io(p.to_path_buf(), e);
    let mut children = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| io_error(path, e))? {
        let entry = entry.map_err(|e| io_error(path, e))?;
        let kind = entry.file_type().map_err(|e| io_error(&entry.path(), e))?;
        if kind.is_symlink() {
            continue;
        }
        let size = if kind.is_dir() {
            0
        } else {
            entry
                .metadata()
                .map_err(|e| io_error(&entry.path(), e))?
                .len()
        };
        children.push((entry.file_name(), entry.path(), kind.is_dir(), size));
    }
    children.sort();
    for (name, child, child_is_dir, size) in children {
        let child_rel = format!("{rel}/{}", name.to_string_lossy());
        walk(&child, &child_rel, child_is_dir, size, out, total)?;
    }
    Ok(())
}

// The spool.

/// A directory that holds one remote offer's files on this side. Removed when
/// dropped.
#[derive(Debug)]
pub struct Spool {
    dir: PathBuf,
}

static NEXT_SPOOL: AtomicU32 = AtomicU32::new(0);

impl Spool {
    /// Make a directory in `base` for one offer, named from the process id
    /// and a counter. See [`Spool::sweep_stale`] for what a run leaves.
    pub fn create_in(base: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(extended(base))?;
        let pid = std::process::id();
        let n = NEXT_SPOOL.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("{pid}-{n}"));
        std::fs::create_dir(extended(&dir))?;
        Ok(Self { dir })
    }

    /// Remove the spool directories under `base` that belong to processes
    /// `alive` says are gone, at the start of a run. A directory of a running
    /// process (a second client, or this one) is never touched, and neither
    /// is anything not named `<process id>-<number>`.
    pub fn sweep_stale(base: &Path, alive: impl Fn(u32) -> bool) {
        let own = std::process::id();
        Self::remove_matching(base, |pid| pid != own && !alive(pid));
    }

    /// Remove this process's spool directories under `base`, at the end of
    /// a run.
    pub fn remove_own(base: &Path) {
        let own = std::process::id();
        Self::remove_matching(base, |pid| pid == own);
    }

    fn remove_matching(base: &Path, wanted: impl Fn(u32) -> bool) {
        let Ok(entries) = std::fs::read_dir(extended(base)) else {
            return;
        };
        for entry in entries.flatten() {
            let owner = entry
                .file_name()
                .to_str()
                .and_then(|name| name.split_once('-'))
                .and_then(|(pid, n)| Some((pid.parse::<u32>().ok()?, n.parse::<u32>().ok()?)))
                .map(|(pid, _)| pid);
            if !owner.is_some_and(&wanted) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path();
            if let Err(error) = std::fs::remove_dir_all(&path) {
                clip_log::emit(Event::SpoolNotRemoved { kind: error.kind() });
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(extended(&self.dir))
            && error.kind() != io::ErrorKind::NotFound
        {
            clip_log::emit(Event::SpoolNotRemoved { kind: error.kind() });
        }
    }
}

/// Whether a process with this id is running. A process that cannot be asked
/// about (no access) counts as running, so its spool is kept.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
        use windows::Win32::System::Threading::GetExitCodeProcess;
        use windows::Win32::System::Threading::OpenProcess;
        use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
        /// `STILL_ACTIVE`: the exit code of a process that has not ended.
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: the handle is closed before returning; the exit code is a
        // live local.
        unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => {
                    let mut code = 0;
                    let known = GetExitCodeProcess(handle, &mut code).is_ok();
                    let _ = CloseHandle(handle);
                    !known || code == STILL_ACTIVE
                }
                Err(error) => error.code() == ERROR_ACCESS_DENIED.to_hresult(),
            }
        }
    }
    #[cfg(not(windows))]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
}

// The `CF_HDROP` payload.

/// The registered format that tells a drop target to copy, not move.
pub const DROP_EFFECT_FORMAT: &str = "Preferred DropEffect";
/// `DROPEFFECT_COPY`.
pub const DROPEFFECT_COPY: u32 = 1;
/// Bytes of the `DROPFILES` header.
const DROPFILES_HEADER: usize = 20;

/// A `CF_HDROP` body for these paths: a `DROPFILES` header (wide characters,
/// not in a non-client area), each path ending in NUL, and a final NUL.
pub fn hdrop_bytes(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&u32::try_from(DROPFILES_HEADER).unwrap_or(20).to_le_bytes());
    // The drop point (x, y) and fNC.
    out.extend_from_slice(&[0; 12]);
    // fWide.
    out.extend_from_slice(&1u32.to_le_bytes());
    for path in paths {
        for unit in path.to_string_lossy().encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// The paths in a `CF_HDROP` body. A body that is too short, has a bad
/// offset, or lists narrow characters gives `None`.
pub fn parse_hdrop(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    let header = bytes.get(..DROPFILES_HEADER)?;
    let offset = usize::try_from(u32::from_le_bytes(header[..4].try_into().ok()?)).ok()?;
    let wide = u32::from_le_bytes(header[16..20].try_into().ok()?) != 0;
    if !wide || offset < DROPFILES_HEADER {
        return None;
    }
    let (units, _) = bytes.get(offset..)?.as_chunks::<2>();
    let units: Vec<u16> = units.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let mut paths = Vec::new();
    for name in units.split(|unit| *unit == 0) {
        if name.is_empty() {
            break;
        }
        paths.push(PathBuf::from(String::from_utf16_lossy(name)));
    }
    Some(paths)
}

#[cfg(test)]
mod tests;
