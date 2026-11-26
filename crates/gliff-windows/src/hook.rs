//! The low-level keyboard hook. It runs on its own
//! thread with a message loop, so a slow UI thread cannot make Windows give
//! up on it, and it does as little as possible: look at the key, decide
//! swallow or pass, and hand the key to the session.
//!
//! The decision itself is in [`crate::super_l`] and
//! [`crate::input::KeyPolicy`], both tested on the build machine. This file
//! is the part that needs Windows, and is only compiled there.

use std::cell::RefCell;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use windows::Win32::Foundation::ERROR_INVALID_THREAD_ID;
use windows::Win32::Foundation::ERROR_NOT_ENOUGH_QUOTA;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0;
use windows::Win32::UI::Input::KeyboardAndMouse::INPUT_KEYBOARD;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY;
use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::HC_ACTION;
use windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;
use windows::Win32::UI::WindowsAndMessaging::LLKHF_EXTENDED;
use windows::Win32::UI::WindowsAndMessaging::LLKHF_INJECTED;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE;
use windows::Win32::UI::WindowsAndMessaging::PeekMessageW;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetWindowsHookExW;
use windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx;
use windows::Win32::UI::WindowsAndMessaging::WH_KEYBOARD_LL;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYUP;
use windows::Win32::UI::WindowsAndMessaging::WM_QUIT;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYUP;

use crate::altgr::Origin;
use crate::altgr::RawKey;
use crate::chord_watch::ChordWatch;
use crate::chord_watch::Decision as WatchDecision;
use crate::chord_watch::Fate;
use crate::chord_watch::HookGate;
use crate::chord_watch::SwitchChords;
use crate::chord_watch::Taker;
use crate::chord_watch::switch_word;
use crate::grab_state::HookMode;
use crate::input::KeyPolicy;
use crate::key_ledger::KeyId;
use crate::key_ledger::KeyLedger;
use crate::key_ledger::Physical;
use crate::key_ledger::Route;
use crate::key_ledger::Seen;
use crate::key_ledger::describe_down;
use crate::key_ledger::describe_key;
use crate::keymap::Chord;
use crate::keymap::Prefix;
use crate::keymap::Press;
use crate::local_chords::ChordDecision;
use crate::local_chords::LocalChord;
use crate::local_chords::LocalChords;
use crate::modifiers::MODIFIERS;
use crate::pass_log::HookTiming;
use crate::pass_log::OWN_TAG;
use crate::pass_log::PassLog;
use crate::route::InputRoute;
use crate::session::UiCmd;
use crate::super_l::Decision;
use crate::super_l::SuperLTracker;
use crate::tabs::SwitchTo;
use crate::test_hooks::TestHooks;
use crate::waker::WM_APP_SWITCH;
use crate::win_owed::WinOwed;

/// A virtual key no program binds (`VK_E8` is unassigned): tapped between
/// the Win key's Down and Up so Start does not open.
const VK_MASK: u16 = 0xe8;

/// What the callback does now. A static because the callback has no context
/// pointer; the UI thread sets it, the hook thread reads it.
///
/// Held by the callback from reading the mode to handing the key to the
/// session, and by [`set_mode`] while it stores. So when `set_mode` returns,
/// no key decided under the old mode is still on its way: a `ReleaseInput`
/// sent afterwards is ordered after every key the hook forwarded.
static GATE: Mutex<HookGate> = Mutex::new(HookGate::new());

/// The keys down while the hook looks, for the release chord. Set when the
/// hook starts; locked only under [`GATE`].
static WATCH: Mutex<Option<ChordWatch>> = Mutex::new(None);

/// The window that gets a posted tab switch, as an integer (an `HWND` is not
/// `Send`); 0 until the UI thread sets it.
static SWITCH_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Sets the window a switch chord posts to.
pub fn set_switch_window(hwnd: HWND) {
    SWITCH_WINDOW.store(hwnd.0 as isize, Ordering::Release);
}

/// Replaces the release chord and the switch chords the hook watches for.
/// Takes effect with the next key.
pub fn set_chords(release: Chord, switch: SwitchChords) {
    if let Some(watch) = WATCH
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        watch.set_chords(release, switch);
    }
}

/// Posts a switch to the UI thread. False when there is no window to post to
/// or the post failed.
fn post_switch(to: SwitchTo) -> bool {
    let hwnd = HWND(SWITCH_WINDOW.load(Ordering::Acquire) as *mut core::ffi::c_void);
    if hwnd.0.is_null() {
        return false;
    }
    // SAFETY: PostMessageW takes any HWND value; a destroyed window makes
    // it fail, and the key is then left to Windows.
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_APP_SWITCH,
            WPARAM(switch_word(to)),
            LPARAM(0),
        )
    }
    .is_ok()
}

/// Keys the hook let through while grabbing; see [`crate::pass_log`].
static PASSED: Mutex<PassLog> = Mutex::new(PassLog::new());

/// True if the hook let this key through on purpose a moment ago, so the key
/// message that just reached the window is not a sign of a silent hook.
pub fn was_passed(vk: u16, press: Press, time_ms: u32) -> bool {
    PASSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take(vk, press, time_ms)
}

/// True if the hook noted this key's family a while ago (see
/// [`PassLog::noted_near`]); for the silent-hook line.
pub fn noted_near(vk: u16, time_ms: u32) -> bool {
    PASSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .noted_near(vk, time_ms)
}

/// How the hook's callbacks went since it was installed, kept in atomics
/// because the callback must stay fast and takes no lock for it. Written by
/// the hook thread only, read by the UI thread for the silent-hook line.
static CALLS: AtomicU32 = AtomicU32::new(0);
static LAST_US: AtomicU32 = AtomicU32::new(0);
static SLOWEST_US: AtomicU32 = AtomicU32::new(0);
static INJECTED: AtomicU32 = AtomicU32::new(0);
/// When the last callback ended, in milliseconds after [`epoch`] plus one;
/// 0 if none has.
static LAST_END_MS: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn reset_timing() {
    for counter in [&CALLS, &LAST_US, &SLOWEST_US, &INJECTED] {
        counter.store(0, Ordering::Relaxed);
    }
    LAST_END_MS.store(0, Ordering::Relaxed);
}

fn note_call(started: Instant) {
    let us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
    CALLS.fetch_add(1, Ordering::Relaxed);
    LAST_US.store(us, Ordering::Relaxed);
    SLOWEST_US.fetch_max(us, Ordering::Relaxed);
    let end = u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX);
    LAST_END_MS.store(end.saturating_add(1), Ordering::Relaxed);
}

/// The hook's callback timing, for [`crate::pass_log::silent_line`].
pub fn timing() -> HookTiming {
    let end = LAST_END_MS.load(Ordering::Relaxed);
    let now = u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX);
    HookTiming {
        calls: CALLS.load(Ordering::Relaxed),
        last_us: LAST_US.load(Ordering::Relaxed),
        slowest_us: SLOWEST_US.load(Ordering::Relaxed),
        injected: INJECTED.load(Ordering::Relaxed),
        ago_ms: end.checked_sub(1).map(|end| now.saturating_sub(end)),
    }
}

/// What Windows and the remote saw of each key during the grab (see
/// [`crate::key_ledger`]). It lives as long as the process, not as long as one
/// hook: a hook that is installed again goes on with it. Locked only under
/// [`GATE`].
static LEDGER: Mutex<KeyLedger> = Mutex::new(KeyLedger::new());

/// The keys as the user holds them, by what the hook and the window messages
/// saw (see [`Physical`]). Never held while another lock is taken.
static PHYSICAL: Mutex<Physical> = Mutex::new(Physical::new());

fn physical_saw(vk: u16, press: Press) {
    PHYSICAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .saw(vk, press);
}

/// The Win keys whose Up the hook kept from Windows (see
/// [`crate::win_owed`]). Never held while another lock is taken.
static WIN_OWED: Mutex<WinOwed> = Mutex::new(WinOwed::new());

fn win_owed() -> std::sync::MutexGuard<'static, WinOwed> {
    WIN_OWED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every Win key whose Up Windows is still owed, for the exit's release. A
/// `try_lock`, because the exit's release also runs from the panic hook.
pub fn owed_wins() -> Vec<u16> {
    match WIN_OWED.try_lock() {
        Ok(owed) => owed.owed(),
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().owed(),
        Err(std::sync::TryLockError::WouldBlock) => Vec::new(),
    }
}

/// After the callback: a Win key event the hook swallowed leaves Windows
/// owed its Up, and a Win Up that reached Windows pays it. When the
/// swallowed event is the Up itself, Windows gets an Up of its own now (the
/// Start-menu mask first), so a Win key Windows took as down ahead of the
/// hook (Win+L) does not stay down.
fn note_win(vk: u16, press: Press, swallowed: bool, time_ms: u32) {
    if !matches!(vk, crate::modifiers::VK_LWIN | crate::modifiers::VK_RWIN) {
        return;
    }
    if !swallowed {
        if press == Press::Up {
            win_owed().windows_saw_up(vk);
        }
        return;
    }
    win_owed().swallowed(vk);
    if press == Press::Up {
        give_win_up(vk, time_ms);
    }
}

/// Injects the Start-menu mask and an Up of this Win key, both this client's
/// own: noted in the pass log first, so the key messages they make are not
/// taken for a silent hook.
fn give_win_up(vk: u16, time_ms: u32) {
    {
        let mut passed = PASSED.lock().unwrap_or_else(PoisonError::into_inner);
        passed.record(VK_MASK, Press::Down, time_ms);
        passed.record(VK_MASK, Press::Up, time_ms);
        passed.record(vk, Press::Up, time_ms);
    }
    let inputs = [
        key_input(VK_MASK, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_MASK, KEYEVENTF_KEYUP),
        key_input(vk, KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY),
    ];
    // SAFETY: `inputs` is a valid slice of INPUT and the size is that of
    // one INPUT, as SendInput requires.
    let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    win_owed().settle(&[vk]);
}

/// What the hook has seen so far, as a copy.
fn physical_now() -> Physical {
    PHYSICAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// True if the hook has handled an event of this key later than a key
/// message from `time_ms` (see [`Physical::saw_later`]).
pub fn saw_later(key: KeyId, time_ms: u32) -> bool {
    PHYSICAL
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .saw_later(key, time_ms)
}

/// What Windows, the ledger and the hook's own view say is down: a count and
/// the modifier groups of each, no other key. For the log at a grab end and a
/// hook reinstall, which is where a stuck modifier shows.
fn state_line() -> String {
    format!(
        "Windows: {}; Win Up kept from Windows, so Windows may count it as down: {}; \
         ledger (Windows saw go down): {}; user holds (hook's view): {}",
        describe_down(held_keys()),
        describe_down(win_owed().owed()),
        describe_down(
            LEDGER
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .windows_held()
        ),
        describe_down(
            PHYSICAL
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .down_keys()
        )
    )
}

/// A grab has ended: releases the modifiers Windows counts as down, except
/// those the hook saw the user press and not let go of (the user's own Up
/// reaches Windows). Returns what it released.
pub fn release_at_grab_end() -> Vec<crate::modifiers::Modifier> {
    // Platform: SendInput runs this process's own hook, so no lock is held across it.
    let real = physical_now();
    let due = win_owed().due(|vk| real.seen(vk) == Seen::Down);
    let released =
        crate::modifiers::release_held_where(|down, vk| real.stuck_at_grab_end(down, vk), &due);
    win_owed().clear();
    released
}

/// The hook was installed again: releases the modifiers Windows counts as
/// down although the hook saw them come up.
fn release_at_reinstall() -> Vec<crate::modifiers::Modifier> {
    // Platform: SendInput runs this process's own hook, so no lock is held across it.
    let real = physical_now();
    let due = win_owed().due(|vk| real.seen(vk) == Seen::Down);
    let released =
        crate::modifiers::release_held_where(|down, vk| real.stuck_at_reinstall(down, vk), &due);
    win_owed().settle(&due);
    released
}

/// The mode the hook is in, which is not always the mode the UI thread
/// wants: after the release chord the hook is off until the UI thread has
/// caught up.
pub fn mode() -> HookMode {
    GATE.lock().unwrap_or_else(PoisonError::into_inner).mode()
}

/// A key message reached the window while the hook looked silent, and `sent`
/// says whether the window passed it on to the remote. Windows saw it either
/// way. `key` names the key as the hook does (see [`KeyId::of_message`]),
/// so its Up meets the Down the hook saw.
pub fn note_legacy(key: KeyId, press: Press, sent: bool, time_ms: u32) {
    let vk = key.vk();
    physical_saw(vk, press);
    let _gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
    LEDGER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .legacy(key, press, sent, time_ms);
}

/// The virtual keys Windows has down now, for [`crate::pass_log::silent_line`],
/// which logs how many, and which modifier groups, and nothing more.
pub fn held_now() -> Vec<u16> {
    held_keys()
}

/// Sets what the hook does with the next key. When a grab starts, the keys
/// Windows holds down at that moment are noted, so the hook can let their
/// Ups through instead of swallowing them. After the hook took the keyboard
/// back by itself (the release chord), a request for a grab is refused until
/// [`release_handled`].
pub fn set_mode(mode: HookMode) {
    let mut gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
    let was = gate.mode();
    let now = gate.request(mode);
    let ended = (was == HookMode::Grab && now != HookMode::Grab).then(state_line);
    if was == HookMode::Off && now != HookMode::Off {
        PHYSICAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .forget();
    }
    let mut ledger = LEDGER.lock().unwrap_or_else(PoisonError::into_inner);
    if now != HookMode::Grab {
        ledger.forget_windows();
    } else if was != HookMode::Grab {
        ledger.seed(held_keys());
    }
    let mut watch = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(watch) = watch.as_mut() {
        if now == HookMode::Off {
            watch.clear();
        } else if was == HookMode::Off {
            watch.seed(held_keys());
        }
    }
    drop(watch);
    drop(ledger);
    drop(gate);
    if let Some(line) = ended {
        crate::logging::line(&format!("grab ended, keys down: {line}"));
    }
}

/// The UI thread has taken in a release chord: its state no longer wants a
/// grab, so the hook may be asked for one again (a click).
pub fn release_handled() {
    GATE.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .acknowledge();
}

/// One UI tick: true when a release chord the hook fired has waited too long
/// for [`release_handled`] (see [`HookGate::overdue`]). The caller then takes
/// the chord in and calls [`release_handled`].
pub fn release_overdue() -> bool {
    GATE.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .overdue()
}

/// The virtual keys Windows has down now. Virtual keys 1 to 6 are the mouse
/// buttons; the rest up to 0xfe are keys.
fn held_keys() -> Vec<u16> {
    (7u16..0xff)
        .filter(|vk| {
            // SAFETY: GetAsyncKeyState has no preconditions.
            let state = unsafe { GetAsyncKeyState(i32::from(*vk)) };
            state < 0
        })
        .collect()
}

/// Feeds the key to the chord watch; what it decided, if a chord completed. Locked only under [`GATE`].
fn watch_decides(raw: &RawKey) -> Option<WatchDecision> {
    WATCH
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
        .and_then(|watch| watch.push(raw))
}

/// What the callback needs, owned by the hook thread.
struct Ctx {
    test_hooks: TestHooks,
    cmds: Arc<InputRoute<UiCmd>>,
    taker: Taker,
    policy: KeyPolicy,
    tracker: SuperLTracker,
    chords: LocalChords,
    last_mode: HookMode,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

/// The hook and its thread. Dropping it removes the hook.
#[derive(Debug)]
pub struct HookThread {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
    /// Disconnects when the hook thread ends.
    ended: Receiver<Infallible>,
}

/// How long dropping the hook waits for its thread. The thread leaves its
/// loop at once unless the callback is stuck; the UI thread must not hang
/// on that.
const STOP_WAIT: Duration = Duration::from_millis(500);

impl HookThread {
    /// Installs the hook on a new thread. Fails with Windows' reason when
    /// the hook cannot be set.
    pub fn start(
        cmds: Arc<InputRoute<UiCmd>>,
        policy: KeyPolicy,
        release_chord: Chord,
        switch_chords: SwitchChords,
        test_hooks: TestHooks,
        local_chords: Vec<LocalChord>,
    ) -> Result<Self, String> {
        reset_timing();
        let mode = {
            let gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
            let mode = gate.mode();
            let held = if mode == HookMode::Off {
                Vec::new()
            } else {
                held_keys()
            };
            if mode == HookMode::Grab {
                LEDGER
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .reinstall(held.iter().copied());
            }
            *WATCH.lock().unwrap_or_else(PoisonError::into_inner) = Some(
                ChordWatch::after_reinstall(release_chord, switch_chords, mode, held),
            );
            mode
        };
        let remote = (mode == HookMode::Grab).then(|| cmds.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (ended_tx, ended) = std::sync::mpsc::channel::<Infallible>();
        let join = std::thread::Builder::new()
            .name("keyboard-hook".to_owned())
            .spawn(move || {
                let ctx = Ctx {
                    test_hooks,
                    cmds,
                    taker: Taker::new(),
                    policy,
                    tracker: SuperLTracker::default(),
                    chords: LocalChords::new(local_chords),
                    last_mode: HookMode::Off,
                };
                run(ctx, &ready_tx);
                drop(ended_tx);
            })
            .map_err(|e| format!("could not start the keyboard hook thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => {
                if mode != HookMode::Off {
                    crate::logging::line(&format!(
                        "hook installed again, keys down: {}",
                        state_line()
                    ));
                    if let Some(remote) = remote {
                        let user_holds = PHYSICAL
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .held_modifiers();
                        remote.send(UiCmd::ClearModifiers { user_holds });
                        crate::logging::line(
                            "hook installed again: told the remote Win, Ctrl, Alt and Shift are \
                             up, except the keys the user holds",
                        );
                    }
                    let released = release_at_reinstall();
                    if !released.is_empty() {
                        crate::logging::line(&format!(
                            "hook installed again: {} modifier key(s) Windows still counted as \
                             down, though the hook saw them come up, were released ({})",
                            released.len(),
                            crate::modifiers::names(&released)
                        ));
                    }
                }
                Ok(Self {
                    thread_id,
                    join: Some(join),
                    ended,
                })
            }
            Ok(Err(reason)) => {
                let _ = join.join();
                Err(reason)
            }
            Err(_) => Err("the keyboard hook thread ended before it was ready".to_owned()),
        }
    }
}

impl HookThread {
    /// Asks the thread to quit and waits for it up to [`STOP_WAIT`], then
    /// detaches it. Returns the lines to log: a refused quit post, named by
    /// its Windows error, and a thread that did not stop in time. A second
    /// call does nothing.
    fn stop(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let Some(join) = self.join.take() else {
            return lines;
        };
        // SAFETY: PostThreadMessageW takes any thread id.
        if let Err(e) = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
        {
            lines.push(format!(
                "PostThreadMessageW(WM_QUIT) failed: {} ({e})",
                win32_error_name(&e)
            ));
        }
        if crate::bounded_wait::ended_within(&self.ended, STOP_WAIT) {
            let _ = join.join();
        } else {
            lines.push("keyboard hook thread did not stop in time; not waiting for it".to_owned());
        }
        lines
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        for line in self.stop() {
            crate::logging::line(&line);
        }
    }
}

/// The name of the Windows error `PostThreadMessageW` returns for a refused
/// post, or its HRESULT for any other error.
fn win32_error_name(e: &windows::core::Error) -> String {
    let code = e.code();
    if code == ERROR_NOT_ENOUGH_QUOTA.to_hresult() {
        "ERROR_NOT_ENOUGH_QUOTA".to_owned()
    } else if code == ERROR_INVALID_THREAD_ID.to_hresult() {
        "ERROR_INVALID_THREAD_ID".to_owned()
    } else {
        format!("HRESULT {:#010x}", code.0)
    }
}

fn run(ctx: Ctx, ready: &std::sync::mpsc::Sender<Result<u32, String>>) {
    CTX.with(|c| *c.borrow_mut() = Some(ctx));
    // SAFETY: GetModuleHandleW(None) returns this exe's handle and has no
    // other precondition.
    let module = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module,
        Err(e) => {
            let _ = ready.send(Err(format!("GetModuleHandleW failed: {e}")));
            return;
        }
    };
    // SAFETY: `hook_proc` has the LowLevelKeyboardProc signature and lives
    // for the program; the module is this exe; thread id 0 makes the hook
    // global, which a low-level hook requires.
    let hook =
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(module.into()), 0) }
        {
            Ok(hook) => hook,
            Err(e) => {
                let _ = ready.send(Err(format!("SetWindowsHookExW failed: {e}")));
                return;
            }
        };
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG. PM_NOREMOVE only creates this thread's
    // message queue, so a WM_QUIT posted right after `ready` is not lost.
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
    // SAFETY: no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    let _ = ready.send(Ok(thread_id));
    // SAFETY: `msg` is a valid MSG for the whole loop.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
    // SAFETY: `hook` came from SetWindowsHookExW and is removed once.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
    CTX.with(|c| *c.borrow_mut() = None);
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && code as u32 == HC_ACTION && lparam.0 != 0 {
        // SAFETY: for HC_ACTION, lparam points at a KBDLLHOOKSTRUCT that
        // stays valid for this call.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let started = Instant::now();
        if info.flags.0 & LLKHF_INJECTED.0 != 0 && info.dwExtraInfo != OWN_TAG {
            INJECTED.fetch_add(1, Ordering::Relaxed);
        }
        let press = match wparam.0 as u32 {
            WM_KEYUP | WM_SYSKEYUP => Press::Up,
            _ => Press::Down,
        };
        let swallow = CTX.with(|c| {
            // Platform: SendInput inside the callback calls the callback again.
            c.try_borrow_mut()
                .ok()
                .and_then(|mut ctx| ctx.as_mut().map(|ctx| ctx.handle(wparam.0 as u32, info)))
                .unwrap_or(false)
        });
        note_win(info.vkCode as u16, press, swallow, info.time);
        note_call(started);
        if swallow {
            return LRESULT(1);
        }
    }
    // SAFETY: the arguments are the ones Windows gave this hook.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

impl Ctx {
    /// True to swallow the key.
    fn handle(&mut self, message: u32, key: &KBDLLHOOKSTRUCT) -> bool {
        let press = match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => Press::Down,
            WM_KEYUP | WM_SYSKEYUP => Press::Up,
            _ => return false,
        };
        let vk = key.vkCode as u16;
        if PASSED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .note_if_own(key.dwExtraInfo, vk, press, key.time)
        {
            return false;
        }
        {
            let mut real = PHYSICAL.lock().unwrap_or_else(PoisonError::into_inner);
            real.saw(vk, press);
            let extended = key.flags.0 & LLKHF_EXTENDED.0 != 0;
            real.moved(KeyId::of_hook(vk, extended), key.time);
        }
        self.test_hooks
            .on_key(vk, press, key.flags.0 & LLKHF_INJECTED.0 != 0);
        let mut gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
        let id = KeyId::of_hook(vk, key.flags.0 & LLKHF_EXTENDED.0 != 0);
        if LEDGER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .window_took(id, press, key.time)
        {
            drop(gate);
            crate::logging::line(&format!(
                "keyboard hook: a key the window already sent came late from a hook call \
                 Windows gave up on; dropped ({})",
                describe_key(vk, press)
            ));
            return false;
        }
        let now = gate.mode();
        if now != self.last_mode {
            self.tracker.reset();
            self.chords.reset();
            if self.last_mode == HookMode::Grab {
                self.release_remote();
            }
            self.last_mode = now;
        }
        let raw = RawKey {
            vk,
            scan: key.scanCode as u16,
            prefix: if key.flags.0 & LLKHF_EXTENDED.0 != 0 {
                Prefix::E0
            } else {
                Prefix::None
            },
            press,
            time_ms: key.time,
            origin: Origin::Hook,
            injected: key.flags.0 & LLKHF_INJECTED.0 != 0,
        };
        let decision = if now != HookMode::Off && self.policy.forwards(&raw) {
            watch_decides(&raw)
        } else {
            None
        };
        let fate = self
            .taker
            .fate(now, self.cmds.target().is_some(), decision, vk, press);
        match fate {
            Fate::Release => {
                gate.chord_fired();
                WATCH
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter_mut()
                    .for_each(ChordWatch::clear);
                let sent = self.cmds.send(UiCmd::ReleaseGrab);
                if sent {
                    LEDGER
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take_remote_held();
                }
                return sent;
            }
            Fate::Switch(to) => {
                if post_switch(to) {
                    return true;
                }
                self.taker = Taker::new();
            }
            Fate::Swallow => return true,
            Fate::Pass => {
                self.windows_gets(&raw, now);
                return false;
            }
            Fate::Normal => {}
        }
        match now {
            HookMode::Off => false,
            HookMode::SuperLOnly => self.super_l_only(&raw),
            HookMode::Grab => self.grab(raw),
        }
    }

    fn grab(&mut self, raw: RawKey) -> bool {
        let route = LEDGER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .route(raw.vk, raw.press);
        let forwards = self.policy.forwards(&raw);
        if route != Route::Remote || !forwards {
            self.chords.push(raw.vk, raw.press, true);
            PASSED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .record(raw.vk, raw.press, raw.time_ms);
            if route == (Route::Windows { remote: true }) && forwards {
                let _ = self.cmds.send(UiCmd::Key(raw));
            }
            return false;
        }
        match self.chords.push(raw.vk, raw.press, false) {
            ChordDecision::Forward => {}
            ChordDecision::Swallow => return true,
            ChordDecision::Take { modifiers, trigger } => {
                if self.local_chord(&raw, &modifiers, trigger) {
                    return true;
                }
                self.chords.cancel_take();
            }
        }
        let (vk, press, time_ms) = (raw.vk, raw.press, raw.time_ms);
        let sent = self.cmds.send(UiCmd::Key(raw));
        let mut ledger = LEDGER.lock().unwrap_or_else(PoisonError::into_inner);
        if sent {
            ledger.remote_saw(vk, press);
        } else {
            ledger.windows_saw(vk, press);
            PASSED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .record(vk, press, time_ms);
        }
        sent
    }

    /// The hook lets this key go to Windows while it is grabbing: Windows
    /// sees its Down, so Windows must get its Up. Noted in the ledger and in
    /// the pass log, so the window does not take the message for a silent
    /// hook, and in the local chords, so a modifier let go here is not
    /// replayed by a local chord later in the grab.
    fn windows_gets(&mut self, raw: &RawKey, now: HookMode) {
        if now != HookMode::Grab {
            return;
        }
        self.chords.push(raw.vk, raw.press, true);
        LEDGER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .windows_saw(raw.vk, raw.press);
        PASSED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(raw.vk, raw.press, raw.time_ms);
    }

    /// The grab ended (or the hook found it had): the remote machine lets go
    /// of every key the hook sent it that has not come up. The UI thread does
    /// this too when it ends a grab; this covers a command that did not get
    /// through.
    fn release_remote(&self) {
        let held = LEDGER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take_remote_held();
        if !held.is_empty() {
            let _ = self.cmds.send(UiCmd::ReleaseInput);
        }
    }

    /// A local chord completed (see [`crate::local_chords`]): the remote
    /// machine lets go of every key it holds, and Windows gets the chord
    /// replayed, because the grab swallowed the modifiers' Downs. Their real
    /// Ups and auto-repeats are Windows' too, from here on. True to swallow
    /// the real trigger key; false, with nothing replayed, when the session
    /// is gone.
    fn local_chord(&mut self, raw: &RawKey, modifiers: &[u16], trigger: u16) -> bool {
        if !self.cmds.send(UiCmd::ReleaseInput) {
            return false;
        }
        let mut ledger = LEDGER.lock().unwrap_or_else(PoisonError::into_inner);
        ledger.take_remote_held();
        let mut passed = PASSED.lock().unwrap_or_else(PoisonError::into_inner);
        let mut inputs = Vec::with_capacity(modifiers.len() + 2);
        for &vk in modifiers {
            ledger.windows_saw(vk, Press::Down);
            let extended = MODIFIERS.iter().any(|m| m.vk == vk && m.extended);
            let flags = if extended {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            };
            inputs.push(key_input(vk, flags));
            passed.record(vk, Press::Down, raw.time_ms);
        }
        inputs.push(key_input(trigger, KEYBD_EVENT_FLAGS(0)));
        inputs.push(key_input(trigger, KEYEVENTF_KEYUP));
        passed.record(trigger, Press::Down, raw.time_ms);
        passed.record(trigger, Press::Up, raw.time_ms);
        drop(passed);
        drop(ledger);
        // SAFETY: `inputs` is a valid slice of INPUT and the size is that of
        // one INPUT, as SendInput requires.
        let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        crate::logging::line(&format!(
            "local chord kept on Windows: {} + a key",
            crate::modifiers::names(
                &MODIFIERS
                    .iter()
                    .copied()
                    .filter(|m| modifiers.contains(&m.vk))
                    .collect::<Vec<_>>()
            )
        ));
        true
    }

    fn super_l_only(&mut self, raw: &RawKey) -> bool {
        if raw.injected {
            return false;
        }
        match self.tracker.push(raw.vk, raw.press) {
            Decision::Pass => false,
            Decision::Swallow => true,
            Decision::SendSuperL(events) => {
                if !self.cmds.send(UiCmd::Keys(events)) {
                    return false;
                }
                tap_mask();
                true
            }
        }
    }
}

pub(crate) fn key_input(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: OWN_TAG,
            },
        },
    }
}

/// Taps [`VK_MASK`] so a Win key release that follows Win+L is not read as
/// a bare Win tap (which would open Start).
pub(crate) fn tap_mask() {
    let inputs = [
        key_input(VK_MASK, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_MASK, KEYEVENTF_KEYUP),
    ];
    // SAFETY: `inputs` is a valid slice of INPUT and the size is that of
    // one INPUT, as SendInput requires.
    let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

#[cfg(test)]
mod tests;
