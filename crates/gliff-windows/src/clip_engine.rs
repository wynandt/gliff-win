//! The clipboard bridge: joins upstream's transfer engine to the Windows
//! clipboard thread. It mirrors upstream's client bridge (v0.4.1), with the
//! GTK thread replaced by [`Command`]s in and [`HostEvent`]s out.
//!
//! Remote to local: the remote offer becomes a [`HostEvent::RemoteOffer`],
//! which the Windows thread puts on the clipboard as promised formats. When an
//! application pastes, the Windows thread sends [`Command::FetchMime`] (the
//! body comes back whole, at most 32 MiB) or [`Command::FetchFiles`] (every
//! file is spooled once per offer, then the paths come back).
//!
//! Local to remote: the Windows thread reports a changed clipboard as
//! [`Command::LocalOffer`]. When the remote side requests a flavour, the
//! Windows thread is asked to read it ([`HostEvent::ReadLocal`]) and the body
//! is served in chunks; a file is served from disk here. Every local change
//! first revokes the current offer ([`Command::RevokeLocal`]), so nothing
//! copied earlier stays readable while the next offer is built.
//!
//! Everything here runs on one thread, inside a tokio `LocalSet`.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use bytes::Bytes;
use gliff_proto::ClipboardFile;
use gliff_proto::ClipboardItem;
use gliff_proto::ClipboardMsg;
use gliff_proto::clipboard::CHUNK;
use gliff_proto::clipboard::MAX_ITEM;
use gliff_proto::clipboard::resolve_mime;
use gliff_transport::clipboard::ChunkSink;
use gliff_transport::clipboard::ChunkSource;
use gliff_transport::clipboard::Event;
use gliff_transport::clipboard::IDLE_TIMEOUT;
use gliff_transport::clipboard::ReadSource;
use gliff_transport::clipboard::TransferError;
use gliff_transport::clipboard::Transfers;
use gliff_transport::clipboard::WriteSink;
use gliff_transport::clipboard::files::LocalFiles;
use gliff_transport::clipboard::files::SPOOL_GRACE;
use gliff_transport::clipboard::progress::Jobs;
use gliff_transport::clipboard::progress::Meter;
use gliff_transport::clipboard::progress::Progress;
use gliff_transport::clipboard::progress::describe_files;
use gliff_transport::clipboard::progress::describe_mime;
use tokio::sync::OnceCell;
use tokio::sync::oneshot;

use crate::clip_core;
use crate::clip_files;
use crate::clip_files::Spool;
use crate::clip_log;
use crate::clip_log::Cause;
use crate::clip_log::Event as Log;
use crate::clip_log::Files;
use crate::clip_log::Item;
use crate::clip_log::MimeList;
use crate::clip_log::MimeName;
use crate::clip_log::PasteItem;
use crate::clip_log::RevokeWhy;
use crate::clip_log::ServeEnd;

/// What the Windows thread sends the engine.
pub enum Command {
    /// The local clipboard changed: refuse requests against the current offer
    /// and stop serving it, for the reason given (it is logged). A
    /// `LocalOffer` may follow.
    RevokeLocal(RevokeWhy),
    /// The local clipboard changed to these forwardable flavours and files.
    /// An empty offer withdraws the previous one: it takes a fresh serial, so
    /// requests against the old one are refused.
    LocalOffer {
        mime_types: Vec<String>,
        files: LocalFiles,
        /// The Windows thread's offer generation, handed back with each read
        /// so it can refuse a read for an offer it has since moved past.
        generation: u64,
    },
    /// An application pastes `mime_type` from the remote offer with `serial`.
    FetchMime {
        mime_type: String,
        serial: u32,
        result: oneshot::Sender<Result<Vec<u8>, Cause>>,
    },
    /// An application pastes the files of the remote offer with `serial`;
    /// the answer is the spool paths of the top-level entries.
    FetchFiles {
        serial: u32,
        result: oneshot::Sender<Result<Vec<PathBuf>, Cause>>,
    },
    /// The user cancelled the transfer reported as `id`.
    CancelTransfer(u32),
}

/// What the engine tells the Windows thread.
pub enum HostEvent {
    /// The remote clipboard changed. Empty lists mean cleared or nothing
    /// forwardable. `files` is empty when the list failed validation.
    RemoteOffer {
        serial: u32,
        mime_types: Vec<String>,
        files: Vec<ClipboardFile>,
    },
    /// The remote side wants `mime_type` of the local offer: read it from the
    /// clipboard and answer with the converted body.
    ReadLocal {
        mime_type: String,
        generation: u64,
        reply: oneshot::Sender<Result<Vec<u8>, Cause>>,
    },
    /// A transfer is slow enough to show, is done, or failed.
    Transfer { id: u32, progress: Progress },
}

#[derive(Default)]
struct RemoteOffer {
    serial: u32,
    files: Vec<ClipboardFile>,
    spooled: Rc<OnceCell<Vec<PathBuf>>>,
    spool: Option<Rc<Spool>>,
}

/// A running paste: its number, and the flag that records its cancellation.
type Tracked = (u32, Rc<Cell<bool>>);

pub struct Engine {
    transfers: Transfers,
    jobs: Jobs,
    host: Rc<dyn Fn(HostEvent)>,
    spool_base: PathBuf,
    /// None while no local selection is offered: before the first one and
    /// after a revocation. Serial zero is a valid wire value, so it must not
    /// stand in for an absent offer. Shared with file opens in flight, which
    /// check it again before serving.
    local_serial: Rc<Cell<Option<u32>>>,
    /// The last serial offered, kept across revocations so a new offer does
    /// not restart at 1 and match a serial the peer still holds.
    last_serial: Cell<u32>,
    local_mimes: RefCell<Vec<String>>,
    local_files: RefCell<LocalFiles>,
    local_generation: Cell<u64>,
    remote: RefCell<RemoteOffer>,
    /// The number of the last paste, for the log.
    pastes: Cell<u32>,
    /// How many transfers are being served, for the log.
    serving: Rc<Cell<u32>>,
    /// The pastes that can still be cancelled, by job id, for the log.
    cancels: RefCell<HashMap<u32, Tracked>>,
}

impl Engine {
    pub fn new(transfers: Transfers, spool_base: PathBuf, host: Rc<dyn Fn(HostEvent)>) -> Self {
        let report_to = host.clone();
        Self {
            transfers,
            jobs: Jobs::new(move |id, progress| report_to(HostEvent::Transfer { id, progress })),
            host,
            spool_base,
            local_serial: Rc::new(Cell::new(None)),
            last_serial: Cell::new(0),
            local_mimes: RefCell::default(),
            local_files: RefCell::default(),
            local_generation: Cell::new(0),
            remote: RefCell::default(),
            pastes: Cell::new(0),
            serving: Rc::new(Cell::new(0)),
            cancels: RefCell::default(),
        }
    }

    /// A clipboard message from the remote side, with its payload.
    pub fn on_peer_msg(&self, msg: ClipboardMsg, payload: Bytes) {
        if let ClipboardMsg::Abort { id } = &msg {
            // The ids this client numbers are odd, the remote side's even.
            clip_log::emit(Log::AbortReceived {
                id: *id,
                serve: id % 2 == 0,
            });
        }
        match self.transfers.on_msg(msg, payload) {
            Some(Event::Offer {
                serial,
                mime_types,
                files,
            }) => self.on_remote_offer(serial, mime_types, files),
            Some(Event::Request { id, serial, item }) => self.on_request(id, serial, item),
            None => {}
        }
    }

    fn on_remote_offer(&self, serial: u32, mime_types: Vec<String>, files: Vec<ClipboardFile>) {
        let files_valid = clip_files::validate_offer(&files).is_ok();
        clip_log::emit(Log::OfferReceived {
            serial,
            mimes: MimeList::of(&mime_types),
            files: Files::of(&files),
            files_valid,
        });
        let files = if files_valid { files } else { Vec::new() };
        // An offer takes over the clipboard, and so replaces the local offer,
        // only when Windows would write something of it. An offer with
        // nothing usable (the server's empty one at connect) leaves the
        // clipboard as it is, as upstream's client does. Revoking then would
        // refuse the reads the server's history watcher makes of the offer it
        // was just sent, and each refusal is a toast on the server.
        if clip_core::remote_write(&mime_types, &files, false).is_some() {
            self.revoke_local(RevokeWhy::RemoteOffer);
        }
        let previous = std::mem::replace(
            &mut *self.remote.borrow_mut(),
            RemoteOffer {
                serial,
                files: files.clone(),
                ..RemoteOffer::default()
            },
        );
        if let Some(spool) = previous.spool {
            retire(spool);
        }
        (self.host)(HostEvent::RemoteOffer {
            serial,
            mime_types,
            files,
        });
    }

    fn on_request(&self, id: u32, serial: u32, item: ClipboardItem) {
        clip_log::emit(Log::RequestReceived {
            id,
            serial,
            item: Item::of(&item),
        });
        match self.local_serial.get() {
            None => return refuse(&self.transfers, id, Cause::NoOffer),
            Some(current) if current != serial => {
                return refuse(&self.transfers, id, Cause::StaleSerial);
            }
            Some(_) => {}
        }
        match item {
            ClipboardItem::Mime(mime_type) => {
                // The Windows thread reads the live clipboard, which may hold
                // flavours deliberately left out of the offer.
                if resolve_mime(&mime_type, &self.local_mimes.borrow()).is_none() {
                    return refuse(&self.transfers, id, Cause::NotOffered);
                }
                let (reply, answer) = oneshot::channel();
                let asked = Instant::now();
                (self.host)(HostEvent::ReadLocal {
                    mime_type,
                    generation: self.local_generation.get(),
                    reply,
                });
                let transfers = self.transfers.clone();
                let current = self.local_serial.clone();
                let serving = self.serving.clone();
                tokio::task::spawn_local(async move {
                    let body = match tokio::time::timeout(IDLE_TIMEOUT, answer).await {
                        Ok(Ok(Ok(body))) => body,
                        Ok(Ok(Err(cause))) => return refuse(&transfers, id, cause),
                        Ok(Err(_)) => return refuse(&transfers, id, Cause::ReadUnanswered),
                        Err(_) => return refuse(&transfers, id, Cause::ReadTimeout),
                    };
                    // A revocation while the clipboard was read has nothing
                    // to abort yet, so it is caught here instead.
                    if current.get() != Some(serial) {
                        return refuse(&transfers, id, Cause::RevokedWhileReading);
                    }
                    let bytes = u64::try_from(body.len()).expect("a length fits u64");
                    if bytes > MAX_ITEM {
                        return refuse(&transfers, id, Cause::OverLimit);
                    }
                    clip_log::emit(Log::ServeStarted {
                        id,
                        bytes: Some(bytes),
                        read: Some(asked.elapsed()),
                    });
                    transfers.serve(
                        id,
                        Logged::new(id, BytesSource(Bytes::from(body)), &serving),
                    );
                });
            }
            ClipboardItem::File(_) => {
                let Some(path) = self.local_files.borrow().path_for(&item).map(PathBuf::from)
                else {
                    return refuse(&self.transfers, id, Cause::UnknownFile);
                };
                let transfers = self.transfers.clone();
                let current = self.local_serial.clone();
                let serving = self.serving.clone();
                tokio::task::spawn_local(async move {
                    let opened = tokio::fs::File::open(clip_files::extended(&path)).await;
                    if current.get() != Some(serial) {
                        return refuse(&transfers, id, Cause::RevokedWhileReading);
                    }
                    match opened {
                        Ok(file) => {
                            clip_log::emit(Log::ServeStarted {
                                id,
                                bytes: None,
                                read: None,
                            });
                            transfers.serve(id, Logged::new(id, ReadSource(file), &serving));
                        }
                        Err(error) => refuse(&transfers, id, Cause::Io(error.kind())),
                    }
                });
            }
        }
    }

    pub fn on_command(&self, command: Command) {
        match command {
            Command::RevokeLocal(why) => self.revoke_local(why),
            Command::LocalOffer {
                mime_types,
                files,
                generation,
            } => self.local_offer(mime_types, files, generation),
            Command::FetchMime {
                mime_type,
                serial,
                result,
            } => self.fetch_mime(mime_type, serial, result),
            Command::FetchFiles { serial, result } => self.fetch_files(serial, result),
            Command::CancelTransfer(id) => {
                let paste = self.cancels.borrow_mut().remove(&id);
                if let Some((paste, cancelled)) = paste {
                    // A paste that already ended has dropped its handle.
                    if Rc::strong_count(&cancelled) > 1 {
                        cancelled.set(true);
                        clip_log::emit(Log::PasteCancelled { paste });
                    }
                }
                self.jobs.cancel(id);
            }
        }
    }

    fn revoke_local(&self, why: RevokeWhy) {
        // A revocation with nothing offered and nothing served is not news.
        let serial = self.local_serial.get();
        let serving = self.serving.get();
        if serial.is_some() || serving > 0 {
            clip_log::emit(Log::Revoked {
                why,
                serial,
                serving,
            });
        }
        self.local_serial.set(None);
        self.local_mimes.borrow_mut().clear();
        *self.local_files.borrow_mut() = LocalFiles::default();
        self.transfers.abort_serving();
    }

    fn local_offer(&self, mime_types: Vec<String>, files: LocalFiles, generation: u64) {
        let serial = self.last_serial.get().wrapping_add(1);
        self.last_serial.set(serial);
        self.local_serial.set(Some(serial));
        self.local_generation.set(generation);
        *self.local_mimes.borrow_mut() = mime_types.clone();
        let entries = files.entries.clone();
        clip_log::emit(Log::OfferSent {
            serial,
            generation,
            mimes: MimeList::of(&mime_types),
            files: Files::of(&entries),
        });
        *self.local_files.borrow_mut() = files;
        let transfers = self.transfers.clone();
        tokio::task::spawn_local(async move {
            let _ = transfers
                .send(ClipboardMsg::Offer {
                    serial,
                    mime_types,
                    files: entries,
                })
                .await;
        });
    }

    fn fetch_mime(
        &self,
        mime_type: String,
        serial: u32,
        result: oneshot::Sender<Result<Vec<u8>, Cause>>,
    ) {
        let paste = PasteLog::start(
            self.next_paste(),
            serial,
            PasteItem::Mime(MimeName::of(&mime_type)),
        );
        if self.remote.borrow().serial != serial {
            paste.fail(Cause::OfferReplaced);
            let _ = result.send(Err(Cause::OfferReplaced));
            return;
        }
        let transfers = self.transfers.clone();
        let tracked = paste.tracked();
        let job = self
            .jobs
            .run(describe_mime(&mime_type), None, |meter| async move {
                let body = Rc::new(RefCell::new(Vec::new()));
                let sink = meter.wrap(SharedSink(body.clone()));
                let fetched = transfers
                    .fetch(ClipboardItem::Mime(mime_type), serial, sink, Some(MAX_ITEM))
                    .await;
                paste.end(&meter, fetched.as_ref().err());
                let answer = match &fetched {
                    Ok(_) => Ok(std::mem::take(&mut *body.borrow_mut())),
                    Err(error) => Err(Cause::of_transfer(error)),
                };
                let _ = result.send(answer);
                fetched.map(|_| ())
            });
        self.track(job, tracked);
    }

    fn fetch_files(&self, serial: u32, result: oneshot::Sender<Result<Vec<PathBuf>, Cause>>) {
        let (files, spooled, spool, paste) = {
            let mut remote = self.remote.borrow_mut();
            let paste = PasteLog::start(
                self.next_paste(),
                serial,
                PasteItem::Files(Files::of(&remote.files)),
            );
            if remote.serial != serial {
                paste.fail(Cause::OfferReplaced);
                let _ = result.send(Err(Cause::OfferReplaced));
                return;
            }
            if remote.files.is_empty() {
                paste.fail(Cause::NoFiles);
                let _ = result.send(Err(Cause::NoFiles));
                return;
            }
            if remote.spool.is_none() {
                match Spool::create_in(&self.spool_base) {
                    Ok(spool) => remote.spool = Some(Rc::new(spool)),
                    Err(error) => {
                        let cause = Cause::Io(error.kind());
                        paste.fail(cause);
                        let _ = result.send(Err(cause));
                        return;
                    }
                }
            }
            (
                remote.files.clone(),
                remote.spooled.clone(),
                remote.spool.clone().expect("the spool was just created"),
                paste,
            )
        };
        let transfers = self.transfers.clone();
        let (label, total) = describe_files(&files);
        let tracked = paste.tracked();
        let job = self.jobs.run(label, total, |meter| async move {
            let paths = spooled
                .get_or_try_init(|| spool_files(&transfers, &files, serial, &spool, &meter))
                .await;
            let (answer, outcome) = match paths {
                Ok(paths) => (Ok(paths.clone()), Ok(())),
                Err(error) => (Err(Cause::of_transfer(&error)), Err(error)),
            };
            paste.end(&meter, outcome.as_ref().err());
            let _ = result.send(answer);
            outcome
        });
        self.track(job, tracked);
    }

    /// Remember a running paste so that cancelling its job can say so.
    fn track(&self, job: u32, tracked: Tracked) {
        let mut cancels = self.cancels.borrow_mut();
        // A paste that ended dropped its log, which held the other handle.
        cancels.retain(|_, (_, cancelled)| Rc::strong_count(cancelled) > 1);
        cancels.insert(job, tracked);
    }

    fn next_paste(&self) -> u32 {
        let paste = self.pastes.get().wrapping_add(1);
        self.pastes.set(paste);
        paste
    }
}

/// Keep a spool alive for [`SPOOL_GRACE`] after its offer is replaced, so a
/// program still reading a pasted file keeps it. The end of the session
/// drops the task, and with it the spool, earlier.
pub fn retire(spool: Rc<Spool>) {
    tokio::task::spawn_local(async move {
        tokio::time::sleep(SPOOL_GRACE).await;
        drop(spool);
    });
}

/// Stream every entry of the remote offer into `spool` under sanitised
/// Windows names, and return the paths of the top-level entries in offer
/// order. The list is validated first; any bad path fails the whole paste.
pub async fn spool_files(
    transfers: &Transfers,
    files: &[ClipboardFile],
    serial: u32,
    spool: &Spool,
    meter: &Meter,
) -> Result<Vec<PathBuf>, TransferError> {
    clip_files::validate_offer(files)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, clip_log::FilesInvalid))?;
    let relative = clip_files::spool_paths_within(files, clip_files::top_level_units(spool.dir()));
    let mut partial = Partial::new(spool.dir());
    for (index, (file, relative)) in files.iter().zip(&relative).enumerate() {
        let destination = spool.dir().join(relative);
        if file.dir {
            tokio::fs::create_dir_all(clip_files::extended(&destination)).await?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(clip_files::extended(parent)).await?;
        }
        let target = tokio::fs::File::create(clip_files::extended(&destination)).await?;
        let index = u32::try_from(index).expect("validate_offer caps the entry count");
        let got = transfers
            .fetch(
                ClipboardItem::File(index),
                serial,
                meter.wrap(WriteSink(target)),
                Some(file.size),
            )
            .await?;
        if got != file.size {
            return Err(io::Error::new(io::ErrorKind::InvalidData, clip_log::WrongFileSize).into());
        }
    }
    partial.keep();
    // The top level is what `top_level` selects: no `/` in the path.
    Ok(files
        .iter()
        .zip(&relative)
        .filter(|(file, _)| !file.path.contains('/'))
        .map(|(_, relative)| spool.dir().join(relative))
        .collect())
}

/// Empties the spool directory when dropped unless [`Partial::keep`] was
/// called: a paste that failed, was cancelled, or was dropped leaves no
/// half-written file behind, offered or not.
struct Partial<'a> {
    dir: &'a Path,
    keep: bool,
}

impl<'a> Partial<'a> {
    fn new(dir: &'a Path) -> Self {
        Self { dir, keep: false }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for Partial<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let Ok(entries) = std::fs::read_dir(clip_files::extended(self.dir)) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(error) = removed {
                clip_log::emit(Log::SpoolNotRemoved { kind: error.kind() });
            }
        }
    }
}

/// Refuse the request `id` (the peer is sent an Abort) and log why.
fn refuse(transfers: &Transfers, id: u32, cause: Cause) {
    clip_log::emit(Log::RequestRefused { id, cause });
    transfers.refuse(id);
}

/// A source that counts what it gives out and logs how the serving ended
/// when it is dropped: whether the transfer was done, cut off by the peer
/// or by a revocation, or failed. It never sees more than lengths and kinds.
struct Logged<S> {
    inner: S,
    id: u32,
    started: Instant,
    bytes: u64,
    end: ServeEnd,
    serving: Rc<Cell<u32>>,
}

impl<S> Logged<S> {
    fn new(id: u32, inner: S, serving: &Rc<Cell<u32>>) -> Self {
        serving.set(serving.get().saturating_add(1));
        Self {
            inner,
            id,
            started: Instant::now(),
            bytes: 0,
            end: ServeEnd::Cut,
            serving: serving.clone(),
        }
    }
}

impl<S: ChunkSource> ChunkSource for Logged<S> {
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        match self.inner.next().await {
            Ok(Some(chunk)) => {
                self.bytes += chunk.len() as u64;
                Ok(Some(chunk))
            }
            Ok(None) => {
                self.end = ServeEnd::Done;
                Ok(None)
            }
            Err(error) => {
                self.end = ServeEnd::Failed(error.kind());
                Err(error)
            }
        }
    }
}

impl<S> Drop for Logged<S> {
    fn drop(&mut self) {
        self.serving.set(self.serving.get().saturating_sub(1));
        clip_log::emit(Log::ServeEnded {
            id: self.id,
            bytes: self.bytes,
            elapsed: self.started.elapsed(),
            end: self.end,
        });
    }
}

/// The log of one paste: a start line now, and an end line when it is told
/// how it ended, or when it is dropped without being told (the user
/// cancelled it, or the session ended).
struct PasteLog {
    paste: u32,
    started: Instant,
    ended: bool,
    /// Set when the user cancels the paste, so its drop says so.
    cancelled: Rc<Cell<bool>>,
}

impl PasteLog {
    fn start(paste: u32, serial: u32, item: PasteItem) -> Self {
        let started = Instant::now();
        clip_log::emit(Log::PasteStarted {
            paste,
            serial,
            item,
        });
        Self {
            paste,
            started,
            ended: false,
            cancelled: Rc::new(Cell::new(false)),
        }
    }

    /// The paste number and the flag that records its cancellation.
    fn tracked(&self) -> Tracked {
        (self.paste, self.cancelled.clone())
    }

    /// The paste ended before anything was fetched.
    fn fail(mut self, cause: Cause) {
        self.finish(0, Some(cause));
    }

    /// The paste ended after `meter` counted what arrived.
    fn end(mut self, meter: &Meter, error: Option<&TransferError>) {
        self.finish(meter.get(), error.map(Cause::of_transfer));
    }

    fn finish(&mut self, bytes: u64, cause: Option<Cause>) {
        self.ended = true;
        clip_log::emit(Log::PasteEnded {
            paste: self.paste,
            bytes,
            elapsed: self.started.elapsed(),
            cause,
        });
    }
}

impl Drop for PasteLog {
    fn drop(&mut self) {
        if !self.ended {
            let cause = if self.cancelled.get() {
                Cause::Cancelled
            } else {
                Cause::Dropped
            };
            self.finish(0, Some(cause));
        }
    }
}

/// A body held in memory, served in chunks.
struct BytesSource(Bytes);

impl ChunkSource for BytesSource {
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        if self.0.is_empty() {
            return Ok(None);
        }
        let take = self.0.len().min(CHUNK);
        Ok(Some(self.0.split_to(take)))
    }
}

/// A sink that collects a body in memory, which the caller keeps a handle to.
struct SharedSink(Rc<RefCell<Vec<u8>>>);

impl ChunkSink for SharedSink {
    async fn write(&mut self, chunk: Bytes) -> io::Result<()> {
        self.0.borrow_mut().extend_from_slice(&chunk);
        Ok(())
    }

    async fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
