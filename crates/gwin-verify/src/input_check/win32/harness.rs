//! [`Harness`] on the real machine.

use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use anyhow::Context;
use anyhow::bail;
use gliff_windows::history::HostTarget;
use gliff_windows::layout::remote_to_client;
use gliff_windows::session::Geometry;
use gliff_windows::stats_out::ClientStats;
use gliff_windows::strip_win::height_of;
use gliff_windows::window::CLASS_NAME;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows::Win32::Graphics::Gdi::MONITORINFO;
use windows::Win32::Graphics::Gdi::MonitorFromWindow;
use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::Input::KeyboardAndMouse::KLF_NOTELLSHELL;
use windows::Win32::UI::Input::KeyboardAndMouse::LoadKeyboardLayoutW;
use windows::Win32::UI::Input::KeyboardAndMouse::UnloadKeyboardLayout;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::WindowsAndMessaging::GA_ROOT;
use windows::Win32::UI::WindowsAndMessaging::GetAncestor;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::SPI_GETDEFAULTINPUTLANG;
use windows::Win32::UI::WindowsAndMessaging::SPI_SETDEFAULTINPUTLANG;
use windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::Win32::UI::WindowsAndMessaging::WindowFromPoint;
use windows::core::HSTRING;
use windows::core::PCWSTR;

use super::input;
use super::sink::Sink;
use crate::input_check::cases::Act;
use crate::input_check::cases::LayoutStep;
use crate::input_check::cases::Place;
use crate::input_check::cases::Window;
use crate::input_check::fresh::wait_modified_after;
use crate::input_check::placement;
use crate::input_check::remote_log::Entry;
use crate::input_check::remote_log::parse;
use crate::input_check::run::Harness;
use crate::input_check::run::Seen;
use crate::strip_offset::picture_in_client;

/// The handles of the layouts installed for this session.
fn installed_layouts() -> Vec<isize> {
    // SAFETY: a null list asks for the count; the second call fills a buffer
    // of exactly that many.
    unsafe {
        let n = GetKeyboardLayoutList(None);
        let mut list = vec![HKL::default(); usize::try_from(n).unwrap_or(0)];
        let got = GetKeyboardLayoutList(Some(&mut list));
        list.truncate(usize::try_from(got).unwrap_or(0));
        list.iter().map(|h| h.0 as isize).collect()
    }
}

/// The session's default input language: what a client started now begins with.
fn default_input_language() -> Option<HKL> {
    let mut hkl = HKL::default();
    // SAFETY: `hkl` is the HKL the call writes.
    unsafe {
        SystemParametersInfoW(
            SPI_GETDEFAULTINPUTLANG,
            0,
            Some(std::ptr::from_mut(&mut hkl).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()
    .map(|()| hkl)
}

fn set_default_input_language(mut hkl: HKL) {
    // SAFETY: `hkl` is the HKL the call reads.
    let _ = unsafe {
        SystemParametersInfoW(
            SPI_SETDEFAULTINPUTLANG,
            0,
            Some(std::ptr::from_mut(&mut hkl).cast()),
            SPIF_SENDCHANGE,
        )
    };
}

/// `WM_INPUTLANGCHANGEREQUEST`.
const WM_INPUTLANGCHANGEREQUEST: u32 = 0x0050;

/// How the client and the remote machine are reached.
#[derive(Debug, Clone)]
pub struct Launch {
    pub client: PathBuf,
    pub host: HostTarget,
    pub server_path: String,
    pub ssh: PathBuf,
    pub log_dir: PathBuf,
    pub stats_out: PathBuf,
    /// The remote machine's `gwin-keys` command, run as `<cmd> log` over ssh.
    pub remote_keys: String,
    /// Wait for Enter at each lock-step press instead of the countdown.
    pub prompt_enter: bool,
}

/// How long `observe` waits for a stats file written after it asked: a bit
/// more than the client's one second between writes.
const STATS_FRESH: Duration = Duration::from_millis(2500);

/// How long the pointer stays where `pointer` put it before the next act: the
/// client learns of a leave or an enter from window messages Windows sends
/// after the move, not during `SetCursorPos`.
const POINTER_SETTLE: Duration = Duration::from_millis(250);

/// How long a client gets to show its first picture.
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// The client area's screen origin and size (physical pixels), and its dpi.
struct ClientArea {
    origin: (i32, i32),
    size: (u32, u32),
    dpi: u32,
}

pub struct Win32Harness {
    pub(super) launch: Launch,
    pub(super) child: Option<Child>,
    pub(super) hwnd: Option<HWND>,
    /// `CabinetWClass` windows that were open before Win+E (see L4).
    pub(super) explorer_before: Vec<isize>,
    /// The poller that watches for a new Explorer window from `note_explorer_windows`
    /// until `explorer_opened_and_closed`.
    pub(super) explorer_watch: Option<super::lockstep::ExplorerWatch>,
    sink: Option<Sink>,
    /// Layouts this run knows by KLID, and whether it loaded each one (and so
    /// unloads it at the end; a layout the machine already had stays).
    layouts: Vec<(String, HKL, bool)>,
    /// The session's default input language before the run changed it;
    /// put back at the end, even when the run fails.
    original_default: Option<HKL>,
    /// Diagnosis files written so far, for their names.
    diag_files: u32,
    pub(super) locked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(super) stop_poller: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Win32Harness {
    /// Makes this process per-monitor DPI aware, so every point is in
    /// physical pixels like the client's own.
    pub fn new(launch: Launch) -> anyhow::Result<Self> {
        // SAFETY: no arguments to get wrong.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
            .context("SetProcessDpiAwarenessContext(per-monitor v2)")?;
        let locked = std::sync::Arc::default();
        let stop_poller = std::sync::Arc::default();
        super::lockstep::watch_session(
            std::sync::Arc::clone(&locked),
            std::sync::Arc::clone(&stop_poller),
        );
        Ok(Self {
            launch,
            child: None,
            hwnd: None,
            explorer_before: Vec::new(),
            explorer_watch: None,
            sink: None,
            layouts: Vec::new(),
            original_default: None,
            diag_files: 0,
            locked,
            stop_poller,
        })
    }

    pub(super) fn hwnd(&self) -> anyhow::Result<HWND> {
        let Some(hwnd) = self.hwnd else {
            bail!("no client window");
        };
        // SAFETY: IsWindow accepts any handle value.
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            bail!("the client window is gone");
        }
        Ok(hwnd)
    }

    pub(super) fn stats(&self) -> Option<ClientStats> {
        let text = std::fs::read_to_string(&self.launch.stats_out).ok()?;
        ClientStats::from_json(&text).ok()
    }

    /// Starts the client and waits for its window and a first picture.
    pub(super) fn launch_client(&mut self, flags: &[String]) -> anyhow::Result<()> {
        self.stop();
        self.require_remote_submap()?;
        self.save_remote_diag("remote-diag-before-start");
        let l = &self.launch;
        match std::fs::remove_file(&l.stats_out) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context(format!("removing {}", l.stats_out.display())),
        }
        let child = crate::client_config::client_command(&l.client, &l.log_dir)
            .context("preparing the client's config folder")?
            .args(crate::input_check::run::gate_flags(flags))
            .arg("--stats-out")
            .arg(&l.stats_out)
            .arg("--server-path")
            .arg(&l.server_path)
            .arg("--ssh")
            .arg(&l.ssh)
            .arg("--log-dir")
            .arg(&l.log_dir)
            .arg("--")
            .arg(l.host.as_str())
            .spawn()
            .with_context(|| format!("starting {}", l.client.display()))?;
        self.child = Some(child);
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.hwnd.is_none() {
                self.hwnd = self.find_window();
            }
            let shown = self.stats().is_some_and(|s| s.input.geometry.is_some());
            if let Some(hwnd) = self.hwnd
                && shown
            {
                // A new window opens at the next cascade step, which on a
                // tall window ends under the taskbar after a few restarts.
                // The same corner each time keeps every click point on the
                // client.
                move_to_work_area_corner(hwnd);
                // Let the first mouse move and the hook settle.
                std::thread::sleep(Duration::from_millis(500));
                self.save_remote_diag("remote-diag-after-start");
                return Ok(());
            }
            if let Some(child) = self.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                bail!("the client exited at start ({status})");
            }
            if Instant::now() >= deadline {
                bail!("no picture within {} s", START_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn find_window(&self) -> Option<HWND> {
        let pid = self.child.as_ref()?.id();
        let mut after = None;
        loop {
            // SAFETY: a static class name; no window title filter.
            let hwnd = unsafe { FindWindowExW(None, after, CLASS_NAME, PCWSTR::null()) }.ok()?;
            let mut owner = 0;
            // SAFETY: a window handle FindWindowExW just returned; `owner`
            // is a live u32.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            if owner == pid {
                return Some(hwnd);
            }
            after = Some(hwnd);
        }
    }

    /// Ends the client the way a user would, then forces it.
    pub(super) fn stop(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            // SAFETY: posting copies the integers.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        let Some(mut child) = self.child.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn client_area(&self) -> anyhow::Result<ClientArea> {
        let hwnd = self.hwnd()?;
        let mut rect = RECT::default();
        // SAFETY: a live window and a live RECT.
        unsafe { GetClientRect(hwnd, &mut rect) }.context("GetClientRect")?;
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: a live window and a live POINT.
        if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
            bail!("ClientToScreen failed");
        }
        // SAFETY: a live window.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        // The tab strip is the top of the client area; the picture, which
        // every position in the checks is relative to, is the rest.
        let (origin, size) = picture_in_client(
            (origin.x, origin.y),
            (
                u32::try_from(rect.right - rect.left).context("client width")?,
                u32::try_from(rect.bottom - rect.top).context("client height")?,
            ),
            height_of(hwnd),
        );
        Ok(ClientArea { origin, size, dpi })
    }

    pub(super) fn focus(&mut self, window: Window) -> anyhow::Result<()> {
        match window {
            Window::Client => {
                let hwnd = self.hwnd()?;
                if !super::foreground::force(hwnd) {
                    bail!("Windows would not make the client the foreground window");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Window::Notepad => {
                let sink = self.sink()?;
                if !sink.focus() {
                    bail!("Windows would not make the text sink the foreground window");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        Ok(())
    }

    /// The text sink (created once): beside the client, clear of its frame,
    /// so that the pointer on the sink is off the client and not merely
    /// covered. Over the client's top-left corner only when the client leaves
    /// no room on the monitor.
    fn sink(&mut self) -> anyhow::Result<&Sink> {
        if self.sink.is_none() {
            self.sink = Some(Sink::open()?);
        }
        let (x, y) = match self.sink_beside_client() {
            Some(at) => at,
            None => {
                eprintln!("   (no room beside the client: the text sink goes over its corner)");
                let (x, y) = self.client_area()?.origin;
                (x + 4, y + 4)
            }
        };
        let sink = self.sink.as_ref().context("text sink")?;
        sink.place_over(x, y)?;
        Ok(sink)
    }

    fn sink_beside_client(&self) -> Option<(i32, i32)> {
        let hwnd = self.hwnd?;
        let mut frame = RECT::default();
        // SAFETY: a live window and a live RECT.
        unsafe { GetWindowRect(hwnd, &mut frame) }.ok()?;
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        // SAFETY: a live window; `info` has its size set.
        if !unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }
            .as_bool()
        {
            return None;
        }
        let rect = |r: RECT| placement::Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        placement::beside(rect(frame), rect(info.rcWork), super::sink::SIZE)
    }

    pub(super) fn pointer(&mut self, place: Place) -> anyhow::Result<()> {
        let (x, y) = match place {
            Place::OverClient => {
                let area = self.client_area()?;
                (
                    area.origin.0 + (area.size.0 / 2) as i32,
                    area.origin.1 + (area.size.1 / 2) as i32,
                )
            }
            Place::OffClient => self.sink()?.center(),
        };
        if place == Place::OffClient {
            self.require_not_client_at(x, y)?;
        }
        // `SetCursorPos` moves the pointer but Windows may not work out which
        // window it is over (and so send `WM_MOUSELEAVE` or `WM_MOUSEMOVE`)
        // until a real mouse event comes: one pixel out and back as input
        // does that, and the waits give the messages time to arrive before
        // the next key does. (The client also polls the pointer every tick.)
        input::cursor_to(x, y)?;
        std::thread::sleep(POINTER_SETTLE);
        input::nudge()?;
        input::cursor_to(x, y)?;
        std::thread::sleep(POINTER_SETTLE);
        if place == Place::OverClient {
            self.require_client_at(x, y)?;
        }
        Ok(())
    }

    /// Fails when the client is the window at this screen point: a pointer
    /// put here is not off the client.
    fn require_not_client_at(&self, x: i32, y: i32) -> anyhow::Result<()> {
        let hwnd = self.hwnd()?;
        // SAFETY: plain coordinates.
        let under = unsafe { WindowFromPoint(POINT { x, y }) };
        // SAFETY: a window handle, or null, which GetAncestor accepts.
        if unsafe { GetAncestor(under, GA_ROOT) } == hwnd {
            bail!("the point {x},{y} meant to be off the client is over it");
        }
        Ok(())
    }

    pub(super) fn foreground_is_client(&self) -> bool {
        // SAFETY: no arguments.
        let fg = unsafe { GetForegroundWindow() };
        self.hwnd.is_some_and(|h| h == fg)
    }

    fn layout(&mut self, step: LayoutStep) -> anyhow::Result<()> {
        match step {
            LayoutStep::Load(klid) => {
                let id = HSTRING::from(klid);
                self.remember_default();
                let before = installed_layouts();
                // SAFETY: a NUL-terminated wide string that outlives the call.
                let hkl = unsafe { LoadKeyboardLayoutW(PCWSTR(id.as_ptr()), KLF_NOTELLSHELL) }
                    .with_context(|| format!("LoadKeyboardLayoutW({klid})"))?;
                let ours = !before.contains(&(hkl.0 as isize));
                self.layouts.retain(|(k, _, _)| k != klid);
                self.layouts.push((klid.to_owned(), hkl, ours));
            }
            LayoutStep::Activate(klid) => {
                let hkl = self.hkl(klid)?;
                let hwnd = self.hwnd()?;
                // SAFETY: posting copies the integers.
                unsafe {
                    PostMessageW(
                        Some(hwnd),
                        WM_INPUTLANGCHANGEREQUEST,
                        WPARAM(0),
                        LPARAM(hkl.0 as isize),
                    )
                }
                .context("PostMessageW(WM_INPUTLANGCHANGEREQUEST)")?;
                std::thread::sleep(Duration::from_millis(400));
            }
            LayoutStep::Unload(klid) => {
                let hkl = self.hkl(klid)?;
                let ours = self.layouts.iter().any(|(k, _, o)| k == klid && *o);
                if ours {
                    // SAFETY: a handle LoadKeyboardLayoutW returned.
                    unsafe { UnloadKeyboardLayout(hkl) }
                        .with_context(|| format!("UnloadKeyboardLayout({klid})"))?;
                }
                self.layouts.retain(|(k, _, _)| k != klid);
            }
            LayoutStep::Restore => self.restore_default(),
        }
        Ok(())
    }

    /// Notes the session's default input language the first time the run is
    /// about to change the layout.
    fn remember_default(&mut self) {
        if self.original_default.is_none() {
            self.original_default = default_input_language();
        }
    }

    /// Puts the session's default input language (what a new client starts
    /// with) and the client's own layout back to what they were.
    fn restore_default(&mut self) {
        let Some(original) = self.original_default else {
            return;
        };
        set_default_input_language(original);
        if let Ok(hwnd) = self.hwnd() {
            // SAFETY: posting copies the integers.
            let _ = unsafe {
                PostMessageW(
                    Some(hwnd),
                    WM_INPUTLANGCHANGEREQUEST,
                    WPARAM(0),
                    LPARAM(original.0 as isize),
                )
            };
            std::thread::sleep(Duration::from_millis(400));
        }
    }

    fn hkl(&self, klid: &str) -> anyhow::Result<HKL> {
        self.layouts
            .iter()
            .find(|(k, _, _)| k == klid)
            .map(|(_, h, _)| *h)
            .with_context(|| format!("layout {klid} was not loaded by this run"))
    }

    fn click(&mut self, remote: (f64, f64)) -> anyhow::Result<()> {
        let stats = self.stats().context("no stats file yet")?;
        let g = stats
            .input
            .geometry
            .context("the client has shown no picture")?;
        let geometry = Geometry {
            stream: g.stream,
            view: g.view,
            scale_milli: g.scale_milli,
        };
        let area = self.client_area()?;
        let (cx, cy) = remote_to_client(&geometry, area.size, area.dpi, remote)
            .context("the window has no area to click in")?;
        let (x, y) = (area.origin.0 + cx, area.origin.1 + cy);
        input::cursor_to(x, y)?;
        std::thread::sleep(Duration::from_millis(60));
        self.require_client_at(x, y)?;
        input::left_click()
    }

    /// Fails when another window (the taskbar, say) is on top of the client
    /// at this screen point: a click there never reaches the client.
    fn require_client_at(&self, x: i32, y: i32) -> anyhow::Result<()> {
        let hwnd = self.hwnd()?;
        // SAFETY: plain coordinates.
        let under = unsafe { WindowFromPoint(POINT { x, y }) };
        // SAFETY: a window handle, or null, which GetAncestor accepts.
        let root = unsafe { GetAncestor(under, GA_ROOT) };
        if root == hwnd {
            return Ok(());
        }
        let mut class = [0u16; 64];
        // SAFETY: a live buffer.
        let n = unsafe { GetClassNameW(root, &mut class) };
        let class = String::from_utf16_lossy(&class[..usize::try_from(n).unwrap_or(0)]);
        bail!("the point {x},{y} is covered by a window of class `{class}`, not the client")
    }

    /// Runs `<remote_keys> <args>` over ssh: whether it succeeded, and its
    /// output (stdout, then stderr).
    pub(super) fn remote_command(&self, args: &[&str]) -> anyhow::Result<(bool, String)> {
        let out = Command::new(&self.launch.ssh)
            .args(crate::target::ssh_command_args(
                self.launch.host.as_str(),
                &[&[self.launch.remote_keys.as_str()], args].concat(),
            ))
            .output()
            .with_context(|| format!("running ssh for `gwin-keys {}`", args.join(" ")))?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.success(), text))
    }

    /// Writes `text` to `<log dir>/<name>-<n>.txt` and returns the path.
    pub(super) fn save_diag(&mut self, name: &str, text: &str) -> PathBuf {
        self.diag_files += 1;
        let path = self
            .launch
            .log_dir
            .join(format!("{name}-{}.txt", self.diag_files));
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("could not write {}: {e}", path.display());
        }
        path
    }

    /// What Hyprland says about focus, saved as `<name>-<n>.txt` (best
    /// effort: a failing remote machine shows in the cases anyway).
    fn save_remote_diag(&mut self, name: &str) {
        if let Ok((_, text)) = self.remote_command(&["diag"]) {
            self.save_diag(name, &text);
        }
    }

    /// The remote machine must be in the test submap before a client grabs: a key
    /// sent to a remote machine outside it would act on the remote machine itself.
    pub(super) fn require_remote_submap(&mut self) -> anyhow::Result<()> {
        let (ok, text) = self.remote_command(&["check"])?;
        if ok {
            return Ok(());
        }
        let path = self.save_diag("remote-submap-check-failed", &text);
        let first = text.lines().next().unwrap_or("no output").trim().to_owned();
        bail!(
            "the remote machine is not in the test submap: {first} (run `gwin-keys arm` there; details in {})",
            path.display()
        );
    }

    /// Asks the remote machine to give its `wev` window the focus; an error carries
    /// what it saw and the file the diagnosis went to.
    fn remote_focus(&mut self) -> anyhow::Result<()> {
        let (ok, text) = self.remote_command(&["focus"])?;
        if ok {
            return Ok(());
        }
        let path = self.save_diag("remote-focus-failed", &text);
        let first = text.lines().next().unwrap_or("no output").trim().to_owned();
        bail!(
            "the remote machine would not give wev the focus: {first} (hyprctl state in {})",
            path.display()
        );
    }

    fn read_remote_log(&self) -> anyhow::Result<Vec<Entry>> {
        let out = Command::new(&self.launch.ssh)
            .args(crate::target::ssh_command_args(
                self.launch.host.as_str(),
                &[self.launch.remote_keys.as_str(), "log"],
            ))
            .output()
            .context("running ssh for the remote log")?;
        if !out.status.success() {
            bail!(
                "`{} log` failed ({}): {}",
                self.launch.remote_keys,
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let parsed = parse(&String::from_utf8_lossy(&out.stdout));
        if parsed.malformed > 0 {
            bail!(
                "{} line(s) of the remote log could not be read",
                parsed.malformed
            );
        }
        Ok(parsed.entries)
    }
}

impl Drop for Win32Harness {
    fn drop(&mut self) {
        self.stop();
        self.stop_poller
            .store(true, std::sync::atomic::Ordering::Release);
        // The default input language goes back first (the client is gone), then
        // the layouts this run loaded; neither may outlive it.
        self.restore_default();
        for (_, hkl, ours) in self.layouts.drain(..) {
            if ours {
                // SAFETY: a handle LoadKeyboardLayoutW returned.
                let _ = unsafe { UnloadKeyboardLayout(hkl) };
            }
        }
    }
}

impl Harness for Win32Harness {
    fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()> {
        self.launch_client(flags)
    }

    fn stop_client(&mut self) {
        self.stop();
        input::release_modifiers("stopping the client");
    }

    fn act(&mut self, act: &Act) -> anyhow::Result<()> {
        match act {
            Act::Keys(sequence) => input::keys(sequence),
            Act::Pause(d) => {
                std::thread::sleep(*d);
                Ok(())
            }
            Act::Focus(window) => self.focus(*window),
            Act::Pointer(place) => self.pointer(*place),
            Act::Layout(step) => self.layout(*step),
            Act::Dismiss => {
                // Only when the sink really has the focus, so Escape cannot
                // go to the grabbed client and on to the remote machine.
                if self.focus(Window::Notepad).is_ok() {
                    input::keys(
                        &crate::input_check::scan::chord_presses("Escape").unwrap_or_default(),
                    )?;
                    std::thread::sleep(Duration::from_millis(150));
                }
                Ok(())
            }
            Act::ClearNotepad => {
                self.sink()?.clear();
                Ok(())
            }
            Act::Click { remote } => self.click(*remote),
            Act::Button(side) => input::button(*side),
            Act::RemoteFocus => self.remote_focus(),
            Act::WheelEvent { wheel, delta } => input::wheel(*wheel, *delta),
        }
    }

    fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>> {
        self.read_remote_log()
    }

    fn observe(&mut self) -> Seen {
        // The stats file is rewritten once a second: wait for a copy written
        // after the case's last act, or the grab it shows may be the one
        // before the case's last key.
        wait_modified_after(&self.launch.stats_out, SystemTime::now(), STATS_FRESH);
        let seen = Seen {
            foreground_is_client: self.foreground_is_client(),
            notepad_text: self.sink.as_ref().map(Sink::text),
            input: self.stats().map(|s| s.input),
        };
        // The case is over: nothing it pressed may stay down for the next.
        input::release_modifiers("a case");
        seen
    }

    fn remote_size(&mut self) -> Option<(u32, u32)> {
        let g = self.stats()?.input.geometry?;
        let scale = f64::from(g.scale_milli.max(1)) / 1000.0;
        Some((
            (f64::from(g.stream.0) / scale).round() as u32,
            (f64::from(g.stream.1) / scale).round() as u32,
        ))
    }
}

/// Moves the window, size and stacking unchanged, to the top-left corner of
/// the work area of the monitor it is on (best effort).
fn move_to_work_area_corner(hwnd: HWND) {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: a live window; `info` is a live MONITORINFO with its size set.
    let ok =
        unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }
            .as_bool();
    if !ok {
        return;
    }
    // SAFETY: a live window.
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            info.rcWork.left,
            info.rcWork.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}
