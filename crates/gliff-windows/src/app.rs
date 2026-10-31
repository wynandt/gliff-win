//! `gliff-windows`' entry on Windows: the window, the presenter, the
//! session and the main loop.

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTOPRIMARY;
use windows::Win32::Graphics::Gdi::MONITORINFO;
use windows::Win32::Graphics::Gdi::MonitorFromWindow;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use windows::Win32::UI::WindowsAndMessaging::CS_HREDRAW;
use windows::Win32::UI::WindowsAndMessaging::CS_VREDRAW;
use windows::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::IDC_ARROW;
use windows::Win32::UI::WindowsAndMessaging::LoadCursorW;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::MWMO_INPUTAVAILABLE;
use windows::Win32::UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx;
use windows::Win32::UI::WindowsAndMessaging::PM_REMOVE;
use windows::Win32::UI::WindowsAndMessaging::PeekMessageW;
use windows::Win32::UI::WindowsAndMessaging::QS_ALLINPUT;
use windows::Win32::UI::WindowsAndMessaging::RegisterClassExW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
use windows::Win32::UI::WindowsAndMessaging::ShowWindow;
use windows::Win32::UI::WindowsAndMessaging::TranslateMessage;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WM_QUIT;
use windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW;
use windows::Win32::UI::WindowsAndMessaging::WS_CLIPCHILDREN;
use windows::Win32::UI::WindowsAndMessaging::WS_OVERLAPPEDWINDOW;
use windows::core::w;

use crate::cli::Args;
use crate::clip_hub::ClipHub;
use crate::clip_win;
use crate::clip_win::ClipWin;
use crate::grab::Grab;
use crate::grab::GrabConfig;
use crate::hook;
use crate::input::SuperL;
use crate::lockreg;
use crate::lockreg::LockGuard;
use crate::machine_choices::Rules;
use crate::present::Presenter;
use crate::resize_gate::request_for;
use crate::route::InputRoute;
use crate::session::driver::Options;
use crate::session::driver::SessionHandle;
use crate::session::driver::Waker;
use crate::session::driver::start_with_clipboard;
use crate::session_set::ClipSide;
use crate::session_set::HubClip;
use crate::session_set::Launch as SessionLaunch;
use crate::session_set::SessionSet;
use crate::session_set::Starter;
use crate::settings::GrabTimeout;
use crate::settings::Settings;
use crate::single_instance;
use crate::single_instance::Launch;
use crate::ssh::SshSpec;
use crate::ssh::validate;
use crate::startup::Startup;
use crate::stats_out::ChordsInfo;
use crate::strip::Metrics;
use crate::tabs::Tabs;
use crate::test_hooks::TestHooks;
use crate::waker::PostWaker;
use crate::window;
use crate::window::App;
use crate::window::AppParts;
use crate::window::CLASS_NAME;
use crate::window::Files;
use crate::window::LaunchBook;
use crate::window::Launched;
use crate::window_input::WindowInput;
use crate::window_input::current_klid;
use crate::xkb::KeymapTracker;

/// The first window's client area, in logical pixels. Deliberately not
/// `gwin-verify present`'s 1280x800 (2560x1600 at 200%), so its windowed
/// step is a real resize; and small enough for a 1080p 100% screen.
pub const INITIAL_CLIENT: (i32, i32) = (1440, 900);
/// The longest the loop sleeps with nothing to do (timers and posts wake
/// it sooner).
const IDLE_WAIT_MS: u32 = 1000;

/// Runs the client until the window closes.
pub fn run(args: Args) -> ExitCode {
    let started = Instant::now();
    // SAFETY: called before any window exists; failure (already set by a
    // manifest) leaves the process DPI-aware anyway or unaware, both
    // usable.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let startup = match Startup::load(&args) {
        Ok(startup) => startup,
        Err(e) => return fail(&format!("refusing to start: {e}")),
    };
    let settings = startup.resolved.settings.clone();
    let log_dir = settings.log_dir_or(
        std::env::var_os("LOCALAPPDATA").map(Into::into),
        std::env::temp_dir(),
    );
    // One client per configuration folder: a later launch hands its request
    // to the running one and leaves.
    let instance_name = single_instance::instance_name(startup.dir.path());
    let _instance = match single_instance::win::acquire(&instance_name) {
        Ok(instance) => {
            if instance.is_some() {
                // Ours; nothing to decide.
                instance
            } else {
                return match single_instance::decide(true, &args.targets, false) {
                    Launch::HandOver(forward) => {
                        match single_instance::win::hand_over(&instance_name, &forward) {
                            Ok(()) => ExitCode::SUCCESS,
                            Err(e) => fail(&format!("could not reach the running client: {e}")),
                        }
                    }
                    Launch::Quit | Launch::Run => ExitCode::SUCCESS,
                };
            }
        }
        // Without the mutex a second client could start; carry on as the
        // only one we know of.
        Err(e) => {
            eprintln!("gliff-windows: single-instance check failed: {e}");
            None
        }
    };
    let (tabs, effects) = Tabs::start_up(startup.config.tabs(), &args.targets);
    match crate::logging::init(&log_dir) {
        Ok(path) => crate::logging::line(&format!(
            "gliff-windows {} starting; log {}",
            env!("CARGO_PKG_VERSION"),
            path.display()
        )),
        Err(e) => eprintln!("gliff-windows: no log in {}: {e}", log_dir.display()),
    }
    crate::logging::line(&format!("config folder {}", startup.dir.path().display()));
    for note in &startup.notes {
        crate::logging::line(&format!("config: {note}"));
    }
    // A panic or a console control event lets go of the modifiers Windows
    // still counts as down; the window's own exit path does it in
    // `Grab::shutdown`.
    crate::modifiers::install_exit_paths();
    // Every way out of this function puts the lock setting back (the panic
    // hook and the console handler cover the rest); nothing is held until
    // the first grab.
    let _restore = (settings.super_l == SuperL::Registry).then(|| {
        if lockreg::init().is_none() {
            crate::logging::line(
                "lock setting: this process's start time is unreadable, so the registry route is off",
            );
        }
        LockGuard
    });
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(e) => return fail(&format!("window creation failed: {e}")),
    };
    if let Err(e) = single_instance::win::mark_window(hwnd, &instance_name) {
        crate::logging::line(&format!("single instance: window not marked: {e}"));
    }
    let strip = match crate::strip_win::create(hwnd, window::window_dpi(hwnd)) {
        Ok(strip) => strip,
        Err(e) => return fail(&format!("tab strip creation failed: {e}")),
    };
    let panel = match crate::panel_win::create(hwnd) {
        Ok(panel) => panel,
        Err(e) => return fail(&format!("failure panel creation failed: {e}")),
    };
    let client = window::client_size(hwnd);
    let presenter = match Presenter::new(hwnd, client, settings.adapter) {
        Ok(p) => p,
        Err(e) => return fail(&format!("Direct3D 11 setup failed: {e}")),
    };
    crate::logging::line(&format!("adapter: {:?}", presenter.adapter()));
    let mut keymap = KeymapTracker::default();
    let first_keymap = keymap.start(&current_klid().unwrap_or_default());
    crate::logging::line(&first_keymap.log);
    let route = Arc::new(InputRoute::new());
    let launched = LaunchBook::default();
    let (clip_side, clip_win) = match start_clipboard(&settings) {
        Some((side, clip_win)) => (Some(side), Some(clip_win)),
        None => (None, None),
    };
    let rules = Rc::new(Rules::new(
        startup.resolved.clone(),
        startup.locks.clone(),
        Rc::clone(startup.config.choices()),
    ));
    let set = SessionSet::new(
        Arc::clone(&route),
        clip_side,
        starter(
            hwnd,
            settings.clone(),
            Rc::clone(&rules),
            log_dir.clone(),
            Rc::clone(&launched),
        ),
    )
    .with_share_policy({
        let rules = Rc::clone(&rules);
        Box::new(move |machine| rules.shares_clipboard(machine))
    });
    let input_options = settings.input_options(String::new());
    let grab = Grab::new(
        hwnd,
        Arc::clone(&route),
        GrabConfig {
            follow: settings.grab_follows,
            timeout_secs: settings.grab_timeout_secs.map(GrabTimeout::get),
            policy: input_options.policy,
            release_chord: input_options.release_chord,
            super_l: settings.super_l,
            local_chords: settings.local_chords.0.clone(),
            switch_chords: settings.switch_chords(),
            test_hooks: TestHooks::from_args(&args),
        },
    );
    hook::set_switch_window(hwnd);
    let mut input = WindowInput::new(
        hwnd,
        grab,
        Arc::clone(&route),
        keymap,
        settings.side_buttons,
    );
    input.note_keymap(first_keymap.log);
    window::install(App::new(AppParts {
        hwnd,
        adapter_choice: settings.adapter,
        presenter,
        tabs,
        set,
        launched,
        files: Files {
            config: startup.config,
            history: startup.history,
        },
        rules,
        chords: ChordsInfo::of(&settings),
        stats_out: args.stats_out.clone(),
        started,
        input,
        strip,
        panel,
        log_dir,
    }));
    if let Err(e) = window::add_menu_item(hwnd) {
        crate::logging::line(&format!("window menu item failed: {e}"));
    }
    window::with_app(|app| {
        app.start_stats();
        app.begin(effects);
    });
    // SAFETY: our window.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
    main_loop();
    let code = match window::uninstall() {
        Some(app) => app.finish(),
        None => fail("the window state vanished"),
    };
    // Every session (and with it every engine thread) is over; the Windows
    // thread is last, so a paste that waits is released first.
    if let Some(clip_win) = clip_win {
        clip_win.stop();
    }
    code
}

/// What starts one session: the same settings for every session, a fresh
/// keymap snapshot, the window's size now, and a waker that names the
/// session. The window's notes for the session go in `launched`.
fn starter(
    hwnd: HWND,
    settings: Settings,
    rules: Rc<Rules>,
    log_dir: PathBuf,
    launched: LaunchBook,
) -> Starter<SessionHandle> {
    Box::new(move |launch: SessionLaunch| {
        let connection = rules.connection(&launch.target);
        let spec = SshSpec {
            ssh: settings.ssh.clone(),
            target: launch.target.clone(),
            server_path: settings.server_path.clone(),
            mode: rules.desktop_mode(&launch.target),
            log_dir: log_dir.clone(),
            log_tag: Some(launch.session.get()),
        };
        validate(&spec).map_err(std::io::Error::other)?;
        let initial = request_for(window::picture_size(hwnd), window::window_dpi(hwnd));
        let keymap = KeymapTracker::default().start(&current_klid().unwrap_or_default());
        let waker = Arc::new(PostWaker::new(hwnd, launch.session));
        let opts = Options {
            spec,
            panel: panel_size(hwnd),
            initial,
            input: settings.input_options(keymap.text),
        };
        let handle = start_with_clipboard(opts, Arc::clone(&waker) as Arc<dyn Waker>, launch.clip)?;
        launched.borrow_mut().insert(
            launch.session,
            Launched {
                initial,
                waker,
                running: connection.running(),
            },
        );
        Ok(handle)
    })
}

/// The clipboard: the Windows thread, the hub that joins it to every
/// session, and the clipboard side the set uses. `None` for `--no-clipboard`
/// or when the thread does not start (the client then runs without a shared
/// clipboard).
fn start_clipboard(settings: &Settings) -> Option<(Box<dyn ClipSide>, ClipWin)> {
    if !settings.share_clipboard {
        crate::logging::line("clipboard: off (share_clipboard)");
        return None;
    }
    let (hub, to_hub) = match ClipHub::new() {
        Ok(made) => made,
        Err(e) => {
            // The kind only: the text of an OS error can name a path.
            crate::logging::line(&format!("clipboard: not started: io_{:?}", e.kind()));
            return None;
        }
    };
    let (clip_win, host) = match clip_win::start(to_hub, settings.clipboard_history) {
        Ok(started) => started,
        Err(e) => {
            // The text is ours: a fixed phrase and a Win32 error number.
            crate::logging::line(&format!("clipboard: not started: {e}"));
            return None;
        }
    };
    hub.attach_windows(host);
    let spool_base = std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("gliff-windows")
        .join("clipboard");
    let rescan = Box::new(clip_win.rescan_fn());
    Some((
        Box::new(HubClip {
            hub,
            spool_base,
            rescan,
        }),
        clip_win,
    ))
}

/// Messages first, then the session, then sleep on messages and (while a
/// present is wanted) the swap chain's waitable.
fn main_loop() {
    let mut msg = MSG::default();
    let started = std::time::Instant::now();
    let mut passes = crate::change::Passes::default();
    loop {
        if let Some(n) = passes.pass(started.elapsed().as_millis() as u64)
            && n > crate::change::BUSY_PASSES
        {
            crate::logging::line(&format!(
                "ui loop: busy, {n} passes in {} s (it should sleep)",
                crate::change::PASS_WINDOW_MS / 1000
            ));
        }
        // SAFETY: a live MSG; all windows of this thread.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                return;
            }
            // SAFETY: a message just retrieved.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let handle = window::with_app(|app| {
            app.pump();
            app.wait_handle()
        })
        .flatten();
        let handles: Vec<HANDLE> = handle.into_iter().collect();
        // SAFETY: the handles are the presenter's live waitable, owned by
        // the installed App, which only this thread touches.
        let woke = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&handles),
                IDLE_WAIT_MS,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if !handles.is_empty() && woke == WAIT_OBJECT_0 {
            window::with_app(App::on_waitable);
        }
    }
}

fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: plain Win32 setup; every pointer is to a live local or a
    // static string, and the window procedure is ours.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window::wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let dpi = GetDpiForSystem();
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: INITIAL_CLIENT.0 * dpi as i32 / 96,
            // The picture gets the initial size; the strip sits above it.
            bottom: INITIAL_CLIENT.1 * dpi as i32 / 96 + Metrics::at(dpi).strip_height,
        };
        AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi,
        )?;
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            w!("gliff"),
            // The swap chain paints the whole client; the strip child must be
            // clipped out of it or the picture covers the strip.
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

/// The monitor's full size, advertised as the panel.
fn panel_size(hwnd: HWND) -> (u32, u32) {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: our window; a live MONITORINFO with cbSize set.
    let ok = unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        GetMonitorInfoW(monitor, &mut info).as_bool()
    };
    if !ok {
        return (0, 0);
    }
    let r = info.rcMonitor;
    (
        u32::try_from(r.right - r.left).unwrap_or(0),
        u32::try_from(r.bottom - r.top).unwrap_or(0),
    )
}

fn fail(message: &str) -> ExitCode {
    crate::logging::line(message);
    eprintln!("gliff-windows: {message}");
    ExitCode::from(1)
}
