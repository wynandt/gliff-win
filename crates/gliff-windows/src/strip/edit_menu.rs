//! The right-click menu of the address bar's edit field: Cut, Copy, Paste and
//! Select All. The stock Windows edit menu also offers reading order, Unicode
//! control characters and input-method items, which mean nothing in a
//! `user@host` field. This is the pure side: which rows, which are enabled,
//! and which message of the edit control carries each one out. The window
//! shows the menu and sends the message.

/// What a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// What the field and the clipboard hold when the menu opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Some text in the field is selected.
    pub has_selection: bool,
    /// The field is not empty.
    pub has_text: bool,
    /// The clipboard holds text.
    pub can_paste: bool,
}

/// One row: its command, its label (with the shortcut after a tab) and whether
/// it can be used now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub command: EditCommand,
    pub label: &'static str,
    pub enabled: bool,
}

/// The rows, top to bottom. Select All follows a separator, which the window
/// draws.
pub fn rows(facts: Facts) -> Vec<Row> {
    let row = |command, label, enabled| Row {
        command,
        label,
        enabled,
    };
    vec![
        row(EditCommand::Cut, "Cut\tCtrl+X", facts.has_selection),
        row(EditCommand::Copy, "Copy\tCtrl+C", facts.has_selection),
        row(EditCommand::Paste, "Paste\tCtrl+V", facts.can_paste),
        row(EditCommand::SelectAll, "Select All\tCtrl+A", facts.has_text),
    ]
}

/// A window message for the edit control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub msg: u32,
    pub wparam: usize,
    pub lparam: isize,
}

const WM_CUT: u32 = 0x0300;
const WM_COPY: u32 = 0x0301;
const WM_PASTE: u32 = 0x0302;
const EM_SETSEL: u32 = 0x00b1;

/// The message that does what the row says. These are the messages the stock
/// menu sends, so Cut, Copy and Paste behave as they did.
pub fn message(command: EditCommand) -> Message {
    let (msg, wparam, lparam) = match command {
        EditCommand::Cut => (WM_CUT, 0, 0),
        EditCommand::Copy => (WM_COPY, 0, 0),
        EditCommand::Paste => (WM_PASTE, 0, 0),
        // Start 0, end -1: the whole text.
        EditCommand::SelectAll => (EM_SETSEL, 0, -1),
    };
    Message {
        msg,
        wparam,
        lparam,
    }
}

#[cfg(test)]
mod tests;
