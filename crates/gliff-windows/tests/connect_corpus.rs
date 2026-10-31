//! What a person reads when ssh fails, from real ssh output.
//!
//! Each file in `tests/ssh_corpus/<version>/` is the stderr of a real OpenSSH
//! run. The Linux directory comes from `scripts/capture-ssh-corpus`, the
//! Windows one from `scripts/capture-ssh-corpus-windows.ps1`; Windows' ssh
//! ends its lines with CRLF, as ssh's own messages do on Linux, and the files
//! keep the bytes as they were. The exit code is not in
//! the file: ssh exits 255 for its own failures, and the remote shell's code
//! (127) when it could not find the server. A new bug report adds a file and
//! a row here.

use std::collections::BTreeSet;
use std::path::Path;

use gliff_windows::connect::{Button, Phase, SshFailure, classify, message};

const LINUX: &str = "openssh-10.0p2-linux";
const WINDOWS: &str = "windows-openssh-9.5p2";
const DIRS: [&str; 2] = [LINUX, WINDOWS];

struct Case {
    dir: &'static str,
    file: &'static str,
    exit: i32,
    class: &'static str,
    /// Words the person must find in the message.
    says: &'static str,
    button: Button,
}

const fn case_in(
    dir: &'static str,
    file: &'static str,
    exit: i32,
    class: &'static str,
    says: &'static str,
    button: Button,
) -> Case {
    Case {
        dir,
        file,
        exit,
        class,
        says,
        button,
    }
}

const fn case(
    file: &'static str,
    exit: i32,
    class: &'static str,
    says: &'static str,
    button: Button,
) -> Case {
    case_in(LINUX, file, exit, class, says, button)
}

const fn win(
    file: &'static str,
    exit: i32,
    class: &'static str,
    says: &'static str,
    button: Button,
) -> Case {
    case_in(WINDOWS, file, exit, class, says, button)
}

/// Windows' ssh: only the cases that need no server. Where the wording
/// differs from Linux: a refused connection with `ConnectTimeout` set (as the
/// client does) reads "banner exchange: Connection to UNKNOWN port -1:
/// Connection refused"; "No such host is known." has a trailing space; a
/// route that does not exist reads "Unknown error"; a config file path has
/// doubled backslashes.
fn windows_cases() -> Vec<Case> {
    use Button::*;
    vec![
        win(
            "bad_config",
            255,
            "other",
            "ssh said: (a file): terminating, 7 bad configuration options",
            Details,
        ),
        win(
            "missing_key_file_then_unresolved",
            255,
            "name_not_found",
            "Can't find",
            EditAddress,
        ),
        win(
            "name_not_found_no_address",
            255,
            "name_not_found",
            "Can't find",
            EditAddress,
        ),
        win(
            "name_not_found_unknown",
            255,
            "name_not_found",
            "Can't find",
            EditAddress,
        ),
        win(
            "refused",
            255,
            "refused",
            "Nothing is listening for ssh",
            TryAgain,
        ),
        win(
            "refused_without_connect_timeout",
            255,
            "refused",
            "Nothing is listening for ssh",
            TryAgain,
        ),
        win(
            "reset",
            255,
            "other",
            "ssh said: Connection reset by",
            Details,
        ),
        win(
            "timeout",
            255,
            "unreachable",
            "did not answer in 10 seconds",
            TryAgain,
        ),
        win(
            "unreachable",
            255,
            "unreachable",
            "did not answer in 10 seconds",
            TryAgain,
        ),
    ]
}

fn cases() -> Vec<Case> {
    let mut all = linux_cases();
    all.extend(windows_cases());
    all
}

fn linux_cases() -> Vec<Case> {
    use Button::*;
    vec![
        case(
            "bad_config",
            255,
            "other",
            "ssh said: (a file): terminating, 1 bad configuration options",
            Details,
        ),
        case(
            "host_changed",
            255,
            "host_key_changed",
            "different identity",
            Close,
        ),
        case(
            "host_unknown",
            255,
            "host_unknown",
            "not known to this computer yet",
            TryAgain,
        ),
        case(
            "key_file_permissions",
            255,
            "key_file_permissions",
            "Other users can read your key file",
            TryAgain,
        ),
        case(
            "key_refused",
            255,
            "key_refused",
            "did not accept a key",
            TryAgain,
        ),
        case(
            "key_refused_passwords_on",
            255,
            "key_refused",
            "did not accept a key",
            TryAgain,
        ),
        case(
            "missing_key_file_then_refused",
            255,
            "key_refused",
            "did not accept a key",
            TryAgain,
        ),
        case(
            "missing_key_file_then_unresolved",
            255,
            "name_not_found",
            "Can't find",
            EditAddress,
        ),
        case(
            "name_not_found_no_address",
            255,
            "name_not_found",
            "Can't find",
            EditAddress,
        ),
        case(
            "name_not_found_unknown",
            255,
            "name_not_found",
            "Can't find",
            EditAddress,
        ),
        case(
            "refused",
            255,
            "refused",
            "Nothing is listening for ssh",
            TryAgain,
        ),
        case(
            "reset",
            255,
            "other",
            "ssh said: Connection reset by",
            Details,
        ),
        case(
            "server_missing_bare",
            127,
            "server_missing",
            "gliff is not installed",
            CheckAgain,
        ),
        case(
            "server_missing_dash",
            127,
            "server_missing",
            "gliff is not installed",
            CheckAgain,
        ),
        case(
            "server_missing_path",
            127,
            "server_missing",
            "gliff is not installed",
            CheckAgain,
        ),
        case(
            "timeout",
            255,
            "unreachable",
            "did not answer in 10 seconds",
            TryAgain,
        ),
        case(
            "too_many_keys",
            255,
            "too_many_keys",
            "more keys than",
            TryAgain,
        ),
        case(
            "unreachable",
            255,
            "unreachable",
            "did not answer in 10 seconds",
            TryAgain,
        ),
    ]
}

fn corpus_dir(dir: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/ssh_corpus")
        .join(dir)
}

fn read(c: &Case) -> String {
    let path = corpus_dir(c.dir).join(format!("{}.stderr", c.file));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn name(c: &Case) -> String {
    format!("{}/{}", c.dir, c.file)
}

#[test]
fn every_real_ssh_failure_is_classified_and_explained() {
    let target = "me@lab-pc".parse().expect("a target");
    for c in cases() {
        let failure = classify(&read(&c), Some(c.exit), Phase::Session);
        assert_eq!(failure.name(), c.class, "{}: {failure:?}", name(&c));
        let m = message(&failure, &target);
        assert!(m.text().contains(c.says), "{}: {}", name(&c), m.text());
        assert_eq!(m.button, c.button, "{}", name(&c));
    }
}

#[test]
fn every_corpus_file_has_a_row_above() {
    let on_disk: BTreeSet<String> = DIRS
        .iter()
        .flat_map(|dir| {
            std::fs::read_dir(corpus_dir(dir))
                .expect("the corpus")
                .map(|e| e.expect("entry").path())
                .filter(|p| p.extension().is_some_and(|x| x == "stderr"))
                .map(move |p| format!("{dir}/{}", p.file_stem().expect("stem").to_string_lossy()))
        })
        .collect();
    let in_table: BTreeSet<String> = cases().iter().map(name).collect();
    assert_eq!(on_disk, in_table);
    let on_disk_dirs: BTreeSet<String> = std::fs::read_dir(corpus_dir(""))
        .expect("the corpus")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    let known: BTreeSet<String> = DIRS.iter().map(|d| (*d).to_owned()).collect();
    assert_eq!(on_disk_dirs, known, "a corpus directory with no table");
}

#[test]
fn the_message_never_repeats_ssh_text_beyond_the_last_line() {
    // The raw text goes to the log. The message may quote only one line, and
    // never a file path or the address ssh printed.
    let target = "me@lab-pc".parse().expect("a target");
    for c in cases() {
        let text = read(&c);
        let m = message(&classify(&text, Some(c.exit), Phase::Session), &target).text();
        assert!(!m.contains("/home/"), "{}: {m}", name(&c));
        assert!(!m.contains(":\\"), "{}: {m}", name(&c));
        assert!(!m.contains(['\n', '\r']), "{}: {m}", name(&c));
    }
}

#[test]
fn a_session_without_text_but_with_exit_127_is_a_missing_server() {
    assert_eq!(
        classify("", Some(127), Phase::Session),
        SshFailure::ServerMissing
    );
}

#[test]
fn the_same_text_is_classified_the_same_in_the_version_check() {
    for c in cases() {
        let session = classify(&read(&c), Some(c.exit), Phase::Session);
        let check = classify(&read(&c), Some(c.exit), Phase::VersionCheck);
        assert_eq!(session, check, "{}", name(&c));
    }
}

#[test]
fn every_line_of_the_windows_files_ends_with_crlf() {
    // The corpus is byte for byte what ssh printed. ssh's own messages end
    // with CRLF on Linux too (a remote shell's lines end with LF, so some
    // Linux files mix the two); a tool that rewrote line ends would make the
    // Windows files test nothing.
    for c in cases().into_iter().filter(|c| c.dir == WINDOWS) {
        let text = read(&c);
        let (crlf, lf) = (text.matches("\r\n").count(), text.matches('\n').count());
        assert!(
            lf > 0 && crlf == lf,
            "{}: {crlf} CRLF of {lf} lines",
            name(&c)
        );
    }
}

#[test]
fn line_ends_do_not_change_what_a_person_reads() {
    // The same text with every line end CRLF, and with every one LF, reads the same.
    let target = "me@lab-pc".parse().expect("a target");
    for c in cases() {
        let text = read(&c);
        let lf = text.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let a = classify(&lf, Some(c.exit), Phase::Session);
        let b = classify(&crlf, Some(c.exit), Phase::Session);
        assert_eq!(a, b, "{}", name(&c));
        assert_eq!(message(&a, &target), message(&b, &target), "{}", name(&c));
    }
}
