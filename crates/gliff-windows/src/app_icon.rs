//! Loads the application icon compiled into the executable.
//!
//! `build.rs` embeds `assets/gliff.ico` as resource ID 1. The window class
//! and `WM_SETICON` both use sizes from `GetSystemMetricsForDpi`, so the
//! title bar and the taskbar pick 24 or 40 px at 150% or 160% scale.

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::Win32::UI::WindowsAndMessaging::ICON_BIG;
use windows::Win32::UI::WindowsAndMessaging::ICON_SMALL;
use windows::Win32::UI::WindowsAndMessaging::IMAGE_ICON;
use windows::Win32::UI::WindowsAndMessaging::LR_SHARED;
use windows::Win32::UI::WindowsAndMessaging::LoadImageW;
use windows::Win32::UI::WindowsAndMessaging::SM_CXICON;
use windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON;
use windows::Win32::UI::WindowsAndMessaging::SM_CYICON;
use windows::Win32::UI::WindowsAndMessaging::SM_CYSMICON;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::UI::WindowsAndMessaging::WM_SETICON;
use windows::core::PCWSTR;

/// Resource ID written by `build.rs` into the generated `.rc`.
const IDI_GLIFF: u16 = 1;

fn resource_id(id: u16) -> PCWSTR {
    PCWSTR(id as usize as *const u16)
}

/// The large and small icons for this DPI, or null handles if the resource
/// cannot be loaded (the window then keeps the default Windows icon).
pub fn load(dpi: u32) -> (HICON, HICON) {
    // SAFETY: GetSystemMetricsForDpi only reads the system metric table for
    // the given DPI; it has no handle or lifetime requirements.
    let (big_cx, big_cy, small_cx, small_cy) = unsafe {
        (
            GetSystemMetricsForDpi(SM_CXICON, dpi),
            GetSystemMetricsForDpi(SM_CYICON, dpi),
            GetSystemMetricsForDpi(SM_CXSMICON, dpi),
            GetSystemMetricsForDpi(SM_CYSMICON, dpi),
        )
    };
    (
        load_size(big_cx, big_cy).unwrap_or_default(),
        load_size(small_cx, small_cy).unwrap_or_default(),
    )
}

fn load_size(cx: i32, cy: i32) -> Option<HICON> {
    if cx <= 0 || cy <= 0 {
        return None;
    }
    // SAFETY: GetModuleHandleW(None) returns this process's exe module,
    // which is where the icon resource is compiled.
    let instance = unsafe { GetModuleHandleW(None) }.ok()?;
    // SAFETY: the icon is compiled into this executable as resource ID 1;
    // the instance is this module; LR_SHARED means the system owns the
    // handle for the life of the process (do not DestroyIcon).
    let handle = unsafe {
        LoadImageW(
            Some(instance.into()),
            resource_id(IDI_GLIFF),
            IMAGE_ICON,
            cx,
            cy,
            LR_SHARED,
        )
    }
    .ok()?;
    Some(HICON(handle.0))
}

/// Point the window's large and small icons at `icons`, so the title bar
/// and the taskbar follow a DPI change.
pub fn apply(hwnd: HWND, icons: (HICON, HICON)) {
    let (big, small) = icons;
    // SAFETY: hwnd is a live window of ours; the handles are LR_SHARED
    // icons from this module, or null, which WM_SETICON accepts.
    unsafe {
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(big.0 as isize)),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small.0 as isize)),
        );
    }
}
