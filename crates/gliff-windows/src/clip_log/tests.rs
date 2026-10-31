//! The log's vocabulary: names, causes and formats.

use std::io;
use std::io::ErrorKind;
use std::time::Duration;

use gliff_proto::ClipboardFile;
use gliff_proto::ClipboardItem;
use gliff_proto::ClipboardMsg;
use gliff_transport::clipboard::TransferError;

use super::Cause;
use super::Event;
use super::Files;
use super::MimeList;
use super::MimeName;
use super::Own;
use super::ServeEnd;
use super::testing;

const SENTINEL: &str = "Zq7-sentinel-4417";

fn line(event: Event) -> String {
    let (_, lines) = testing::capture(|| super::emit(event));
    assert_eq!(lines.len(), 1, "{lines:?}");
    lines.into_iter().next().expect("one line")
}

#[test]
fn a_mime_type_outside_the_table_is_logged_as_other() {
    assert_eq!(MimeName::of("text/plain").to_string(), "text/plain");
    assert_eq!(MimeName::of(" IMAGE/PNG ").to_string(), "image/png");
    assert_eq!(MimeName::of(SENTINEL).to_string(), "other");
    assert_eq!(
        MimeName::of(&format!("text/{SENTINEL}")).to_string(),
        "other"
    );
    let list = MimeList::of(&["image/png".to_owned(), SENTINEL.to_owned()]);
    assert_eq!(list.to_string(), "image/png,other");
    assert_eq!(MimeList::of(&[]).to_string(), "none");
}

#[test]
fn files_are_logged_by_count_and_size_only() {
    let files = [
        ClipboardFile {
            path: format!("{SENTINEL}/a"),
            size: 0,
            dir: true,
        },
        ClipboardFile {
            path: format!("{SENTINEL}/a/{SENTINEL}.txt"),
            size: 1000,
            dir: false,
        },
        ClipboardFile {
            path: format!("{SENTINEL}.bin"),
            size: 24,
            dir: false,
        },
    ];
    assert_eq!(Files::of(&files).to_string(), "files=3 file_bytes=1024");
    assert_eq!(Files::of(&[]).to_string(), "files=0 file_bytes=0");
}

#[test]
fn an_io_error_is_logged_by_its_kind_and_never_its_text() {
    let error = io::Error::new(ErrorKind::NotFound, format!("{SENTINEL}/x: gone"));
    let cause = Cause::of_transfer(&TransferError::Io(error));
    assert_eq!(cause, Cause::Io(ErrorKind::NotFound));
    assert_eq!(cause.to_string(), "io_NotFound");
    assert!(!cause.to_string().contains(SENTINEL));
}

#[test]
fn transfer_errors_map_to_a_cause_and_who_started_it() {
    let cases = [
        (TransferError::Aborted, Cause::PeerAborted, "peer"),
        (TransferError::Timeout, Cause::IdleTimeout, "timeout"),
    ];
    for (error, cause, initiator) in cases {
        let got = Cause::of_transfer(&error);
        assert_eq!(got, cause, "{error:?}");
        assert_eq!(got.initiator(), initiator, "{error:?}");
    }
    assert_eq!(Cause::PeerGone.initiator(), "link");
    assert_eq!(Cause::ClipboardChanged.initiator(), "local");
}

#[test]
fn a_cause_is_a_fixed_word() {
    for cause in [
        Cause::NoOffer,
        Cause::StaleSerial,
        Cause::ClipboardSequenceMoved,
        Cause::OfferReplaced,
        Cause::Io(ErrorKind::PermissionDenied),
    ] {
        let text = cause.to_string();
        assert!(
            text.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == '_'),
            "{text}"
        );
    }
}

#[test]
fn events_have_the_documented_form() {
    assert_eq!(
        line(Event::LocalChange {
            sequence: 41,
            own: Own::No
        }),
        "clipboard: local change sequence=41 own=no"
    );
    assert_eq!(
        line(Event::AbortSent { id: 6 }),
        "clipboard: abort sent id=6"
    );
    assert_eq!(
        line(Event::ServeEnded {
            id: 2,
            bytes: 7,
            elapsed: Duration::from_millis(3),
            end: ServeEnd::Cut,
        }),
        "clipboard: serve ended id=2 result=cut bytes=7 ms=3"
    );
}

#[test]
fn only_requests_and_aborts_are_logged_from_the_outbound_queue() {
    let lines = testing::capture(|| {
        super::outbound(&ClipboardMsg::Request {
            id: 3,
            serial: 9,
            item: ClipboardItem::File(2),
        });
        super::outbound(&ClipboardMsg::Abort { id: 3 });
        super::outbound(&ClipboardMsg::Ack {
            id: 3,
            received: 10,
        });
    })
    .1;
    assert_eq!(
        lines,
        [
            "clipboard: request sent id=3 serial=9 file_index=2",
            "clipboard: abort sent id=3"
        ]
    );
}

#[test]
fn the_form_check_rejects_anything_that_could_carry_contents() {
    for line in [
        "clipboard: paste started paste=1 serial=1 mime=text/plain;filename=secret.txt",
        "clipboard: serve started id=2 bytes=4 read_ms=Zq7-sentinel",
        "clipboard: request refused id=2 cause=io_Not-Found initiator=local",
        "clipboard: not started: C:\\Users\\x\\file.txt",
        "clipboard: offer sent mimes=text/plain,text/secret",
        "clipboard: serve ended id=2 result=done hash=3fa9",
        "no prefix",
    ] {
        let rejected = std::panic::catch_unwind(|| testing::assert_well_formed(line));
        assert!(rejected.is_err(), "{line:?} was accepted");
    }
}
