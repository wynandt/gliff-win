//! The failure panel's window: a child over the picture area that draws
//! [`crate::failure_panel`] and reports what was clicked. All the layout and
//! look is in the pure module; this only owns the window, a text measure and
//! the pointer.

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::BeginPaint;
use windows::Win32::Graphics::Gdi::BitBlt;
use windows::Win32::Graphics::Gdi::EndPaint;
use windows::Win32::Graphics::Gdi::GetDC;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::Graphics::Gdi::PAINTSTRUCT;
use windows::Win32::Graphics::Gdi::ReleaseDC;
use windows::Win32::Graphics::Gdi::SRCCOPY;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT;
use windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::Win32::UI::WindowsAndMessaging::HWND_TOP;
use windows::Win32::UI::WindowsAndMessaging::IDC_ARROW;
use windows::Win32::UI::WindowsAndMessaging::IDC_HAND;
use windows::Win32::UI::WindowsAndMessaging::LoadCursorW;
use windows::Win32::UI::WindowsAndMessaging::RegisterClassExW;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW;
use windows::Win32::UI::WindowsAndMessaging::SetCursor;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::ShowWindow;
use windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_PAINT;
use windows::Win32::UI::WindowsAndMessaging::WM_SETCURSOR;
use windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW;
use windows::Win32::UI::WindowsAndMessaging::WS_CHILD;
use windows::Win32::UI::WindowsAndMessaging::WS_CLIPSIBLINGS;
use windows::core::PCWSTR;
use windows::core::w;

use crate::failure_panel::Action;
use crate::failure_panel::Content;
use crate::failure_panel::Layout;
use crate::failure_panel::Metrics;
use crate::failure_panel::layout;
use crate::failure_panel::ops;
use crate::palette::Palette;
use crate::strip_gdi::DcMeasure;
use crate::strip_gdi::Fonts;
use crate::strip_gdi::Surface;
use crate::window::with_app;

const CLASS: PCWSTR = w!("GwinFailurePanel");
const WM_MOUSELEAVE: u32 = 0x02a3;

/// The panel's window and what it draws from.
pub struct PanelWin {
    hwnd: HWND,
    fonts: Fonts,
    palette: Palette,
    layout: Option<Layout>,
    hover: Option<Action>,
    tracking: bool,
    visible: bool,
    size: (i32, i32),
}

/// Creates the panel under `parent`, hidden.
pub fn create(parent: HWND) -> windows::core::Result<PanelWin> {
    // SAFETY: plain Win32 setup with static strings; the window procedure is
    // ours and the parent is ours.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            Default::default(),
            CLASS,
            w!(""),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )?;
        Ok(PanelWin {
            hwnd,
            fonts: Fonts::new(),
            palette: Palette::DARK,
            layout: None,
            hover: None,
            tracking: false,
            visible: false,
            size: (0, 0),
        })
    }
}

impl PanelWin {
    /// Shows `content` over the rectangle (`x`, `y`, `width`, `height`) of the
    /// parent, or hides the panel when there is none.
    pub fn show(&mut self, content: Option<&Content>, at: (i32, i32, i32, i32), dpi: u32) {
        let Some(content) = content else {
            self.layout = None;
            self.hover = None;
            if self.visible {
                // SAFETY: our own window.
                let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
                self.visible = false;
            }
            return;
        };
        let (x, y, width, height) = at;
        // SAFETY: a DC of our window, released before return.
        let placed = unsafe {
            let hdc = GetDC(Some(self.hwnd));
            let measure = DcMeasure {
                hdc,
                fonts: &self.fonts,
            };
            let placed = layout(&Metrics::at(dpi), content, width, height, &measure);
            ReleaseDC(Some(self.hwnd), hdc);
            placed
        };
        if self.layout.as_ref() != Some(&placed) || self.size != (width, height) {
            self.layout = Some(placed);
            self.size = (width, height);
            self.hover = None;
        }
        // The first show comes to the top; later moves keep the order, so the
        // strip overlay of full screen stays above the panel.
        let flags = if self.visible {
            SWP_NOZORDER | SWP_NOACTIVATE
        } else {
            SWP_SHOWWINDOW | SWP_NOACTIVATE
        };
        // SAFETY: our own window.
        let _ = unsafe { SetWindowPos(self.hwnd, Some(HWND_TOP), x, y, width, height, flags) };
        self.visible = true;
        self.invalidate();
    }

    fn invalidate(&self) {
        // SAFETY: our own window; None invalidates all of it.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Paints the draw list on an off-screen bitmap, then one copy.
    fn paint(&self) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: our window, paired with EndPaint; the surface is dropped
        // before return.
        unsafe {
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if let Some(l) = &self.layout {
                let (w, h) = (l.width.max(1), l.height.max(1));
                let list = ops(l, &self.palette, self.hover);
                if let Some(mut surface) = Surface::new(Some(hdc), w, h, self.palette.backdrop) {
                    surface.draw(&self.fonts, &list);
                    let _ = BitBlt(hdc, 0, 0, w, h, Some(surface.dc()), 0, 0, SRCCOPY);
                }
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// The pointer moved to (`x`, `y`); asks for a leave message once.
    fn moved(&mut self, x: i32, y: i32) {
        if !self.tracking {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: asks for a WM_MOUSELEAVE on our own window.
            self.tracking = unsafe { TrackMouseEvent(&mut track) }.is_ok();
        }
        self.set_hover(self.layout.as_ref().and_then(|l| l.hit(x, y)));
    }

    fn left(&mut self) {
        self.tracking = false;
        self.set_hover(None);
    }

    fn set_hover(&mut self, hover: Option<Action>) {
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
    }

    /// What a click at (`x`, `y`) is on, with the layout it is on.
    pub fn click_at(&self, x: i32, y: i32) -> Option<(Action, Layout)> {
        let l = self.layout.as_ref()?;
        Some((l.hit(x, y)?, l.clone()))
    }
}

fn signed(lparam: LPARAM) -> (i32, i32) {
    (
        i32::from(lparam.0 as u16 as i16),
        i32::from((lparam.0 >> 16) as u16 as i16),
    )
}

/// The panel's window procedure.
extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let (x, y) = signed(lparam);
    match msg {
        WM_PAINT => {
            if with_app(|app| app.panel.paint()).is_none() {
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: our window, paired with EndPaint.
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let _ = with_app(|app| app.panel.moved(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let _ = with_app(|app| app.panel.left());
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = with_app(|app| app.panel_click(x, y));
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let hand = with_app(|app| app.panel.hover.is_some()).unwrap_or(false);
            // SAFETY: a stock cursor.
            unsafe {
                if let Ok(cursor) = LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }) {
                    SetCursor(Some(cursor));
                }
            }
            LRESULT(1)
        }
        // SAFETY: the default handling of a message of ours.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Opens a file, a folder, a web address or a Settings page the way the
/// person's Windows does, through Windows' own `explorer.exe`, by full path.
pub fn open(what: &str) {
    if let Err(e) = std::process::Command::new(crate::system_paths::explorer())
        .arg(what)
        .spawn()
    {
        crate::logging::line(&format!("could not open {what}: {e}"));
    }
}
