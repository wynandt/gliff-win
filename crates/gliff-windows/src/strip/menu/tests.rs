use super::Command;
use super::Entry;
use super::Mark;
use super::RecentChoice;
use super::RecentRow;
use super::chevron_list;
use super::dot_command;
use super::recent_menu;
use super::run;
use super::tab_menu;
use crate::cli::Screen;
use crate::history::HostTarget;
use crate::machine_choices::Connection;
use crate::machine_choices::Origin;
use crate::machine_choices::Running;
use crate::strip::DotState;
use crate::tabs::Effect;
use crate::tabs::Status;
use crate::tabs::Tabs;

fn reconnecting() -> Status {
    Status::Reconnecting {
        attempt: 2,
        of: 5,
        why: "gone".into(),
        at: None,
    }
}

/// A machine with nothing chosen and nothing locked.
fn plain() -> Connection {
    Connection {
        screen: Screen::Mirror,
        screen_origin: Origin::Default,
        screen_lock: None,
        share_clipboard: true,
        share_origin: Origin::Default,
        share_lock: None,
        sharp_colour: false,
        sharp_origin: Origin::Default,
        gpu_decode: false,
        gpu_origin: Origin::Default,
        send_win_l: false,
        send_win_l_origin: Origin::Default,
        send_win_l_lock: None,
    }
}

fn labels(status: &Status) -> Vec<&'static str> {
    tab_menu(status, &plain(), None, None)
        .into_iter()
        .map(|e| match e {
            Entry::Item { label, .. } | Entry::Submenu { label, .. } => label,
            Entry::Separator => "-",
            Entry::Choice { .. } | Entry::Note(_) => "?",
        })
        .collect()
}

fn commands(status: &Status) -> Vec<Command> {
    tab_menu(status, &plain(), None, None)
        .into_iter()
        .filter_map(|e| match e {
            Entry::Item { command, .. } => Some(command),
            _ => None,
        })
        .collect()
}

/// The rows of the Connection settings submenu, as text: a heading or a
/// reason as `# text`, a row as `(o) text` or `[x] text`, with `~` when greyed.
fn connection_rows(connection: &Connection) -> Vec<String> {
    rows_for(&Status::Streaming, connection, None)
}

/// The same rows when this window cannot hand Win+L to any machine.
fn rows_unusable(connection: &Connection, reason: &str) -> Vec<String> {
    rows_from(tab_menu(&Status::Streaming, connection, None, Some(reason)))
}

/// The same rows for a tab with this status whose session started with `live`.
/// A row that is a command of its own shows as `=> text`.
fn rows_for(status: &Status, connection: &Connection, live: Option<&Running>) -> Vec<String> {
    rows_from(tab_menu(status, connection, live, None))
}

fn rows_from(entries: Vec<Entry>) -> Vec<String> {
    let rows = entries
        .into_iter()
        .find_map(|e| match e {
            Entry::Submenu {
                label: "Connection settings",
                rows,
            } => Some(rows),
            _ => None,
        })
        .expect("a Connection settings submenu");
    rows.into_iter()
        .map(|row| match row {
            Entry::Note(text) => format!("# {text}"),
            Entry::Separator => "-".to_owned(),
            Entry::Item { label, .. } => format!("=> {label}"),
            Entry::Choice {
                label,
                mark,
                selected,
                enabled,
                ..
            } => {
                let box_ = match (mark, selected) {
                    (Mark::Radio, true) => "(o)",
                    (Mark::Radio, false) => "( )",
                    (Mark::Check, true) => "[x]",
                    (Mark::Check, false) => "[ ]",
                };
                format!("{}{box_} {label}", if enabled { "" } else { "~" })
            }
            other => panic!("unexpected row {other:?}"),
        })
        .collect()
}

fn choice_commands(connection: &Connection) -> Vec<Command> {
    let entries = tab_menu(&Status::Streaming, connection, None, None);
    let rows = entries.into_iter().find_map(|e| match e {
        Entry::Submenu { rows, .. } => Some(rows),
        _ => None,
    });
    rows.expect("a submenu")
        .into_iter()
        .filter_map(|e| match e {
            Entry::Choice { command, .. } => Some(command),
            _ => None,
        })
        .collect()
}

#[test]
fn a_stopped_tab_can_be_connected_copied_and_forgotten() {
    assert_eq!(
        labels(&Status::Stopped),
        [
            "Connect",
            "-",
            "Copy address",
            "Connection settings",
            "-",
            "Forget"
        ]
    );
}

#[test]
fn a_tab_with_a_session_can_be_restarted_or_disconnected() {
    for status in [Status::Connecting, Status::Streaming, reconnecting()] {
        assert_eq!(
            labels(&status),
            [
                "Reconnect",
                "Disconnect",
                "-",
                "Copy address",
                "Connection settings",
                "-",
                "Disconnect and forget"
            ],
            "{status:?}"
        );
    }
}

#[test]
fn a_tab_that_gave_up_can_be_reconnected_but_has_nothing_to_disconnect() {
    let status = Status::GaveUp("no route".into());
    assert_eq!(
        labels(&status),
        [
            "Reconnect",
            "-",
            "Copy address",
            "Connection settings",
            "-",
            "Forget"
        ]
    );
    assert!(!commands(&status).contains(&Command::Disconnect));
}

#[test]
fn the_dot_button_stops_a_running_tab_and_forgets_the_rest() {
    for status in [Status::Connecting, Status::Streaming, reconnecting()] {
        assert_eq!(dot_command(&status), Command::Disconnect, "{status:?}");
    }
    for status in [Status::Stopped, Status::GaveUp("x".into())] {
        assert_eq!(dot_command(&status), Command::Forget, "{status:?}");
    }
}

#[test]
fn commands_become_calls_on_the_tabs() {
    let mut tabs = Tabs::new();
    tabs.open(&"u@one".parse().expect("a host"));
    tabs.open(&"u@two".parse().expect("a host"));
    let first = tabs.iter().next().expect("a tab").id();
    let second = tabs.iter().nth(1).expect("a tab").id();
    assert_eq!(tabs.tab(second).expect("tab").status(), &Status::Connecting);

    let effects = run(Command::Disconnect, second, &mut tabs);
    assert!(effects.contains(&Effect::Persist));
    assert_eq!(tabs.tab(second).expect("tab").status(), &Status::Stopped);

    let effects = run(Command::Connect, second, &mut tabs);
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Start { .. })),
        "{effects:?}"
    );
    assert_eq!(tabs.tab(second).expect("tab").status(), &Status::Connecting);

    assert!(run(Command::CopyAddress, first, &mut tabs).is_empty());
    for command in [Command::ShowAs(Screen::Own), Command::ShareClipboard(false)] {
        assert!(run(command, first, &mut tabs).is_empty(), "{command:?}");
    }
    assert_eq!(tabs.len(), 2, "a choice does not touch the tabs");
    run(Command::Forget, first, &mut tabs);
    assert_eq!(tabs.len(), 1);
}

#[test]
fn the_chevron_lists_every_tab_with_its_dot_chord_and_check() {
    let mut tabs = Tabs::new();
    for n in 1..=11 {
        tabs.open(&format!("u@h{n}").parse().expect("a host"));
    }
    let rows: Vec<_> = tabs
        .iter()
        .map(|t| (t.id(), t.target().to_string(), DotState::Live))
        .collect();
    let shown = rows[1].0;
    let list = chevron_list(&rows, Some(shown));
    assert_eq!(list.len(), 11);
    assert_eq!(list[0].label, "\u{25CF} u@h1\tCtrl+Alt+1");
    assert_eq!(list[8].label, "\u{25CF} u@h9\tCtrl+Alt+9");
    // The chords stop at 9.
    assert_eq!(list[9].label, "\u{25CF} u@h10");
    assert_eq!(
        list.iter()
            .filter(|r| r.checked)
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [shown]
    );
}

#[test]
fn every_dot_has_its_own_menu_character() {
    let mut seen: Vec<char> = [
        DotState::Stopped,
        DotState::Connecting,
        DotState::Live,
        DotState::Reconnecting,
        DotState::Failed,
    ]
    .into_iter()
    .map(super::dot_char)
    .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 5);
}

mod recent {
    use super::*;

    fn host(text: &str) -> HostTarget {
        text.parse().expect("a host")
    }

    fn item(label: &str, choice: RecentChoice) -> RecentRow {
        RecentRow::Item {
            label: label.to_owned(),
            choice: Some(choice),
        }
    }

    #[test]
    fn hosts_come_first_then_forget_and_clear_history() {
        let hosts = [host("user@203.0.113.1"), host("b@example.net")];
        assert_eq!(
            recent_menu(&hosts, true),
            vec![
                item("user@203.0.113.1", RecentChoice::Pick(hosts[0].clone())),
                item("b@example.net", RecentChoice::Pick(hosts[1].clone())),
                RecentRow::Separator,
                RecentRow::Submenu {
                    label: "Forget".to_owned(),
                    rows: vec![
                        item("user@203.0.113.1", RecentChoice::Forget(hosts[0].clone())),
                        item("b@example.net", RecentChoice::Forget(hosts[1].clone())),
                    ],
                },
                item("Clear history", RecentChoice::Clear),
            ]
        );
    }

    #[test]
    fn with_no_hosts_the_menu_says_so_and_offers_nothing() {
        assert_eq!(
            recent_menu(&[], true),
            vec![RecentRow::Item {
                label: "No recent hosts".to_owned(),
                choice: None,
            }]
        );
    }

    #[test]
    fn with_remembering_off_the_menu_says_that_instead() {
        assert_eq!(
            recent_menu(&[host("user@203.0.113.1")], false),
            vec![RecentRow::Item {
                label: "Recent hosts are off".to_owned(),
                choice: None,
            }]
        );
    }
}

#[test]
fn every_tab_menu_has_connection_settings_between_copy_address_and_forget() {
    for status in [
        Status::Stopped,
        Status::Connecting,
        Status::Streaming,
        reconnecting(),
        Status::GaveUp("x".into()),
    ] {
        let labels = labels(&status);
        let at = labels
            .iter()
            .position(|l| *l == "Connection settings")
            .unwrap_or_else(|| panic!("no submenu for {status:?}"));
        assert_eq!(labels[at - 1], "Copy address");
        assert_eq!(labels[at + 1], "-");
    }
}

#[test]
fn the_submenu_shows_the_machines_choices_and_when_they_apply() {
    assert_eq!(
        connection_rows(&plain()),
        [
            "# Show as",
            "(o) Mirror the remote screen",
            "( ) Own screen",
            "-",
            "[x] Share clipboard",
            "[ ] Sharp colour",
            "[ ] GPU decode",
            "-",
            "[ ] Send Win+L to this machine",
            "# Applies at once. Windows lock shortcuts are off while this window has the keyboard",
            "-",
            "# Applies the next time this tab connects",
        ]
    );
    let own = Connection {
        screen: Screen::Own,
        screen_origin: Origin::Machine,
        share_clipboard: false,
        share_origin: Origin::Machine,
        sharp_colour: true,
        sharp_origin: Origin::Machine,
        gpu_decode: true,
        gpu_origin: Origin::Machine,
        ..plain()
    };
    let rows = connection_rows(&own);
    assert_eq!(rows[1], "( ) Mirror the remote screen");
    assert_eq!(rows[2], "(o) Own screen");
    assert_eq!(rows[4], "[ ] Share clipboard");
    assert_eq!(rows[5], "[x] Sharp colour");
    assert_eq!(rows[6], "[x] GPU decode");
}

#[test]
fn a_row_asks_for_the_other_value_than_the_one_it_shows() {
    assert_eq!(
        choice_commands(&plain()),
        [
            Command::ShowAs(Screen::Mirror),
            Command::ShowAs(Screen::Own),
            Command::ShareClipboard(false),
            Command::SharpColour(true),
            Command::GpuDecode(true),
            Command::SendWinL(true),
        ]
    );
    let off = Connection {
        share_clipboard: false,
        ..plain()
    };
    assert_eq!(choice_commands(&off)[2], Command::ShareClipboard(true));
}

#[test]
fn a_locked_row_is_greyed_and_gives_its_reason() {
    let locked = Connection {
        screen: Screen::Own,
        screen_origin: Origin::Flag,
        screen_lock: Some("Locked: this window was started with --headless, which applies to every machine.".to_owned()),
        share_clipboard: false,
        share_origin: Origin::Flag,
        share_lock: Some("Locked: this window was started with --no-clipboard, which turns the clipboard off for every machine.".to_owned()),
        sharp_colour: false,
        sharp_origin: Origin::Default,
        gpu_decode: false,
        gpu_origin: Origin::Default,
        send_win_l: false,
        send_win_l_origin: Origin::Flag,
        send_win_l_lock: Some(
            "Locked: this window was started with --super-l, which applies to every machine."
                .to_owned(),
        ),
    };
    assert_eq!(
        connection_rows(&locked),
        [
            "# Show as",
            "~( ) Mirror the remote screen",
            "~(o) Own screen",
            "# Locked: this window was started with --headless, which applies to every machine.",
            "-",
            "~[ ] Share clipboard",
            "# Locked: this window was started with --no-clipboard, which turns the clipboard off for every machine.",
            "[ ] Sharp colour",
            "[ ] GPU decode",
            "-",
            "~[ ] Send Win+L to this machine",
            "# Locked: this window was started with --super-l, which applies to every machine.",
            "-",
            "# Applies the next time this tab connects",
        ]
    );
    // Only the screen locked: the share row still works.
    let screen_only = Connection {
        share_lock: None,
        share_clipboard: true,
        ..locked
    };
    let rows = connection_rows(&screen_only);
    assert!(rows.contains(&"[x] Share clipboard".to_owned()), "{rows:?}");
    // A route this window cannot use: the row keeps its tick, so it can be
    // turned off, and says why under it; a flag's reason still comes first.
    let unusable = "Not available: the registry refused it. Win+L locks Windows.";
    let chosen = Connection {
        send_win_l: true,
        send_win_l_origin: Origin::Machine,
        send_win_l_lock: None,
        ..plain()
    };
    let rows = rows_unusable(&chosen, unusable);
    let at = rows
        .iter()
        .position(|row| row == "[x] Send Win+L to this machine")
        .expect("the Win+L row, ticked and usable");
    assert_eq!(rows[at + 1], format!("# {unusable}"));
    let rows = rows_unusable(&screen_only, unusable);
    assert!(
        rows.contains(
            &"# Locked: this window was started with --super-l, which applies to every machine."
                .to_owned()
        ),
        "{rows:?}"
    );
}

/// What the live session of `plain()` started with.
fn started_like_plain() -> Running {
    plain().running()
}

fn own_screen() -> Connection {
    Connection {
        screen: Screen::Own,
        screen_origin: Origin::Machine,
        ..plain()
    }
}

#[test]
fn a_changed_choice_on_a_running_tab_offers_to_reconnect_instead_of_the_old_line() {
    let sharp = Connection {
        sharp_colour: true,
        sharp_origin: Origin::Machine,
        ..plain()
    };
    for (status, chosen) in [
        (Status::Connecting, own_screen()),
        (Status::Streaming, own_screen()),
        (Status::Streaming, sharp),
    ] {
        let rows = rows_for(&status, &chosen, Some(&started_like_plain()));
        assert_eq!(
            rows.last().map(String::as_str),
            Some("=> Reconnect now to apply"),
            "{status:?}: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("Applies the next time")),
            "{status:?}: {rows:?}"
        );
    }
}

#[test]
fn the_reconnect_row_does_what_the_reconnect_item_does() {
    let entries = tab_menu(
        &Status::Streaming,
        &own_screen(),
        Some(&started_like_plain()),
        None,
    );
    let rows = entries
        .iter()
        .find_map(|e| match e {
            Entry::Submenu { rows, .. } => Some(rows),
            _ => None,
        })
        .expect("a submenu");
    assert_eq!(
        rows.last(),
        Some(&Entry::Item {
            command: Command::Reconnect,
            label: "Reconnect now to apply",
        })
    );
    // The same command the top-level Reconnect runs: a new session now.
    assert_eq!(commands(&Status::Streaming)[0], Command::Reconnect);
}

#[test]
fn with_nothing_pending_the_static_line_stays() {
    // The session started with what is saved.
    let rows = rows_for(&Status::Streaming, &plain(), Some(&started_like_plain()));
    assert_eq!(
        rows.last().map(String::as_str),
        Some("# Applies the next time this tab connects")
    );
    // Changed and changed back.
    let live = own_screen().running();
    let rows = rows_for(&Status::Streaming, &own_screen(), Some(&live));
    assert_eq!(
        rows.last().map(String::as_str),
        Some("# Applies the next time this tab connects")
    );
}

#[test]
fn a_tab_with_no_live_session_keeps_the_static_line() {
    // Stopped, waiting to retry, or given up: the next connect applies the
    // choice anyway, so there is nothing to offer.
    for status in [Status::Stopped, reconnecting(), Status::GaveUp("x".into())] {
        let rows = rows_for(&status, &own_screen(), Some(&started_like_plain()));
        assert_eq!(
            rows.last().map(String::as_str),
            Some("# Applies the next time this tab connects"),
            "{status:?}"
        );
    }
    let rows = rows_for(&Status::Streaming, &own_screen(), None);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("# Applies the next time this tab connects")
    );
}
