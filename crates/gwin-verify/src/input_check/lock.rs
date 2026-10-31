//! The lock-setting step, L1 to L7: the one
//! maintainer-approved run in which the *client* (never gwin-verify) holds
//! the per-user `DisableLockWorkstation` value. gwin-verify only reads the
//! registry; every write is the client's, and the end state is checked
//! against the value read first.
//!
//! Generic over [`LockHarness`] so the build machine runs the whole
//! sequence against a fake registry; `win32::Win32Lock` is the real one.

use std::time::Duration;

use super::cases::Place;
use super::cases::Window;
use super::remote_log::Entry;
use super::remote_log::is_bind;
use super::remote_log::last_n;
use super::remote_log::since;
use super::run::CaseResult;

/// What the registry holds about the lock setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// `DisableLockWorkstation`, or `None` when the value is absent.
    pub value: Option<u32>,
    /// The write-ahead marker under `HKCU\Software\gliff-windows\LockPolicy`.
    pub marker: bool,
}

/// How a client is killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kill {
    /// `taskkill /F` on the client alone: the watchdog survives.
    Client,
    /// `taskkill /F /T` on the client and every child.
    Tree,
}

/// What the step does to the machine. Nothing here writes the registry.
pub trait LockHarness {
    fn read_policy(&mut self) -> anyhow::Result<Reading>;
    /// Starts the client with the extra arguments and waits for its window.
    fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()>;
    fn focus(&mut self, window: Window) -> anyhow::Result<()>;
    fn pointer(&mut self, place: Place) -> anyhow::Result<()>;
    fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>>;
    /// Makes sure the remote machine is in the test submap (arming it if need be) and
    /// verifies it. `Ok(true)` when this call armed it, `Ok(false)` when it
    /// already was; an error says what the remote machine reported.
    fn arm_remote(&mut self) -> anyhow::Result<bool>;
    /// Whether the remote machine is still in the test submap right now.
    fn remote_submap_ok(&mut self) -> bool;
    /// Takes the remote machine out of the test submap again.
    fn disarm_remote(&mut self);
    /// Shows the instruction with the seconds left, on a window that never
    /// takes the focus, and returns when the time is up. The step does not
    /// wait for an Enter.
    fn countdown(&mut self, instruction: &str, secs: u32);
    /// Injects the client's release chord, so no prompt leaves it grabbing.
    fn release_grab(&mut self) -> anyhow::Result<()>;
    /// Waits up to `max` for the Windows machine to be unlocked again.
    fn wait_unlocked(&mut self, max: Duration) -> bool;
    /// Asks a yes or no question.
    fn ask(&mut self, text: &str) -> bool;
    fn session_active(&mut self) -> bool;
    fn foreground_is_client(&mut self) -> bool;
    fn client_running(&mut self) -> bool;
    fn watchdog_running(&mut self) -> bool;
    /// `WM_CLOSE`, then waits for the exit.
    fn close_client(&mut self) -> anyhow::Result<()>;
    /// Injects the client's panic test key (`VK_F23`).
    fn panic_client(&mut self) -> anyhow::Result<()>;
    fn kill(&mut self, how: Kill) -> anyhow::Result<()>;
    /// Runs `gliff-windows.exe --restore-lock-policy`.
    fn restore_flag(&mut self) -> anyhow::Result<()>;
    /// Notes the Explorer windows that are open now, before Win+E is pressed,
    /// and starts watching for new ones until `explorer_opened_and_closed`.
    fn note_explorer_windows(&mut self);
    /// Presses nothing: stops the watch and reports whether any Explorer
    /// window that was not among the noted ones showed up since (it may have
    /// opened and gone during the step). Each new window is closed as soon as
    /// it is seen.
    fn explorer_opened_and_closed(&mut self) -> bool;
    fn sleep(&mut self, d: Duration);
}

/// Time the client gets to set or put back the value (one tick is 125 ms;
/// the watchdog polls faster than 2 s).
const SETTLE: Duration = Duration::from_millis(1500);
const WATCHDOG: Duration = Duration::from_millis(2500);
/// A restore that has not happened yet is looked for again this often, for
/// this long, before it is called missing.
const RESTORE_POLL: Duration = Duration::from_millis(250);
const RESTORE_GRACE: Duration = Duration::from_millis(3000);
const RESTORE_POLLS: u32 = (RESTORE_GRACE.as_millis() / RESTORE_POLL.as_millis()) as u32;
/// A placement (client in front, or the text field in front of it) is looked at
/// again this many times before it is called lost.
const PLACE_TRIES: u32 = 3;

/// The client lets go of any grab this long after it starts or re-arms,
/// whatever happens to this process, so a lost run cannot hold the keyboard.
const GRAB_TIMEOUT_SECS: u32 = 120;
/// Seconds the maintainer gets for a press.
const PRESS_SECS: u32 = 20;
const PRESS_TWO_SECS: u32 = 30;
/// L5 locks the machine: more time, and a longer wait to be unlocked.
const LOCK_PRESS_SECS: u32 = 45;
const UNLOCK_WAIT: Duration = Duration::from_secs(180);

fn registry_flags() -> Vec<String> {
    vec![
        "--super-l".to_owned(),
        "registry".to_owned(),
        "--grab-timeout-secs".to_owned(),
        GRAB_TIMEOUT_SECS.to_string(),
    ]
}

/// The registry route plus the client's panic test (an injected F23 makes
/// its hook thread panic).
fn panic_flags() -> Vec<String> {
    let mut flags = registry_flags();
    flags.push("--test-panic".to_owned());
    flags
}

/// Which window a placement puts in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Front {
    /// The client.
    Client,
    /// The gwin-verify text field, with the pointer off the client.
    TextField,
}

struct Steps<'a, H: LockHarness> {
    h: &'a mut H,
    prior: Option<Reading>,
    /// This step armed the remote machine (and so disarms it at the end).
    armed_here: bool,
    results: Vec<CaseResult>,
}

impl<H: LockHarness> Steps<'_, H> {
    fn record(&mut self, name: &str, reasons: Vec<String>) {
        self.results.push(CaseResult {
            name: name.to_owned(),
            reasons,
        });
    }

    fn read(&mut self, reasons: &mut Vec<String>) -> Option<Reading> {
        match self.h.read_policy() {
            Ok(r) => Some(r),
            Err(e) => {
                reasons.push(format!("the registry could not be read: {e:#}"));
                None
            }
        }
    }

    /// Value is the prior one and the marker is gone. The client takes a
    /// tick and then a short delay to put it back, and the watchdog a poll,
    /// so this looks again for [`RESTORE_GRACE`] before it says no.
    fn expect_restored(&mut self, what: &str, reasons: &mut Vec<String>) {
        let Some(prior) = self.prior else {
            return;
        };
        let mut last = None;
        for poll in 0..=RESTORE_POLLS {
            let mut read_errors = Vec::new();
            last = self.read(&mut read_errors);
            match last {
                Some(now) if now.value == prior.value && !now.marker => return,
                None => {
                    reasons.append(&mut read_errors);
                    return;
                }
                Some(_) if poll < RESTORE_POLLS => self.h.sleep(RESTORE_POLL),
                Some(_) => {}
            }
        }
        if let Some(now) = last {
            reasons.push(format!(
                "{what}: registry is {now:?} after {} ms, wanted the prior value {:?} and no marker",
                RESTORE_GRACE.as_millis(),
                prior.value
            ));
        }
    }

    /// Value is 1 and the marker is present.
    fn expect_held(&mut self, what: &str, reasons: &mut Vec<String>) {
        if let Some(now) = self.read(reasons)
            && (now.value != Some(1) || !now.marker)
        {
            reasons.push(format!(
                "{what}: registry is {now:?}, wanted value 1 and the marker{}",
                self.front_note()
            ));
        }
    }

    /// Says which window is in front, for a failure message: the client
    /// holds the setting only while it is in front, so a failure reads
    /// differently when something else took the foreground.
    fn front_note(&mut self) -> &'static str {
        if self.h.foreground_is_client() {
            " (the client is the foreground window)"
        } else {
            " (the client is not the foreground window; a click or a window opening or \
             closing outside this run moved it)"
        }
    }

    /// Puts the client or the text field in front of it (`front`), waits
    /// [`SETTLE`], and looks. The client grabs when it gains the foreground
    /// and lets go when it loses it, so a click or a window opening or
    /// closing between the move and the look changes what every later step
    /// measures; this tries again up to [`PLACE_TRIES`] times. What a failed
    /// try said is kept only when no try succeeds.
    fn place(&mut self, front: Front, what: &str, reasons: &mut Vec<String>) -> bool {
        let mut errors = Vec::new();
        for _ in 0..PLACE_TRIES {
            let moves = match front {
                Front::Client => vec![self.h.focus(Window::Client)],
                Front::TextField => vec![
                    self.h.focus(Window::Notepad),
                    self.h.pointer(Place::OffClient),
                ],
            };
            let mut failed = false;
            for step in moves {
                if let Err(e) = step {
                    errors.push(format!("{what} placement failed: {e:#}"));
                    failed = true;
                }
            }
            self.h.sleep(SETTLE);
            if !failed && self.h.foreground_is_client() == (front == Front::Client) {
                return true;
            }
        }
        errors.dedup();
        reasons.append(&mut errors);
        reasons.push(format!(
            "{what}: the {} kept {} the foreground after {PLACE_TRIES} tries; a click or a \
             window opening or closing outside this run is moving it",
            match front {
                Front::Client => "client",
                Front::TextField => "text field",
            },
            match front {
                Front::Client => "losing",
                Front::TextField => "behind",
            },
        ));
        false
    }

    /// The client of this run is running, so a Win+L pressed now is the
    /// client's to take or leave. Without it the press would lock this
    /// machine and prove nothing, so it is not asked for.
    fn client_up(&mut self, what: &str, reasons: &mut Vec<String>) -> bool {
        if self.h.client_running() {
            return true;
        }
        reasons.push(format!(
            "{what}: the client is not running, so the press was not asked for (Win+L would \
             lock this machine and prove nothing)"
        ));
        false
    }

    fn start_registry(&mut self, reasons: &mut Vec<String>) -> bool {
        self.start_with(&registry_flags(), reasons)
    }

    fn start_with(&mut self, flags: &[String], reasons: &mut Vec<String>) -> bool {
        if let Err(e) = self.h.start_client(flags) {
            reasons.push(format!("the client did not start: {e:#}"));
            return false;
        }
        // A client that is not in front holds nothing and has no hook, so
        // the leg cannot be measured, and a later press of Win+L would lock
        // this machine. The client this run started is closed again.
        if !self.place(Front::Client, "the client start", reasons) {
            if let Err(e) = self.h.close_client() {
                reasons.push(format!("closing the client failed: {e:#}"));
            }
            return false;
        }
        true
    }

    fn l1(&mut self) {
        let mut reasons = Vec::new();
        if let Some(r) = self.read(&mut reasons) {
            if r.marker {
                reasons.push(
                    "a marker is already present: run `gliff-windows.exe --restore-lock-policy`"
                        .to_owned(),
                );
            }
            if r.value == Some(1) {
                reasons.push(
                    "DisableLockWorkstation is already 1: the lock step needs it 0 or absent, \
                     or it proves nothing about the client's own write"
                        .to_owned(),
                );
            }
            self.prior = Some(r);
        }
        self.record("L1 before", reasons);
    }

    /// The remote machine is in the test submap, or the step stops: without it a
    /// press would reach the remote machine's own bindings, not the test log.
    fn arm(&mut self) {
        let mut reasons = Vec::new();
        match self.h.arm_remote() {
            Ok(armed_here) => self.armed_here = armed_here,
            Err(e) => reasons.push(format!("{e:#}")),
        }
        self.record("L1 remote machine armed", reasons);
    }

    fn l2(&mut self) {
        let mut reasons = Vec::new();
        if self.start_registry(&mut reasons) {
            self.expect_held("L2", &mut reasons);
            if !self.h.watchdog_running() {
                reasons.push("L2: no watchdog process is running".to_owned());
            }
        }
        self.record("L2 launch and grab", reasons);
    }

    /// The binds that fire on the remote machine while the maintainer presses keys,
    /// during a countdown of `secs` (the raw `key:` and `typed:` lines the
    /// log also carries are left out: they say which keys went down, and
    /// Win+L is one bind however many that is). `None` when the remote machine left the test
    /// submap, in which case nothing is asked of the maintainer.
    fn pressed(&mut self, text: &str, secs: u32, reasons: &mut Vec<String>) -> Option<Vec<String>> {
        if !self.h.remote_submap_ok() {
            reasons.push(
                "the remote machine is no longer in the test submap, so the press was not asked for \
                 (run `gwin-keys check`)"
                    .to_owned(),
            );
            return None;
        }
        let mark = self.h.remote_log().map(|l| last_n(&l)).unwrap_or(0);
        // Forget a lock seen before this press.
        let _ = self.h.session_active();
        self.h.countdown(text, secs);
        self.h.sleep(Duration::from_millis(600));
        Some(
            self.h
                .remote_log()
                .map(|l| {
                    since(&l, mark)
                        .into_iter()
                        .map(|e| e.what)
                        .filter(|what| is_bind(what))
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    /// The end of every countdown: the client lets go, whatever happened.
    fn release(&mut self, reasons: &mut Vec<String>) {
        if let Err(e) = self.h.release_grab() {
            reasons.push(format!("releasing the grab failed: {e:#}"));
        }
        self.h.sleep(Duration::from_millis(300));
    }

    fn l3(&mut self) {
        let mut reasons = Vec::new();
        // The client holds the setting only while it is in front: a Win+L
        // pressed otherwise locks this machine for real.
        if !self.client_up("L3", &mut reasons) || !self.place(Front::Client, "L3", &mut reasons) {
            self.record("L3 focused Win+L", reasons);
            return;
        }
        let seen = self.pressed("Press Win+L now (once)", PRESS_SECS, &mut reasons);
        if let Some(seen) = seen {
            if seen != ["SUPER+L"] {
                reasons.push(format!(
                    "L3: remote logged {seen:?}, wanted exactly [\"SUPER+L\"]"
                ));
            }
            if !self.h.session_active() {
                reasons.push("L3: the Windows machine locked".to_owned());
            }
            if !self.h.foreground_is_client() {
                reasons.push("L3: the client is no longer foreground (Start opened?)".to_owned());
            }
            self.release(&mut reasons);
        }
        self.record("L3 focused Win+L", reasons);
    }

    fn l4(&mut self) {
        let mut reasons = Vec::new();
        if !self.client_up("L4", &mut reasons) {
            self.record("L4 hover only", reasons);
            return;
        }
        for step in [
            self.h.focus(Window::Notepad),
            self.h.pointer(Place::OverClient),
        ] {
            if let Err(e) = step {
                reasons.push(format!("L4 placement failed: {e:#}"));
            }
        }
        self.h.note_explorer_windows();
        let seen = self.pressed(
            "Press Win+L, then Win+E, once each. Win+L must reach the remote machine; Win+E opens \
             File Explorer on this machine (hover captures only Win+L) and the check closes \
             that window again within a moment, so a flash or nothing visible is normal. \
             Do not click (focus is in the gwin-verify text field)",
            PRESS_TWO_SECS,
            &mut reasons,
        );
        // The watch stops whether or not the press was asked for: left
        // running it would close any Explorer window the maintainer opens.
        let opened = self.h.explorer_opened_and_closed();
        if let Some(seen) = seen {
            if seen != ["SUPER+L"] {
                let mut reason = format!(
                    "L4: remote logged {seen:?}, wanted exactly [\"SUPER+L\"] (Win+E stays local)"
                );
                if seen.iter().any(|s| s.starts_with("mouse:")) {
                    reason.push_str(
                        "; a mouse button reached the remote machine, and this step sends none, so \
                         the window was clicked while it ran",
                    );
                }
                reasons.push(reason);
            }
            if !self.h.session_active() {
                reasons.push("L4: the Windows machine locked".to_owned());
            }
            if !opened {
                reasons.push("L4: Win+E did not open Explorer locally".to_owned());
            }
            self.release(&mut reasons);
        }
        self.record("L4 hover only", reasons);
    }

    fn l5(&mut self) {
        let mut reasons = Vec::new();
        if !self.client_up("L5", &mut reasons) || !self.place(Front::TextField, "L5", &mut reasons)
        {
            // With the client in front the setting is held and Win+L would
            // not lock: nothing here can pass, and asking for the press only
            // spends the maintainer's time.
            self.record("L5 blur and leave", reasons);
            return;
        }
        self.expect_restored("L5 after blur and leave", &mut reasons);
        let seen = self.pressed(
            "Press Win+L now: this machine WILL lock, unlock it afterwards",
            LOCK_PRESS_SECS,
            &mut reasons,
        );
        if seen.is_some() {
            // The press locked the machine, or the setting still held it.
            if self.h.session_active() {
                reasons.push(
                    "L5: with the setting released, Win+L did not lock the Windows machine"
                        .to_owned(),
                );
            } else if !self.h.wait_unlocked(UNLOCK_WAIT) {
                reasons.push(format!(
                    "L5: the machine is still locked after {} s; unlock it, then run \
                     `gliff-windows.exe --restore-lock-policy` if a marker is left",
                    UNLOCK_WAIT.as_secs()
                ));
            }
        }
        self.record("L5 blur and leave", reasons);
    }

    fn l6(&mut self) {
        let mut reasons = Vec::new();
        if self.start_registry(&mut reasons) {
            self.expect_held("L6 normal close, before", &mut reasons);
            if let Err(e) = self.h.close_client() {
                reasons.push(format!("closing the client failed: {e:#}"));
            }
            self.h.sleep(SETTLE);
            self.expect_restored("L6 after a normal close", &mut reasons);
        }
        if self.start_with(&panic_flags(), &mut reasons) {
            self.expect_held("L6 panic, before", &mut reasons);
            // The hook exists only while the client grabs, so the panic key
            // does nothing to a client that is not in front.
            if !self.h.foreground_is_client() {
                self.place(Front::Client, "L6 panic", &mut reasons);
            }
            if let Err(e) = self.h.panic_client() {
                reasons.push(format!("injecting the panic key failed: {e:#}"));
            }
            self.h.sleep(WATCHDOG);
            // The hook thread's panic must end the process (a panic cannot
            // unwind out of the hook, so it aborts); a client that is still
            // running did not take the panic path at all.
            if self.h.client_running() {
                reasons.push(format!(
                    "L6: the client was still running after the panic key{}",
                    self.front_note()
                ));
            }
            self.expect_restored("L6 after the panic test", &mut reasons);
        }
        self.record("L6 exit paths", reasons);
    }

    fn l7(&mut self) {
        let mut reasons = Vec::new();
        if self.start_registry(&mut reasons) {
            self.expect_held("L7 kill, before", &mut reasons);
            self.kill(Kill::Client, &mut reasons);
            self.h.sleep(WATCHDOG);
            self.expect_restored("L7 the watchdog after /F", &mut reasons);
        }
        // Killing both leaves the value set: documented, and the recovery
        // paths below must put it back.
        if self.start_registry(&mut reasons) {
            self.kill(Kill::Tree, &mut reasons);
            self.h.sleep(WATCHDOG);
            // Documented: with both processes gone the value stays set and
            // the marker stays; the recovery paths below depend on it.
            self.expect_held("L7 /F /T (both die)", &mut reasons);
            if let Err(e) = self.h.restore_flag() {
                reasons.push(format!("--restore-lock-policy failed: {e:#}"));
            }
            self.expect_restored("L7 after --restore-lock-policy", &mut reasons);
        }
        // Second round: the startup check of the next launch recovers.
        if self.start_registry(&mut reasons) {
            self.kill(Kill::Tree, &mut reasons);
            self.h.sleep(WATCHDOG);
            // There must be a stale marker for the next launch to recover.
            self.expect_held("L7 before the startup check", &mut reasons);
            if self.h.start_client(&registry_flags()).is_ok() {
                self.h.sleep(SETTLE);
                // The new client holds the value only once grabbed; it has
                // not been focused, so what matters is that the stale
                // marker is gone or replaced by its own.
                if let Err(e) = self.h.close_client() {
                    reasons.push(format!("closing the client failed: {e:#}"));
                }
                self.h.sleep(SETTLE);
                self.expect_restored("L7 after the startup check", &mut reasons);
            } else {
                reasons.push("L7: the client did not start for the startup check".to_owned());
            }
        }
        self.record("L7 kill", reasons);
    }

    fn kill(&mut self, how: Kill, reasons: &mut Vec<String>) {
        if let Err(e) = self.h.kill(how) {
            reasons.push(format!("killing the client ({how:?}) failed: {e:#}"));
        }
    }

    fn end(&mut self) {
        let mut reasons = Vec::new();
        if self.armed_here {
            self.h.disarm_remote();
        }
        let mut registry = Vec::new();
        self.expect_restored("End", &mut registry);
        let client_left = self.h.client_running();
        if client_left || self.h.watchdog_running() {
            reasons.push("End: a client or watchdog is still running".to_owned());
        }
        if registry.is_empty() {
            self.record("End", reasons);
            return;
        }
        reasons.append(&mut registry);
        // Leave the machine as found: the client's own recovery flag. It
        // leaves alone the marker of a client that is still running, so the
        // one this run started is closed first.
        if client_left && let Err(e) = self.h.close_client() {
            reasons.push(format!("End: closing the client failed: {e:#}"));
        }
        if let Err(e) = self.h.restore_flag() {
            reasons.push(format!(
                "End: --restore-lock-policy failed ({e:#}); run it by hand on the Windows machine"
            ));
        }
        // Said only when the registry shows it.
        let mut after = Vec::new();
        self.expect_restored("End after --restore-lock-policy", &mut after);
        if after.is_empty() {
            reasons.push("End: repaired with --restore-lock-policy".to_owned());
        } else {
            reasons.append(&mut after);
        }
        self.record("End", reasons);
    }
}

/// Runs L1 to L7 and the end check. The value read in L1 is the one the
/// end state is judged against.
pub fn run(h: &mut impl LockHarness) -> Vec<CaseResult> {
    let mut steps = Steps {
        h,
        prior: None,
        armed_here: false,
        results: Vec::new(),
    };
    steps.l1();
    // Nothing is started if the first read failed or a stale marker is
    // there: the maintainer has to look first. The remote machine is armed and
    // verified before the first client starts.
    if steps.results[0].passed() {
        steps.arm();
    }
    if steps.results.iter().all(CaseResult::passed) {
        steps.l2();
        steps.l3();
        steps.l4();
        steps.l5();
        steps.l6();
        steps.l7();
    }
    steps.end();
    steps.results
}

/// The handles in `now` that are not in `before`, in order, each once.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn unseen(before: &[isize], now: &[isize]) -> Vec<isize> {
    let mut out: Vec<isize> = Vec::new();
    for hwnd in now {
        if !before.contains(hwnd) && !out.contains(hwnd) {
            out.push(*hwnd);
        }
    }
    out
}

#[cfg(test)]
mod tests;
