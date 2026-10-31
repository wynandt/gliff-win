//! The engine against upstream's transfer engine playing the remote side
//! (`Side::Server`), wired through in-memory channels.

use std::cell::Cell;
use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use bytes::Bytes;
use gliff_proto::ClipboardFile;
use gliff_proto::ClipboardItem;
use gliff_proto::ClipboardMsg;
use gliff_proto::clipboard::MAX_ITEM;
use gliff_transport::clipboard::ChunkSink;
use gliff_transport::clipboard::ChunkSource;
use gliff_transport::clipboard::Event;
use gliff_transport::clipboard::Side;
use gliff_transport::clipboard::TransferError;
use gliff_transport::clipboard::Transfers;
use gliff_transport::clipboard::files::LocalFiles;
use gliff_transport::clipboard::files::SPOOL_GRACE;
use gliff_transport::clipboard::outbound_channel;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::LocalSet;

use super::BytesSource;
use super::Command;
use super::Engine;
use super::HostEvent;
use super::SharedSink;
use crate::clip_files;
use crate::clip_log::Cause;
use crate::clip_log::RevokeWhy;

mod logged;

const TEXT: &str = "text/plain;charset=utf-8";

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gwin-clip-engine-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(test: impl std::future::Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the test runtime");
    LocalSet::new().block_on(&runtime, test);
}

/// Both ends of a connection: the engine under test and a stand-in for the
/// remote side, with what each reports.
struct Rig {
    engine: Rc<Engine>,
    server: Transfers,
    /// Offers and requests that reached the remote side.
    events: mpsc::UnboundedReceiver<Event>,
    /// What the engine told the Windows thread.
    host: mpsc::UnboundedReceiver<HostEvent>,
    dir: TempDir,
}

fn rig(label: &str) -> Rig {
    let dir = TempDir::new(label);
    let (client_out, mut client_rx) = outbound_channel();
    let (server_out, mut server_rx) = outbound_channel();
    let server = Transfers::new(server_out, Side::Server);
    let (host_tx, host) = mpsc::unbounded_channel();
    let engine = Rc::new(Engine::new(
        Transfers::new(client_out, Side::Client),
        dir.path().join("spool"),
        Rc::new(move |event| {
            let _ = host_tx.send(event);
        }),
    ));
    let (event_tx, events) = mpsc::unbounded_channel();
    let remote = server.clone();
    tokio::task::spawn_local(async move {
        while let Some((msg, payload)) = client_rx.recv().await {
            crate::clip_log::outbound(&msg);
            if let Some(event) = remote.on_msg(msg, payload) {
                let _ = event_tx.send(event);
            }
        }
    });
    let local = engine.clone();
    tokio::task::spawn_local(async move {
        while let Some((msg, payload)) = server_rx.recv().await {
            local.on_peer_msg(msg, payload);
        }
    });
    Rig {
        engine,
        server,
        events,
        host,
        dir,
    }
}

impl Rig {
    /// Offer these flavours from the engine and return the serial the remote
    /// side saw.
    async fn offer(&mut self, mimes: &[&str]) -> u32 {
        self.offer_files(mimes, LocalFiles::default()).await
    }

    async fn offer_files(&mut self, mimes: &[&str], files: LocalFiles) -> u32 {
        self.engine.on_command(Command::LocalOffer {
            mime_types: mimes.iter().map(|m| (*m).to_owned()).collect(),
            files,
            generation: 7,
        });
        match self.events.recv().await.expect("the offer arrives") {
            Event::Offer { serial, .. } => serial,
            Event::Request { .. } => panic!("expected an offer, got a request"),
        }
    }

    /// The next read the engine asked of the Windows thread.
    async fn next_read(&mut self) -> (String, u64, oneshot::Sender<Result<Vec<u8>, Cause>>) {
        loop {
            match self.host.recv().await.expect("the engine is alive") {
                HostEvent::ReadLocal {
                    mime_type,
                    generation,
                    reply,
                } => return (mime_type, generation, reply),
                HostEvent::Transfer { .. } | HostEvent::RemoteOffer { .. } => {}
            }
        }
    }

    async fn next_remote_offer(&mut self) -> (u32, Vec<String>, Vec<ClipboardFile>) {
        loop {
            match self.host.recv().await.expect("the engine is alive") {
                HostEvent::RemoteOffer {
                    serial,
                    mime_types,
                    files,
                } => return (serial, mime_types, files),
                HostEvent::Transfer { .. } | HostEvent::ReadLocal { .. } => {}
            }
        }
    }

    /// Hand the remote side's events to a responder task.
    fn take_events(&mut self) -> mpsc::UnboundedReceiver<Event> {
        let (_, empty) = mpsc::unbounded_channel();
        std::mem::replace(&mut self.events, empty)
    }

    fn no_read_asked(&mut self) {
        while let Ok(event) = self.host.try_recv() {
            assert!(
                !matches!(event, HostEvent::ReadLocal { .. }),
                "the clipboard was read for a refused request"
            );
        }
    }

    /// The remote side fetches an item of the engine's offer, the Windows
    /// thread answering a read with `answer`.
    async fn remote_fetch(
        &mut self,
        mime: &str,
        serial: u32,
        answer: Result<Vec<u8>, Cause>,
    ) -> Result<Vec<u8>, TransferError> {
        let body = Rc::new(RefCell::new(Vec::new()));
        let server = self.server.clone();
        let fetch = server.fetch(
            ClipboardItem::Mime(mime.to_owned()),
            serial,
            SharedSink(body.clone()),
            Some(MAX_ITEM),
        );
        let reader = async {
            let (_, _, reply) = self.next_read().await;
            let _ = reply.send(answer);
        };
        let (fetched, ()) = tokio::join!(fetch, reader);
        fetched.map(|_| body.take())
    }

    /// Make the remote side offer, and wait until the engine reported it.
    async fn remote_offer(
        &mut self,
        serial: u32,
        mimes: &[&str],
        files: Vec<ClipboardFile>,
    ) -> (Vec<String>, Vec<ClipboardFile>) {
        self.server
            .send(ClipboardMsg::Offer {
                serial,
                mime_types: mimes.iter().map(|m| (*m).to_owned()).collect(),
                files,
            })
            .await
            .expect("the offer is sent");
        let (seen, mimes, files) = self.next_remote_offer().await;
        assert_eq!(seen, serial);
        (mimes, files)
    }

    /// Ask the engine to paste a flavour of the remote offer.
    fn paste(&self, mime: &str, serial: u32) -> oneshot::Receiver<Result<Vec<u8>, Cause>> {
        let (result, receiver) = oneshot::channel();
        self.engine.on_command(Command::FetchMime {
            mime_type: mime.to_owned(),
            serial,
            result,
        });
        receiver
    }

    fn paste_files(&self, serial: u32) -> oneshot::Receiver<Result<Vec<PathBuf>, Cause>> {
        let (result, receiver) = oneshot::channel();
        self.engine
            .on_command(Command::FetchFiles { serial, result });
        receiver
    }
}

/// Answer the remote side's file requests from `contents`, counting them.
fn serve_files(
    server: Transfers,
    mut events: mpsc::UnboundedReceiver<Event>,
    contents: Vec<Vec<u8>>,
) -> Rc<Cell<u32>> {
    let served = Rc::new(Cell::new(0));
    let count = served.clone();
    tokio::task::spawn_local(async move {
        while let Some(event) = events.recv().await {
            if let Event::Request {
                id,
                item: ClipboardItem::File(index),
                ..
            } = event
            {
                count.set(count.get() + 1);
                server.serve(
                    id,
                    BytesSource(Bytes::from(contents[index as usize].clone())),
                );
            }
        }
    });
    served
}

fn file(path: &str, size: u64) -> ClipboardFile {
    ClipboardFile {
        path: path.to_owned(),
        size,
        dir: false,
    }
}

fn dir(path: &str) -> ClipboardFile {
    ClipboardFile {
        path: path.to_owned(),
        size: 0,
        dir: true,
    }
}

// Local to remote.

#[test]
fn a_request_before_any_offer_is_refused() {
    run(async {
        let mut rig = rig("before-offer");
        let result = rig.remote_fetch_unanswered(TEXT, 0).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        rig.no_read_asked();
    });
}

#[test]
fn a_local_offer_is_served_on_request() {
    run(async {
        let mut rig = rig("served");
        let serial = rig.offer(&[TEXT, "text/plain"]).await;
        assert_eq!(serial, 1);
        let body = rig
            .remote_fetch("text/plain", serial, Ok(b"hello".to_vec()))
            .await
            .expect("fetched");
        assert_eq!(body, b"hello");
    });
}

#[test]
fn the_windows_thread_is_told_the_offer_generation() {
    run(async {
        let mut rig = rig("generation");
        let serial = rig.offer(&[TEXT]).await;
        let body = Rc::new(RefCell::new(Vec::new()));
        let server = rig.server.clone();
        let fetch = server.fetch(
            ClipboardItem::Mime(TEXT.to_owned()),
            serial,
            SharedSink(body),
            Some(MAX_ITEM),
        );
        let reader = async {
            let (mime, generation, reply) = rig.next_read().await;
            assert_eq!(mime, TEXT);
            assert_eq!(generation, 7);
            let _ = reply.send(Ok(Vec::new()));
        };
        let (fetched, ()) = tokio::join!(fetch, reader);
        fetched.expect("an empty body is a body");
    });
}

#[test]
fn serials_are_never_reused() {
    run(async {
        let mut rig = rig("serials");
        assert_eq!(rig.offer(&[TEXT]).await, 1);
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));
        assert_eq!(rig.offer(&[TEXT]).await, 2);
        assert_eq!(
            rig.offer(&[]).await,
            3,
            "an empty offer takes a fresh serial"
        );
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));
        assert_eq!(rig.offer(&[TEXT]).await, 4);
    });
}

#[test]
fn a_request_against_a_replaced_offer_is_refused() {
    run(async {
        let mut rig = rig("replaced");
        let old = rig.offer(&[TEXT]).await;
        let new = rig.offer(&[TEXT]).await;
        assert_ne!(old, new);
        let result = rig.remote_fetch_unanswered(TEXT, old).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        rig.no_read_asked();
    });
}

#[test]
fn a_request_after_a_revocation_is_refused() {
    run(async {
        let mut rig = rig("revoked");
        let serial = rig.offer(&[TEXT]).await;
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));
        let result = rig.remote_fetch_unanswered(TEXT, serial).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        rig.no_read_asked();
    });
}

#[test]
fn a_flavour_that_was_not_offered_is_refused() {
    run(async {
        let mut rig = rig("unoffered");
        let serial = rig.offer(&[TEXT]).await;
        let result = rig.remote_fetch_unanswered("image/png", serial).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        rig.no_read_asked();
    });
}

#[test]
fn a_failed_read_is_refused_and_the_next_one_works() {
    run(async {
        let mut rig = rig("failed-read");
        let serial = rig.offer(&[TEXT]).await;
        let failed = rig
            .remote_fetch(TEXT, serial, Err(Cause::ClipboardBusy))
            .await;
        assert!(matches!(failed, Err(TransferError::Aborted)), "{failed:?}");
        let body = rig
            .remote_fetch(TEXT, serial, Ok(b"again".to_vec()))
            .await
            .expect("fetched");
        assert_eq!(body, b"again");
    });
}

#[test]
fn a_body_over_the_limit_is_refused_and_the_next_copy_works() {
    run(async {
        let mut rig = rig("over-limit");
        let serial = rig.offer(&[TEXT]).await;
        let too_big = vec![0u8; 33 * 1024 * 1024];
        let result = rig.remote_fetch(TEXT, serial, Ok(too_big)).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        let serial = rig.offer(&[TEXT]).await;
        let body = rig
            .remote_fetch(TEXT, serial, Ok(b"small".to_vec()))
            .await
            .expect("fetched");
        assert_eq!(body, b"small");
    });
}

#[test]
fn a_body_at_the_limit_is_served_whole() {
    run(async {
        let mut rig = rig("at-limit");
        let serial = rig.offer(&[TEXT]).await;
        let exact = vec![7u8; usize::try_from(MAX_ITEM).expect("fits")];
        let body = rig
            .remote_fetch(TEXT, serial, Ok(exact.clone()))
            .await
            .expect("fetched");
        assert!(body == exact);
    });
}

/// A paste target that holds the first chunk until it is released.
struct StallSink {
    started: Rc<Notify>,
    released: Rc<Cell<bool>>,
    release: Rc<Notify>,
}

impl ChunkSink for StallSink {
    async fn write(&mut self, _chunk: Bytes) -> std::io::Result<()> {
        self.started.notify_one();
        while !self.released.get() {
            self.release.notified().await;
        }
        Ok(())
    }

    async fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_revocation_stops_a_transfer_in_flight() {
    run(async {
        let mut rig = rig("revoke-in-flight");
        let serial = rig.offer(&[TEXT]).await;
        let started = Rc::new(Notify::new());
        let released = Rc::new(Cell::new(false));
        let release = Rc::new(Notify::new());
        let fetch = tokio::task::spawn_local({
            let server = rig.server.clone();
            let sink = StallSink {
                started: started.clone(),
                released: released.clone(),
                release: release.clone(),
            };
            async move {
                server
                    .fetch(
                        ClipboardItem::Mime(TEXT.to_owned()),
                        serial,
                        sink,
                        Some(MAX_ITEM),
                    )
                    .await
            }
        });
        let (_, _, reply) = rig.next_read().await;
        reply
            .send(Ok(vec![1u8; 8 * 1024 * 1024]))
            .expect("the read is awaited");
        started.notified().await;
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));
        released.set(true);
        release.notify_one();
        let result = fetch.await.expect("the task ran");
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
    });
}

#[test]
fn a_revocation_while_the_clipboard_is_read_refuses_the_request() {
    run(async {
        let mut rig = rig("revoke-while-reading");
        let serial = rig.offer(&[TEXT]).await;
        let body = Rc::new(RefCell::new(Vec::new()));
        let fetch = tokio::task::spawn_local({
            let server = rig.server.clone();
            async move {
                server
                    .fetch(
                        ClipboardItem::Mime(TEXT.to_owned()),
                        serial,
                        SharedSink(body),
                        Some(MAX_ITEM),
                    )
                    .await
            }
        });
        let (_, _, reply) = rig.next_read().await;
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));
        let _ = reply.send(Ok(b"too late".to_vec()));
        let result = fetch.await.expect("the task ran");
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
    });
}

#[test]
fn local_files_are_served_from_disk() {
    run(async {
        let mut rig = rig("local-files");
        let tree = rig.dir.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).expect("create");
        std::fs::write(tree.join("a.txt"), b"alpha").expect("write");
        std::fs::write(tree.join("sub").join("b.txt"), b"bravo!").expect("write");
        let files = clip_files::list_files(std::slice::from_ref(&tree)).expect("listed");
        assert_eq!(files.entries.len(), 4, "{:?}", files.entries);
        let entries = files.entries.clone();
        let serial = rig.offer_files(&[], files).await;
        for (index, entry) in entries.iter().enumerate().filter(|(_, e)| !e.dir) {
            let body = Rc::new(RefCell::new(Vec::new()));
            let got = rig
                .server
                .fetch(
                    ClipboardItem::File(u32::try_from(index).expect("small")),
                    serial,
                    SharedSink(body.clone()),
                    Some(entry.size),
                )
                .await
                .expect("served");
            assert_eq!(got, entry.size);
            let expected = std::fs::read(rig.dir.path().join(&entry.path)).expect("read");
            assert_eq!(*body.borrow(), expected, "{}", entry.path);
        }
        let missing = rig
            .server
            .fetch(
                ClipboardItem::File(99),
                serial,
                SharedSink(Rc::default()),
                Some(1),
            )
            .await;
        assert!(
            matches!(missing, Err(TransferError::Aborted)),
            "{missing:?}"
        );
    });
}

// Remote to local.

#[test]
fn a_remote_offer_is_reported_and_replaces_the_local_one() {
    run(async {
        let mut rig = rig("remote-offer");
        let local = rig.offer(&[TEXT]).await;
        let (mimes, files) = rig.remote_offer(9, &["text/plain"], vec![]).await;
        assert_eq!(mimes, ["text/plain"]);
        assert!(files.is_empty());
        let result = rig.remote_fetch_unanswered(TEXT, local).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        rig.no_read_asked();
    });
}

/// Fetch the text of `serial` and answer the read, failing instead of
/// hanging when the engine refuses the request and never asks for a read.
async fn served(rig: &mut Rig, serial: u32) -> Vec<u8> {
    let fetched = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rig.remote_fetch(TEXT, serial, Ok(b"hello".to_vec())),
    )
    .await
    .expect("the engine asked for a read, so it did not refuse the request");
    fetched.expect("the request is served")
}

/// The server offers its clipboard when a session starts, and when it has
/// nothing to offer the offer is empty. Upstream's client does not let an
/// empty offer touch the local clipboard, so the local offer stays valid.
#[test]
fn an_empty_remote_offer_leaves_the_local_offer_in_place() {
    run(async {
        let mut rig = rig("remote-empty");
        let local = rig.offer(&[TEXT]).await;
        // The engine still reports it, so Windows can drop promises of an
        // older remote offer that are still its own.
        let (mimes, files) = rig.remote_offer(1, &[], vec![]).await;
        assert!(mimes.is_empty() && files.is_empty());
        let body = served(&mut rig, local).await;
        assert_eq!(body, b"hello");
    });
}

#[test]
fn invalid_remote_files_are_not_passed_on() {
    run(async {
        let mut rig = rig("bad-files");
        let (_, files) = rig
            .remote_offer(1, &["text/plain"], vec![file("../escape.txt", 1)])
            .await;
        assert!(files.is_empty());
    });
}

#[test]
fn a_remote_item_is_fetched_whole() {
    run(async {
        let mut rig = rig("fetch");
        rig.remote_offer(5, &["text/plain"], vec![]).await;
        let pasted = rig.paste("text/plain", 5);
        let Some(Event::Request { id, serial, item }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        assert_eq!(serial, 5);
        assert_eq!(item, ClipboardItem::Mime("text/plain".to_owned()));
        let body: Vec<u8> = (0..700_000u32).map(|n| (n % 251) as u8).collect();
        rig.server.serve(id, BytesSource(Bytes::from(body.clone())));
        assert!(pasted.await.expect("answered").expect("fetched") == body);
    });
}

#[test]
fn a_paste_from_a_replaced_offer_fails_without_asking_the_remote_side() {
    run(async {
        let mut rig = rig("fetch-stale");
        rig.remote_offer(5, &["text/plain"], vec![]).await;
        rig.remote_offer(6, &["text/plain"], vec![]).await;
        let error = rig
            .paste("text/plain", 5)
            .await
            .expect("answered")
            .expect_err("stale");
        assert_eq!(error, Cause::OfferReplaced);
        assert!(rig.events.try_recv().is_err(), "no request was sent");
    });
}

#[test]
fn a_remote_item_over_the_limit_fails() {
    run(async {
        let mut rig = rig("fetch-over-limit");
        rig.remote_offer(5, &["image/png"], vec![]).await;
        let pasted = rig.paste("image/png", 5);
        let Some(Event::Request { id, .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        rig.server
            .serve(id, BytesSource(Bytes::from(vec![0u8; 33 * 1024 * 1024])));
        let result = pasted.await.expect("answered");
        assert!(result.is_err());
    });
}

#[test]
fn the_remote_side_refusing_fails_the_paste() {
    run(async {
        let mut rig = rig("fetch-refused");
        rig.remote_offer(5, &["text/plain"], vec![]).await;
        let pasted = rig.paste("text/plain", 5);
        let Some(Event::Request { id, .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        rig.server.refuse(id);
        assert!(pasted.await.expect("answered").is_err());
    });
}

#[test]
fn a_transfer_with_no_progress_is_abandoned_after_the_idle_timeout() {
    run(async {
        let mut rig = rig("idle");
        rig.remote_offer(5, &["text/plain"], vec![]).await;
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let pasted = rig.paste("text/plain", 5);
        let Some(Event::Request { .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        let error = pasted.await.expect("answered").expect_err("idle");
        assert_eq!(error, Cause::IdleTimeout);
        assert!(
            started.elapsed() >= Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
    });
}

/// A source that never has anything, and says so when it is dropped.
struct Silent(Rc<Cell<bool>>);

impl ChunkSource for Silent {
    async fn next(&mut self) -> std::io::Result<Option<Bytes>> {
        std::future::pending().await
    }
}

impl Drop for Silent {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[test]
fn cancelling_a_transfer_tells_the_remote_side_to_stop() {
    run(async {
        let mut rig = rig("cancel");
        rig.remote_offer(5, &["image/png"], vec![]).await;
        let pasted = rig.paste("image/png", 5);
        let Some(Event::Request { id, .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        let dropped = Rc::new(Cell::new(false));
        rig.server.serve(id, Silent(dropped.clone()));
        rig.engine.on_command(Command::CancelTransfer(1));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dropped.get() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the remote side stopped serving");
        assert!(
            pasted.await.is_err(),
            "the cancelled paste is never answered"
        );
    });
}

#[test]
fn remote_files_are_spooled_once_under_windows_names() {
    run(async {
        let mut rig = rig("spool");
        let files = vec![
            dir("pics"),
            file("pics/a:b.txt", 5),
            file("pics/c.txt", 3),
            file("top.txt", 2),
        ];
        rig.remote_offer(2, &[], files).await;
        let contents = vec![vec![], b"alpha".to_vec(), b"abc".to_vec(), b"hi".to_vec()];
        let server = rig.server.clone();
        let served = serve_files(server, rig.take_events(), contents);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert!(paths[0].ends_with("pics"));
        assert!(paths[1].ends_with("top.txt"));
        assert_eq!(std::fs::read(&paths[1]).expect("read"), b"hi");
        assert_eq!(
            std::fs::read(paths[0].join("a_b.txt")).expect("read"),
            b"alpha"
        );
        assert_eq!(std::fs::read(paths[0].join("c.txt")).expect("read"), b"abc");
        assert_eq!(served.get(), 3);
        let again = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        assert_eq!(again, paths);
        assert_eq!(served.get(), 3, "the second paste reuses the spool");
    });
}

#[test]
fn a_short_file_fails_the_paste() {
    run(async {
        let mut rig = rig("short");
        rig.remote_offer(2, &[], vec![file("a.txt", 5)]).await;
        serve_files(
            rig.server.clone(),
            rig.take_events(),
            vec![b"four".to_vec()],
        );
        let error = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect_err("short");
        assert_eq!(error, Cause::WrongFileSize);
    });
}

#[test]
fn pasting_files_needs_the_current_offer_and_some_files() {
    run(async {
        let mut rig = rig("files-stale");
        rig.remote_offer(2, &["text/plain"], vec![]).await;
        let error = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect_err("none");
        assert_eq!(error, Cause::NoFiles);
        let error = rig
            .paste_files(1)
            .await
            .expect("answered")
            .expect_err("stale");
        assert_eq!(error, Cause::OfferReplaced);
    });
}

#[test]
fn a_replaced_offers_spool_is_kept_for_the_grace_period() {
    run(async {
        let mut rig = rig("grace");
        rig.remote_offer(2, &[], vec![file("a.txt", 1)]).await;
        serve_files(rig.server.clone(), rig.take_events(), vec![b"x".to_vec()]);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        let spool_dir = paths[0].parent().expect("in a spool").to_owned();
        assert!(spool_dir.is_dir());
        rig.remote_offer(3, &["text/plain"], vec![]).await;
        tokio::time::pause();
        tokio::time::advance(SPOOL_GRACE - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert!(spool_dir.is_dir(), "still inside the grace period");
        tokio::time::advance(Duration::from_secs(2)).await;
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert!(!spool_dir.exists(), "removed after the grace period");
    });
}

// Files: the fake file system. Trees are built in a temporary directory and
// compared byte for byte.

/// `len` bytes that do not repeat within a chunk: a linear congruential
/// stream from `seed`.
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state.to_le_bytes()[3]
        })
        .collect()
}

/// The tree both directions are tried with: nested folders, a Unicode name,
/// an empty file, an empty folder and a file of several chunks.
fn tree() -> Vec<(ClipboardFile, Vec<u8>)> {
    let big = pattern(16 << 20, 9);
    let small = pattern(1000, 3);
    vec![
        (dir("日本語 ünï 🎉"), vec![]),
        (file("日本語 ünï 🎉/données.txt", 1000), small),
        (dir("日本語 ünï 🎉/deep"), vec![]),
        (dir("日本語 ünï 🎉/deep/deeper"), vec![]),
        (file("日本語 ünï 🎉/deep/deeper/empty.bin", 0), vec![]),
        (file("日本語 ünï 🎉/deep/big.bin", big.len() as u64), big),
        (dir("日本語 ünï 🎉/nothing here"), vec![]),
        (file("loose.txt", 5), b"loose".to_vec()),
    ]
}

/// What is on disk under `root`, as (path with `/`, bytes; `None` for a
/// folder), sorted.
fn on_disk(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    fn walk(path: &Path, rel: &str, out: &mut Vec<(String, Option<Vec<u8>>)>) {
        let meta = std::fs::symlink_metadata(path).expect("metadata");
        if meta.is_dir() {
            out.push((rel.to_owned(), None));
            for entry in std::fs::read_dir(path).expect("list") {
                let entry = entry.expect("entry");
                let name = entry.file_name().to_string_lossy().into_owned();
                walk(&entry.path(), &format!("{rel}/{name}"), out);
            }
        } else {
            out.push((rel.to_owned(), Some(std::fs::read(path).expect("read"))));
        }
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).expect("list") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        walk(&entry.path(), &name, &mut out);
    }
    out.sort();
    out
}

/// The same, for the entries an offer lists with these contents.
fn expected(entries: &[(ClipboardFile, Vec<u8>)]) -> Vec<(String, Option<Vec<u8>>)> {
    let mut out: Vec<_> = entries
        .iter()
        .map(|(f, bytes)| (f.path.clone(), (!f.dir).then(|| bytes.clone())))
        .collect();
    out.sort();
    out
}

fn spool_entries(rig: &Rig) -> usize {
    std::fs::read_dir(rig.dir.path().join("spool"))
        .map(|spools| {
            spools
                .flatten()
                .map(|spool| {
                    std::fs::read_dir(spool.path())
                        .map(Iterator::count)
                        .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Answer file requests from `contents`, but refuse the one for `refused`
/// and every one after it.
fn serve_files_until(
    server: Transfers,
    mut events: mpsc::UnboundedReceiver<Event>,
    contents: Vec<Vec<u8>>,
    refused: u32,
) {
    tokio::task::spawn_local(async move {
        while let Some(event) = events.recv().await {
            if let Event::Request {
                id,
                item: ClipboardItem::File(index),
                ..
            } = event
            {
                if index >= refused {
                    server.refuse(id);
                } else {
                    server.serve(
                        id,
                        BytesSource(Bytes::from(contents[index as usize].clone())),
                    );
                }
            }
        }
    });
}

#[test]
fn a_tree_goes_from_disk_to_the_remote_side_byte_for_byte() {
    run(async {
        let mut rig = rig("tree-out");
        let src = rig.dir.path().join("src");
        let entries = tree();
        for (entry, bytes) in &entries {
            let path = src.join(&entry.path);
            if entry.dir {
                std::fs::create_dir_all(&path).expect("dir");
            } else {
                std::fs::create_dir_all(path.parent().expect("parent")).expect("parents");
                std::fs::write(&path, bytes).expect("file");
            }
        }
        let roots = [src.join("日本語 ünï 🎉"), src.join("loose.txt")];
        let listed = clip_files::list_files(&roots).expect("listed");
        let offered: Vec<ClipboardFile> = listed.entries.clone();
        let serial = rig.offer_files(&[], listed).await;
        let mut got = Vec::new();
        for (index, entry) in offered.iter().enumerate() {
            if entry.dir {
                got.push((entry.path.clone(), None));
                continue;
            }
            let body = Rc::new(RefCell::new(Vec::new()));
            let size = rig
                .server
                .fetch(
                    ClipboardItem::File(u32::try_from(index).expect("small")),
                    serial,
                    SharedSink(body.clone()),
                    Some(entry.size),
                )
                .await
                .expect("served");
            assert_eq!(size, entry.size, "{}", entry.path);
            got.push((entry.path.clone(), Some(body.take())));
        }
        got.sort();
        assert_eq!(got, expected(&entries));
    });
}

#[test]
fn a_local_file_that_vanished_fails_its_paste_and_no_other() {
    run(async {
        let mut rig = rig("vanish");
        let src = rig.dir.path().join("src");
        std::fs::create_dir_all(&src).expect("dir");
        std::fs::write(src.join("a.txt"), b"alpha").expect("a");
        std::fs::write(src.join("b.txt"), b"bravo").expect("b");
        let listed = clip_files::list_files(&[src.join("a.txt"), src.join("b.txt")]).expect("ok");
        let serial = rig.offer_files(&[], listed).await;
        std::fs::remove_file(src.join("a.txt")).expect("gone");
        let gone = rig
            .server
            .fetch(
                ClipboardItem::File(0),
                serial,
                SharedSink(Rc::default()),
                Some(5),
            )
            .await;
        assert!(matches!(gone, Err(TransferError::Aborted)), "{gone:?}");
        let body = Rc::new(RefCell::new(Vec::new()));
        rig.server
            .fetch(
                ClipboardItem::File(1),
                serial,
                SharedSink(body.clone()),
                Some(5),
            )
            .await
            .expect("the next file is served");
        assert_eq!(body.take(), b"bravo");
    });
}

#[test]
fn a_local_file_that_shrank_arrives_short_for_the_remote_side_to_refuse() {
    run(async {
        let mut rig = rig("shrank");
        let src = rig.dir.path().join("src");
        std::fs::create_dir_all(&src).expect("dir");
        std::fs::write(src.join("a.txt"), b"alpha").expect("a");
        let listed = clip_files::list_files(&[src.join("a.txt")]).expect("ok");
        let serial = rig.offer_files(&[], listed).await;
        std::fs::write(src.join("a.txt"), b"al").expect("shrunk");
        let got = rig
            .server
            .fetch(
                ClipboardItem::File(0),
                serial,
                SharedSink(Rc::default()),
                Some(5),
            )
            .await
            .expect("served");
        assert_eq!(got, 2, "the remote side compares this with the offer's 5");
    });
}

#[test]
fn rapid_local_copies_serve_only_the_last() {
    run(async {
        let mut rig = rig("rapid-out");
        let src = rig.dir.path().join("src");
        std::fs::create_dir_all(&src).expect("dir");
        std::fs::write(src.join("one.txt"), b"one").expect("one");
        std::fs::write(src.join("two.txt"), b"two").expect("two");
        let first = rig
            .offer_files(
                &[],
                clip_files::list_files(&[src.join("one.txt")]).expect("ok"),
            )
            .await;
        let second = rig
            .offer_files(
                &[],
                clip_files::list_files(&[src.join("two.txt")]).expect("ok"),
            )
            .await;
        assert_ne!(first, second, "serials are never reused");
        let stale = rig
            .server
            .fetch(
                ClipboardItem::File(0),
                first,
                SharedSink(Rc::default()),
                Some(3),
            )
            .await;
        assert!(matches!(stale, Err(TransferError::Aborted)), "{stale:?}");
        let body = Rc::new(RefCell::new(Vec::new()));
        rig.server
            .fetch(
                ClipboardItem::File(0),
                second,
                SharedSink(body.clone()),
                Some(3),
            )
            .await
            .expect("the last copy is served");
        assert_eq!(body.take(), b"two");
    });
}

#[test]
fn a_remote_tree_is_spooled_byte_for_byte_and_only_top_level_is_listed() {
    run(async {
        let mut rig = rig("tree-in");
        let entries = tree();
        let files: Vec<ClipboardFile> = entries.iter().map(|(f, _)| f.clone()).collect();
        rig.remote_offer(2, &[], files).await;
        let contents = entries.iter().map(|(_, b)| b.clone()).collect();
        serve_files(rig.server.clone(), rig.take_events(), contents);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert_eq!(
            paths
                .iter()
                .map(|p| p.file_name().expect("name").to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["日本語 ünï 🎉", "loose.txt"]
        );
        let spool = paths[0].parent().expect("in a spool").to_owned();
        assert_eq!(on_disk(&spool), expected(&entries));
    });
}

#[test]
fn names_windows_refuses_and_collisions_land_apart_with_every_byte() {
    run(async {
        let mut rig = rig("names");
        let files = vec![
            file("CON.txt", 1),
            file("a:b?.txt", 2),
            file("trail.", 3),
            dir("Docs"),
            file("Docs/Readme.md", 4),
            file("Docs/readme.md", 5),
            dir("docs"),
            file("docs/x", 6),
            file("Readme.md", 7),
            file("readme.md", 8),
        ];
        rig.remote_offer(2, &[], files.clone()).await;
        let contents: Vec<Vec<u8>> = files
            .iter()
            .map(|f| {
                if f.dir {
                    vec![]
                } else {
                    vec![f.size as u8; f.size as usize]
                }
            })
            .collect();
        serve_files(rig.server.clone(), rig.take_events(), contents);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        assert_eq!(paths.len(), 7, "every top-level entry has a path");
        let spool = paths[0].parent().expect("in a spool").to_owned();
        let mut seen = on_disk(&spool);
        seen.sort();
        let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
        // Every file is there once, whatever Windows had to call it.
        let mut sizes: Vec<usize> = seen
            .iter()
            .filter_map(|(_, bytes)| bytes.as_ref().map(Vec::len))
            .collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [1, 2, 3, 4, 5, 6, 7, 8], "{names:?}");
        let lower: std::collections::HashSet<String> =
            names.iter().map(|n| n.to_lowercase()).collect();
        assert_eq!(lower.len(), names.len(), "no two names differ only in case");
        assert!(names.contains(&"_CON.txt"), "{names:?}");
        assert!(names.contains(&"a_b_.txt"), "{names:?}");
        assert!(names.contains(&"trail"), "{names:?}");
        assert!(names.contains(&"docs (2)"), "{names:?}");
    });
}

#[test]
fn a_top_level_name_is_short_enough_for_a_program_without_long_paths() {
    run(async {
        let mut rig = rig("long-top");
        let long = format!("{}.txt", "n".repeat(250));
        let deep = format!("{}/{}", "d".repeat(200), "e".repeat(200));
        rig.remote_offer(
            2,
            &[],
            vec![
                file(&long, 1),
                dir(&"d".repeat(200)),
                dir(&deep),
                file(&format!("{deep}/f.txt"), 1),
            ],
        )
        .await;
        serve_files(
            rig.server.clone(),
            rig.take_events(),
            vec![b"1".to_vec(), vec![], vec![], b"2".to_vec()],
        );
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        for path in &paths {
            let units = path.to_string_lossy().encode_utf16().count();
            assert!(units <= 259, "{units}: {}", path.display());
            assert!(path.exists(), "{}", path.display());
        }
        assert!(paths[0].to_string_lossy().ends_with(".txt"));
        let nested = paths[1].join("e".repeat(200)).join("f.txt");
        assert_eq!(
            std::fs::read(clip_files::extended(&nested)).expect("the deep file is there"),
            b"2"
        );
    });
}

#[test]
fn a_failure_in_the_middle_lists_nothing_and_leaves_no_partial_file() {
    run(async {
        let mut rig = rig("fail-mid");
        let entries = tree();
        let files: Vec<ClipboardFile> = entries.iter().map(|(f, _)| f.clone()).collect();
        rig.remote_offer(2, &[], files).await;
        let contents: Vec<Vec<u8>> = entries.iter().map(|(_, b)| b.clone()).collect();
        // The 16 MiB file (index 5) is refused after the small ones arrived.
        serve_files_until(rig.server.clone(), rig.take_events(), contents, 5);
        let error = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect_err("the paste fails");
        assert_eq!(error, Cause::PeerAborted);
        assert_eq!(spool_entries(&rig), 0, "nothing is left to offer");
    });
}

#[test]
fn a_paste_after_a_failure_starts_over_and_succeeds() {
    run(async {
        let mut rig = rig("retry");
        rig.remote_offer(2, &[], vec![file("a.txt", 5), file("b.txt", 3)])
            .await;
        let events = rig.take_events();
        let server = rig.server.clone();
        // The first run: b.txt is refused. Every request after is answered.
        let first = Rc::new(Cell::new(true));
        tokio::task::spawn_local(async move {
            let mut events = events;
            while let Some(event) = events.recv().await {
                let Event::Request {
                    id,
                    item: ClipboardItem::File(index),
                    ..
                } = event
                else {
                    continue;
                };
                if index == 1 && first.replace(false) {
                    server.refuse(id);
                } else {
                    let body = if index == 0 {
                        &b"alpha"[..]
                    } else {
                        &b"bri"[..]
                    };
                    server.serve(id, BytesSource(Bytes::from_static(body)));
                }
            }
        });
        rig.paste_files(2)
            .await
            .expect("answered")
            .expect_err("b.txt was refused");
        assert_eq!(spool_entries(&rig), 0);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("the second paste works");
        assert_eq!(paths.len(), 2);
        assert_eq!(std::fs::read(&paths[0]).expect("a"), b"alpha");
        assert_eq!(std::fs::read(&paths[1]).expect("b"), b"bri");
    });
}

#[test]
fn cancelling_a_files_paste_leaves_no_partial_file_and_no_answer() {
    run(async {
        let mut rig = rig("cancel-files");
        rig.remote_offer(2, &[], vec![file("a.txt", 5), file("b.txt", 5)])
            .await;
        let mut events = rig.take_events();
        let pasted = rig.paste_files(2);
        // a.txt arrives; b.txt never does.
        let Some(Event::Request { id, .. }) = events.recv().await else {
            panic!("expected the first request");
        };
        rig.server
            .serve(id, BytesSource(Bytes::from_static(b"alpha")));
        let Some(Event::Request { id, .. }) = events.recv().await else {
            panic!("expected the second request");
        };
        let dropped = Rc::new(Cell::new(false));
        rig.server.serve(id, Silent(dropped.clone()));
        rig.engine.on_command(Command::CancelTransfer(1));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dropped.get() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the remote side stopped serving");
        assert!(pasted.await.is_err(), "a cancelled paste is never answered");
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(spool_entries(&rig), 0, "a.txt is removed with the rest");
    });
}

#[test]
fn rapid_remote_copies_paste_the_last_one_and_the_old_paste_fails() {
    run(async {
        let mut rig = rig("rapid-in");
        rig.remote_offer(2, &[], vec![file("old.txt", 3)]).await;
        let mut events = rig.take_events();
        let old = rig.paste_files(2);
        let Some(Event::Request { id: old_id, .. }) = events.recv().await else {
            panic!("expected the old request");
        };
        // The user copies again before the old transfer is answered.
        rig.remote_offer(3, &[], vec![file("new.txt", 3)]).await;
        let stale = rig.paste_files(2).await.expect("answered");
        assert!(stale.is_err(), "the old offer cannot be pasted any more");
        let new = rig.paste_files(3);
        let Some(Event::Request { id: new_id, .. }) = events.recv().await else {
            panic!("expected the new request");
        };
        rig.server
            .serve(new_id, BytesSource(Bytes::from_static(b"new")));
        let paths = new.await.expect("answered").expect("the last copy pastes");
        assert_eq!(std::fs::read(&paths[0]).expect("new"), b"new");
        // The old transfer finishing late changes nothing the user sees.
        rig.server
            .serve(old_id, BytesSource(Bytes::from_static(b"old")));
        let _ = old.await;
        assert!(paths[0].ends_with("new.txt"));
        assert_eq!(std::fs::read(&paths[0]).expect("new"), b"new");
    });
}

#[test]
fn an_empty_file_and_an_empty_folder_are_spooled() {
    run(async {
        let mut rig = rig("empties");
        rig.remote_offer(2, &[], vec![dir("hollow"), file("zero.bin", 0)])
            .await;
        serve_files(rig.server.clone(), rig.take_events(), vec![vec![], vec![]]);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        assert!(paths[0].is_dir());
        assert_eq!(std::fs::metadata(&paths[1]).expect("meta").len(), 0);
    });
}

impl Rig {
    /// The remote side requests an item, expecting a refusal, so the
    /// Windows thread is never asked and nothing answers.
    async fn remote_fetch_unanswered(
        &mut self,
        mime: &str,
        serial: u32,
    ) -> Result<u64, TransferError> {
        self.server
            .fetch(
                ClipboardItem::Mime(mime.to_owned()),
                serial,
                SharedSink(Rc::default()),
                Some(MAX_ITEM),
            )
            .await
    }
}
