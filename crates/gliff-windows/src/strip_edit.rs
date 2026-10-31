//! The address bar's edit field: a plain Win32 EDIT child of the strip,
//! subclassed to take Enter and Escape, to show the hint in an empty field
//! and to tell the window when it gains or loses the keyboard.
//!
//! There is no comctl32 version 6 in this program (no manifest), so the field
//! has no cue banner and no `SetWindowSubclass`; the window procedure is
//! replaced by hand and the old one kept in [`ORIGINAL`].
//!
//! Nothing here holds the window state: every message that matters goes to
//! [`crate::window::with_app`], which does nothing when the state is busy.
//! A focus change is therefore never made from inside the state; the strip
//! queues it and posts itself a message ([`crate::strip_win::WM_APP_DEFER`]).

use std::sync::atomic::AtomicIsize;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::DT_LEFT;
use windows::Win32::Graphics::Gdi::DT_NOPREFIX;
use windows::Win32::Graphics::Gdi::DT_SINGLELINE;
use windows::Win32::Graphics::Gdi::DrawTextW;
use windows::Win32::Graphics::Gdi::GetDC;
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::Graphics::Gdi::ReleaseDC;
use windows::Win32::Graphics::Gdi::SelectObject;
use windows::Win32::Graphics::Gdi::SetBkMode;
use windows::Win32::Graphics::Gdi::SetTextColor;
use windows::Win32::Graphics::Gdi::TRANSPARENT;
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
use windows::Win32::UI::WindowsAndMessaging::AppendMenuW;
use windows::Win32::UI::WindowsAndMessaging::CallWindowProcW;
use windows::Win32::UI::WindowsAndMessaging::CreatePopupMenu;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DestroyMenu;
use windows::Win32::UI::WindowsAndMessaging::ES_AUTOHSCROLL;
use windows::Win32::UI::WindowsAndMessaging::ES_LEFT;
use windows::Win32::UI::WindowsAndMessaging::GWLP_WNDPROC;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
use windows::Win32::UI::WindowsAndMessaging::MF_GRAYED;
use windows::Win32::UI::WindowsAndMessaging::MF_SEPARATOR;
use windows::Win32::UI::WindowsAndMessaging::MF_STRING;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowTextW;
use windows::Win32::UI::WindowsAndMessaging::TPM_LEFTALIGN;
use windows::Win32::UI::WindowsAndMessaging::TPM_RETURNCMD;
use windows::Win32::UI::WindowsAndMessaging::TPM_RIGHTBUTTON;
use windows::Win32::UI::WindowsAndMessaging::TPM_TOPALIGN;
use windows::Win32::UI::WindowsAndMessaging::TrackPopupMenuEx;
use windows::Win32::UI::WindowsAndMessaging::WM_CHAR;
use windows::Win32::UI::WindowsAndMessaging::WM_CONTEXTMENU;
use windows::Win32::UI::WindowsAndMessaging::WM_GETFONT;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_KILLFOCUS;
use windows::Win32::UI::WindowsAndMessaging::WM_PAINT;
use windows::Win32::UI::WindowsAndMessaging::WM_SETFOCUS;
use windows::Win32::UI::WindowsAndMessaging::WM_SETFONT;
use windows::Win32::UI::WindowsAndMessaging::WNDPROC;
use windows::Win32::UI::WindowsAndMessaging::WS_CHILD;
use windows::core::PCWSTR;
use windows::core::w;

use crate::palette::Palette;
use crate::strip::address::Event;
use crate::strip::draw::PLACEHOLDER;
use crate::strip::edit_menu;
use crate::window::with_app;

/// Edit-control messages (their header, `Win32_UI_Controls`, is not in the
/// feature list; the numbers are fixed).
const EM_SETSEL: u32 = 0x00b1;
const EM_GETRECT: u32 = 0x00b2;
const EM_GETSEL: u32 = 0x00b0;
const EM_LIMITTEXT: u32 = 0x00c5;

/// The clipboard format of Unicode text (`CF_UNICODETEXT`).
const CF_UNICODETEXT: u32 = 13;

/// The most the field takes: longer than any `user@host`.
const LIMIT: usize = 256;

/// The window procedure the field had before it was subclassed.
static ORIGINAL: AtomicIsize = AtomicIsize::new(0);

/// Makes the (hidden) edit field under `strip` and subclasses it.
pub fn create(strip: HWND, font: HFONT) -> windows::core::Result<HWND> {
    // SAFETY: plain Win32 setup under our own window with static strings;
    // the replacement procedure below is ours, and the old one is kept to
    // pass on every message it does not take.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let edit = CreateWindowExW(
            Default::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD
                | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                    ES_LEFT as u32 | ES_AUTOHSCROLL as u32,
                ),
            0,
            0,
            10,
            10,
            Some(strip),
            None,
            Some(instance.into()),
            None,
        )?;
        SendMessageW(
            edit,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        SendMessageW(edit, EM_LIMITTEXT, Some(WPARAM(LIMIT)), Some(LPARAM(0)));
        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, proc as *const () as usize as isize);
        ORIGINAL.store(old, Ordering::Relaxed);
        Ok(edit)
    }
}

/// What is typed in `edit`.
pub fn text_of(edit: HWND) -> String {
    // SAFETY: reads the text of our own child into a buffer sized from its
    // own length.
    unsafe {
        let len = usize::try_from(GetWindowTextLengthW(edit)).unwrap_or(0);
        let mut buffer = vec![0u16; len + 1];
        let got = usize::try_from(GetWindowTextW(edit, &mut buffer)).unwrap_or(0);
        String::from_utf16_lossy(&buffer[..got.min(len)])
    }
}

/// Puts `text` in `edit` with the caret at its end.
pub fn set_text(edit: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: our own child; the string outlives the call.
    unsafe {
        let _ = SetWindowTextW(edit, PCWSTR(wide.as_ptr()));
        let end = wide.len() - 1;
        SendMessageW(
            edit,
            EM_SETSEL,
            Some(WPARAM(end)),
            Some(LPARAM(end as isize)),
        );
    }
}

/// The colours of the edit field, for `WM_CTLCOLOREDIT`.
pub fn colours(pal: &Palette) -> (u32, u32) {
    (pal.text.colorref(), pal.field.colorref())
}

fn pass_on(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let old = ORIGINAL.load(Ordering::Relaxed);
    // SAFETY: `old` is the procedure `SetWindowLongPtrW` returned when the
    // field was subclassed, a valid WNDPROC (or 0, which becomes None).
    unsafe {
        let old: WNDPROC = std::mem::transmute::<isize, WNDPROC>(old);
        CallWindowProcW(old, hwnd, msg, wparam, lparam)
    }
}

/// The hint in an empty field, drawn over what the field painted.
fn paint_hint(hwnd: HWND) {
    // SAFETY: a DC of our own child, released below; the font selected is
    // put back; the rectangle and the string are local.
    unsafe {
        if GetWindowTextLengthW(hwnd) != 0 {
            return;
        }
        let hdc = GetDC(Some(hwnd));
        let mut rect = RECT::default();
        SendMessageW(
            hwnd,
            EM_GETRECT,
            Some(WPARAM(0)),
            Some(LPARAM(std::ptr::addr_of_mut!(rect) as isize)),
        );
        let font = SendMessageW(hwnd, WM_GETFONT, None, None).0;
        let previous = SelectObject(hdc, HFONT(font as *mut _).into());
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(
            hdc,
            windows::Win32::Foundation::COLORREF(Palette::DARK.placeholder.colorref()),
        );
        let mut hint: Vec<u16> = PLACEHOLDER.encode_utf16().collect();
        DrawTextW(
            hdc,
            &mut hint,
            &mut rect,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
        );
        SelectObject(hdc, previous);
        ReleaseDC(Some(hwnd), hdc);
    }
}

/// Our own right-click menu: Cut, Copy, Paste and Select All, in place of the
/// stock one (reading order, Unicode control characters, input-method items).
/// `lparam` is the screen position, or -1 when the menu key asked for it.
fn context_menu(hwnd: HWND, lparam: LPARAM) {
    // The two words are signed 16-bit screen coordinates.
    let x = i32::from(lparam.0 as u16 as i16);
    let y = i32::from((lparam.0 >> 16) as u16 as i16);
    // SAFETY: our own child and a menu made and destroyed here; the strings
    // outlive each call; `start` and `end` outlive the `EM_GETSEL` call.
    unsafe {
        let _ = SetFocus(Some(hwnd));
        let (mut start, mut end) = (0u32, 0u32);
        SendMessageW(
            hwnd,
            EM_GETSEL,
            Some(WPARAM(std::ptr::addr_of_mut!(start) as usize)),
            Some(LPARAM(std::ptr::addr_of_mut!(end) as isize)),
        );
        let facts = edit_menu::Facts {
            has_selection: start != end,
            has_text: GetWindowTextLengthW(hwnd) != 0,
            can_paste: IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok(),
        };
        let (x, y) = if x == -1 && y == -1 {
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            (rect.left, rect.bottom)
        } else {
            (x, y)
        };
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let rows = edit_menu::rows(facts);
        for (i, row) in rows.iter().enumerate() {
            if row.command == edit_menu::EditCommand::SelectAll {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            }
            let wide: Vec<u16> = row.label.encode_utf16().chain(Some(0)).collect();
            let flags = if row.enabled {
                MF_STRING
            } else {
                MF_STRING | MF_GRAYED
            };
            let _ = AppendMenuW(menu, flags, i + 1, PCWSTR(wide.as_ptr()));
        }
        let chosen = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        let picked = usize::try_from(chosen.0)
            .ok()
            .and_then(|id| id.checked_sub(1))
            .and_then(|i| rows.get(i));
        if let Some(row) = picked {
            let m = edit_menu::message(row.command);
            SendMessageW(hwnd, m.msg, Some(WPARAM(m.wparam)), Some(LPARAM(m.lparam)));
        }
    }
}

/// The edit field's window procedure.
extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    const CTRL_A: usize = 0x01;
    crate::window::forward_menu_loop(hwnd, msg, wparam, lparam);
    match msg {
        WM_KEYDOWN if wparam.0 == usize::from(VK_RETURN.0) => {
            let _ = with_app(|app| app.address_event(Event::Enter));
            return LRESULT(0);
        }
        WM_KEYDOWN if wparam.0 == usize::from(VK_ESCAPE.0) => {
            let _ = with_app(|app| app.address_event(Event::Escape));
            return LRESULT(0);
        }
        // Enter and Escape would beep as characters; Ctrl+A is not
        // select-all in a plain field.
        WM_CHAR if wparam.0 == 0x0d || wparam.0 == 0x1b => return LRESULT(0),
        WM_CHAR if wparam.0 == CTRL_A => {
            // SAFETY: our own child.
            unsafe {
                SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
            return LRESULT(0);
        }
        WM_CONTEXTMENU => {
            context_menu(hwnd, lparam);
            return LRESULT(0);
        }
        WM_SETFOCUS => {
            let result = pass_on(hwnd, msg, wparam, lparam);
            let _ = with_app(|app| app.address_event(Event::Focus(true)));
            return result;
        }
        WM_KILLFOCUS => {
            let result = pass_on(hwnd, msg, wparam, lparam);
            let _ = with_app(|app| app.address_event(Event::Focus(false)));
            return result;
        }
        WM_PAINT => {
            let result = pass_on(hwnd, msg, wparam, lparam);
            paint_hint(hwnd);
            return result;
        }
        _ => {}
    }
    pass_on(hwnd, msg, wparam, lparam)
}
