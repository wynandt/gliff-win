use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use clap::Parser;

use super::Startup;
use super::StartupError;
use crate::cli::Args;
use crate::test_dir::TestDir;

fn args(dir: &TestDir, extra: &[&str]) -> Args {
    let mut all = vec![
        "gliff-windows".to_owned(),
        "--config-dir".to_owned(),
        dir.path().display().to_string(),
    ];
    all.extend(extra.iter().map(|s| (*s).to_owned()));
    Args::try_parse_from(all).expect("parse")
}

fn load(args: &Args) -> Startup {
    Startup::load_with(args, None, None).expect("startup")
}

#[test]
fn flag_beats_file_beats_default_in_the_report() {
    let dir = TestDir::new();
    fs::write(
        dir.path().join("config.toml"),
        "[settings]\ngrab_follows = \"hover\"\ninjected_keys = \"local\"\n",
    )
    .expect("write");
    let report = load(&args(
        &dir,
        &["--print-settings", "--injected-keys", "forward"],
    ))
    .report();
    let settings = &report["settings"];
    assert_eq!(settings["grab_follows"]["value"], "hover");
    assert_eq!(settings["grab_follows"]["source"], "file");
    assert_eq!(settings["injected_keys"]["value"], "forward");
    assert_eq!(settings["injected_keys"]["source"], "flag");
    assert_eq!(settings["adapter"]["value"], "monitor");
    assert_eq!(settings["adapter"]["source"], "default");
    assert_eq!(report["config_dir"], dir.path().display().to_string());
    assert_eq!(report["config_dir_is_default"], false);
}

#[test]
fn the_report_lists_the_machines_the_history_and_the_notes() {
    let dir = TestDir::new();
    fs::write(
        dir.path().join("config.toml"),
        "machines = [\"me@a\", \"me@b\"]\nopen = [\"me@b\"]\nshown = \"me@b\"\n[settings]\nadapter = \"banana\"\n",
    )
    .expect("write");
    fs::write(
        dir.path().join("recent-hosts.toml"),
        "hosts = [\"me@c\", \"-oProxyCommand=x\", \"me@d\"]\n",
    )
    .expect("write");
    let startup = load(&args(&dir, &["--print-settings"]));
    let report = startup.report();
    assert_eq!(report["machines"], serde_json::json!(["me@a", "me@b"]));
    assert_eq!(report["open"], serde_json::json!(["me@b"]));
    assert_eq!(report["shown"], "me@b");
    assert_eq!(
        report["recent_hosts"],
        serde_json::json!(["me@c", "me@d"]),
        "the hostile line is not listed"
    );
    let notes = report["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 2, "{notes:?}");
}

#[test]
fn remembering_off_lists_no_hosts() {
    let dir = TestDir::new();
    fs::write(
        dir.path().join("config.toml"),
        "[settings]\nremember_recent_hosts = false\n",
    )
    .expect("write");
    fs::write(dir.path().join("recent-hosts.toml"), "hosts = [\"me@c\"]\n").expect("write");
    let report = load(&args(&dir, &["--print-settings"])).report();
    assert_eq!(report["recent_hosts"], serde_json::json!([]));
    assert!(
        dir.path().join("recent-hosts.toml").exists(),
        "reading deletes nothing"
    );
}

#[test]
fn reading_changes_nothing_on_disk() {
    let dir = TestDir::new();
    let inside = dir.path().join("fresh");
    let a = Args::try_parse_from([
        "gliff-windows",
        "--config-dir",
        inside.to_str().expect("text"),
        "--print-settings",
    ])
    .expect("parse");
    let _ = load(&a);
    assert!(!inside.exists(), "no folder is created");

    fs::write(dir.path().join("config.toml"), "not toml [").expect("write");
    fs::write(dir.path().join("recent-hosts.toml"), "also [ not").expect("write");
    let _ = load(&args(&dir, &["--print-settings"]));
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .expect("folder")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["config.toml", "recent-hosts.toml"],
        "no .broken, no .tmp"
    );
}

#[test]
fn overlapping_chords_stop_the_start_with_the_reason() {
    let dir = TestDir::new();
    let error = Startup::load_with(
        &args(
            &dir,
            &["--print-settings", "--release-chord", "ctrl+alt+pagedown"],
        ),
        None,
        None,
    )
    .expect_err("overlap");
    assert!(matches!(error, StartupError::Chords(_)));
    assert!(error.to_string().contains("--release-chord"), "{error}");
}

#[test]
fn the_folder_comes_from_the_flag_then_the_environment_then_app_data() {
    let dir = TestDir::new();
    let none = Args::try_parse_from(["gliff-windows", "--print-settings"]).expect("parse");
    let from_env = Startup::load_with(&none, Some(OsString::from(dir.path())), None).expect("env");
    assert_eq!(from_env.dir.path(), dir.path());
    let from_app_data =
        Startup::load_with(&none, None, Some(PathBuf::from("/appdata"))).expect("app data");
    assert!(from_app_data.dir.is_default());
    let error = Startup::load_with(&none, None, None).expect_err("no folder");
    assert!(error.to_string().contains("--config-dir"), "{error}");
}

#[test]
fn the_report_shows_each_repair_of_the_machine_list() {
    let dir = TestDir::new();
    fs::write(
        dir.path().join("config.toml"),
        "machines = [\"me@a\", \"me@a\"]\nopen = [\"me@b\"]\nshown = \"me@a\"\n",
    )
    .expect("write");
    let report = load(&args(&dir, &["--print-settings"])).report();
    let notes: Vec<&str> = report["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .map(|n| n.as_str().expect("text"))
        .collect();
    let all = notes.join("\n");
    assert!(all.contains("me@a is listed twice"), "{all}");
    assert!(all.contains("me@b was not in machines"), "{all}");
    assert!(all.contains("me@a is not open"), "{all}");
    assert_eq!(report["machines"], serde_json::json!(["me@a", "me@b"]));
}

const TWO_MACHINES: &str = "machines = [\"me@a\", \"me@b\"]\n\
    [overrides.\"me@a\"]\nscreen = \"own\"\nshare_clipboard = false\nsharp_colour = true\ngpu_decode = true\n\
    [overrides.\"me@c\"]\nscreen = \"mirror\"\n";

#[test]
fn the_report_shows_each_machines_saved_choice_and_where_it_came_from() {
    let dir = TestDir::new();
    fs::write(dir.path().join("config.toml"), TWO_MACHINES).expect("write");
    let report = load(&args(&dir, &["--print-settings"])).report();
    let connection = &report["connection"];
    assert_eq!(
        connection["me@a"],
        serde_json::json!({
            "screen": {"value": "own", "source": "machine", "locked": false},
            "share_clipboard": {"value": false, "source": "machine", "locked": false},
            "sharp_colour": {"value": true, "source": "machine", "locked": false},
            "gpu_decode": {"value": true, "source": "machine", "locked": false},
            "send_win_l": {"value": false, "source": "default", "locked": false},
        })
    );
    assert_eq!(
        connection["me@b"],
        serde_json::json!({
            "screen": {"value": "mirror", "source": "default", "locked": false},
            "share_clipboard": {"value": true, "source": "default", "locked": false},
            "sharp_colour": {"value": false, "source": "default", "locked": false},
            "gpu_decode": {"value": false, "source": "default", "locked": false},
            "send_win_l": {"value": false, "source": "default", "locked": false},
        })
    );
    // A machine with a choice but no tab is listed too.
    assert_eq!(connection["me@c"]["screen"]["value"], "mirror");
    assert_eq!(connection["me@c"]["screen"]["source"], "machine");
}

#[test]
fn a_flag_shows_as_the_source_and_locks_the_machines_choice_in_the_report() {
    let dir = TestDir::new();
    fs::write(dir.path().join("config.toml"), TWO_MACHINES).expect("write");
    let report = load(&args(
        &dir,
        &["--print-settings", "--no-clipboard", "--output", "DP-2"],
    ))
    .report();
    let a = &report["connection"]["me@a"];
    assert_eq!(
        a["screen"],
        serde_json::json!({"value": "mirror", "source": "flag", "locked": true})
    );
    assert_eq!(
        a["share_clipboard"],
        serde_json::json!({"value": false, "source": "flag", "locked": true})
    );
    // The saved choice is still in the file, and loads.
    assert_eq!(
        fs::read_to_string(dir.path().join("config.toml")).expect("read"),
        TWO_MACHINES,
        "reading changes nothing"
    );
}
