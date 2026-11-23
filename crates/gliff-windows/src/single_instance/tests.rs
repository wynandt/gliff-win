use std::path::Path;

use super::DecodeError;
use super::Forward;
use super::Launch;
use super::LockCheck;
use super::MAX_MESSAGE;
use super::MAX_TARGETS;
use super::Start;
use super::after_check;
use super::decide;
use super::decode;
use super::encode;
use super::instance_name;
use crate::history::HostTarget;

fn host(text: &str) -> HostTarget {
    text.parse().expect("a target")
}

#[test]
fn the_first_launch_runs_whatever_it_was_given() {
    assert_eq!(decide(false, &[], false), Launch::Run);
    assert_eq!(decide(false, &[host("a@one")], false), Launch::Run);
    assert_eq!(decide(false, &[], true), Launch::Run, "a logon start runs");
}

#[test]
fn a_second_launch_with_no_target_shows_the_window() {
    assert_eq!(decide(true, &[], false), Launch::HandOver(Forward::Show));
}

#[test]
fn a_second_launch_with_targets_hands_them_over_in_order() {
    let targets = [host("a@one"), host("b@two")];
    assert_eq!(
        decide(true, &targets, false),
        Launch::HandOver(Forward::Open(targets.to_vec()))
    );
}

#[test]
fn a_second_logon_start_exits_quietly_even_with_targets() {
    assert_eq!(decide(true, &[], true), Launch::Quit);
    assert_eq!(decide(true, &[host("a@one")], true), Launch::Quit);
}

#[test]
fn a_failed_lock_check_stops_the_launch() {
    for (targets, autostart) in [
        (Vec::new(), false),
        (vec![host("a@one")], false),
        (vec![host("a@one"), host("b@two")], true),
    ] {
        assert_eq!(
            after_check(
                LockCheck::Failed("access denied".to_owned()),
                &targets,
                autostart
            ),
            Start::Stop("single-instance check failed: access denied".to_owned()),
            "a failed check stops for every target list"
        );
    }
    assert_eq!(
        after_check(LockCheck::Failed(String::new()), &[], false),
        Start::Stop("single-instance check failed: ".to_owned()),
        "an empty error still stops the launch"
    );

    let source = include_str!("../app.rs");
    assert!(
        !source.contains("eprintln!(\"gliff-windows: single-instance check failed:"),
        "an acquire error is still printed to stderr and the launch continues"
    );
    assert!(
        source.contains("single_instance::after_check"),
        "the launch does not ask after_check"
    );
    assert!(
        source.contains("MessageBoxW"),
        "a failed check does not show a message box"
    );
    let stopped = source
        .find("fn lock_stopped")
        .expect("a failed check does not stop with the exit line");
    let body = &source[stopped..];
    let end = body.find("\nfn ").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("client_exit_line"),
        "the stop is not a client-log line"
    );
    assert!(
        body.contains("show_lock_error"),
        "the stop does not show the message box"
    );
    assert!(
        !body.contains("eprintln!"),
        "the lock stop prints to stderr"
    );
    let stop = source
        .find("Start::Stop")
        .expect("a failed check does not stop the launch");
    let startup = source.find("Tabs::start_up").expect("startup");
    assert!(
        stop < startup,
        "the launch reaches window startup after a failed check"
    );
}

#[test]
fn an_existing_client_is_still_the_second_launch() {
    let targets = [host("a@one"), host("b@two")];
    for autostart in [false, true] {
        assert_eq!(
            after_check(LockCheck::Held, &targets, autostart),
            Start::Run,
            "a held lock runs"
        );
        assert_eq!(
            after_check(LockCheck::AlreadyRunning, &targets, autostart),
            Start::Second(decide(true, &targets, autostart)),
            "an existing client is decide"
        );
        assert_eq!(
            after_check(LockCheck::AlreadyRunning, &[], autostart),
            Start::Second(decide(true, &[], autostart))
        );
    }
}

#[test]
fn the_name_follows_the_folder_not_its_spelling() {
    let name = instance_name(Path::new(r"C:\Users\user\AppData\gliff-windows"));
    assert_eq!(
        name,
        instance_name(Path::new(r"c:\users\user\appdata\gliff-windows\")),
        "case and a trailing separator"
    );
    assert_eq!(
        name,
        instance_name(Path::new("C:/Users/user/AppData/gliff-windows")),
        "forward slashes"
    );
    assert_ne!(
        name,
        instance_name(Path::new(r"C:\Users\user\AppData\other")),
        "another folder, another instance"
    );
    assert!(name.starts_with("Local\\gliff-windows-"), "{name}");
    assert!(name.len() < 100, "a mutex name is at most 259 characters");
    assert!(
        name.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'\\' || b == b'-'),
        "{name}"
    );
}

#[test]
fn a_request_survives_the_trip() {
    for forward in [
        Forward::Show,
        Forward::Open(vec![host("a@one")]),
        Forward::Open(vec![
            host("a@one"),
            host("b@two"),
            host("[::1]".trim_matches(['[', ']'])),
        ]),
    ] {
        assert_eq!(decode(&encode(&forward)), Ok(forward));
    }
}

#[test]
fn a_message_that_is_not_ours_is_refused() {
    assert_eq!(decode(b""), Err(DecodeError::BadHeader));
    assert_eq!(decode(b"hello"), Err(DecodeError::BadHeader));
    assert_eq!(decode(&[0xff, 0xfe, 0x00]), Err(DecodeError::NotText));
    assert_eq!(
        decode(&vec![b'a'; MAX_MESSAGE + 1]),
        Err(DecodeError::TooLong(MAX_MESSAGE + 1))
    );
    assert!(matches!(
        decode(b"gliff-forward 1\nrun"),
        Err(DecodeError::UnknownRequest(_))
    ));
    assert!(matches!(
        decode(b"gliff-forward 1\nshow\nextra"),
        Err(DecodeError::UnknownRequest(_))
    ));
    assert!(matches!(
        decode(b"gliff-forward 1"),
        Err(DecodeError::UnknownRequest(_))
    ));
    assert_eq!(
        decode(b"gliff-forward 2\nshow"),
        Err(DecodeError::BadHeader)
    );
}

#[test]
fn an_open_request_checks_every_target_as_the_command_line_does() {
    assert_eq!(
        decode(b"gliff-forward 1\nopen"),
        Err(DecodeError::NoTargets)
    );
    for bad in [
        "-oProxyCommand=x",
        "a@b:22",
        "ssh://a@b",
        "a b@c",
        "a;b@c",
        "",
    ] {
        let message = format!("gliff-forward 1\nopen\nok@host\n{bad}");
        assert!(
            matches!(decode(message.as_bytes()), Err(DecodeError::BadTarget(_))),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn too_many_targets_are_refused() {
    let mut message = String::from("gliff-forward 1\nopen");
    for n in 0..=MAX_TARGETS {
        message.push_str(&format!("\nu@host{n}"));
    }
    assert_eq!(
        decode(message.as_bytes()),
        Err(DecodeError::TooManyTargets(MAX_TARGETS + 1))
    );
}
