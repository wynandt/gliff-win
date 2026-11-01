use std::time::Duration;

use gliff_windows::grab_state::Event;
use gliff_windows::grab_state::Focus;
use gliff_windows::grab_state::Follow;
use gliff_windows::grab_state::GrabState;
use gliff_windows::grab_state::HookMode;
use gliff_windows::grab_state::Pointer;

use super::Act;
use super::MANUAL_CHORDS;
use super::Notepad;
use super::Place;
use super::Remote;
use super::Window;
use super::buttons;
use super::capture_follows_hover;
use super::chord;
use super::click_targets;
use super::clicks;
use super::hook_stall;
use super::injected_local;
use super::layouts;
use super::names;
use super::parse_chord_table;
use super::release_chords;
use super::standin_chord;
use super::super_chords;
use super::system_chords;
use super::wheel;
use crate::input_check::keys_plan;

/// The committed table, so the test fails if a row becomes unsendable.
const TABLE: &str = include_str!("../../../../../docs/phase4/super-chords.tsv");

#[test]
fn the_committed_table_has_twenty_rows_and_all_but_the_manual_ones_are_sent() {
    let rows = parse_chord_table(TABLE).expect("table parses");
    assert_eq!(rows.len(), 20, "20 Super chords");
    let cases = super_chords(&rows).expect("every row's keys are known");
    assert_eq!(
        cases.len(),
        18,
        "a case per row, less the two on the manual list"
    );
    for chord in MANUAL_CHORDS {
        assert!(
            rows.iter().any(|r| r.chord == chord),
            "{chord} is a row of the table, so the manual list covers a real chord"
        );
        assert!(
            cases.iter().all(|c| !c.name.contains(chord)),
            "{chord} is not sent by the gate"
        );
    }
}

#[test]
fn every_chord_case_that_windows_could_act_on_is_marked_risky() {
    let rows = parse_chord_table(TABLE).expect("table parses");
    let cases = super_chords(&rows).expect("cases");
    assert!(cases.iter().all(|c| c.risky), "Super chords are risky");
    assert!(system_chords().iter().all(|c| c.risky), "system chords too");
    assert!(
        layouts()
            .iter()
            .chain(injected_local().iter())
            .all(|c| !c.risky),
        "plain keys are not"
    );
}

#[test]
fn a_chord_case_expects_exactly_its_own_name() {
    let rows = parse_chord_table(TABLE).expect("table parses");
    let cases = super_chords(&rows).expect("cases");
    let shift_s = cases
        .iter()
        .find(|c| c.name.contains("SUPER+CTRL+D"))
        .expect("the virtual desktop row");
    assert_eq!(
        shift_s.expect.remote,
        Remote::Exactly(vec!["SUPER+CTRL+D".into()]),
        "the row's chord is the logged name"
    );
    let alone = cases
        .iter()
        .find(|c| c.name.contains("SUPER alone"))
        .expect("the first row");
    let keys = alone
        .acts
        .iter()
        .find_map(|a| match a {
            Act::Keys(k) => Some(k.len()),
            _ => None,
        })
        .expect("a Keys act");
    assert_eq!(keys, 2, "Super alone is one down and one up");
}

#[test]
fn a_table_row_without_a_number_is_an_error() {
    let err = parse_chord_table("n\tchord\nx\tSUPER+D\n").expect_err("bad row");
    assert!(err.contains("row number"), "says what is wrong: {err}");
}

#[test]
fn an_unknown_key_in_a_row_is_an_error_naming_it() {
    let rows = parse_chord_table("n\tchord\n1\tSUPER+Banana\n").expect("parses");
    let err = super_chords(&rows).expect_err("unknown key");
    assert!(err.contains("Banana"), "names the key: {err}");
}

#[test]
fn the_system_chords_are_eight_cases_ending_in_five_shifts() {
    let cases = system_chords();
    assert_eq!(cases.len(), 8, "seven chords and the Shift x5");
    assert_eq!(
        cases[7].expect.remote,
        Remote::Exactly(names(&[
            "SHIFT",
            "SHIFT",
            "SHIFT",
            "SHIFT",
            "SHIFT",
            "probe:none"
        ])),
        "five Shift entries, then the probe"
    );
}

#[test]
fn the_german_case_expects_a_z_typed_for_the_y_position() {
    let german = layouts()
        .into_iter()
        .find(|c| c.name == "layout german")
        .expect("german case");
    assert_eq!(german.expect.typed, Some(vec!["z"]), "the Y key types z");
    assert_eq!(
        german.expect.remote,
        Remote::Nothing,
        "no bind: binds follow the remote machine's layout, not the one sent"
    );
    assert!(
        german.expect.client.keymap_contains.contains(&"layout=de"),
        "the client reports the German keymap"
    );
}

#[test]
fn the_right_shift_case_follows_the_us_layout_and_expects_a_question_mark() {
    let cases = layouts();
    let names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
    let us = names.iter().position(|n| *n == "layout us").expect("us");
    let german = names
        .iter()
        .position(|n| *n == "layout german")
        .expect("german");
    let at = names
        .iter()
        .position(|n| *n == "layout us right shift")
        .expect("the right Shift case");
    assert!(us < at && at < german, "run while the US layout is active");
    let case = &cases[at];
    assert_eq!(case.expect.typed, Some(vec!["question"]), "`?`, not `/`");
    assert_eq!(case.expect.keys.need, [54, 53], "Shift_R and slash go down");
    assert_eq!(case.expect.keys.need_up, [54, 53], "and come back up");
}

#[test]
fn the_layout_cases_unload_what_they_load() {
    use super::LayoutStep;
    let steps: Vec<LayoutStep> = layouts()
        .into_iter()
        .flat_map(|c| c.acts)
        .filter_map(|a| match a {
            Act::Layout(s) => Some(s),
            _ => None,
        })
        .collect();
    let loads = steps
        .iter()
        .filter(|s| matches!(s, LayoutStep::Load(_)))
        .count();
    let unloads = steps
        .iter()
        .filter(|s| matches!(s, LayoutStep::Unload(_)))
        .count();
    assert_eq!(
        (loads, unloads),
        (2, 1),
        "US is loaded so it can be named (and is never unloaded: it was there); German is both"
    );
    assert_eq!(
        steps.last().map(|s| matches!(s, LayoutStep::Restore)),
        Some(true),
        "the last step puts the session's layout back"
    );
}

#[test]
fn the_release_chord_is_judged_by_raw_keys_not_modifier_release_binds() {
    use super::release_chords;
    let case = &release_chords()[0];
    assert_eq!(case.expect.keys.need, vec![29, 56], "Ctrl and Alt go down");
    assert_eq!(case.expect.keys.never, vec![102], "Home never does");
}

#[test]
fn local_injected_keys_must_never_reach_the_remote() {
    let keys = &injected_local()[0].expect.keys;
    assert!(
        keys.never.contains(&125),
        "LWin stays on the Windows machine"
    );
    assert_eq!(injected_local()[0].expect.remote, Remote::Nothing);
}

#[test]
fn flags_selected_by_the_cases_are_the_documented_ones() {
    assert_eq!(injected_local()[0].flags, ["--injected-keys", "local"]);
    assert_eq!(standin_chord()[0].flags, ["--super-l", "chord"]);
    assert_eq!(hook_stall()[0].flags, ["--hook-stall-test-ms", "2500"]);
    assert_eq!(
        hook_stall()[0].expect.client.min_hook_reinstalls,
        1,
        "a stall that never unhooked proves nothing"
    );
}

#[test]
fn the_held_ctrl_stall_case_judges_what_windows_typed() {
    let cases = hook_stall();
    let case = &cases[1];
    assert_eq!(case.flags, cases[0].flags, "the same stall, no restart");
    assert_eq!(case.expect.notepad, Notepad::Text("a"), "no Ctrl held");
    assert_eq!(case.expect.keys.never, vec![102], "Home never reaches it");
}

#[test]
fn there_are_nine_click_targets_inside_the_output() {
    let targets = click_targets((1920, 1200));
    assert_eq!(targets.len(), 9, "nine targets");
    assert!(
        targets
            .iter()
            .all(|(x, y)| (0.0..1920.0).contains(x) && (0.0..1200.0).contains(y)),
        "all inside"
    );
    assert_eq!(clicks((1920, 1200)).len(), 9, "one case each");
}

#[test]
fn the_thumb_buttons_are_checked_by_the_codes_each_mapping_sends() {
    let cases = buttons();
    let seen: Vec<_> = cases
        .iter()
        .map(|c| match &c.expect.remote {
            Remote::Exactly(entries) => (entries[0].clone(), c.flags.join(" ")),
            other => panic!("expected Exactly, got {other:?}"),
        })
        .collect();
    let want = [
        ("mouse:273", ""),
        ("mouse:274", ""),
        ("mouse:278", ""),
        ("mouse:277", ""),
        ("mouse:275", "--side-buttons linux-mouse"),
        ("mouse:276", "--side-buttons linux-mouse"),
    ];
    let want: Vec<_> = want
        .iter()
        .map(|(n, f)| ((*n).to_owned(), (*f).to_owned()))
        .collect();
    assert_eq!(seen, want, "upstream 278 and 277, Linux mouse 275 and 276");
}

#[test]
fn every_wheel_step_is_followed_by_more_than_the_scroll_event_delay() {
    // Hyprland's `binds:scroll_event_delay` defaults to 300 ms.
    let mut steps_without_wait = 0;
    for case in wheel() {
        let acts: Vec<_> = case
            .acts
            .iter()
            .filter(|a| matches!(a, Act::WheelEvent { .. } | Act::Pause(_)))
            .collect();
        for (i, act) in acts.iter().enumerate() {
            // Only a whole notch is a wheel step; anything finer is smooth.
            let Act::WheelEvent { delta, .. } = act else {
                continue;
            };
            if i32::from(*delta) % 120 == 0 {
                let waits = match acts.get(i + 1) {
                    Some(Act::Pause(d)) => *d > Duration::from_millis(300),
                    None => true,
                    Some(_) => false,
                };
                if !waits {
                    steps_without_wait += 1;
                }
            }
        }
    }
    assert_eq!(steps_without_wait, 0, "a step runs into the next event");
}

#[test]
fn the_layout_cases_ask_the_remote_for_focus_right_before_their_keys() {
    for case in layouts() {
        let Some(first_key) = case.acts.iter().position(|a| matches!(a, Act::Keys(_))) else {
            continue;
        };
        assert_eq!(
            case.acts.get(first_key.wrapping_sub(1)),
            Some(&Act::RemoteFocus),
            "{}: focus is the act before the first key",
            case.name
        );
    }
}

#[test]
fn the_hover_case_expects_what_the_remote_has_binds_for() {
    let hover = capture_follows_hover();
    let case = hover.last().expect("a case");
    assert_eq!(
        case.expect.remote,
        Remote::Exactly(names(&["A", "B"])),
        "the remote machine has no CTRL bind, and an extra bind fails the case"
    );
    assert_eq!(case.expect.keys.need, [29, 56], "modifiers by raw keys");
    assert_eq!(case.expect.keys.need_up, [29, 56], "and they come back up");
    assert_eq!(case.expect.client.grab, Some(true), "re-armed");
}

#[test]
fn the_release_chord_cases_need_the_modifiers_back_up() {
    for case in release_chords() {
        assert!(
            !case.expect.keys.need_up.is_empty(),
            "{}: a modifier left down shows only in the up",
            case.name
        );
        for code in &case.expect.keys.need {
            assert!(
                case.expect.keys.need_up.contains(code),
                "{}: {code} goes down, so it must come up",
                case.name
            );
        }
    }
}

#[test]
fn a_click_after_the_release_chord_takes_the_keyboard_back() {
    let cases = release_chords();
    let case = cases
        .iter()
        .find(|c| c.name.contains("click grabs again"))
        .expect("the re-grab case");
    let click = case
        .acts
        .iter()
        .position(|a| matches!(a, Act::Click { .. }))
        .expect("a click");
    let chord_at = case
        .acts
        .iter()
        .position(|a| matches!(a, Act::Keys(_)))
        .expect("the chord");
    assert!(chord_at < click, "the chord comes first");
    assert_eq!(
        case.flags,
        ["--grab-follows", "focus"],
        "its own flags, so the client starts afresh and grabbed"
    );
    assert_eq!(case.expect.client.grab, Some(true));
    assert!(
        case.expect.keys.never.contains(&46),
        "C stays off the remote machine"
    );
}

#[test]
fn every_system_chord_is_followed_by_the_modifier_probe() {
    for case in system_chords() {
        let Remote::Exactly(entries) = &case.expect.remote else {
            panic!("{}: Exactly", case.name);
        };
        assert_eq!(
            entries.last().map(String::as_str),
            Some("probe:none"),
            "{}: ends in the probe",
            case.name
        );
        let keys: Vec<_> = case
            .acts
            .iter()
            .filter(|a| matches!(a, Act::Keys(_)))
            .collect();
        assert_eq!(keys.len(), 2, "{}: the chord, then the probe", case.name);
    }
}

/// Replays the default plan against the client's own [`GrabState`] and
/// checks that every release case sends its chord to a client that is
/// grabbed. A case that follows another with the same flags inherits the
/// client the harness kept: after a release in focus mode, focusing the
/// window again does not grab (only a click does), so a second chord would go
/// to Windows and no Ctrl or Alt would be forwarded.
#[test]
fn a_release_case_sends_its_chord_to_a_client_that_is_grabbed() {
    let plan = keys_plan(&[]).expect("the default plan");
    let mut running: Option<Vec<String>> = None;
    let mut state = GrabState::new(Follow::Focus);
    let mut checked = 0;
    for case in &plan {
        if running.as_ref() != Some(&case.flags) {
            let hover = case.flags.iter().any(|f| f == "hover");
            state = GrabState::new(if hover { Follow::Hover } else { Follow::Focus });
            // A fresh client is focused with the pointer over it.
            state.handle(Event::Focus(Focus::Gained));
            state.handle(Event::Pointer(Pointer::Entered));
            running = Some(case.flags.clone());
        }
        let release = if case.flags.iter().any(|f| f == "shift+esc") {
            chord("Shift+Escape")
        } else {
            chord("Ctrl+Alt+Home")
        };
        for act in &case.acts {
            match act {
                Act::Focus(Window::Client) => {
                    state.handle(Event::Focus(Focus::Gained));
                }
                Act::Focus(Window::Notepad) => {
                    state.handle(Event::Focus(Focus::Lost));
                }
                Act::Pointer(Place::OverClient) => {
                    state.handle(Event::Pointer(Pointer::Entered));
                }
                Act::Pointer(Place::OffClient) => {
                    state.handle(Event::Pointer(Pointer::Left));
                }
                Act::Click { .. } => {
                    state.handle(Event::Click);
                }
                Act::Keys(presses) if *presses == release => {
                    if case.name.starts_with("release ") {
                        assert_eq!(
                            state.mode(),
                            HookMode::Grab,
                            "{}: the chord must reach a grabbed hook",
                            case.name
                        );
                        checked += 1;
                    }
                    state.handle(Event::ReleaseChord);
                }
                _ => {}
            }
        }
    }
    assert!(checked >= 3, "the release cases were replayed: {checked}");
}
