//! `scripts/remote/gwin-keys` on the build machine, against the fake
//! `hyprctl` (`scripts/fake-bin/hyprctl`). The fake keeps each lua chunk the
//! script sends; the test replays the `exec` binds from it, as Hyprland
//! would on a key press, and reads the log the way `gwin-verify` does.

// Bash scripts and fakes: build machine only.
#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

const SIG: &str = "a1b2c3_1761250000_200";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A remote machine in a temp dir: the fakes first on PATH, an interactive
/// session's Hyprland environment, and a gliff output at (100, 50).
struct Remote {
    dir: Scratch,
}

/// A scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("gwin-keys-{}-{n}", std::process::id()));
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

impl Remote {
    fn new() -> Self {
        let dir = Scratch::new();
        let run = dir.path().join("run");
        let instance = run.join("hypr").join(SIG);
        std::fs::create_dir_all(&instance).expect("instance dir");
        std::fs::write(instance.join(".socket.sock"), "").expect("socket");
        let hypr = dir.path().join("hypr-world");
        std::fs::create_dir_all(&hypr).expect("world");
        std::fs::write(
            hypr.join("monitors.json"),
            r#"[{"id":0,"name":"eDP-1","width":2560,"height":1600,"x":0,"y":0,"scale":1.0},
                {"id":1,"name":"gliff-7","width":2560,"height":1600,"x":100,"y":50,"scale":2.0}]"#,
        )
        .expect("monitors");
        Remote { dir }
    }

    fn hypr(&self) -> PathBuf {
        self.dir.path().join("hypr-world")
    }

    fn script(&self, args: &[&str]) -> Output {
        self.script_at(&repo().join("scripts/remote/gwin-keys"), args)
    }

    fn script_at(&self, script: &Path, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            repo().join("scripts/fake-bin").display(),
            std::env::var("PATH").expect("PATH")
        );
        Command::new("bash")
            .arg(script)
            .args(args)
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("HYPRLAND_INSTANCE_SIGNATURE", SIG)
            .env("GWIN_FAKE_HYPR_DIR", self.hypr())
            .env(
                "GWIN_KEYS_CHORDS",
                repo().join("docs/phase4/super-chords.tsv"),
            )
            .output()
            .expect("run gwin-keys")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.script(args);
        assert!(
            out.status.success(),
            "gwin-keys {args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf8")
    }

    /// The lua chunks the script sent with `hyprctl eval`, oldest first.
    fn evals(&self) -> Vec<String> {
        let dir = self.hypr().join("evals");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|d| d.map(|e| e.expect("entry").path()).collect())
            .unwrap_or_default();
        files.sort();
        files
            .iter()
            .map(|f| std::fs::read_to_string(f).expect("read eval"))
            .collect()
    }

    /// Presses a bind: runs the `exec_cmd` of the line whose keys are
    /// exactly `keys`, in the first (arming) chunk.
    fn press(&self, keys: &str) {
        let lua = &self.evals()[0];
        let prefix = format!("hl.bind(\"{keys}\", hl.dsp.exec_cmd(\"");
        let line = lua
            .lines()
            .find(|l| l.starts_with(&prefix))
            .unwrap_or_else(|| panic!("no bind for `{keys}` in:\n{lua}"));
        // The lua string literal, read back: `\\` and `\"` are one character.
        let cmd = line[prefix.len()..]
            .split("\")")
            .next()
            .expect("the command")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        let out = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    repo().join("scripts/fake-bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            )
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("HYPRLAND_INSTANCE_SIGNATURE", SIG)
            .env("GWIN_FAKE_HYPR_DIR", self.hypr())
            .output()
            .expect("run the bind");
        assert!(
            out.status.success(),
            "bind `{keys}` ({cmd}) failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn lines(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

#[test]
fn a_path_with_quotes_and_a_backslash_still_runs_its_binds_and_its_timer() {
    let remote = Remote::new();
    let dir = remote.dir.path().join("it's \"q\" b\\s");
    std::fs::create_dir_all(&dir).expect("odd dir");
    let copy = dir.join("gwin-keys");
    std::fs::copy(repo().join("scripts/remote/gwin-keys"), &copy).expect("copy");
    let armed = remote.script_at(&copy, &["arm", "--minutes", "1"]);
    assert!(
        armed.status.success(),
        "arm failed: {}{}",
        String::from_utf8_lossy(&armed.stdout),
        String::from_utf8_lossy(&armed.stderr)
    );
    remote.press("SUPER + E");
    let log = remote.script_at(&copy, &["log"]);
    let all = lines(&String::from_utf8(log.stdout).expect("utf8"));
    assert_eq!(
        all.iter().map(|f| f[2].as_str()).collect::<Vec<_>>(),
        ["SUPER+E"],
        "the bind ran the copy in the odd directory: {all:?}"
    );
    // The timer is a bash with the script's path as an argument of its own.
    let state =
        std::fs::read_to_string(remote.dir.path().join("run/gwin-keys.state")).expect("state");
    let pid = state
        .lines()
        .find_map(|l| l.strip_prefix("timer="))
        .expect("a timer pid");
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
    let want = std::fs::canonicalize(&copy).expect("path");
    assert!(
        args.iter().any(|a| *a == want.to_string_lossy().as_bytes()),
        "the timer got the path as one argument: {:?}",
        String::from_utf8_lossy(&cmdline)
    );
    let out = remote.script_at(&copy, &["disarm"]);
    assert!(out.status.success());
}

#[test]
fn selftest_passes() {
    let remote = Remote::new();
    let out = remote.ok(&["selftest"]);
    assert!(out.contains("selftest ok"), "selftest said: {out}");
}

#[test]
fn arm_enters_a_fresh_submap_and_refuses_a_second_arm() {
    let remote = Remote::new();
    let out = remote.ok(&["arm", "--minutes", "1"]);
    assert!(out.contains("armed: submap gwin-test-"), "arm said: {out}");
    let lua = &remote.evals()[0];
    assert!(
        lua.starts_with("hl.define_submap(\"gwin-test-"),
        "the chunk opens a submap: {lua}"
    );
    for want in [
        "SUPER + SUPER_L\", hl.dsp.exec_cmd(\"'",
        "SUPER + SHIFT + F12",
        "hl.on(\"input.keyboard.key\"",
        "mouse_down",
        "CTRL + Q",
    ] {
        assert!(lua.contains(want), "the chunk lacks `{want}`:\n{lua}");
    }
    for unwanted in ["catchall", "Control_L", "Alt_L", "SUPER + Y", "MOD5"] {
        assert!(
            !lua.contains(unwanted),
            "the chunk must not have `{unwanted}`:\n{lua}"
        );
    }
    let submaps = std::fs::read_to_string(remote.hypr().join("submaps.log")).expect("submaps");
    assert!(
        submaps.contains("hl.dsp.submap(\"gwin-test-"),
        "it entered the submap: {submaps}"
    );
    let second = remote.script(&["arm"]);
    assert!(!second.status.success(), "a second arm is refused");
    remote.ok(&["disarm"]);
}

#[test]
fn a_failed_eval_leaves_nothing_armed() {
    let remote = Remote::new();
    std::fs::write(remote.hypr().join("eval-fails"), "").expect("flag");
    let out = remote.script(&["arm"]);
    assert!(!out.status.success(), "arm fails when hyprctl eval fails");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("arm FAILED"),
        "it says so: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_file(remote.hypr().join("eval-fails")).expect("remove flag");
    remote.ok(&["arm", "--minutes", "1"]);
    remote.ok(&["disarm"]);
}

#[test]
fn binds_log_numbered_lines_and_the_pointer_relative_to_the_output() {
    let remote = Remote::new();
    remote.ok(&["arm", "--minutes", "1"]);
    remote.press("SUPER + E");
    remote.press("SUPER + SUPER_L");
    std::fs::write(remote.hypr().join("cursor.json"), r#"{"x":150,"y":250}"#).expect("cursor");
    remote.press("mouse:272");
    remote.press("mouse_down");
    remote.ok(&["_key", "down", "56"]);
    let all = lines(&remote.ok(&["log"]));
    let whats: Vec<&str> = all.iter().map(|f| f[2].as_str()).collect();
    assert_eq!(
        whats,
        [
            "SUPER+E",
            "SUPER alone",
            "mouse:272 50,200",
            "wheel:down",
            "key:down:56"
        ],
        "what fired, in order: {all:?}"
    );
    let ns: Vec<&str> = all.iter().map(|f| f[1].as_str()).collect();
    assert_eq!(ns, ["1", "2", "3", "4", "5"], "the log's own numbers");
    assert!(
        all.iter().all(|f| f.len() == 3 && f[0].contains(':')),
        "ts<TAB>n<TAB>what: {all:?}"
    );
    let since = lines(&remote.ok(&["log", "--since", "3"]));
    assert_eq!(since.len(), 2, "two entries after number 3: {since:?}");
    remote.ok(&["disarm"]);
}

#[test]
fn disarm_resets_the_submap_unbinds_and_deletes_the_log() {
    let remote = Remote::new();
    remote.ok(&["arm", "--minutes", "1"]);
    remote.press("SUPER + E");
    let out = remote.ok(&["disarm"]);
    assert!(out.contains("SUPER+E"), "disarm prints the log: {out}");
    let submaps = std::fs::read_to_string(remote.hypr().join("submaps.log")).expect("submaps");
    assert!(
        submaps.contains("hl.dsp.submap(\"reset\")"),
        "it left the submap: {submaps}"
    );
    let evals = remote.evals();
    assert_eq!(evals.len(), 3, "arm, take the key tap off, unbind");
    assert!(
        evals[1].contains("gwin_keys_tap:remove()"),
        "the key tap is removed on its own: {}",
        evals[1]
    );
    let binds = evals[0].matches("hl.bind(").count();
    let unbinds = evals[2].matches("hl.unbind(").count();
    assert_eq!(unbinds, binds, "every bind is unbound again");
    assert_eq!(remote.ok(&["log"]), "", "the log is deleted by disarm");
    remote.ok(&["arm", "--minutes", "1"]);
    remote.ok(&["disarm"]);
}

#[test]
fn key_events_are_logged_only_while_armed() {
    let remote = Remote::new();
    remote.ok(&["_key", "down", "56"]);
    assert_eq!(
        remote.ok(&["log"]),
        "",
        "nothing is armed: nothing is logged"
    );
    remote.ok(&["arm", "--minutes", "1"]);
    remote.ok(&["_key", "down", "125"]);
    remote.ok(&["_key", "up", "125"]);
    let whats: Vec<String> = lines(&remote.ok(&["log"]))
        .into_iter()
        .map(|f| f[2].clone())
        .collect();
    assert_eq!(
        whats,
        ["key:down:125", "key:up:125"],
        "evdev codes, in order"
    );
    remote.ok(&["disarm"]);
    remote.ok(&["_key", "down", "56"]);
    assert_eq!(
        remote.ok(&["log"]),
        "",
        "a key after the disarm is not logged"
    );
}

#[test]
fn a_failed_arm_takes_the_key_tap_off_again() {
    let remote = Remote::new();
    std::fs::write(remote.hypr().join("eval-fails"), "").expect("flag");
    remote.script(&["arm"]);
    std::fs::remove_file(remote.hypr().join("eval-fails")).expect("remove flag");
    let evals = remote.evals();
    assert!(
        evals.iter().any(|e| e.contains("gwin_keys_tap:remove()")),
        "a half-installed tap is removed: {evals:?}"
    );
}

#[test]
fn the_typed_stream_logs_keysyms_of_presses_and_stops_at_disarm() {
    let remote = Remote::new();
    std::fs::write(remote.hypr().join("typed"), "").expect("typed flag");
    remote.ok(&["arm", "--minutes", "1"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let want = ["typed:z", "typed:at"];
    let whats = loop {
        let whats: Vec<String> = lines(&remote.ok(&["log"]))
            .into_iter()
            .map(|f| f[2].clone())
            .collect();
        if whats == want || std::time::Instant::now() > deadline {
            break whats;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(
        whats, want,
        "a press logs its keysym; a modifier or a release does not"
    );
    remote.ok(&["disarm"]);
    let state = remote.dir.path().join("run/gwin-keys.state");
    assert!(!state.exists(), "disarmed");
}

#[test]
fn both_pairs_of_side_buttons_have_a_bind() {
    let remote = Remote::new();
    remote.ok(&["arm", "--minutes", "1"]);
    for code in [275, 276, 277, 278] {
        remote.press(&format!("mouse:{code}"));
    }
    let whats: Vec<String> = lines(&remote.ok(&["log"]))
        .into_iter()
        .map(|f| f[2].clone())
        .collect();
    assert_eq!(whats.len(), 4, "one entry per button: {whats:?}");
    for (got, code) in whats.iter().zip([275, 276, 277, 278]) {
        assert!(got.starts_with(&format!("mouse:{code} ")), "{got}");
    }
    remote.ok(&["disarm"]);
}

#[test]
fn focus_says_when_wev_has_the_focus() {
    let remote = Remote::new();
    let out = remote.ok(&["focus", "--tries", "3"]);
    assert!(
        out.contains("wev is the active window"),
        "focus said: {out}"
    );
}

#[test]
fn focus_that_does_not_stick_fails_and_prints_the_diagnosis() {
    let remote = Remote::new();
    std::fs::write(remote.hypr().join("focus-fails"), "").expect("flag");
    let out = remote.script(&["focus", "--tries", "2"]);
    assert!(!out.status.success(), "no focus is a failure");
    let text = String::from_utf8_lossy(&out.stdout);
    for want in [
        "focus FAILED",
        "active window is 'none'",
        "hyprctl -j activewindow",
        "hyprctl -j activeworkspace",
        "hyprctl -j workspaces",
        "hyprctl -j monitors all",
        "hyprctl -j clients",
        "scroll_event_delay",
    ] {
        assert!(text.contains(want), "the output lacks `{want}`:\n{text}");
    }
}

#[test]
fn focus_rejects_a_bad_try_count() {
    let remote = Remote::new();
    let out = remote.script(&["focus", "--tries", "x"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn check_says_the_armed_submap_is_active() {
    let remote = Remote::new();
    let out = remote.script(&["check"]);
    assert!(!out.status.success(), "nothing is armed yet");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("not armed"),
        "said: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    remote.ok(&["arm"]);
    let out = remote.ok(&["check"]);
    assert!(
        out.contains("gwin-test-") && out.contains("is active"),
        "said: {out}"
    );
    remote.ok(&["disarm"]);
}

#[test]
fn check_fails_when_hyprland_is_not_in_the_armed_submap() {
    let remote = Remote::new();
    remote.ok(&["arm"]);
    std::fs::write(remote.hypr().join("submap-stuck"), "").expect("flag");
    let out = remote.script(&["check"]);
    assert!(!out.status.success(), "default is not the test submap");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("check FAILED") && text.contains("'default'"),
        "said: {text}"
    );
    remote.ok(&["disarm"]);
}
