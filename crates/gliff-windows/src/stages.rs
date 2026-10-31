//! Which [`Stage`] the window is in, from what it did: pure, so the rules
//! are tested on the build machine and `app` only reports events.
//!
//! - Starting until the first picture is shown.
//! - Resizing from a client-area change until a picture of that size is shown, or
//!   [`Stages::GIVE_UP`] passes (mirror mode never changes its view).
//! - Otherwise Windowed or Fullscreen, as the window is.
//! - Closing from `WM_CLOSE` on, whatever else happens.

use std::time::Duration;
use std::time::Instant;

use crate::present_log::Stage;
use crate::session::ResizeReq;

/// Windowed or borderless fullscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Windowed,
    Fullscreen,
}

/// The stage rules.
#[derive(Debug, Clone)]
pub struct Stages {
    screen: Screen,
    shown: bool,
    /// The size a Resize asked for and when it was sent.
    awaiting: Option<((u32, u32), Instant)>,
    closing: bool,
}

impl Default for Stages {
    fn default() -> Self {
        Self::new()
    }
}

impl Stages {
    /// How long a Resize keeps the window in [`Stage::Resizing`] without a
    /// picture of its size.
    pub const GIVE_UP: Duration = Duration::from_secs(3);

    pub fn new() -> Self {
        Self {
            screen: Screen::Windowed,
            shown: false,
            awaiting: None,
            closing: false,
        }
    }

    /// The stage now.
    pub fn current(&self, now: Instant) -> Stage {
        if self.closing {
            return Stage::Closing;
        }
        if !self.shown {
            return Stage::Starting;
        }
        match self.awaiting {
            Some((_, sent)) if now.saturating_duration_since(sent) < Self::GIVE_UP => {
                Stage::Resizing
            }
            _ => match self.screen {
                Screen::Windowed => Stage::Windowed,
                Screen::Fullscreen => Stage::Fullscreen,
            },
        }
    }

    /// The client area changed to `req`'s size: a Resize follows (after
    /// the settle or the drag) unless the gate finds it already applied.
    /// Not the Hello-time size, which Starting covers.
    pub fn on_resize(&mut self, req: ResizeReq, now: Instant) {
        self.awaiting = Some(((req.width, req.height), now));
    }

    /// F11 or the menu item took effect.
    pub fn on_screen(&mut self, screen: Screen) {
        self.screen = screen;
    }

    /// `WM_CLOSE` (or the session ended): [`Stage::Closing`] from now on,
    /// so the open stage stops at the close and teardown is never judged.
    pub fn on_close(&mut self) {
        self.closing = true;
    }

    /// A picture of `view` was shown.
    pub fn on_shown(&mut self, view: (u32, u32)) {
        self.shown = true;
        if self.awaiting.is_some_and(|(size, _)| size == view) {
            self.awaiting = None;
        }
    }
}

#[cfg(test)]
mod tests;
