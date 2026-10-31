//! The real [`Harness`]: runs `gliff-windows.exe` and acts on its window
//! the way a user would (Windows only).

use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::bail;
use gliff_sw::BgraFrame;
use gliff_windows::cli::DesktopMode;
use gliff_windows::history::HostTarget;
use gliff_windows::ssh::SshSpec;
use gliff_windows::stats_out::ClientStats;
use gliff_windows::strip_win::height_of;
use gliff_windows::window::CLASS_NAME;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::BI_RGB;
use windows::Win32::Graphics::Gdi::BITMAPINFO;
use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
use windows::Win32::Graphics::Gdi::BitBlt;
use windows::Win32::Graphics::Gdi::CAPTUREBLT;
use windows::Win32::Graphics::Gdi::CreateCompatibleBitmap;
use windows::Win32::Graphics::Gdi::CreateCompatibleDC;
use windows::Win32::Graphics::Gdi::DIB_RGB_COLORS;
use windows::Win32::Graphics::Gdi::DeleteDC;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Graphics::Gdi::GetDC;
use windows::Win32::Graphics::Gdi::GetDIBits;
use windows::Win32::Graphics::Gdi::GetMonitorInfoW;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows::Win32::Graphics::Gdi::MONITORINFO;
use windows::Win32::Graphics::Gdi::MonitorFromWindow;
use windows::Win32::Graphics::Gdi::ReleaseDC;
use windows::Win32::Graphics::Gdi::SRCCOPY;
use windows::Win32::Graphics::Gdi::SelectObject;
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_F11;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE;
use windows::Win32::UI::WindowsAndMessaging::GWL_STYLE;
use windows::Win32::UI::WindowsAndMessaging::GetWindowLongW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::core::PCWSTR;

use super::judge::ClientExit;
use super::run::DamageSource;
use super::run::Harness;
use crate::placement;
use crate::placement::PLACEMENT_COMMAND;
use crate::stream_check::PlacementProbe;
use crate::strip_offset::client_height_for;

/// How `gliff-windows.exe` is started.
#[derive(Debug, Clone)]
pub struct Launch {
    pub client: PathBuf,
    pub host: HostTarget,
    pub server_path: String,
    pub ssh: PathBuf,
    pub log_dir: PathBuf,
    pub stats_out: PathBuf,
    /// Whether [`Harness::damage_source`] asks hyprctl on the host.
    pub placement: PlacementProbe,
}

/// How long the hyprctl probe may take (as the stream gate's).
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Win32Harness {
    launch: Launch,
    child: Option<Child>,
    hwnd: Option<HWND>,
}

impl Win32Harness {
    /// Makes this process per-monitor DPI aware, so every rect and the
    /// screen read-back are in physical pixels like the client's own.
    pub fn new(launch: Launch) -> anyhow::Result<Self> {
        // SAFETY: no arguments to get wrong; it fails if already set.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
            .context("SetProcessDpiAwarenessContext(per-monitor v2)")?;
        Ok(Self {
            launch,
            child: None,
            hwnd: None,
        })
    }

    fn hwnd(&self) -> anyhow::Result<HWND> {
        let Some(hwnd) = self.hwnd else {
            bail!("no client window");
        };
        // SAFETY: IsWindow accepts any handle value.
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            bail!("the client window is gone");
        }
        Ok(hwnd)
    }

    fn post(&self, message: u32, wparam: usize, lparam: isize) -> anyhow::Result<()> {
        let hwnd = self.hwnd()?;
        // SAFETY: posting copies the two integers; nothing is borrowed.
        unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(lparam)) }
            .with_context(|| format!("PostMessageW({message:#x})"))
    }
}

impl Drop for Win32Harness {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Harness for Win32Harness {
    fn launch(&mut self) -> anyhow::Result<()> {
        let l = &self.launch;
        // A stale file from an earlier run must not read as this one.
        match std::fs::remove_file(&l.stats_out) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context(format!("removing {}", l.stats_out.display())),
        }
        let child = crate::client_config::client_command(&l.client, &l.log_dir)
            .context("preparing the client's config folder")?
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
        Ok(())
    }

    fn find_window(&mut self) -> bool {
        if self.hwnd().is_ok() {
            return true;
        }
        let Some(pid) = self.child.as_ref().map(Child::id) else {
            return false;
        };
        let mut after = None;
        loop {
            // SAFETY: a static class name; no window title filter.
            let Ok(hwnd) = (unsafe { FindWindowExW(None, after, CLASS_NAME, PCWSTR::null()) })
            else {
                return false;
            };
            let mut owner = 0;
            // SAFETY: a window handle FindWindowExW just returned; `owner`
            // is a live u32.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            if owner == pid {
                self.hwnd = Some(hwnd);
                return true;
            }
            after = Some(hwnd);
        }
    }

    fn stats(&mut self) -> Option<ClientStats> {
        // The client renames a complete file into place, so a read sees a
        // whole one or none.
        let text = std::fs::read_to_string(&self.launch.stats_out).ok()?;
        ClientStats::from_json(&text).ok()
    }

    fn set_client_size(&mut self, size: (u32, u32)) -> anyhow::Result<()> {
        let hwnd = self.hwnd()?;
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: a live window; `monitor` is a sized MONITORINFO.
        let ok = unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        };
        if !ok.as_bool() {
            bail!("GetMonitorInfoW failed");
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: i32::try_from(size.0).context("width")?,
            // The tab strip sits above the picture: the client area is the
            // picture and the strip.
            bottom: i32::try_from(client_height_for(size.1, height_of(hwnd))).context("height")?,
        };
        // SAFETY: a live window; the style words are plain integers and
        // `rect` is a live RECT.
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
            let ex_style = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
            AdjustWindowRectExForDpi(&mut rect, style, false, ex_style, GetDpiForWindow(hwnd))
        }
        .context("AdjustWindowRectExForDpi")?;
        let work = monitor.rcWork;
        // SAFETY: a live window; plain integer geometry.
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                work.left,
                work.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .context("SetWindowPos")
    }

    fn press_f11(&mut self) -> anyhow::Result<()> {
        // Repeat count 1, bit 30 clear: a fresh press, not auto-repeat.
        self.post(WM_KEYDOWN, usize::from(VK_F11.0), 1)
    }

    fn window_rect(&mut self) -> anyhow::Result<(i32, i32, u32, u32)> {
        let hwnd = self.hwnd()?;
        let mut r = RECT::default();
        // SAFETY: a live window and a live RECT.
        unsafe { GetWindowRect(hwnd, &mut r) }.context("GetWindowRect")?;
        Ok((
            r.left,
            r.top,
            u32::try_from(r.right - r.left).context("rect width")?,
            u32::try_from(r.bottom - r.top).context("rect height")?,
        ))
    }

    fn capture(&mut self) -> anyhow::Result<BgraFrame> {
        let hwnd = self.hwnd()?;
        // Best effort: Windows may refuse focus to a background process,
        // and a covered window then fails `pattern`, which is the point.
        // SAFETY: a live window.
        let _ = unsafe { SetForegroundWindow(hwnd) };
        std::thread::sleep(Duration::from_millis(500));
        let (x, y, width, height) = self.window_rect()?;
        screen_pixels(x, y, width, height)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        self.post(WM_CLOSE, 0, 0)
    }

    fn wait_exit(&mut self, timeout: Duration) -> ClientExit {
        let Some(child) = self.child.as_mut() else {
            return ClientExit::Exited(None);
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return ClientExit::Exited(status.code()),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return ClientExit::Killed;
                }
            }
        }
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }

    /// A second ssh to the same host runs [`PLACEMENT_COMMAND`], as the
    /// stream gate does; the session's output is the one gliff-* output.
    fn damage_source(&mut self) -> DamageSource {
        if self.launch.placement == PlacementProbe::Skip {
            return DamageSource::Unchecked;
        }
        let spec = SshSpec {
            ssh: self.launch.ssh.clone(),
            target: self.launch.host.clone(),
            server_path: self.launch.server_path.clone(),
            mode: DesktopMode::Headless,
            log_dir: self.launch.log_dir.clone(),
            log_tag: None,
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(r) => r,
            Err(e) => return DamageSource::Missing(format!("no runtime for the probe: {e}")),
        };
        match runtime.block_on(gliff_windows::ssh::exec(
            &spec,
            PLACEMENT_COMMAND,
            PROBE_TIMEOUT,
        )) {
            Ok(out) => {
                let seen = placement::judge_session(&out.stdout);
                if seen.is_ok() {
                    DamageSource::Covers(seen.describe())
                } else {
                    DamageSource::Missing(seen.describe())
                }
            }
            Err(e) => DamageSource::Missing(format!("hyprctl over ssh: {e}")),
        }
    }
}

/// The screen rect read back as top-down BGRA (what the user sees,
/// layered windows included).
fn screen_pixels(x: i32, y: i32, width: u32, height: u32) -> anyhow::Result<BgraFrame> {
    let w = i32::try_from(width).context("capture width")?;
    let h = i32::try_from(height).context("capture height")?;
    if w == 0 || h == 0 {
        bail!("empty window rect {width}x{height}");
    }
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative: top-down rows.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: every GDI object made here is released before returning;
    // `pixels` holds exactly `h` rows of `w` 32-bit pixels, matching
    // `info`.
    let (copied, rows) = unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(memory, bitmap.into());
        let copied = BitBlt(memory, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        SelectObject(memory, old);
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        (copied, rows)
    };
    copied.context("BitBlt from the screen")?;
    if rows != h {
        bail!("GetDIBits copied {rows} of {h} rows");
    }
    // GDI leaves alpha undefined.
    for px in pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    Ok(BgraFrame {
        width,
        height,
        pixels,
    })
}
