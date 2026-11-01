//! The Windows side of the lock setting: the real [`Registry`], the process
//! table, the `--lock-watchdog` helper and the exit paths. Never run on the
//! build machine: the tests use `lock_policy`'s in-memory registry.
//!
//! Every path that can end the client calls [`restore_now`], which needs only
//! the owner (process id and creation time), so the panic hook, the console
//! control handler, `WM_ENDSESSION` and the drop guard share one function.

use std::os::windows::process::CommandExt;
use std::process::Child;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::WAIT_ABANDONED;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Console::CTRL_BREAK_EVENT;
use windows::Win32::System::Console::CTRL_C_EVENT;
use windows::Win32::System::Console::CTRL_CLOSE_EVENT;
use windows::Win32::System::Console::CTRL_LOGOFF_EVENT;
use windows::Win32::System::Console::CTRL_SHUTDOWN_EVENT;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Registry::HKEY;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::Registry::KEY_READ;
use windows::Win32::System::Registry::KEY_SET_VALUE;
use windows::Win32::System::Registry::REG_DWORD;
use windows::Win32::System::Registry::REG_OPTION_NON_VOLATILE;
use windows::Win32::System::Registry::REG_QWORD;
use windows::Win32::System::Registry::REG_VALUE_TYPE;
use windows::Win32::System::Registry::RegCloseKey;
use windows::Win32::System::Registry::RegCreateKeyExW;
use windows::Win32::System::Registry::RegDeleteKeyW;
use windows::Win32::System::Registry::RegDeleteValueW;
use windows::Win32::System::Registry::RegOpenKeyExW;
use windows::Win32::System::Registry::RegQueryValueExW;
use windows::Win32::System::Registry::RegSetValueExW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::Threading::GetProcessTimes;
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::System::Threading::OpenProcess;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::Win32::System::Threading::PROCESS_SYNCHRONIZE;
use windows::Win32::System::Threading::ReleaseMutex;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::BOOL;
use windows::core::HSTRING;
use windows::core::PCWSTR;
use windows::core::w;

use crate::lock_policy;
use crate::lock_policy::MARKER_KEY;
use crate::lock_policy::Marker;
use crate::lock_policy::Owner;
use crate::lock_policy::OwnerExit;
use crate::lock_policy::POLICY_VALUE;
use crate::lock_policy::Prior;
use crate::lock_policy::Processes;
use crate::lock_policy::Recovery;
use crate::lock_policy::RegError;
use crate::lock_policy::Registry;
use crate::lock_policy::StoredMarker;

const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\System";

/// `CREATE_NO_WINDOW` and `CREATE_BREAKAWAY_FROM_JOB`.
const WATCHDOG_FLAGS: u32 = 0x0800_0000 | BREAKAWAY;
/// `CREATE_BREAKAWAY_FROM_JOB`.
const BREAKAWAY: u32 = 0x0100_0000;

/// The mutex every client and every watchdog helper of this user takes
/// around a look at the setting and the writes that follow it.
const REGISTRY_MUTEX: &str = "Local\\gliff-windows-lock-policy";
/// How long to wait for it; past that the work goes ahead without it (a
/// stuck holder must not keep the lock setting from being put back).
const REGISTRY_WAIT_MS: u32 = 2_000;

/// Set once this process is on its way out (or its window thread has
/// panicked, which ends it: see the hook): the setting is not turned on
/// again after that, so the restore that follows is the last word.
static EXITING: AtomicBool = AtomicBool::new(false);

/// The owner of this process, set once by [`init`].
static OWNER: OnceLock<Owner> = OnceLock::new();
/// The thread that called [`init`]: the window's. A panic there ends the
/// process; one on another thread (a session's, which the window reports
/// and outlives) does not, so it leaves the setting to the window.
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

fn reg_error(what: &str, code: WIN32_ERROR) -> RegError {
    RegError(format!("{what}: error {}", code.0))
}

fn ok(what: &str, code: WIN32_ERROR) -> Result<(), RegError> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(reg_error(what, code))
    }
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Key {
    /// Opens a key under HKCU; `None` when it does not exist.
    fn open(
        path: &str,
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<Option<Self>, RegError> {
        let mut key = HKEY::default();
        let path = HSTRING::from(path);
        // SAFETY: `path` outlives the call and `key` is a valid out pointer.
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                access,
                &mut key,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        ok("RegOpenKeyExW", code)?;
        Ok(Some(Self(key)))
    }

    /// Opens a key under HKCU for writing, creating it.
    fn create(path: &str) -> Result<Self, RegError> {
        let mut key = HKEY::default();
        let path = HSTRING::from(path);
        // SAFETY: `path` outlives the call, `key` is a valid out pointer, and
        // the optional arguments are None.
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        };
        ok("RegCreateKeyExW", code)?;
        Ok(Self(key))
    }

    /// A DWORD or QWORD value as a u64; `None` when it is absent.
    fn read_number(&self, name: PCWSTR) -> Result<Option<u64>, RegError> {
        let mut data = [0_u8; 8];
        let mut len = data.len() as u32;
        let mut kind = REG_VALUE_TYPE::default();
        // SAFETY: `data` and `len` describe the same 8 writable bytes, and
        // `name` is a NUL-terminated wide string.
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                name,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut len),
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        ok("RegQueryValueExW", code)?;
        let value = match (kind, len) {
            (REG_DWORD, 4) => u64::from(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
            (REG_QWORD, 8) => u64::from_le_bytes(data),
            _ => {
                return Err(RegError(format!(
                    "a value of type {} and {len} bytes",
                    kind.0
                )));
            }
        };
        Ok(Some(value))
    }

    fn write_dword(&self, name: PCWSTR, value: u32) -> Result<(), RegError> {
        // SAFETY: `name` is a NUL-terminated wide string; the data slice is
        // valid for the call.
        let code =
            unsafe { RegSetValueExW(self.0, name, None, REG_DWORD, Some(&value.to_le_bytes())) };
        ok("RegSetValueExW", code)
    }

    fn write_qword(&self, name: PCWSTR, value: u64) -> Result<(), RegError> {
        // SAFETY: as in `write_dword`.
        let code =
            unsafe { RegSetValueExW(self.0, name, None, REG_QWORD, Some(&value.to_le_bytes())) };
        ok("RegSetValueExW", code)
    }

    /// Deletes a value; a missing one is fine.
    fn delete_value(&self, name: PCWSTR) -> Result<(), RegError> {
        // SAFETY: `name` is a NUL-terminated wide string.
        let code = unsafe { RegDeleteValueW(self.0, name) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        ok("RegDeleteValueW", code)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `open` or `create` and is closed
        // once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// The real registry, under HKCU only.
#[derive(Debug, Default)]
pub struct WinRegistry;

/// The registry mutex, held until dropped. The mutex is owned by the thread
/// that took it, so a thread may take it again (the panic hook runs on the
/// thread that panicked).
struct RegistryHold(Option<HANDLE>);

impl Drop for RegistryHold {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            // SAFETY: the handle is a mutex this thread owns (or an
            // abandoned one it took over) and is closed once.
            unsafe {
                let _ = ReleaseMutex(handle);
                let _ = CloseHandle(handle);
            }
        }
    }
}

impl Registry for WinRegistry {
    fn hold(&self) -> Box<dyn std::any::Any> {
        let name = HSTRING::from(REGISTRY_MUTEX);
        // SAFETY: `name` is a NUL-terminated wide string that outlives the
        // call; no security attributes and no initial owner.
        let Ok(handle) = (unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }) else {
            return Box::new(RegistryHold(None));
        };
        // SAFETY: the handle was just created and is valid.
        let waited = unsafe { WaitForSingleObject(handle, REGISTRY_WAIT_MS) };
        if waited == WAIT_OBJECT_0 || waited == WAIT_ABANDONED {
            return Box::new(RegistryHold(Some(handle)));
        }
        // Timed out or failed: go ahead without it.
        // SAFETY: `handle` was returned by CreateMutexW and is closed once.
        let _ = unsafe { CloseHandle(handle) };
        Box::new(RegistryHold(None))
    }

    fn set_on(&mut self) -> Result<(), RegError> {
        if EXITING.load(Ordering::SeqCst) {
            return Err(RegError("the client is exiting".to_owned()));
        }
        self.write_value(1)
    }

    fn read_value(&mut self) -> Result<Option<u32>, RegError> {
        let Some(key) = Key::open(POLICY_KEY, KEY_READ)? else {
            return Ok(None);
        };
        let name = HSTRING::from(POLICY_VALUE);
        Ok(key.read_number(PCWSTR(name.as_ptr()))?.map(|v| v as u32))
    }

    fn write_value(&mut self, value: u32) -> Result<(), RegError> {
        let key = Key::create(POLICY_KEY)?;
        let name = HSTRING::from(POLICY_VALUE);
        key.write_dword(PCWSTR(name.as_ptr()), value)
    }

    fn delete_value(&mut self) -> Result<(), RegError> {
        let Some(key) = Key::open(POLICY_KEY, KEY_SET_VALUE)? else {
            return Ok(());
        };
        let name = HSTRING::from(POLICY_VALUE);
        key.delete_value(PCWSTR(name.as_ptr()))
    }

    fn read_marker(&mut self) -> Result<StoredMarker, RegError> {
        let Some(key) = Key::open(MARKER_KEY, KEY_READ)? else {
            return Ok(StoredMarker::Absent);
        };
        let pid = key.read_number(w!("OwnerPid"))?;
        let start = key.read_number(w!("OwnerStart"))?;
        let prior = key.read_number(w!("Prior"))?;
        let (Some(pid), Some(start), Some(prior)) = (pid, start, prior) else {
            return Ok(StoredMarker::Incomplete);
        };
        let prior = Prior::from_stored(prior)?;
        Ok(StoredMarker::Complete(Marker {
            owner: Owner {
                pid: pid as u32,
                start,
            },
            prior,
        }))
    }

    fn write_marker(&mut self, marker: Marker) -> Result<(), RegError> {
        let key = Key::create(MARKER_KEY)?;
        key.write_dword(w!("OwnerPid"), marker.owner.pid)?;
        key.write_qword(w!("OwnerStart"), marker.owner.start)?;
        key.write_qword(w!("Prior"), marker.prior.to_stored())
    }

    fn delete_marker(&mut self) -> Result<(), RegError> {
        let path = HSTRING::from(MARKER_KEY);
        // SAFETY: `path` is a NUL-terminated wide string that outlives the
        // call.
        let code = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        ok("RegDeleteKeyW", code)
    }
}

fn filetime_u64(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// The creation time of a process handle, or `None` when it cannot be read.
fn creation_time(process: HANDLE) -> Option<u64> {
    let mut created = FILETIME::default();
    let mut other = [FILETIME::default(); 3];
    // SAFETY: every out pointer is a valid FILETIME.
    unsafe {
        GetProcessTimes(
            process,
            &mut created,
            &mut other[0],
            &mut other[1],
            &mut other[2],
        )
        .ok()?;
    }
    Some(filetime_u64(created))
}

/// This process as an owner.
///
/// `None` when Windows will not give this process's start time: an owner
/// without it cannot be told from a reused pid, so the registry route is not
/// used at all.
pub fn current_owner() -> Option<Owner> {
    // SAFETY: both calls have no preconditions; the pseudo-handle needs no
    // closing.
    let (pid, start) = unsafe { (GetCurrentProcessId(), creation_time(GetCurrentProcess())) };
    start.map(|start| Owner { pid, start })
}

/// The owner [`init`] recorded: the one value the marker, the watchdog and
/// the exit paths all use.
pub fn owner() -> Option<Owner> {
    OWNER.get().copied()
}

/// The process table.
#[derive(Debug, Default)]
pub struct WinProcesses;

impl Processes for WinProcesses {
    fn is_running(&self, owner: Owner) -> bool {
        // SAFETY: OpenProcess has no preconditions; the handle is closed
        // below.
        let Ok(handle) = (unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                owner.pid,
            )
        }) else {
            return false;
        };
        let start = creation_time(handle);
        let ended = has_ended(handle);
        // SAFETY: `handle` was returned by OpenProcess and is closed once.
        let _ = unsafe { CloseHandle(handle) };
        // A process that has ended still has a process object, and so a
        // readable start time, for as long as anyone (a parent that has not
        // let go of its `Child`, say) holds a handle to it. Only the pid and
        // the start time matching is therefore not "running".
        !ended && start == Some(owner.start)
    }
}

/// Whether the process object is signalled, which is what a process that has
/// exited is.
fn has_ended(process: HANDLE) -> bool {
    // SAFETY: the handle has SYNCHRONIZE access; a zero wait only polls.
    let waited = unsafe { WaitForSingleObject(process, 0) };
    waited == WAIT_OBJECT_0
}

/// Waits on a process handle for the owner to end.
struct WaitOnProcess(HANDLE);

impl OwnerExit for WaitOnProcess {
    fn wait(&mut self) {
        // SAFETY: the handle has SYNCHRONIZE access and stays open for the
        // wait.
        unsafe { WaitForSingleObject(self.0, INFINITE) };
    }
}

/// Records this process as the owner for [`restore_now`] and installs the
/// panic hook (for this thread, the window's: see [`MAIN_THREAD`]) and the
/// console control handler. Call once, early, on the window's thread.
pub fn init() -> Option<Owner> {
    let owner = current_owner();
    if let Some(owner) = owner {
        let _ = OWNER.set(owner);
    }
    let _ = MAIN_THREAD.set(std::thread::current().id());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if MAIN_THREAD.get() == Some(&std::thread::current().id()) {
            restore_now();
        }
        previous(info);
    }));
    // SAFETY: `ctrl_handler` is a valid handler routine for the process's
    // life.
    let _ = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), true) };
    owner
}

/// The shared restore: Drop guard, panic hook, console handler,
/// `WM_ENDSESSION` and `WM_DESTROY` all end here. Does nothing before
/// [`init`], and nothing when this process has no marker.
pub fn restore_now() {
    // Before the restore takes the registry: a turn-on that is under way
    // finishes first (it holds the registry), and none starts after.
    EXITING.store(true, Ordering::SeqCst);
    let Some(owner) = OWNER.get().copied() else {
        return;
    };
    let _ = lock_policy::restore_if_owned(&mut WinRegistry, Some(owner));
}

unsafe extern "system" fn ctrl_handler(kind: u32) -> BOOL {
    if matches!(
        kind,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        restore_now();
    }
    // Not handled: the default handler still ends the process.
    BOOL(0)
}

/// Restores on drop, so `?` exits and normal returns are covered.
#[derive(Debug)]
pub struct LockGuard;

impl Drop for LockGuard {
    fn drop(&mut self) {
        restore_now();
    }
}

/// `--restore-lock-policy`: the startup recovery by hand, printed.
pub fn restore_by_hand() -> Recovery {
    lock_policy::recover(&mut WinRegistry, &WinProcesses)
}

/// `--lock-watchdog <pid> <start>`: waits for the client to end, then
/// restores if the marker still names it. Returns the line to log.
pub fn watchdog(owner: Owner) -> String {
    let line = restore_after_owner(owner);
    // The client cannot let go of the modifiers itself after a hard kill, and
    // the grab may have swallowed the Up of one Windows still counts as down.
    let released = crate::modifiers::release_local();
    if released == 0 {
        line
    } else {
        format!("{line}; modifiers released locally: {released}")
    }
}

fn restore_after_owner(owner: Owner) -> String {
    // SAFETY: OpenProcess has no preconditions; the handle is closed below.
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            owner.pid,
        )
    };
    let Ok(handle) = handle else {
        // The client is already gone (or the pid is not ours to open).
        return match lock_policy::restore_if_owned(&mut WinRegistry, Some(owner)) {
            Ok(Some(_)) => "lock watchdog: restored (owner already gone)".to_owned(),
            Ok(None) => "lock watchdog: nothing to restore".to_owned(),
            Err(e) => format!("lock watchdog: restore failed: {e}"),
        };
    };
    let line = if creation_time(handle) == Some(owner.start) {
        match lock_policy::run_watchdog(&mut WaitOnProcess(handle), &mut WinRegistry, owner) {
            Ok(Some(_)) => "lock watchdog: restored after the client ended".to_owned(),
            Ok(None) => "lock watchdog: nothing to restore".to_owned(),
            Err(e) => format!("lock watchdog: restore failed: {e}"),
        }
    } else {
        // The pid now belongs to another process: the client is gone.
        match lock_policy::restore_if_owned(&mut WinRegistry, Some(owner)) {
            Ok(Some(_)) => "lock watchdog: restored (pid reused)".to_owned(),
            Ok(None) => "lock watchdog: nothing to restore".to_owned(),
            Err(e) => format!("lock watchdog: restore failed: {e}"),
        }
    };
    // SAFETY: `handle` was returned by OpenProcess and is closed once.
    let _ = unsafe { CloseHandle(handle) };
    line
}

/// Starts the helper: the same exe, no window, outside the client's job when
/// that is allowed (a job that does not allow breakaway refuses the flag, so
/// the helper is then started inside the job). Before the setting is first
/// turned on, and again whenever the helper has gone.
pub fn spawn_watchdog(owner: Owner) -> std::io::Result<Child> {
    let exe = std::env::current_exe()?;
    let start = |flags: u32| {
        Command::new(&exe)
            .arg("--lock-watchdog")
            .arg(owner.pid.to_string())
            .arg(owner.start.to_string())
            .creation_flags(flags)
            .spawn()
    };
    start(WATCHDOG_FLAGS).or_else(|_| start(WATCHDOG_FLAGS & !BREAKAWAY))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::process::Stdio;

    use super::WinProcesses;
    use super::current_owner;
    use crate::lock_policy::Owner;
    use crate::lock_policy::Processes;

    /// Windows only (it needs a real process table), so `scripts/check`
    /// compiles it for the Windows target but cannot run it; it runs with
    /// `cargo test -p gliff-windows lockreg` on the Windows machine.
    #[test]
    fn an_ended_process_whose_handle_is_still_held_is_not_running() {
        let me = current_owner().expect("own start time");
        assert!(WinProcesses.is_running(me), "this process is running");

        let mut child = Command::new("cmd")
            .args(["/C", "exit", "0"])
            .stdout(Stdio::null())
            .spawn()
            .expect("cmd");
        let pid = child.id();
        child.wait().expect("wait");
        // `child` still holds its handle here, as the lock step's does after
        // a kill. Its start time stays readable through that handle.
        let start = {
            use windows::Win32::System::Threading::OpenProcess;
            use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
            // SAFETY: a plain open; closed below.
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
                .expect("the held handle keeps the process object");
            let t = super::creation_time(h).expect("start time");
            // SAFETY: the handle just opened.
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(h) };
            t
        };
        assert!(!WinProcesses.is_running(Owner { pid, start }));
        drop(child);
    }
}
