//! The countdown window of the lock step: a small banner at the top of the
//! screen that is always on top, never takes the focus and lets mouse
//! clicks through, so the client keeps the foreground while it counts.
//! It lives on its own thread with its own message loop.

use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::Context;
use anyhow::bail;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics;
use windows::Win32::UI::WindowsAndMessaging::LWA_ALPHA;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
use windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WM_QUIT;
use windows::Win32::UI::WindowsAndMessaging::WM_SETTEXT;
use windows::Win32::UI::WindowsAndMessaging::WS_BORDER;
use windows::Win32::UI::WindowsAndMessaging::WS_EX_LAYERED;
use windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW;
use windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST;
use windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT;
use windows::Win32::UI::WindowsAndMessaging::WS_POPUP;
use windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE;
use windows::core::HSTRING;
use windows::core::w;

/// `SS_CENTER | SS_CENTERIMAGE`: centred both ways.
const STATIC_CENTRED: u32 = 0x0001 | 0x0200;
/// Banner height in physical pixels (the process is per-monitor DPI aware).
const HEIGHT: i32 = 96;
/// Banner width at most, in physical pixels.
const WIDTH: i32 = 1400;

pub struct Overlay {
    hwnd: isize,
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl Overlay {
    pub fn open(text: &str) -> anyhow::Result<Self> {
        let title = HSTRING::from(text);
        let (tx, rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("countdown-overlay".to_owned())
            .spawn(move || {
                // SAFETY: no arguments.
                let screen = unsafe { GetSystemMetrics(SM_CXSCREEN) };
                let width = WIDTH.min(screen);
                let ex = WS_EX_TOPMOST
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT;
                let style = WS_POPUP.0 | WS_VISIBLE.0 | WS_BORDER.0 | STATIC_CENTRED;
                // SAFETY: a system class and a title that outlives the call;
                // no parent. WS_EX_NOACTIVATE keeps the focus where it was.
                let hwnd = unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE(ex.0),
                        w!("STATIC"),
                        &title,
                        WINDOW_STYLE(style),
                        (screen - width) / 2,
                        0,
                        width,
                        HEIGHT,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                // SAFETY: no arguments.
                let thread_id = unsafe { GetCurrentThreadId() };
                if let Ok(h) = hwnd {
                    // Mostly opaque; a layered window also lets clicks through.
                    // SAFETY: our own window.
                    let _ = unsafe { SetLayeredWindowAttributes(h, COLORREF(0), 235, LWA_ALPHA) };
                }
                let made = hwnd
                    .as_ref()
                    .map(|h| h.0 as isize)
                    .map_err(|e| format!("{e}"));
                let _ = tx.send((made, thread_id));
                let Ok(hwnd) = hwnd else {
                    return;
                };
                let mut msg = MSG::default();
                // SAFETY: `msg` is a live MSG; the loop ends on WM_QUIT.
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                    // SAFETY: the message GetMessageW just filled in.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                // SAFETY: the window this thread created.
                let _ = unsafe { DestroyWindow(hwnd) };
            })
            .context("starting the countdown overlay thread")?;
        let (made, thread_id) = rx.recv().context("the countdown overlay thread ended")?;
        match made {
            Ok(hwnd) => Ok(Self {
                hwnd,
                thread_id,
                join: Some(join),
            }),
            Err(e) => bail!("CreateWindowExW(STATIC) failed: {e}"),
        }
    }

    pub fn set_text(&self, text: &str) {
        let wide = HSTRING::from(text);
        // SAFETY: SendMessage across threads to our own window; `wide` is a
        // NUL-terminated wide string that outlives the call.
        unsafe {
            SendMessageW(
                HWND(self.hwnd as *mut _),
                WM_SETTEXT,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            )
        };
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: a thread id this process created; WM_QUIT ends its loop.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
