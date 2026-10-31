//! The config folder choice and the file rules, against a temporary folder.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use super::ConfigDir;
use super::FileGuard;
use super::Read;
use super::keep_copy;
use super::read_file;
use super::rename_with_retry;
use super::write_atomic_checked;
use crate::test_dir::TestDir;

fn write_atomic(path: &std::path::Path, text: &str) -> Result<(), super::FileError> {
    write_atomic_checked(path, text, || Ok(()))
}

fn temp() -> TestDir {
    TestDir::new()
}

fn read_table(path: &std::path::Path) -> Read {
    read_file(path).0
}

fn path(text: &str) -> Option<PathBuf> {
    Some(PathBuf::from(text))
}

#[test]
fn the_flag_beats_the_environment_beats_app_data() {
    let dir = ConfigDir::resolve(
        path("/flag"),
        Some(OsString::from("/env")),
        path("/appdata"),
    )
    .expect("a folder");
    assert_eq!(dir.path(), PathBuf::from("/flag"));
    assert!(!dir.is_default());

    let dir =
        ConfigDir::resolve(None, Some(OsString::from("/env")), path("/appdata")).expect("a folder");
    assert_eq!(dir.path(), PathBuf::from("/env"));
    assert!(!dir.is_default());

    let dir = ConfigDir::resolve(None, None, path("/appdata")).expect("a folder");
    assert_eq!(dir.path(), PathBuf::from("/appdata/gliff-windows"));
    assert!(dir.is_default());
}

#[test]
fn an_empty_environment_value_counts_as_unset() {
    let dir = ConfigDir::resolve(None, Some(OsString::new()), path("/appdata")).expect("a folder");
    assert_eq!(dir.path(), PathBuf::from("/appdata/gliff-windows"));
}

#[test]
fn a_missing_file_a_good_file_and_a_damaged_file_read_differently() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    assert!(matches!(read_table(&file), Read::Missing));

    fs::write(&file, "x = 1\n").expect("write");
    let Read::Parsed(table) = read_table(&file) else {
        panic!("a good file parses");
    };
    assert_eq!(table.get("x").and_then(toml::Value::as_integer), Some(1));

    fs::write(&file, "x = = 1").expect("write");
    assert!(matches!(read_table(&file), Read::Damaged(_)));
    fs::write(&file, [0xff, 0xfe, 0x00]).expect("write");
    assert!(matches!(read_table(&file), Read::Damaged(_)), "not UTF-8");
}

#[test]
fn a_huge_file_is_damaged_not_read() {
    let dir = temp();
    let file = dir.path().join("big.toml");
    fs::write(&file, vec![b'#'; (1 << 20) + 1]).expect("write");
    assert!(matches!(read_table(&file), Read::Damaged(_)));
    fs::write(&file, vec![b'#'; 1 << 20]).expect("write");
    assert!(
        matches!(read_table(&file), Read::Parsed(_)),
        "the limit itself is fine"
    );
}

#[test]
fn an_atomic_write_replaces_the_file_and_leaves_no_temporary() {
    let dir = temp();
    let file = dir.path().join("sub").join("a.toml");
    write_atomic(&file, "x = 1\n").expect("creates the folder and the file");
    write_atomic(&file, "x = 2\n").expect("replaces it");
    assert_eq!(fs::read_to_string(&file).expect("read"), "x = 2\n");
    let names: Vec<_> = fs::read_dir(file.parent().expect("parent"))
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, [OsString::from("a.toml")], "no .tmp is left");
}

#[test]
fn a_failed_write_keeps_the_old_file_and_leaves_no_temporary() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    write_atomic(&file, "x = 1\n").expect("write");
    // A folder where the file must go makes the rename fail.
    let blocked = dir.path().join("b.toml");
    fs::create_dir(&blocked).expect("blocker");
    let err = write_atomic(&blocked, "x = 2\n").expect_err("cannot replace a folder");
    assert!(err.to_string().contains("b.toml"), "names the file: {err}");
    assert_eq!(fs::read_to_string(&file).expect("read"), "x = 1\n");
    let mut names: Vec<_> = fs::read_dir(dir.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    names.sort();
    assert_eq!(names, [OsString::from("a.toml"), OsString::from("b.toml")]);
}

#[test]
fn two_writes_to_one_file_use_different_temporary_names() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    let first = super::temporary_name(&file);
    let second = super::temporary_name(&file);
    assert_ne!(first, second);
    for name in [first, second] {
        let name = name
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with("a.toml."), "{name}");
        assert!(name.contains(&std::process::id().to_string()), "{name}");
        assert!(name.ends_with(".tmp"), "{name}");
    }
}

#[test]
fn a_rename_that_fails_for_a_moment_is_tried_again() {
    let mut calls = 0;
    let result = rename_with_retry(
        std::path::Path::new("a"),
        std::path::Path::new("b"),
        |_, _| {
            calls += 1;
            if calls < 3 {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                Ok(())
            }
        },
        5,
        std::time::Duration::ZERO,
    );
    assert!(result.is_ok());
    assert_eq!(calls, 3);
}

#[test]
fn a_rename_that_keeps_failing_gives_up_with_the_last_error() {
    let mut calls = 0;
    let result = rename_with_retry(
        std::path::Path::new("a"),
        std::path::Path::new("b"),
        |_, _| {
            calls += 1;
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("locked {calls}"),
            ))
        },
        5,
        std::time::Duration::ZERO,
    );
    assert_eq!(calls, 5);
    assert_eq!(result.expect_err("gave up").to_string(), "locked 5");
}

#[test]
fn an_error_that_will_not_go_away_is_not_tried_again() {
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::StorageFull,
        std::io::ErrorKind::IsADirectory,
        std::io::ErrorKind::Other,
    ] {
        let mut calls = 0;
        let result = rename_with_retry(
            std::path::Path::new("a"),
            std::path::Path::new("b"),
            |_, _| {
                calls += 1;
                Err(std::io::Error::from(kind))
            },
            5,
            // A pause this long would make the test fail by timing out.
            std::time::Duration::from_secs(3600),
        );
        assert_eq!(calls, 1, "{kind:?} was tried again");
        assert_eq!(result.expect_err("failed").kind(), kind);
    }
}

#[test]
fn only_a_lock_or_a_permission_counts_as_transient() {
    let error = |kind| super::FileError::new(std::path::Path::new("f"), std::io::Error::from(kind));
    assert!(error(std::io::ErrorKind::PermissionDenied).is_transient());
    assert!(error(std::io::ErrorKind::WouldBlock).is_transient());
    assert!(!error(std::io::ErrorKind::NotFound).is_transient());
    assert!(!error(std::io::ErrorKind::InvalidData).is_transient());
    assert!(!error(std::io::ErrorKind::FileTooLarge).is_transient());
    assert!(!error(std::io::ErrorKind::AlreadyExists).is_transient());
}

#[test]
fn a_text_that_would_not_load_back_is_refused_before_anything_is_written() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "x = 1\n").expect("write");
    let text = format!("# {}\n", "x".repeat(1 << 20));
    let err = write_atomic(&file, &text).expect_err("too large");
    assert!(!err.is_transient(), "{err}");
    assert!(err.to_string().contains("not saved"), "{err}");
    assert_eq!(fs::read_to_string(&file).expect("read"), "x = 1\n");
    assert_eq!(read_names(dir.path()), ["a.toml"], "no temporary either");
}

fn read_names(path: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(path)
        .expect("a folder")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_copy_keeps_the_text_and_leaves_the_file() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "hand = = edit").expect("write");
    let kept = keep_copy(&file, ".broken")
        .expect("copied")
        .expect("a copy");
    assert_eq!(kept, dir.path().join("a.toml.broken"));
    assert_eq!(fs::read_to_string(kept).expect("read"), "hand = = edit");
    assert_eq!(
        fs::read_to_string(&file).expect("read"),
        "hand = = edit",
        "the original stays until the save replaces it"
    );
}

#[test]
fn an_earlier_copy_is_never_replaced() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    for (text, name) in [
        ("first", "a.toml.broken"),
        ("second", "a.toml.broken.1"),
        ("third", "a.toml.broken.2"),
    ] {
        fs::write(&file, text).expect("write");
        let kept = keep_copy(&file, ".broken")
            .expect("copied")
            .expect("a copy");
        assert_eq!(kept, dir.path().join(name));
    }
    for (name, text) in [
        ("a.toml.broken", "first"),
        ("a.toml.broken.1", "second"),
        ("a.toml.broken.2", "third"),
    ] {
        assert_eq!(
            fs::read_to_string(dir.path().join(name)).expect("read"),
            text
        );
    }
}

#[test]
fn a_file_that_is_already_gone_needs_no_copy() {
    let dir = temp();
    assert_eq!(
        keep_copy(&dir.path().join("a.toml"), ".broken").expect("fine"),
        None
    );
}

#[test]
fn when_every_name_is_taken_the_error_says_so_and_nothing_is_lost() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "mine").expect("write");
    fs::write(dir.path().join("a.toml.broken"), "0").expect("write");
    for n in 1..100 {
        fs::write(dir.path().join(format!("a.toml.broken.{n}")), "n").expect("write");
    }
    let err = keep_copy(&file, ".broken").expect_err("no name is free");
    assert!(err.to_string().contains("100 copies"), "{err}");
    assert!(!err.is_transient(), "{err}");
    assert_eq!(fs::read_to_string(&file).expect("read"), "mine");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.toml.broken.99")).expect("read"),
        "n",
        "the last name was not replaced"
    );
}

#[test]
fn a_name_taken_by_a_folder_is_skipped() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "mine").expect("write");
    fs::create_dir(dir.path().join("a.toml.broken")).expect("blocker");
    let kept = keep_copy(&file, ".broken")
        .expect("goes to .broken.1")
        .expect("a copy");
    assert_eq!(kept, dir.path().join("a.toml.broken.1"));
    assert_eq!(fs::read_to_string(kept).expect("read"), "mine");
}

#[cfg(unix)]
#[test]
fn a_name_taken_by_a_dangling_link_is_skipped_not_followed() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "mine").expect("write");
    let outside = dir.path().join("elsewhere");
    std::os::unix::fs::symlink(&outside, dir.path().join("a.toml.broken")).expect("link");
    let kept = keep_copy(&file, ".broken")
        .expect("copied")
        .expect("a copy");
    assert_eq!(kept, dir.path().join("a.toml.broken.1"));
    assert!(!outside.exists(), "the link was not written through");
}

#[cfg(unix)]
#[test]
fn an_error_other_than_taken_stops_the_copy_instead_of_trying_the_next_name() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).expect("folder");
    let file = sub.join("a.toml");
    fs::write(&file, "mine").expect("write");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o500)).expect("read-only folder");
    let result = keep_copy(&file, ".broken");
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o700)).expect("restore");
    if fs::write(sub.join("probe"), "x").is_ok() {
        return; // running as a user the mode does not bind: nothing to prove
    }
    let err = result.expect_err("no write access is an error, not a free name");
    assert!(err.is_transient(), "{err}");
    assert_eq!(read_names(&sub), ["a.toml"]);
}

#[test]
fn a_lock_code_from_windows_is_transient_even_after_the_message_is_wrapped() {
    for code in [32, 33, 303, 1224] {
        let original = std::io::Error::from_raw_os_error(code);
        let wrapped = super::FileError::unreadable_on(true, std::path::Path::new("f"), &original);
        assert!(wrapped.is_transient(), "code {code}: {wrapped}");
        assert!(
            wrapped.to_string().contains("left as it is"),
            "code {code}: {wrapped}"
        );
        let elsewhere =
            super::FileError::unreadable_on(false, std::path::Path::new("f"), &original);
        assert!(
            !elsewhere.is_transient(),
            "{code} means something else off Windows"
        );
    }
    let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
    let wrapped = super::FileError::unreadable_on(true, std::path::Path::new("f"), &gone);
    assert!(!wrapped.is_transient(), "{wrapped}");
}

#[test]
fn a_permission_is_tried_again_by_the_rename_but_is_not_transient_after_it() {
    let mut calls = 0;
    let result = rename_with_retry(
        std::path::Path::new("a"),
        std::path::Path::new("b"),
        |_, _| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        },
        3,
        std::time::Duration::ZERO,
    );
    assert_eq!(calls, 3, "tried again while it might clear");
    let error = result.expect_err("still refused");
    let failed = super::write_failure(std::path::Path::new("f"), error);
    assert!(!failed.is_transient(), "{failed}");
    let busy = super::write_failure(
        std::path::Path::new("f"),
        std::io::Error::from(std::io::ErrorKind::WouldBlock),
    );
    assert!(busy.is_transient(), "a lock still is: {busy}");
}

#[cfg(unix)]
#[test]
fn a_save_into_a_read_only_folder_is_not_transient() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "x = 1\n").expect("write");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).expect("lock");
    let result = write_atomic(&file, "x = 2\n");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("unlock");
    match result {
        Ok(()) => {} // a user the mode does not bind
        Err(err) => assert!(!err.is_transient(), "a caller would loop: {err}"),
    }
}

#[test]
fn a_guard_that_fails_five_times_keeps_one_copy() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    let mut guard = FileGuard::unread(&file);
    for _ in 0..5 {
        fs::write(&file, "hand = = edit").expect("damaged text");
        // Between the look and the rename, a folder takes the name: the
        // rename fails.
        let result = guard.write_between(
            "x = 1\n",
            |_| true,
            || {
                fs::remove_file(&file).expect("gone");
                fs::create_dir(&file).expect("a folder");
            },
        );
        assert!(result.is_err());
        fs::remove_dir(&file).expect("clear");
    }
    assert_eq!(
        read_names(dir.path()),
        ["a.toml.broken"],
        "not .broken.1 to .broken.4"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.toml.broken")).expect("read"),
        "hand = = edit"
    );
}

#[test]
fn a_changed_text_gets_its_own_copy_even_when_an_earlier_one_exists() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    let mut guard = FileGuard::unread(&file);
    for text in ["one = = 1", "two = = 2"] {
        fs::write(&file, text).expect("damaged text");
        let result = guard.write_between(
            "x = 1\n",
            |_| true,
            || {
                fs::remove_file(&file).expect("gone");
                fs::create_dir(&file).expect("a folder");
            },
        );
        assert!(result.is_err());
        fs::remove_dir(&file).expect("clear");
    }
    assert_eq!(read_names(dir.path()), ["a.toml.broken", "a.toml.broken.1"]);
}

#[test]
fn a_write_between_the_look_and_the_rename_is_kept() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "x = 1\n").expect("write");
    let (read, mut guard) = read_file(&file);
    assert!(matches!(read, Read::Parsed(_)));
    guard
        .write_between(
            "x = 3\n",
            |_| true,
            || {
                fs::write(&file, "x = 2 # someone else\n").expect("another client writes");
            },
        )
        .expect("saved");
    assert_eq!(fs::read_to_string(&file).expect("read"), "x = 3\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.toml.replaced")).expect("kept"),
        "x = 2 # someone else\n"
    );
}

#[test]
fn a_file_that_does_not_change_is_not_copied_by_the_last_look() {
    let dir = temp();
    let file = dir.path().join("a.toml");
    fs::write(&file, "x = 1\n").expect("write");
    let (_, mut guard) = read_file(&file);
    guard.write("x = 2\n", |_| true).expect("saved");
    assert_eq!(read_names(dir.path()), ["a.toml"]);
}

mod config_file {
    use std::fs;
    use std::path::Path;

    use crate::cli::Screen;
    use crate::config::ConfigFile;
    use crate::config::TabSet;
    use crate::grab_state::Follow;
    use crate::history::HistoryFile;
    use crate::history::HostTarget;
    use crate::machine_choices::Choice;
    use crate::settings::Key;
    use crate::settings::Layer;
    use crate::test_dir::TestDir;

    fn host(text: &str) -> HostTarget {
        text.parse().expect("a target")
    }

    fn names(hosts: &[HostTarget]) -> Vec<&str> {
        hosts.iter().map(HostTarget::as_str).collect()
    }

    fn write(dir: &TestDir, text: &str) -> std::path::PathBuf {
        let path = dir.path().join("config.toml");
        fs::write(&path, text).expect("write the file");
        path
    }

    fn files_in(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(path)
            .expect("a folder")
            .map(|e| {
                e.expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_loading_writes_nothing() {
        let dir = TestDir::new();
        let path = dir.path().join("inside").join("config.toml");
        let (file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(file.settings(), &Layer::default());
        assert!(file.tabs().machines().is_empty());
        assert!(
            !dir.path().join("inside").exists(),
            "loading creates nothing"
        );
    }

    #[test]
    fn what_is_saved_loads_back_equal() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        let (mut file, _) = ConfigFile::load(&path);
        file.tabs_mut().set_shown(&host("me@b"));
        file.tabs_mut().remember(host("me@a"));
        file.tabs_mut().open_machine(&host("me@a"));
        file.settings_mut().grab_follows = Some(Follow::Hover);
        file.save().expect("save");

        let (again, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(again.tabs(), file.tabs());
        assert_eq!(again.settings(), file.settings());
        assert_eq!(names(again.tabs().machines()), ["me@b", "me@a"]);
        assert_eq!(names(again.tabs().open()), ["me@b", "me@a"]);
        assert_eq!(again.tabs().shown().map(HostTarget::as_str), Some("me@b"));
        assert_eq!(
            files_in(dir.path()),
            ["config.toml"],
            "no temporary file is left"
        );
    }

    #[test]
    fn unknown_keys_are_ignored_and_missing_keys_take_defaults() {
        let dir = TestDir::new();
        let path = write(
            &dir,
            "colour = 1\n[settings]\nmood = \"x\"\ngrab_follows = \"hover\"\n",
        );
        let (file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(file.settings().grab_follows, Some(Follow::Hover));
        assert_eq!(file.settings().release_chord, None);
        assert!(file.tabs().machines().is_empty());
    }

    #[test]
    fn a_bad_setting_is_a_note_and_the_rest_still_loads() {
        let dir = TestDir::new();
        let path = write(
            &dir,
            "machines = [\"me@a\"]\n[settings]\ngrab_follows = \"sideways\"\nadapter = \"minimum-power\"\n",
        );
        let (file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("grab_follows"), "{notes:?}");
        assert_eq!(file.settings().grab_follows, None);
        assert!(file.settings().adapter.is_some());
        assert_eq!(names(file.tabs().machines()), ["me@a"]);
    }

    #[test]
    fn a_broken_file_gives_the_defaults_and_is_kept_as_broken_at_the_first_save() {
        let dir = TestDir::new();
        let path = write(&dir, "machines = [unclosed");
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(file.settings(), &Layer::default());
        assert_eq!(
            fs::read_to_string(&path).expect("untouched"),
            "machines = [unclosed",
            "loading leaves a broken file alone"
        );

        file.tabs_mut().remember(host("me@a"));
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.broken")).expect("kept"),
            "machines = [unclosed"
        );
        let (again, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(names(again.tabs().machines()), ["me@a"]);

        file.save().expect("a second save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.broken")).expect("still the old text"),
            "machines = [unclosed",
            "the good file is not set aside again"
        );
    }

    #[test]
    fn a_file_fixed_by_hand_before_the_save_is_kept_as_a_copy_not_lost() {
        let dir = TestDir::new();
        let path = write(&dir, "this is = = broken");
        let (mut file, _) = ConfigFile::load(&path);
        let fixed = "[settings]\nrelease_chord = \"shift+esc\"\n";
        fs::write(&path, fixed).expect("fix it");
        file.tabs_mut().remember(host("me@a"));
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("the fix is kept"),
            fixed,
            "the hand fix is in a copy, not overwritten with defaults"
        );
        assert_eq!(
            files_in(dir.path()),
            ["config.toml", "config.toml.replaced"]
        );
        let (again, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(names(again.tabs().machines()), ["me@a"]);
    }

    #[test]
    fn a_file_that_became_damaged_after_it_was_sound_is_kept_as_broken() {
        let dir = TestDir::new();
        let path = write(&dir, "machines = [\"me@a\"]\n");
        let (mut file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        fs::write(&path, "machines = [oops").expect("damage it");
        file.tabs_mut().remember(host("me@b"));
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.broken")).expect("kept"),
            "machines = [oops"
        );
    }

    #[test]
    fn a_file_another_client_changed_is_kept_before_it_is_replaced() {
        let dir = TestDir::new();
        let path = write(&dir, "machines = [\"me@a\"]\n");
        let (mut file, _) = ConfigFile::load(&path);
        let other = "machines = [\"me@a\", \"me@other\"]\n";
        fs::write(&path, other).expect("another client saves");
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            other
        );
        file.save().expect("a second save");
        assert_eq!(
            files_in(dir.path()),
            ["config.toml", "config.toml.replaced"],
            "what this client wrote itself is not copied"
        );
    }

    #[test]
    fn a_save_that_fails_loses_nothing_and_the_next_one_still_compares_right() {
        let dir = TestDir::new();
        let path = write(&dir, "= first");
        let (mut file, _) = ConfigFile::load(&path);
        // A folder where the file goes makes the save fail.
        fs::write(&path, "= second").expect("edit");
        let blocked = dir.path().join("config.toml");
        let parked = dir.path().join("parked");
        fs::rename(&blocked, &parked).expect("park");
        fs::create_dir(&blocked).expect("a folder where the file goes");
        file.save().expect_err("cannot replace a folder");
        fs::remove_dir(&blocked).expect("remove the blocker");
        fs::rename(&parked, &blocked).expect("restore");
        file.tabs_mut().remember(host("me@a"));
        file.save().expect("saves once the way is clear");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.broken")).expect("kept"),
            "= second",
            "the text of the last failed try is still kept"
        );
        let (again, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(names(again.tabs().machines()), ["me@a"]);
    }

    #[test]
    fn a_sound_file_gets_the_same_checks_on_every_save() {
        // The rule has no shortcut for a file that loaded fine.
        let dir = TestDir::new();
        let path = write(&dir, "");
        let (mut file, _) = ConfigFile::load(&path);
        file.save().expect("save");
        fs::write(&path, "hand = = edit").expect("damage it");
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.broken")).expect("kept"),
            "hand = = edit"
        );
    }

    #[test]
    fn a_file_that_is_too_large_to_read_is_kept_not_overwritten_unseen() {
        let dir = TestDir::new();
        let path = write(&dir, "");
        let (mut file, _) = ConfigFile::load(&path);
        fs::write(&path, vec![b'#'; (1 << 20) + 5]).expect("grow it");
        file.save().expect("save");
        assert_eq!(
            fs::metadata(dir.path().join("config.toml.broken"))
                .expect("kept")
                .len(),
            (1 << 20) + 5
        );
    }

    #[test]
    fn a_file_dropped_entries_from_is_kept_as_a_copy_at_the_first_save() {
        let dir = TestDir::new();
        let text = "machines = [\"me@a\", {name = \"b\"}, \"has space\"]\n";
        let path = write(&dir, text);
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            text,
            "the entries the client cannot use are in the copy"
        );
        file.save().expect("a second save");
        assert_eq!(
            files_in(dir.path()),
            ["config.toml", "config.toml.replaced"]
        );
    }

    #[test]
    fn a_file_still_broken_at_the_save_is_set_aside_with_its_latest_text() {
        let dir = TestDir::new();
        let path = write(&dir, "= first");
        let (mut file, _) = ConfigFile::load(&path);
        fs::write(&path, "= second").expect("edit");
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.broken")).expect("kept"),
            "= second"
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_damaged_and_is_not_overwritten() {
        let dir = TestDir::new();
        // A folder in place of the file: reading it fails with an I/O error,
        // as a sharing violation would.
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).expect("folder");
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("cannot be read"), "{notes:?}");
        assert!(!notes[0].contains("broken"), "{notes:?}");
        file.tabs_mut().remember(host("me@a"));
        let err = file.save().expect_err("not saved");
        assert!(err.to_string().contains("left as it is"), "{err}");
        assert!(path.is_dir(), "the file is where it was");
        assert_eq!(
            files_in(dir.path()),
            ["config.toml"],
            "no .broken, no temporary"
        );
    }

    #[test]
    fn a_file_that_could_not_be_read_but_parses_at_the_save_is_kept_then_saved() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).expect("folder");
        let (mut file, _) = ConfigFile::load(&path);
        fs::remove_dir(&path).expect("the lock clears");
        let text = "machines = [\"me@a\"]\n";
        fs::write(&path, text).expect("the file is readable now");
        file.tabs_mut().remember(host("me@b"));
        file.save().expect("saves once it can be read");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            text
        );
        file.save()
            .expect("and the health is sound for the next save");
    }

    #[test]
    fn a_file_that_could_not_be_read_and_is_damaged_at_the_save_is_left_alone() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).expect("folder");
        let (mut file, _) = ConfigFile::load(&path);
        fs::remove_dir(&path).expect("the lock clears");
        fs::write(&path, "= damaged").expect("damaged text");
        let err = file.save().expect_err("refused");
        assert!(err.to_string().contains("left as it is"), "{err}");
        assert!(!err.is_transient(), "{err}");
        assert_eq!(files_in(dir.path()), ["config.toml"], "no copy, no write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "= damaged");
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_that_clears_lets_the_next_save_work() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let path = write(&dir, "machines = [\"me@a\"]\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("lock");
        let (mut file, notes) = ConfigFile::load(&path);
        if notes.is_empty() {
            return; // running as a user the mode does not bind
        }
        let err = file.save().expect_err("locked");
        assert!(err.is_transient(), "a permission can be fixed: {err}");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("unlock");
        file.tabs_mut().remember(host("me@b"));
        file.save().expect("saves after the lock clears");
        let (again, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(names(again.tabs().machines()), ["me@b"]);
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            "machines = [\"me@a\"]\n",
            "the text that could not be read is kept"
        );
    }

    #[test]
    fn a_save_does_not_give_up_on_a_name_a_folder_blocks_and_says_why() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).expect("folder");
        let (mut file, _) = ConfigFile::load(&path);
        let started = std::time::Instant::now();
        let err = file.save().expect_err("cannot replace a folder");
        assert!(!err.is_transient(), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(150),
            "no pause for an error that will not go away: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_text_over_the_limit_is_refused_and_the_file_is_untouched() {
        let dir = TestDir::new();
        let mut text = String::from("x = [");
        while text.len() < (1 << 20) - 64 {
            text.push_str("{a=1},");
        }
        text.push_str("]\n");
        let path = write(&dir, &text);
        let (mut file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        // Each kept inline table is written as a table of its own, which is
        // longer: the result no longer loads back.
        let err = file.save().expect_err("the result would not load back");
        assert!(!err.is_transient(), "{err}");
        assert_eq!(fs::read_to_string(&path).expect("read"), text);
        assert_eq!(
            files_in(dir.path()),
            ["config.toml"],
            "no copy, no temporary"
        );
    }

    #[test]
    fn odd_but_valid_toml_survives_a_round_trip() {
        let dir = TestDir::new();
        let odd = "\"a b\" = 1\n'' = 2\n\"x.y\" = { \"k.l\" = [1, {z = \"q\"}], \"\" = 3 }\n\
                   dt = 1979-05-27T07:32:00-08:00\nbig = 9223372036854775807\nneg = -0.0\n\
                   s = \"line\\nnew\\u0000nul\\u202e\"\n[[arr]]\na = 1\n[[arr]]\na = 2\n\
                   [settings]\nssh = \"ssh\"\n\"weird key\" = [1, 2]\nnested.sub = 5\n";
        let path = write(&dir, odd);
        let (mut file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        file.save().expect("save");
        let before: toml::Table = odd.parse().expect("parses");
        let after: toml::Table = fs::read_to_string(&path)
            .expect("read")
            .parse()
            .expect("what was saved parses");
        assert_eq!(after, before);
    }

    #[test]
    fn a_bad_setting_is_kept_until_it_is_replaced_or_reset() {
        let dir = TestDir::new();
        let path = write(&dir, "[settings]\nrelease_chord = \"bogus\"\n");
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        file.save().expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("bogus"), "kept so it can be fixed: {text}");

        file.reset_setting(Key::ReleaseChord);
        file.save().expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(!text.contains("bogus"), "reset removes it: {text}");
        let (_, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_file_that_could_not_be_read_but_has_gone_is_saved() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).expect("folder");
        let (mut file, _) = ConfigFile::load(&path);
        fs::remove_dir(&path).expect("gone");
        file.tabs_mut().remember(host("me@a"));
        file.save().expect("saved");
        assert_eq!(files_in(dir.path()), ["config.toml"]);
    }

    #[test]
    fn a_file_that_vanishes_before_the_save_does_not_fail_it() {
        let dir = TestDir::new();
        let path = write(&dir, "= x");
        let (mut file, _) = ConfigFile::load(&path);
        fs::remove_file(&path).expect("gone");
        file.save().expect("save");
        file.save().expect("and again");
        assert_eq!(files_in(dir.path()), ["config.toml"], "nothing to keep");
    }

    #[test]
    fn keys_the_client_does_not_know_survive_a_save() {
        let dir = TestDir::new();
        let path = write(
            &dir,
            "future = 1\n[settings]\nrelease_chord = \"bogus\"\ngrab_follows = \"hover\"\nmood = \"x\"\n[extra]\na = 2\n",
        );
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "the bad chord is noted: {notes:?}");
        file.settings_mut().adapter = Some(crate::cli::AdapterChoice::MinimumPower);
        file.save().expect("save");
        let text = fs::read_to_string(&path).expect("read");
        let saved: toml::Table = text.parse().expect("TOML");
        assert_eq!(saved["future"].as_integer(), Some(1), "{text}");
        assert_eq!(saved["extra"]["a"].as_integer(), Some(2), "{text}");
        let settings = saved["settings"].as_table().expect("table");
        assert_eq!(settings["mood"].as_str(), Some("x"), "{text}");
        assert_eq!(settings["release_chord"].as_str(), Some("bogus"), "{text}");
        assert_eq!(settings["grab_follows"].as_str(), Some("hover"), "{text}");
        assert_eq!(
            settings["adapter"].as_str(),
            Some("minimum-power"),
            "{text}"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            "future = 1\n[settings]\nrelease_chord = \"bogus\"\ngrab_follows = \"hover\"\nmood = \"x\"\n[extra]\na = 2\n",
            "the text was not in the client's layout, so it is kept once"
        );
        assert_eq!(
            files_in(dir.path()),
            ["config.toml", "config.toml.replaced"]
        );
    }

    #[test]
    fn a_hand_written_file_is_kept_once_before_its_comments_are_lost() {
        let dir = TestDir::new();
        let original = "# my machines\nmachines = [\"me@a\"] # the first\n\n\
                        [settings]\n# how to get the pointer back\nrelease_chord = \"shift+esc\"\n";
        let path = write(&dir, original);
        let (mut file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        file.tabs_mut().remember(host("me@b"));
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            original,
            "the comments are in the copy"
        );
        assert!(
            !fs::read_to_string(&path).expect("read").contains('#'),
            "and not in the file"
        );
        file.tabs_mut().remember(host("me@c"));
        file.save().expect("save again");
        assert_eq!(
            files_in(dir.path()),
            ["config.toml", "config.toml.replaced"],
            "once"
        );
    }

    #[test]
    fn a_file_the_client_wrote_is_replaced_without_a_copy_in_every_run() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        let (mut file, _) = ConfigFile::load(&path);
        file.tabs_mut().remember(host("me@a"));
        file.settings_mut().release_chord = Some("shift+esc".parse().expect("chord"));
        file.save().expect("save");
        for _ in 0..2 {
            let (mut again, notes) = ConfigFile::load(&path);
            assert!(notes.is_empty(), "{notes:?}");
            again.tabs_mut().remember(host("me@b"));
            again.save().expect("save");
        }
        assert_eq!(files_in(dir.path()), ["config.toml"]);
    }

    #[test]
    fn a_file_with_a_setting_note_is_kept_before_the_save() {
        // The notes of the settings come after the tabs' notes: a file whose
        // only problem is a setting must still be kept when the save would
        // write it another way.
        let dir = TestDir::new();
        let original = "[settings]\nrelease_chord = \"bogus\"\n\n\n";
        let path = write(&dir, original);
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        file.tabs_mut().remember(host("me@a"));
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            original
        );
    }

    #[test]
    fn a_setting_note_alone_does_not_add_a_copy_to_the_clients_own_file() {
        let dir = TestDir::new();
        let path = write(&dir, "[settings]\nrelease_chord = \"bogus\"\n");
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        file.save().expect("save");
        assert_eq!(
            files_in(dir.path()),
            ["config.toml"],
            "the bad value is written back as it was; nothing is lost"
        );
    }

    #[test]
    fn a_setting_the_save_writes_another_way_is_kept_before_the_save() {
        // The text is in the client's layout, and the value is read, then
        // written back in its own spelling: no note, and still a change.
        let dir = TestDir::new();
        let original = "[settings]\nrelease_chord = \"SHIFT+ESC\"\n";
        let path = write(&dir, original);
        let (mut file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "[settings]\nrelease_chord = \"shift+esc\"\n",
            "the value is written in its own spelling"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            original,
            "so the old text is kept"
        );
    }

    #[test]
    fn a_setting_the_user_changes_replaces_the_kept_text_of_the_same_key() {
        let dir = TestDir::new();
        let path = write(&dir, "[settings]\nrelease_chord = \"bogus\"\n");
        let (mut file, _) = ConfigFile::load(&path);
        file.settings_mut().release_chord = Some("shift+esc".parse().expect("chord"));
        file.save().expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("shift+esc") && !text.contains("bogus"),
            "{text}"
        );
    }

    #[test]
    fn only_the_users_choices_are_saved() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        let (mut file, _) = ConfigFile::load(&path);
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "",
            "defaults write nothing"
        );

        file.settings_mut().keep_in_tray = Some(true);
        file.save().expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("keep_in_tray = true"), "{text}");
        assert!(!text.contains("release_chord"), "{text}");
        for hidden in ["test_panic", "hook_stall", "stats_out"] {
            assert!(!text.contains(hidden), "{hidden} is never saved");
        }
    }

    #[test]
    fn the_tab_set_keeps_its_rules() {
        let mut tabs = TabSet::default();
        tabs.remember(host("me@a"));
        tabs.remember(host("me@b"));
        tabs.remember(host("me@A"));
        assert_eq!(
            names(tabs.machines()),
            ["me@a", "me@b"],
            "a host name is compared without case"
        );
        tabs.remember(host("ME@a"));
        assert_eq!(
            tabs.machines().len(),
            3,
            "the user part is compared as written"
        );

        tabs.open_machine(&host("me@b"));
        tabs.open_machine(&host("me@b"));
        assert_eq!(names(tabs.open()), ["me@b"], "open once");
        tabs.open_machine(&host("me@new"));
        assert!(
            tabs.machines().iter().any(|m| m.as_str() == "me@new"),
            "an open machine is remembered"
        );

        tabs.set_shown(&host("me@unlisted"));
        assert!(
            tabs.open().iter().any(|m| m.as_str() == "me@unlisted"),
            "a shown tab is open"
        );
        tabs.forget(&host("me@unlisted"));
        assert_eq!(tabs.shown(), None);
        assert!(!tabs.open().iter().any(|m| m.as_str() == "me@unlisted"));
        assert!(!tabs.machines().iter().any(|m| m.as_str() == "me@unlisted"));
    }

    #[test]
    fn a_file_that_breaks_the_rules_is_repaired_on_load() {
        let dir = TestDir::new();
        let path = write(
            &dir,
            "machines = [\"me@a\", \"-bad\", \"me@A\", 5]\nopen = [\"me@a\", \"me@c\"]\nshown = \"me@z\"\n",
        );
        let (file, notes) = ConfigFile::load(&path);
        let all = notes.join("\n");
        assert!(all.contains("-bad") || all.contains("dropped 2"), "{all}");
        assert!(all.contains("me@A is listed twice"), "{all}");
        assert!(all.contains("me@c was not in machines"), "{all}");
        assert!(all.contains("me@z is not open"), "{all}");
        assert_eq!(
            names(file.tabs().machines()),
            ["me@a", "me@c"],
            "an open machine is kept in the list"
        );
        assert_eq!(names(file.tabs().open()), ["me@a", "me@c"]);
        assert_eq!(file.tabs().shown(), None, "shown must be open");
    }

    #[test]
    fn the_tab_set_and_the_history_never_touch_each_other() {
        let dir = TestDir::new();
        let config = dir.path().join("config.toml");
        let history = dir.path().join("recent-hosts.toml");
        let (mut hist, _) = HistoryFile::load(&history, true);
        hist.record(host("me@a")).expect("record");
        let before = fs::read(&history).expect("read");

        let (mut file, _) = ConfigFile::load(&config);
        file.tabs_mut().remember(host("me@a"));
        file.tabs_mut().forget(&host("me@a"));
        file.save().expect("save");
        assert_eq!(
            fs::read(&history).expect("read"),
            before,
            "forgetting a tab keeps the history"
        );

        let config_before = fs::read(&config).expect("read");
        hist.record(host("me@b")).expect("record");
        hist.clear().expect("clear");
        assert_eq!(
            fs::read(&config).expect("read"),
            config_before,
            "history changes keep the tab set"
        );
    }

    fn chosen(file: &ConfigFile, machine: &str) -> Choice {
        file.choices().borrow().of(&host(machine))
    }

    #[test]
    fn the_menu_choice_is_saved_under_the_machines_overrides_table() {
        let dir = TestDir::new();
        let path = dir.path().join("config.toml");
        let (mut file, _) = ConfigFile::load(&path);
        file.choices()
            .borrow_mut()
            .set_screen(&host("me@one"), Screen::Own);
        file.choices()
            .borrow_mut()
            .set_share_clipboard(&host("me@one"), false);
        file.save().expect("save");

        let text = fs::read_to_string(&path).expect("the file");
        let table: toml::Table = text.parse().expect("TOML");
        assert_eq!(table["overrides"]["me@one"]["screen"].as_str(), Some("own"));
        assert_eq!(
            table["overrides"]["me@one"]["share_clipboard"].as_bool(),
            Some(false)
        );
        assert!(!table.contains_key("settings"), "{text}");

        let (again, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            chosen(&again, "me@one"),
            Choice {
                screen: Some(Screen::Own),
                share_clipboard: Some(false)
            }
        );
        assert_eq!(chosen(&again, "me@two"), Choice::default());
    }

    #[test]
    fn an_old_config_file_loads_and_keeps_what_it_does_not_know() {
        // A file from before the menu: tabs, settings, a key of a newer
        // client, and no `[overrides]`.
        let old = "machines = [\"me@a\"]\nopen = [\"me@a\"]\nshown = \"me@a\"\nfuture = 7\n\n[settings]\nscreen = \"own\"\nmood = \"x\"\n";
        let dir = TestDir::new();
        let path = write(&dir, old);
        let (mut file, notes) = ConfigFile::load(&path);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(file.settings().screen, Some(Screen::Own));
        assert_eq!(chosen(&file, "me@a"), Choice::default());
        assert!(
            file.choices().borrow().to_table().is_empty(),
            "no table appears"
        );

        // Saving with no choice made changes nothing that matters.
        file.save().expect("save");
        let table: toml::Table = fs::read_to_string(&path)
            .expect("file")
            .parse()
            .expect("TOML");
        assert!(!table.contains_key("overrides"));

        // A choice joins the file and takes nothing out of it.
        file.choices()
            .borrow_mut()
            .set_screen(&host("me@a"), Screen::Mirror);
        file.save().expect("save");
        let table: toml::Table = fs::read_to_string(&path)
            .expect("file")
            .parse()
            .expect("TOML");
        assert_eq!(table["future"].as_integer(), Some(7));
        assert_eq!(table["settings"]["mood"].as_str(), Some("x"));
        assert_eq!(table["settings"]["screen"].as_str(), Some("own"));
        assert_eq!(table["machines"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            table["overrides"]["me@a"]["screen"].as_str(),
            Some("mirror")
        );
    }

    #[test]
    fn what_an_overrides_table_holds_that_is_not_a_choice_survives_a_save() {
        let dir = TestDir::new();
        let path = write(
            &dir,
            "[overrides.\"me@a\"]\nscreen = \"own\"\nfuture_key = 3\nshare_clipboard = \"yes\"\n",
        );
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(chosen(&file, "me@a").screen, Some(Screen::Own));
        file.choices()
            .borrow_mut()
            .set_screen(&host("me@a"), Screen::Mirror);
        file.save().expect("save");
        let table: toml::Table = fs::read_to_string(&path)
            .expect("file")
            .parse()
            .expect("TOML");
        let own = &table["overrides"]["me@a"];
        assert_eq!(own["screen"].as_str(), Some("mirror"));
        assert_eq!(own["future_key"].as_integer(), Some(3));
        assert_eq!(own["share_clipboard"].as_str(), Some("yes"));
    }

    #[test]
    fn an_overrides_entry_that_is_not_a_table_is_a_note_and_the_old_text_is_kept_as_a_copy() {
        let dir = TestDir::new();
        let path = write(&dir, "overrides = 5\n");
        let (mut file, notes) = ConfigFile::load(&path);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("overrides"), "{notes:?}");
        file.choices()
            .borrow_mut()
            .set_screen(&host("me@a"), Screen::Own);
        file.save().expect("save");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.toml.replaced")).expect("kept"),
            "overrides = 5\n"
        );
        let (again, _) = ConfigFile::load(&path);
        assert_eq!(chosen(&again, "me@a").screen, Some(Screen::Own));
    }
}
