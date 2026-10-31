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
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
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
/// Used by the Win32 harness, so dead on the build machine outside tests.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
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
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
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
        // A real failure: fullscreen on monitor 0 while the output was monitor 2.
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

    /// A real present run's sizes: a Resize of 2560x1600 at 2.0 is a
    /// 2560x1600 mode at scale 2 (gliff-server session.rs:640-650), so the
    /// covering window is 1280x800.
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
