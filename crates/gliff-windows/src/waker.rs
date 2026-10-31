//! `PostWaker`: the session thread's [`Waker`], posting to the window.
//!
//! Each kind is posted at most once until the UI thread takes it
//! ([`PostWaker::take_frame`] / [`PostWaker::take_event`]), so a fast
//! session cannot flood the message queue.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

use crate::session::driver::Waker;
use crate::tabs::SessionId;

/// A picture is in the slot; `wparam` is the session.
pub const WM_APP_FRAME: u32 = WM_APP + 1;
/// A `UiEvent` is queued, or the session ended; `wparam` is the session.
pub const WM_APP_EVENT: u32 = WM_APP + 2;
/// A switch chord completed on the hook thread; `wparam` is the switch
/// ([`crate::chord_watch::switch_of_word`]).
pub const WM_APP_SWITCH: u32 = WM_APP + 3;

/// Posts [`WM_APP_FRAME`] and [`WM_APP_EVENT`] to one window, with the id of
/// the session in `wparam`: each session has its own waker, so a busy hidden
/// session never hides a shown one's message.
pub struct PostWaker {
    /// The `HWND` as an integer: `HWND` is not `Send`, but posting to a
    /// window from any thread is what `PostMessageW` is for.
    hwnd: isize,
    session: usize,
    frame: AtomicBool,
    event: AtomicBool,
}

impl PostWaker {
    pub fn new(hwnd: HWND, session: SessionId) -> Self {
        Self {
            hwnd: hwnd.0 as isize,
            session: session.get() as usize,
            frame: AtomicBool::new(false),
            event: AtomicBool::new(false),
        }
    }

    /// The UI thread saw [`WM_APP_FRAME`] (or is about to look at the
    /// slot anyway); the next `frame()` posts again.
    pub fn take_frame(&self) {
        self.frame.store(false, Ordering::Release);
    }

    /// As [`PostWaker::take_frame`], for events.
    pub fn take_event(&self) {
        self.event.store(false, Ordering::Release);
    }

    fn post(&self, flag: &AtomicBool, msg: u32) {
        if flag.swap(true, Ordering::AcqRel) {
            return;
        }
        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        // SAFETY: PostMessageW takes any HWND value; a destroyed window
        // makes it fail, which is harmless here (the UI is gone).
        let posted = unsafe { PostMessageW(Some(hwnd), msg, WPARAM(self.session), LPARAM(0)) };
        if posted.is_err() {
            flag.store(false, Ordering::Release);
        }
    }
}

impl Waker for PostWaker {
    fn frame(&self) {
        self.post(&self.frame, WM_APP_FRAME);
    }

    fn event(&self) {
        self.post(&self.event, WM_APP_EVENT);
    }
}
