//! When the keyboard goes to the remote machine: the pure decision behind the
//! hook. Events from the UI thread go in; the
//! hook mode, a release request and the lock setting's wish come out.

use clap::ValueEnum;

use crate::lock_policy::Want;

/// What the keyboard follows (`--grab-follows`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Follow {
    /// Keys go to the remote while the window has focus and is armed.
    Focus,
    /// Keys go to the remote while the pointer is over the window.
    Hover,
}

/// What the keyboard hook does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookMode {
    /// Not installed.
    Off,
    /// Only Super+L is taken (the lock setting is on and Windows would still
    /// see every other key).
    SuperLOnly,
    /// Every key is swallowed and forwarded.
    Grab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Gained,
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Entered,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typing {
    /// The address bar's field is open and has the keyboard.
    Started,
    /// It folded or lost the keyboard.
    Ended,
}

/// Which desktop receives input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    /// The normal desktop: ours.
    Normal,
    /// The secure desktop (Ctrl+Alt+Del, UAC, the lock screen): not ours.
    Secure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Unlocked,
    Locked,
}

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Focus(Focus),
    Pointer(Pointer),
    /// A mouse button went down on the picture.
    Click,
    /// The release chord completed.
    ReleaseChord,
    Desktop(Desktop),
    Session(Session),
    /// The address bar's field took or gave up the keyboard.
    Typing(Typing),
    /// The grab timer's clock, in ms since the client started; called every
    /// tick, and the only thing that can end a grab by time.
    Tick(u32),
}

/// Whether held keys and buttons must be released on the remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    Nothing,
    All,
}

/// The answer after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub mode: HookMode,
    pub release: Release,
    /// Whether the Windows lock setting should be on.
    pub lock: Want,
    /// This event ended the grab because `--grab-timeout-secs` ran out.
    pub timed_out: bool,
}

/// See the module documentation.
#[derive(Debug)]
pub struct GrabState {
    follow: Follow,
    focused: bool,
    hovered: bool,
    /// False after the release chord, until a click (focus) or the pointer
    /// leaving (hover).
    armed: bool,
    secure: bool,
    locked: bool,
    /// The address bar's field has the keyboard: keys are its own.
    typing: bool,
    mode: HookMode,
    /// `--grab-timeout-secs` in ms: how long a grab may last from the moment
    /// it starts or is armed again. `None`: no limit.
    timeout_ms: Option<u32>,
    /// The last [`Event::Tick`] time.
    now_ms: u32,
    /// When the current grab started (in `now_ms` time).
    grabbed_since: Option<u32>,
}

impl GrabState {
    pub fn new(follow: Follow) -> Self {
        Self::with_timeout(follow, None)
    }

    /// A state whose grab lasts at most `timeout_secs` seconds. The release
    /// is the one the release chord makes, so a click (focus) or the pointer
    /// leaving (hover) grabs again, for another `timeout_secs`.
    pub fn with_timeout(follow: Follow, timeout_secs: Option<u32>) -> Self {
        Self {
            follow,
            focused: false,
            hovered: false,
            armed: true,
            secure: false,
            locked: false,
            typing: false,
            mode: HookMode::Off,
            timeout_ms: timeout_secs.map(|s| s.saturating_mul(1000)),
            now_ms: 0,
            grabbed_since: None,
        }
    }

    pub fn mode(&self) -> HookMode {
        self.mode
    }

    /// The flags the mode is made of, for the log.
    pub fn describe(&self) -> String {
        format!(
            "follow={:?} focused={} hovered={} armed={} secure={} locked={} typing={}",
            self.follow,
            self.focused,
            self.hovered,
            self.armed,
            self.secure,
            self.locked,
            self.typing
        )
    }

    /// Whether the window messages say the pointer is over the window.
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// Applies an event and returns what the hook and the session do now.
    pub fn handle(&mut self, event: Event) -> Output {
        let mut timed_out = false;
        match event {
            Event::Tick(now) => {
                self.now_ms = now;
                if let (Some(limit), Some(since)) = (self.timeout_ms, self.grabbed_since)
                    && now.wrapping_sub(since) >= limit
                {
                    self.armed = false;
                    timed_out = true;
                }
            }
            Event::Focus(f) => self.focused = f == Focus::Gained,
            Event::Pointer(Pointer::Entered) => self.hovered = true,
            Event::Pointer(Pointer::Left) => {
                self.hovered = false;
                if self.follow == Follow::Hover {
                    self.armed = true;
                }
            }
            Event::Click => {
                if self.follow == Follow::Focus {
                    self.armed = true;
                }
            }
            Event::ReleaseChord => self.armed = false,
            Event::Desktop(d) => self.secure = d == Desktop::Secure,
            Event::Session(s) => self.locked = s == Session::Locked,
            Event::Typing(t) => self.typing = t == Typing::Started,
        }
        let before = self.mode;
        self.mode = self.compute();
        self.grabbed_since = match (self.mode, before) {
            (HookMode::Grab, HookMode::Grab) => self.grabbed_since,
            (HookMode::Grab, _) => Some(self.now_ms),
            _ => None,
        };
        let release = if before == HookMode::Grab && self.mode != HookMode::Grab {
            Release::All
        } else {
            Release::Nothing
        };
        let lock = if self.mode != HookMode::Off {
            Want::On
        } else {
            Want::Off
        };
        Output {
            mode: self.mode,
            release,
            lock,
            timed_out,
        }
    }

    fn compute(&self) -> HookMode {
        if self.secure || self.locked || self.typing {
            return HookMode::Off;
        }
        let grab = match self.follow {
            Follow::Focus => self.focused && self.armed,
            Follow::Hover => self.hovered && self.armed,
        };
        if grab {
            HookMode::Grab
        } else if self.hovered {
            HookMode::SuperLOnly
        } else {
            HookMode::Off
        }
    }
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The window itself.
    Window,
    /// One of its child windows (the strip, the address field, a panel).
    OwnChild,
    /// Another window.
    Other,
}

/// Whether the pointer is over the window's picture: the window itself, and
/// inside its client area. A child window takes the pointer from the window
/// (Windows sends it `WM_MOUSELEAVE`), so over a child it is not.
pub fn pointer_over(hit: Hit, in_client: bool) -> Option<bool> {
    match hit {
        Hit::Window => Some(in_client),
        Hit::OwnChild | Hit::Other => Some(false),
    }
}

/// Windows tells the window about the pointer with `WM_MOUSEMOVE` and
/// `WM_MOUSELEAVE`. Both follow a move of the pointer by the input system,
/// and a pointer put somewhere by another program (`SetCursorPos`) is not
/// always followed by either in good time. So every tick the window also asks
/// where the pointer is, and this decides when the answer is to be believed
/// over the messages.
#[derive(Debug, Default)]
pub struct PointerWatch {
    /// Ticks in a row where the pointer was not where the messages said.
    disagreed: u8,
    /// The poll made up an enter and a leave message undid it: the pointer
    /// is over something else of ours. No more enters from the poll until a
    /// real move message comes, or it would enter again on every tick.
    muted: bool,
    /// The last thing the poll did was to make up an enter.
    entered_by_poll: bool,
}

/// Ticks the two views must differ before the poll wins: one tick may catch
/// a window that is moving or a message that is on its way.
const POINTER_DISAGREE_TICKS: u8 = 2;

impl PointerWatch {
    /// A `WM_MOUSELEAVE` came.
    pub fn left_by_message(&mut self) {
        if std::mem::take(&mut self.entered_by_poll) {
            self.muted = true;
        }
    }

    /// A `WM_MOUSEMOVE` came.
    pub fn moved_by_message(&mut self) {
        self.muted = false;
        self.entered_by_poll = false;
    }

    /// `believed`: the messages say the pointer is over the window.
    /// `actual`: it is. `None` when the poll found nothing to go by (no
    /// cursor position); the count then starts again.
    pub fn observe(&mut self, believed: bool, actual: Option<bool>) -> Option<Pointer> {
        match actual {
            Some(actual) if actual != believed => {
                self.disagreed = self.disagreed.saturating_add(1);
                if self.disagreed < POINTER_DISAGREE_TICKS {
                    return None;
                }
                self.disagreed = 0;
                if actual {
                    if self.muted {
                        return None;
                    }
                    self.entered_by_poll = true;
                    Some(Pointer::Entered)
                } else {
                    self.entered_by_poll = false;
                    Some(Pointer::Left)
                }
            }
            _ => {
                self.disagreed = 0;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
