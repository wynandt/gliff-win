use std::time::Duration;

use super::Kill;
use super::LockHarness;
use super::Reading;
use super::run;
use super::unseen;
use crate::input_check::cases::Place;
use crate::input_check::cases::Window;
use crate::input_check::remote_log::Entry;

/// A fake machine: a registry the *client* model writes, and a maintainer
/// who presses what is asked. `behaviour` switches in failures.
#[derive(Default)]
struct Fake {
    reading: Option<Reading>,
    prior: Option<u32>,
    client: bool,
    watchdog: bool,
    locked: bool,
    log: Vec<Entry>,
    /// Entries a prompt makes the remote log, popped in order.
    presses: Vec<Vec<&'static str>>,
    // Behaviours.
    stale_marker: bool,
    watchdog_broken: bool,
    never_restores_on_close: bool,
    /// The client ignores `--test-panic` (it keeps running on F23).
    ignores_panic_flag: bool,
    /// Win+E opened nothing new (an old Explorer window is not proof).
    no_new_explorer: bool,
    panic_armed: bool,
    /// The client is the foreground window.
    front: bool,
    /// The next this many `focus` calls to the client do not take (something
    /// else keeps the foreground).
    ignored_client_focus: usize,
    /// As above for the text field.
    ignored_notepad_focus: usize,
    /// The foreground leaves the client once, when the panic leg first reads
    /// the registry.
    front_leaves_at_panic_read: bool,
    locks_while_held: bool,
    writes: usize,
    // The remote machine and the countdown.
    /// Order of what the harness did, for the ordering tests.
    events: Vec<String>,
    /// Every countdown: the instruction and its seconds.
    countdowns: Vec<(String, u32)>,
    flags_seen: Vec<Vec<String>>,
    /// The remote machine is not in the test submap, and `arm` cannot fix it.
    cannot_arm: bool,
    /// The remote machine was armed by hand before the run.
    armed_by_hand: bool,
    armed: bool,
    disarmed: bool,
    /// The test submap is lost after the first countdown.
    loses_submap: bool,
    /// Win+L never locks the machine, even with the setting released.
    never_locks: bool,
    /// The machine stays locked (nobody unlocks it).
    stays_locked: bool,
    releases: usize,
    /// The client puts the value back only after this many registry reads
    /// (a tick and a delay in the real one); 0 is at once.
    late_restore_reads: usize,
    restore_in: usize,
    /// The raw lines the remote log carries beside the binds, added to
    /// every press.
    raw_lines: Vec<&'static str>,
    /// The next this many `focus` calls fail with an error.
    focus_errors: usize,
    /// The client does not exit when it is closed.
    client_survives_close: bool,
    /// When the step lets go of the remote machine (its last act before the end
    /// check), the client is running again with the value set.
    client_left_holding_at_end: bool,
    /// Explorer watches started and not yet stopped.
    explorer_watches: usize,
}

impl Fake {
    fn new(prior: Option<u32>) -> Self {
        Self {
            reading: Some(Reading {
                value: prior,
                marker: false,
            }),
            prior,
            presses: vec![vec!["SUPER+L"], vec!["SUPER+L"]],
            ..Self::default()
        }
    }

    fn put(&mut self, value: Option<u32>, marker: bool) {
        self.reading = Some(Reading { value, marker });
        self.writes += 1;
    }
}

impl LockHarness for Fake {
    fn read_policy(&mut self) -> anyhow::Result<Reading> {
        if self.restore_in > 0 {
            self.restore_in -= 1;
            if self.restore_in == 0 {
                self.put(self.prior, false);
            }
        }
        if self.front_leaves_at_panic_read && self.panic_armed {
            self.front_leaves_at_panic_read = false;
            self.front = false;
        }
        let mut r = self.reading.expect("reading");
        if self.stale_marker && self.writes == 0 {
            r.marker = true;
        }
        Ok(r)
    }

    fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()> {
        assert_eq!(
            &flags[..4],
            ["--super-l", "registry", "--grab-timeout-secs", "120"],
            "the step runs in registry mode with a grab safety net"
        );
        assert!(
            flags.len() == 4
                || flags
                    == [
                        "--super-l",
                        "registry",
                        "--grab-timeout-secs",
                        "120",
                        "--test-panic"
                    ],
            "only the panic leg adds a flag: {flags:?}"
        );
        assert!(
            self.armed || self.armed_by_hand,
            "the remote machine was armed before any client"
        );
        self.flags_seen.push(flags.to_vec());
        self.panic_armed = flags.iter().any(|f| f == "--test-panic") && !self.ignores_panic_flag;
        // The client's startup check puts back a stale value.
        if self.reading.expect("reading").marker {
            self.put(self.prior, false);
        }
        self.client = true;
        Ok(())
    }

    fn focus(&mut self, window: Window) -> anyhow::Result<()> {
        if self.focus_errors > 0 {
            self.focus_errors -= 1;
            anyhow::bail!("SetForegroundWindow was refused");
        }
        match window {
            Window::Client if self.ignored_client_focus > 0 => self.ignored_client_focus -= 1,
            Window::Client => {
                self.front = true;
                if self.client {
                    self.put(Some(1), true);
                    self.watchdog = true;
                }
            }
            Window::Notepad if self.ignored_notepad_focus > 0 => self.ignored_notepad_focus -= 1,
            Window::Notepad => self.front = false,
        }
        Ok(())
    }

    fn pointer(&mut self, place: Place) -> anyhow::Result<()> {
        if place == Place::OffClient {
            if self.late_restore_reads > 0 {
                self.restore_in = self.late_restore_reads;
            } else {
                self.put(self.prior, false);
            }
        }
        Ok(())
    }

    fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>> {
        self.events.push("log".into());
        Ok(self.log.clone())
    }

    fn arm_remote(&mut self) -> anyhow::Result<bool> {
        self.events.push("arm".into());
        if self.cannot_arm {
            anyhow::bail!("the remote machine is not in the test submap");
        }
        if self.armed_by_hand {
            return Ok(false);
        }
        self.armed = true;
        Ok(true)
    }

    fn remote_submap_ok(&mut self) -> bool {
        self.events.push("check".into());
        !self.cannot_arm && (self.armed || self.armed_by_hand)
    }

    fn disarm_remote(&mut self) {
        self.events.push("disarm".into());
        self.disarmed = true;
        if self.client_left_holding_at_end {
            self.client = true;
            self.put(Some(1), true);
        }
    }

    fn release_grab(&mut self) -> anyhow::Result<()> {
        self.events.push("release".into());
        self.releases += 1;
        Ok(())
    }

    fn wait_unlocked(&mut self, _max: Duration) -> bool {
        self.events.push("wait_unlocked".into());
        if self.stays_locked {
            return false;
        }
        self.locked = false;
        true
    }

    fn countdown(&mut self, text: &str, secs: u32) {
        self.events.push("countdown".into());
        self.countdowns.push((text.to_owned(), secs));
        if self.loses_submap && self.countdowns.len() == 1 {
            self.cannot_arm = true;
        }
        // With the setting held Win+L reaches the remote machine; released, it locks.
        if self.locks_while_held
            || (self.reading.is_some_and(|r| r.value != Some(1)) && !self.never_locks)
        {
            self.locked = true;
        }
        if !self.presses.is_empty() {
            let mut lines = self.presses.remove(0);
            lines.extend(self.raw_lines.iter().copied());
            for what in lines {
                let n = self.log.len() as u64 + 1;
                self.log.push(Entry {
                    ts: "t".into(),
                    n,
                    what: what.into(),
                });
            }
        }
    }

    fn ask(&mut self, _text: &str) -> bool {
        true
    }

    fn session_active(&mut self) -> bool {
        !self.locked
    }

    fn foreground_is_client(&mut self) -> bool {
        self.front
    }

    fn client_running(&mut self) -> bool {
        self.client
    }

    fn watchdog_running(&mut self) -> bool {
        self.watchdog
    }

    fn close_client(&mut self) -> anyhow::Result<()> {
        self.events.push("close".into());
        if self.client_survives_close {
            return Ok(());
        }
        self.client = false;
        self.watchdog = false;
        if !self.never_restores_on_close {
            self.put(self.prior, false);
        }
        Ok(())
    }

    fn panic_client(&mut self) -> anyhow::Result<()> {
        // The hook exists only while the client is in front.
        if self.panic_armed && self.front {
            self.client = false;
            self.watchdog = false;
            self.put(self.prior, false);
        }
        Ok(())
    }

    fn kill(&mut self, how: Kill) -> anyhow::Result<()> {
        self.client = false;
        match how {
            Kill::Client if !self.watchdog_broken => {
                self.put(self.prior, false);
                self.watchdog = false;
            }
            Kill::Client => self.watchdog = false,
            Kill::Tree => self.watchdog = false,
        }
        Ok(())
    }

    fn restore_flag(&mut self) -> anyhow::Result<()> {
        self.events.push("restore_flag".into());
        // A running client owns the marker: the flag leaves it alone.
        if !self.client {
            self.put(self.prior, false);
        }
        Ok(())
    }

    fn note_explorer_windows(&mut self) {
        self.explorer_watches += 1;
    }

    fn explorer_opened_and_closed(&mut self) -> bool {
        self.explorer_watches = self.explorer_watches.saturating_sub(1);
        !self.no_new_explorer
    }

    fn sleep(&mut self, _d: Duration) {}
}

fn failures(results: &[crate::input_check::run::CaseResult]) -> Vec<String> {
    results
        .iter()
        .filter(|r| !r.passed())
        .map(|r| r.name.clone())
        .collect()
}

#[test]
fn a_clean_run_passes_every_step_and_leaves_the_prior_value() {
    for prior in [None, Some(0)] {
        let mut h = Fake::new(prior);
        let results = run(&mut h);
        assert_eq!(results.len(), 9, "L1, the arm row, L2 to L7 and End");
        assert_eq!(failures(&results), Vec::<String>::new(), "prior {prior:?}");
        assert_eq!(
            h.reading,
            Some(Reading {
                value: prior,
                marker: false
            }),
            "the registry ends as it began"
        );
    }
}

#[test]
fn a_stale_marker_stops_the_run_before_any_client_starts() {
    let mut h = Fake::new(None);
    h.stale_marker = true;
    let results = run(&mut h);
    assert_eq!(results[0].name, "L1 before", "L1 first");
    assert!(!results[0].passed(), "the marker fails L1");
    assert!(!h.client, "no client was started");
    assert_eq!(results.len(), 2, "only L1 and End ran");
}

#[test]
fn a_lock_while_the_setting_is_held_fails_l3_and_l4() {
    let mut h = Fake::new(None);
    h.locks_while_held = true;
    let results = run(&mut h);
    let failed = failures(&results);
    assert!(
        failed.contains(&"L3 focused Win+L".to_owned()),
        "{failed:?}"
    );
    assert!(failed.contains(&"L4 hover only".to_owned()), "{failed:?}");
}

#[test]
fn a_win_l_that_the_remote_did_not_see_fails_l3() {
    let mut h = Fake::new(None);
    h.presses = vec![vec![], vec!["SUPER+L"]];
    let failed = failures(&run(&mut h));
    assert_eq!(failed, ["L3 focused Win+L"], "only L3");
}

#[test]
fn a_close_that_leaves_the_value_set_fails_l6_and_the_end_repairs_it() {
    let mut h = Fake::new(Some(0));
    h.never_restores_on_close = true;
    let results = run(&mut h);
    let failed = failures(&results);
    assert!(failed.contains(&"L6 exit paths".to_owned()), "{failed:?}");
    assert_eq!(
        h.reading.map(|r| (r.value, r.marker)),
        Some((Some(0), false)),
        "the recovery flag put the prior value back"
    );
}

#[test]
fn a_client_that_survives_the_panic_key_fails_l6() {
    let mut h = Fake::new(None);
    h.ignores_panic_flag = true;
    let failed = failures(&run(&mut h));
    assert!(failed.contains(&"L6 exit paths".to_owned()), "{failed:?}");
}

#[test]
fn a_win_e_that_opened_no_new_explorer_fails_l4() {
    let mut h = Fake::new(None);
    h.no_new_explorer = true;
    let failed = failures(&run(&mut h));
    assert_eq!(failed, ["L4 hover only"], "only L4");
}

#[test]
fn a_prior_value_of_one_fails_l1() {
    let mut h = Fake::new(Some(1));
    let results = run(&mut h);
    assert!(!results[0].passed(), "L1 must not accept a value already 1");
}

#[test]
fn a_dead_watchdog_fails_l7() {
    let mut h = Fake::new(None);
    h.watchdog_broken = true;
    let failed = failures(&run(&mut h));
    assert!(failed.contains(&"L7 kill".to_owned()), "{failed:?}");
}

#[test]
fn the_real_lock_harness_never_calls_a_registry_write() {
    let source = include_str!("../win32/lockstep.rs");
    for forbidden in [
        "write_value",
        "delete_value",
        "write_marker",
        "delete_marker",
    ] {
        assert!(
            !source.contains(forbidden),
            "win32/lockstep.rs must only read the registry, found {forbidden}"
        );
    }
}

#[test]
fn the_remote_is_armed_and_checked_before_any_client_starts() {
    let mut h = Fake::new(None);
    let results = run(&mut h);
    assert_eq!(failures(&results), Vec::<String>::new());
    let arm = h.events.iter().position(|e| e == "arm").expect("armed");
    assert_eq!(arm, 0, "arming comes first: {:?}", h.events);
    assert!(
        h.flags_seen.len() >= 5,
        "clients started: {:?}",
        h.flags_seen
    );
}

#[test]
fn a_remote_that_cannot_arm_stops_the_step_before_any_client() {
    let mut h = Fake::new(None);
    h.cannot_arm = true;
    let results = run(&mut h);
    assert!(h.flags_seen.is_empty(), "no client started");
    assert!(!h.client);
    let armed = results
        .iter()
        .find(|r| r.name == "L1 remote machine armed")
        .expect("the arm row");
    assert!(!armed.passed());
    assert!(
        armed.reasons.join(" ").contains("test submap"),
        "{:?}",
        armed.reasons
    );
    assert!(
        !results.iter().any(|r| r.name.starts_with("L2")),
        "nothing after the arm row but End"
    );
    assert!(h.countdowns.is_empty(), "no prompt was shown");
}

#[test]
fn a_remote_armed_by_hand_is_left_armed_and_one_armed_here_is_disarmed() {
    let mut by_hand = Fake::new(None);
    by_hand.armed_by_hand = true;
    run(&mut by_hand);
    assert!(!by_hand.disarmed, "the maintainer's arm stays");

    let mut here = Fake::new(None);
    run(&mut here);
    assert!(here.disarmed, "the step disarms what it armed");
}

#[test]
fn every_prompted_press_gets_a_countdown_and_the_lock_one_a_longer_warning() {
    let mut h = Fake::new(None);
    run(&mut h);
    assert_eq!(h.countdowns.len(), 3, "L3, L4 and L5: {:?}", h.countdowns);
    for (text, secs) in &h.countdowns[..2] {
        assert!(*secs >= 15, "{text}: {secs} s is a fair time to press");
    }
    let (text, secs) = &h.countdowns[2];
    assert!(text.contains("WILL lock"), "L5 says it will lock: {text}");
    assert!(
        *secs > h.countdowns[0].1,
        "L5 gets a longer window than L3: {secs}"
    );
}

#[test]
fn the_log_is_judged_and_then_the_grab_is_released_after_each_press() {
    let mut h = Fake::new(None);
    run(&mut h);
    assert!(h.releases >= 2, "L3 and L4 each release: {}", h.releases);
    let first = h
        .events
        .iter()
        .position(|e| e == "countdown")
        .expect("countdown");
    let release = h
        .events
        .iter()
        .position(|e| e == "release")
        .expect("release");
    assert!(release > first, "release follows the countdown");
    assert!(
        h.events[first..release].iter().any(|e| e == "log"),
        "the log is read between the countdown and the release: {:?}",
        h.events
    );
}

#[test]
fn a_remote_that_leaves_the_submap_skips_the_press_and_fails_the_step() {
    let mut h = Fake::new(None);
    h.loses_submap = true;
    let results = run(&mut h);
    assert_eq!(h.countdowns.len(), 1, "only L3 got its countdown");
    let failed = failures(&results);
    assert!(failed.contains(&"L4 hover only".to_owned()), "{failed:?}");
    assert!(
        failed.contains(&"L5 blur and leave".to_owned()),
        "{failed:?}"
    );
    let l4 = results
        .iter()
        .find(|r| r.name == "L4 hover only")
        .expect("L4 row");
    assert!(
        l4.reasons.join(" ").contains("test submap"),
        "{:?}",
        l4.reasons
    );
}

#[test]
fn l5_fails_when_win_l_did_not_lock_the_machine() {
    let mut h = Fake::new(None);
    h.never_locks = true;
    let results = run(&mut h);
    assert_eq!(failures(&results), ["L5 blur and leave"]);
}

#[test]
fn l5_fails_and_says_so_when_nobody_unlocks_the_machine() {
    let mut h = Fake::new(None);
    h.stays_locked = true;
    let results = run(&mut h);
    let l5 = results
        .iter()
        .find(|r| r.name == "L5 blur and leave")
        .expect("L5 row");
    assert!(
        l5.reasons.join(" ").contains("still locked"),
        "{:?}",
        l5.reasons
    );
}

#[test]
fn the_step_waits_for_the_unlock_before_it_starts_the_next_client() {
    let mut h = Fake::new(None);
    run(&mut h);
    let wait = h
        .events
        .iter()
        .position(|e| e == "wait_unlocked")
        .expect("waited");
    assert!(
        h.events[..wait]
            .iter()
            .filter(|e| *e == "countdown")
            .count()
            == 3,
        "after the L5 countdown: {:?}",
        h.events
    );
}

#[test]
fn raw_key_lines_beside_the_bind_do_not_fail_l3_and_l4() {
    // What the remote logged in the live run: Win down twice, L down, the
    // bind, then the ups.
    let mut h = Fake::new(None);
    h.raw_lines = vec![
        "key:down:125",
        "key:down:125",
        "key:down:38",
        "key:up:38",
        "key:up:125",
        "typed:l",
    ];
    let failed = failures(&run(&mut h));
    assert_eq!(failed, Vec::<String>::new());
}

#[test]
fn a_second_bind_or_a_click_in_the_log_still_fails_l3_and_l4() {
    let mut h = Fake::new(None);
    h.presses = vec![vec!["SUPER+L", "SUPER+L"], vec!["SUPER+L", "mouse:272 5,5"]];
    h.raw_lines = vec!["key:down:125"];
    let failed = failures(&run(&mut h));
    assert_eq!(failed, ["L3 focused Win+L", "L4 hover only"]);
}

#[test]
fn a_restore_that_arrives_a_few_reads_late_passes() {
    let mut h = Fake::new(None);
    h.late_restore_reads = 4;
    let failed = failures(&run(&mut h));
    assert_eq!(failed, Vec::<String>::new());
}

#[test]
fn a_restore_that_never_arrives_fails_l5_after_the_grace_and_says_how_long() {
    let mut h = Fake::new(None);
    h.late_restore_reads = 1000;
    let results = run(&mut h);
    let l5 = results
        .iter()
        .find(|r| r.name == "L5 blur and leave")
        .expect("L5");
    assert!(!l5.passed());
    assert!(
        l5.reasons.iter().any(|r| r.contains("after 3000 ms")),
        "{:?}",
        l5.reasons
    );
}

#[test]
fn the_l4_banner_names_the_text_box_and_asks_for_no_click() {
    let mut h = Fake::new(None);
    let _ = run(&mut h);
    let banner = &h.countdowns[1].0;
    assert!(banner.contains("Win+L"), "{banner}");
    assert!(banner.contains("not click"), "{banner}");
    assert!(banner.contains("text field"), "{banner}");
    assert!(
        banner.contains("Win+E opens File Explorer on this machine"),
        "{banner}"
    );
    assert!(
        banner.contains("closes that window again within a moment"),
        "{banner}"
    );
    assert!(
        banner.contains("a flash or nothing visible is normal"),
        "{banner}"
    );
    assert!(banner.contains("hover captures only Win+L"), "{banner}");
    assert!(!banner.contains("Notepad"), "{banner}");
}

#[test]
fn a_click_in_l4_is_named_as_the_cause() {
    let mut h = Fake::new(None);
    h.presses = vec![vec!["SUPER+L"], vec!["SUPER+L", "mouse:272 651,63"]];
    let results = run(&mut h);
    let l4 = results
        .iter()
        .find(|r| r.name == "L4 hover only")
        .expect("L4");
    assert!(
        l4.reasons
            .iter()
            .any(|r| r.contains("the window was clicked while it ran")),
        "{:?}",
        l4.reasons
    );
}

#[test]
fn unseen_lists_only_new_handles_once() {
    assert_eq!(unseen(&[1, 2], &[2, 3, 3, 4, 1]), vec![3, 4]);
    assert!(unseen(&[1, 2], &[2, 1]).is_empty());
    assert_eq!(unseen(&[], &[7]), vec![7]);
}

#[test]
fn a_foreground_that_comes_back_once_is_put_back_and_the_run_passes() {
    let mut h = Fake::new(None);
    h.ignored_notepad_focus = 1;
    h.ignored_client_focus = 1;
    let failed = failures(&run(&mut h));
    assert_eq!(failed, Vec::<String>::new());
}

#[test]
fn a_client_that_keeps_the_foreground_fails_l5_and_asks_for_no_lock_press() {
    let mut h = Fake::new(None);
    h.ignored_notepad_focus = 1000;
    let results = run(&mut h);
    let l5 = results
        .iter()
        .find(|r| r.name == "L5 blur and leave")
        .expect("L5");
    assert!(!l5.passed());
    assert!(
        l5.reasons
            .iter()
            .any(|r| r.contains("outside this run is moving it")),
        "{:?}",
        l5.reasons
    );
    assert!(
        !h.countdowns
            .iter()
            .any(|(text, _)| text.contains("WILL lock")),
        "{:?}",
        h.countdowns
    );
}

#[test]
fn a_client_that_never_gets_the_foreground_asks_for_no_press_and_is_closed() {
    let mut h = Fake::new(None);
    h.ignored_client_focus = 1000;
    let results = run(&mut h);
    let failed = failures(&results);
    for step in ["L2 launch and grab", "L3 focused Win+L", "L6 exit paths"] {
        assert!(failed.contains(&step.to_owned()), "{step}: {failed:?}");
    }
    let l2 = results
        .iter()
        .find(|r| r.name == "L2 launch and grab")
        .expect("L2");
    assert!(
        l2.reasons
            .iter()
            .any(|r| r.contains("kept losing the foreground")),
        "{:?}",
        l2.reasons
    );
    assert!(
        h.countdowns.is_empty(),
        "a Win+L pressed with the client not in front would lock this machine: {:?}",
        h.countdowns
    );
    assert!(!h.client, "the client that could not be placed was closed");
}

#[test]
fn a_placement_that_fails_once_and_then_works_leaves_no_reason_behind() {
    let mut h = Fake::new(None);
    h.focus_errors = 1;
    let failed = failures(&run(&mut h));
    assert_eq!(failed, Vec::<String>::new());
}

#[test]
fn a_placement_that_never_works_says_what_each_try_said() {
    let mut h = Fake::new(None);
    h.focus_errors = 1000;
    let results = run(&mut h);
    let l2 = results
        .iter()
        .find(|r| r.name == "L2 launch and grab")
        .expect("L2");
    assert!(
        l2.reasons.iter().any(|r| r.contains("was refused")),
        "{:?}",
        l2.reasons
    );
}

#[test]
fn the_explorer_watch_stops_even_when_the_press_was_not_asked_for() {
    let mut h = Fake::new(None);
    h.loses_submap = true;
    run(&mut h);
    assert_eq!(h.explorer_watches, 0, "no watch is left running");
}

#[test]
fn the_end_does_not_say_repaired_when_it_repaired_nothing() {
    // A client that will not close, and a registry that is fine: the recovery
    // flag is not run, and the row says what is left.
    let mut h = Fake::new(Some(0));
    h.client_survives_close = true;
    let results = run(&mut h);
    let end = results.iter().find(|r| r.name == "End").expect("End");
    assert!(
        end.reasons.iter().any(|r| r.contains("still running")),
        "{:?}",
        end.reasons
    );
    assert!(
        !end.reasons.iter().any(|r| r.contains("repaired")),
        "{:?}",
        end.reasons
    );
    let at_end = &h.events[h.events.iter().position(|e| e == "disarm").expect("disarm")..];
    assert!(!at_end.iter().any(|e| e == "restore_flag"), "{at_end:?}");
}

#[test]
fn the_end_closes_its_own_client_before_the_recovery_flag_and_then_says_repaired() {
    let mut h = Fake::new(Some(0));
    h.client_left_holding_at_end = true;
    h.never_restores_on_close = true;
    let results = run(&mut h);
    let end = results.iter().find(|r| r.name == "End").expect("End");
    assert!(
        end.reasons.iter().any(|r| r.contains("repaired")),
        "{:?}",
        end.reasons
    );
    assert_eq!(
        h.reading.map(|r| (r.value, r.marker)),
        Some((Some(0), false)),
        "the flag only works once the client is gone"
    );
    let tail: Vec<&str> = h.events.iter().rev().take(2).map(String::as_str).collect();
    assert_eq!(tail, ["restore_flag", "close"], "{:?}", h.events);
}

#[test]
fn a_foreground_lost_before_the_panic_key_is_put_back_first() {
    let mut h = Fake::new(None);
    h.front_leaves_at_panic_read = true;
    let results = run(&mut h);
    let l6 = results
        .iter()
        .find(|r| r.name == "L6 exit paths")
        .expect("L6");
    assert!(
        l6.reasons
            .iter()
            .all(|r| !r.contains("still running after the panic key")),
        "the panic key reached a client in front: {:?}",
        l6.reasons
    );
}
