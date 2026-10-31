//! The address bar's state: whether it is open, what is typed, and what a
//! press of Enter does.
//!
//! The bar is a text field that opens on the strip when the `+` is clicked,
//! and by itself when the window has no tab. It is pure: the Win32 field
//! sends [`Event`]s and carries out the [`Act`]s.

use crate::history::HostTarget;
use crate::history::HostTargetError;

/// What the field or the strip reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The `+` was clicked.
    Open,
    /// The field's text changed to this.
    Text(String),
    /// The field gained or lost the keyboard.
    Focus(bool),
    /// Enter was pressed in the field.
    Enter,
    /// The go button was clicked.
    Go,
    /// Escape was pressed in the field.
    Escape,
    /// A recent host was picked from the list.
    Pick(String),
    /// The list of recent hosts opened or closed.
    List(bool),
    /// The tab on show connected.
    Connected,
}

/// What the Win32 side must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Give the field the keyboard.
    Focus,
    /// Put this text in the field and the caret at its end.
    SetText(String),
    /// Open this machine (the tab model decides between a new tab and an
    /// existing one).
    Connect(HostTarget),
    /// Give the keyboard back to the picture.
    FocusMain,
}

/// The bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Address {
    open: bool,
    text: String,
    focused: bool,
    list: bool,
    error: Option<HostTargetError>,
    /// The user typed in the field since it opened (text the program put
    /// there does not count).
    edited: bool,
}

impl Address {
    /// A folded bar.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the field is shown.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The text in the field.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the field has the keyboard (the grab must let go).
    pub fn typing(&self) -> bool {
        self.open && self.focused
    }

    /// Why the last Enter was refused, if it was and nothing was typed since.
    pub fn error(&self) -> Option<&HostTargetError> {
        self.error.as_ref()
    }

    /// Handle an event. `tabs` is how many tabs the window has.
    pub fn input(&mut self, event: Event, tabs: usize) -> Vec<Act> {
        match event {
            Event::Open => {
                if !self.open {
                    self.edited = false;
                }
                self.open = true;
                vec![Act::Focus]
            }
            Event::Text(text) => {
                self.edited |= text != self.text;
                self.text = text;
                self.error = None;
                Vec::new()
            }
            Event::Focus(focused) => {
                self.focused = focused;
                if !focused && self.open && !self.list && self.text.is_empty() && tabs > 0 {
                    self.fold();
                }
                Vec::new()
            }
            Event::Enter | Event::Go => self.connect(),
            Event::Escape if tabs == 0 => {
                // There is no tab to go back to, so the bar stays.
                self.error = None;
                if std::mem::take(&mut self.text).is_empty() {
                    Vec::new()
                } else {
                    vec![Act::SetText(String::new())]
                }
            }
            Event::Escape => {
                self.fold();
                Vec::new()
            }
            Event::Pick(text) => {
                self.list = false;
                self.edited = false;
                self.error = None;
                self.text.clone_from(&text);
                vec![Act::SetText(text), Act::Focus]
            }
            Event::Connected if self.open && !self.edited && tabs > 0 => {
                self.fold();
                vec![Act::FocusMain]
            }
            Event::Connected => Vec::new(),
            Event::List(shown) => {
                self.list = shown;
                if !shown && self.open && !self.focused {
                    vec![Act::Focus]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// The window now has `tabs` tabs: with none, the bar opens.
    pub fn tabs_changed(&mut self, tabs: usize) -> Vec<Act> {
        if tabs == 0 && !self.open {
            self.open = true;
            vec![Act::Focus]
        } else {
            Vec::new()
        }
    }

    fn connect(&mut self) -> Vec<Act> {
        let text = self.text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        match text.parse::<HostTarget>() {
            Ok(target) => {
                self.fold();
                vec![Act::Connect(target)]
            }
            Err(error) => {
                self.error = Some(error);
                Vec::new()
            }
        }
    }

    fn fold(&mut self) {
        self.open = false;
        self.list = false;
        self.text.clear();
        self.error = None;
        self.edited = false;
    }
}

#[cfg(test)]
mod tests;
