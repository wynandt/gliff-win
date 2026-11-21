use std::io::Write;

use super::ClientExit;
use super::RunTally;
use super::client_exit_line;
use super::read_stderr_tail;
use super::run_tally;
use super::session_end_line;
use super::stderr_tail;
use crate::connect::SshFailure;
use crate::session::EndReason;
use crate::session::driver::SessionEnd;
use crate::session::machine::SessionStats;
use crate::test_dir::TestDir;

fn ended(reason: Option<EndReason>, ssh_exit: Option<i32>, errors: Vec<String>) -> SessionEnd {
    SessionEnd {
        reason,
        ssh_exit,
        errors,
        stats: SessionStats::default(),
        slot_dropped: 0,
        stderr_log: None,
    }
}

#[test]
fn every_client_exit_has_its_words_and_the_code() {
    let cases = [
        (
            ClientExit::WindowClosed { waited_out: false },
            Some(0),
            "client exit: the window was closed; exit code 0",
        ),
        (
            ClientExit::WindowClosed { waited_out: true },
            Some(1),
            "client exit: the window was closed; a session did not say Bye within 3 s; exit code 1",
        ),
        (
            ClientExit::WindowGone,
            Some(1),
            "client exit: the window was destroyed without a close request; exit code 1",
        ),
        (
            ClientExit::WindowsEnding,
            None,
            "client exit: Windows is signing out or shutting down; exit code not known",
        ),
        (
            ClientExit::HandedOver,
            Some(0),
            "client exit: another client was running; the machines were handed to it; exit code 0",
        ),
        (
            ClientExit::AlreadyRunning,
            Some(0),
            "client exit: another client was running; nothing to hand over; exit code 0",
        ),
        (
            ClientExit::Failed("refusing to start: bad\nconfig\u{1b}".to_owned()),
            Some(1),
            "client exit: error: refusing to start: badconfig; exit code 1",
        ),
    ];
    for (exit, code, want) in cases {
        assert_eq!(client_exit_line(&exit, code, None), want);
    }
}

#[test]
fn a_tally_is_appended_only_when_one_is_given() {
    let exit = ClientExit::WindowClosed { waited_out: false };
    let tally = RunTally {
        clean: 1,
        unclean: 1,
        start_failures: 2,
    };
    assert_eq!(
        client_exit_line(&exit, Some(1), Some(tally)),
        "client exit: the window was closed; exit code 1; sessions: 1 clean, 1 not clean, 2 failed to start"
    );
    assert!(
        !client_exit_line(&exit, Some(0), None).contains("sessions:"),
        "no tally means no session counts"
    );
}

#[test]
fn a_run_tally_counts_a_clean_bye_apart_from_every_other_end() {
    let ends = [
        ended(Some(EndReason::Bye), Some(0), Vec::new()),
        ended(Some(EndReason::Bye), Some(3), Vec::new()),
        ended(
            Some(EndReason::Link("closed".to_owned())),
            Some(1),
            Vec::new(),
        ),
        ended(None, None, vec!["session thread panicked".to_owned()]),
    ];
    assert_eq!(
        run_tally(2, &ends),
        RunTally {
            clean: 1,
            unclean: 3,
            start_failures: 2,
        }
    );
    assert_eq!(
        run_tally(0, &[]),
        RunTally {
            clean: 0,
            unclean: 0,
            start_failures: 0,
        }
    );
}

#[test]
fn a_session_line_names_each_reason_the_ssh_exit_and_the_errors() {
    let bye = session_end_line(
        Some(2),
        &ended(Some(EndReason::Bye), Some(0), Vec::new()),
        None,
    );
    assert_eq!(bye, "session 2: ended: the client said Bye; ssh exit 0");
    assert!(!bye.contains("ssh stderr tail"));

    let incompatible = session_end_line(
        Some(3),
        &ended(
            Some(EndReason::Incompatible("no chroma".to_owned())),
            Some(0),
            Vec::new(),
        ),
        None,
    );
    assert_eq!(
        incompatible,
        "session 3: ended: the server is incompatible: no chroma; ssh exit 0"
    );

    let server = session_end_line(
        None,
        &ended(
            Some(EndReason::ServerError {
                code: 300,
                message: "capture died".to_owned(),
            }),
            None,
            vec!["write: broken".to_owned(), "wait: late".to_owned()],
        ),
        Some("out of memory"),
    );
    assert_eq!(
        server,
        "session: ended: the server sent error 300: capture died; ssh exit none; \
         errors: write: broken; wait: late; ssh stderr tail: out of memory"
    );

    let link = session_end_line(
        Some(1),
        &ended(
            Some(EndReason::Link("read: closed\nmore".to_owned())),
            Some(1),
            Vec::new(),
        ),
        None,
    );
    assert_eq!(
        link,
        "session 1: ended: the link was lost: read: closedmore; ssh exit 1"
    );

    let ssh = session_end_line(
        Some(4),
        &ended(
            Some(EndReason::Ssh(SshFailure::HostKeyChanged)),
            Some(255),
            Vec::new(),
        ),
        None,
    );
    assert_eq!(
        ssh,
        "session 4: ended: ssh failed: host_key_changed; ssh exit 255"
    );
}

#[test]
fn a_thread_that_ended_before_the_protocol_names_the_panic() {
    let line = session_end_line(
        None,
        &ended(None, None, vec!["session thread panicked".to_owned()]),
        None,
    );
    assert_eq!(
        line,
        "session: ended: the session thread ended before the protocol ran; ssh exit none; \
         errors: session thread panicked"
    );
    assert!(!line.contains("ssh stderr tail"));
}

#[test]
fn an_empty_tail_is_left_off_the_session_line() {
    let line = session_end_line(
        Some(1),
        &ended(Some(EndReason::Bye), Some(3), Vec::new()),
        Some("  \n"),
    );
    assert_eq!(line, "session 1: ended: the client said Bye; ssh exit 3");
}

#[test]
fn a_stderr_tail_keeps_the_last_six_lines_and_cuts_a_long_one() {
    let seven: String = (1..=7).map(|n| format!("line {n}\n")).collect();
    assert_eq!(
        stderr_tail(&seven).as_deref(),
        Some("line 2 | line 3 | line 4 | line 5 | line 6 | line 7")
    );

    let long = "!".repeat(300);
    let cut = stderr_tail(&long).expect("a long line still yields a tail");
    assert_eq!(cut, format!("{}...", "!".repeat(200)));
    assert!(!cut.contains(&"!".repeat(201)));
}

#[test]
fn a_stderr_tail_drops_control_characters_and_keeps_crlf_lines() {
    let marked = "hello\u{1b}[31mworld\r\nnext\r\n";
    assert_eq!(
        stderr_tail(marked).as_deref(),
        Some("hello[31mworld | next")
    );
    assert_eq!(stderr_tail("only\0\x1bdone").as_deref(), Some("onlydone"));
}

#[test]
fn a_fingerprint_a_path_and_a_key_blob_do_not_survive_the_tail() {
    let text = "\
The fingerprint for the ED25519 key sent by the remote host is
SHA256:kewFiRGj02X1n6KzFRID5mmHmKrpaXNy/Wgiqp4o4rw.
Offending ED25519 key in /home/user/.ssh/known_hosts:4
MD5:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
Host key verification failed.
";
    let tail = stderr_tail(text).expect("tail");
    assert!(tail.contains("SHA256:(hidden)"), "{tail}");
    assert!(tail.contains("MD5:(hidden)"), "{tail}");
    assert!(tail.contains("(a file)"), "{tail}");
    assert!(tail.contains("(hidden)"), "{tail}");
    assert!(tail.contains("Host key verification failed."), "{tail}");
    assert!(!tail.contains("kewFiRGj02X1n6KzFRID5mmHmKrpaXNy"), "{tail}");
    assert!(!tail.contains("/home/user"), "{tail}");
    assert!(!tail.contains("AAAAC3Nza"), "{tail}");
    assert!(!tail.contains("11:22:33:44"), "{tail}");
}

#[test]
fn the_host_changed_corpus_leaks_no_path() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/ssh_corpus/openssh-10.0p2-linux/host_changed.stderr");
    let text = std::fs::read_to_string(&path).expect("corpus");
    let tail = stderr_tail(&text).expect("corpus tail");
    assert!(tail.contains("(a file)"), "{tail}");
    assert!(!tail.contains("/home/"), "{tail}");
    assert!(!tail.contains("kh_other"), "{tail}");
    assert!(tail.contains("Host key verification failed."), "{tail}");
}

#[test]
fn a_file_larger_than_the_read_keeps_only_the_tail_and_drops_the_cut_line() {
    let dir = TestDir::new();
    let path = dir.path().join("stderr.log");
    let mut file = std::fs::File::create(&path).expect("create");
    let early = "secret-early-marker\n";
    file.write_all(early.as_bytes()).expect("write marker");
    let filler = "filler line that must not be the tail\n";
    while file.metadata().expect("len").len() < 20 * 1024 {
        file.write_all(filler.as_bytes()).expect("write filler");
    }
    let xs = "X".repeat(16 * 1024 + 32);
    file.write_all(xs.as_bytes()).expect("write partial");
    file.write_all(b"\nonly-complete\n").expect("write tail");
    drop(file);

    let tail = read_stderr_tail(&path).expect("tail of a long file");
    assert_eq!(tail, "only-complete");
    assert!(!tail.contains("secret-early-marker"), "{tail}");
    assert!(!tail.contains('X'), "{tail}");
    assert!(!tail.contains("filler"), "{tail}");
}
