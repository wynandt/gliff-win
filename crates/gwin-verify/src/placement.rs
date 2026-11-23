//! Where the test pattern window is: parses what
//! [`PLACEMENT_COMMAND`] prints on the remote machine and decides whether a window
//! titled `gwin-pattern` covers the session's output.

use serde_json::Value;

/// Run over `ssh::exec` (a constant, so nothing is interpolated into the
/// remote shell). Prints two JSON arrays, monitors then clients.
///
/// A non-interactive ssh has no `HYPRLAND_INSTANCE_SIGNATURE`, and without
/// it hyprctl exits 1 with "HYPRLAND_INSTANCE_SIGNATURE not set!" on
/// stdout (hyprwm/Hyprland hyprctl/src/main.cpp, `main`). So when it is
/// unset the command finds the instance itself, where hyprctl looks for
/// its socket (`getRuntimeDir`: `$XDG_RUNTIME_DIR/hypr`, else
/// `/run/user/<uid>/hypr`, then `<sig>/.socket.sock`), taking the newest
/// socket: a crashed Hyprland can leave an older one behind. What it
/// picked, or that it found none, goes to stderr. `sh -c` keeps it POSIX
/// whatever the login shell is.
pub const PLACEMENT_COMMAND: &str = concat!(
    "sh -c '",
    "if [ -z \"${HYPRLAND_INSTANCE_SIGNATURE:-}\" ]; then ",
    "d=\"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/hypr\"; ",
    "s=$(ls -td \"$d\"/*/.socket.sock 2>/dev/null | head -n 1); ",
    "if [ -n \"$s\" ]; then ",
    "HYPRLAND_INSTANCE_SIGNATURE=$(basename \"$(dirname \"$s\")\"); ",
    "export HYPRLAND_INSTANCE_SIGNATURE; ",
    "echo \"gwin: HYPRLAND_INSTANCE_SIGNATURE=$HYPRLAND_INSTANCE_SIGNATURE (newest socket under $d)\" >&2; ",
    "else echo \"gwin: no Hyprland instance socket under $d\" >&2; fi; ",
    "fi; ",
    "hyprctl -j monitors all && hyprctl -j clients",
    "'"
);

/// The title `scripts/remote/gwin-pattern` gives its ffplay window.
pub const PATTERN_TITLE: &str = "gwin-pattern";

/// A monitor, logical size (Hyprland reports physical `width`/`height`
/// and a `scale`; windows are sized in logical pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    pub at: (i64, i64),
    pub logical: (i64, i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub monitor: i64,
    pub at: (i64, i64),
    pub size: (i64, i64),
    pub fullscreen: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// One pattern window covers the output (to within 1 px).
    Covers { monitor: Monitor, window: Window },
    /// The output exists but no window covers it.
    Elsewhere {
        monitor: Monitor,
        windows: Vec<Window>,
    },
    /// No monitor of that name.
    NoOutput { monitors: Vec<String> },
    /// More than one session output, so none is "the" session's
    /// ([`judge_session`], Windows and tests only).
    #[cfg_attr(
        not(any(windows, test)),
        allow(dead_code, reason = "only the Windows build and the tests use this")
    )]
    Ambiguous { outputs: Vec<String> },
    /// The command's output did not parse.
    Unreadable(String),
}

impl Placement {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Covers { .. })
    }

    /// One line for the report.
    pub fn describe(&self) -> String {
        match self {
            Self::Covers { monitor, window } => format!(
                "{PATTERN_TITLE} covers {} (monitor {}) at {:?} size {:?} fullscreen={}",
                monitor.name, monitor.id, window.at, window.size, window.fullscreen
            ),
            Self::Elsewhere { monitor, windows } if windows.is_empty() => format!(
                "no {PATTERN_TITLE} window; want one on {} (monitor {}) at {:?} size {:?}",
                monitor.name, monitor.id, monitor.at, monitor.logical
            ),
            Self::Elsewhere { monitor, windows } => format!(
                "{PATTERN_TITLE} is not on {} (monitor {} at {:?} size {:?}): {:?}",
                monitor.name, monitor.id, monitor.at, monitor.logical, windows
            ),
            Self::NoOutput { monitors } => {
                format!("no output of that name; monitors: {monitors:?}")
            }
            Self::Ambiguous { outputs } => {
                format!("more than one gliff-* output, cannot tell the session's: {outputs:?}")
            }
            Self::Unreadable(why) => format!("unreadable hyprctl output: {why}"),
        }
    }
}

/// Decides placement for the output `name` from [`PLACEMENT_COMMAND`]'s
/// stdout.
pub fn judge(stdout: &str, name: &str) -> Placement {
    let docs: Vec<Value> = match serde_json::Deserializer::from_str(stdout)
        .into_iter::<Value>()
        .collect::<Result<_, _>>()
    {
        Ok(docs) => docs,
        Err(e) => return Placement::Unreadable(e.to_string()),
    };
    let [monitors, clients] = docs.as_slice() else {
        return Placement::Unreadable(format!("want 2 JSON documents, got {}", docs.len()));
    };
    let (Some(monitors), Some(clients)) = (monitors.as_array(), clients.as_array()) else {
        return Placement::Unreadable("monitors and clients must be arrays".to_owned());
    };
    let monitors: Vec<Monitor> = monitors.iter().filter_map(monitor).collect();
    let Some(target) = monitors.iter().find(|m| m.name == name).cloned() else {
        return Placement::NoOutput {
            monitors: monitors.into_iter().map(|m| m.name).collect(),
        };
    };
    let windows: Vec<Window> = clients
        .iter()
        .filter(|c| c["title"].as_str() == Some(PATTERN_TITLE))
        .filter_map(window)
        .collect();
    let covering = windows.iter().find(|w| {
        w.monitor == target.id
            && w.at == target.at
            && (w.size.0 - target.logical.0).abs() <= 1
            && (w.size.1 - target.logical.1).abs() <= 1
    });
    match covering {
        Some(window) => Placement::Covers {
            monitor: target,
            window: window.clone(),
        },
        None => Placement::Elsewhere {
            monitor: target,
            windows,
        },
    }
}

/// Decides placement for the session's output when its name is not
/// known (the present gate: the client does not report it): the one
/// gliff-* output. Two or more is [`Placement::Ambiguous`], not a guess.
#[cfg_attr(
    not(any(windows, test)),
    allow(dead_code, reason = "only the Windows build and the tests use this")
)]
pub fn judge_session(stdout: &str) -> Placement {
    let Some(monitors) = serde_json::Deserializer::from_str(stdout)
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
    else {
        return judge(stdout, "");
    };
    let gliff: Vec<String> = monitors
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["name"].as_str())
        .filter(|n| n.starts_with(SESSION_PREFIX))
        .map(str::to_owned)
        .collect();
    match gliff.as_slice() {
        [name] => judge(stdout, name),
        [] => judge(stdout, ""),
        _ => Placement::Ambiguous { outputs: gliff },
    }
}

/// Upstream names each session's headless output `gliff-<pid>`.
#[cfg_attr(
    not(any(windows, test)),
    allow(dead_code, reason = "only the Windows build and the tests use this")
)]
const SESSION_PREFIX: &str = "gliff-";

fn monitor(m: &Value) -> Option<Monitor> {
    let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
    let logical = |v: &Value| v.as_f64().map(|v| (v / scale).round() as i64);
    Some(Monitor {
        id: m["id"].as_i64()?,
        name: m["name"].as_str()?.to_owned(),
        at: (m["x"].as_i64()?, m["y"].as_i64()?),
        logical: (logical(&m["width"])?, logical(&m["height"])?),
    })
}

fn window(c: &Value) -> Option<Window> {
    let pair = |v: &Value| Some((v[0].as_i64()?, v[1].as_i64()?));
    Some(Window {
        monitor: c["monitor"].as_i64()?,
        at: pair(&c["at"])?,
        size: pair(&c["size"])?,
        fullscreen: c["fullscreen"].as_i64().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::Placement;
    use super::judge;

    const MONITORS: &str = r#"[{"id":0,"name":"HDMI-A-1","width":2560,"height":1440,"x":0,"y":0,"scale":1.0},
        {"id":2,"name":"gliff-339504","width":3840,"height":2400,"x":2560,"y":0,"scale":2.0}]"#;

    #[test]
    fn a_fullscreen_window_on_the_output_covers_it_in_logical_pixels() {
        let clients = r#"[{"title":"gwin-pattern","monitor":2,"at":[2560,0],"size":[1920,1200],"fullscreen":2}]"#;
        let placement = judge(&format!("{MONITORS}\n{clients}\n"), "gliff-339504");
        assert!(placement.is_ok(), "{}", placement.describe());
    }

    #[test]
    fn fullscreen_on_the_wrong_monitor_is_caught() {
        let clients = r#"[{"title":"gwin-pattern","monitor":0,"at":[0,0],"size":[2560,1440],"fullscreen":2}]"#;
        let placement = judge(&format!("{MONITORS}{clients}"), "gliff-339504");
        assert!(
            matches!(placement, Placement::Elsewhere { ref windows, .. } if windows.len() == 1),
            "{placement:?}"
        );
    }

    #[test]
    fn missing_output_window_and_garbage_are_told_apart() {
        assert!(
            matches!(
                judge(&format!("{MONITORS}[]"), "gliff-1"),
                Placement::NoOutput { .. }
            ),
            "no such output"
        );
        assert!(
            matches!(
                judge(&format!("{MONITORS}[]"), "gliff-339504"),
                Placement::Elsewhere { ref windows, .. } if windows.is_empty()
            ),
            "no window"
        );
        assert!(
            matches!(
                judge("hyprctl: command not found", "x"),
                Placement::Unreadable(_)
            ),
            "garbage"
        );
        assert!(
            matches!(judge(MONITORS, "x"), Placement::Unreadable(_)),
            "one document"
        );
    }

    #[test]
    fn judge_session_finds_the_one_gliff_output_at_scale_2() {
        let stdout = concat!(
            r#"[{"id":0,"name":"eDP-1","width":2560,"height":1600,"x":0,"y":0,"scale":1.0},"#,
            r#"{"id":1,"name":"gliff-4242","width":2560,"height":1600,"x":2560,"y":0,"scale":2.0}]"#,
            "\n",
            r#"[{"title":"gwin-pattern","monitor":1,"at":[2560,0],"size":[1280,800],"fullscreen":2}]"#,
        );
        let placement = super::judge_session(stdout);
        assert!(placement.is_ok(), "covers: {}", placement.describe());
        assert!(
            placement.describe().contains("gliff-4242"),
            "{}",
            placement.describe()
        );
    }

    #[test]
    fn judge_session_without_a_gliff_output_says_so() {
        let stdout = concat!(
            r#"[{"id":0,"name":"eDP-1","width":2560,"height":1600,"x":0,"y":0,"scale":1.0}]"#,
            "\n[]",
        );
        let placement = super::judge_session(stdout);
        assert_eq!(
            placement,
            super::Placement::NoOutput {
                monitors: vec!["eDP-1".to_owned()]
            },
            "{}",
            placement.describe()
        );
    }

    #[test]
    fn judge_session_refuses_to_guess_between_two_gliff_outputs() {
        let stdout = concat!(
            r#"[{"id":1,"name":"gliff-1","width":2560,"height":1600,"x":2560,"y":0,"scale":2.0},"#,
            r#"{"id":2,"name":"gliff-2","width":1920,"height":1080,"x":3840,"y":0,"scale":1.0}]"#,
            "\n[]",
        );
        let placement = super::judge_session(stdout);
        assert!(!placement.is_ok(), "{}", placement.describe());
        assert!(
            placement.describe().contains("gliff-1") && placement.describe().contains("gliff-2"),
            "names both: {}",
            placement.describe()
        );
    }
}

#[cfg(all(test, unix))]
mod instance_picker {
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;
    use std::process::Output;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    use super::PLACEMENT_COMMAND;

    const LIVE: &str = "a1b2c3_1761250000_200";
    const DEAD: &str = "0d1e_1761000000_100";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("gwin-hypr-instance-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).expect("scratch dir");
            Scratch(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn output_text(out: &Output) -> String {
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        text
    }

    fn instance_in(text: &str) -> String {
        if let Some(sig) = text
            .lines()
            .find_map(|line| line.split("HYPRLAND_INSTANCE_SIGNATURE=").nth(1))
        {
            return sig
                .split(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or("")
                .to_owned();
        }
        for line in text.lines() {
            if let Some(idx) = line.find("/.socket.sock") {
                let path = &line[..idx];
                if let Some(sig) = path.rsplit('/').next()
                    && !sig.is_empty()
                {
                    return sig.to_owned();
                }
            }
        }
        panic!("no Hyprland instance in:\n{text}");
    }

    fn touch_date(path: &Path, date: &str) {
        let out = Command::new("touch")
            .arg("-d")
            .arg(date)
            .arg(path)
            .output()
            .expect("touch");
        assert!(
            out.status.success(),
            "touch {date}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn first_line(script: &str, dir: &Path) -> String {
        let out = Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(dir)
            .output()
            .expect("sh");
        assert!(
            out.status.success(),
            "{script} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .expect("utf8")
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_owned()
    }

    fn run(
        program: &str,
        args: &[&str],
        run_dir: &Path,
        path: &str,
        signature: Option<&str>,
    ) -> Output {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env("XDG_RUNTIME_DIR", run_dir)
            .env("PATH", path)
            .env("HOME", run_dir)
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE");
        if let Some(sig) = signature {
            cmd.env("HYPRLAND_INSTANCE_SIGNATURE", sig);
        }
        cmd.output().expect("spawn")
    }

    #[test]
    fn one_picker_takes_the_newest_socket() {
        let scratch = Scratch::new();
        let run_dir = scratch.path().join("run");
        let hypr = run_dir.join("hypr");
        let dead = hypr.join(DEAD);
        let live = hypr.join(LIVE);
        std::fs::create_dir_all(&dead).expect("dead instance");
        std::fs::create_dir_all(&live).expect("live instance");
        std::fs::write(dead.join(".socket.sock"), "").expect("dead socket");
        std::fs::write(live.join(".socket.sock"), "").expect("live socket");
        std::fs::write(dead.join(".dead"), "").expect("dead marker");
        touch_date(&dead.join(".socket.sock"), "2020-01-01 00:00:00");
        touch_date(&live.join(".socket.sock"), "2024-06-01 00:00:00");
        touch_date(&dead, "2026-01-01 00:00:00");
        touch_date(&live, "2021-01-01 00:00:00");

        assert_eq!(
            first_line(r#"ls -1t "$1""#, &hypr),
            DEAD,
            "fixture: the newest directory is the previous instance"
        );
        assert_eq!(
            first_line(
                r#"basename "$(dirname "$(ls -td "$1"/*/.socket.sock | head -n 1)")""#,
                &hypr,
            ),
            LIVE,
            "fixture: the newest socket is the running instance"
        );

        let path = format!(
            "{}:{}",
            repo().join("scripts/fake-bin").display(),
            std::env::var("PATH").expect("PATH")
        );
        let placement = run("bash", &["-c", PLACEMENT_COMMAND], &run_dir, &path, None);
        let placement_text = output_text(&placement);
        assert!(
            placement.status.success(),
            "placement failed: {placement_text}"
        );
        assert_eq!(
            instance_in(&placement_text),
            LIVE,
            "placement:\n{placement_text}"
        );

        let script = repo().join("scripts/remote/gwin-keys");
        let script = script.to_str().expect("utf-8 script path");
        let keys = run("bash", &[script, "diag"], &run_dir, &path, None);
        let keys_text = output_text(&keys);
        assert!(keys.status.success(), "gwin-keys diag failed: {keys_text}");
        assert_eq!(
            instance_in(&keys_text),
            LIVE,
            "gwin-keys took a different instance than the newest socket:\n{keys_text}"
        );

        let kept = run(
            "bash",
            &["-c", PLACEMENT_COMMAND],
            &run_dir,
            &path,
            Some(DEAD),
        );
        let kept_text = output_text(&kept);
        assert!(
            !kept.status.success(),
            "a preset signature must stay on the previous instance: {kept_text}"
        );
        assert_eq!(
            instance_in(&kept_text),
            DEAD,
            "preset signature:\n{kept_text}"
        );
        assert!(
            !kept_text.contains("newest socket"),
            "picker replaced a set signature: {kept_text}"
        );

        let picker = std::fs::read_to_string(repo().join("scripts/remote/hypr-instance.sh"))
            .expect("scripts/remote/hypr-instance.sh");
        let keys_src =
            std::fs::read_to_string(repo().join("scripts/remote/gwin-keys")).expect("gwin-keys");
        assert!(
            keys_src.contains("hypr-instance.sh"),
            "gwin-keys sources the shared picker"
        );
        assert!(
            PLACEMENT_COMMAND.contains(picker.trim()),
            "the placement command is the shared picker"
        );
    }
}
