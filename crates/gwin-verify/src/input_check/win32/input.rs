//! `SendInput` and the cursor.

use std::time::Duration;

use anyhow::Context;
use anyhow::bail;
use gliff_windows::keymap::Press;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_MOUSE;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_SCANCODE;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_HWHEEL;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_MIDDLEDOWN;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_MIDDLEUP;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_MOVE;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTDOWN;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTUP;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_WHEEL;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_XDOWN;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_XUP;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEINPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

use crate::input_check::cases::Side;
use crate::input_check::cases::Wheel;
use crate::input_check::scan::Scan;

/// Spacing between injected events, so the hook and the remote machine see them in
/// order.
const GAP: Duration = Duration::from_millis(15);

fn send(inputs: &[INPUT]) -> anyhow::Result<()> {
    // SAFETY: `inputs` is a valid slice of INPUT and the size is that of one
    // INPUT, as SendInput requires.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        bail!(
            "SendInput injected {sent} of {} events (blocked by another desktop or UIPI?)",
            inputs.len()
        );
    }
    Ok(())
}

fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Scancodes, in order, one event at a time.
pub fn keys(sequence: &[(Scan, Press)]) -> anyhow::Result<()> {
    let sent = keys_in_order(sequence);
    if sent.is_err() {
        release_modifiers("a failed key sequence");
    }
    sent
}

fn keys_in_order(sequence: &[(Scan, Press)]) -> anyhow::Result<()> {
    for (scan, press) in sequence {
        let mut flags = KEYEVENTF_SCANCODE;
        if scan.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if *press == Press::Up {
            flags |= KEYEVENTF_KEYUP;
        }
        send(&[keyboard(0, u16::from(scan.code), flags)])?;
        std::thread::sleep(GAP);
    }
    Ok(())
}

/// A virtual-key tap (no scancode), for F23 and F24.
pub fn vk_tap(vk: u16) -> anyhow::Result<()> {
    send(&[keyboard(vk, 0, KEYBD_EVENT_FLAGS(0))])?;
    std::thread::sleep(GAP);
    send(&[keyboard(vk, 0, KEYEVENTF_KEYUP)])
}

fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub fn left_click() -> anyhow::Result<()> {
    send(&[mouse(MOUSEEVENTF_LEFTDOWN, 0)])?;
    std::thread::sleep(GAP);
    send(&[mouse(MOUSEEVENTF_LEFTUP, 0)])
}

/// Down then up of a button other than the left.
pub fn button(side: Side) -> anyhow::Result<()> {
    let (down, up, data) = match side {
        Side::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
        Side::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
        // XBUTTON1 and XBUTTON2.
        Side::Back => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 1),
        Side::Forward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 2),
    };
    send(&[mouse(down, data)])?;
    std::thread::sleep(GAP);
    send(&[mouse(up, data)])
}

pub fn wheel(wheel: Wheel, delta: i16) -> anyhow::Result<()> {
    let flags = match wheel {
        Wheel::Vertical => MOUSEEVENTF_WHEEL,
        Wheel::Horizontal => MOUSEEVENTF_HWHEEL,
    };
    send(&[mouse(flags, i32::from(delta))])?;
    std::thread::sleep(GAP);
    Ok(())
}

/// Moves the cursor to a screen point (physical pixels; this process is
/// per-monitor DPI aware).
pub fn cursor_to(x: i32, y: i32) -> anyhow::Result<()> {
    // SAFETY: plain integer coordinates.
    unsafe { SetCursorPos(x, y) }.context("SetCursorPos")?;
    std::thread::sleep(GAP);
    Ok(())
}

/// A relative mouse move of one pixel out and back, as `SendInput`. Unlike
/// `SetCursorPos` it goes through the input system as a real mouse event, so
/// Windows works out which window the pointer is over, and sends
/// `WM_MOUSELEAVE` and `WM_MOUSEMOVE`, from it.
pub fn nudge() -> anyhow::Result<()> {
    for dx in [1, -1] {
        let mut event = mouse(MOUSEEVENTF_MOVE, 0);
        event.Anonymous.mi.dx = dx;
        send(&[event])?;
        std::thread::sleep(GAP);
    }
    Ok(())
}

/// Lets go of every modifier Windows still counts as down (the same sweep the
/// client makes when a grab ends), and says so when it had to. A step that
/// ends early, a key a person held a moment too long, or a lock screen that
/// took an Up must not leave Ctrl, Alt, Shift or Win held for what follows.
pub fn release_modifiers(after: &str) {
    let released = gliff_windows::modifiers::release_local();
    if released > 0 {
        eprintln!("   ({released} modifier key(s) were still down after {after}: released)");
    }
}
