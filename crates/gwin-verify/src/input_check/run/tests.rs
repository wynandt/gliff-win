use std::time::Duration;

use super::Harness;
use super::Seen;
use super::gate_flags;
use super::run_cases;
use super::summary;
use crate::input_check::cases::Act;
use crate::input_check::cases::Case;
use crate::input_check::cases::Expect;
use crate::input_check::cases::Remote;
use crate::input_check::remote_log::Entry;

/// A harness whose remote log grows by scripted entries when an act runs.
#[derive(Default)]
struct Fake {
    log: Vec<Entry>,
    /// What each `Pause` act appends, popped in order.
    on_pause: Vec<Vec<&'static str>>,
    starts: Vec<Vec<String>>,
    stops: usize,
    fail_start: bool,
    fail_act: bool,
    /// Reads of the remote log after this many succeed no more.
    log_reads_allowed: Option<usize>,
    log_reads: usize,
    /// The client is not the foreground window.
    away: bool,
    acts: Vec<Act>,
}

impl Harness for Fake {
    fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()> {
        if self.fail_start {
            anyhow::bail!("no window");
        }
        self.starts.push(flags.to_vec());
        Ok(())
    }

    fn stop_client(&mut self) {
        self.stops += 1;
    }

    fn act(&mut self, act: &Act) -> anyhow::Result<()> {
        self.acts.push(act.clone());
        if self.fail_act && !matches!(act, Act::Dismiss) {
            anyhow::bail!("SendInput refused");
        }
        if matches!(act, Act::Pause(_)) && !self.on_pause.is_empty() {
            for what in self.on_pause.remove(0) {
                let n = self.log.len() as u64 + 1;
                self.log.push(Entry {
                    ts: "t".into(),
                    n,
                    what: what.into(),
                });
            }
        }
        Ok(())
    }

    fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>> {
        self.log_reads += 1;
        if self.log_reads_allowed.is_some_and(|n| self.log_reads > n) {
            anyhow::bail!("ssh dropped");
        }
        Ok(self.log.clone())
    }

    fn observe(&mut self) -> Seen {
        Seen {
            foreground_is_client: !self.away,
            ..Seen::default()
        }
    }

    fn remote_size(&mut self) -> Option<(u32, u32)> {
        Some((1920, 1200))
    }
}

fn case(name: &str, flags: &[&str], want: &[&str]) -> Case {
    Case {
        risky: false,
        name: name.into(),
        flags: flags.iter().map(|s| (*s).to_owned()).collect(),
        acts: vec![Act::Pause(Duration::ZERO)],
        expect: Expect::remote(Remote::Exactly(
            want.iter().map(|s| (*s).to_owned()).collect(),
        )),
    }
}

#[test]
fn only_the_entries_a_case_caused_are_judged() {
    let mut h = Fake {
        log: vec![Entry {
            ts: "t".into(),
            n: 1,
            what: "old".into(),
        }],
        on_pause: vec![vec!["SUPER+Y"], vec!["CTRL+Q"]],
        ..Fake::default()
    };
    let results = run_cases(
        &mut h,
        &[case("a", &[], &["SUPER+Y"]), case("b", &[], &["CTRL+Q"])],
    );
    assert!(results.iter().all(|r| r.passed()), "both pass: {results:?}");
}

#[test]
fn the_client_restarts_only_when_the_flags_change() {
    let mut h = Fake {
        on_pause: vec![vec![], vec![], vec![]],
        ..Fake::default()
    };
    let cases = [
        case("a", &[], &[]),
        case("b", &[], &[]),
        case("c", &["--grab-follows", "hover"], &[]),
    ];
    run_cases(&mut h, &cases);
    assert_eq!(h.starts.len(), 2, "two starts: {:?}", h.starts);
    assert_eq!(
        h.starts[1],
        ["--grab-follows", "hover"],
        "the second group's flags"
    );
}

#[test]
fn a_client_that_does_not_start_fails_its_cases_and_the_run_goes_on() {
    let mut h = Fake {
        fail_start: true,
        ..Fake::default()
    };
    let results = run_cases(&mut h, &[case("a", &[], &[]), case("b", &["x"], &[])]);
    assert_eq!(results.len(), 2, "every case has a result");
    assert!(
        results
            .iter()
            .all(|r| r.reasons[0].contains("did not start")),
        "the reason is the start: {results:?}"
    );
}

#[test]
fn an_act_that_fails_fails_the_case_with_the_error() {
    let mut h = Fake {
        fail_act: true,
        ..Fake::default()
    };
    let results = run_cases(&mut h, &[case("a", &[], &[])]);
    assert!(
        results[0].reasons[0].contains("SendInput refused"),
        "{results:?}"
    );
}

#[test]
fn the_summary_fails_on_any_failure_and_on_no_cases() {
    let mut h = Fake {
        on_pause: vec![vec!["SUPER+Y"], vec![]],
        ..Fake::default()
    };
    let results = run_cases(
        &mut h,
        &[case("a", &[], &["SUPER+Y"]), case("b", &[], &["CTRL+Q"])],
    );
    let line = summary("keys", &results);
    assert!(!line.ok, "one failed");
    assert_eq!(line.metrics["passed"], 1, "one passed");
    assert_eq!(line.metrics["failed"], serde_json::json!(["b"]), "b failed");
    assert!(!summary("keys", &[]).ok, "no cases is not a pass");
}

#[test]
fn a_risky_chord_that_never_reaches_the_remote_stops_the_other_risky_ones() {
    let risky = |name: &str, want: &[&str]| Case {
        risky: true,
        ..case(name, &[], want)
    };
    let mut h = Fake {
        // The first chord logs and the second reaches nothing; the third is
        // skipped (it takes no pause), so the plain case gets the last one.
        on_pause: vec![vec!["A"], vec![], vec![]],
        ..Fake::default()
    };
    let results = run_cases(
        &mut h,
        &[
            risky("one", &["A"]),
            risky("two", &["B"]),
            risky("three", &["C"]),
            case("plain", &[], &[]),
        ],
    );
    assert!(results[0].passed(), "the first passes: {results:?}");
    assert!(!results[1].passed(), "the silent one fails: {results:?}");
    assert!(
        results[2].reasons[0].contains("skipped") && results[2].reasons[0].contains("two"),
        "the next risky one is skipped and names why: {results:?}"
    );
    assert!(
        results[3].passed(),
        "a case that cannot open a window still runs"
    );
}

#[test]
fn a_gate_client_runs_in_chord_mode_unless_the_case_names_a_mode() {
    let none: Vec<String> = Vec::new();
    assert_eq!(gate_flags(&none), ["--super-l", "chord"]);
    let other = vec!["--injected-keys".to_owned(), "local".to_owned()];
    assert_eq!(
        gate_flags(&other),
        ["--super-l", "chord", "--injected-keys", "local"]
    );
    let lock: Vec<String> = ["--super-l", "registry", "--grab-timeout-secs", "120"]
        .map(str::to_owned)
        .into();
    assert_eq!(gate_flags(&lock), lock, "the lock step's mode is kept");
}

fn risky(name: &str, want: &[&str]) -> Case {
    Case {
        risky: true,
        ..case(name, &[], want)
    }
}

fn dismissals(h: &Fake) -> usize {
    h.acts.iter().filter(|a| matches!(a, Act::Dismiss)).count()
}

/// Two risky chords: the first goes wrong as `h` is set up, so the second must
/// be skipped, and the first must have been followed by a dismissal. (The first
/// may still pass its own expectations: the guard is about what Windows saw.)
fn first_of_two_risky_cases_goes_wrong(mut h: Fake) {
    let results = run_cases(&mut h, &[risky("one", &["A"]), risky("two", &["B"])]);
    assert!(
        results[1].reasons[0].contains("skipped") && results[1].reasons[0].contains("one"),
        "the guard tripped: {results:?}"
    );
    assert_eq!(dismissals(&h), 1, "a window the chord opened is dismissed");
}

#[test]
fn a_risky_chord_whose_act_fails_trips_the_guard_and_is_dismissed() {
    first_of_two_risky_cases_goes_wrong(Fake {
        fail_act: true,
        ..Fake::default()
    });
}

#[test]
fn a_risky_chord_whose_log_cannot_be_read_trips_the_guard_and_is_dismissed() {
    first_of_two_risky_cases_goes_wrong(Fake {
        // The read before the keys works, the one after does not.
        log_reads_allowed: Some(1),
        on_pause: vec![vec!["A"]],
        ..Fake::default()
    });
}

#[test]
fn a_risky_chord_with_the_client_not_in_front_trips_the_guard_and_is_dismissed() {
    first_of_two_risky_cases_goes_wrong(Fake {
        away: true,
        on_pause: vec![vec!["A"]],
        ..Fake::default()
    });
}

#[test]
fn a_plain_case_is_never_dismissed() {
    let mut h = Fake {
        on_pause: vec![vec!["A"]],
        ..Fake::default()
    };
    run_cases(&mut h, &[case("a", &[], &["A"])]);
    assert_eq!(dismissals(&h), 0);
}
