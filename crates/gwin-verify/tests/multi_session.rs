//! Three sessions in one window, end to end on the build machine: the real
//! `Tabs` and `SessionSet` over three real session drivers, each through
//! `scripts/fake-ssh` to its own `gwin-fake-server`, and each server draws a
//! different flat colour (`GWIN_FAKE_TINT`). The test thread stands in for the
//! UI thread.
//!
//! What it proves: every session decodes and acks while it is hidden; after a
//! switch the picture on screen is the new session's; and what three busy
//! sessions cost the client in CPU (printed, and kept in the test's temporary
//! folder for the docs).

// The loop runs through bash wrappers (`scripts/fake-ssh`): build machine only.
#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;
use std::time::Instant;

use gliff_windows::cli::DesktopMode;
use gliff_windows::config::TabSet;
use gliff_windows::history::HostTarget;
use gliff_windows::input::InputOptions;
use gliff_windows::keymap::Press;
use gliff_windows::pointer::PointerEvent;
use gliff_windows::route::InputRoute;
use gliff_windows::session::ResizeReq;
use gliff_windows::session::UiCmd;
use gliff_windows::session::UiEvent;
use gliff_windows::session::driver::Options;
use gliff_windows::session::driver::SessionHandle;
use gliff_windows::session::driver::Waker;
use gliff_windows::session::driver::start;
use gliff_windows::session_set::Launch;
use gliff_windows::session_set::Presented;
use gliff_windows::session_set::SessionSet;
use gliff_windows::ssh::SshSpec;
use gliff_windows::tabs::Status;
use gliff_windows::tabs::SwitchTo;
use gliff_windows::tabs::Tabs;

/// The test that reads this process's CPU time runs alone: every thread of
/// every other test in this process would be in the number. The others share
/// the gate and run together. Tests that share never queue behind a waiting
/// test that wants to be alone (a std `RwLock` makes them), so the alone test
/// waits for the ones already running and then the clock is quiet.
static CPU_CLOCK: Gate = Gate {
    state: Mutex::new(GateState {
        shared: 0,
        alone: false,
    }),
    changed: Condvar::new(),
};

struct GateState {
    shared: usize,
    alone: bool,
}

struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

/// Held while a test runs. Dropping it opens the gate again.
struct Held {
    alone: bool,
}

impl Gate {
    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn shared(&self) -> Held {
        let mut state = self.lock();
        while state.alone {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.shared += 1;
        Held { alone: false }
    }

    fn alone(&self) -> Held {
        let mut state = self.lock();
        while state.alone || state.shared > 0 {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.alone = true;
        Held { alone: true }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut state = CPU_CLOCK.lock();
        if self.alone {
            state.alone = false;
        } else {
            state.shared -= 1;
        }
        CPU_CLOCK.changed.notify_all();
    }
}

const SERVER: &str = env!("CARGO_BIN_EXE_gwin-fake-server");

const SIZE: ResizeReq = ResizeReq {
    width: 1280,
    height: 800,
    scale: 1.0,
};

/// The three machines and the grey each fake server draws.
const MACHINES: [(&str, u8); 3] = [("a@one", 40), ("b@two", 120), ("c@three", 220)];

struct Quiet;

impl Waker for Quiet {
    fn frame(&self) {}
    fn event(&self) {}
}

fn tint_of(target: &HostTarget) -> u8 {
    MACHINES
        .iter()
        .find(|(name, _)| *name == target.as_str())
        .expect("a known machine")
        .1
}

/// An ssh that runs the fake server with this session's tint.
fn ssh_for(dir: &Path, tint: u8, fault: &str) -> PathBuf {
    let fake_ssh = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh");
    let wrapper = dir.join("ssh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/usr/bin/env bash\nGWIN_FAKE_TINT={tint},{tint},{tint} GWIN_FAKE_SERVER={fault} exec {} \"$@\"\n",
            fake_ssh.display()
        ),
    )
    .expect("write ssh wrapper");
    let mut perms = std::fs::metadata(&wrapper).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&wrapper, perms).expect("chmod");
    wrapper
}

fn window(name: &str) -> (SessionSet<SessionHandle>, Arc<InputRoute<UiCmd>>) {
    window_with(name, "")
}

/// A window whose second machine's fake server has `fault` (for example
/// `reload`: it loses its output after a compositor reload, as a real one
/// can); the others are healthy.
fn window_with(
    name: &str,
    fault: &'static str,
) -> (SessionSet<SessionHandle>, Arc<InputRoute<UiCmd>>) {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("multi-{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("log dir");
    let route = Arc::new(InputRoute::new());
    let starter = Box::new(move |launch: Launch| {
        let dir = base.join(format!("s{}", launch.session.get()));
        std::fs::create_dir_all(&dir)?;
        let opts = Options {
            spec: SshSpec {
                ssh: ssh_for(
                    &dir,
                    tint_of(&launch.target),
                    if launch.target.as_str() == MACHINES[1].0 {
                        fault
                    } else {
                        ""
                    },
                ),
                target: launch.target.clone(),
                server_path: SERVER.to_owned(),
                mode: DesktopMode::Headless,
                log_dir: dir,
                log_tag: Some(launch.session.get()),
            },
            panel: (3840, 2400),
            initial: SIZE,
            input: InputOptions::default(),
        };
        start(opts, Arc::new(Quiet))
    });
    (SessionSet::new(route.clone(), None, starter), route)
}

fn targets() -> Vec<HostTarget> {
    MACHINES
        .iter()
        .map(|(name, _)| name.parse().expect("a target"))
        .collect()
}

/// The UI thread's loop: poll, and take the shown session's pictures (the
/// others' stay in their slots, as hidden tabs' do). Returns the luma of the
/// last picture taken.
fn pump(
    set: &mut SessionSet<SessionHandle>,
    tabs: &mut Tabs,
    shown: Option<gliff_windows::tabs::SessionId>,
    within: Duration,
    until: impl Fn(&Tabs, &SessionSet<SessionHandle>) -> bool,
) -> Option<u8> {
    let deadline = Instant::now() + within;
    let mut luma = None;
    while Instant::now() < deadline {
        set.poll(tabs);
        if let Some(picture) = shown
            .and_then(|id| set.get(id))
            .and_then(|link| link.slot().take())
        {
            let size = picture.frame.size();
            assert!(size.0 > 10 && size.1 > 10);
            luma = Some(picture.frame.y()[10 * size.0 as usize + 10]);
        }
        if until(tabs, set) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    luma
}

/// BT.601 limited-range luma of a grey level.
fn luma_of(grey: u8) -> i32 {
    16 + (i32::from(grey) * 219 + 127) / 255
}

fn frames_of(set: &SessionSet<SessionHandle>) -> Vec<(u64, u64)> {
    set.reports()
        .iter()
        .map(|r| (r.stats.frames, r.stats.acks))
        .collect()
}

fn cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat");
    let after = stat.rsplit_once(')').expect("comm").1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    // utime and stime are fields 14 and 15 of the line, 12 and 13 after comm.
    let ticks: f64 =
        fields[11].parse::<f64>().expect("utime") + fields[12].parse::<f64>().expect("stime");
    ticks / 100.0
}

#[test]
fn three_sessions_stream_while_hidden_and_a_switch_shows_the_new_ones_picture() {
    let _shared = CPU_CLOCK.shared();
    let (mut set, route) = window("switch");
    let (mut tabs, effects) = Tabs::start_up(&TabSet::default(), &targets());
    let applied = set.run(&mut tabs, effects);
    let a = match applied.presented {
        Some(Presented::Session(id)) => id,
        other => panic!("the first target is shown: {other:?}"),
    };
    assert_eq!(set.sessions().len(), 3);
    assert_eq!(route.target(), Some(a));

    let all_streaming = |tabs: &Tabs, _: &SessionSet<SessionHandle>| {
        tabs.iter().all(|tab| *tab.status() == Status::Streaming)
    };
    pump(
        &mut set,
        &mut tabs,
        Some(a),
        Duration::from_secs(30),
        all_streaming,
    );
    assert!(
        tabs.iter().all(|tab| *tab.status() == Status::Streaming),
        "all three connect: {:?}",
        tabs.iter().map(|t| t.status().clone()).collect::<Vec<_>>()
    );

    // Hidden sessions keep decoding and acking with nobody looking at them.
    let busy = |_: &Tabs, set: &SessionSet<SessionHandle>| {
        frames_of(set)
            .iter()
            .all(|&(frames, acks)| frames >= 30 && acks >= 28)
    };
    let shown_luma = pump(&mut set, &mut tabs, Some(a), Duration::from_secs(30), busy);
    assert!(
        busy(&tabs, &set),
        "every session streams: {:?}",
        frames_of(&set)
    );
    let want = luma_of(MACHINES[0].1);
    assert!(
        shown_luma.is_some_and(|l| (i32::from(l) - want).abs() <= 8),
        "the shown session draws its own grey: {shown_luma:?} want {want}"
    );

    // Switch to the second tab: the picture on screen is its picture.
    let effects = tabs.switch(SwitchTo::Next);
    let applied = set.run(&mut tabs, effects);
    let b = match applied.presented {
        Some(Presented::Session(id)) => id,
        other => panic!("a switch presents a session: {other:?}"),
    };
    assert_ne!(a, b);
    assert_eq!(route.target(), Some(b));
    let luma = pump(
        &mut set,
        &mut tabs,
        Some(b),
        Duration::from_secs(10),
        |_, _| false,
    );
    let want = luma_of(MACHINES[1].1);
    assert!(
        luma.is_some_and(|l| (i32::from(l) - want).abs() <= 8),
        "after the switch the picture is the second session's: {luma:?} want {want}"
    );

    let ended = set.shutdown();
    assert_eq!(ended.len(), 3);
    for (session, end) in ended {
        assert_eq!(end.ssh_exit, Some(0), "{session:?}: {end:?}");
        assert!(end.errors.is_empty(), "{session:?}: {end:?}");
        assert_eq!(
            end.stats.frames, end.stats.acks,
            "every frame acked: {end:?}"
        );
        assert_eq!(end.stats.decode_errors, 0, "{end:?}");
    }
}

fn click_thrice(route: &InputRoute<UiCmd>) {
    for _ in 0..3 {
        for press in [Press::Down, Press::Up] {
            route.send(UiCmd::Pointer(PointerEvent::Button {
                button: 0x110,
                press,
            }));
        }
    }
}

/// Polls like the UI thread, taking the shown session's pictures, and
/// collects the resync events of every session until `done` or `within`.
fn resync_events(
    set: &mut SessionSet<SessionHandle>,
    tabs: &mut Tabs,
    shown: gliff_windows::tabs::SessionId,
    within: Duration,
    done: impl Fn(&[(gliff_windows::tabs::SessionId, bool)]) -> bool,
) -> Vec<(gliff_windows::tabs::SessionId, bool)> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + within;
    while Instant::now() < deadline && !done(&seen) {
        for (session, event) in set.poll(tabs).events {
            match event {
                UiEvent::Resyncing => seen.push((session, true)),
                UiEvent::Resynced => seen.push((session, false)),
                _ => {}
            }
        }
        if let Some(link) = set.get(shown) {
            while link.slot().take().is_some() {}
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

#[test]
fn a_resync_belongs_to_the_session_that_went_quiet_and_only_when_input_reaches_it() {
    let _shared = CPU_CLOCK.shared();
    let (mut set, route) = window_with("resync", "reload");
    let two: Vec<HostTarget> = targets().into_iter().take(2).collect();
    let (mut tabs, effects) = Tabs::start_up(&TabSet::default(), &two);
    let applied = set.run(&mut tabs, effects);
    let a = match applied.presented {
        Some(Presented::Session(id)) => id,
        other => panic!("the first target is shown: {other:?}"),
    };
    let streaming = |tabs: &Tabs, _: &SessionSet<SessionHandle>| {
        tabs.iter().all(|tab| *tab.status() == Status::Streaming)
    };
    pump(
        &mut set,
        &mut tabs,
        Some(a),
        Duration::from_secs(30),
        streaming,
    );
    assert!(tabs.iter().all(|tab| *tab.status() == Status::Streaming));

    // The second server loses its output 1 s after its Resize. Input goes to
    // the shown, healthy session, so nothing resyncs: not the first (it
    // answers) and not the second (nothing reached it).
    std::thread::sleep(Duration::from_millis(1500));
    click_thrice(&route);
    let seen = resync_events(&mut set, &mut tabs, a, Duration::from_secs(5), |_| false);
    assert!(seen.is_empty(), "no session resyncs yet: {seen:?}");

    // Switch to the quiet one and click: it, and only it, resyncs.
    let effects = tabs.switch(SwitchTo::Next);
    let b = match set.run(&mut tabs, effects).presented {
        Some(Presented::Session(id)) => id,
        other => panic!("a switch presents a session: {other:?}"),
    };
    assert_ne!(a, b);
    click_thrice(&route);
    let seen = resync_events(&mut set, &mut tabs, b, Duration::from_secs(15), |seen| {
        seen.len() >= 2
    });
    assert_eq!(seen, vec![(b, true), (b, false)], "second session resyncs");
    let resyncs: Vec<(gliff_windows::tabs::SessionId, u64)> = set
        .reports()
        .iter()
        .map(|r| (r.session, r.stats.resyncs))
        .collect();
    assert_eq!(resyncs, vec![(a, 0), (b, 1)]);
    for (session, end) in set.shutdown() {
        assert_eq!(end.ssh_exit, Some(0), "{session:?}: {end:?}");
    }
}

#[test]
fn what_three_busy_sessions_cost_the_client_in_cpu() {
    let _alone = CPU_CLOCK.alone();
    let measure = |count: usize| {
        let (mut set, _route) = window(&format!("cpu{count}"));
        let targets: Vec<HostTarget> = targets().into_iter().take(count).collect();
        let (mut tabs, effects) = Tabs::start_up(&TabSet::default(), &targets);
        set.run(&mut tabs, effects);
        let shown = tabs.shown_session();
        let streaming = |tabs: &Tabs, _: &SessionSet<SessionHandle>| {
            tabs.iter().all(|tab| *tab.status() == Status::Streaming)
        };
        pump(
            &mut set,
            &mut tabs,
            shown,
            Duration::from_secs(30),
            streaming,
        );
        // Past the connect and the first key frames.
        let warm = |_: &Tabs, set: &SessionSet<SessionHandle>| {
            frames_of(set).iter().all(|&(frames, _)| frames >= 10)
        };
        pump(&mut set, &mut tabs, shown, Duration::from_secs(30), warm);
        let (cpu_start, wall_start) = (cpu_seconds(), Instant::now());
        let frames_start: u64 = frames_of(&set).iter().map(|f| f.0).sum();
        pump(
            &mut set,
            &mut tabs,
            shown,
            Duration::from_secs(6),
            |_, _| false,
        );
        let (cpu, wall) = (
            cpu_seconds() - cpu_start,
            wall_start.elapsed().as_secs_f64(),
        );
        let frames: u64 = frames_of(&set).iter().map(|f| f.0).sum::<u64>() - frames_start;
        for (_, end) in set.shutdown() {
            assert_eq!(end.stats.frames, end.stats.acks, "{end:?}");
        }
        assert!(frames > 0, "the sessions streamed during the window");
        (
            cpu / wall * 100.0,
            frames as f64 / wall,
            cpu * 1000.0 / frames as f64,
        )
    };
    let one = measure(1);
    let three = measure(3);
    let line = format!(
        "client CPU at 1280x800, fake server pattern, {} build:\n\
         1 session : {:.1}% of one core, {:.1} frames/s, {:.2} ms CPU per frame\n\
         3 sessions: {:.1}% of one core, {:.1} frames/s, {:.2} ms CPU per frame\n",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        one.0,
        one.1,
        one.2,
        three.0,
        three.1,
        three.2
    );
    println!("{line}");
    std::fs::write(
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("multi-session-cpu.txt"),
        &line,
    )
    .expect("record the measurement");
    assert!(
        three.1 > one.1,
        "three sessions decode more frames per second than one"
    );
}
