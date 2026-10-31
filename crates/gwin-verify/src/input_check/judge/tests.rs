use gliff_windows::stats_out::InputInfo;

use super::Observed;
use super::judge;
use crate::input_check::cases::ClientExpect;
use crate::input_check::cases::Expect;
use crate::input_check::cases::Foreground;
use crate::input_check::cases::Notepad;
use crate::input_check::cases::Remote;
use crate::input_check::remote_log::Entry;

fn seen(whats: &[&str]) -> Observed {
    Observed {
        remote: whats
            .iter()
            .enumerate()
            .map(|(i, w)| Entry {
                ts: "t".into(),
                n: i as u64 + 1,
                what: (*w).into(),
            })
            .collect(),
        sent_keys: Vec::new(),
        foreground_is_client: true,
        notepad_text: Some(String::new()),
        input: None,
    }
}

fn exactly(names: &[&str]) -> Expect {
    Expect::remote(Remote::Exactly(
        names.iter().map(|s| (*s).to_owned()).collect(),
    ))
}

#[test]
fn exact_entries_in_order_pass() {
    assert!(
        judge(
            &exactly(&["SUPER+Y", "CTRL+Q"]),
            &seen(&["SUPER+Y", "CTRL+Q"])
        )
        .is_empty()
    );
}

#[test]
fn an_extra_or_missing_entry_fails_with_both_lists() {
    let reasons = judge(&exactly(&["SUPER+Y"]), &seen(&["SUPER+Y", "Home"]));
    assert_eq!(reasons.len(), 1, "one reason: {reasons:?}");
    assert!(
        reasons[0].contains("Home"),
        "names the extra entry: {reasons:?}"
    );
    assert_eq!(
        judge(&exactly(&["SUPER+Y"]), &seen(&[])).len(),
        1,
        "missing fails"
    );
}

#[test]
fn a_swapped_order_fails() {
    assert_eq!(judge(&exactly(&["A", "B"]), &seen(&["B", "A"])).len(), 1);
}

#[test]
fn nothing_fails_on_any_entry() {
    let expect = Expect::remote(Remote::Nothing);
    assert!(judge(&expect, &seen(&[])).is_empty(), "no entries passes");
    assert_eq!(judge(&expect, &seen(&["A"])).len(), 1, "an entry fails");
}

#[test]
fn in_order_allows_extras_between_but_not_the_forbidden_one() {
    let expect = Expect::remote(Remote::InOrder {
        entries: vec!["CTRL".into(), "ALT".into()],
        never: vec!["Home".into()],
    });
    assert!(
        judge(&expect, &seen(&["CTRL", "x", "ALT"])).is_empty(),
        "extras are fine"
    );
    assert_eq!(
        judge(&expect, &seen(&["ALT", "CTRL"])).len(),
        1,
        "wrong order"
    );
    assert_eq!(
        judge(&expect, &seen(&["CTRL", "ALT", "Home"])).len(),
        1,
        "Home appeared"
    );
}

#[test]
fn a_detail_after_a_space_still_matches_the_name() {
    let expect = Expect::remote(Remote::InOrder {
        entries: vec!["mouse:273".into()],
        never: vec![],
    });
    assert!(judge(&expect, &seen(&["mouse:273 10,20"])).is_empty());
    assert_eq!(
        judge(&expect, &seen(&["mouse:2730"])).len(),
        1,
        "only a space separates detail"
    );
}

#[test]
fn clicks_pass_within_a_pixel_and_fail_beyond() {
    let expect = Expect::remote(Remote::Clicks {
        targets: vec![(100.0, 200.0)],
        tolerance: 1.0,
    });
    assert!(
        judge(&expect, &seen(&["mouse:272 100.9,199.2"])).is_empty(),
        "inside"
    );
    let far = judge(&expect, &seen(&["mouse:272 102,200"]));
    assert_eq!(far.len(), 1, "2 px off fails: {far:?}");
    assert_eq!(
        judge(&expect, &seen(&["SUPER+Y"])).len(),
        1,
        "not a click entry"
    );
    assert_eq!(judge(&expect, &seen(&[])).len(), 1, "no click at all");
}

#[test]
fn foreground_and_notepad_are_judged_when_asked() {
    let mut expect = exactly(&[]);
    expect.foreground = Foreground::Client;
    expect.notepad = Notepad::Text("ab");
    let mut s = seen(&[]);
    s.notepad_text = Some("ab".into());
    assert!(judge(&expect, &s).is_empty(), "both hold");
    s.foreground_is_client = false;
    s.notepad_text = Some("a".into());
    assert_eq!(judge(&expect, &s).len(), 2, "both fail");
    s.notepad_text = None;
    assert!(
        judge(&expect, &s)
            .iter()
            .any(|r| r.contains("could not be read")),
        "an unreadable Notepad is a failure, not a pass"
    );
}

#[test]
fn client_expectations_read_the_input_section() {
    let mut expect = exactly(&[]);
    expect.client = ClientExpect {
        grab: Some(false),
        keymap_contains: vec!["layout=de"],
        ..ClientExpect::default()
    };
    let mut s = seen(&[]);
    assert_eq!(
        judge(&expect, &s).len(),
        1,
        "no input section is one failure"
    );
    s.input = Some(InputInfo {
        grab: false,
        keymap_sent: vec!["keymap_sent klid=00000407 layout=de reason=inputlang".into()],
        ..InputInfo::default()
    });
    assert!(judge(&expect, &s).is_empty(), "grab false and German sent");
    s.input.as_mut().expect("input").grab = true;
    assert_eq!(judge(&expect, &s).len(), 1, "grab still on");
}

#[test]
fn a_stall_that_never_unhooked_is_inconclusive_not_a_pass() {
    let mut expect = exactly(&[]);
    expect.client = ClientExpect {
        min_hook_reinstalls: 1,
        ..ClientExpect::default()
    };
    let mut s = seen(&[]);
    s.input = Some(InputInfo::default());
    let reasons = judge(&expect, &s);
    assert!(
        reasons.iter().any(|r| r.contains("inconclusive")),
        "no reinstall must fail: {reasons:?}"
    );
    s.input = Some(InputInfo {
        hook_reinstalls: 1,
        ..InputInfo::default()
    });
    assert!(judge(&expect, &s).is_empty(), "one reinstall satisfies it");
}

fn with_keys(whats: &[&str], sent: &[u16]) -> Observed {
    Observed {
        sent_keys: sent.to_vec(),
        ..seen(whats)
    }
}

#[test]
fn raw_key_and_typed_lines_do_not_count_as_bind_entries() {
    let s = seen(&["key:down:125", "SUPER+E", "key:up:125", "typed:y"]);
    let mut expect = exactly(&["SUPER+E"]);
    expect.keys.extra = vec![125];
    assert!(judge(&expect, &s).is_empty(), "only the bind is compared");
}

#[test]
fn a_key_the_case_did_not_send_is_a_stray() {
    let reasons = judge(&exactly(&[]), &with_keys(&["key:down:30"], &[29]));
    assert_eq!(reasons.len(), 1, "one reason: {reasons:?}");
    assert!(reasons[0].contains("30"), "names the code: {reasons:?}");
    let ok = judge(
        &exactly(&[]),
        &with_keys(&["key:down:29", "key:up:29"], &[29]),
    );
    assert!(ok.is_empty(), "a sent key is not a stray: {ok:?}");
}

#[test]
fn a_key_the_client_adds_is_allowed_only_when_the_case_says_so() {
    let mut expect = exactly(&[]);
    let s = with_keys(&["key:down:125", "key:up:125"], &[29]);
    assert_eq!(judge(&expect, &s).len(), 1, "undeclared extra fails");
    expect.keys.extra = vec![125];
    assert!(judge(&expect, &s).is_empty(), "declared extra passes");
}

#[test]
fn needed_and_forbidden_keys_are_judged_on_key_downs() {
    let mut expect = exactly(&[]);
    expect.keys.need = vec![29, 56];
    expect.keys.never = vec![102];
    let good = with_keys(
        &["key:down:29", "key:down:56", "key:up:56", "key:up:29"],
        &[29, 56, 102],
    );
    assert!(
        judge(&expect, &good).is_empty(),
        "Ctrl and Alt down, no Home"
    );
    let bad = with_keys(&["key:down:29", "key:down:102"], &[29, 56, 102]);
    let reasons = judge(&expect, &bad);
    assert_eq!(reasons.len(), 2, "Alt missing and Home down: {reasons:?}");
}

#[test]
fn a_key_that_must_go_up_fails_when_the_last_event_is_its_down() {
    let mut expect = exactly(&[]);
    expect.keys.need_up = vec![29];
    let sent = [29];
    let up = with_keys(&["key:down:29", "key:up:29"], &sent);
    assert!(judge(&expect, &up).is_empty(), "down then up passes");
    let held = with_keys(&["key:down:29"], &sent);
    let reasons = judge(&expect, &held);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("stuck"), "{reasons:?}");
    let again = with_keys(&["key:down:29", "key:up:29", "key:down:29"], &sent);
    assert_eq!(judge(&expect, &again).len(), 1, "held again at the end");
    let none = judge(&expect, &seen(&[]));
    assert_eq!(none.len(), 1, "a key that never arrived: {none:?}");
}

#[test]
fn the_typed_keysyms_must_match_in_order() {
    let mut expect = exactly(&[]);
    expect.typed = Some(vec!["z"]);
    assert!(judge(&expect, &seen(&["typed:z"])).is_empty());
    assert_eq!(judge(&expect, &seen(&["typed:y"])).len(), 1, "wrong keymap");
    let none = judge(&expect, &seen(&[]));
    assert!(
        none[0].contains("no `wev` window"),
        "an empty stream says why: {none:?}"
    );
}

/// The remote log of the first hardware run (remote machine time): every chord was
/// preceded by an `other` line the catch-all bind wrote for the lone modifier.
const RUN1: &str = include_str!("fixtures/run1-remote.log");

/// Entries `from..=to` of the first run's log, less the `other` lines the
/// remote-side script no longer writes.
fn run1(from: u64, to: u64) -> Observed {
    let parsed = crate::input_check::remote_log::parse(RUN1);
    assert_eq!(parsed.malformed, 0, "the fixture is whole");
    Observed {
        remote: parsed
            .entries
            .into_iter()
            .filter(|e| e.n >= from && e.n <= to && e.what != "other")
            .collect(),
        ..seen(&[])
    }
}

#[test]
fn the_first_runs_other_lines_are_what_failed_every_chord() {
    let parsed = crate::input_check::remote_log::parse(RUN1);
    let others = parsed.entries.iter().filter(|e| e.what == "other").count();
    assert!(others >= 40, "a line per chord at least: {others}");
    let with_noise = Observed {
        remote: parsed.entries[6..8].to_vec(),
        ..seen(&[])
    };
    assert_eq!(
        with_noise
            .remote
            .iter()
            .map(|e| e.what.as_str())
            .collect::<Vec<_>>(),
        ["other", "SUPER+E"],
        "chord 4 as it was logged"
    );
    assert_eq!(
        judge(&exactly(&["SUPER+E"]), &with_noise).len(),
        1,
        "failed on the noise"
    );
    assert!(
        judge(&exactly(&["SUPER+E"]), &run1(7, 8)).is_empty(),
        "and passes without it"
    );
}

#[test]
fn the_first_runs_other_chords_pass_once_the_noise_is_gone() {
    use crate::input_check::cases::parse_chord_table;
    use crate::input_check::cases::super_chords;
    let rows = parse_chord_table(include_str!("../../../../../docs/phase4/super-chords.tsv"))
        .expect("table");
    let cases = super_chords(&rows).expect("cases");
    // (case name part, log entries) of the run, row by row; the chords the
    // run logged one entry each.
    let by_entry: &[(&str, u64)] = &[
        ("SUPER alone", 2),
        ("SUPER+Return", 4),
        ("SUPER+D", 6),
        ("SUPER+R", 10),
        ("SUPER+Tab", 12),
        ("SUPER+Left", 26),
        ("SUPER+CTRL+D", 36),
    ];
    for (name, n) in by_entry {
        let case = cases
            .iter()
            .find(|c| c.name.ends_with(name))
            .unwrap_or_else(|| panic!("a case for {name}"));
        let mut observed = run1(*n, *n);
        observed.sent_keys = Vec::new();
        observed.foreground_is_client = true;
        // The key lines are not in this older log; the bind line is.
        let reasons = judge(&case.expect, &observed);
        assert!(reasons.is_empty(), "{name}: {reasons:?}");
    }
}

#[test]
fn the_stand_in_and_shift_x5_of_the_first_run_pass_the_new_expectations() {
    use crate::input_check::cases::standin_chord;
    use crate::input_check::cases::system_chords;
    let standin = standin_chord().remove(0);
    let mut observed = run1(90, 95);
    observed.foreground_is_client = true;
    assert_eq!(
        observed
            .remote
            .iter()
            .map(|e| e.what.as_str())
            .collect::<Vec<_>>(),
        ["SUPER+L", "probe:none"]
    );
    assert!(judge(&standin.expect, &observed).is_empty(), "stand-in");
    // The first run had no probe after the Shifts: the stand-in's probe
    // entry stands in for the one the case now adds.
    let probe = run1(94, 94).remote;
    let shifts = system_chords().pop().expect("Shift x5 is last");
    let mut observed = run1(51, 60);
    observed.remote.extend(probe);
    observed.foreground_is_client = true;
    assert!(
        judge(&shifts.expect, &observed).is_empty(),
        "five Shift entries"
    );
}

#[test]
fn the_injected_local_case_expected_the_wrong_thing_in_the_first_run() {
    // The first run expected a `probe:none` entry from a key that stays on
    // the Windows machine; the remote machine (correctly) saw nothing of it.
    use crate::input_check::cases::injected_local;
    let case = injected_local().remove(0);
    let mut observed = seen(&[]);
    observed.foreground_is_client = true;
    assert!(
        judge(&case.expect, &observed).is_empty(),
        "nothing is right"
    );
    let leaked = with_keys(&["key:down:125"], &[125]);
    let reasons = judge(&case.expect, &leaked);
    assert_eq!(reasons.len(), 1, "the forbidden key: {reasons:?}");
}
