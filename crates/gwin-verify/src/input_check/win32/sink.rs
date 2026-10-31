//! A small always-on-top text field the gate types into, standing in for
//! Notepad: it needs no app, has no autosave or completion, and its text
//! reads back with one message. Created on first use, on its own thread
//! with its own message loop.

use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::Context;
use anyhow::bail;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WM_GETTEXT;
use windows::Win32::UI::WindowsAndMessaging::WM_GETTEXTLENGTH;
use windows::Win32::UI::WindowsAndMessaging::WM_QUIT;
use windows::Win32::UI::WindowsAndMessaging::WM_SETTEXT;
use windows::Win32::UI::WindowsAndMessaging::WS_BORDER;
use windows::Win32::UI::WindowsAndMessaging::WS_CAPTION;
use windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE;
use windows::core::w;

/// `ES_MULTILINE`.
const ES_MULTILINE: u32 = 0x0004;
pub const SIZE: (i32, i32) = (320, 160);

pub struct Sink {
    hwnd: isize,
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl Sink {
    pub fn open() -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("text-sink".to_owned())
            .spawn(move || {
                let style = WS_VISIBLE.0 | WS_CAPTION.0 | WS_BORDER.0 | ES_MULTILINE;
                // SAFETY: a system class and a static title; no parent.
                let hwnd = unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("EDIT"),
                        w!("gwin-verify text sink"),
                        WINDOW_STYLE(style),
                        0,
                        0,
                        SIZE.0,
                        SIZE.1,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                // SAFETY: no arguments.
                let thread_id = unsafe { GetCurrentThreadId() };
                let made = hwnd.map(|h| h.0 as isize).map_err(|e| format!("{e}"));
                let ok = made.is_ok();
                let _ = tx.send((made, thread_id));
                if !ok {
                    return;
                }
                let mut msg = MSG::default();
                // SAFETY: `msg` is a live MSG; the loop ends on WM_QUIT.
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                    // SAFETY: the message GetMessageW just filled in.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .context("starting the text sink thread")?;
        let (made, thread_id) = rx.recv().context("the text sink thread ended")?;
        match made {
            Ok(hwnd) => Ok(Self {
                hwnd,
                thread_id,
                join: Some(join),
            }),
            Err(e) => bail!("CreateWindowExW(EDIT) failed: {e}"),
        }
    }

    fn handle(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    /// Puts the window over the client's top-left corner (screen pixels).
    pub fn place_over(&self, x: i32, y: i32) -> anyhow::Result<()> {
        // SAFETY: our own window; plain integers.
        unsafe {
            SetWindowPos(
                self.handle(),
                Some(HWND_TOPMOST),
                x,
                y,
                SIZE.0,
                SIZE.1,
                SWP_NOACTIVATE,
            )
        }
        .context("SetWindowPos(text sink)")
    }

    pub fn center(&self) -> (i32, i32) {
        let mut r = windows::Win32::Foundation::RECT::default();
        // SAFETY: our own window and a live RECT.
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(self.handle(), &mut r)
        };
        ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
    }

    pub fn focus(&self) -> bool {
        super::foreground::force(self.handle())
    }

    pub fn clear(&self) {
        // SAFETY: SendMessage across threads to our own window; the string
        // is a static NUL-terminated wide string.
        unsafe {
            SendMessageW(
                self.handle(),
                WM_SETTEXT,
                Some(WPARAM(0)),
                Some(LPARAM(w!("").as_ptr() as isize)),
            )
        };
    }

    pub fn text(&self) -> String {
        // SAFETY: our own window.
        let len = unsafe { SendMessageW(self.handle(), WM_GETTEXTLENGTH, None, None) }.0;
        let mut buf = vec![0u16; usize::try_from(len).unwrap_or(0) + 1];
        // SAFETY: `buf` holds `len + 1` wide characters, which is what
        // WM_GETTEXT is told it may write.
        let copied = unsafe {
            SendMessageW(
                self.handle(),
                WM_GETTEXT,
                Some(WPARAM(buf.len())),
                Some(LPARAM(buf.as_mut_ptr() as isize)),
            )
        }
        .0;
        buf.truncate(usize::try_from(copied).unwrap_or(0));
        String::from_utf16_lossy(&buf)
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        // SAFETY: a thread id this process created; WM_QUIT ends its loop.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
