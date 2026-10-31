//! `gliff-windows --print-settings` as a user runs it: the real program, a
//! temporary config folder, no session and no change to anything.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use serde_json::Value;

fn client(dir: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gliff-windows"))
        .args(["--print-settings", "--super-l", "off", "--config-dir"])
        .arg(dir)
        .args(extra)
        .env_remove("GLIFF_WINDOWS_CONFIG_DIR")
        .output()
        .expect("the client starts")
}

fn report(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "exit {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the report is JSON")
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gliff-print-settings-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a temporary folder");
    dir
}

#[test]
fn a_flag_beats_the_file_and_the_file_beats_the_default() {
    let dir = scratch("precedence");
    fs::write(
        dir.join("config.toml"),
        "[settings]\ngrab_follows = \"hover\"\ninjected_keys = \"local\"\n",
    )
    .expect("write");
    let out = report(&client(&dir, &["--injected-keys", "forward"]));
    let settings = &out["settings"];
    assert_eq!(settings["super_l"]["value"], "off");
    assert_eq!(settings["super_l"]["source"], "flag");
    assert_eq!(settings["injected_keys"]["value"], "forward");
    assert_eq!(settings["injected_keys"]["source"], "flag");
    assert_eq!(settings["grab_follows"]["value"], "hover");
    assert_eq!(settings["grab_follows"]["source"], "file");
    assert_eq!(settings["side_buttons"]["source"], "default");
    assert_eq!(out["config_dir"], dir.display().to_string());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_decided_defaults_are_what_a_fresh_folder_reports() {
    let dir = scratch("defaults");
    let out = report(&client(&dir, &[]));
    let s = &out["settings"];
    assert_eq!(s["keep_in_tray"]["value"], false);
    assert_eq!(s["remember_recent_hosts"]["value"], true);
    assert_eq!(s["switch_next"]["value"], "ctrl+alt+pagedown");
    assert_eq!(s["switch_previous"]["value"], "ctrl+alt+pageup");
    assert_eq!(s["switch_jump"]["value"], "ctrl+alt");
    assert_eq!(out["recent_hosts"], serde_json::json!([]));
    assert_eq!(out["machines"], serde_json::json!([]));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn damaged_files_are_reported_and_never_copied_or_changed() {
    let dir = scratch("damaged-untouched");
    fs::write(dir.join("config.toml"), "machines = [oops\n").expect("write");
    fs::write(dir.join("recent-hosts.toml"), "hosts = 5\n").expect("write");
    let out = report(&client(&dir, &["host"]));
    assert_eq!(out["notes"].as_array().expect("notes").len(), 2, "{out}");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("folder")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["config.toml", "recent-hosts.toml"],
        "no .broken copy"
    );
    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).expect("read"),
        "machines = [oops\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("recent-hosts.toml")).expect("read"),
        "hosts = 5\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn it_starts_no_session_and_creates_and_changes_nothing() {
    let dir = scratch("untouched");
    let fresh = dir.join("not-yet");
    let _ = report(&client(&fresh, &[]));
    assert!(
        !fresh.exists(),
        "a folder that does not exist stays missing"
    );

    fs::write(dir.join("config.toml"), "][ damaged").expect("write");
    fs::write(dir.join("recent-hosts.toml"), "me@a\n").expect("write");
    let out = report(&client(&dir, &[]));
    assert!(!out["notes"].as_array().expect("notes").is_empty());
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("folder")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["config.toml", "recent-hosts.toml"]);
    assert_eq!(
        fs::read_to_string(dir.join("config.toml")).expect("read"),
        "][ damaged"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn overlapping_chords_exit_2_and_say_which_flags() {
    let dir = scratch("overlap");
    let output = client(&dir, &["--release-chord", "ctrl+alt+pagedown"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("--release-chord"), "{message}");
    let _ = fs::remove_dir_all(&dir);
}

// On Windows a bare start opens the real window and waits, so this runs
// only where the client stops after loading its settings.
#[cfg(not(windows))]
#[test]
fn a_start_with_no_target_is_not_an_argument_error() {
    // Off Windows the client only parses and loads its settings; a bare
    // start must get as far as saying so, not stop at a missing TARGET.
    let dir = std::env::temp_dir().join(format!("gwin-bare-start-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_gliff-windows"))
        .arg("--config-dir")
        .arg(&dir)
        .args(["--super-l", "off"])
        .output()
        .expect("the client starts");
    assert_eq!(output.status.code(), Some(2));
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(!message.contains("TARGET"), "{message}");
    assert!(message.contains("targets []"), "{message}");
    let _ = fs::remove_dir_all(&dir);
}
