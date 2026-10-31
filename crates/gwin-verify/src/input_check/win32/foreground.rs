//! Making a window of ours the foreground window when Windows is reluctant.
//!
//! `SetForegroundWindow` is refused unless the caller is the foreground
//! process or just received input. After the Win+L lock screen closes, the
//! shell holds the foreground for a moment and a plain call fails, so this
//! tries again with the input queues joined, which Windows allows, and last
//! with `SwitchToThisWindow`. It sends no key (no Alt tap), so what the remote machine
//! logs is not changed.

use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::AttachThreadInput;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::BringWindowToTop;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::SwitchToThisWindow;

/// How many times to try, a short wait after each.
const ATTEMPTS: u32 = 6;
const WAIT: Duration = Duration::from_millis(150);

fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: no arguments.
    let front = unsafe { GetForegroundWindow() };
    front == hwnd
}

/// Tries to make `hwnd` the foreground window; whether it is by the end.
pub fn force(hwnd: HWND) -> bool {
    for attempt in 0..ATTEMPTS {
        if is_foreground(hwnd) {
            return true;
        }
        match attempt {
            0 => {
                // SAFETY: a live window; Windows may refuse, which is
                // checked on the next pass.
                let _ = unsafe { SetForegroundWindow(hwnd) };
            }
            1..=3 => with_input_joined(|| {
                // SAFETY: a live window.
                let _ = unsafe { BringWindowToTop(hwnd) };
                // SAFETY: as above.
                let _ = unsafe { SetForegroundWindow(hwnd) };
            }),
            _ => {
                // SAFETY: a live window.
                unsafe { SwitchToThisWindow(hwnd, true) };
            }
        }
        std::thread::sleep(WAIT);
    }
    is_foreground(hwnd)
}

/// Runs `act` with this thread's input queue joined to the foreground
/// window's, so Windows treats it as the one that has the input.
fn with_input_joined(act: impl FnOnce()) {
    // SAFETY: no arguments.
    let front = unsafe { GetForegroundWindow() };
    // SAFETY: a window handle (possibly none); the process id is not wanted.
    let front_thread = unsafe { GetWindowThreadProcessId(front, None) };
    // SAFETY: no arguments.
    let mine = unsafe { GetCurrentThreadId() };
    let joined = front_thread != 0
        && front_thread != mine
        // SAFETY: two thread ids; undone below.
        && unsafe { AttachThreadInput(mine, front_thread, true) }.as_bool();
    act();
    if joined {
        // SAFETY: undoes the join made above.
        let _ = unsafe { AttachThreadInput(mine, front_thread, false) };
    }
}
