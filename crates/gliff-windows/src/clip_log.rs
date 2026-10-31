//! The clipboard's event log: one line per event, in the client's log file.
//!
//! The log must never hold what was copied. That is enforced by the types in
//! this module, not by care at each call site: an [`Event`] is built from
//! numbers, durations, [`MimeName`]s and the fixed enums below, and nothing
//! else. No `String`, path, byte buffer or error text can be put in one, so
//! no clipboard content, file name, path, directory name, or text derived
//! from them can reach the log. [`emit`] is the only function in the
//! clipboard modules that writes to the log; `scripts/lint-clip-log` keeps it
//! so and keeps this file free of free-form text.
//!
//! A mime type is a type of data, not data, but the string a peer or a
//! program sends is still outside text. [`MimeName`] therefore never keeps
//! it: it picks the name from a fixed table of well known types and logs
//! `other` for anything else.
//!
//! The lines and what each field means are in `docs/clipboard.md`, "The log".

use std::cell::Cell;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::io::ErrorKind;
use std::time::Duration;

use gliff_proto::ClipboardFile;
use gliff_proto::ClipboardItem;
use gliff_proto::ClipboardMsg;
use gliff_proto::clipboard::FilesError;
use gliff_transport::clipboard::TransferError;
use gliff_transport::clipboard::files::ListError;

use crate::clip_formats::ConvertError;

/// The mime types the log names. Everything else is `other`. A name here is
/// a type of data the clipboard bridge knows (see `clip_formats`) or one a
/// copying program commonly advertises.
// clip-log: table
const KNOWN_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
    "text/html",
    "text/rtf",
    "application/rtf",
    "text/uri-list",
    "text/x-moz-url",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/bmp",
    "image/webp",
    "image/tiff",
    "image/svg+xml",
    "application/pdf",
    "application/json",
    "application/octet-stream",
    "x-special/gnome-copied-files",
    "x-kde-passwordManagerHint",
];

/// A mime type name from [`KNOWN_MIMES`], or `other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MimeName(&'static str);

impl MimeName {
    /// The name `mime` is logged under. The input is compared, never kept.
    // clip-log: mime-name
    pub fn of(mime: &str) -> Self {
        let mime = mime.trim();
        let name = KNOWN_MIMES
            .iter()
            .find(|known| known.eq_ignore_ascii_case(mime))
            .copied();
        Self(name.unwrap_or("other"))
    }
}

impl Display for MimeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// The mime types of an offer, as names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MimeList(Vec<MimeName>);

impl MimeList {
    // clip-log: mime-name
    pub fn of(mimes: &[String]) -> Self {
        Self(mimes.iter().map(|mime| MimeName::of(mime)).collect())
    }
}

impl Display for MimeList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("none");
        }
        for (index, mime) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(",")?;
            }
            write!(f, "{mime}")?;
        }
        Ok(())
    }
}

/// How many files an offer lists (folders count as entries) and the sum of
/// their sizes. Never a name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Files {
    pub count: u32,
    pub bytes: u64,
}

impl Files {
    pub fn of(files: &[ClipboardFile]) -> Self {
        Self {
            count: u32::try_from(files.len()).unwrap_or(u32::MAX),
            bytes: files
                .iter()
                .filter(|file| !file.dir)
                .fold(0u64, |sum, file| sum.saturating_add(file.size)),
        }
    }
}

impl Display for Files {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "files={} file_bytes={}", self.count, self.bytes)
    }
}

/// What a request or a paste asks for: a flavour, or one file by its index
/// in the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Mime(MimeName),
    File(u32),
}

impl Item {
    pub fn of(item: &ClipboardItem) -> Self {
        match item {
            ClipboardItem::Mime(mime) => Self::Mime(MimeName::of(mime)),
            ClipboardItem::File(index) => Self::File(*index),
        }
    }
}

impl Display for Item {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mime(mime) => write!(f, "mime={mime}"),
            Self::File(index) => write!(f, "file_index={index}"),
        }
    }
}

/// Whether a change of the Windows clipboard was this client's own write,
/// and how that was told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Own {
    /// Someone else's change: a copy is offered.
    No,
    /// The sequence number is the one this client's last write left.
    Sequence,
    /// This client's window owns the clipboard.
    Owner,
    /// This client's marker format is on the clipboard.
    Marker,
}

impl Own {
    fn name(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Sequence => "sequence",
            Self::Owner => "owner",
            Self::Marker => "marker",
        }
    }
}

/// Why the local offer was withdrawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokeWhy {
    /// The Windows clipboard changed, so what was offered is stale.
    LocalChange,
    /// The remote side offered something, and that is now the clipboard.
    RemoteOffer,
    /// The tab was left for another one.
    TabLeft,
    /// Clipboard sharing was turned off for the session.
    SharingOff,
}

impl RevokeWhy {
    fn name(self) -> &'static str {
        match self {
            Self::LocalChange => "local_change",
            Self::RemoteOffer => "remote_offer",
            Self::TabLeft => "tab_left",
            Self::SharingOff => "sharing_off",
        }
    }
}

/// Why something was refused, skipped or failed: a fixed list, never text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    // A request this client refused (it sends an Abort).
    /// There is no local offer.
    NoOffer,
    /// The request names a serial that is not the current offer.
    StaleSerial,
    /// The flavour is not in the offer.
    NotOffered,
    /// The file index is not in the offer.
    UnknownFile,
    /// The offer was withdrawn while the body was being read.
    RevokedWhileReading,
    /// The Windows thread did not answer a read in time.
    ReadTimeout,
    /// The answer to a read never came (the thread or the session is gone).
    ReadUnanswered,
    /// The body is over the item limit.
    OverLimit,
    // A read of the Windows clipboard that failed.
    /// The offer generation moved on: a newer copy replaced the offered one.
    ClipboardChanged,
    /// The clipboard sequence number moved: something wrote to the
    /// clipboard since the offer was built.
    ClipboardSequenceMoved,
    /// Another program holds the clipboard open.
    ClipboardBusy,
    /// The clipboard thread was busy.
    ThreadBusy,
    /// A different machine's tab is shown, so a background tab may not read.
    NotShown,
    /// The clipboard was not ready (no thread yet).
    NotReady,
    /// The clipboard has no data in the format the flavour needs.
    FormatMissing,
    /// A picture could not be converted.
    ImageConversion,
    // A paste of the remote offer that failed.
    /// The offer the paste names was replaced.
    OfferReplaced,
    /// The offer lists no files.
    NoFiles,
    /// The offered file list failed validation.
    FilesInvalid,
    /// A file arrived with fewer or more bytes than offered.
    WrongFileSize,
    /// The remote side sent an Abort for the transfer.
    PeerAborted,
    /// The link to the remote side ended.
    PeerGone,
    /// No progress within the idle timeout.
    IdleTimeout,
    /// The remote side sent chunks that break the protocol.
    ChunkProtocol,
    /// The paste was cancelled by the user.
    Cancelled,
    /// The transfer was dropped without an outcome (the session ended).
    Dropped,
    // Offers this client could not make.
    /// The copy is marked private (a password manager's).
    Private,
    /// Nothing in the copy can be forwarded.
    NothingForwardable,
    /// A copied path has no file name (a drive root).
    NoFileName,
    /// More files than the limit.
    TooManyFiles,
    /// More bytes than the limit.
    FilesTooLarge,
    /// A file operation failed, with the kind of error and nothing else.
    Io(ErrorKind),
}

impl Cause {
    /// The cause of a failed transfer. The error's text is not used: an
    /// I/O error's text can hold a path.
    pub fn of_transfer(error: &TransferError) -> Self {
        match error {
            TransferError::Aborted => Self::PeerAborted,
            TransferError::PeerGone => Self::PeerGone,
            TransferError::Timeout => Self::IdleTimeout,
            TransferError::Chunk(_) => Self::ChunkProtocol,
            TransferError::Io(error) => match error.get_ref() {
                Some(inner) if inner.is::<WrongFileSize>() => Self::WrongFileSize,
                Some(inner) if inner.is::<FilesInvalid>() => Self::FilesInvalid,
                _ => Self::Io(error.kind()),
            },
        }
    }

    /// The cause of a copy of files that cannot be offered. The error's text
    /// is not used: it holds a path.
    pub fn of_list(error: &ListError) -> Self {
        match error {
            ListError::Files(FilesError::TooMany(_)) => Self::TooManyFiles,
            ListError::Files(FilesError::TooLarge(_)) => Self::FilesTooLarge,
            ListError::Files(_) => Self::FilesInvalid,
            ListError::Io(_, error) => Self::Io(error.kind()),
            ListError::NoName(_) => Self::NoFileName,
        }
    }

    /// The cause of a body that could not be read or converted. The error's
    /// text is not used.
    pub fn of_convert(error: &ConvertError) -> Self {
        match error {
            ConvertError::Image(_) => Self::ImageConversion,
            ConvertError::TooLarge(..) => Self::OverLimit,
            ConvertError::Missing(_) => Self::FormatMissing,
            ConvertError::Unknown(_) => Self::NotOffered,
        }
    }

    /// Who ended the transfer, for the causes that say.
    fn initiator(self) -> &'static str {
        match self {
            Self::PeerAborted | Self::ChunkProtocol => "peer",
            Self::PeerGone => "link",
            Self::IdleTimeout => "timeout",
            _ => "local",
        }
    }
}

impl Display for Cause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::NoOffer => "no_offer",
            Self::StaleSerial => "stale_serial",
            Self::NotOffered => "not_offered",
            Self::UnknownFile => "unknown_file",
            Self::RevokedWhileReading => "revoked_while_reading",
            Self::ReadTimeout => "read_timeout",
            Self::ReadUnanswered => "read_unanswered",
            Self::OverLimit => "over_limit",
            Self::ClipboardChanged => "clipboard_changed",
            Self::ClipboardSequenceMoved => "clipboard_sequence_moved",
            Self::ClipboardBusy => "clipboard_busy",
            Self::ThreadBusy => "thread_busy",
            Self::NotShown => "not_shown",
            Self::NotReady => "not_ready",
            Self::FormatMissing => "format_missing",
            Self::ImageConversion => "image_conversion",
            Self::OfferReplaced => "offer_replaced",
            Self::NoFiles => "no_files",
            Self::FilesInvalid => "files_invalid",
            Self::WrongFileSize => "wrong_file_size",
            Self::PeerAborted => "peer_aborted",
            Self::PeerGone => "peer_gone",
            Self::IdleTimeout => "idle_timeout",
            Self::ChunkProtocol => "chunk_protocol",
            Self::Cancelled => "cancelled",
            Self::Dropped => "dropped",
            Self::Private => "private",
            Self::NothingForwardable => "nothing_forwardable",
            Self::NoFileName => "no_file_name",
            Self::TooManyFiles => "too_many_files",
            Self::FilesTooLarge => "files_too_large",
            // `ErrorKind`'s `Debug` is the variant name and nothing else.
            Self::Io(kind) => return write!(f, "io_{kind:?}"),
        };
        f.write_str(name)
    }
}

/// The error of a paste of files whose file arrived with the wrong number of
/// bytes. It carries no name, so the text of the error is safe to keep.
#[derive(Debug)]
pub struct WrongFileSize;

impl Display for WrongFileSize {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("a file arrived with the wrong size")
    }
}

impl std::error::Error for WrongFileSize {}

/// The error of a paste of files whose list failed validation. It carries
/// no path, unlike the validator's own error.
#[derive(Debug)]
pub struct FilesInvalid;

impl Display for FilesInvalid {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("the offered files are not valid")
    }
}

impl std::error::Error for FilesInvalid {}

/// How a served transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServeEnd {
    /// The last chunk was handed to the link.
    Done,
    /// The transfer stopped before the last chunk: the peer or this client
    /// aborted it (the abort lines say which), or the session ended.
    Cut,
    /// Reading the source failed.
    Failed(ErrorKind),
}

/// Where the clipboard stayed busy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusyAt {
    /// Reading a copy to offer it: the copy is not forwarded.
    LocalCopy,
    /// Taking back promises: the old remote copy stays.
    ClearRemote,
    /// Putting a remote copy on the clipboard: it is dropped.
    PutRemote,
}

/// What became of a remote offer on the Windows clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Its formats are promised on the clipboard.
    Promised,
    /// Nothing in it is usable, so earlier promises were taken back.
    Cleared,
}

/// The end of a transfer as the Windows thread is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Final {
    Done,
    Cancelled,
    Failed,
}

/// Fixed one-off notices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    WindowsThreadPanicked,
    EngineThreadPanicked,
    EngineRuntimeFailed,
    FilesNotRenderedAtExit,
    /// A paste of text, a picture or files was asked for and failed in the
    /// Windows thread after the engine had the body.
    PasteNotConverted,
}

/// A clipboard event. Every field is a number, a duration, a [`MimeName`] or
/// a fixed enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The Windows clipboard changed. `own` tells whether it was this
    /// client's own write (then nothing is offered).
    LocalChange { sequence: u32, own: Own },
    /// A change of the clipboard that is not offered, and why.
    LocalSkipped { sequence: u32, cause: Cause },
    /// The files of a change are not offered, and why. Its other
    /// flavours still are.
    FilesSkipped { sequence: u32, cause: Cause },
    /// The clipboard stayed busy.
    Busy { at: BusyAt },
    /// This client offered its clipboard (an empty offer withdraws).
    OfferSent {
        serial: u32,
        generation: u64,
        mimes: MimeList,
        files: Files,
    },
    /// The remote side offered its clipboard.
    OfferReceived {
        serial: u32,
        mimes: MimeList,
        files: Files,
        files_valid: bool,
    },
    /// What the Windows clipboard got of a remote offer.
    OfferApplied { serial: u32, applied: Applied },
    /// The local offer was withdrawn. `serving` transfers were running.
    Revoked {
        why: RevokeWhy,
        serial: Option<u32>,
        serving: u32,
    },
    /// The remote side asked for an item of the local offer.
    RequestReceived { id: u32, serial: u32, item: Item },
    /// This client refused a request (it sends an Abort).
    RequestRefused { id: u32, cause: Cause },
    /// This client sent a request for an item of the remote offer.
    RequestSent { id: u32, serial: u32, item: Item },
    /// The remote side aborted a transfer. `serve` is true for a transfer
    /// this client is serving, false for one it fetches (by id parity).
    AbortReceived { id: u32, serve: bool },
    /// This client sent an Abort for a transfer.
    AbortSent { id: u32 },
    /// A body is ready and is being sent. `read` is how long the Windows
    /// clipboard read took, when there was one.
    ServeStarted {
        id: u32,
        bytes: Option<u64>,
        read: Option<Duration>,
    },
    /// A served transfer ended.
    ServeEnded {
        id: u32,
        bytes: u64,
        elapsed: Duration,
        end: ServeEnd,
    },
    /// A paste of the remote offer started.
    PasteStarted {
        paste: u32,
        serial: u32,
        item: PasteItem,
    },
    /// A paste ended. `cause` is none when it succeeded.
    PasteEnded {
        paste: u32,
        bytes: u64,
        elapsed: Duration,
        cause: Option<Cause>,
    },
    /// The user cancelled a transfer.
    PasteCancelled { paste: u32 },
    /// The end of a transfer as the Windows thread is told it.
    TransferFinal { transfer: u32, state: Final },
    /// A fixed notice.
    Notice(Notice),
    /// A spool entry could not be removed, with the kind of error.
    SpoolNotRemoved { kind: ErrorKind },
}

/// What a paste fetches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteItem {
    Mime(MimeName),
    Files(Files),
}

impl Display for PasteItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mime(mime) => write!(f, "mime={mime}"),
            Self::Files(files) => write!(f, "{files}"),
        }
    }
}

fn ms(duration: Duration) -> u128 {
    duration.as_millis()
}

impl Display for Event {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalChange { sequence, own } => {
                write!(f, "local change sequence={sequence} own={}", own.name())
            }
            Self::LocalSkipped { sequence, cause } => {
                write!(
                    f,
                    "local change not offered sequence={sequence} cause={cause}"
                )
            }
            Self::FilesSkipped { sequence, cause } => {
                write!(
                    f,
                    "local files not offered sequence={sequence} cause={cause}"
                )
            }
            Self::Busy { at } => {
                let what = match at {
                    BusyAt::LocalCopy => "copy_not_forwarded",
                    BusyAt::ClearRemote => "old_remote_copy_stays",
                    BusyAt::PutRemote => "remote_copy_dropped",
                };
                write!(f, "clipboard busy result={what}")
            }
            Self::OfferSent {
                serial,
                generation,
                mimes,
                files,
            } => write!(
                f,
                "offer sent serial={serial} generation={generation} mimes={mimes} {files}"
            ),
            Self::OfferReceived {
                serial,
                mimes,
                files,
                files_valid,
            } => write!(
                f,
                "offer received serial={serial} mimes={mimes} {files} files_valid={files_valid}"
            ),
            Self::OfferApplied { serial, applied } => {
                let what = match applied {
                    Applied::Promised => "promised",
                    Applied::Cleared => "cleared",
                };
                write!(f, "offer applied serial={serial} result={what}")
            }
            Self::Revoked {
                why,
                serial,
                serving,
            } => {
                write!(f, "offer revoked why={}", why.name())?;
                match serial {
                    Some(serial) => write!(f, " serial={serial}")?,
                    None => f.write_str(" serial=none")?,
                }
                write!(f, " serving={serving}")
            }
            Self::RequestReceived { id, serial, item } => {
                write!(f, "request received id={id} serial={serial} {item}")
            }
            Self::RequestRefused { id, cause } => {
                write!(f, "request refused id={id} cause={cause} initiator=local")
            }
            Self::RequestSent { id, serial, item } => {
                write!(f, "request sent id={id} serial={serial} {item}")
            }
            Self::AbortReceived { id, serve } => write!(
                f,
                "abort received id={id} transfer={} initiator=peer",
                if *serve { "serving" } else { "fetching" }
            ),
            Self::AbortSent { id } => write!(f, "abort sent id={id}"),
            Self::ServeStarted { id, bytes, read } => {
                write!(f, "serve started id={id}")?;
                if let Some(bytes) = bytes {
                    write!(f, " bytes={bytes}")?;
                }
                if let Some(read) = read {
                    write!(f, " read_ms={}", ms(*read))?;
                }
                Ok(())
            }
            Self::ServeEnded {
                id,
                bytes,
                elapsed,
                end,
            } => {
                write!(f, "serve ended id={id} result=")?;
                match end {
                    ServeEnd::Done => f.write_str("done")?,
                    ServeEnd::Cut => f.write_str("cut")?,
                    // `ErrorKind`'s `Debug` is the variant name and nothing else.
                    ServeEnd::Failed(kind) => write!(f, "failed cause=io_{kind:?}")?,
                }
                write!(f, " bytes={bytes} ms={}", ms(*elapsed))
            }
            Self::PasteStarted {
                paste,
                serial,
                item,
            } => write!(f, "paste started paste={paste} serial={serial} {item}"),
            Self::PasteEnded {
                paste,
                bytes,
                elapsed,
                cause,
            } => {
                write!(f, "paste ended paste={paste} ")?;
                match cause {
                    None => write!(f, "result=done")?,
                    Some(cause) => write!(
                        f,
                        "result=failed cause={cause} initiator={}",
                        cause.initiator()
                    )?,
                }
                write!(f, " bytes={bytes} ms={}", ms(*elapsed))
            }
            Self::PasteCancelled { paste } => {
                write!(f, "paste cancelled paste={paste} initiator=local")
            }
            Self::TransferFinal { transfer, state } => {
                let state = match state {
                    Final::Done => "done",
                    Final::Cancelled => "cancelled",
                    Final::Failed => "failed",
                };
                write!(f, "transfer final transfer={transfer} state={state}")
            }
            Self::Notice(notice) => f.write_str(match notice {
                Notice::WindowsThreadPanicked => "the Windows thread panicked",
                Notice::EngineThreadPanicked => "the engine thread panicked",
                Notice::EngineRuntimeFailed => "the engine could not start its runtime",
                Notice::FilesNotRenderedAtExit => "promised files are not rendered at exit",
                Notice::PasteNotConverted => "a pasted body could not be converted",
            }),
            Self::SpoolNotRemoved { kind } => {
                write!(f, "spool entry not removed io_{kind:?}")
            }
        }
    }
}

thread_local! {
    /// The session an engine thread works for, put on its lines.
    static SESSION: Cell<Option<u32>> = const { Cell::new(None) };
}

/// Marks this thread's lines with a session number (a tab's connection
/// attempt), for the engine thread of a session. The Windows thread and the
/// hub serve all sessions and have none.
pub fn tag_thread(session: u32) {
    SESSION.set(Some(session));
}

/// Writes the line for `event`: `clipboard: [session=N] <event>`.
pub fn emit(event: Event) {
    // clip-log: line
    let session = SESSION.get().map(|session| format!(" session={session}"));
    // clip-log: line
    let line = format!("clipboard:{} {event}", session.unwrap_or_default());
    #[cfg(test)]
    testing::record(&line);
    crate::logging::line(&line);
}

/// The request or abort a message sent to the remote side stands for. Other
/// messages (offers are logged where they are made; chunks and
/// acknowledgements are not logged) log nothing.
pub fn outbound(msg: &ClipboardMsg) {
    match msg {
        ClipboardMsg::Request { id, serial, item } => emit(Event::RequestSent {
            id: *id,
            serial: *serial,
            item: Item::of(item),
        }),
        ClipboardMsg::Abort { id } => emit(Event::AbortSent { id: *id }),
        ClipboardMsg::Offer { .. } | ClipboardMsg::Data { .. } | ClipboardMsg::Ack { .. } => {}
    }
}

/// Capturing the lines a test emits, on the test's own thread.
#[cfg(test)]
pub mod testing {
    use std::cell::RefCell;

    thread_local! {
        static CAPTURE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    }

    pub(super) fn record(line: &str) {
        CAPTURE.with(|capture| {
            if let Some(lines) = capture.borrow_mut().as_mut() {
                lines.push(line.to_owned());
            }
        });
    }

    /// Fails unless `line` has only the forms a line may have: a fixed word
    /// or a `key=value` whose value is a number, a fixed word, an `io_<kind>`
    /// or a list of names from the closed mime table. A leak of any kind
    /// (a name, a path, a piece of text, a hash) has none of these forms.
    pub fn assert_well_formed(line: &str) {
        let rest = line
            .strip_prefix("clipboard:")
            .unwrap_or_else(|| panic!("not a clipboard line: {line:?}"));
        let word = |text: &str| {
            !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        };
        for token in rest.split(' ').filter(|token| !token.is_empty()) {
            let ok = match token.split_once('=') {
                None => word(token),
                Some((key, value)) => {
                    word(key)
                        && (value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty()
                            || word(value)
                            || value.strip_prefix("io_").is_some_and(|kind| {
                                !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_alphabetic())
                            })
                            || value
                                .split(',')
                                .all(|name| name == "other" || super::KNOWN_MIMES.contains(&name)))
                }
            };
            assert!(ok, "{token:?} is not a form a log line may have: {line:?}");
        }
    }

    /// Runs `body` and returns what it returned and the lines emitted on
    /// this thread meanwhile.
    pub fn capture<R>(body: impl FnOnce() -> R) -> (R, Vec<String>) {
        CAPTURE.with(|capture| *capture.borrow_mut() = Some(Vec::new()));
        let result = body();
        let lines = CAPTURE
            .with(|capture| capture.borrow_mut().take())
            .unwrap_or_default();
        (result, lines)
    }
}

#[cfg(test)]
mod tests;
