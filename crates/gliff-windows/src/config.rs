//! The config folder and the file rules the settings and the history share.
//!
//! Pure of Win32: nothing here imports the `windows` crate, so nothing here
//! can reach the registry (a test and `scripts/lint-registry` keep it so).
//! Files are read tolerantly, text a save replaces is kept as a numbered
//! `.broken` or `.replaced` copy first, and every save writes a temporary
//! file and renames it.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::Read as _;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::history::HostTarget;
use crate::machine_choices::ChoiceBook;
use crate::machine_choices::Choices;
use crate::settings::Key;
use crate::settings::Layer;

/// The environment variable that names the config folder when
/// `--config-dir` is not given.
pub const CONFIG_DIR_ENV: &str = "GLIFF_WINDOWS_CONFIG_DIR";

/// The folder name under `%APPDATA%`.
const APP_FOLDER: &str = "gliff-windows";

/// The largest file read; a bigger one is treated as damaged.
const MAX_FILE_BYTES: u64 = 1 << 20;

/// The config folder: where `config.toml` and `recent-hosts.toml` live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDir {
    path: PathBuf,
    is_default: bool,
}

/// No config folder could be chosen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no config folder: %APPDATA% is not set; pass --config-dir DIR or set {CONFIG_DIR_ENV}")]
pub struct NoConfigDir;

impl ConfigDir {
    /// `flag` (`--config-dir`), else `env` ([`CONFIG_DIR_ENV`]; empty counts as
    /// unset), else `<app_data>\gliff-windows`.
    pub fn resolve(
        flag: Option<PathBuf>,
        env: Option<OsString>,
        app_data: Option<PathBuf>,
    ) -> Result<Self, NoConfigDir> {
        if let Some(path) = flag {
            return Ok(Self::chosen(path));
        }
        if let Some(path) = env.filter(|value| !value.is_empty()) {
            return Ok(Self::chosen(PathBuf::from(path)));
        }
        match app_data.filter(|dir| !dir.as_os_str().is_empty()) {
            Some(dir) => Ok(Self {
                path: dir.join(APP_FOLDER),
                is_default: true,
            }),
            None => Err(NoConfigDir),
        }
    }

    /// The folder the process environment names: `--config-dir`, else
    /// [`CONFIG_DIR_ENV`], else `%APPDATA%`.
    pub fn from_environment(flag: Option<PathBuf>) -> Result<Self, NoConfigDir> {
        Self::resolve(
            flag,
            std::env::var_os(CONFIG_DIR_ENV),
            std::env::var_os("APPDATA").map(PathBuf::from),
        )
    }

    fn chosen(path: PathBuf) -> Self {
        Self {
            path,
            is_default: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True for the folder under `%APPDATA%`, false when a flag or the
    /// environment chose it. Tests and checks never use the default one.
    pub fn is_default(&self) -> bool {
        self.is_default
    }

    /// `config.toml` in this folder.
    pub fn config_file(&self) -> PathBuf {
        self.path.join("config.toml")
    }

    /// `recent-hosts.toml` in this folder.
    pub fn history_file(&self) -> PathBuf {
        self.path.join("recent-hosts.toml")
    }
}

/// A file that could not be written, with the path in the message.
#[derive(Debug, thiserror::Error)]
#[error("{}: {source}", path.display())]
pub struct FileError {
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
    /// Worked out from the error the system gave, before any message was
    /// put round it, since a wrapped error no longer has its code.
    transient: bool,
}

impl FileError {
    pub(crate) fn new(path: &Path, source: io::Error) -> Self {
        let transient = is_transient(&source);
        Self {
            path: path.to_owned(),
            source,
            transient,
        }
    }

    /// An error whose class was decided by the caller.
    fn classed(path: &Path, source: io::Error, transient: bool) -> Self {
        Self {
            path: path.to_owned(),
            source,
            transient,
        }
    }

    /// The save was refused because `original` stopped the file being read:
    /// the message says the file is left as it is, and the class is that of
    /// `original`.
    fn unreadable(path: &Path, original: &io::Error) -> Self {
        Self::unreadable_on(cfg!(windows), path, original)
    }

    fn unreadable_on(windows: bool, path: &Path, original: &io::Error) -> Self {
        Self::classed(
            path,
            io::Error::new(
                original.kind(),
                format!("not saved, the file is left as it is: {original}"),
            ),
            is_transient_on(windows, original),
        )
    }

    /// True when trying again later may work (a lock, a permission being
    /// fixed). False when only a change to the file or the folder, or a
    /// person, helps: a file that is damaged, too large, or has too many
    /// copies next to it, a folder that is gone, a file that is read-only
    /// after the rename was tried again.
    pub fn is_transient(&self) -> bool {
        self.transient
    }
}

/// Whether `error` is the kind that goes away by itself.
fn is_transient(error: &io::Error) -> bool {
    is_transient_on(cfg!(windows), error)
}

/// [`is_transient`], with whether this is Windows given, so that a test on
/// any system can use the Windows codes.
fn is_transient_on(windows: bool, error: &io::Error) -> bool {
    // Windows says "sharing violation" (32) or "lock violation" (33) when
    // another program has the file open, "delete pending" (303) when one
    // has deleted it and not closed it, and "user-mapped section" (1224)
    // when one has it mapped.
    let lock = windows && matches!(error.raw_os_error(), Some(32 | 33 | 303 | 1224));
    lock || matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::TimedOut
            | io::ErrorKind::ResourceBusy
    )
}

/// What reading a TOML file gave.
#[derive(Debug)]
pub(crate) enum Read {
    /// No such file: the same as an empty one.
    Missing,
    Parsed(toml::Table),
    /// Present and its content is not usable (not TOML, not UTF-8, too
    /// large). The text says why. The file must be kept before a save
    /// replaces it.
    Damaged(String),
    /// Present, and the system would not let it be read (a lock, a
    /// permission). Nothing is known about its content, so it is never
    /// called damaged and never replaced.
    Unreadable(String),
}

/// The bytes a file held, before any parsing.
enum Raw {
    Missing,
    Bytes(Vec<u8>),
    TooLarge,
    Unreadable(io::Error),
}

/// Reads at most [`MAX_FILE_BYTES`] of `path`: a bigger file is not read
/// into memory.
fn read_raw(path: &Path) -> Raw {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Raw::Missing,
        // Windows refuses to open a folder as a file with "access denied",
        // which reads as a permission that may be fixed; say what it is.
        Err(e) if path.is_dir() => {
            return Raw::Unreadable(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!("{e}; the name is a folder"),
            ));
        }
        Err(e) => return Raw::Unreadable(e),
    };
    let mut bytes = Vec::new();
    match file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 > MAX_FILE_BYTES => Raw::TooLarge,
        Ok(_) => Raw::Bytes(bytes),
        Err(e) => Raw::Unreadable(e),
    }
}

/// What a file held when it was last read or written by this process.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    /// Never read (the history is off, so it was not loaded).
    Unknown,
    Missing,
    Bytes(Vec<u8>),
    TooLarge,
    Unreadable,
}

/// Parses `bytes` as a TOML table.
fn parse_bytes(bytes: &[u8]) -> Read {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Read::Damaged("not UTF-8 text".to_owned());
    };
    match text.parse::<toml::Table>() {
        Ok(table) => Read::Parsed(table),
        Err(e) => Read::Damaged(e.message().to_owned()),
    }
}

/// Reads and parses `path`, and remembers what the file held.
pub(crate) fn read_file(path: &Path) -> (Read, FileGuard) {
    let (read, seen) = match read_raw(path) {
        Raw::Missing => (Read::Missing, Seen::Missing),
        Raw::Unreadable(e) => (Read::Unreadable(e.to_string()), Seen::Unreadable),
        Raw::TooLarge => (
            Read::Damaged(format!("larger than {MAX_FILE_BYTES} bytes")),
            Seen::TooLarge,
        ),
        Raw::Bytes(bytes) => (parse_bytes(&bytes), Seen::Bytes(bytes)),
    };
    (read, FileGuard::new(path, seen))
}

/// The rule every save of a file follows. A save is the client's state
/// replacing the file, so it must never lose text a person wrote:
///
/// - The file is read again at every save, since it may have changed since
///   it was loaded, by a person or by another client.
/// - Text the save is about to replace is first kept as a numbered copy
///   next to it. A file that is not usable (not TOML, not UTF-8, too
///   large) goes to `.broken`. A usable file that is not what was loaded
///   (changed since, loading repaired it, or its layout is not the one the
///   client writes: comments, spacing, order) goes to `.replaced`. A file
///   that is what was loaded, and is the client's own text, is replaced
///   without a copy.
/// - The same copy is used again when a save fails and is tried again with
///   the file unchanged, so a file does not pile up copies of itself.
/// - A file that could not be read at load is replaced only when it is
///   missing now or parses now. One that is damaged now is left as it is.
/// - What the guard remembers (the text last seen) changes only after a
///   write succeeds, so a failed save can be tried again, and the next one
///   still compares with the right text.
/// - The file is read once more just before the rename; if it changed
///   since the first read, that text is kept too.
#[derive(Debug)]
pub(crate) struct FileGuard {
    path: PathBuf,
    seen: Seen,
    /// Loading dropped or repaired something, so the text in the file is
    /// not what the next write holds.
    repaired: bool,
    /// The text the client writes for the state it loaded. A file that holds
    /// something else (comments, another layout) is kept before the first
    /// save replaces it.
    canonical: Option<Vec<u8>>,
    /// The last copy this guard made: its suffix and where it went.
    kept: Option<(&'static str, PathBuf)>,
}

impl FileGuard {
    fn new(path: &Path, seen: Seen) -> Self {
        Self {
            path: path.to_owned(),
            seen,
            repaired: false,
            canonical: None,
            kept: None,
        }
    }

    /// The guard of a file that has not been read.
    pub(crate) fn unread(path: &Path) -> Self {
        Self::new(path, Seen::Unknown)
    }

    /// Says that loading dropped or repaired some of the content.
    pub(crate) fn repaired(&mut self, repaired: bool) {
        self.repaired = repaired;
    }

    /// Says what the client writes for the state it loaded, unchanged.
    pub(crate) fn canonical(&mut self, text: &str) {
        self.canonical = Some(text.as_bytes().to_vec());
    }

    /// Makes the way clear for a write over the file: see the type. A
    /// parsed file is usable only if `usable` says so. Returns what the file
    /// held when it was looked at.
    fn prepare(&mut self, usable: &impl Fn(&toml::Table) -> bool) -> Result<Seen, FileError> {
        let path = self.path.clone();
        let (now, usable_now) = match read_raw(&path) {
            Raw::Missing => return Ok(Seen::Missing),
            Raw::Unreadable(e) => return Err(FileError::unreadable(&path, &e)),
            Raw::TooLarge => (Seen::TooLarge, false),
            Raw::Bytes(bytes) => {
                let ok = matches!(parse_bytes(&bytes), Read::Parsed(table) if usable(&table));
                (Seen::Bytes(bytes), ok)
            }
        };
        if self.seen == Seen::Unreadable && !usable_now {
            return Err(FileError::classed(
                &path,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "not saved, the file is left as it is: it could not be read \
                     when it was loaded, and it is damaged now",
                ),
                false,
            ));
        }
        let own_layout = match (&self.canonical, &now) {
            (Some(canonical), Seen::Bytes(bytes)) => canonical == bytes,
            _ => true,
        };
        if usable_now && now == self.seen && !self.repaired && own_layout {
            return Ok(now);
        }
        self.keep(if usable_now { ".replaced" } else { ".broken" })?;
        Ok(now)
    }

    /// Keeps a copy of the file, unless the copy this guard made last, with
    /// this suffix, still holds the same text.
    fn keep(&mut self, suffix: &'static str) -> Result<(), FileError> {
        if let Some((kept_suffix, copy)) = &self.kept
            && *kept_suffix == suffix
            && same_content(&self.path, copy)
        {
            return Ok(());
        }
        if let Some(copy) = keep_copy(&self.path, suffix)? {
            self.kept = Some((suffix, copy));
        }
        Ok(())
    }

    /// The last look before the file is replaced: if it is not what
    /// [`Self::prepare`] saw, someone wrote in between, and that text is
    /// kept as well. A file that cannot be read now is left to the rename.
    fn recheck(
        &mut self,
        examined: &Seen,
        usable: &impl Fn(&toml::Table) -> bool,
    ) -> Result<(), FileError> {
        let (now, usable_now) = match read_raw(&self.path) {
            Raw::Missing | Raw::Unreadable(_) => return Ok(()),
            Raw::TooLarge => (Seen::TooLarge, false),
            Raw::Bytes(bytes) => {
                let ok = matches!(parse_bytes(&bytes), Read::Parsed(table) if usable(&table));
                (Seen::Bytes(bytes), ok)
            }
        };
        if now == *examined {
            return Ok(());
        }
        self.keep(if usable_now { ".replaced" } else { ".broken" })
    }

    /// Replaces the file with `text`, after [`Self::prepare`].
    pub(crate) fn write(
        &mut self,
        text: &str,
        usable: impl Fn(&toml::Table) -> bool,
    ) -> Result<(), FileError> {
        self.write_between(text, usable, || {})
    }

    /// [`Self::write`], running `between` after the file is looked at and
    /// before the last look: where a test changes the file.
    fn write_between(
        &mut self,
        text: &str,
        usable: impl Fn(&toml::Table) -> bool,
        between: impl FnOnce(),
    ) -> Result<(), FileError> {
        check_size(&self.path, text)?;
        let examined = self.prepare(&usable)?;
        between();
        let path = self.path.clone();
        write_atomic_checked(&path, text, || self.recheck(&examined, &usable))?;
        self.seen = Seen::Bytes(text.as_bytes().to_vec());
        self.repaired = false;
        self.canonical = None;
        Ok(())
    }

    /// Deletes the file, after [`Self::prepare`]. One that is already gone
    /// is not an error.
    pub(crate) fn remove(
        &mut self,
        usable: impl Fn(&toml::Table) -> bool,
    ) -> Result<(), FileError> {
        let examined = self.prepare(&usable)?;
        self.recheck(&examined, &usable)?;
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(FileError::new(&self.path, e)),
        }
        self.seen = Seen::Missing;
        self.repaired = false;
        self.canonical = None;
        Ok(())
    }
}

/// Whether the files at `a` and `b` hold the same bytes; false when either
/// cannot be read.
fn same_content(a: &Path, b: &Path) -> bool {
    let compare = || -> io::Result<bool> {
        let (mut a, mut b) = (fs::File::open(a)?, fs::File::open(b)?);
        if a.metadata()?.len() != b.metadata()?.len() {
            return Ok(false);
        }
        let (mut left, mut right) = ([0u8; 8192], [0u8; 8192]);
        loop {
            let n = a.read(&mut left)?;
            if n == 0 {
                return Ok(b.read(&mut right)? == 0);
            }
            let mut got = 0;
            while got < n {
                match b.read(&mut right[got..n])? {
                    0 => return Ok(false),
                    m => got += m,
                }
            }
            if left[..n] != right[..n] {
                return Ok(false);
            }
        }
    };
    compare().unwrap_or(false)
}

/// `<path>` with `suffix` appended to the file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// How often a rename that fails with a transient error is tried, and the
/// pause between tries. A virus scanner or an indexer can hold a file for a
/// moment. An error that will not go away (the folder is gone, the disk is
/// full) is not tried again.
const RENAME_TRIES: u32 = 5;
const RENAME_PAUSE: Duration = Duration::from_millis(50);

/// `rename(from, to)` through `rename`, tried up to `tries` times, `pause`
/// apart, while the error is transient; the last error is the result.
fn rename_with_retry(
    from: &Path,
    to: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    tries: u32,
    pause: Duration,
) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= tries || !is_transient(&e) => return Err(e),
            Err(_) => {
                attempt += 1;
                std::thread::sleep(pause);
            }
        }
    }
}

/// The temporary file for one write: unique to this process and this write,
/// so two clients, or two writes, never share one.
fn temporary_name(path: &Path) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    with_suffix(path, &format!(".{}.{n}.tmp", std::process::id()))
}

/// A failed write. A permission that is refused is not given by waiting
/// (the rename was tried again before this, a read-only file stays so), and
/// a caller that tries again would loop: it is not transient here.
fn write_failure(path: &Path, error: io::Error) -> FileError {
    let transient = is_transient(&error) && error.kind() != io::ErrorKind::PermissionDenied;
    FileError::classed(path, error, transient)
}

/// Refuses a text that the next load would call damaged for its size.
fn check_size(path: &Path, text: &str) -> Result<(), FileError> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(FileError::new(
            path,
            io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!(
                    "not saved: the file would be {} bytes, and more than {MAX_FILE_BYTES} \
                     bytes is not read back",
                    text.len()
                ),
            ),
        ));
    }
    Ok(())
}

/// Writes `text` to a temporary file next to `path`, flushes it, and renames
/// it over `path`, so a reader sees the old file or the new one. The folder
/// is created if needed. A text larger than [`MAX_FILE_BYTES`] is refused:
/// the next load would call it damaged. The rename is tried a few times
/// while the error is transient, since a lock on the target is usually
/// short. The temporary file is removed if a step fails; if that fails too
/// (the disk went away), a `.tmp` file is left, and nothing reads it. A
/// destination that is read-only on Windows cannot be replaced: the save
/// fails with an access error after the tries, and that error is not
/// transient.
///
/// `before_rename` runs once the temporary file is complete and just before
/// the rename; its error stops the save.
pub(crate) fn write_atomic_checked(
    path: &Path,
    text: &str,
    before_rename: impl FnOnce() -> Result<(), FileError>,
) -> Result<(), FileError> {
    let fail = |e| write_failure(path, e);
    check_size(path, text)?;
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    let tmp = temporary_name(path);
    let staged = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = staged {
        let _ = fs::remove_file(&tmp);
        return Err(fail(e));
    }
    if let Err(e) = before_rename() {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // On Windows, `rename` replaces an existing file.
    let renamed = rename_with_retry(
        &tmp,
        path,
        |a, b| fs::rename(a, b),
        RENAME_TRIES,
        RENAME_PAUSE,
    );
    if let Err(e) = renamed {
        let _ = fs::remove_file(&tmp);
        return Err(fail(e));
    }
    Ok(())
}

/// The most copies of one kind kept side by side.
const MAX_COPIES: u32 = 100;

/// Copies `path` to `<name><suffix>`, or `<name><suffix>.1`, `.2`, and so
/// on when that is taken. Each copy is created exclusively, so one that
/// exists (even a dangling link) is never replaced: it may be the only copy
/// of something a person wrote. Only "not found" means a name is free; any
/// other error stops the save. The original stays where it is, so a save
/// that fails afterwards loses nothing. A file that is already gone needs
/// no copy. Returns where the copy went.
pub(crate) fn keep_copy(path: &Path, suffix: &str) -> Result<Option<PathBuf>, FileError> {
    let mut source = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(FileError::new(path, e)),
    };
    for n in 0..MAX_COPIES {
        let name = if n == 0 {
            with_suffix(path, suffix)
        } else {
            with_suffix(path, &format!("{suffix}.{n}"))
        };
        let mut target = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&name)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // Windows says "access denied", not "exists", when the name is a
            // folder.
            Err(_) if name.is_dir() => continue,
            Err(e) => return Err(FileError::new(&name, e)),
        };
        let copied = io::copy(&mut source, &mut target).and_then(|_| target.sync_all());
        if let Err(e) = copied {
            drop(target);
            let _ = fs::remove_file(&name);
            return Err(FileError::new(&name, e));
        }
        return Ok(Some(name));
    }
    Err(FileError::new(
        path,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "not saved: {MAX_COPIES} copies named {} exist already; \
                 remove some, then save again",
                with_suffix(path, suffix).display()
            ),
        ),
    ))
}

/// The machines of `config.toml` (`machines`, `open`, `shown`). The rules
/// that hold at all times: no machine twice (the host name is compared
/// without case, as in the recent hosts), every open machine is in
/// `machines`, and the shown machine is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabSet {
    machines: Vec<HostTarget>,
    open: Vec<HostTarget>,
    shown: Option<HostTarget>,
}

impl TabSet {
    /// The remembered machines, in the user's order.
    pub fn machines(&self) -> &[HostTarget] {
        &self.machines
    }

    /// The machines to reconnect at start, in tab order.
    pub fn open(&self) -> &[HostTarget] {
        &self.open
    }

    /// The machine that was shown, if one was and it is open.
    pub fn shown(&self) -> Option<&HostTarget> {
        self.shown.as_ref()
    }

    /// Adds a machine at the end; one already there stays where it is, with
    /// its spelling.
    pub fn remember(&mut self, machine: HostTarget) {
        if !self.machines.iter().any(|known| known.same(&machine)) {
            self.machines.push(machine);
        }
    }

    /// Removes a machine from all three.
    pub fn forget(&mut self, machine: &HostTarget) {
        self.machines.retain(|known| !known.same(machine));
        self.open.retain(|known| !known.same(machine));
        if self.shown.as_ref().is_some_and(|shown| shown.same(machine)) {
            self.shown = None;
        }
    }

    /// Opens a machine, remembering it.
    pub fn open_machine(&mut self, machine: &HostTarget) {
        self.remember(machine.clone());
        if !self.open.iter().any(|known| known.same(machine)) {
            self.open.push(machine.clone());
        }
    }

    /// Makes a machine the shown one, opening it if it is not.
    pub fn set_shown(&mut self, machine: &HostTarget) {
        self.open_machine(machine);
        self.shown = Some(machine.clone());
    }

    /// The tab set a file holds: what does not parse is dropped, and what
    /// breaks the rules is repaired (a repeat is dropped; an open machine
    /// joins the list; a shown machine that is not open is not shown). Every
    /// drop and repair is a note, so none is silent.
    fn from_table(table: &toml::Table, notes: &mut Vec<String>) -> Self {
        let mut tabs = Self::default();
        for machine in targets(table, "machines", notes) {
            if tabs.machines.iter().any(|known| known.same(&machine)) {
                notes.push(format!(
                    "machines in the file: {machine} is listed twice; kept once"
                ));
            } else {
                tabs.machines.push(machine);
            }
        }
        for machine in targets(table, "open", notes) {
            if tabs.open.iter().any(|known| known.same(&machine)) {
                notes.push(format!(
                    "open in the file: {machine} is listed twice; kept once"
                ));
                continue;
            }
            if !tabs.machines.iter().any(|known| known.same(&machine)) {
                notes.push(format!(
                    "open in the file: {machine} was not in machines; added there"
                ));
            }
            tabs.open_machine(&machine);
        }
        let shown = table.get("shown").and_then(|value| match value {
            toml::Value::String(text) => text.parse::<HostTarget>().ok(),
            _ => None,
        });
        match (table.contains_key("shown"), shown) {
            (false, _) => {}
            (true, None) => {
                notes.push("shown in the file is ignored: expected a target".to_owned())
            }
            (true, Some(shown)) => {
                tabs.shown = tabs.open.iter().find(|open| open.same(&shown)).cloned();
                if tabs.shown.is_none() {
                    notes.push(format!(
                        "shown in the file: {shown} is not open; nothing is shown"
                    ));
                }
            }
        }
        tabs
    }

    fn insert_into(&self, table: &mut toml::Table) {
        let list = |hosts: &[HostTarget]| {
            toml::Value::Array(
                hosts
                    .iter()
                    .map(|host| toml::Value::String(host.as_str().to_owned()))
                    .collect(),
            )
        };
        for (key, hosts) in [("machines", &self.machines), ("open", &self.open)] {
            if !hosts.is_empty() {
                table.insert(key.to_owned(), list(hosts));
            }
        }
        if let Some(shown) = &self.shown {
            table.insert(
                "shown".to_owned(),
                toml::Value::String(shown.as_str().to_owned()),
            );
        }
    }
}

/// The targets of a list in a file; an entry that is not one is dropped and
/// counted in a note.
fn targets(table: &toml::Table, key: &str, notes: &mut Vec<String>) -> Vec<HostTarget> {
    let Some(value) = table.get(key) else {
        return Vec::new();
    };
    let toml::Value::Array(items) = value else {
        notes.push(format!(
            "{key} in the file is ignored: expected a list of targets"
        ));
        return Vec::new();
    };
    let parsed: Vec<HostTarget> = items
        .iter()
        .filter_map(|item| item.as_str()?.parse().ok())
        .collect();
    if parsed.len() != items.len() {
        notes.push(format!(
            "{key} in the file: dropped {} entries that are not targets",
            items.len() - parsed.len()
        ));
    }
    parsed
}

/// The top-level keys the client owns; every other key of the file is kept.
const OWNED_KEYS: [&str; 5] = ["machines", "open", "shown", "settings", "overrides"];

/// `config.toml`: the tab set, the `[settings]` table and the per-machine
/// `[overrides]` tables.
#[derive(Debug)]
pub struct ConfigFile {
    path: PathBuf,
    tabs: TabSet,
    /// What the user chose; only these are written back.
    settings: Layer,
    /// Top-level keys the client does not know, and `[settings]` keys it does
    /// not know or could not read, as the file had them. They are written
    /// back unchanged, so a newer client's keys, and a value the user has
    /// yet to fix, survive a save. Comments do not, which is why a file that
    /// is not in the client's own layout is kept as a copy before the first
    /// save.
    kept_top: toml::Table,
    kept_settings: toml::Table,
    /// The per-machine choices of `[overrides]`, which the window changes
    /// and the session starter reads.
    choices: ChoiceBook,
    /// What the file held when it was loaded or last written.
    guard: FileGuard,
}

impl ConfigFile {
    /// Reads the file in `path`, without changing anything on disk. A
    /// missing file is the defaults. A damaged file is the defaults and a
    /// note. A file that cannot be read is the defaults and a note. What a
    /// save then does with the file is the rule of [`FileGuard`]: the text
    /// it replaces is kept as a copy first. A key that is not known is kept;
    /// a setting that does not parse is a note and is left out, and the rest
    /// still loads. A machine entry that is not a target is dropped, and
    /// the copy of the file is what keeps it.
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let (read, guard) = read_file(path);
        let mut file = Self {
            path: path.to_owned(),
            tabs: TabSet::default(),
            settings: Layer::default(),
            kept_top: toml::Table::new(),
            kept_settings: toml::Table::new(),
            choices: ChoiceBook::default(),
            guard,
        };
        match read {
            Read::Missing => {}
            Read::Damaged(why) => {
                notes.push(format!("{}: {why}; using the defaults", path.display()));
            }
            Read::Unreadable(why) => notes.push(format!(
                "{}: cannot be read ({why}); using the defaults, and the file is left alone",
                path.display()
            )),
            Read::Parsed(table) => {
                file.tabs = TabSet::from_table(&table, &mut notes);
                let mut repaired = !notes.is_empty();
                file.kept_top = table
                    .iter()
                    .filter(|(key, _)| !OWNED_KEYS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                match table.get("settings") {
                    None => {}
                    Some(toml::Value::Table(settings)) => {
                        let (layer, setting_notes) = Layer::from_table(settings);
                        let chosen = layer.to_table();
                        file.kept_settings = settings
                            .iter()
                            .filter(|(key, _)| !chosen.contains_key(key.as_str()))
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect();
                        file.settings = layer;
                        notes.extend(setting_notes);
                    }
                    Some(_) => {
                        repaired = true;
                        notes.push("settings in the file is ignored: expected a table".to_owned());
                    }
                }
                match table.get("overrides") {
                    None => {}
                    Some(toml::Value::Table(overrides)) => {
                        *file.choices.borrow_mut() = Choices::from_table(overrides, &mut notes);
                    }
                    Some(_) => {
                        repaired = true;
                        notes.push("overrides in the file is ignored: expected a table".to_owned());
                    }
                }
                file.guard.repaired(repaired);
                if let Ok(text) = file.render() {
                    file.guard.canonical(&text);
                }
            }
        }
        (file, notes)
    }

    pub fn tabs(&self) -> &TabSet {
        &self.tabs
    }

    pub fn tabs_mut(&mut self) -> &mut TabSet {
        &mut self.tabs
    }

    pub fn settings(&self) -> &Layer {
        &self.settings
    }

    /// The per-machine choices (`[overrides]`), shared with whoever reads
    /// them at connect time.
    pub fn choices(&self) -> &ChoiceBook {
        &self.choices
    }

    pub fn settings_mut(&mut self) -> &mut Layer {
        &mut self.settings
    }

    /// Gives a setting no value, and forgets the text the file held for it,
    /// if that did not parse (such a value is kept, so that it can be
    /// fixed, until it is replaced or reset here).
    pub fn reset_setting(&mut self, key: Key) {
        self.settings.clear(key);
        self.kept_settings.remove(key.name());
    }

    /// Writes the file atomically, by the rule of [`FileGuard`]: the file
    /// is read again, and what this save replaces is kept as a numbered
    /// `.broken` or `.replaced` copy unless it is what was loaded. A file
    /// that could not be read at load and is damaged now is not touched:
    /// this returns an error.
    pub fn save(&mut self) -> Result<(), FileError> {
        let text = self.render()?;
        self.guard.write(&text, |_| true)
    }

    /// The text a save writes for the state in memory.
    fn render(&self) -> Result<String, FileError> {
        let mut table = self.kept_top.clone();
        self.tabs.insert_into(&mut table);
        let mut settings = self.kept_settings.clone();
        settings.extend(self.settings.to_table());
        if !settings.is_empty() {
            table.insert("settings".to_owned(), toml::Value::Table(settings));
        }
        let overrides = self.choices.borrow().to_table();
        if !overrides.is_empty() {
            table.insert("overrides".to_owned(), toml::Value::Table(overrides));
        }
        toml::to_string(&table)
            .map_err(|e| FileError::new(&self.path, io::Error::new(io::ErrorKind::InvalidData, e)))
    }
}

#[cfg(test)]
mod tests;
