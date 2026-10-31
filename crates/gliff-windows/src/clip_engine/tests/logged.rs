//! What the engine writes to the log, for the causes of an Abort and for
//! content that must never be there.
//!
//! Each test runs a whole scenario on the test thread, captures the lines
//! `clip_log` emitted meanwhile and compares them with the lines a reader of
//! the log would see. The lines are printed too: `cargo test -- --nocapture`
//! shows what each cause looks like.

use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use bytes::Bytes;
use gliff_proto::ClipboardItem;
use gliff_proto::clipboard::MAX_ITEM;
use gliff_transport::clipboard::Event;
use gliff_transport::clipboard::TransferError;
use tokio::sync::Notify;

use super::SharedSink;
use super::Silent;
use super::StallSink;
use super::TEXT;
use super::dir;
use super::file;
use super::rig;
use super::run;
use super::serve_files;
use crate::clip_engine::BytesSource;
use crate::clip_engine::Command;
use crate::clip_files;
use crate::clip_log::Cause;
use crate::clip_log::RevokeWhy;
use crate::clip_log::testing;

/// Runs `test` and returns the log lines it caused, with durations replaced
/// by `N` so the lines can be compared.
fn logged(test: impl Future<Output = ()>) -> Vec<String> {
    let ((), lines) = testing::capture(|| run(test));
    for line in &lines {
        println!("{line}");
        // Every line of every test must have only the forms of a log line.
        testing::assert_well_formed(line);
    }
    lines.iter().map(|line| without_durations(line)).collect()
}

fn without_durations(line: &str) -> String {
    line.split(' ')
        .map(|word| match word.split_once('=') {
            Some((key @ ("ms" | "read_ms"), _)) => format!("{key}=N"),
            _ => word.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn offer_line() -> &'static str {
    "clipboard: offer sent serial=1 generation=7 mimes=text/plain;charset=utf-8 files=0 file_bytes=0"
}

// What aborts a transfer, as the log shows it.

#[test]
fn a_clipboard_that_moved_before_the_read_aborts_the_fetch() {
    for (cause, name) in [
        (Cause::ClipboardChanged, "clipboard_changed"),
        (Cause::ClipboardSequenceMoved, "clipboard_sequence_moved"),
        (Cause::ClipboardBusy, "clipboard_busy"),
        (Cause::ThreadBusy, "thread_busy"),
        (Cause::FormatMissing, "format_missing"),
    ] {
        let lines = logged(async {
            let mut rig = rig("log-read-failed");
            let serial = rig.offer(&[TEXT]).await;
            let result = rig.remote_fetch(TEXT, serial, Err(cause)).await;
            assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        });
        let refused = format!("clipboard: request refused id=2 cause={name} initiator=local");
        assert_eq!(
            lines,
            [
                offer_line(),
                "clipboard: request received id=2 serial=1 mime=text/plain;charset=utf-8",
                refused.as_str(),
                "clipboard: abort sent id=2",
            ],
            "{name}"
        );
    }
}

#[test]
fn a_revocation_while_the_clipboard_is_read_aborts_the_fetch() {
    let lines = logged(async {
        let mut rig = rig("log-revoke-reading");
        let serial = rig.offer(&[TEXT]).await;
        let fetch = tokio::task::spawn_local({
            let server = rig.server.clone();
            async move {
                server
                    .fetch(
                        ClipboardItem::Mime(TEXT.to_owned()),
                        serial,
                        SharedSink(Rc::default()),
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
    assert_eq!(
        lines,
        [
            offer_line(),
            "clipboard: request received id=2 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: offer revoked why=local_change serial=1 serving=0",
            "clipboard: request refused id=2 cause=revoked_while_reading initiator=local",
            "clipboard: abort sent id=2",
        ]
    );
}

#[test]
fn a_revocation_while_a_body_is_sent_cuts_it_and_aborts_the_fetch() {
    let lines = logged(async {
        let mut rig = rig("log-revoke-serving");
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
    assert_eq!(
        lines,
        [
            offer_line(),
            "clipboard: request received id=2 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: serve started id=2 bytes=8388608 read_ms=N",
            "clipboard: offer revoked why=local_change serial=1 serving=1",
            "clipboard: serve ended id=2 result=cut bytes=1048576 ms=N",
            "clipboard: abort sent id=2",
        ]
    );
}

#[test]
fn requests_that_name_no_current_item_are_refused_with_their_cause() {
    let lines = logged(async {
        let mut rig = rig("log-refusals");
        let result = rig.remote_fetch_unanswered(TEXT, 9).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        let old = rig.offer(&[TEXT]).await;
        let new = rig.offer(&[TEXT]).await;
        assert_ne!(old, new);
        let result = rig.remote_fetch_unanswered(TEXT, old).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
        let result = rig.remote_fetch_unanswered("image/png", new).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
    });
    assert_eq!(
        lines,
        [
            "clipboard: request received id=2 serial=9 mime=text/plain;charset=utf-8",
            "clipboard: request refused id=2 cause=no_offer initiator=local",
            "clipboard: abort sent id=2",
            "clipboard: offer sent serial=1 generation=7 mimes=text/plain;charset=utf-8 files=0 file_bytes=0",
            "clipboard: offer sent serial=2 generation=7 mimes=text/plain;charset=utf-8 files=0 file_bytes=0",
            "clipboard: request received id=4 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: request refused id=4 cause=stale_serial initiator=local",
            "clipboard: abort sent id=4",
            "clipboard: request received id=6 serial=2 mime=image/png",
            "clipboard: request refused id=6 cause=not_offered initiator=local",
            "clipboard: abort sent id=6",
        ]
    );
}

/// The peer gives up on a request while the Windows thread is still reading
/// the clipboard, as a server does that drops a fetch at its own deadline.
/// The log shows `abort received` and no `abort sent`: this side is not the
/// one that cancelled, and the other side's toast is not "the peer aborted".
#[test]
fn a_peer_that_drops_its_fetch_during_the_read_is_logged_as_the_peer() {
    let lines = logged(async {
        // The clock only moves when every task waits, so the order of the
        // events does not depend on the machine's speed.
        tokio::time::pause();
        let mut rig = rig("log-peer-cut");
        let serial = rig.offer(&[TEXT]).await;
        let waited = tokio::time::timeout(
            Duration::from_secs(2),
            rig.server.fetch(
                ClipboardItem::Mime(TEXT.to_owned()),
                serial,
                SharedSink(Rc::default()),
                Some(MAX_ITEM),
            ),
        )
        .await;
        assert!(waited.is_err(), "the read is never answered");
        let (_, _, reply) = rig.next_read().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = reply.send(Ok(b"late".to_vec()));
        tokio::time::sleep(Duration::from_millis(20)).await;
    });
    assert_eq!(
        lines,
        [
            offer_line(),
            "clipboard: request received id=2 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: abort received id=2 transfer=serving initiator=peer",
            "clipboard: serve started id=2 bytes=4 read_ms=N",
            "clipboard: serve ended id=2 result=done bytes=4 ms=N",
        ]
    );
}

/// The log of a real session start: this client offers its clipboard, the
/// server's own empty offer crosses it, and the server's history watcher then
/// reads the offer it was sent. The empty offer replaces nothing, so the
/// requests are served; no `abort sent` (a toast on the server) is logged.
#[test]
fn the_servers_empty_offer_at_connect_does_not_abort_its_watchers_reads() {
    let lines = logged(async {
        let mut rig = rig("log-connect-cross");
        let serial = rig.offer(&[TEXT]).await;
        rig.remote_offer(serial, &[], vec![]).await;
        for _ in 0..2 {
            let fetched = tokio::time::timeout(
                Duration::from_secs(5),
                rig.remote_fetch(TEXT, serial, Ok(b"copy".to_vec())),
            )
            .await;
            let body = fetched.expect("the engine asked for a read, so it served");
            assert_eq!(body.expect("served"), b"copy");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    });
    assert_eq!(
        lines,
        [
            offer_line(),
            "clipboard: offer received serial=1 mimes=none files=0 file_bytes=0 files_valid=true",
            "clipboard: request received id=2 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: serve started id=2 bytes=4 read_ms=N",
            "clipboard: serve ended id=2 result=done bytes=4 ms=N",
            "clipboard: request received id=4 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: serve started id=4 bytes=4 read_ms=N",
            "clipboard: serve ended id=4 result=done bytes=4 ms=N",
        ]
    );
}

/// The same crossing with an offer that does carry something: it takes over
/// the clipboard, so the local offer is revoked and a read of it is refused.
/// This abort is a real failure and still surfaces.
#[test]
fn a_usable_remote_offer_still_aborts_a_read_of_the_replaced_local_offer() {
    let lines = logged(async {
        let mut rig = rig("log-connect-replaced");
        let serial = rig.offer(&[TEXT]).await;
        rig.remote_offer(serial, &[TEXT], vec![]).await;
        let result = rig.remote_fetch_unanswered(TEXT, serial).await;
        assert!(matches!(result, Err(TransferError::Aborted)), "{result:?}");
    });
    assert_eq!(
        lines,
        [
            offer_line(),
            "clipboard: offer received serial=1 mimes=text/plain;charset=utf-8 files=0 file_bytes=0 files_valid=true",
            "clipboard: offer revoked why=remote_offer serial=1 serving=0",
            "clipboard: request received id=2 serial=1 mime=text/plain;charset=utf-8",
            "clipboard: request refused id=2 cause=no_offer initiator=local",
            "clipboard: abort sent id=2",
        ]
    );
}

// Pastes.

/// A cancelled paste ends as `cancelled`, not as `dropped` (the session
/// ended); a cancel of a paste that is over, or of none, says nothing.
#[test]
fn a_cancelled_paste_is_logged_as_cancelled_and_a_stray_cancel_is_not() {
    let lines = logged(async {
        let mut rig = rig("log-cancel");
        rig.remote_offer(5, &["image/png"], vec![]).await;
        let pasted = rig.paste("image/png", 5);
        let Some(Event::Request { id, .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        let dropped = Rc::new(Cell::new(false));
        rig.server.serve(id, Silent(dropped.clone()));
        rig.engine.on_command(Command::CancelTransfer(1));
        while !dropped.get() {
            tokio::task::yield_now().await;
        }
        assert!(pasted.await.is_err());
        rig.engine.on_command(Command::CancelTransfer(1));
        rig.engine.on_command(Command::CancelTransfer(99));
    });
    assert_eq!(
        lines,
        [
            "clipboard: offer received serial=5 mimes=image/png files=0 file_bytes=0 files_valid=true",
            "clipboard: paste started paste=1 serial=5 mime=image/png",
            "clipboard: request sent id=1 serial=5 mime=image/png",
            "clipboard: paste cancelled paste=1 initiator=local",
            "clipboard: paste ended paste=1 result=failed cause=cancelled initiator=local bytes=0 ms=N",
            "clipboard: abort sent id=1",
        ]
    );
}

#[test]
fn a_paste_logs_its_offer_request_and_size() {
    let lines = logged(async {
        let mut rig = rig("log-paste");
        rig.remote_offer(5, &["text/plain"], vec![]).await;
        let pasted = rig.paste("text/plain", 5);
        let Some(Event::Request { id, .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        rig.server
            .serve(id, BytesSource(Bytes::from_static(b"hello")));
        assert_eq!(pasted.await.expect("answered").expect("fetched"), b"hello");
    });
    assert_eq!(
        lines,
        [
            "clipboard: offer received serial=5 mimes=text/plain files=0 file_bytes=0 files_valid=true",
            "clipboard: paste started paste=1 serial=5 mime=text/plain",
            "clipboard: request sent id=1 serial=5 mime=text/plain",
            "clipboard: paste ended paste=1 result=done bytes=5 ms=N",
        ]
    );
}

#[test]
fn a_paste_that_the_peer_aborts_says_who() {
    let lines = logged(async {
        let mut rig = rig("log-paste-aborted");
        rig.remote_offer(5, &["text/plain"], vec![]).await;
        let pasted = rig.paste("text/plain", 5);
        let Some(Event::Request { id, .. }) = rig.events.recv().await else {
            panic!("expected a request");
        };
        rig.server.refuse(id);
        let error = pasted.await.expect("answered").expect_err("aborted");
        assert_eq!(error, Cause::PeerAborted);
    });
    assert_eq!(
        lines,
        [
            "clipboard: offer received serial=5 mimes=text/plain files=0 file_bytes=0 files_valid=true",
            "clipboard: paste started paste=1 serial=5 mime=text/plain",
            "clipboard: request sent id=1 serial=5 mime=text/plain",
            "clipboard: abort received id=1 transfer=fetching initiator=peer",
            "clipboard: paste ended paste=1 result=failed cause=peer_aborted initiator=peer bytes=0 ms=N",
        ]
    );
}

// Content never reaches the log.

/// A string that every kind of clipboard data below carries. Neither it nor
/// anything made of it may appear in a line.
const SENTINEL: &str = "Zq7-sentinel-4417";

fn assert_clean(lines: &[String], dir_name: &str) {
    assert!(!lines.is_empty(), "nothing was logged");
    let lower = SENTINEL.to_lowercase();
    let upper = SENTINEL.to_uppercase();
    for line in lines {
        // `logged` already checked the form of the raw line; this names the
        // strings that were fed in.
        for banned in [SENTINEL, lower.as_str(), upper.as_str(), dir_name, "folder"] {
            assert!(!line.contains(banned), "{banned:?} is in {line:?}");
        }
    }
}

#[test]
fn nothing_a_clipboard_holds_reaches_the_log() {
    let name = format!("{SENTINEL} folder");
    let leaf = format!("{SENTINEL}.txt");
    let mime = format!("text/{SENTINEL}");
    // A table name with a parameter that carries a name is not the table name.
    let with_parameter = format!("text/plain;charset=utf-8;filename={SENTINEL}");
    let lines = logged(async {
        let mut rig = rig("log-sentinel");

        // Local files and flavours, served and refused.
        let tree = rig.dir.path().join(&name);
        std::fs::create_dir_all(&tree).expect("create");
        std::fs::write(tree.join(&leaf), SENTINEL).expect("write");
        let listed = clip_files::list_files(std::slice::from_ref(&tree)).expect("listed");
        let entries = listed.entries.clone();
        let serial = rig
            .offer_files(&[&mime, &with_parameter, TEXT], listed)
            .await;
        for (index, entry) in entries.iter().enumerate().filter(|(_, e)| !e.dir) {
            let body = Rc::new(RefCell::new(Vec::new()));
            rig.server
                .fetch(
                    ClipboardItem::File(u32::try_from(index).expect("small")),
                    serial,
                    SharedSink(body.clone()),
                    Some(entry.size),
                )
                .await
                .expect("served");
            assert_eq!(body.take(), SENTINEL.as_bytes());
        }
        let body = rig
            .remote_fetch(&mime, serial, Ok(SENTINEL.as_bytes().to_vec()))
            .await
            .expect("fetched");
        assert_eq!(body, SENTINEL.as_bytes());
        let _ = rig
            .remote_fetch(TEXT, serial, Err(Cause::ClipboardChanged))
            .await;
        let _ = rig.remote_fetch_unanswered(SENTINEL, serial).await;
        let _ = rig
            .server
            .fetch(
                ClipboardItem::File(99),
                serial,
                SharedSink(Rc::default()),
                Some(MAX_ITEM),
            )
            .await;
        rig.engine
            .on_command(Command::RevokeLocal(RevokeWhy::LocalChange));

        // Remote files and flavours, pasted, refused and failing.
        let remote = vec![
            dir(&name),
            file(&format!("{name}/{leaf}"), SENTINEL.len() as u64),
        ];
        rig.remote_offer(2, &[&mime, &with_parameter, TEXT], remote)
            .await;
        let contents = vec![vec![], SENTINEL.as_bytes().to_vec()];
        serve_files(rig.server.clone(), rig.take_events(), contents);
        let paths = rig
            .paste_files(2)
            .await
            .expect("answered")
            .expect("spooled");
        assert_eq!(paths.len(), 1);
        rig.remote_offer(4, &[&mime], vec![]).await;
        let _ = rig.paste(&mime, 3).await;
        rig.remote_offer(6, &["text/plain"], vec![file("../x", 1)])
            .await;
        let bad = vec![file(&leaf, 99)];
        rig.remote_offer(8, &[], bad).await;
        let _ = rig.paste_files(8).await;
    });
    let dir_name = format!("gwin-clip-engine-log-sentinel-{}", std::process::id());
    assert_clean(&lines, &dir_name);
    assert!(
        lines.iter().any(|line| line.contains("mime=other")),
        "a flavour outside the table is named `other`: {lines:?}"
    );
}
