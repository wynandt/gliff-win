use super::Button;
use super::GUIDE_FILE;
use super::Guide;
use super::Phase;
use super::SshFailure;
use super::classify;
use super::from_spawn_error;
use super::log_line;
use super::message;
use crate::connect::ServerVersion;
use crate::history::HostTarget;

fn target() -> HostTarget {
    "me@lab-pc".parse().expect("a target")
}

fn every_failure() -> Vec<SshFailure> {
    vec![
        SshFailure::ClientMissing,
        SshFailure::NameNotFound,
        SshFailure::Refused,
        SshFailure::Unreachable,
        SshFailure::HostUnknown,
        SshFailure::HostKeyChanged,
        SshFailure::KeyRefused,
        SshFailure::KeyFilePermissions,
        SshFailure::TooManyKeys,
        SshFailure::ServerMissing,
        SshFailure::ServerOld {
            found: ServerVersion::new(0, 3, 9),
        },
        SshFailure::ProtocolMismatch,
        SshFailure::Other {
            last_line: "something odd".into(),
        },
    ]
}

#[test]
fn every_failure_has_two_plain_sentences_and_one_button() {
    for failure in every_failure() {
        let m = message(&failure, &target());
        assert!(m.what.ends_with('.'), "{failure:?}: {m:?}");
        assert!(!m.to_do.is_empty(), "{failure:?}: {m:?}");
        assert!(!m.text().contains("Permission denied"), "{failure:?}");
        assert!(!m.text().contains("0x"), "no error code: {failure:?}");
    }
}

#[test]
fn a_message_names_the_machine_but_not_the_user() {
    let named = [
        SshFailure::NameNotFound,
        SshFailure::Refused,
        SshFailure::Unreachable,
        SshFailure::HostUnknown,
        SshFailure::HostKeyChanged,
        SshFailure::KeyRefused,
        SshFailure::TooManyKeys,
        SshFailure::ServerMissing,
        SshFailure::ProtocolMismatch,
        SshFailure::Other {
            last_line: String::new(),
        },
    ];
    for failure in named {
        let text = message(&failure, &target()).text();
        assert!(text.contains("lab-pc"), "{failure:?}: {text}");
        assert!(!text.contains("me@"), "{failure:?}: {text}");
    }
}

#[test]
fn a_changed_identity_offers_no_way_to_continue() {
    let m = message(&SshFailure::HostKeyChanged, &target());
    assert_eq!(m.button, Button::Close);
    assert_eq!(m.guide, Some(Guide::ChangedIdentity));
    assert!(m.to_do.contains("attack"));
}

#[test]
fn the_server_messages_say_which_version_is_needed() {
    let old = message(
        &SshFailure::ServerOld {
            found: ServerVersion::new(0, 3, 9),
        },
        &target(),
    );
    assert!(old.what.contains("0.3.9"), "{old:?}");
    assert!(old.what.contains("0.4.1"), "{old:?}");
    let missing = message(&SshFailure::ServerMissing, &target());
    assert!(missing.to_do.contains("0.4.1"), "{missing:?}");
    assert_eq!(missing.button, Button::CheckAgain);
}

/// There is no settings window, so the text must name what works today: the
/// flag and the `config.toml` key.
#[test]
fn a_missing_server_names_the_flag_and_the_config_key_not_a_settings_window() {
    let missing = message(&SshFailure::ServerMissing, &target());
    assert!(missing.to_do.contains("--server-path"), "{missing:?}");
    assert!(missing.to_do.contains("server_path"), "{missing:?}");
    assert!(missing.to_do.contains("config.toml"), "{missing:?}");
    assert!(!missing.to_do.contains("Settings"), "{missing:?}");
    assert!(!missing.to_do.contains("Server path"), "{missing:?}");
}

#[test]
fn an_unknown_failure_shows_the_last_line_without_file_paths() {
    let failure = classify(
        "/home/user/.ssh/config: line 2: Bad configuration option: x\n\
         /home/user/.ssh/config: terminating, 1 bad configuration options\n",
        Some(255),
        Phase::Session,
    );
    let m = message(&failure, &target());
    assert_eq!(m.button, Button::Details);
    assert_eq!(
        m.to_do,
        "ssh said: (a file): terminating, 1 bad configuration options"
    );
    let windows = message(
        &SshFailure::Other {
            last_line: r#"Could not open "C:\Users\user\.ssh\config": denied"#.into(),
        },
        &target(),
    );
    assert!(!windows.to_do.contains("Users"), "{windows:?}");
    assert!(windows.to_do.contains("(a file)"), "{windows:?}");
}

#[test]
fn text_from_the_server_is_cleaned_and_bounded() {
    let hostile = format!("Received disconnect: \u{1b}[2J\u{7}{}", "x".repeat(500));
    let failure = classify(&hostile, Some(255), Phase::Session);
    let SshFailure::Other { last_line } = &failure else {
        panic!("{failure:?}");
    };
    assert!(!last_line.contains('\u{1b}') && !last_line.contains('\u{7}'));
    assert!(last_line.chars().count() <= 203, "{}", last_line.len());
}

#[test]
fn ssh_warnings_do_not_decide_the_class() {
    let text = "Warning: Identity file /home/user/.ssh/id_x not accessible: No such file or directory.\n\
                ssh: connect to host lab-pc port 22: Connection refused\n";
    assert_eq!(
        classify(text, Some(255), Phase::Session),
        SshFailure::Refused
    );
}

#[test]
fn windows_ssh_with_no_words_for_a_failed_connect_is_an_unreachable_machine() {
    let text = "ssh: connect to host 2001:db8::1 port 22: Unknown error\r\n";
    assert_eq!(
        classify(text, Some(255), Phase::Session),
        SshFailure::Unreachable
    );
    // Only a failed connect: an unknown error elsewhere stays unknown.
    let other = "ssh: something else: Unknown error\r\n";
    assert!(matches!(
        classify(other, Some(255), Phase::Session),
        SshFailure::Other { .. }
    ));
}

#[test]
fn crlf_line_ends_do_not_change_the_class_or_the_words() {
    let text = "Warning: Identity file C:\\k not accessible: No such file or directory.\r\n\
                ssh: connect to host lab-pc port 22: Connection refused\r\n";
    assert_eq!(
        classify(text, Some(255), Phase::Session),
        SshFailure::Refused
    );
    let other = classify("ssh: odd\r\nlast words\r\n", Some(255), Phase::Session);
    assert_eq!(
        other,
        SshFailure::Other {
            last_line: "last words".to_owned()
        }
    );
}

#[test]
fn a_changed_host_key_is_not_taken_for_an_unknown_one() {
    // ssh prints "Host key verification failed." after both.
    let text = "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\n\
                Host key verification failed.\n";
    assert_eq!(
        classify(text, Some(255), Phase::Session),
        SshFailure::HostKeyChanged
    );
}

#[test]
fn an_unusable_key_file_is_not_taken_for_a_refused_key() {
    // ssh prints "Permission denied (publickey)" after it too.
    let text = "WARNING: UNPROTECTED PRIVATE KEY FILE!\n\
                user@h: Permission denied (publickey).\n";
    assert_eq!(
        classify(text, Some(255), Phase::Session),
        SshFailure::KeyFilePermissions
    );
}

#[test]
fn only_a_remote_shell_exit_means_the_server_is_missing() {
    let text = "bash: line 1: gliff-server: command not found\n";
    assert_eq!(
        classify(text, Some(127), Phase::Session),
        SshFailure::ServerMissing
    );
    // Another exit code is the server's own business, not a missing server.
    assert!(matches!(
        classify(text, Some(1), Phase::Session),
        SshFailure::Other { .. }
    ));
    // ssh's own failures exit 255, and a killed ssh has no exit at all.
    let own = "Load key \"k\": No such file or directory\n";
    for exit in [Some(255), None] {
        assert!(
            matches!(
                classify(own, exit, Phase::Session),
                SshFailure::Other { .. }
            ),
            "{exit:?}"
        );
    }
}

#[test]
fn exit_127_with_no_text_is_a_missing_server_only_for_a_session() {
    assert_eq!(
        classify("", Some(127), Phase::Session),
        SshFailure::ServerMissing
    );
    assert!(matches!(
        classify("", Some(127), Phase::VersionCheck),
        SshFailure::Other { .. }
    ));
}

#[test]
fn the_windows_shell_saying_ssh_is_not_a_command_is_a_missing_client() {
    let text = "'ssh' is not recognized as an internal or external command,\n\
                operable program or batch file.\n";
    assert_eq!(
        classify(text, None, Phase::Session),
        SshFailure::ClientMissing
    );
}

#[test]
fn a_program_that_is_not_there_is_a_missing_client() {
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    assert_eq!(from_spawn_error(&missing), SshFailure::ClientMissing);
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(matches!(
        from_spawn_error(&denied),
        SshFailure::Other { .. }
    ));
}

#[test]
fn the_log_line_has_the_class_the_exit_and_the_words() {
    let line = log_line(
        &SshFailure::KeyRefused,
        &target(),
        Some(255),
        "ssh-stderr-3.log",
    );
    assert!(
        line.starts_with("ssh failed: key_refused (ssh exit 255): "),
        "{line}"
    );
    assert!(line.contains("lab-pc did not accept a key"), "{line}");
    assert!(
        line.ends_with("(ssh's own text: ssh-stderr-3.log)"),
        "{line}"
    );
    let killed = log_line(&SshFailure::Unreachable, &target(), None, "x.log");
    assert!(killed.contains("ssh exit none"), "{killed}");
}

/// GitHub's heading anchor: lower case, letters digits hyphens and spaces
/// kept, spaces to hyphens.
fn anchor_of(heading: &str) -> String {
    heading
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == ' ')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

#[test]
fn every_guide_link_points_at_a_heading_of_the_guide() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(GUIDE_FILE);
    let guide = std::fs::read_to_string(&path).expect("the key guide");
    let anchors: Vec<String> = guide
        .lines()
        .filter(|line| line.starts_with('#'))
        .map(anchor_of)
        .collect();
    for guide_page in [
        Guide::TrustMachine,
        Guide::SetUpKey,
        Guide::KeyFileRights,
        Guide::PickOneKey,
        Guide::ChangedIdentity,
    ] {
        assert!(
            anchors.iter().any(|a| a == guide_page.anchor()),
            "{guide_page:?} -> {} in {anchors:?}",
            guide_page.anchor()
        );
    }
}

#[test]
fn a_connection_that_never_happened_beats_a_key_warning_before_it() {
    let text = "Load key \"/home/user/.ssh/id\": bad permissions\n\
                ssh: connect to host lab-pc port 22: Connection refused\n";
    assert_eq!(
        classify(text, Some(255), Phase::Session),
        SshFailure::Refused
    );
}

#[test]
fn text_that_redirects_or_hides_other_text_is_not_shown() {
    let text = "Received disconnect: \u{202E}evil\u{200B}\u{2028}line\u{2069}\n";
    let SshFailure::Other { last_line } = classify(text, Some(255), Phase::Session) else {
        panic!("expected Other");
    };
    assert_eq!(last_line, "Received disconnect: evilline");
}
