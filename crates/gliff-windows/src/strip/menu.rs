//! What the tab menu and the chevron list hold, as plain data. The window
//! builds a native popup menu from it and turns the chosen entry back into a
//! call on [`Tabs`].

use super::DotState;
use crate::cli::Screen;
use crate::history::HostTarget;
use crate::machine_choices::Connection;
use crate::machine_choices::Running;
use crate::tabs::Status;
use crate::tabs::TabId;
use crate::tabs::Tabs;

/// A command of a tab's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Start a tab that has no session.
    Connect,
    /// Start a new session now: the tab has one, or is waiting to retry, or
    /// gave up.
    Reconnect,
    /// End the session and keep the tab.
    Disconnect,
    /// Put the tab's address on the clipboard. The window does this; the
    /// model of the tabs has nothing to change.
    CopyAddress,
    /// Remove the tab, ending its session first.
    Forget,
    /// Choose how the machine is shown. The window saves the choice; the
    /// model of the tabs has nothing to change.
    ShowAs(Screen),
    /// Choose whether the machine's clipboard is shared. The value is the
    /// new choice.
    ShareClipboard(bool),
    /// Choose whether the machine's session asks for full-resolution colour
    /// (Dual420). The value is the new choice.
    SharpColour(bool),
}

/// How a row of a choice is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// One of several: a dot when it is the choice.
    Radio,
    /// On or off: a check when on.
    Check,
}

/// One row of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item {
        command: Command,
        label: &'static str,
    },
    /// A row that opens a menu of its own.
    Submenu {
        label: &'static str,
        rows: Vec<Entry>,
    },
    /// A row of a choice. A row that is not `enabled` is greyed, and does
    /// nothing.
    Choice {
        command: Command,
        label: &'static str,
        mark: Mark,
        selected: bool,
        enabled: bool,
    },
    /// Text that is greyed and does nothing: a heading or a reason.
    Note(String),
    Separator,
}

fn item(command: Command, label: &'static str) -> Entry {
    Entry::Item { command, label }
}

/// Whether the tab has a session or wants one.
fn running(status: &Status) -> bool {
    matches!(
        status,
        Status::Connecting | Status::Streaming | Status::Reconnecting { .. }
    )
}

/// The menu of a tab with this status. The entries that open a settings
/// window or a details panel come with those windows.
///
/// `live` is what the tab's session started with. When the tab has a session
/// and the saved choices would run differently now, the settings submenu ends
/// with a row that reconnects at once, in place of the line that says the
/// choice applies at the next connect.
pub fn tab_menu(status: &Status, connection: &Connection, live: Option<&Running>) -> Vec<Entry> {
    let pending = live.is_some_and(|live| {
        matches!(status, Status::Connecting | Status::Streaming) && connection.pending(live)
    });
    let mut entries = match status {
        Status::Stopped => vec![item(Command::Connect, "Connect")],
        Status::GaveUp(_) => vec![item(Command::Reconnect, "Reconnect")],
        _ => vec![
            item(Command::Reconnect, "Reconnect"),
            item(Command::Disconnect, "Disconnect"),
        ],
    };
    entries.push(Entry::Separator);
    entries.push(item(Command::CopyAddress, "Copy address"));
    entries.push(Entry::Submenu {
        label: "Connection settings",
        rows: connection_rows(connection, pending),
    });
    entries.push(Entry::Separator);
    entries.push(item(
        Command::Forget,
        if running(status) {
            "Disconnect and forget"
        } else {
            "Forget"
        },
    ));
    entries
}

/// The rows of a tab's Connection settings submenu: how the machine is
/// shown, whether its clipboard is shared and whether it asks for sharp
/// colour. A choice a flag has made for
/// every machine is greyed, with the reason under it. The last row is the
/// Reconnect command when a change `pending` waits for the next connect, and
/// a line that says when it applies otherwise.
fn connection_rows(connection: &Connection, pending: bool) -> Vec<Entry> {
    let mut rows = vec![Entry::Note("Show as".to_owned())];
    for (screen, label) in [
        (Screen::Mirror, "Mirror the remote screen"),
        (Screen::Own, "Own screen"),
    ] {
        rows.push(Entry::Choice {
            command: Command::ShowAs(screen),
            label,
            mark: Mark::Radio,
            selected: connection.screen == screen,
            enabled: connection.screen_lock.is_none(),
        });
    }
    if let Some(reason) = &connection.screen_lock {
        rows.push(Entry::Note(reason.clone()));
    }
    rows.push(Entry::Separator);
    rows.push(Entry::Choice {
        command: Command::ShareClipboard(!connection.share_clipboard),
        label: "Share clipboard",
        mark: Mark::Check,
        selected: connection.share_clipboard,
        enabled: connection.share_lock.is_none(),
    });
    if let Some(reason) = &connection.share_lock {
        rows.push(Entry::Note(reason.clone()));
    }
    rows.push(Entry::Choice {
        command: Command::SharpColour(!connection.sharp_colour),
        label: "Sharp colour",
        mark: Mark::Check,
        selected: connection.sharp_colour,
        enabled: true,
    });
    rows.push(Entry::Separator);
    rows.push(if pending {
        item(Command::Reconnect, "Reconnect now to apply")
    } else {
        Entry::Note("Applies the next time this tab connects".to_owned())
    });
    rows
}

/// What the button in a tab's slot does: it stops a tab that has a session
/// or wants one, and forgets one that has not. The middle button of the mouse
/// does the same on the whole tab.
pub fn dot_command(status: &Status) -> Command {
    if running(status) {
        Command::Disconnect
    } else {
        Command::Forget
    }
}

/// Runs a command on the tabs; the effects are for the caller to carry out.
/// `CopyAddress`, `ShowAs`, `ShareClipboard` and `SharpColour` have none:
/// the caller reads the address or saves the choice.
pub fn run(command: Command, id: TabId, tabs: &mut Tabs) -> Vec<crate::tabs::Effect> {
    match command {
        Command::Connect | Command::Reconnect => tabs.reconnect(id),
        Command::Disconnect => tabs.stop(id),
        Command::Forget => tabs.forget(id),
        Command::CopyAddress
        | Command::ShowAs(_)
        | Command::ShareClipboard(_)
        | Command::SharpColour(_) => Vec::new(),
    }
}

/// A row of the chevron's list: every tab, so one scrolled out of view can
/// be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub id: TabId,
    /// The dot as a character, then the name, then (for tabs 1 to 9) a tab
    /// and the chord.
    pub label: String,
    /// The shown tab carries a check.
    pub checked: bool,
}

/// The character that stands for a dot in a menu, which cannot draw one.
pub fn dot_char(dot: DotState) -> char {
    match dot {
        DotState::Stopped => '\u{25CB}',
        DotState::Connecting => '\u{25CC}',
        DotState::Live => '\u{25CF}',
        DotState::Reconnecting => '\u{25D0}',
        DotState::Failed => '\u{2715}',
    }
}

/// The chevron's list for tabs in the strip's order.
pub fn chevron_list(tabs: &[(TabId, String, DotState)], shown: Option<TabId>) -> Vec<ListRow> {
    tabs.iter()
        .enumerate()
        .map(|(i, (id, name, dot))| {
            let mut label = format!("{} {}", dot_char(*dot), name);
            if i < 9 {
                label.push_str(&format!("\tCtrl+Alt+{}", i + 1));
            }
            ListRow {
                id: *id,
                label,
                checked: shown == Some(*id),
            }
        })
        .collect()
}

/// What a pick in the recent-hosts menu does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecentChoice {
    /// Put this host in the field.
    Pick(HostTarget),
    /// Drop this host from the history.
    Forget(HostTarget),
    /// Drop the whole history.
    Clear,
}

/// A row of the recent-hosts menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecentRow {
    /// A row; with no choice it is greyed.
    Item {
        label: String,
        choice: Option<RecentChoice>,
    },
    Separator,
    /// A row that opens a menu of its own.
    Submenu {
        label: String,
        rows: Vec<RecentRow>,
    },
}

/// The menu of the list button: the hosts, most recent first, then a Forget
/// submenu with one row per host and Clear history. `remember` is the
/// privacy switch: with it off nothing is kept, and the menu says so.
pub fn recent_menu(hosts: &[HostTarget], remember: bool) -> Vec<RecentRow> {
    let note = |text: &str| {
        vec![RecentRow::Item {
            label: text.to_owned(),
            choice: None,
        }]
    };
    if !remember {
        return note("Recent hosts are off");
    }
    if hosts.is_empty() {
        return note("No recent hosts");
    }
    let row = |host: &HostTarget, choice: RecentChoice| RecentRow::Item {
        label: host.to_string(),
        choice: Some(choice),
    };
    let mut rows: Vec<RecentRow> = hosts
        .iter()
        .map(|host| row(host, RecentChoice::Pick(host.clone())))
        .collect();
    rows.push(RecentRow::Separator);
    rows.push(RecentRow::Submenu {
        label: "Forget".to_owned(),
        rows: hosts
            .iter()
            .map(|host| row(host, RecentChoice::Forget(host.clone())))
            .collect(),
    });
    rows.push(RecentRow::Item {
        label: "Clear history".to_owned(),
        choice: Some(RecentChoice::Clear),
    });
    rows
}

#[cfg(test)]
mod tests;
