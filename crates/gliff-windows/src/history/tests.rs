//! The recent hosts: the list's rules, then the file's.

use std::fs;

use super::HistoryFile;
use super::HostTarget;
use super::HostTargetError;
use super::MAX_RECENT;
use super::RecentHosts;
use crate::test_dir::TestDir;

fn host(text: &str) -> HostTarget {
    text.parse().expect("a host")
}

fn names(hosts: &RecentHosts) -> Vec<&str> {
    hosts.hosts().iter().map(HostTarget::as_str).collect()
}

#[test]
fn a_target_is_checked_where_it_enters() {
    assert_eq!("user@host".parse::<HostTarget>(), Ok(host("user@host")));
    assert_eq!(
        "alias".parse::<HostTarget>(),
        Ok(host("alias")),
        "an ssh alias"
    );
    let cases = [
        ("", HostTargetError::Empty),
        ("-oProxyCommand=x", HostTargetError::LeadingDash),
        ("user@ho st", HostTargetError::Unprintable),
        (" user@host", HostTargetError::Unprintable),
        ("user@host\n", HostTargetError::Unprintable),
        ("user@host\u{7}", HostTargetError::Unprintable),
        (&"a".repeat(254), HostTargetError::TooLong),
    ];
    for (text, why) in cases {
        assert_eq!(text.parse::<HostTarget>(), Err(why), "{text:?}");
    }
    let refused = [
        ("user@", HostTargetError::EmptyPart),
        ("@host", HostTargetError::EmptyPart),
        ("@", HostTargetError::EmptyPart),
        ("user@-oProxyCommand=x", HostTargetError::PartLeadingDash),
        ("a@-x", HostTargetError::PartLeadingDash),
        ("host:22@x", HostTargetError::BadCharacter(':')),
        ("user@host;rm", HostTargetError::BadCharacter(';')),
        ("u@h`id`", HostTargetError::BadCharacter('`')),
        ("\"x\"", HostTargetError::BadCharacter('"')),
        ("user@host\\x", HostTargetError::BadCharacter('\\')),
        (
            "user@host\u{200b}",
            HostTargetError::BadCharacter('\u{200b}'),
        ),
        ("user@[::1]", HostTargetError::BadAddress),
        ("user@[x]", HostTargetError::BadCharacter('[')),
        ("us$er@host", HostTargetError::BadCharacter('$')),
        ("host:22", HostTargetError::BadAddress),
        ("user@host:22", HostTargetError::BadAddress),
        ("user@1:2", HostTargetError::BadAddress),
        ("user@fe80::1::2", HostTargetError::BadAddress),
        ("user@%APPDATA%", HostTargetError::BadCharacter('%')),
        ("a%", HostTargetError::BadCharacter('%')),
        ("a%b", HostTargetError::BadCharacter('%')),
        ("user@fe80::1%", HostTargetError::BadZone),
        ("user@fe80::1%a%b", HostTargetError::BadZone),
        ("user@fe80::1%a/b", HostTargetError::BadZone),
        ("user@fe80::1%e:0", HostTargetError::BadZone),
        (":", HostTargetError::BadAddress),
        ("%", HostTargetError::BadCharacter('%')),
        ("a=b", HostTargetError::BadCharacter('=')),
        ("x/y", HostTargetError::BadCharacter('/')),
    ];
    for (text, why) in refused {
        assert_eq!(text.parse::<HostTarget>(), Err(why), "{text:?}");
    }
    let windows_destinations = [
        "user@domain.com@host",
        "DOMAIN\\user@host",
        "M\u{fc}ller@host",
        "me@m\u{fc}nchen.de",
        "m\u{fc}nchen.de",
        "\u{e9}t\u{e9}-alias",
        "DOMAIN\\first.last@fe80::1%eth0",
        "a@@host",
    ];
    for text in windows_destinations {
        assert_eq!(
            text.parse::<HostTarget>().map(|t| t.as_str().to_owned()),
            Ok(text.to_owned()),
            "{text:?} is a legitimate destination"
        );
    }
    for text in [
        "me@host.example",
        "me.name+tag@host-1.example",
        "user@203.0.113.7",
        "me@fe80::1%eth0",
        "me@fe80::1%Local_Area-1.2",
        "::1",
        "::",
        "user@::1",
        "host.",
        "u+x@h",
        "HOST",
        "xn--mnchen-3ya.de",
        "me@2001:db8::7",
        "ssh-alias",
        "me@a",
    ] {
        assert!(
            text.parse::<HostTarget>().is_ok(),
            "{text:?} stays accepted"
        );
    }
    assert!(
        "a".repeat(253).parse::<HostTarget>().is_ok(),
        "253 bytes is the limit"
    );
}

#[test]
fn the_host_part_is_compared_without_case_and_the_user_part_with_it() {
    assert!(host("me@Host.Example").same(&host("me@host.example")));
    assert!(!host("me@host").same(&host("Me@host")), "user names differ");
    assert!(!host("me@host").same(&host("me@other")));
    assert!(host("me@FE80::1%eth0").same(&host("me@fe80::1%eth0")));
    assert!(
        !host("me@fe80::1%eth0").same(&host("me@fe80::1%ETH0")),
        "a zone is compared as written"
    );
    assert!(!host("me@fe80::1%eth0").same(&host("me@fe80::1")));
    assert!(
        !host("host").same(&host("me@host")),
        "no user is not a user"
    );
}

#[test]
fn recording_puts_a_host_first_and_a_repeat_moves_it() {
    let mut hosts = RecentHosts::default();
    hosts.record(host("a@one"));
    hosts.record(host("a@two"));
    hosts.record(host("a@three"));
    assert_eq!(names(&hosts), ["a@three", "a@two", "a@one"]);
    hosts.record(host("a@one"));
    assert_eq!(
        names(&hosts),
        ["a@one", "a@three", "a@two"],
        "moved, not added"
    );
}

#[test]
fn a_repeat_in_another_case_moves_the_host_and_keeps_the_newest_spelling() {
    let mut hosts = RecentHosts::default();
    hosts.record(host("a@Host.Example"));
    hosts.record(host("a@other"));
    hosts.record(host("a@host.example"));
    assert_eq!(names(&hosts), ["a@host.example", "a@other"]);
}

#[test]
fn the_list_keeps_the_ten_most_recent() {
    let mut hosts = RecentHosts::default();
    for n in 0..(MAX_RECENT + 3) {
        hosts.record(host(&format!("u@h{n}")));
    }
    assert_eq!(hosts.hosts().len(), MAX_RECENT);
    assert_eq!(hosts.hosts()[0].as_str(), "u@h12", "the newest is first");
    assert_eq!(
        hosts.hosts()[MAX_RECENT - 1].as_str(),
        "u@h3",
        "the oldest kept"
    );
}

#[test]
fn forgetting_removes_one_host_and_clearing_removes_all() {
    let mut hosts = RecentHosts::default();
    hosts.record(host("a@one"));
    hosts.record(host("a@two"));
    assert!(hosts.forget(&host("a@ONE")));
    assert!(!hosts.forget(&host("a@one")), "already gone");
    assert_eq!(names(&hosts), ["a@two"]);
    hosts.clear();
    assert!(hosts.hosts().is_empty());
}

fn file_in(dir: &TestDir) -> std::path::PathBuf {
    dir.path().join("recent-hosts.toml")
}

#[test]
fn the_list_is_in_order_after_a_reload() {
    let dir = TestDir::new();
    let (mut file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(notes.is_empty());
    file.record(host("a@one")).expect("saved");
    file.record(host("a@two")).expect("saved");
    file.record(host("a@one")).expect("saved");
    let (again, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(names(again.hosts()), ["a@one", "a@two"]);
}

#[test]
fn a_missing_file_is_an_empty_list_and_is_not_created_by_loading() {
    let dir = TestDir::new();
    let (file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(file.hosts().hosts().is_empty());
    assert!(notes.is_empty());
    assert!(!file_in(&dir).exists());
}

#[test]
fn hostile_lines_are_dropped_on_load_and_the_rest_kept_in_order() {
    let dir = TestDir::new();
    let long = "a".repeat(300);
    fs::write(
        file_in(&dir),
        format!(
            "hosts = [\"a@one\", \"\", \"-oProxyCommand=x\", \"a b\", \"a@\\u0007x\", \"{long}\", \
             7, \"A@one\", \"a@two\", \"a@ONE\"]\n"
        ),
    )
    .expect("write");
    let (file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(names(file.hosts()), ["a@one", "A@one", "a@two"]);
    assert_eq!(notes.len(), 1, "one note counts them: {notes:?}");
    assert!(notes[0].contains("dropped 7"), "{notes:?}");
}

#[test]
fn a_file_longer_than_the_cap_loads_only_the_newest_ten() {
    let dir = TestDir::new();
    let lines: Vec<String> = (0..15).map(|n| format!("\"u@h{n}\"")).collect();
    fs::write(file_in(&dir), format!("hosts = [{}]\n", lines.join(", "))).expect("write");
    let (file, _) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(file.hosts().hosts().len(), MAX_RECENT);
    assert_eq!(file.hosts().hosts()[0].as_str(), "u@h0");
}

#[test]
fn a_damaged_file_gives_an_empty_list_and_is_kept_as_broken_at_the_next_change() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = = [").expect("write");
    let (mut file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(file.hosts().hosts().is_empty());
    assert_eq!(notes.len(), 1);
    assert!(file_in(&dir).exists(), "loading changes nothing on disk");
    file.record(host("a@one")).expect("saved");
    let kept = dir.path().join("recent-hosts.toml.broken");
    assert_eq!(fs::read_to_string(kept).expect("kept"), "hosts = = [");
    let (again, _) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(names(again.hosts()), ["a@one"]);
}

#[test]
fn a_hosts_value_that_is_not_a_list_is_damage() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = \"a@one\"\n").expect("write");
    let (mut file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(file.hosts().hosts().is_empty());
    assert!(notes[0].contains("not a list"), "{notes:?}");
    file.record(host("a@two")).expect("saved");
    assert!(dir.path().join("recent-hosts.toml.broken").exists());
}

#[test]
fn a_save_leaves_no_temporary_file() {
    let dir = TestDir::new();
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    let names: Vec<String> = fs::read_dir(dir.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["recent-hosts.toml"]);
}

#[test]
fn with_remembering_off_nothing_is_recorded_and_loading_ignores_the_file() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = [\"a@one\"]\n").expect("write");
    let (mut file, notes) = HistoryFile::load(&file_in(&dir), false);
    assert!(file.hosts().hosts().is_empty());
    assert!(notes.is_empty());
    file.record(host("a@two")).expect("a no-op");
    assert!(file.hosts().hosts().is_empty());
    assert_eq!(
        fs::read_to_string(file_in(&dir)).expect("untouched"),
        "hosts = [\"a@one\"]\n",
        "loading and recording with the setting off do not touch the file"
    );
}

#[test]
fn turning_remembering_off_forgets_the_list_and_deletes_the_file() {
    let dir = TestDir::new();
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    file.set_remember(false).expect("deleted");
    assert!(!file_in(&dir).exists());
    assert!(file.hosts().hosts().is_empty());
    file.record(host("a@two")).expect("a no-op");
    assert!(!file_in(&dir).exists(), "still nothing recorded");
    file.set_remember(false).expect("already gone is fine");
    file.set_remember(true).expect("on again");
    file.record(host("a@three")).expect("saved");
    assert!(file_in(&dir).exists());
}

#[test]
fn forgetting_and_clearing_are_saved() {
    let dir = TestDir::new();
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    file.record(host("a@two")).expect("saved");
    file.forget(&host("a@one")).expect("saved");
    let (again, _) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(names(again.hosts()), ["a@two"]);
    file.clear().expect("saved");
    let (again, _) = HistoryFile::load(&file_in(&dir), true);
    assert!(again.hosts().hosts().is_empty());
}

#[test]
fn the_history_never_touches_the_config_file() {
    let dir = TestDir::new();
    let config = dir.path().join("config.toml");
    fs::write(&config, "machines = [\"a@one\"]\n").expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    file.forget(&host("a@one")).expect("saved");
    file.clear().expect("saved");
    file.set_remember(false).expect("deleted");
    assert_eq!(
        fs::read_to_string(&config).expect("read"),
        "machines = [\"a@one\"]\n"
    );
}

#[test]
fn a_file_that_cannot_be_read_is_left_alone_and_not_called_damaged() {
    let dir = TestDir::new();
    // A folder in place of the file makes the read fail with an I/O error.
    fs::create_dir(file_in(&dir)).expect("folder");
    let (mut file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(notes[0].contains("cannot be read"), "{notes:?}");
    let err = file.record(host("a@one")).expect_err("not saved");
    assert!(err.to_string().contains("left as it is"), "{err}");
    assert!(file_in(&dir).is_dir(), "left where it was");
    assert!(!dir.path().join("recent-hosts.toml.broken").exists());
}

#[test]
fn a_file_fixed_before_the_save_is_kept_as_a_copy_not_lost() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = = [").expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    let fixed = "hosts = [\"a@old\"]\n";
    fs::write(file_in(&dir), fixed).expect("fix");
    file.record(host("a@one")).expect("saved");
    assert_eq!(
        fs::read_to_string(dir.path().join("recent-hosts.toml.replaced")).expect("kept"),
        fixed,
        "the hand fix is in a copy, not overwritten"
    );
    assert!(!dir.path().join("recent-hosts.toml.broken").exists());
}

#[test]
fn a_file_that_became_damaged_after_loading_is_kept_as_broken() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = [\"a@old\"]\n").expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    fs::write(file_in(&dir), "hosts = = [").expect("damage");
    file.record(host("a@one")).expect("saved");
    assert_eq!(
        fs::read_to_string(dir.path().join("recent-hosts.toml.broken")).expect("kept"),
        "hosts = = ["
    );
}

#[test]
fn another_key_in_the_file_is_in_the_copy_a_save_keeps() {
    let dir = TestDir::new();
    let text = "note = \"mine\"\nhosts = [\"a@old\"]\n";
    fs::write(file_in(&dir), text).expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    assert_eq!(
        fs::read_to_string(dir.path().join("recent-hosts.toml.replaced")).expect("kept"),
        text
    );
    let (again, _) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(names(again.hosts()), ["a@one", "a@old"]);
}

#[test]
fn turning_remembering_off_keeps_a_damaged_file_as_a_copy() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = = [").expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.set_remember(false).expect("deleted");
    assert!(!file_in(&dir).exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("recent-hosts.toml.broken")).expect("kept"),
        "hosts = = [",
        "the text a person may want back is not deleted"
    );
}

#[test]
fn turning_remembering_off_does_not_delete_a_file_that_cannot_be_read() {
    let dir = TestDir::new();
    fs::create_dir(file_in(&dir)).expect("folder");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    let err = file.set_remember(false).expect_err("refused");
    assert!(err.to_string().contains("left as it is"), "{err}");
    assert!(file_in(&dir).is_dir(), "left where it was");
}

#[test]
fn turning_remembering_off_keeps_a_file_that_is_not_what_was_loaded() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = [\"a@old\"]\n").expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    let other = "hosts = [\"a@old\", \"a@other\"]\n";
    fs::write(file_in(&dir), other).expect("another client");
    file.set_remember(false).expect("deleted");
    assert_eq!(
        fs::read_to_string(dir.path().join("recent-hosts.toml.replaced")).expect("kept"),
        other
    );
}

#[test]
fn a_forget_that_failed_to_save_writes_when_it_is_asked_again() {
    let dir = TestDir::new();
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    file.record(host("a@two")).expect("saved");
    // A folder in the way makes the save fail; the list in memory changed.
    fs::remove_file(file_in(&dir)).expect("remove");
    fs::create_dir(file_in(&dir)).expect("blocker");
    file.forget(&host("a@one")).expect_err("cannot save");
    fs::remove_dir(file_in(&dir)).expect("the blocker goes");
    file.forget(&host("a@one")).expect("the retry writes");
    let (again, _) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(
        names(again.hosts()),
        ["a@two"],
        "the forget reached the disk"
    );
}

#[test]
fn a_save_that_fails_leaves_the_state_for_the_next_one_to_compare_with() {
    let dir = TestDir::new();
    fs::write(file_in(&dir), "hosts = = [").expect("write");
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    fs::remove_file(file_in(&dir)).expect("remove");
    fs::create_dir(file_in(&dir)).expect("blocker");
    file.record(host("a@one")).expect_err("cannot save");
    fs::remove_dir(file_in(&dir)).expect("the blocker goes");
    fs::write(file_in(&dir), "hosts = = [").expect("damaged again");
    file.record(host("a@two")).expect("saved");
    assert!(
        dir.path().join("recent-hosts.toml.broken").exists(),
        "still seen as damaged, so still kept"
    );
}

#[test]
fn a_hundred_copies_is_a_clear_error_and_the_file_stays() {
    let dir = TestDir::new();
    let path = file_in(&dir);
    let mut failed = None;
    for i in 0..105 {
        fs::write(&path, format!("junk{i} [[\n")).expect("damaged");
        let (mut file, _) = HistoryFile::load(&path, true);
        if let Err(e) = file.record(host("a@one")) {
            failed = Some((i, e.to_string()));
            break;
        }
    }
    let (at, message) = failed.expect("the cap is reached");
    assert_eq!(at, 100, "the first 100 saves each keep a copy");
    assert!(message.contains("100 copies"), "{message}");
    assert_eq!(
        fs::read_to_string(&path).expect("kept"),
        "junk100 [[\n",
        "the file the save could not keep is left as it is"
    );
}

#[test]
fn the_user_is_everything_before_the_last_at() {
    let target = host("user@domain.com@Host");
    assert_eq!(target.split(), ("user@domain.com@", "Host"));
    assert!(
        target.same(&host("user@domain.com@host")),
        "host without case"
    );
    assert!(!target.same(&host("user@host")), "another user");
}

#[test]
fn a_url_is_refused_with_a_hint_to_drop_the_scheme() {
    for text in ["ssh://me@host", "ssh://host", "SSH://me@host:22", "x://y"] {
        let err = text.parse::<HostTarget>().expect_err(text);
        assert_eq!(err, HostTargetError::UrlScheme, "{text:?}");
        assert!(err.to_string().contains("leave out the scheme"), "{err}");
    }
}

#[test]
fn the_targets_the_spec_names_are_accepted_or_refused_as_it_says() {
    for refused in [
        "a b",
        "-oProxyCommand=x",
        "$(x)",
        "me@$(x)",
        "`x`",
        "me`x`@host",
        "host:22",
        "me@host:22",
        "ssh://me@host",
        "me;x@host",
        "me|x@host",
        "me&x@host",
        "me<x@host",
        "me>x@host",
        "me'x@host",
        "me\"x@host",
        "me\tx@host",
        "me\u{202e}x@host",
    ] {
        assert!(refused.parse::<HostTarget>().is_err(), "{refused:?}");
    }
}

#[test]
fn a_hand_written_list_is_kept_once_before_its_comments_are_lost() {
    let dir = TestDir::new();
    let original = "# the ones I use\nhosts = [\n  \"a@one\", # work\n  \"a@two\",\n]\n";
    fs::write(file_in(&dir), original).expect("write");
    let (mut file, notes) = HistoryFile::load(&file_in(&dir), true);
    assert!(notes.is_empty(), "{notes:?}");
    file.record(host("a@three")).expect("saved");
    file.record(host("a@four")).expect("saved");
    assert_eq!(
        fs::read_to_string(dir.path().join("recent-hosts.toml.replaced")).expect("kept"),
        original
    );
    assert!(
        !dir.path().join("recent-hosts.toml.replaced.1").exists(),
        "once"
    );
    let (again, _) = HistoryFile::load(&file_in(&dir), true);
    assert_eq!(
        names(again.hosts()),
        ["a@four", "a@three", "a@one", "a@two"]
    );
}

#[test]
fn a_list_the_client_wrote_is_replaced_without_a_copy() {
    let dir = TestDir::new();
    let (mut file, _) = HistoryFile::load(&file_in(&dir), true);
    file.record(host("a@one")).expect("saved");
    let (mut again, _) = HistoryFile::load(&file_in(&dir), true);
    again.record(host("a@two")).expect("saved");
    assert!(!dir.path().join("recent-hosts.toml.replaced").exists());
}
