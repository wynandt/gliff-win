//! `App` and the window procedure: one method per message arm.
//!
//! The state lives in a thread-local `RefCell`. Win32 re-enters the
//! window procedure from inside calls the arms make (`SetWindowPos` sends
//! `WM_SIZE`, `DestroyWindow` sends `WM_DESTROY`); such a nested message
//! finds the state borrowed, is queued in `DEFERRED` and handled right
//! after the arm returns (a `WM_ENDSESSION` also restores the lock setting
//! on the spot). Every unhandled message reaches
//! `DefWindowProcW` with no borrow held, so its modal loops (move, size,
//! the window menu) can dispatch to us.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::BeginPaint;
use windows::Win32::Graphics::Gdi::DEVMODEW;
use windows::Win32::Graphics::Gdi::ENUM_CURRENT_SETTINGS;
use windows::Win32::Graphics::Gdi::EndPaint;
use windows::Win32::Graphics::Gdi::EnumDisplaySettingsW;
use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows::Win32::Graphics::Gdi::MONITORINFO;
use windows::Win32::Graphics::Gdi::MONITORINFOEXW;
use windows::Win32::Graphics::Gdi::MonitorFromWindow;
use windows::Win32::Graphics::Gdi::PAINTSTRUCT;
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_F11;
use windows::Win32::UI::WindowsAndMessaging::AppendMenuW;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
use windows::Win32::UI::WindowsAndMessaging::GA_ROOT;
use windows::Win32::UI::WindowsAndMessaging::GetAncestor;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::GetSystemMenu;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::Win32::UI::WindowsAndMessaging::HTCLIENT;
use windows::Win32::UI::WindowsAndMessaging::IsChild;
use windows::Win32::UI::WindowsAndMessaging::IsIconic;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::Win32::UI::WindowsAndMessaging::KillTimer;
use windows::Win32::UI::WindowsAndMessaging::MF_SEPARATOR;
use windows::Win32::UI::WindowsAndMessaging::MF_STRING;
use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use windows::Win32::UI::WindowsAndMessaging::SW_RESTORE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::SetTimer;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::SetWindowTextW;
use windows::Win32::UI::WindowsAndMessaging::ShowWindow;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::Win32::UI::WindowsAndMessaging::WM_COPYDATA;
use windows::Win32::UI::WindowsAndMessaging::WM_DESTROY;
use windows::Win32::UI::WindowsAndMessaging::WM_DPICHANGED;
use windows::Win32::UI::WindowsAndMessaging::WM_ENDSESSION;
use windows::Win32::UI::WindowsAndMessaging::WM_ENTERMENULOOP;
use windows::Win32::UI::WindowsAndMessaging::WM_ENTERSIZEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND;
use windows::Win32::UI::WindowsAndMessaging::WM_EXITMENULOOP;
use windows::Win32::UI::WindowsAndMessaging::WM_EXITSIZEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_PAINT;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_SETCURSOR;
use windows::Win32::UI::WindowsAndMessaging::WM_SIZE;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSCOMMAND;
use windows::Win32::UI::WindowsAndMessaging::WM_TIMER;
use windows::Win32::UI::WindowsAndMessaging::WM_XBUTTONDOWN;
use windows::core::HSTRING;
use windows::core::PCWSTR;
use windows::core::w;

use crate::change::Last;
use crate::chord_watch::switch_of_word;
use crate::cli::AdapterChoice;
use crate::config::ConfigFile;
use crate::connect::Button;
use crate::cursor::RemoteCursor;
use crate::cursor_image::Desktop;
use crate::failure_panel;
use crate::failure_panel::Action as PanelAction;
use crate::fullscreen::Fullscreen;
use crate::history::HistoryFile;
use crate::history::HostTarget;
use crate::layout::Filter;
use crate::layout::Viewport;
use crate::layout::layout;
use crate::machine_choices::Rules;
use crate::panel_win::PanelWin;
use crate::present::Presenter;
use crate::present_log::PresentLog;
use crate::present_log::PresentOutcome;
use crate::resize_gate::Moving;
use crate::resize_gate::ResizeGate;
use crate::resize_gate::Settled;
use crate::resize_gate::request_for;
use crate::session::Geometry;
use crate::session::Picture;
use crate::session::ResizeReq;
use crate::session::SessionStats;
use crate::session::UiCmd;
use crate::session::UiEvent;
use crate::session::driver::SessionEnd;
use crate::session::driver::SessionHandle;
use crate::session_set::Applied;
use crate::session_set::Presented;
use crate::session_set::SessionSet;
use crate::single_instance::Forward;
use crate::single_instance::win::Received;
use crate::stages::Screen;
use crate::stages::Stages;
use crate::stats_out::ChordsInfo;
use crate::stats_out::ClientStats;
use crate::stats_out::MonitorInfo;
use crate::stats_out::ResizeLog;
use crate::stats_out::SessionCounters;
use crate::stats_out::WindowInfo;
use crate::stats_out::end_name;
use crate::stats_out::session_entries;
use crate::strip::address::Act as AddressAct;
use crate::strip::address::Event as AddressEvent;
use crate::strip::dot_of;
use crate::strip::menu;
use crate::strip::menu::Command as MenuCommand;
use crate::strip::menu::Entry as MenuEntry;
use crate::strip::menu::ListRow;
use crate::strip::menu::RecentChoice;
use crate::strip::menu::RecentRow;
use crate::strip::menu::recent_menu;
use crate::strip::reveal::Pointer;
use crate::strip::ui::Action as StripAction;
use crate::strip::ui::Input as StripInput;
use crate::strip_win::Later;
use crate::strip_win::Popup;
use crate::strip_win::StripWin;
use crate::tabs::Effect;
use crate::tabs::SessionId;
use crate::tabs::Tabs;
use crate::waker::PostWaker;
use crate::waker::WM_APP_EVENT;
use crate::waker::WM_APP_FRAME;
use crate::waker::WM_APP_SWITCH;
use crate::window_input::View;
use crate::window_input::WindowInput;

/// The window class; `gwin-verify present` finds the window by it.
pub const CLASS_NAME: PCWSTR = w!("gliff-windows");
/// [`CLASS_NAME`] as text, for a search by class.
pub const CLASS_NAME_TEXT: &str = "gliff-windows";
/// The window menu's "Fullscreen" item (system command ids are multiples
/// of 16 below 0xF000).
pub const IDM_FULLSCREEN: usize = 0x0010;

const TIMER_SETTLE: usize = 1;
/// Presents while a drag's or a menu's modal loop runs in place of the
/// main loop.
const TIMER_MODAL: usize = 2;
const TIMER_CLOSE: usize = 3;
const TIMER_STATS: usize = 4;
const TIMER_GRAB: usize = 5;
/// The next tab retry (`Tabs::next_due`); re-armed after every event.
const TIMER_RETRY: usize = 6;
/// The modal timer: about one 60 Hz frame.
const MODAL_TICK: Duration = Duration::from_millis(16);
/// How long a closing window waits for the session's Bye.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// How often the stats file is rewritten.
pub const STATS_EVERY: Duration = Duration::from_secs(1);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static DEFERRED: RefCell<Vec<Deferred>> = const { RefCell::new(Vec::new()) };
}

/// A message that arrived while an arm held the state.
#[derive(Clone, Copy)]
enum Deferred {
    Size,
    Destroy,
    /// `WM_ENDSESSION` (the session is ending): the rest of the shutdown,
    /// after the lock setting was already put back at once.
    EndSession,
}

/// What the starter records for each session it starts, for its view.
pub struct Launched {
    /// The request the session starts with, logged at `Connected`.
    pub initial: ResizeReq,
    pub waker: Arc<PostWaker>,
    /// The connection choices the session started with, so the tab menu can
    /// tell when the saved choice has changed since.
    pub running: crate::machine_choices::Running,
}

/// The starter's notes for the window: filled when a session starts, emptied
/// when the window makes the view.
pub type LaunchBook = Rc<RefCell<BTreeMap<SessionId, Launched>>>;

/// The two files the window keeps up to date.
pub struct Files {
    pub config: ConfigFile,
    pub history: HistoryFile,
}

/// What the window keeps for one session: everything that must not leak from
/// one session into the next.
struct SessionView {
    waker: Arc<PostWaker>,
    gate: ResizeGate,
    cursor: RemoteCursor,
    /// The request the session started with, logged at `Connected`.
    initial: ResizeReq,
    /// The last StreamConfig's fps cap, for the stats file.
    fps_cap: u32,
    /// The view of the last picture shown, for redraws.
    shown_view: Option<(u32, u32)>,
    /// The geometry of the last picture shown, for the pointer.
    shown_geometry: Option<Geometry>,
    /// The picture shown last, kept to show again when the user comes back
    /// to this session and no newer one is waiting.
    last: Option<Picture>,
    /// `last` must be presented again (the view was just switched to).
    repeat: bool,
    /// The session is re-syncing its display; the title says so while this
    /// session is the shown one.
    resyncing: bool,
    /// The connection choices this session runs with: the clipboard one
    /// follows the menu while the session runs.
    running: crate::machine_choices::Running,
    /// The session started with its clipboard engine, so sharing can be
    /// turned off and on again in place.
    started_sharing: bool,
}

impl SessionView {
    fn new(launched: Launched) -> Self {
        Self {
            waker: launched.waker,
            gate: ResizeGate::new(launched.initial),
            cursor: RemoteCursor::new(),
            initial: launched.initial,
            fps_cap: 0,
            shown_view: None,
            shown_geometry: None,
            last: None,
            repeat: false,
            resyncing: false,
            running: launched.running,
            started_sharing: launched.running.share_clipboard,
        }
    }
}

/// Which of Windows' modal loops runs in place of the main loop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModalKind {
    /// A drag or resize of the window (`WM_ENTERSIZEMOVE`).
    SizeMove,
    /// A popup or window menu (`WM_ENTERMENULOOP`).
    Menu,
}

/// A modal loop under way: which, when it began, and the pictures shown
/// since.
struct Modal {
    kind: ModalKind,
    began: Instant,
    shown: u32,
}

/// Everything the UI thread owns.
pub struct App {
    hwnd: HWND,
    /// The monitor and window rect at the last live stats write: the
    /// final file is written after `DestroyWindow`, when the window can
    /// no longer be asked.
    screen: RefCell<(MonitorInfo, (i32, i32, u32, u32))>,
    adapter_choice: AdapterChoice,
    presenter: Option<Presenter>,
    /// The tabs and what each wants.
    tabs: Tabs,
    /// The running sessions.
    set: SessionSet<SessionHandle>,
    launched: LaunchBook,
    files: Files,
    rules: Rc<Rules>,
    views: BTreeMap<SessionId, SessionView>,
    /// The session whose picture the window shows.
    shown: Option<SessionId>,
    /// Sessions that are over, with how they ended.
    finals: Vec<(SessionId, SessionEnd)>,
    /// Sessions that never started.
    start_failures: u32,
    stages: Stages,
    log: PresentLog,
    resizes: ResizeLog,
    fullscreen: Fullscreen,
    /// The tab strip above the picture.
    pub(crate) strip: StripWin,
    /// The failure panel over the picture of a tab that gave up.
    pub(crate) panel: PanelWin,
    /// The tab whose panel Close hid, until it is shown again.
    panel_dismissed: Option<crate::tabs::TabId>,
    /// Where the log and ssh's logs are, for Details.
    log_dir: PathBuf,
    started: Instant,
    /// The client size last seen, to tell a change from a repeat.
    client: (u32, u32),
    dpi: u32,
    input: WindowInput,
    chords: ChordsInfo,
    /// The waitable was signalled and not yet used by a `Present`.
    ready: bool,
    /// One of Windows' modal loops runs in place of the main loop.
    modal: Option<Modal>,
    /// `WM_CLOSE` sent Bye to every session; the window goes when they are
    /// over or the close timer fires.
    closing: bool,
    title: String,
    /// What the strip last told the grab about typing, and when the retry
    /// timer was last set for: the window works through both on every pass
    /// of the message loop, and acts only on a change.
    typing_sent: Last<bool>,
    retry_armed: Last<Option<Instant>>,
    stats_out: Option<PathBuf>,
}

/// What `App::new` needs.
pub struct AppParts {
    pub hwnd: HWND,
    pub adapter_choice: AdapterChoice,
    pub presenter: Presenter,
    pub tabs: Tabs,
    pub set: SessionSet<SessionHandle>,
    pub launched: LaunchBook,
    pub files: Files,
    /// What each machine runs with; the tab menu shows it.
    pub rules: Rc<Rules>,
    pub chords: ChordsInfo,
    pub stats_out: Option<PathBuf>,
    pub started: Instant,
    pub input: WindowInput,
    pub strip: StripWin,
    pub panel: PanelWin,
    pub log_dir: PathBuf,
}

impl App {
    pub fn new(parts: AppParts) -> Self {
        let hwnd = parts.hwnd;
        Self {
            hwnd,
            adapter_choice: parts.adapter_choice,
            presenter: Some(parts.presenter),
            tabs: parts.tabs,
            set: parts.set,
            launched: parts.launched,
            files: parts.files,
            rules: parts.rules,
            views: BTreeMap::new(),
            shown: None,
            finals: Vec::new(),
            start_failures: 0,
            stages: Stages::new(),
            log: PresentLog::new(parts.started),
            resizes: ResizeLog::default(),
            fullscreen: Fullscreen::default(),
            strip: parts.strip,
            panel: parts.panel,
            panel_dismissed: None,
            log_dir: parts.log_dir,
            started: parts.started,
            client: picture_size(hwnd),
            dpi: window_dpi(hwnd),
            input: parts.input,
            chords: parts.chords,
            ready: false,
            modal: None,
            closing: false,
            title: "gliff".to_owned(),
            typing_sent: Last::new(),
            retry_armed: Last::new(),
            stats_out: parts.stats_out,
            screen: RefCell::new((MonitorInfo::default(), (0, 0, 0, 0))),
        }
    }

    /// Runs the start-up effects (the sessions of the command line and of the
    /// last run) once the state is installed.
    pub fn begin(&mut self, effects: Vec<Effect>) {
        self.apply(effects);
        self.sync_strip();
    }

    fn ms(&self, now: Instant) -> u64 {
        elapsed_ms(self.started, now)
    }

    /// The handle to wait on with messages, while a present is wanted.
    pub fn wait_handle(&self) -> Option<windows::Win32::Foundation::HANDLE> {
        match (&self.presenter, self.ready) {
            (Some(p), false) => Some(p.waitable()),
            _ => None,
        }
    }

    /// The main loop's wait saw the waitable.
    pub fn on_waitable(&mut self) {
        self.ready = true;
    }

    /// Events, then a picture if one is due: after every wake of the main
    /// loop and on the size-move timer.
    pub fn pump(&mut self) {
        self.drain_events();
        self.present_due();
    }

    fn update_stage(&mut self, now: Instant) {
        self.log.stage(self.stages.current(now), now);
    }

    /// Runs effects and then what they asked of the window.
    fn apply(&mut self, effects: Vec<Effect>) {
        let applied = self.set.run(&mut self.tabs, effects);
        self.after(applied);
    }

    /// Takes every session's queued events and lets the model answer.
    fn drain_events(&mut self) {
        for view in self.views.values() {
            view.waker.take_event();
        }
        let applied = self.set.poll(&mut self.tabs);
        self.after(applied);
        if self.closing && self.set.running() == 0 {
            self.destroy();
        }
    }

    /// What the window does with what the runner returned: make the views of
    /// new sessions, hand each event to its view, keep the files, show the
    /// session the model chose, and note the sessions that are over.
    fn after(&mut self, applied: Applied) {
        self.adopt_launched();
        self.start_failures += u32::try_from(applied.failed.len()).unwrap_or(u32::MAX);
        for (session, event) in applied.events {
            let connected_here =
                matches!(event, UiEvent::Connected(_)) && self.shown == Some(session);
            self.on_event(session, event);
            if connected_here {
                // The tab on show is up: the bar gives the keyboard back to
                // the picture, so the grab goes on without an Enter or Esc.
                self.address_event(AddressEvent::Connected);
            }
        }
        if applied.persist {
            self.persist();
        }
        for host in applied.recorded {
            self.record(host);
        }
        match applied.presented {
            Some(Presented::Session(session)) => self.show(Some(session)),
            Some(Presented::Nothing) => self.show(None),
            None => {}
        }
        for (session, end) in applied.ended {
            self.on_session_over(session, end);
        }
        self.refresh_title();
        self.arm_retry_timer();
        self.sync_strip();
    }

    /// One timer for the earliest waiting retry, set again after every event
    /// so a retry that an event moved is never missed. The timer fires a
    /// poll, which starts what is due. A closing window starts no retry, so
    /// it sets none.
    fn arm_retry_timer(&mut self) {
        let due = self.tabs.next_due().filter(|_| !self.closing);
        if !self.retry_armed.update(due) {
            return;
        }
        match due {
            Some(at) => self.set_timer(TIMER_RETRY, at.saturating_duration_since(Instant::now())),
            None => self.kill_timer(TIMER_RETRY),
        }
    }

    /// Makes the view of every session the starter started since.
    fn adopt_launched(&mut self) {
        let launched = std::mem::take(&mut *self.launched.borrow_mut());
        for (session, launched) in launched {
            self.views.insert(session, SessionView::new(launched));
        }
    }

    fn on_event(&mut self, session: SessionId, event: UiEvent) {
        let now = Instant::now();
        let ms = self.ms(now);
        let shown = self.shown == Some(session);
        let Some(view) = self.views.get_mut(&session) else {
            return;
        };
        match event {
            UiEvent::Connected(info) => {
                crate::logging::line(&format!(
                    "session {}: connected: output {:?}, headless {}",
                    session.get(),
                    info.output,
                    info.headless
                ));
                view.cursor.connected(Desktop::from_headless(info.headless));
                if shown {
                    let initial = view.initial;
                    self.resizes.sent(initial, ms);
                }
            }
            UiEvent::StreamConfig(config) => {
                let g = config.geometry;
                crate::logging::line(&format!(
                    "session {}: stream config: view {}x{}, stream {}x{}, scale {}, cap {} fps, {:?}, encoder {}",
                    session.get(),
                    g.view.0,
                    g.view.1,
                    g.stream.0,
                    g.stream.1,
                    g.scale_milli,
                    config.fps_cap,
                    config.chroma,
                    config.pipeline.map_or("unstated", |p| p.label())
                ));
                view.fps_cap = config.fps_cap;
                view.gate.on_view(g.view);
                if shown {
                    self.resizes.on_config(g.view, g.scale_milli, ms);
                }
            }
            UiEvent::Cursor(image) => view.cursor.shape(&image),
            UiEvent::Resyncing => {
                crate::logging::line(&format!("session {}: resync: started", session.get()));
                view.resyncing = true;
            }
            UiEvent::Resynced => {
                crate::logging::line(&format!(
                    "session {}: resync: pictures are flowing again",
                    session.get()
                ));
                view.resyncing = false;
            }
            UiEvent::GrabReleased => {
                if shown {
                    self.input.grab_released();
                }
            }
            // The runner gives an end to the model, which answers with the
            // tab's new state.
            UiEvent::Ended(_) => {}
        }
    }

    /// A session is over and joined: keep how it ended, drop its view.
    fn on_session_over(&mut self, session: SessionId, end: SessionEnd) {
        for error in &end.errors {
            crate::logging::line(&format!("session {}: error: {error}", session.get()));
        }
        crate::logging::line(&format!(
            "session {}: over: {:?}, ssh exit {:?}",
            session.get(),
            end.reason,
            end.ssh_exit
        ));
        self.views.remove(&session);
        if self.shown == Some(session) {
            self.shown = None;
        }
        self.finals.push((session, end));
    }

    /// The window shows `session` (or nothing): point the input at it, catch
    /// its size up with the window, and draw its picture.
    fn show(&mut self, session: Option<SessionId>) {
        let changed = self.shown != session;
        self.shown = session;
        if changed {
            self.input.switched();
        }
        let mut has_last = false;
        if let Some(view) = session.and_then(|id| self.views.get_mut(&id)) {
            view.waker.take_frame();
            view.repeat = view.last.is_some();
            has_last = view.repeat;
            let now = Instant::now();
            if let Some(delay) = view.gate.on_size(self.client, self.dpi, Moving::Free, now) {
                self.set_timer(TIMER_SETTLE, delay);
            }
        }
        let drawn = self.present_due();
        if !drawn && !has_last {
            // Nothing to show of this session yet: black, not the last
            // session's picture.
            self.redraw();
        }
    }

    /// The title says why the shown tab has no session, or that the shown
    /// session is re-syncing its display.
    fn refresh_title(&mut self) {
        let status = self
            .tabs
            .shown()
            .and_then(|tab| self.tabs.tab(tab))
            .map(|tab| tab.status().clone());
        let resyncing = self
            .shown
            .and_then(|id| self.views.get(&id))
            .is_some_and(|view| view.resyncing);
        let text = crate::tabs::window_title(status.as_ref(), resyncing);
        if text != self.title {
            let title = HSTRING::from(text.as_str());
            // SAFETY: our window; the string outlives the call.
            let _ = unsafe { SetWindowTextW(self.hwnd, &title) };
            self.title = text;
        }
    }

    /// The tab set, saved. Errors are logged, never fatal.
    fn persist(&mut self) {
        *self.files.config.tabs_mut() = self.tabs.tab_set();
        if let Err(e) = self.files.config.save() {
            crate::logging::line(&format!("config: tabs not saved: {e}"));
        }
    }

    fn record(&mut self, host: HostTarget) {
        if let Err(e) = self.files.history.record(host) {
            crate::logging::line(&format!("recent hosts not saved: {e}"));
        }
    }

    /// A switch chord completed on the hook thread.
    fn on_switch(&mut self, word: usize) {
        if self.closing {
            return;
        }
        if let Some(to) = switch_of_word(word) {
            let effects = self.tabs.switch(to);
            self.apply(effects);
        }
    }

    /// A later launch handed us its request.
    fn on_copy_data(&mut self, lparam: LPARAM) -> Option<LRESULT> {
        // SAFETY: lparam is the one of the WM_COPYDATA being handled.
        match unsafe { crate::single_instance::win::receive(lparam) } {
            Received::Foreign => None,
            Received::Refused(e) => {
                crate::logging::line(&format!("a hand-over was refused: {e}"));
                Some(LRESULT(0))
            }
            Received::Request(forward) => {
                if !self.closing {
                    if let Forward::Open(targets) = forward {
                        for target in targets {
                            crate::logging::line(&format!("hand-over: open {target}"));
                            let effects = self.tabs.open(&target);
                            self.apply(effects);
                        }
                    }
                    self.bring_to_front();
                }
                Some(LRESULT(1))
            }
        }
    }

    fn bring_to_front(&self) {
        // SAFETY: our window.
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            // Windows may refuse; the other launch asked for the right.
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    /// Present the shown session's picture if the waitable allows and the
    /// window has an area. True when a picture was drawn.
    fn present_due(&mut self) -> bool {
        if !self.ready || self.client.0 == 0 || self.client.1 == 0 {
            return false;
        }
        let Some(id) = self.shown else {
            return false;
        };
        let Some(link) = self.set.get(id) else {
            return false;
        };
        let slot = Arc::clone(link.slot());
        let (client, dpi, top) = (self.client, self.dpi, self.top());
        let Some(view) = self.views.get_mut(&id) else {
            return false;
        };
        view.waker.take_frame();
        // No presenter (a recreate failed): leave the picture in the slot
        // for the heartbeat's retry rather than dropping it unrecorded.
        let Some(presenter) = self.presenter.as_mut() else {
            return false;
        };
        let (picture, repeat) = match slot.take() {
            Some(picture) => (picture, false),
            None if view.repeat => match view.last.take() {
                Some(picture) => (picture, true),
                None => return false,
            },
            None => return false,
        };
        view.repeat = false;
        let shown_view = picture.geometry.view;
        let (viewport, filter) = placement(shown_view, client, dpi, top);
        let outcome = match presenter.present(&picture.frame, viewport, filter) {
            Ok(outcome) => outcome,
            Err(e) => {
                crate::logging::line(&format!("present failed: {e}"));
                PresentOutcome::DeviceLost
            }
        };
        self.ready = false;
        let now = Instant::now();
        let ms = elapsed_ms(self.started, now);
        self.log.stage(self.stages.current(now), now);
        // A picture shown again has no decode time to measure.
        if !repeat {
            self.log
                .record(picture.frame.decoded_at(), now, outcome, slot.dropped());
        }
        if outcome == PresentOutcome::Shown {
            view.shown_view = Some(shown_view);
            view.shown_geometry = Some(picture.geometry);
        }
        view.last = Some(picture);
        match outcome {
            PresentOutcome::Shown => {
                self.stages.on_shown(shown_view);
                self.resizes.on_picture(shown_view, ms);
                self.log.stage(self.stages.current(now), now);
            }
            PresentOutcome::Occluded => {}
            PresentOutcome::DeviceLost => self.recreate(),
        }
        if let Some(modal) = &mut self.modal {
            modal.shown += 1;
        }
        true
    }

    /// Show the last picture again (or black): paint, resize, drag.
    fn redraw(&mut self) {
        let shown_view = self.shown_view();
        let top = self.top();
        let Some(presenter) = self.presenter.as_mut() else {
            return;
        };
        let (viewport, filter) = match shown_view {
            Some(view) => placement(view, self.client, self.dpi, top),
            None => (
                Viewport {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                Filter::Point,
            ),
        };
        // A redraw is a Present too: it uses up the waitable's slot, so the
        // next picture waits for the waitable instead of queueing behind it.
        self.ready = false;
        match presenter.redraw(viewport, filter) {
            Ok(PresentOutcome::DeviceLost) => self.recreate(),
            Ok(_) => {}
            Err(e) => {
                crate::logging::line(&format!("redraw failed: {e}"));
                self.recreate();
            }
        }
    }

    /// The shown session's view of the window, if a session is shown.
    fn view_mut(&mut self) -> Option<&mut SessionView> {
        let id = self.shown?;
        self.views.get_mut(&id)
    }

    fn shown_view(&self) -> Option<(u32, u32)> {
        self.views.get(&self.shown?)?.shown_view
    }

    /// A lost device: drop the presenter (and its swap chain) and build
    /// a new one on the same window. The session is untouched.
    fn recreate(&mut self) {
        crate::logging::line("device lost: recreating the presenter");
        self.presenter = None;
        self.ready = false;
        match Presenter::new(self.hwnd, client_size(self.hwnd), self.adapter_choice) {
            Ok(presenter) => self.presenter = Some(presenter),
            Err(e) => crate::logging::line(&format!("presenter recreate failed: {e}")),
        }
    }

    fn send_resize(&mut self, req: ResizeReq) {
        let now = Instant::now();
        crate::logging::line(&format!(
            "resize sent: {}x{} at scale {}",
            req.width, req.height, req.scale
        ));
        if let Some(link) = self.shown.and_then(|id| self.set.get(id)) {
            link.send(UiCmd::Resize(req));
        }
        self.resizes.sent(req, self.ms(now));
        self.stages.on_resize(req, now);
        self.update_stage(now);
    }

    fn on_size(&mut self) {
        let full = client_size(self.hwnd);
        if full.0 == 0 || full.1 == 0 {
            // Minimised: keep everything as it was.
            return;
        }
        self.sync_strip();
        let client = picture_size(self.hwnd);
        let now = Instant::now();
        let changed = client != self.client;
        self.client = client;
        if let Some(presenter) = self.presenter.as_mut()
            && let Err(e) = presenter.resize(full)
        {
            crate::logging::line(&format!("ResizeBuffers failed: {e}"));
            self.recreate();
        }
        self.redraw();
        if changed {
            self.stages.on_resize(request_for(client, self.dpi), now);
            self.update_stage(now);
        }
        let (dpi, moving) = (self.dpi, self.moving());
        if let Some(delay) = self
            .view_mut()
            .and_then(|view| view.gate.on_size(client, dpi, moving, now))
        {
            self.set_timer(TIMER_SETTLE, delay);
        }
    }

    fn on_settle_timer(&mut self) {
        self.kill_timer(TIMER_SETTLE);
        let Some(view) = self.view_mut() else {
            return;
        };
        match view.gate.on_settle(Instant::now()) {
            Settled::Send(req) => self.send_resize(req),
            Settled::Wait(delay) => self.set_timer(TIMER_SETTLE, delay),
            Settled::Nothing => {}
        }
    }

    /// Whether a drag or resize of the window is under way.
    fn moving(&self) -> Moving {
        match &self.modal {
            Some(modal) if modal.kind == ModalKind::SizeMove => Moving::InSizeMove,
            _ => Moving::Free,
        }
    }

    fn on_exit_size_move(&mut self) {
        self.on_exit_modal(ModalKind::SizeMove);
        let (client, dpi) = (self.client, self.dpi);
        if let Some(req) = self
            .view_mut()
            .and_then(|view| view.gate.on_exit_size_move(client, dpi))
        {
            self.send_resize(req);
        }
    }

    /// A modal loop starves the main loop: poll the waitable and present
    /// from here.
    fn on_modal_tick(&mut self) {
        self.poll_waitable();
        self.drain_events();
        self.present_due();
    }

    /// Sets `ready` if the waitable is signalled, without waiting.
    fn poll_waitable(&mut self) {
        if !self.ready
            && let Some(presenter) = &self.presenter
        {
            // SAFETY: the presenter's own waitable, polled without waiting.
            let event = unsafe { WaitForSingleObjectEx(presenter.waitable(), 0, false) };
            self.ready = event == WAIT_OBJECT_0;
        }
    }

    /// A drag began or a menu opened: Windows runs its own message loop
    /// until it ends, so the main loop no longer waits on the waitable.
    fn on_enter_modal(&mut self, kind: ModalKind) {
        self.modal = Some(Modal {
            kind,
            began: Instant::now(),
            shown: 0,
        });
        self.set_timer(TIMER_MODAL, MODAL_TICK);
    }

    fn on_exit_modal(&mut self, kind: ModalKind) {
        if !self.modal.as_ref().is_some_and(|modal| modal.kind == kind) {
            return;
        }
        self.kill_timer(TIMER_MODAL);
        if let Some(Modal { kind, began, shown }) = self.modal.take() {
            let ms = began.elapsed().as_millis();
            crate::logging::line(&match kind {
                ModalKind::Menu => {
                    format!("menu: {shown} pictures shown in {ms} ms while it was open")
                }
                ModalKind::SizeMove => {
                    format!("move: {shown} pictures shown in {ms} ms while dragging")
                }
            });
        }
    }

    fn on_dpi_changed(&mut self, dpi: u32, suggested: RECT) {
        self.dpi = dpi;
        // SAFETY: our window; the rect is the one Windows suggested. The
        // WM_SIZE this sends is deferred and handled after this arm.
        let moved = unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        if let Err(e) = moved {
            crate::logging::line(&format!("WM_DPICHANGED SetWindowPos failed: {e}"));
        }
        self.sync_strip();
        let client = picture_size(self.hwnd);
        if let Some(req) = self
            .view_mut()
            .and_then(|view| view.gate.on_dpi_changed(client, dpi))
        {
            self.send_resize(req);
        }
    }

    fn toggle_fullscreen(&mut self) {
        // SetWindowPos inside sends WM_SIZE; it is deferred until this
        // arm returns.
        match self.fullscreen.toggle(self.hwnd) {
            Ok(screen) => {
                crate::logging::line(&format!("fullscreen: {screen:?}"));
                self.stages.on_screen(screen);
                self.update_stage(Instant::now());
            }
            Err(e) => crate::logging::line(&format!("fullscreen toggle failed: {e}")),
        }
    }

    /// The strip's height above the picture, 0 in full screen.
    fn top(&self) -> u32 {
        u32::try_from(self.strip.height()).unwrap_or(0)
    }

    /// Fits the strip to the window, the scale and the tabs, and draws it.
    fn sync_strip(&mut self) {
        let fullscreen = self.fullscreen.screen() == Screen::Fullscreen;
        // A window with no tab opens the bar, and keeps it.
        let acts = self.strip.address.tabs_changed(self.tabs.len());
        self.run_address_acts(acts);
        self.strip.sync(&self.tabs, self.dpi, fullscreen);
        self.strip.hold_bar(self.strip.address.is_open());
        self.sync_panel();
        // While the field has the keyboard the grab lets go of it.
        let typing = self.strip.address.typing();
        if self.typing_sent.update(typing) {
            self.input.grab_mut().typing(typing);
        }
        self.sync_win_l();
    }

    /// Shows the failure panel over the picture while the shown tab has
    /// given up with a failure, and hides it otherwise.
    fn sync_panel(&mut self) {
        let shown = self.tabs.shown().and_then(|id| self.tabs.tab(id));
        let failed = shown.and_then(|tab| {
            let failure = tab.failure()?;
            Some((tab.id(), failure_panel::content(failure, tab.target())))
        });
        // Close hides the panel until the tab is shown again or changes.
        if self.panel_dismissed.is_some() && self.panel_dismissed != failed.as_ref().map(|f| f.0) {
            self.panel_dismissed = None;
        }
        let content = failed
            .filter(|(id, _)| Some(*id) != self.panel_dismissed)
            .map(|(_, content)| content);
        let top = i32::try_from(self.top()).unwrap_or(0);
        let (width, height) = client_size(self.hwnd);
        let (width, height) = (
            i32::try_from(width).unwrap_or(0),
            i32::try_from(height).unwrap_or(0),
        );
        self.panel.show(
            content.as_ref(),
            (0, top, width, (height - top).max(0)),
            self.dpi,
        );
    }

    /// A click on the failure panel: what the button or link under it does.
    pub(crate) fn panel_click(&mut self, x: i32, y: i32) {
        let Some((action, layout)) = self.panel.click_at(x, y) else {
            return;
        };
        let Some(id) = self.tabs.shown() else {
            return;
        };
        let button = match action {
            PanelAction::Primary => layout.primary_button(),
            PanelAction::Details => Button::Details,
            PanelAction::Guide => {
                if let Some(guide) = layout.guide() {
                    crate::panel_win::open(&failure_panel::guide_url(guide));
                }
                return;
            }
        };
        match button {
            Button::TryAgain | Button::CheckAgain => {
                let effects = self.tabs.reconnect(id);
                self.apply(effects);
            }
            Button::EditAddress => {
                let target = self.tabs.tab(id).map(|tab| tab.target().to_string());
                self.address_event(AddressEvent::Open);
                if let Some(target) = target {
                    self.address_event(AddressEvent::Pick(target));
                }
            }
            Button::OpenOptionalFeatures => crate::panel_win::open("ms-settings:optionalfeatures"),
            Button::Close => {
                self.panel_dismissed = Some(id);
                self.sync_panel();
            }
            Button::Details => crate::panel_win::open(&self.log_dir.to_string_lossy()),
        }
    }

    /// One strip input. What it asks of the tabs is done here; the popups
    /// it asks for are returned, to be shown once the state is free.
    pub(crate) fn strip_input(&mut self, input: StripInput) -> Vec<Popup> {
        let actions = self.strip.input(input);
        let mut popups = Vec::new();
        for action in actions {
            match action {
                StripAction::Show(id) => self.strip_show(id),
                StripAction::Dot(id) => {
                    let command = self.tabs.tab(id).map(|tab| menu::dot_command(tab.status()));
                    if let Some(command) = command {
                        self.strip_command(command, id);
                    }
                }
                StripAction::Menu(id, at) => popups.push(Popup::Tab(id, at)),
                StripAction::Move { id, before } => {
                    let effects = self.tabs.move_tab(id, before);
                    self.apply(effects);
                }
                StripAction::Add => self.address_event(AddressEvent::Open),
                StripAction::Go => self.address_event(AddressEvent::Go),
                StripAction::List => {
                    self.address_event(AddressEvent::List(true));
                    popups.push(Popup::Recent);
                }
                StripAction::Chevron => popups.push(Popup::List),
                StripAction::Fullscreen => self.toggle_fullscreen(),
                StripAction::Scroll(px) => self.strip.scroll_by(px),
            }
        }
        popups
    }

    pub(crate) fn strip_show(&mut self, id: crate::tabs::TabId) {
        let effects = self.tabs.click(id);
        self.apply(effects);
        // A click on a tab hands the keyboard back to the picture.
        self.strip.defer(Later::FocusMain);
    }

    /// The address bar's field or buttons reported something.
    pub(crate) fn address_event(&mut self, event: AddressEvent) {
        if self.closing {
            return;
        }
        let acts = self.strip.address.input(event, self.tabs.len());
        self.run_address_acts(acts);
        self.sync_strip();
    }

    fn run_address_acts(&mut self, acts: Vec<AddressAct>) {
        for act in acts {
            match act {
                AddressAct::Focus => self.strip.defer(Later::Focus),
                AddressAct::SetText(text) => self.strip.defer(Later::SetText(text)),
                AddressAct::FocusMain => self.strip.defer(Later::FocusMain),
                AddressAct::Connect(target) => {
                    crate::logging::line(&format!("address bar: open {target}"));
                    let effects = self.tabs.open(&target);
                    self.apply(effects);
                    self.strip.defer(Later::FocusMain);
                }
            }
        }
    }

    /// The rows of the recent-hosts menu.
    pub(crate) fn recent_rows(&self) -> Vec<RecentRow> {
        recent_menu(
            self.files.history.hosts().hosts(),
            self.files.history.remember(),
        )
    }

    /// The recent-hosts menu closed with this pick, or none.
    pub(crate) fn recent_chosen(&mut self, chosen: Option<RecentChoice>) {
        match chosen {
            Some(RecentChoice::Pick(host)) => {
                self.address_event(AddressEvent::List(false));
                self.address_event(AddressEvent::Pick(host.to_string()));
                return;
            }
            Some(RecentChoice::Forget(host)) => {
                if let Err(e) = self.files.history.forget(&host) {
                    crate::logging::line(&format!("recent hosts not saved: {e}"));
                }
            }
            Some(RecentChoice::Clear) => {
                if let Err(e) = self.files.history.clear() {
                    crate::logging::line(&format!("recent hosts not saved: {e}"));
                }
            }
            None => {}
        }
        self.address_event(AddressEvent::List(false));
    }

    pub(crate) fn tab_menu_entries(&self, id: crate::tabs::TabId) -> Option<Vec<MenuEntry>> {
        let tab = self.tabs.tab(id)?;
        let connection = self.rules.connection(tab.target());
        let live = tab
            .session()
            .and_then(|session| self.views.get(&session))
            .map(|view| &view.running);
        Some(menu::tab_menu(tab.status(), &connection, live))
    }

    pub(crate) fn strip_command(&mut self, command: MenuCommand, id: crate::tabs::TabId) {
        if command == MenuCommand::CopyAddress {
            if let Some(tab) = self.tabs.tab(id)
                && let Err(e) = crate::strip_win::copy_text(self.hwnd, &tab.target().to_string())
            {
                crate::logging::line(&format!("copy address failed: {e}"));
            }
            return;
        }
        if let MenuCommand::ShowAs(_)
        | MenuCommand::ShareClipboard(_)
        | MenuCommand::SharpColour(_)
        | MenuCommand::GpuDecode(_)
        | MenuCommand::SendWinL(_) = command
        {
            self.choose(command, id);
            return;
        }
        let effects = menu::run(command, id, &mut self.tabs);
        self.apply(effects);
    }

    /// A choice from the Connection settings submenu: kept for the tab's
    /// machine and saved at once. The screen, the colour and the decoder
    /// apply at the next connect; the clipboard applies to the tab's session
    /// at once, and so does Send Win+L when the tab is shown. A row a flag has
    /// locked is greyed in the menu and cannot get here; if it did, the
    /// rules would still ignore it.
    fn choose(&mut self, command: MenuCommand, id: crate::tabs::TabId) {
        let Some(machine) = self.tabs.tab(id).map(|tab| tab.target().clone()) else {
            return;
        };
        {
            let mut book = self.files.config.choices().borrow_mut();
            match command {
                MenuCommand::ShowAs(screen) => book.set_screen(&machine, screen),
                MenuCommand::ShareClipboard(share) => book.set_share_clipboard(&machine, share),
                MenuCommand::SharpColour(sharp) => book.set_sharp_colour(&machine, sharp),
                MenuCommand::GpuDecode(gpu) => book.set_gpu_decode(&machine, gpu),
                MenuCommand::SendWinL(send) => book.set_send_win_l(&machine, send),
                _ => return,
            }
        }
        // The file is written whole, so it gets the tabs as they are now.
        *self.files.config.tabs_mut() = self.tabs.tab_set();
        if let Err(e) = self.files.config.save() {
            crate::logging::line(&format!("config: connection choice not saved: {e}"));
        }
        match command {
            MenuCommand::ShareClipboard(_) => {
                self.share_now(id, self.rules.shares_clipboard(&machine));
            }
            MenuCommand::SendWinL(_) => self.sync_win_l(),
            _ => {}
        }
    }

    /// Hands the grab the Send Win+L switch of the shown tab's machine (off
    /// with no tab shown). Called with every strip sync, so a tab switch, a
    /// new tab or a forgotten one is caught up at once.
    fn sync_win_l(&mut self) {
        let send = self
            .tabs
            .shown()
            .and_then(|id| self.tabs.tab(id))
            .is_some_and(|tab| self.rules.connection(tab.target()).send_win_l);
        self.input.grab_mut().set_send_win_l(send);
    }

    /// Applies the clipboard choice to the tab's session. One that started
    /// sharing stops or resumes in place; one that started without has no
    /// clipboard engine to resume, so turning sharing on reconnects the tab.
    fn share_now(&mut self, id: crate::tabs::TabId, share: bool) {
        let Some(session) = self.tabs.tab(id).and_then(|tab| tab.session()) else {
            return;
        };
        let Some(view) = self.views.get_mut(&session) else {
            return;
        };
        if view.running.share_clipboard == share {
            return;
        }
        if view.started_sharing {
            view.running.share_clipboard = share;
            self.set.set_sharing(session, share);
        } else {
            let effects = menu::run(MenuCommand::Reconnect, id, &mut self.tabs);
            self.apply(effects);
        }
    }

    pub(crate) fn chevron_rows(&self) -> Vec<ListRow> {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.id(),
                    tab.target().to_string(),
                    dot_of(tab.status(), tab.failure()),
                )
            })
            .collect();
        menu::chevron_list(&tabs, self.tabs.shown())
    }

    fn on_close(&mut self) {
        if self.closing {
            return;
        }
        if self.set.running() == 0 {
            self.destroy();
            return;
        }
        crate::logging::line("close: sending Bye");
        self.closing = true;
        self.begin_closing();
        // The tabs stay as they were: what was open is open at the next start.
        self.set.close_all();
        self.shown = None;
        self.set_timer(TIMER_CLOSE, CLOSE_WAIT);
        // The sessions may already be over.
        self.drain_events();
    }

    /// End the measured stages: Bye, the ssh wait and teardown are not
    /// part of what the gate judges.
    fn begin_closing(&mut self) {
        self.stages.on_close();
        self.update_stage(Instant::now());
    }

    fn destroy(&mut self) {
        self.kill_timer(TIMER_CLOSE);
        // SAFETY: our window; WM_DESTROY is deferred and quits the loop.
        if let Err(e) = unsafe { DestroyWindow(self.hwnd) } {
            crate::logging::line(&format!("DestroyWindow failed: {e}"));
        }
    }

    fn set_timer(&self, id: usize, after: Duration) {
        let ms = u32::try_from(after.as_millis()).unwrap_or(u32::MAX).max(1);
        // SAFETY: a timer on our own window, delivered as WM_TIMER.
        unsafe { SetTimer(Some(self.hwnd), id, ms, None) };
    }

    fn kill_timer(&self, id: usize) {
        // SAFETY: our own window's timer; killing an unset one fails
        // harmlessly.
        let _ = unsafe { KillTimer(Some(self.hwnd), id) };
    }

    /// Starts the 1 s heartbeat: the stats file and the presenter retry.
    pub fn start_stats(&self) {
        self.set_timer(
            TIMER_GRAB,
            Duration::from_millis(u64::from(crate::grab::TICK_MS)),
        );
        self.set_timer(TIMER_STATS, STATS_EVERY);
        self.write_stats(Completion::Running);
    }

    /// Each second: retry a presenter that failed to come back, then
    /// write the stats file.
    fn on_heartbeat(&mut self) {
        if self.presenter.is_none() && !self.closing {
            // The new swap chain's waitable restarts presenting.
            self.recreate();
        }
        self.write_stats(Completion::Running);
    }

    /// The stats file now; errors are logged, never fatal.
    fn write_stats(&self, completion: Completion) {
        let Some(path) = &self.stats_out else {
            return;
        };
        let stats = self.stats(completion);
        if let Err(e) = stats.write(path) {
            crate::logging::line(&format!("stats write to {} failed: {e}", path.display()));
        }
    }

    /// Every session's counters, live ones first, then those that are over.
    /// The first session started is the one the top-level fields describe.
    fn session_counters(&self) -> Vec<(SessionId, SessionStats, u64)> {
        let mut all: Vec<(SessionId, SessionStats, u64)> = self
            .set
            .reports()
            .into_iter()
            .map(|report| {
                let dropped = self
                    .set
                    .get(report.session)
                    .map_or(0, |link| link.slot().dropped());
                (report.session, report.stats, dropped)
            })
            .collect();
        all.extend(
            self.finals
                .iter()
                .map(|(id, end)| (*id, end.stats.clone(), end.slot_dropped)),
        );
        all.sort_by_key(|(id, _, _)| *id);
        all
    }

    fn stats(&self, completion: Completion) -> ClientStats {
        let now = Instant::now();
        let counters = self.session_counters();
        let primary = counters.first();
        let session = primary.map_or_else(SessionCounters::default, |(_, stats, dropped)| {
            SessionCounters::new(stats, *dropped)
        });
        let first_end = self
            .finals
            .iter()
            .min_by_key(|(id, _)| *id)
            .map(|(_, end)| end);
        let (end, complete, ssh_exit) = match completion {
            Completion::Running => (String::new(), false, None),
            Completion::Done => (
                first_end
                    .and_then(|end| end.reason.as_ref())
                    .map_or("none", end_name)
                    .to_string(),
                true,
                first_end.and_then(|end| end.ssh_exit),
            ),
        };
        let (monitor, rect) = self.screen();
        let shown = self.shown.and_then(|id| self.views.get(&id));
        let mut input = self.input.info(shown.and_then(|view| view.shown_geometry));
        if let Some((_, sent, _)) = primary {
            input.wheel_steps_sent.clone_from(&sent.wheel_steps_sent);
            input.smooth_scrolls_sent = sent.smooth_scrolls_sent;
            input.scroll_stops_sent = sent.scroll_stops_sent;
            input
                .button_presses_sent
                .clone_from(&sent.button_presses_sent);
        }
        ClientStats {
            version: env!("CARGO_PKG_VERSION").to_string(),
            complete,
            session,
            present: self.log.summary(now),
            adapter: self
                .presenter
                .as_ref()
                .map(|p| p.adapter().clone())
                .unwrap_or_default(),
            monitor,
            window: WindowInfo {
                client: self.client,
                rect,
                fullscreen: self.fullscreen.screen() == crate::stages::Screen::Fullscreen,
                dpi: self.dpi,
            },
            swap_chain: self
                .presenter
                .as_ref()
                .map(Presenter::swap_chain_info)
                .unwrap_or_default(),
            resizes: self.resizes.records().to_vec(),
            fps_cap: shown.map_or(0, |view| view.fps_cap),
            end,
            ssh_exit,
            input,
            sessions: session_entries(&self.set.reports(), &self.tabs),
            chords: self.chords.clone(),
        }
    }

    /// The live monitor and window rect, or the last ones seen once the
    /// window is gone.
    fn screen(&self) -> (MonitorInfo, (i32, i32, u32, u32)) {
        // SAFETY: IsWindow accepts any handle value.
        if unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
            *self.screen.borrow_mut() = (monitor_info(self.hwnd), window_rect(self.hwnd));
        }
        self.screen.borrow().clone()
    }

    /// After the loop: wait for every session, write the last stats file
    /// and pick the exit code (0 only when every session ended with Bye and
    /// ssh exit 0, and none failed to start).
    pub fn finish(mut self) -> std::process::ExitCode {
        self.begin_closing();
        let sessions = session_entries(&self.set.reports(), &self.tabs);
        for (session, end) in self.set.shutdown() {
            self.on_session_over(session, end);
        }
        let mut stats = self.stats(Completion::Done);
        stats.sessions = sessions;
        if let Some(path) = &self.stats_out
            && let Err(e) = stats.write(path)
        {
            crate::logging::line(&format!("final stats write failed: {e}"));
        }
        let ends: Vec<_> = self.finals.iter().map(|(_, end)| end.clone()).collect();
        let clean = crate::session::driver::run_was_clean(self.start_failures, &ends);
        if clean {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::from(1)
        }
    }

    /// One message. `None` sends it on to `DefWindowProcW` (with no
    /// borrow held).
    fn on_message(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        // Input first: with the grab on, a key message that reaches the
        // window (F11 included) belongs to the remote machine.
        let view = View {
            geometry: self
                .shown
                .and_then(|id| self.views.get(&id))
                .and_then(|view| view.shown_geometry),
            client: self.client,
            top: i32::try_from(self.top()).unwrap_or(0),
            dpi: self.dpi,
        };
        // Full screen: the pointer at the top edge brings the strip out. The
        // strip's own timer reads the pointer when it leaves (no
        // `WM_MOUSELEAVE` here: the strip is a child, and moving onto it
        // would look like leaving).
        if msg == WM_MOUSEMOVE {
            self.strip
                .pointer(Pointer::At(i32::from((lparam.0 >> 16) as u16 as i16)));
        }
        if let Some(result) = self.input.on_message(&view, msg, wparam, lparam) {
            return Some(result);
        }
        match msg {
            // A hidden session's picture waits in its slot until its tab is shown.
            WM_APP_FRAME => {
                if self.shown.is_some_and(|id| id.get() as usize == wparam.0) {
                    // Inside a modal loop the main loop never sees the
                    // waitable, so look here: each picture goes out as it
                    // arrives, not on the next timer tick.
                    if self.modal.is_some() {
                        self.poll_waitable();
                    }
                    self.present_due();
                }
                Some(LRESULT(0))
            }
            WM_APP_SWITCH => {
                self.on_switch(wparam.0);
                Some(LRESULT(0))
            }
            WM_COPYDATA => self.on_copy_data(lparam),
            WM_APP_EVENT => {
                self.drain_events();
                Some(LRESULT(0))
            }
            WM_SIZE => {
                self.on_size();
                Some(LRESULT(0))
            }
            WM_ENTERSIZEMOVE => {
                self.on_enter_modal(ModalKind::SizeMove);
                Some(LRESULT(0))
            }
            WM_EXITSIZEMOVE => {
                self.on_exit_size_move();
                Some(LRESULT(0))
            }
            WM_ENTERMENULOOP => {
                self.on_enter_modal(ModalKind::Menu);
                Some(LRESULT(0))
            }
            WM_EXITMENULOOP => {
                self.on_exit_modal(ModalKind::Menu);
                Some(LRESULT(0))
            }
            WM_TIMER => {
                match wparam.0 {
                    TIMER_SETTLE => self.on_settle_timer(),
                    TIMER_MODAL => self.on_modal_tick(),
                    TIMER_CLOSE => {
                        crate::logging::line("close: no Bye within 3 s");
                        self.destroy();
                    }
                    TIMER_STATS => self.on_heartbeat(),
                    TIMER_GRAB => self.input.on_timer(),
                    TIMER_RETRY => self.drain_events(),
                    _ => return None,
                }
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                let dpi = u32::from(hiword(wparam.0));
                // SAFETY: for WM_DPICHANGED, lparam points at the suggested
                // RECT for the duration of the message.
                let suggested = unsafe { *(lparam.0 as *const RECT) };
                self.on_dpi_changed(dpi, suggested);
                Some(LRESULT(0))
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: our window, a live PAINTSTRUCT, paired with
                // EndPaint (which validates the area).
                unsafe { BeginPaint(self.hwnd, &mut ps) };
                self.redraw();
                // SAFETY: as above.
                let _ = unsafe { EndPaint(self.hwnd, &ps) };
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_KEYDOWN if wparam.0 == usize::from(VK_F11.0) => {
                // Bit 30: the key was already down (auto-repeat).
                if lparam.0 & (1 << 30) == 0 {
                    self.toggle_fullscreen();
                }
                Some(LRESULT(0))
            }
            WM_SYSCOMMAND if wparam.0 & 0xFFF0 == IDM_FULLSCREEN => {
                self.toggle_fullscreen();
                Some(LRESULT(0))
            }
            WM_SETCURSOR if u32::from(loword(lparam.0 as usize)) == HTCLIENT => {
                match self.shown.and_then(|id| self.views.get(&id)) {
                    Some(view) => {
                        view.cursor.on_set_cursor(self.hwnd);
                        Some(LRESULT(1))
                    }
                    None => None,
                }
            }
            WM_CLOSE => {
                self.on_close();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                self.on_destroy();
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn on_destroy(&mut self) {
        self.input.shutdown();
        self.kill_timer(TIMER_GRAB);
        self.kill_timer(TIMER_STATS);
        self.kill_timer(TIMER_RETRY);
        self.kill_timer(TIMER_SETTLE);
        self.kill_timer(TIMER_MODAL);
        // SAFETY: ends this thread's message loop.
        unsafe { PostQuitMessage(0) };
    }

    fn on_deferred(&mut self, deferred: Deferred) {
        match deferred {
            Deferred::Size => self.on_size(),
            Deferred::Destroy => self.on_destroy(),
            Deferred::EndSession => self.input.shutdown(),
        }
    }
}

/// Milliseconds from `started` to `now`.
fn elapsed_ms(started: Instant, now: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(started).as_millis()).unwrap_or(u64::MAX)
}

/// Whether the stats file is the last one.
#[derive(Clone, Copy)]
enum Completion {
    Running,
    Done,
}

/// Puts the state where the window procedure finds it.
pub fn install(app: App) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

/// Takes the state back after the loop.
pub fn uninstall() -> Option<App> {
    APP.with(|cell| cell.borrow_mut().take())
}

/// Runs `f` on the state, then anything deferred meanwhile. `None` when
/// the state is not installed or already borrowed.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let result = APP.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        let app = guard.as_mut()?;
        Some(f(app))
    });
    replay();
    result
}

/// Handles the messages deferred while the state was borrowed.
fn replay() {
    loop {
        let next = DEFERRED.with(|d| {
            let mut d = d.borrow_mut();
            if d.is_empty() {
                None
            } else {
                Some(d.remove(0))
            }
        });
        let Some(deferred) = next else {
            return;
        };
        let handled = APP.with(|cell| {
            let Ok(mut guard) = cell.try_borrow_mut() else {
                return false;
            };
            if let Some(app) = guard.as_mut() {
                app.on_deferred(deferred);
            }
            true
        });
        if !handled {
            // Still borrowed further up the stack: that frame replays.
            DEFERRED.with(|d| d.borrow_mut().insert(0, deferred));
            return;
        }
    }
}

/// A child window's menu tells the child it opened or closed; the
/// top-level window, which presents, needs to know.
pub fn forward_menu_loop(child: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) {
    if matches!(msg, WM_ENTERMENULOOP | WM_EXITMENULOOP) {
        // SAFETY: our own child; the top-level window is ours too.
        unsafe {
            let top = GetAncestor(child, GA_ROOT);
            if !top.0.is_null() && top != child {
                SendMessageW(top, msg, Some(wparam), Some(lparam));
            }
        }
    }
}

/// The window procedure.
pub extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A click on the picture while the address bar's edit field has the
    // keyboard takes it back. The focus moves here, with the state free, so
    // the edit field's own focus messages reach it.
    if matches!(
        msg,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    ) {
        // SAFETY: our own windows; the focus window is only compared.
        unsafe {
            let focus = GetFocus();
            if !focus.0.is_null() && focus != hwnd && IsChild(hwnd, focus).as_bool() {
                let _ = SetFocus(Some(hwnd));
            }
        }
    }
    let handled = APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => match guard.as_mut() {
            Some(app) => Handled::Done(app.on_message(msg, wparam, lparam)),
            None => Handled::Done(None),
        },
        Err(_) => Handled::Busy,
    });
    let result = match handled {
        Handled::Done(result) => {
            replay();
            result
        }
        Handled::Busy => {
            let deferred = match msg {
                WM_SIZE => Some(Deferred::Size),
                WM_DESTROY => Some(Deferred::Destroy),
                // Windows may end the process as soon as this returns, so
                // the setting goes back now (it needs no window state);
                // the rest of the shutdown follows when the state is free.
                WM_ENDSESSION if wparam.0 != 0 => {
                    crate::lockreg::restore_now();
                    Some(Deferred::EndSession)
                }
                _ => None,
            };
            if let Some(deferred) = deferred {
                DEFERRED.with(|d| d.borrow_mut().push(deferred));
            }
            None
        }
    };
    match result {
        Some(result) => result,
        // SAFETY: the arguments are this call's own.
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

enum Handled {
    Done(Option<LRESULT>),
    Busy,
}

/// Adds "Fullscreen\tF11" to the window menu.
pub fn add_menu_item(hwnd: HWND) -> windows::core::Result<()> {
    // SAFETY: our window's own menu; the strings are static.
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
        AppendMenuW(menu, MF_STRING, IDM_FULLSCREEN, w!("Fullscreen\tF11"))?;
    }
    Ok(())
}

/// The viewport and filter for `view` in a `client` area at `dpi`.
fn placement(view: (u32, u32), client: (u32, u32), dpi: u32, top: u32) -> (Viewport, Filter) {
    let device = f64::from(dpi) / 96.0;
    let logical = (f64::from(client.0) / device, f64::from(client.1) / device);
    match layout(view, device, logical.0, logical.1) {
        Some(l) => {
            let mut viewport = l.viewport(device);
            viewport.y += i32::try_from(top).unwrap_or(0);
            (viewport, l.filter())
        }
        None => (
            Viewport {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            Filter::Point,
        ),
    }
}

pub fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    // SAFETY: our window, a live RECT.
    if unsafe { GetClientRect(hwnd, &mut r) }.is_err() {
        return (0, 0);
    }
    (
        u32::try_from(r.right - r.left).unwrap_or(0),
        u32::try_from(r.bottom - r.top).unwrap_or(0),
    )
}

/// The part of the client area the picture gets: all of it below the strip
/// (which is hidden in full screen).
pub fn picture_size(hwnd: HWND) -> (u32, u32) {
    let (width, height) = client_size(hwnd);
    (
        width,
        height.saturating_sub(crate::strip_win::height_of(hwnd)),
    )
}

pub fn window_dpi(hwnd: HWND) -> u32 {
    // SAFETY: our window.
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    }
}

fn window_rect(hwnd: HWND) -> (i32, i32, u32, u32) {
    let mut r = RECT::default();
    // SAFETY: our window, a live RECT.
    let _ = unsafe { GetWindowRect(hwnd, &mut r) };
    rect_tuple(r)
}

fn rect_tuple(r: RECT) -> (i32, i32, u32, u32) {
    (
        r.left,
        r.top,
        u32::try_from(r.right - r.left).unwrap_or(0),
        u32::try_from(r.bottom - r.top).unwrap_or(0),
    )
}

/// The window's monitor: rect, DPI and refresh rate.
pub fn monitor_info(hwnd: HWND) -> MonitorInfo {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: our window; `info` is a live MONITORINFOEXW with cbSize set,
    // which GetMonitorInfoW accepts through its MONITORINFO prefix.
    let ok = unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()).as_bool()
    };
    if !ok {
        return MonitorInfo::default();
    }
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: the device name is NUL-terminated by GetMonitorInfoW; `mode`
    // is a live DEVMODEW with dmSize set.
    let refresh = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    };
    MonitorInfo {
        rect: rect_tuple(info.monitorInfo.rcMonitor),
        dpi: window_dpi(hwnd),
        refresh_hz: if refresh.as_bool() {
            f64::from(mode.dmDisplayFrequency)
        } else {
            0.0
        },
    }
}

fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}
