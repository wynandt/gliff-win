//! A tab whose session ends, end to end on the build machine: the real
//! `Tabs` and `SessionSet` over real session drivers. Each machine's `ssh` is
//! a script that follows a plan, one action per attempt (the last repeats):
//!
//! - `ok`: the fake server, as `scripts/fake-ssh` runs it;
//! - `die:N`: the fake server, killed after N seconds (a server that dies);
//! - `refused`, `key_refused`, `host_changed`: real ssh output from
//!   `crates/gliff-windows/tests/ssh_corpus`, exit 255.
//!
//! The delays are short (see [`FAST`]) so a test takes seconds, not minutes;
//! the schedule itself is the model's, tested with a fake clock in
//! `gliff-windows`. What these tests add is the real thing around it: a
//! session that dies for real, the classified failure of real ssh text, and
//! a close while a tab waits. Each prints the tab's status as it changed and
//! the attempts the scripts saw (run with `--nocapture` to read them).

// The loop runs through bash wrappers (`scripts/fake-ssh`): build machine only.
#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gliff_windows::cli::DesktopMode;
use gliff_windows::config::TabSet;
use gliff_windows::history::HostTarget;
use gliff_windows::input::InputOptions;
use gliff_windows::reconnect::Policy;
use gliff_windows::route::InputRoute;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::driver::Options;
use gliff_windows::session::driver::SessionHandle;
use gliff_windows::session::driver::Waker;
use gliff_windows::session::driver::start;
use gliff_windows::session_set::Launch;
use gliff_windows::session_set::SessionSet;
use gliff_windows::ssh::SshSpec;
use gliff_windows::tabs::Status;
use gliff_windows::tabs::Tabs;

const SERVER: &str = env!("CARGO_BIN_EXE_gwin-fake-server");

const SIZE: ResizeReq = ResizeReq {
    width: 640,
    height: 400,
    scale: 1.0,
};

/// Short delays: 100 ms, 200, 400, 400, 400, and five retries as in the
/// real policy. A session counts as stable after 3 s.
const FAST: Policy = Policy {
    first_delay: Duration::from_millis(100),
    max_delay: Duration::from_millis(400),
    max_retries: 5,
    stable_after: Duration::from_secs(3),
};

struct Quiet;

impl Waker for Quiet {
    fn frame(&self) {}
    fn event(&self) {}
}

/// The ssh script of one machine. It counts its calls in `<dir>/count`,
/// reads its action from `<dir>/plan`, and logs each attempt to `<dir>/log`.
fn write_ssh(dir: &Path, plan: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus = root.join("crates/gliff-windows/tests/ssh_corpus/openssh-10.0p2-linux");
    std::fs::create_dir_all(dir).expect("machine dir");
    std::fs::write(dir.join("plan"), plan).expect("plan");
    let wrapper = dir.join("ssh");
    std::fs::write(
        &wrapper,
        format!(
            r#"#!/usr/bin/env bash
dir="{dir}"
n=$(( $(cat "$dir/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$dir/count"
read -ra plan < "$dir/plan"
i=$(( n - 1 ))
(( i >= ${{#plan[@]}} )) && i=$(( ${{#plan[@]}} - 1 ))
act="${{plan[$i]}}"
echo "attempt $n: $act" >> "$dir/log"
export GWIN_FAKE_TINT=90,90,90
case "$act" in
  ok) exec "{fake}" "$@" ;;
  die:*) exec timeout --signal=KILL "${{act#die:}}" "{fake}" "$@" ;;
  *) GWIN_FAKE_SSH_STDERR="{corpus}/$act.stderr" GWIN_FAKE_SSH_EXIT=255 exec "{fake}" "$@" ;;
esac
"#,
            dir = dir.display(),
            fake = root.join("scripts/fake-ssh").display(),
            corpus = corpus.display(),
        ),
    )
    .expect("write ssh");
    let mut perms = std::fs::metadata(&wrapper).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&wrapper, perms).expect("chmod");
    wrapper
}

/// A window of machines with plans, and the UI thread's loop.
struct Rig {
    set: SessionSet<SessionHandle>,
    tabs: Tabs,
    route: Arc<InputRoute<UiCmd>>,
    base: PathBuf,
    names: Vec<String>,
    began: Instant,
    last: Vec<String>,
}

impl Rig {
    fn new(name: &str, machines: &[(&str, &str)]) -> Self {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("reconnect-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        let mut ssh = std::collections::BTreeMap::new();
        for (target, plan) in machines {
            ssh.insert(
                (*target).to_owned(),
                write_ssh(&base.join(target.replace('@', "-")), plan),
            );
        }
        let route = Arc::new(InputRoute::new());
        let starter = {
            let base = base.clone();
            Box::new(move |launch: Launch| {
                let dir = base.join(format!("log-s{}", launch.session.get()));
                std::fs::create_dir_all(&dir)?;
                let opts = Options {
                    spec: SshSpec {
                        ssh: ssh[launch.target.as_str()].clone(),
                        target: launch.target.clone(),
                        server_path: SERVER.to_owned(),
                        mode: DesktopMode::Headless,
                        log_dir: dir,
                        log_tag: Some(launch.session.get()),
                    },
                    panel: (1920, 1200),
                    initial: SIZE,
                    input: InputOptions::default(),
                };
                start(opts, Arc::new(Quiet))
            })
        };
        let set = SessionSet::new(route.clone(), None, starter);
        let targets: Vec<HostTarget> = machines
            .iter()
            .map(|(target, _)| target.parse().expect("a target"))
            .collect();
        let (tabs, effects) = Tabs::start_up(&TabSet::default(), &targets);
        let tabs = tabs.with_policy(FAST);
        let mut rig = Self {
            set,
            tabs,
            route,
            base,
            names: machines.iter().map(|(t, _)| (*t).to_owned()).collect(),
            began: Instant::now(),
            last: Vec::new(),
        };
        rig.set.run(&mut rig.tabs, effects);
        rig.note();
        rig
    }

    fn status(&self, target: &str) -> Status {
        self.tabs
            .iter()
            .find(|tab| tab.target().as_str() == target)
            .expect("the tab")
            .status()
            .clone()
    }

    fn describe(&self, target: &str) -> String {
        match self.status(target) {
            Status::Reconnecting {
                attempt,
                of,
                why,
                at,
            } => format!(
                "reconnecting, retry {attempt} of {of} {} ({why})",
                if at.is_some() { "waiting" } else { "running" }
            ),
            other => format!("{other:?}"),
        }
    }

    /// Prints a line for each tab whose status changed.
    fn note(&mut self) {
        let now: Vec<String> = self.names.iter().map(|n| self.describe(n)).collect();
        let ms = self.began.elapsed().as_millis();
        for (i, line) in now.iter().enumerate() {
            if self.last.get(i) != Some(line) {
                println!("[{ms:>5} ms] {}: {line}", self.names[i]);
            }
        }
        self.last = now;
    }

    /// Polls like the UI thread until `until` holds or `within` passes.
    fn run(&mut self, within: Duration, until: impl Fn(&Self) -> bool) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            self.set.poll(&mut self.tabs);
            for session in self.set.sessions() {
                if let Some(link) = self.set.get(session) {
                    while link.slot().take().is_some() {}
                }
            }
            self.note();
            if until(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// How many times this machine's ssh was run.
    fn attempts(&self, target: &str) -> usize {
        let dir = self.base.join(target.replace('@', "-"));
        // The fake ssh rewrites the file in place, so a read can land between
        // the truncate and the write and see nothing: read again.
        for _ in 0..50 {
            match std::fs::read_to_string(dir.join("count")) {
                Err(_) => return 0,
                Ok(text) => {
                    if let Ok(count) = text.trim().parse() {
                        return count;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the attempt count never became readable");
    }

    fn print_attempts(&self) {
        for target in &self.names {
            let dir = self.base.join(target.replace('@', "-"));
            let log = std::fs::read_to_string(dir.join("log")).unwrap_or_default();
            for line in log.lines() {
                println!("  ssh of {target}: {line}");
            }
        }
    }

    fn finish(mut self) {
        self.print_attempts();
        self.set.shutdown();
    }
}

fn streaming(target: &'static str) -> impl Fn(&Rig) -> bool {
    move |rig| rig.status(target) == Status::Streaming
}

fn gave_up(rig: &Rig, target: &str) -> Option<String> {
    match rig.status(target) {
        Status::GaveUp(text) => Some(text),
        _ => None,
    }
}

fn waiting(rig: &Rig, target: &str) -> bool {
    matches!(rig.status(target), Status::Reconnecting { at: Some(_), .. })
}

#[test]
fn a_server_that_dies_is_reconnected_and_streams_again() {
    let mut rig = Rig::new("dies", &[("a@one", "die:5 ok")]);
    assert!(rig.run(Duration::from_secs(30), streaming("a@one")), "up");
    assert!(
        rig.run(Duration::from_secs(30), |r| waiting(r, "a@one")),
        "the server dies and the tab waits"
    );
    assert!(
        rig.run(Duration::from_secs(30), |r| r.attempts("a@one") == 2
            && streaming("a@one")(r)),
        "the retry connects and streams: {}",
        rig.describe("a@one")
    );
    assert_eq!(rig.attempts("a@one"), 2);
    rig.finish();
}

#[test]
fn ssh_refused_twice_then_recovering_is_reconnected() {
    let mut rig = Rig::new("refused", &[("a@one", "refused refused ok")]);
    assert!(
        rig.run(Duration::from_secs(30), streaming("a@one")),
        "recovers: {}",
        rig.describe("a@one")
    );
    assert_eq!(rig.attempts("a@one"), 3, "two refusals, then the connect");
    rig.finish();
}

#[test]
fn a_refused_key_is_not_retried() {
    let mut rig = Rig::new("key", &[("a@one", "key_refused")]);
    assert!(rig.run(Duration::from_secs(30), |r| gave_up(r, "a@one").is_some()));
    // Far longer than the schedule's whole span (about 1.5 s).
    rig.run(Duration::from_secs(4), |_| false);
    assert_eq!(rig.attempts("a@one"), 1, "no automatic retry");
    let text = gave_up(&rig, "a@one").expect("still failed");
    assert!(text.contains("did not accept a key"), "{text}");

    let id = rig
        .tabs
        .iter()
        .find(|tab| tab.target().as_str() == "a@one")
        .expect("the tab")
        .id();
    // A click on a tab that needs the person only selects it.
    let effects = rig.tabs.click(id);
    rig.set.run(&mut rig.tabs, effects);
    rig.run(Duration::from_secs(2), |_| false);
    assert_eq!(rig.attempts("a@one"), 1, "a click does not retry");

    // "Try again" is the person's retry: one more attempt, again not repeated.
    let effects = rig.tabs.reconnect(id);
    rig.set.run(&mut rig.tabs, effects);
    assert!(rig.run(Duration::from_secs(30), |r| gave_up(r, "a@one").is_some()));
    rig.run(Duration::from_secs(3), |_| false);
    assert_eq!(rig.attempts("a@one"), 2);
    rig.finish();
}

#[test]
fn a_changed_host_key_is_not_retried() {
    let mut rig = Rig::new("hostkey", &[("a@one", "host_changed")]);
    assert!(rig.run(Duration::from_secs(30), |r| gave_up(r, "a@one").is_some()));
    rig.run(Duration::from_secs(3), |_| false);
    assert_eq!(rig.attempts("a@one"), 1);
    rig.finish();
}

#[test]
fn closing_the_tab_while_it_waits_stops_the_retries() {
    let mut rig = Rig::new("stop", &[("a@one", "refused")]);
    assert!(rig.run(Duration::from_secs(30), |r| waiting(r, "a@one")));
    let id = rig
        .tabs
        .iter()
        .find(|tab| tab.target().as_str() == "a@one")
        .expect("the tab")
        .id();
    let effects = rig.tabs.stop(id);
    rig.set.run(&mut rig.tabs, effects);
    let before = rig.attempts("a@one");
    rig.run(Duration::from_secs(4), |_| false);
    assert_eq!(rig.attempts("a@one"), before, "nothing starts after stop");
    assert_eq!(rig.status("a@one"), Status::Stopped);
    rig.finish();
}

#[test]
fn closing_the_window_while_a_tab_waits_stops_the_retries() {
    let mut rig = Rig::new("close", &[("a@one", "refused")]);
    assert!(rig.run(Duration::from_secs(30), |r| waiting(r, "a@one")));
    rig.set.close_all();
    let before = rig.attempts("a@one");
    rig.run(Duration::from_secs(4), |_| false);
    assert_eq!(rig.attempts("a@one"), before);
    rig.finish();
}

#[test]
fn a_server_that_connects_and_drops_at_once_gives_up_instead_of_looping() {
    let mut rig = Rig::new("flap", &[("a@one", "die:1")]);
    assert!(
        rig.run(Duration::from_secs(60), |r| gave_up(r, "a@one").is_some()),
        "{}",
        rig.describe("a@one")
    );
    let text = gave_up(&rig, "a@one").expect("gave up");
    assert!(text.ends_with("; gave up after 5 retries"), "{text}");
    rig.run(Duration::from_secs(3), |_| false);
    assert_eq!(rig.attempts("a@one"), 6, "the first try and five retries");
    rig.finish();
}

#[test]
fn a_hidden_tab_reconnects_and_the_shown_tab_keeps_the_keys() {
    let mut rig = Rig::new("hidden", &[("a@one", "ok"), ("b@two", "die:5 ok")]);
    assert!(rig.run(Duration::from_secs(30), |r| {
        streaming("a@one")(r) && streaming("b@two")(r)
    }));
    let shown = rig.tabs.shown_session().expect("a is shown");
    assert!(rig.run(Duration::from_secs(30), |r| waiting(r, "b@two")));
    assert_eq!(rig.route.target(), Some(shown), "the keys stay on a");
    assert!(rig.run(Duration::from_secs(30), |r| {
        r.attempts("b@two") == 2 && streaming("b@two")(r)
    }));
    assert_eq!(rig.route.target(), Some(shown), "and still do");
    assert_eq!(rig.tabs.shown_session(), Some(shown));
    assert_eq!(rig.attempts("a@one"), 1, "the shown tab never restarted");
    rig.finish();
}
