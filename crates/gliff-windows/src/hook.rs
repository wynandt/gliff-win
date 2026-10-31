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
use crate::key_ledger::KeyLedger;
use crate::key_ledger::Physical;
use crate::key_ledger::Route;
use crate::key_ledger::describe_down;
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

/// What Windows, the ledger and the hook's own view say is down: a count and
/// the modifier groups of each, no other key. For the log at a grab end and a
/// hook reinstall, which is where a stuck modifier shows.
fn state_line() -> String {
    format!(
        "Windows: {}; ledger (Windows saw go down): {}; user holds (hook's view): {}",
        describe_down(held_keys()),
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
    let real = PHYSICAL.lock().unwrap_or_else(PoisonError::into_inner);
    crate::modifiers::release_held_where(|down, vk| real.stuck_at_grab_end(down, vk))
}

/// The hook was installed again: releases the modifiers Windows counts as
/// down although the hook saw them come up.
fn release_at_reinstall() -> Vec<crate::modifiers::Modifier> {
    let real = PHYSICAL.lock().unwrap_or_else(PoisonError::into_inner);
    crate::modifiers::release_held_where(|down, vk| real.stuck_at_reinstall(down, vk))
}

/// The mode the hook is in, which is not always the mode the UI thread
/// wants: after the release chord the hook is off until the UI thread has
/// caught up.
pub fn mode() -> HookMode {
    GATE.lock().unwrap_or_else(PoisonError::into_inner).mode()
}

/// A key message reached the window while the hook looked silent, and `sent`
/// says whether the window passed it on to the remote. Windows saw it either
/// way.
pub fn note_legacy(vk: u16, press: Press, sent: bool) {
    physical_saw(vk, press);
    let _gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
    LEDGER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .legacy(vk, press, sent);
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
    let mut ledger = LEDGER.lock().unwrap_or_else(PoisonError::into_inner);
    if now != HookMode::Grab {
        // What the remote still holds stays, until the hook has made sure
        // that the session let go of it.
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
            // A hook installed again in the middle of a grab starts from the
            // keys Windows holds down now, not from nothing: a key that went
            // down while no hook looked still gets its Up, and a release
            // chord that is half held can be finished.
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
                    // Installed again while a hook mode is on (the window
                    // took a key for a silent hook, or the hook was gone):
                    // what is down now, and a release of any modifier the
                    // hook saw come up that Windows still counts as down.
                    // Sent after the new hook is in, so it ignores them.
                    crate::logging::line(&format!(
                        "hook installed again, keys down: {}",
                        state_line()
                    ));
                    if let Some(remote) = remote {
                        // The remote may have missed an Up while no hook looked.
                        remote.send(UiCmd::ClearModifiers);
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

impl Drop for HookThread {
    fn drop(&mut self) {
        // SAFETY: PostThreadMessageW takes any thread id; if the thread is
        // already gone it fails, which is fine because it is then joined.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        let Some(join) = self.join.take() else {
            return;
        };
        if crate::bounded_wait::ended_within(&self.ended, STOP_WAIT) {
            let _ = join.join();
        } else {
            // Stuck in a callback: the thread removes the hook when it gets
            // back to its loop. Waiting here would hang the window.
            crate::logging::line("keyboard hook thread did not stop in time; not waiting for it");
        }
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
    // The loop is what lets Windows call the hook on this thread; it ends
    // on WM_QUIT (GetMessageW returns 0) or an error (-1).
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
        let swallow = CTX.with(|c| {
            // try_borrow_mut: SendInput inside the callback can call it
            // again; the nested call passes the key.
            c.try_borrow_mut()
                .ok()
                .and_then(|mut ctx| ctx.as_mut().map(|ctx| ctx.handle(wparam.0 as u32, info)))
                .unwrap_or(false)
        });
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
        // A key this client injected passes, and then reaches the window as
        // a key message: noted, so the window does not take it for a silent
        // hook and install the hook again.
        if PASSED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .note_if_own(key.dwExtraInfo, vk, press, key.time)
        {
            return false;
        }
        physical_saw(vk, press);
        self.test_hooks
            .on_key(vk, press, key.flags.0 & LLKHF_INJECTED.0 != 0);
        let mut gate = GATE.lock().unwrap_or_else(PoisonError::into_inner);
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
                // The release chord: the keyboard goes back to Windows now,
                // from here, whatever the session or the UI thread are doing.
                // The session releases the remote's keys and the UI catches
                // up from the command.
                gate.chord_fired();
                WATCH
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .iter_mut()
                    .for_each(ChordWatch::clear);
                let sent = self.cmds.send(UiCmd::ReleaseGrab);
                if sent {
                    // The session lets go of every key it holds.
                    LEDGER
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take_remote_held();
                }
                return sent;
            }
            Fate::Switch(to) => {
                // The grab stays on. The UI thread moves the route; keys
                // after this one go to the new session. The trigger is
                // taken like a local chord's, so the new session never sees
                // it.
                if post_switch(to) {
                    return true;
                }
                // Nowhere to post: the key is an ordinary one.
                self.taker = Taker::new();
            }
            Fate::Swallow => return true,
            Fate::Pass => {
                // No session to send the key to: Windows gets it, and has to
                // get its Up too.
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
        // The rest of the life of a key that was down before the grab goes
        // to Windows, which saw it go down, and not to the remote, which did
        // not.
        // If the remote saw its Down too (a key sent from the window before
        // the hook was installed again), it gets the rest as well.
        let route = LEDGER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .route(raw.vk, raw.press);
        let forwards = self.policy.forwards(&raw);
        if route != Route::Remote || !forwards {
            // Not forwarded, but a held modifier still counts for a local
            // chord.
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
                return self.local_chord(&raw, &modifiers, trigger);
            }
        }
        // No session (or a closed channel) means the key goes to Windows
        // rather than being lost, and then Windows saw the Down.
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
    /// hook.
    fn windows_gets(&self, raw: &RawKey, now: HookMode) {
        if now != HookMode::Grab {
            return;
        }
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
        // The remote let go of every key; the replay below is Windows' alone.
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
        // The log says that a local chord was kept and which modifiers it
        // had, never the key that completed it.
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
