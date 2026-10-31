//! `RemoteCursor`: the pointer over the video, as upstream's
//! `set_remote_cursor` decides it ([`CursorImage::pointer`]), made a
//! Win32 cursor with `CreateIconIndirect` and set on `WM_SETCURSOR`.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::BI_RGB;
use windows::Win32::Graphics::Gdi::BITMAPINFO;
use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
use windows::Win32::Graphics::Gdi::CreateBitmap;
use windows::Win32::Graphics::Gdi::CreateDIBSection;
use windows::Win32::Graphics::Gdi::DIB_RGB_COLORS;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Graphics::Gdi::HGDIOBJ;
use windows::Win32::UI::WindowsAndMessaging::CreateIconIndirect;
use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
use windows::Win32::UI::WindowsAndMessaging::GetCursor;
use windows::Win32::UI::WindowsAndMessaging::HCURSOR;
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::Win32::UI::WindowsAndMessaging::ICONINFO;
use windows::Win32::UI::WindowsAndMessaging::IDC_ARROW;
use windows::Win32::UI::WindowsAndMessaging::LoadCursorW;
use windows::Win32::UI::WindowsAndMessaging::SetCursor;

use crate::cursor_image::CursorImage;
use crate::cursor_image::Desktop;
use crate::cursor_image::LogGate;
use crate::cursor_image::Pointer;

/// The current pointer and the icon behind it.
pub struct RemoteCursor {
    desktop: Desktop,
    pointer: Pointer,
    /// The cursor made from the latest usable shape.
    shape: Option<HICON>,
    /// One log line per changed decision (see [`CursorImage::log_line`]).
    gate: LogGate,
}

impl RemoteCursor {
    /// Before `Connected`: the system arrow.
    pub fn new() -> Self {
        Self {
            desktop: Desktop::Mirror,
            pointer: Pointer::Default,
            shape: None,
            gate: LogGate::default(),
        }
    }

    /// `UiEvent::Connected`: the system arrow until a usable shape arrives.
    pub fn connected(&mut self, desktop: Desktop) {
        self.desktop = desktop;
        self.gate.reset();
        self.pointer = Pointer::Default;
    }

    /// `UiEvent::Cursor`. A shape that cannot be made falls back as an
    /// unusable one does.
    pub fn shape(&mut self, image: &CursorImage) {
        self.pointer = image.pointer();
        let line = image.log_line(self.desktop);
        if self.gate.should_log(&line) {
            crate::logging::line(&line);
        }
        if self.pointer != Pointer::Shape {
            return;
        }
        match make_cursor(image) {
            Ok(icon) => {
                // Swap first: the old icon may be the cursor on screen, and
                // destroying the active cursor is undefined.
                // SAFETY: GetCursor only reads the thread's cursor.
                let active = unsafe { GetCursor() };
                let old = self.shape.replace(icon);
                if let Some(old) = old
                    && active.0 == old.0
                {
                    // SAFETY: the icon was just made and is owned here.
                    unsafe { SetCursor(Some(HCURSOR(icon.0))) };
                }
                if let Some(old) = old {
                    // SAFETY: made by CreateIconIndirect, owned here alone,
                    // and no longer the active cursor.
                    let _ = unsafe { DestroyIcon(old) };
                }
            }
            Err(e) => {
                crate::logging::line(&format!(
                    "cursor {}: CreateIconIndirect failed: {e}",
                    image.id
                ));
                self.pointer = Pointer::Default;
            }
        }
    }

    /// `WM_SETCURSOR` over the client area: set the pointer; true when
    /// handled.
    pub fn on_set_cursor(&self, _hwnd: HWND) -> bool {
        let cursor = match (self.pointer, self.shape) {
            (Pointer::Shape, Some(icon)) => Some(HCURSOR(icon.0)),
            // SAFETY: IDC_ARROW is a system cursor; no module handle.
            _ => unsafe { LoadCursorW(None, IDC_ARROW) }.ok(),
        };
        // SAFETY: a cursor we own, a system one, or none.
        unsafe { SetCursor(cursor) };
        true
    }

    fn drop_shape(&mut self) {
        if let Some(icon) = self.shape.take() {
            // SAFETY: made by CreateIconIndirect and owned here alone. A
            // cursor still set is replaced on the next WM_SETCURSOR.
            let _ = unsafe { DestroyIcon(icon) };
        }
    }
}

impl Default for RemoteCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RemoteCursor {
    fn drop(&mut self) {
        self.drop_shape();
    }
}

/// A 32-bit alpha cursor: a top-down BGRA DIB plus an all-zero mask.
fn make_cursor(image: &CursorImage) -> windows::core::Result<HICON> {
    let (w, h) = (image.width, image.height);
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        // Negative: rows top down, as the payload.
        biHeight: -(h as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: `info` describes a w x h 32-bit DIB; `bits` receives its
    // buffer.
    let color = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)? };
    let len = (w * h * 4) as usize;
    // SAFETY: the DIB holds exactly w*h*4 bytes at `bits`, and `usable()`
    // checked the payload holds at least as many.
    unsafe {
        std::ptr::copy_nonoverlapping(image.argb.as_ptr(), bits.cast::<u8>(), len);
    }
    // An all-zero AND mask: with no bits CreateBitmap's contents are
    // undefined. Rows are padded to 16 bits.
    let mask_bits = vec![0u8; (w as usize).div_ceil(16) * 2 * h as usize];
    // SAFETY: a monochrome w x h bitmap; `mask_bits` holds every padded
    // row and outlives the call (CreateBitmap copies it).
    let mask = unsafe { CreateBitmap(w as i32, h as i32, 1, 1, Some(mask_bits.as_ptr().cast())) };
    let hot = |v: i32, side: u32| v.clamp(0, side as i32 - 1) as u32;
    let icon_info = ICONINFO {
        fIcon: false.into(),
        xHotspot: hot(image.hot.0, w),
        yHotspot: hot(image.hot.1, h),
        hbmMask: mask,
        hbmColor: color,
    };
    // SAFETY: both bitmaps are live; CreateIconIndirect copies them, so
    // they are deleted afterwards either way.
    let icon = unsafe { CreateIconIndirect(&icon_info) };
    // SAFETY: bitmaps created above and not selected into any DC.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
    }
    icon
}
