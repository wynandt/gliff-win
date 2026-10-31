//! The Windows half of [`super`]: the named mutex, the mark on the running
//! client's window, the `WM_COPYDATA` hand-over and its receiving end. Never
//! run on the build machine (it is compile-checked for the Windows target).

use std::time::Duration;
use std::time::Instant;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::ERROR_ALREADY_EXISTS;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
use windows::Win32::UI::WindowsAndMessaging::GetPropW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG;
use windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW;
use windows::Win32::UI::WindowsAndMessaging::SetPropW;
use windows::Win32::UI::WindowsAndMessaging::WM_COPYDATA;
use windows::core::BOOL;
use windows::core::HSTRING;

use super::COPY_DATA_TAG;
use super::DecodeError;
use super::Forward;
use super::MAX_MESSAGE;
use super::decode;
use super::encode;

/// How long a second launch waits for the first one's window to exist (the
/// first has taken the mutex and is still starting up).
const WINDOW_WAIT: Duration = Duration::from_secs(5);
/// How long a hand-over may take before the first client counts as hung.
const SEND_TIMEOUT_MS: u32 = 3000;

/// Holds the mutex while the client runs.
#[derive(Debug)]
pub struct Instance {
    mutex: HANDLE,
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: `mutex` is the handle `CreateMutexW` returned and is closed
        // once, here.
        let _ = unsafe { CloseHandle(self.mutex) };
    }
}

/// The mutex: `Ok(Some)` when this is the first client, `Ok(None)` when one
/// already runs under this name.
pub fn acquire(name: &str) -> windows::core::Result<Option<Instance>> {
    let name = HSTRING::from(name);
    // SAFETY: `name` outlives the call; no security attributes, not owned.
    let mutex = unsafe { CreateMutexW(None, false, &name) }?;
    // SAFETY: reads the calling thread's last error, set by the call above.
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if exists {
        // SAFETY: the handle is ours and is not used again.
        let _ = unsafe { CloseHandle(mutex) };
        Ok(None)
    } else {
        Ok(Some(Instance { mutex }))
    }
}

/// Marks the client's main window with the instance name, so a second launch
/// finds this one and not another client that has its own folder.
pub fn mark_window(hwnd: HWND, name: &str) -> windows::core::Result<()> {
    let name = HSTRING::from(name);
    // SAFETY: `hwnd` is a live window of this thread and `name` outlives the
    // call; the property holds a flag, not a pointer.
    unsafe { SetPropW(hwnd, &name, Some(HANDLE(std::ptr::dangling_mut()))) }
}

/// Hands `forward` to the running client, waiting for its window if it is
/// still starting. Returns what went wrong, for the log.
pub fn hand_over(name: &str, forward: &Forward) -> Result<(), String> {
    let deadline = Instant::now() + WINDOW_WAIT;
    let hwnd = loop {
        if let Some(hwnd) = find_window(name) {
            break hwnd;
        }
        if Instant::now() >= deadline {
            return Err("the running client has no window yet".to_owned());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut pid = 0u32;
    // SAFETY: `hwnd` is a window handle and `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    // SAFETY: only grants `pid` the right to take the foreground.
    let _ = unsafe { AllowSetForegroundWindow(pid) };
    let mut bytes = encode(forward);
    let data = COPYDATASTRUCT {
        dwData: COPY_DATA_TAG,
        cbData: u32::try_from(bytes.len()).map_err(|_| "the request is too long".to_owned())?,
        lpData: bytes.as_mut_ptr().cast(),
    };
    let mut result = 0usize;
    // SAFETY: `data` and the bytes it points to live until the call returns,
    // and `SendMessageTimeoutW` is synchronous.
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(std::ptr::from_ref(&data) as isize),
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    if sent.0 == 0 {
        Err("the running client did not answer".to_owned())
    } else {
        Ok(())
    }
}

/// The window marked with `name`, if any.
fn find_window(name: &str) -> Option<HWND> {
    struct Search {
        name: HSTRING,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Search` that `find_window` passed, alive
        // for the whole enumeration.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut class = [0u16; 64];
        // SAFETY: a valid window handle and a buffer of the length given.
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        let is_ours = usize::try_from(len).is_ok_and(|len| {
            String::from_utf16_lossy(&class[..len.min(class.len())])
                == crate::window::CLASS_NAME_TEXT
        });
        // SAFETY: reads a window property by name.
        if is_ours && !unsafe { GetPropW(hwnd, &search.name) }.is_invalid() {
            search.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut search = Search {
        name: HSTRING::from(name),
        found: None,
    };
    // SAFETY: the callback only touches `search`, which outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(visit),
            LPARAM(std::ptr::from_mut(&mut search) as isize),
        )
    };
    search.found
}

/// What came in a `WM_COPYDATA` to the main window.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    Request(Forward),
    /// Not ours (another tag): leave the message to the default handler.
    Foreign,
    /// Ours, but not valid.
    Refused(DecodeError),
}

/// Reads the `COPYDATASTRUCT` that `lparam` points to.
///
/// # Safety
///
/// `lparam` must be the `lParam` of a `WM_COPYDATA` message being handled: a
/// pointer to a `COPYDATASTRUCT` that is valid for the call.
pub unsafe fn receive(lparam: LPARAM) -> Received {
    // SAFETY: the caller's contract.
    let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
    if data.dwData != COPY_DATA_TAG {
        return Received::Foreign;
    }
    let len = data.cbData as usize;
    if len > MAX_MESSAGE {
        return Received::Refused(DecodeError::TooLong(len));
    }
    let bytes: &[u8] = if len == 0 || data.lpData.is_null() {
        &[]
    } else {
        // SAFETY: the sender's `cbData` bytes at `lpData` are readable for
        // the duration of the message.
        unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), len) }
    };
    match decode(bytes) {
        Ok(forward) => Received::Request(forward),
        Err(e) => Received::Refused(e),
    }
}
