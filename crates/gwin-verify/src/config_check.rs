//! `gwin-verify config`: the client's settings and history files, through the
//! real program.
//!
//! Runs `gliff-windows --print-settings` against a scratch config folder and
//! checks the precedence (flag, then file, then default), that a damaged line
//! in the history is dropped, that two chords that collide stop the start, and
//! that reading changed nothing. No session starts, and the real config
//! folder is never read or written.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::client_config::fresh_config_dir;
use crate::line::Line;

const CONFIG: &str = "machines = [\"user@one\", \"user@two\"]\n\
open = [\"user@two\"]\n\
[settings]\n\
grab_follows = \"hover\"\n\
injected_keys = \"local\"\n\
adapter = \"high-performance\"\n";

const HISTORY: &str = "hosts = [\"user@recent\", \"-oProxyCommand=x\", \"user@older\"]\n";

pub struct Options {
    pub client: PathBuf,
    pub log_dir: PathBuf,
}

/// One setting of a `--print-settings` report is `value` from `source`.
fn expect_setting(report: &Value, key: &str, value: &str, source: &str) -> Result<(), String> {
    let setting = &report["settings"][key];
    let got = (setting["value"].as_str(), setting["source"].as_str());
    if got == (Some(value), Some(source)) {
        Ok(())
    } else {
        Err(format!("{key}: want {value} from {source}, got {setting}"))
    }
}

fn expect_list(report: &Value, key: &str, want: &[&str]) -> Result<(), String> {
    let got: Vec<&str> = report[key]
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if got == want {
        Ok(())
    } else {
        Err(format!("{key}: want {want:?}, got {got:?}"))
    }
}

/// Every wrong thing in the two reports, as text; empty when all is right.
fn judge(plain: &Value, with_flag: &Value) -> Vec<String> {
    [
        expect_setting(plain, "grab_follows", "hover", "file"),
        expect_setting(plain, "injected_keys", "local", "file"),
        expect_setting(plain, "adapter", "high-performance", "file"),
        expect_setting(plain, "super_l", "off", "flag"),
        expect_setting(plain, "side_buttons", "upstream", "default"),
        expect_setting(with_flag, "grab_follows", "focus", "flag"),
        expect_setting(with_flag, "injected_keys", "local", "file"),
        expect_list(plain, "machines", &["user@one", "user@two"]),
        expect_list(plain, "open", &["user@two"]),
        expect_list(plain, "recent_hosts", &["user@recent", "user@older"]),
    ]
    .into_iter()
    .filter_map(Result::err)
    .collect()
}

fn print_settings(
    client: &Path,
    dir: &Path,
    extra: &[&str],
) -> Result<std::process::Output, String> {
    Command::new(client)
        .args(["--print-settings", "--super-l", "off", "--config-dir"])
        .arg(dir)
        .args(extra)
        .output()
        .map_err(|e| format!("starting {}: {e}", client.display()))
}

fn report_of(output: &std::process::Output) -> Result<Value, String> {
    if !output.status.success() {
        return Err(format!(
            "exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("the report is not JSON: {e}"))
}

fn names_in(dir: &Path) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

fn steps(opts: &Options) -> Result<Vec<String>, String> {
    let dir = fresh_config_dir(&opts.log_dir).map_err(|e| format!("config folder: {e}"))?;
    std::fs::write(dir.join("config.toml"), CONFIG).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("recent-hosts.toml"), HISTORY).map_err(|e| e.to_string())?;
    let plain = report_of(&print_settings(&opts.client, &dir, &[])?)?;
    let with_flag = report_of(&print_settings(
        &opts.client,
        &dir,
        &["--grab-follows", "focus"],
    )?)?;
    let mut problems = judge(&plain, &with_flag);
    let overlap = print_settings(
        &opts.client,
        &dir,
        &["--release-chord", "ctrl+alt+pagedown"],
    )?;
    if overlap.status.code() != Some(2)
        || !String::from_utf8_lossy(&overlap.stderr).contains("--release-chord")
    {
        problems.push(format!(
            "a colliding chord: want exit 2 naming --release-chord, got {:?}",
            overlap.status.code()
        ));
    }
    let names = names_in(&dir).map_err(|e| e.to_string())?;
    if names != ["config.toml", "recent-hosts.toml"] {
        problems.push(format!("reading changed the folder: {names:?}"));
    }
    if std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .as_deref()
        != Some(CONFIG)
    {
        problems.push("reading changed config.toml".to_owned());
    }
    Ok(problems)
}

pub fn run(opts: &Options) -> Line {
    match steps(opts) {
        Ok(problems) if problems.is_empty() => Line::new("config", true)
            .with("client", opts.client.display().to_string())
            .with("runs", 3),
        Ok(problems) => Line::new("config", false).with("problems", problems),
        Err(e) => Line::new("config", false).with("error", e),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;

    use super::judge;

    fn report(follows: &str, source: &str) -> Value {
        json!({
            "settings": {
                "grab_follows": {"value": follows, "source": source},
                "injected_keys": {"value": "local", "source": "file"},
                "adapter": {"value": "high-performance", "source": "file"},
                "super_l": {"value": "off", "source": "flag"},
                "side_buttons": {"value": "upstream", "source": "default"},
            },
            "machines": ["user@one", "user@two"],
            "open": ["user@two"],
            "recent_hosts": ["user@recent", "user@older"],
        })
    }

    #[test]
    fn a_right_pair_of_reports_has_no_problems() {
        assert_eq!(
            judge(&report("hover", "file"), &report("focus", "flag")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_flag_that_does_not_win_is_named() {
        let problems = judge(&report("hover", "file"), &report("hover", "file"));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("grab_follows"), "{problems:?}");
    }

    #[test]
    fn a_hostile_history_line_that_stays_is_named() {
        let mut plain = report("hover", "file");
        plain["recent_hosts"] = json!(["user@recent", "-oProxyCommand=x", "user@older"]);
        let problems = judge(&plain, &report("focus", "flag"));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("recent_hosts"), "{problems:?}");
    }
}
