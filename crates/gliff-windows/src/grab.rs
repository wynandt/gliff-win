//! The UI thread's side of the keyboard grab:
//! it feeds window events to [`GrabState`], applies what comes out (install
//! or remove the hook, release held keys, hold the lock setting), and polls
//! the things Windows does not announce (the input desktop, a silent hook).
//! The decisions are in `grab_state`, `lock_policy` and `input`, tested on
//! the build machine; this file is the Win32 glue and is only compiled on
//! Windows.

use std::sync::Arc;
use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::RemoteDesktop::NOTIFY_FOR_THIS_SESSION;
use windows::Win32::System::RemoteDesktop::WTSRegisterSessionNotification;
use windows::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification;
use windows::Win32::System::StationsAndDesktops::CloseDesktop;
use windows::Win32::System::StationsAndDesktops::DESKTOP_CONTROL_FLAGS;
use windows::Win32::System::StationsAndDesktops::DESKTOP_SWITCHDESKTOP;
use windows::Win32::System::StationsAndDesktops::OpenInputDesktop;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::Ime::HIMC;
use windows::Win32::UI::Input::Ime::ImmAssociateContext;
use windows::Win32::UI::Input::KeyboardAndMouse::GetCapture;
use windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT;
use windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent;
use windows::Win32::UI::WindowsAndMessaging::GA_ROOT;
use windows::Win32::UI::WindowsAndMessaging::GA_ROOTOWNER;
use windows::Win32::UI::WindowsAndMessaging::GetAncestor;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::WindowFromPoint;

use crate::altgr::Origin;
use crate::altgr::RawKey;
use crate::chord_watch::SwitchChords;
use crate::foreground_watch::ForegroundWatch;
use crate::foreground_watch::Relation;
use crate::foreground_watch::classify;
use crate::foreground_watch::grabbing_by_focus;
use crate::grab_state::Desktop;
use crate::grab_state::Event;
use crate::grab_state::Focus;
use crate::grab_state::Follow;
use crate::grab_state::GrabState;
use crate::grab_state::Hit;
use crate::grab_state::HookMode;
use crate::grab_state::Output;
use crate::grab_state::Pointer;
use crate::grab_state::PointerWatch;
use crate::grab_state::Release;
use crate::grab_state::Session;
use crate::grab_state::Typing;
use crate::grab_state::pointer_over;
use crate::hook;
use crate::hook::HookThread;
use crate::input::KeyPolicy;
use crate::input::SuperL;
use crate::keymap::Chord;
use crate::keymap::Prefix;
use crate::keymap::Press;
use crate::local_chords::LocalChord;
use crate::lock_policy;
use crate::lock_policy::Change;
use crate::lock_policy::LockPolicy;
use crate::lock_policy::Owner;
use crate::lock_policy::Recovery;
use crate::lock_policy::Want;
use crate::lockreg;
use crate::lockreg::WinProcesses;
use crate::lockreg::WinRegistry;
use crate::modifiers::Sweep;
use crate::pass_log::SilentDetail;
use crate::pass_log::silent_hook_judge;
use crate::pass_log::silent_line;
use crate::route::InputRoute;
use crate::session::UiCmd;
use crate::test_hooks::TestHooks;

/// How often [`Grab::tick`] is called (the window's grab timer). The input
/// desktop is looked at every second call.
pub const TICK_MS: u32 = 125;

/// Ticks between two looks at the foreground window: about one second.
const FOREGROUND_TICKS: u32 = 8;

/// Ticks to wait after the session unlocks or the secure desktop closes
/// before the modifiers are looked at (a lock screen's last key Ups arrive
/// late).
const SWEEP_TICKS: u8 = 4;

/// What the command line decides about the grab.
#[derive(Clone, Debug)]
pub struct GrabConfig {
    pub follow: Follow,
    /// `--grab-timeout-secs`.
    pub timeout_secs: Option<u32>,
    pub policy: KeyPolicy,
    /// The release chord, which the hook also watches for.
    pub release_chord: Chord,
    /// The tab-switch chords, which the hook also watches for.
    pub switch_chords: SwitchChords,
    pub super_l: SuperL,
    /// Chords the hook keeps for Windows while grabbing.
    pub local_chords: Vec<LocalChord>,
    /// The hidden test flags (nothing in a build without `test-hooks`).
    pub test_hooks: TestHooks,
}

/// Counters for the stats file and the gate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GrabCounters {
    /// Times the hook was found silent and installed again.
    pub hook_reinstalls: u32,
    /// Keys that reached the window while the grab was on and were sent
    /// from there instead.
    pub legacy_keys: u32,
    /// Grabs ended by `--grab-timeout-secs`.
    pub grab_timeouts: u32,
    /// Modifier Ups injected into Windows when a grab ended (the release
    /// chord, the timeout, losing focus): keys it still counted as down.
    pub modifiers_released: u32,
    /// The same, when the client exited.
    pub modifiers_released_on_exit: u32,
}

pub struct Grab {
    hwnd: HWND,
    config: GrabConfig,
    state: GrabState,
    hook: Option<HookThread>,
    cmds: Arc<InputRoute<UiCmd>>,
    lock: Option<LockPolicy<WinRegistry>>,
    lock_owner: Option<Owner>,
    /// The helper that puts the lock setting back after a hard kill.
    watchdog: Option<std::process::Child>,
    /// The helper could not start; it is not tried again for modifiers.
    watchdog_refused: bool,
    tracking: bool,
    pointer_watch: PointerWatch,
    /// The once-a-second check that our window is still in front while the
    /// keyboard is grabbed by focus.
    foreground_watch: ForegroundWatch,
    /// What the last pointer poll said, to log only when it changes.
    last_poll: Option<PollKey>,
    /// A look at Windows' modifiers after the session or the secure desktop
    /// came back.
    sweep: Sweep,
    on_secure: bool,
    ime: Option<HIMC>,
    counters: GrabCounters,
    epoch: Instant,
    ticks: u32,
    wts: bool,
}

impl Grab {
    /// The startup recovery runs here, before anything can be held.
    pub fn new(hwnd: HWND, cmds: Arc<InputRoute<UiCmd>>, config: GrabConfig) -> Self {
        // One owner for the marker, the watchdog and the exit paths; none
        // when the start time is unreadable, and then there is no registry
        // route (Super+L goes by the chord). The repair of a dead client's
        // leftover runs in every mode.
        let owner = lockreg::owner();
        let (lock, recovery) = lock_policy::start_up(
            WinRegistry,
            config.super_l == SuperL::Registry,
            owner,
            &WinProcesses,
        );
        if recovery != Recovery::Nothing {
            crate::logging::line(&format!("lock setting at start: {recovery}"));
        }
        let owner = owner.filter(|_| lock.is_some());
        // SAFETY: `hwnd` is our window; the call only registers it.
        let wts = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) }.is_ok();
        Self {
            hwnd,
            state: GrabState::with_timeout(config.follow, config.timeout_secs),
            config,
            hook: None,
            cmds,
            lock,
            lock_owner: owner,
            watchdog: None,
            watchdog_refused: false,
            tracking: false,
            pointer_watch: PointerWatch::default(),
            foreground_watch: ForegroundWatch::default(),
            last_poll: None,
            sweep: Sweep::default(),
            on_secure: false,
            ime: None,
            counters: GrabCounters::default(),
            epoch: Instant::now(),
            ticks: 0,
            wts,
        }
    }

    pub fn counters(&self) -> GrabCounters {
        self.counters
    }

    /// Whether the Windows lock setting is in force now.
    pub fn lock_held(&self) -> bool {
        self.lock.as_ref().is_some_and(LockPolicy::is_effective)
    }

    pub fn mode(&self) -> HookMode {
        self.state.mode()
    }

    /// A focus message from Windows. It settles anything the foreground
    /// check had guessed.
    pub fn focus(&mut self, focus: Focus) {
        self.foreground_watch.focus_message();
        self.dispatch(Event::Focus(focus));
    }

    /// The address bar's edit field has the keyboard (or lost it): while it
    /// does, typing goes to Windows in both hook modes.
    pub fn typing(&mut self, typing: bool) {
        self.dispatch(Event::Typing(if typing {
            Typing::Started
        } else {
            Typing::Ended
        }));
    }

    pub fn click(&mut self) {
        self.dispatch(Event::Click);
    }

    /// The release chord completed (the hook has already given the keyboard
    /// back). Order matters: the state stops wanting a grab, then the hook
    /// may be asked for one again.
    pub fn release_chord(&mut self) {
        self.dispatch(Event::ReleaseChord);
        hook::release_handled();
    }

    pub fn session(&mut self, session: Session) {
        self.dispatch(Event::Session(session));
        if session == Session::Unlocked {
            self.sweep.arm(SWEEP_TICKS);
        }
    }

    /// The pointer moved inside the window: the first one after a leave asks
    /// Windows for `WM_MOUSELEAVE` and counts as entering.
    pub fn pointer_moved(&mut self) {
        self.pointer_watch.moved_by_message();
        if self.tracking {
            return;
        }
        let mut track = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: `track` is a valid TRACKMOUSEEVENT with its size set.
        if unsafe { TrackMouseEvent(&mut track) }.is_ok() {
            self.tracking = true;
            crate::logging::line("pointer: entered the window (mouse message)");
            self.dispatch(Event::Pointer(Pointer::Entered));
        }
    }

    /// Asks where the pointer is and, when the window messages disagree for
    /// two ticks, believes the answer: a pointer moved by another program is
    /// not always followed by `WM_MOUSELEAVE` or `WM_MOUSEMOVE` in time.
    fn check_pointer(&mut self) {
        let reading = read_pointer(self.hwnd);
        let key = reading.key();
        if self.last_poll.as_ref() != Some(&key) {
            // Only a change is logged: this is what settles a hover that
            // does not follow the pointer.
            crate::logging::line(&format!(
                "pointer poll: over={:?} under={} capture={} messages-say-over={}",
                reading.over,
                reading.under,
                reading.captured,
                self.state.hovered()
            ));
            self.last_poll = Some(key);
        }
        match self
            .pointer_watch
            .observe(self.state.hovered(), reading.over)
        {
            Some(Pointer::Left) => {
                crate::logging::line("pointer: left the window (found by the poll, no message)");
                self.leave();
            }
            Some(Pointer::Entered) => {
                // No mouse message came, so none is asked for either: the
                // poll also finds the leave. Asking Windows to track the
                // pointer from here made it answer with a leave at once
                // whenever the pointer was over a child window, and the poll
                // entered again on the next tick.
                crate::logging::line("pointer: entered the window (found by the poll, no message)");
                self.dispatch(Event::Pointer(Pointer::Entered));
            }
            None => {}
        }
    }

    /// `WM_MOUSELEAVE` came.
    pub fn pointer_left(&mut self) {
        self.pointer_watch.left_by_message();
        self.leave();
    }

    fn leave(&mut self) {
        crate::logging::line("pointer: left the window");
        self.tracking = false;
        self.dispatch(Event::Pointer(Pointer::Left));
    }

    /// A key message reached the window. While the grab is on that means the
    /// hook is not seeing keys (Windows removes a hook that answers too
    /// slowly), so the key is sent from here and the hook is installed
    /// again. `None`: not ours to handle.
    ///
    /// Windows saw the key go down, so the ledger notes it
    /// ([`crate::key_ledger`]): the key's Up goes to Windows too, through the
    /// new hook.
    pub fn legacy_key(
        &mut self,
        vk: u16,
        lparam: isize,
        press: Press,
        time_ms: u32,
        extra_info: bool,
    ) -> Option<()> {
        // A key the hook let through on purpose (a media key, a key under
        // `--injected-keys local`, a key with no session to send it to) is
        // Windows' to handle: do not send it, do not count it, and let
        // `DefWindowProc` see it. So is any key while the hook itself is not
        // grabbing (the release chord just fired).
        let was_passed = crate::hook::was_passed(vk, press, time_ms);
        if !silent_hook_judge(self.state.mode(), crate::hook::mode(), was_passed) {
            return None;
        }
        // A message the hook has already overtaken is dropped: it handled a
        // later event of the key (its Up, say) after the window installed it
        // again, and that event decided where the key's life goes. It is a
        // key of the grab, so `DefWindowProc` does not see it either (a late
        // Alt+F4 would close the window).
        if crate::hook::saw_later(crate::key_ledger::KeyId::of_message(vk, lparam), time_ms) {
            return Some(());
        }
        self.counters.legacy_keys += 1;
        let raw = RawKey {
            vk,
            scan: ((lparam >> 16) & 0xff) as u16,
            prefix: if (lparam >> 24) & 1 == 1 {
                Prefix::E0
            } else {
                Prefix::None
            },
            press,
            time_ms,
            origin: Origin::LegacyMessage,
            injected: false,
        };
        let sent = self.config.policy.forwards(&raw) && self.cmds.send(UiCmd::Key(raw));
        crate::hook::note_legacy(vk, press, sent);
        if self.counters.legacy_keys == 1 || self.counters.legacy_keys.is_multiple_of(100) {
            // How many keys are down and which modifier groups, and how the
            // hook has been running: no key is named.
            crate::logging::line(&silent_line(
                self.counters.legacy_keys,
                crate::hook::held_now(),
                SilentDetail {
                    extra_info,
                    hook: crate::hook::timing(),
                    noted_near: crate::hook::noted_near(vk, time_ms),
                },
            ));
        }
        self.hook = None;
        self.counters.hook_reinstalls += 1;
        self.install();
        Some(())
    }

    /// Called every [`TICK_MS`]: the lock setting's delayed clear, and every
    /// other call a look at the input desktop.
    pub fn tick(&mut self) {
        let now = self.now_ms();
        let out = self.state.handle(Event::Tick(now));
        if out.timed_out {
            self.counters.grab_timeouts += 1;
            crate::logging::line(&format!(
                "grab released: --grab-timeout-secs {} ran out; click the window (focus mode) \
                 or move the pointer out and in (hover mode) to grab again",
                self.config.timeout_secs.unwrap_or(0)
            ));
            self.apply(out);
        }
        if let Some(lock) = &mut self.lock
            && lock.clear_pending()
        {
            let change = lock.tick(now);
            log_change(&change);
        }
        if hook::release_overdue() {
            // The release chord went unanswered: the session it went to has
            // ended or is not the shown one. The state lets go of the grab
            // here, or the hook would refuse every later one.
            crate::logging::line("release chord: no session answered; the window took it in");
            self.release_chord();
        }
        self.check_pointer();
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(FOREGROUND_TICKS) {
            self.check_foreground();
        }
        if self.ticks.is_multiple_of(2) {
            let desktop = if input_desktop_is_ours() {
                Desktop::Normal
            } else {
                Desktop::Secure
            };
            let secure = desktop == Desktop::Secure;
            if self.on_secure && !secure {
                self.sweep.arm(SWEEP_TICKS);
            }
            self.on_secure = secure;
            self.dispatch(Event::Desktop(desktop));
        }
        if self.sweep.tick() {
            let released = crate::hook::release_at_grab_end();
            self.counters.modifiers_released += released.len() as u32;
            if !released.is_empty() {
                crate::logging::line(&format!(
                    "back from the lock screen or a secure prompt: {} modifier key(s) \
                     Windows still counted as down were released ({})",
                    released.len(),
                    crate::modifiers::names(&released)
                ));
            }
        }
    }

    /// While the keyboard is grabbed by focus, about once a second: is the
    /// window in front still ours? Windows does not always send
    /// `WM_KILLFOCUS` when a game takes the foreground, and without it the
    /// grab would last until something else ended it. Two looks in a row at
    /// another program's window are a lost focus, by the usual path (so held
    /// keys are released on both machines); our own window in front again is
    /// a gained one. Outside a focus grab this does nothing and asks Windows
    /// nothing.
    fn check_foreground(&mut self) {
        let grabbing = grabbing_by_focus(self.config.follow, self.state.mode());
        if !self.foreground_watch.active(grabbing) {
            return;
        }
        let seen = classify(read_relation(self.hwnd));
        let Some(focus) = self.foreground_watch.look(grabbing, seen) else {
            return;
        };
        crate::logging::line(match focus {
            Focus::Lost => {
                "foreground check: another program's window has been in front for two looks \
                 in a row while the keyboard was grabbed, and Windows sent no focus-lost \
                 message; treating the focus as lost"
            }
            Focus::Gained => {
                "foreground check: this window is in front again with no focus message; \
                 treating the focus as gained"
            }
        });
        self.dispatch(Event::Focus(focus));
    }

    /// Every exit path of the window: nothing is held afterwards.
    pub fn shutdown(&mut self) {
        hook::set_mode(HookMode::Off);
        self.hook = None;
        self.restore_ime();
        if let Some(lock) = &mut self.lock {
            log_change(&lock.exit());
        }
        // The remote side was let go of by the session (Bye); Windows' own
        // modifier state is cleaned here, with the hook gone.
        let released = crate::modifiers::release_held();
        self.counters.modifiers_released_on_exit += released.len() as u32;
        if !released.is_empty() {
            crate::logging::line(&format!(
                "exit: {} modifier key(s) Windows still counted as down were released ({})",
                released.len(),
                crate::modifiers::names(&released)
            ));
        }
        if self.wts {
            // SAFETY: undoes the registration made in `new`.
            let _ = unsafe { WTSUnRegisterSessionNotification(self.hwnd) };
            self.wts = false;
        }
    }

    /// True while the watchdog helper runs. One that has ended (it was
    /// killed) is let go of, so the next wish for the setting starts another.
    fn watchdog_alive(&mut self) -> bool {
        let Some(child) = &mut self.watchdog else {
            return false;
        };
        if matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        crate::logging::line("lock watchdog: the helper is gone");
        self.watchdog = None;
        false
    }

    /// Starts the watchdog helper unless one runs.
    fn ensure_watchdog(&mut self) -> Result<(), String> {
        if self.watchdog_alive() {
            return Ok(());
        }
        let owner = self
            .lock_owner
            .or_else(lockreg::current_owner)
            .ok_or_else(|| "this process's start time is unreadable".to_owned())?;
        self.watchdog = Some(lockreg::spawn_watchdog(owner).map_err(|e| e.to_string())?);
        Ok(())
    }

    fn dispatch(&mut self, event: Event) {
        let before = self.state.mode();
        let out = self.state.handle(event);
        if out.mode != before {
            crate::logging::line(&format!(
                "hook mode {before:?} -> {:?} after {event:?} ({})",
                out.mode,
                self.state.describe()
            ));
        }
        if let Some(secs) = self.config.timeout_secs
            && out.mode == HookMode::Grab
            && before != HookMode::Grab
        {
            crate::logging::line(&format!(
                "grab started: it ends by itself in {secs} s (--grab-timeout-secs); \
                 the release chord still works"
            ));
        }
        self.apply(out);
    }

    fn apply(&mut self, out: Output) {
        match out.mode {
            HookMode::Off => {
                hook::set_mode(HookMode::Off);
                self.hook = None;
            }
            mode => {
                if self.hook.is_none() {
                    self.install();
                }
                hook::set_mode(mode);
            }
        }
        if out.mode == HookMode::Grab {
            self.ime_off();
        } else {
            self.restore_ime();
        }
        if out.release == Release::All {
            self.cmds.send(UiCmd::ReleaseInput);
            // The hook is no longer swallowing (or is Super+L only): clean
            // Windows' side too, but not a modifier the user is still holding.
            let released = crate::hook::release_at_grab_end();
            self.counters.modifiers_released += released.len() as u32;
            if !released.is_empty() {
                crate::logging::line(&format!(
                    "grab ended: {} modifier key(s) Windows still counted as down were \
                     released ({})",
                    released.len(),
                    crate::modifiers::names(&released)
                ));
            }
        }
        if out.mode == HookMode::Grab && !self.watchdog_refused && !self.watchdog_alive() {
            // The helper that cleans the modifiers after a hard kill (and
            // restores the lock setting, in registry mode).
            if let Err(e) = self.ensure_watchdog() {
                self.watchdog_refused = true;
                crate::logging::line(&format!(
                    "watchdog did not start, so a hard kill leaves modifiers to Windows: {e}"
                ));
            }
        }
        let now = self.now_ms();
        // The helper that puts the setting back after a kill is started
        // before the setting is first turned on; if it cannot start the
        // setting stays off (and the next wish tries again), so a killed
        // client never leaves it set without a helper.
        if out.lock == Want::On
            && self.lock.as_ref().is_some_and(|l| !l.is_effective())
            && self.lock_owner.is_some()
            && let Err(e) = self.ensure_watchdog()
        {
            crate::logging::line(&format!(
                "lock watchdog did not start, so the lock setting stays off: {e}"
            ));
            return;
        }
        if let Some(lock) = &mut self.lock {
            log_change(&lock.want(out.lock, now));
        }
    }

    fn install(&mut self) {
        match HookThread::start(
            self.cmds.clone(),
            self.config.policy,
            self.config.release_chord.clone(),
            self.config.switch_chords.clone(),
            self.config.test_hooks.clone(),
            self.config.local_chords.clone(),
        ) {
            Ok(hook) => self.hook = Some(hook),
            Err(reason) => crate::logging::line(&format!("keyboard hook: {reason}")),
        }
    }

    fn ime_off(&mut self) {
        if self.ime.is_none() {
            // SAFETY: our window; a null context turns the IME off for it
            // and returns the one it had.
            let old = unsafe { ImmAssociateContext(self.hwnd, HIMC::default()) };
            self.ime = Some(old);
        }
    }

    fn restore_ime(&mut self) {
        if let Some(old) = self.ime.take() {
            // SAFETY: `old` is the context this window had before
            // `ime_off`.
            unsafe { ImmAssociateContext(self.hwnd, old) };
        }
    }

    fn now_ms(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }
}

fn log_change(change: &Change) {
    match change {
        Change::Nothing => {}
        other => crate::logging::line(&format!("lock setting: {other:?}")),
    }
}

/// False when Windows is showing the secure desktop (Ctrl+Alt+Del, UAC, the
/// lock screen): a program on the normal desktop is refused access to it.
fn input_desktop_is_ours() -> bool {
    // SAFETY: no pointers; the handle, when returned, is closed below.
    let desktop =
        unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) };
    match desktop {
        Ok(desktop) => {
            // SAFETY: `desktop` came from OpenInputDesktop and is closed once.
            let _ = unsafe { CloseDesktop(desktop) };
            true
        }
        Err(_) => false,
    }
}

/// What one look at the pointer found.
struct PointerReading {
    /// Over the client area with no other window in front; `None` when it
    /// cannot be told (the window holds the mouse, or no cursor position).
    over: Option<bool>,
    /// Whose window is under the pointer. Never the other window's class or
    /// the pointer's position: the log is not a trail of what the user does
    /// in other programs.
    under: &'static str,
    captured: bool,
}

/// The parts of a reading that, when they change, are logged.
#[derive(Debug, PartialEq, Eq)]
struct PollKey {
    over: Option<bool>,
    under: &'static str,
    captured: bool,
}

impl PointerReading {
    fn key(&self) -> PollKey {
        PollKey {
            over: self.over,
            under: self.under,
            captured: self.captured,
        }
    }
}

/// Whether the pointer is over the window's client area right now, with no
/// other window in front of it, and what it found on the way.
/// How the window in front relates to `hwnd`, for the foreground check.
fn read_relation(hwnd: HWND) -> Relation {
    // SAFETY: no arguments.
    let front = unsafe { GetForegroundWindow() };
    if front.0.is_null() {
        return Relation::None;
    }
    if front == hwnd {
        return Relation::Is;
    }
    // SAFETY: a window handle; the answers are handles, compared only.
    let (root, owner) = unsafe {
        (
            GetAncestor(front, GA_ROOT),
            GetAncestor(front, GA_ROOTOWNER),
        )
    };
    if root == hwnd || owner == hwnd {
        return Relation::OwnedOrChild;
    }
    let mut pid = 0u32;
    // SAFETY: a live u32 to write the process id into.
    let thread = unsafe { GetWindowThreadProcessId(front, Some(&mut pid)) };
    if thread == 0 || pid == 0 {
        return Relation::Unreadable;
    }
    // SAFETY: no arguments.
    if pid == unsafe { GetCurrentProcessId() } {
        Relation::SameProcess
    } else {
        Relation::Foreign
    }
}

fn read_pointer(hwnd: HWND) -> PointerReading {
    let mut out = PointerReading {
        over: None,
        under: "unknown",
        captured: false,
    };
    // A drag that started in the window keeps the mouse (`SetCapture`) and
    // the messages, wherever the pointer goes: the messages are right then.
    // SAFETY: no arguments.
    out.captured = unsafe { GetCapture() } == hwnd;
    let mut at = POINT::default();
    // SAFETY: a live POINT.
    if unsafe { GetCursorPos(&mut at) }.is_err() {
        return out;
    }
    // SAFETY: plain coordinates.
    let under = unsafe { WindowFromPoint(at) };
    // SAFETY: a window handle (null is fine) and the window's own.
    let top = unsafe { GetAncestor(under, GA_ROOT) };
    let hit = if under == hwnd {
        Hit::Window
    } else if top == hwnd {
        Hit::OwnChild
    } else {
        Hit::Other
    };
    out.under = match hit {
        Hit::Window => "this window",
        Hit::OwnChild => "a child of this window",
        Hit::Other => "another window",
    };
    if out.captured {
        return out;
    }
    if hit != Hit::Window {
        out.over = pointer_over(hit, false);
        return out;
    }
    let mut client = RECT::default();
    // SAFETY: our window and a live RECT.
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return out;
    }
    // SAFETY: our window and a live POINT.
    if !unsafe { ScreenToClient(hwnd, &mut at) }.as_bool() {
        return out;
    }
    out.over = pointer_over(
        hit,
        at.x >= client.left && at.x < client.right && at.y >= client.top && at.y < client.bottom,
    );
    out
}
