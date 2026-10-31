use std::time::Duration;

use gliff_windows::stats_out::SessionEntry;

use super::judge::ClientExit;
use super::judge::Expect;
use super::judge::Observed;
use super::judge::judge;
use super::run::Harness;
use super::run::Plan;
use super::run::run;
use crate::present_check::fixtures::good_stats;

fn targets() -> Vec<String> {
    ["a@one", "b@two", "c@three"].map(String::from).to_vec()
}

fn entries(shown: usize, acks: u64) -> Vec<SessionEntry> {
    targets()
        .into_iter()
        .enumerate()
        .map(|(i, target)| SessionEntry {
            id: i as u32 + 1,
            target,
            shown: i == shown,
            status: "streaming".into(),
            frames: acks,
            acks,
            ..SessionEntry::default()
        })
        .collect()
}

/// What a client that does everything right would show.
fn good() -> Observed {
    let expect = Expect::standard(targets());
    Observed {
        start: entries(0, 100),
        steps: expect
            .steps
            .iter()
            .enumerate()
            .map(|(n, s)| entries(s.shown, 200 + 100 * n as u64))
            .collect(),
        history: targets(),
        exit: ClientExit::Exited(Some(0)),
    }
}

fn reasons_of(observed: &Observed) -> Vec<String> {
    let line = judge(observed, &Expect::standard(targets()));
    line.metrics["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .map(|r| r.as_str().expect("text").to_owned())
        .collect()
}

fn has(reasons: &[String], kind: &str) -> bool {
    reasons.iter().any(|r| r.starts_with(kind))
}

#[test]
fn a_correct_run_passes() {
    let line = judge(&good(), &Expect::standard(targets()));
    assert!(line.ok, "{}", line.to_json());
    assert_eq!(line.metrics["steps"].as_array().expect("steps").len(), 5);
}

#[test]
fn the_wrong_session_shown_is_named() {
    let mut o = good();
    o.steps[1] = entries(0, 300);
    assert!(has(&reasons_of(&o), "wrong_shown"));
}

#[test]
fn two_sessions_shown_or_none_fails() {
    let mut o = good();
    o.steps[0][0].shown = true;
    assert!(has(&reasons_of(&o), "shown_count"));
    let mut o = good();
    o.steps[0].iter_mut().for_each(|e| e.shown = false);
    assert!(has(&reasons_of(&o), "shown_count"));
}

#[test]
fn a_key_or_button_still_down_in_any_session_fails() {
    let mut o = good();
    o.steps[2][0].keys_down = 1;
    assert!(has(&reasons_of(&o), "held_input"));
    let mut o = good();
    o.steps[3][2].buttons_down = 1;
    assert!(has(&reasons_of(&o), "held_input"));
}

#[test]
fn a_hidden_session_that_stopped_acking_fails() {
    let mut o = good();
    o.steps[1][1].acks = o.steps[0][1].acks;
    assert!(has(&reasons_of(&o), "not_acked"));
}

#[test]
fn the_start_must_list_every_target_in_order_and_streaming() {
    let mut o = good();
    o.start.pop();
    assert!(has(&reasons_of(&o), "sessions"));
    let mut o = good();
    o.start.swap(0, 1);
    o.start[0].id = 1;
    o.start[1].id = 2;
    assert!(has(&reasons_of(&o), "target_order"));
    let mut o = good();
    o.start[2].status = "connecting".into();
    assert!(has(&reasons_of(&o), "not_streaming"));
}

#[test]
fn a_target_missing_from_the_history_fails() {
    let mut o = good();
    o.history.retain(|h| h != "b@two");
    let reasons = reasons_of(&o);
    assert!(has(&reasons, "history_missing"));
    assert!(reasons[0].contains("b@two"));
}

#[test]
fn a_client_that_did_not_exit_cleanly_fails() {
    let mut o = good();
    o.exit = ClientExit::Killed;
    assert!(has(&reasons_of(&o), "exit"));
    o.exit = ClientExit::Exited(Some(1));
    assert!(has(&reasons_of(&o), "exit"));
}

#[test]
fn fewer_steps_than_planned_fails() {
    let mut o = good();
    o.steps.pop();
    assert!(has(&reasons_of(&o), "steps_missing"));
}

#[test]
fn the_plan_stays_inside_the_tabs_so_wrapping_does_not_matter() {
    let plan = Expect::standard(targets());
    let mut at = 0usize;
    for step in &plan.steps {
        match step.name {
            "next" | "next again" => at += 1,
            "previous" => at -= 1,
            _ => at = step.shown,
        }
        assert_eq!(at, step.shown, "{}", step.name);
        assert!(at < 3);
    }
}

/// A client that follows the chords, with faults to turn on.
struct Fake {
    shown: usize,
    ticks: u64,
    launched: bool,
    focused: bool,
    log: Vec<String>,
    /// The chord that does nothing.
    ignore: Option<&'static str>,
    held_key: bool,
    freeze_hidden: Option<usize>,
    stuck_connecting: bool,
    history: Vec<String>,
    exit: ClientExit,
    launch_error: bool,
}

impl Fake {
    fn new() -> Self {
        Self {
            shown: 0,
            ticks: 0,
            launched: false,
            focused: false,
            log: Vec::new(),
            ignore: None,
            held_key: false,
            freeze_hidden: None,
            stuck_connecting: false,
            history: targets(),
            exit: ClientExit::Exited(Some(0)),
            launch_error: false,
        }
    }
}

impl Harness for Fake {
    fn launch(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.launch_error, "no window");
        self.launched = true;
        Ok(())
    }
    fn focus(&mut self) -> anyhow::Result<()> {
        self.focused = true;
        Ok(())
    }
    fn chord(&mut self, chord: &str) -> anyhow::Result<()> {
        assert!(self.focused, "chords go to a focused window");
        self.log.push(chord.to_owned());
        if self.ignore == Some(chord) {
            return Ok(());
        }
        self.shown = match chord {
            "ctrl+alt+pagedown" => (self.shown + 1) % 3,
            "ctrl+alt+pageup" => (self.shown + 2) % 3,
            "ctrl+alt+1" => 0,
            "ctrl+alt+3" => 2,
            other => panic!("unexpected chord {other}"),
        };
        Ok(())
    }
    fn stats(&mut self) -> Option<gliff_windows::stats_out::ClientStats> {
        if !self.launched {
            return None;
        }
        self.ticks += 1;
        let mut stats = good_stats();
        stats.sessions = entries(self.shown, self.ticks * 10);
        for (i, e) in stats.sessions.iter_mut().enumerate() {
            if self.freeze_hidden == Some(i) {
                e.acks = 5;
            }
            if self.stuck_connecting && i == 2 {
                e.status = "connecting".into();
            }
            if self.held_key && e.shown {
                e.keys_down = 1;
            }
        }
        Some(stats)
    }
    fn recent_hosts(&mut self) -> Vec<String> {
        self.history.clone()
    }
    fn close(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn wait_exit(&mut self, _timeout: Duration) -> ClientExit {
        self.exit
    }
    fn sleep(&mut self, _d: Duration) {}
}

fn plan() -> Plan {
    let mut plan = Plan::new(Expect::standard(targets()));
    // The fake's clock is its stats calls, not time.
    plan.start_timeout = Duration::from_millis(1);
    plan.switch_timeout = Duration::from_millis(1);
    plan
}

fn reasons(line: &crate::line::Line) -> Vec<String> {
    line.metrics["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .map(|r| r.as_str().expect("text").to_owned())
        .collect()
}

#[test]
fn a_client_that_follows_the_chords_passes() {
    let mut fake = Fake::new();
    let line = run(&mut fake, &plan());
    assert!(line.ok, "{}", line.to_json());
    assert_eq!(fake.log.len(), 5);
}

#[test]
fn a_chord_the_client_ignores_is_caught() {
    let mut fake = Fake::new();
    fake.ignore = Some("ctrl+alt+pageup");
    let line = run(&mut fake, &plan());
    assert!(!line.ok);
    assert!(has(&reasons(&line), "wrong_shown"));
}

#[test]
fn a_stuck_key_is_caught() {
    let mut fake = Fake::new();
    fake.held_key = true;
    assert!(has(&reasons(&run(&mut fake, &plan())), "held_input"));
}

#[test]
fn a_frozen_hidden_session_is_caught() {
    let mut fake = Fake::new();
    fake.freeze_hidden = Some(2);
    assert!(has(&reasons(&run(&mut fake, &plan())), "not_acked"));
}

#[test]
fn a_session_that_never_streams_stops_the_run_before_any_chord() {
    let mut fake = Fake::new();
    fake.stuck_connecting = true;
    let line = run(&mut fake, &plan());
    assert!(!line.ok);
    assert!(
        fake.log.is_empty(),
        "no chord goes to a client that is not ready"
    );
    assert!(has(&reasons(&line), "not_streaming"));
}

#[test]
fn a_client_that_does_not_start_is_reported_as_such() {
    let mut fake = Fake::new();
    fake.launch_error = true;
    let line = run(&mut fake, &plan());
    assert!(!line.ok);
    assert!(has(&reasons(&line), "launch"));
}

#[test]
fn a_history_without_the_targets_is_caught() {
    let mut fake = Fake::new();
    fake.history.clear();
    assert!(has(&reasons(&run(&mut fake, &plan())), "history_missing"));
}
