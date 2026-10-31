//! The pure decision behind the once-a-second foreground check.
//!
//! The keyboard grab follows `WM_SETFOCUS` and `WM_KILLFOCUS`. Windows does not
//! always send the second one: a game that starts in front of the client took
//! the foreground, the client heard nothing, and the hook went on swallowing
//! every key until Ctrl+Alt+Del. So while the keyboard is grabbed by focus, the
//! window asks which window is in front, about once a second, and this module
//! says what that means. No Win32 here; the window turns what it read into a
//! [`Relation`] and passes it in.
//!
//! One rule keeps it safe: only another program's window counts against the
//! grab, and only when it is seen twice in a row. No window, an unreadable
//! window, our own windows and any other window of our process all count for
//! nothing.

use crate::grab_state::Focus;
use crate::grab_state::Follow;
use crate::grab_state::HookMode;

/// How many looks in a row at another program's window make a lost focus. At
/// the window's cadence of about one look a second this is about two seconds.
pub const LOOKS_TO_DECIDE: u8 = 2;

/// How the window in front relates to ours, as read from Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// There is no foreground window (null): the foreground is changing, or it
    /// is the secure desktop.
    None,
    /// It is our top-level window.
    Is,
    /// It is a child of ours, or a window our window owns: a menu, a tooltip,
    /// a dialog.
    OwnedOrChild,
    /// It belongs to our process but to no window of ours: a menu window or an
    /// IME window made on our thread.
    SameProcess,
    /// It belongs to another process.
    Foreign,
    /// Windows would not say which process it belongs to.
    Unreadable,
}

/// What a look shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foreground {
    /// The window in front is ours.
    Ours,
    /// The window in front is another program's.
    Other,
    /// The look proves nothing either way.
    Unknown,
}

pub fn classify(relation: Relation) -> Foreground {
    match relation {
        Relation::Is | Relation::OwnedOrChild | Relation::SameProcess => Foreground::Ours,
        Relation::Foreign => Foreground::Other,
        Relation::None | Relation::Unreadable => Foreground::Unknown,
    }
}

/// Whether this grab is the kind the check is for: the keyboard held because
/// our window has the focus. Hover mode grabs on purpose while another window
/// is in front.
pub fn grabbing_by_focus(follow: Follow, mode: HookMode) -> bool {
    follow == Follow::Focus && mode == HookMode::Grab
}

/// The count of looks, and whether the last loss was made up by a look.
#[derive(Debug, Default)]
pub struct ForegroundWatch {
    /// Looks in a row at another program's window while grabbing.
    others: u8,
    /// Looks in a row at our own window since a made-up loss.
    ours: u8,
    /// A look ended the grab and no focus message has said anything since.
    made_up_loss: bool,
}

impl ForegroundWatch {
    /// Whether the window needs to ask Windows now. False in the common case
    /// of a mode that is not a focus grab, so the check costs nothing there.
    pub fn active(&self, grabbing: bool) -> bool {
        grabbing || self.made_up_loss
    }

    /// One look. `grabbing` is whether the grab is on by focus right now.
    /// Gives the focus event to feed the grab state, if the looks decide one.
    pub fn look(&mut self, grabbing: bool, seen: Foreground) -> Option<Focus> {
        if self.made_up_loss {
            self.others = 0;
            if seen == Foreground::Ours {
                self.ours += 1;
            } else {
                self.ours = 0;
            }
            if self.ours >= LOOKS_TO_DECIDE {
                self.made_up_loss = false;
                self.ours = 0;
                return Some(Focus::Gained);
            }
            return None;
        }
        if !grabbing {
            self.others = 0;
            return None;
        }
        if seen == Foreground::Other {
            self.others += 1;
        } else {
            self.others = 0;
        }
        if self.others >= LOOKS_TO_DECIDE {
            self.others = 0;
            self.made_up_loss = true;
            return Some(Focus::Lost);
        }
        None
    }

    /// A real `WM_SETFOCUS` or `WM_KILLFOCUS` arrived: Windows has spoken, so
    /// the looks start over.
    pub fn focus_message(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests;
