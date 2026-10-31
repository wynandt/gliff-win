//! Borderless fullscreen on the window's monitor: F11 and
//! the window menu's "Fullscreen" item. No exclusive mode; a flip-model
//! swap chain the size of the monitor gets DWM independent flip.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows::Win32::Graphics::Gdi::MONITORINFO;
use windows::Win32::Graphics::Gdi::MonitorFromWindow;
use windows::Win32::UI::WindowsAndMessaging::GWL_STYLE;
use windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowPlacement;
use windows::Win32::UI::WindowsAndMessaging::HWND_TOP;
use windows::Win32::UI::WindowsAndMessaging::SWP_FRAMECHANGED;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOOWNERZORDER;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPlacement;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT;
use windows::Win32::UI::WindowsAndMessaging::WS_OVERLAPPEDWINDOW;
use windows::Win32::UI::WindowsAndMessaging::WS_POPUP;

use crate::stages::Screen;

/// The windowed style and placement while fullscreen.
#[derive(Default)]
pub struct Fullscreen {
    saved: Option<(isize, WINDOWPLACEMENT)>,
}

impl Fullscreen {
    pub fn screen(&self) -> Screen {
        match self.saved {
            Some(_) => Screen::Fullscreen,
            None => Screen::Windowed,
        }
    }

    /// Enter or leave. `SetWindowPos` sends `WM_SIZE` before this returns,
    /// so the caller must not hold the window state borrowed.
    pub fn toggle(&mut self, hwnd: HWND) -> windows::core::Result<Screen> {
        match self.saved.take() {
            None => {
                // SAFETY: `hwnd` is our live window; the out structs are
                // live locals with their size fields set.
                unsafe {
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    let mut placement = WINDOWPLACEMENT {
                        length: size_of::<WINDOWPLACEMENT>() as u32,
                        ..Default::default()
                    };
                    GetWindowPlacement(hwnd, &mut placement)?;
                    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut info = MONITORINFO {
                        cbSize: size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                        return Err(windows::core::Error::from_thread());
                    }
                    let popup = (style & !(WS_OVERLAPPEDWINDOW.0 as isize)) | WS_POPUP.0 as isize;
                    self.saved = Some((style, placement));
                    SetWindowLongPtrW(hwnd, GWL_STYLE, popup);
                    let r = info.rcMonitor;
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                    )?;
                }
                Ok(Screen::Fullscreen)
            }
            Some((style, placement)) => {
                // SAFETY: as above; the saved style and placement are this
                // window's own.
                unsafe {
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style);
                    SetWindowPlacement(hwnd, &placement)?;
                    SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE
                            | SWP_NOSIZE
                            | SWP_NOZORDER
                            | SWP_NOOWNERZORDER
                            | SWP_FRAMECHANGED,
                    )?;
                }
                Ok(Screen::Windowed)
            }
        }
    }
}
