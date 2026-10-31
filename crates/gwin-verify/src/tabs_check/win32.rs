//! The real [`Harness`]: runs `gliff-windows.exe` with several targets and
//! acts on its window with `SendInput` (Windows only).
//!
//! This grabs the keyboard of whoever is logged in, so it only runs when a
//! person asks for it, with a warning and a countdown, never from a script.

use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::bail;
use gliff_windows::history::HistoryFile;
use gliff_windows::history::HostTarget;
use gliff_windows::stats_out::ClientStats;
use gliff_windows::window::CLASS_NAME;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
use windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::core::PCWSTR;

use super::judge::ClientExit;
use super::run::Harness;
use crate::input_check::scan::chord_presses;
use crate::input_check::win32::input;

/// How the client and the remote machines are reached.
#[derive(Debug, Clone)]
pub struct Launch {
    pub client: PathBuf,
    /// Command-line order is tab order.
    pub hosts: Vec<HostTarget>,
    pub server_path: String,
    pub ssh: PathBuf,
    pub log_dir: PathBuf,
    pub stats_out: PathBuf,
}

pub struct Win32Harness {
    launch: Launch,
    child: Option<Child>,
    hwnd: Option<HWND>,
    config_dir: Option<PathBuf>,
}

impl Win32Harness {
    /// Makes this process per-monitor DPI aware, so every point is in
    /// physical pixels like the client's own.
    pub fn new(launch: Launch) -> anyhow::Result<Self> {
        // SAFETY: no arguments to get wrong.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
            .context("SetProcessDpiAwarenessContext(per-monitor v2)")?;
        Ok(Self {
            launch,
            child: None,
            hwnd: None,
            config_dir: None,
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
}

impl Drop for Win32Harness {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        input::release_modifiers("the end of the tabs check");
    }
}

impl Harness for Win32Harness {
    fn launch(&mut self) -> anyhow::Result<()> {
        let l = &self.launch;
        let _ = std::fs::remove_file(&l.stats_out);
        let config = crate::client_config::fresh_config_dir(&l.log_dir)
            .context("preparing the client's config folder")?;
        let mut command = crate::client_config::client_command(&l.client, &l.log_dir)
            .context("preparing the client's config folder")?;
        command
            .arg("--stats-out")
            .arg(&l.stats_out)
            .arg("--server-path")
            .arg(&l.server_path)
            .arg("--ssh")
            .arg(&l.ssh)
            .arg("--log-dir")
            .arg(&l.log_dir)
            .arg("--");
        for host in &l.hosts {
            command.arg(host.as_str());
        }
        let child = command
            .spawn()
            .with_context(|| format!("starting {}", l.client.display()))?;
        self.child = Some(child);
        self.config_dir = Some(config);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if self.hwnd.is_none() {
                self.hwnd = self.find_window();
            }
            if self.hwnd.is_some() {
                return Ok(());
            }
            if let Some(child) = self.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                bail!("the client exited at start ({status})");
            }
            if Instant::now() >= deadline {
                bail!("no window within 60 s");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn focus(&mut self) -> anyhow::Result<()> {
        let hwnd = self.hwnd()?;
        let mut rect = RECT::default();
        // SAFETY: a live window and a live RECT.
        unsafe { GetClientRect(hwnd, &mut rect) }.context("GetClientRect")?;
        let mut centre = POINT {
            x: (rect.right - rect.left) / 2,
            y: (rect.bottom - rect.top) / 2,
        };
        // SAFETY: a live window and a live POINT.
        if !unsafe { ClientToScreen(hwnd, &mut centre) }.as_bool() {
            bail!("ClientToScreen failed");
        }
        // SAFETY: a live window; the call only asks for the foreground.
        let _ = unsafe { SetForegroundWindow(hwnd) };
        input::cursor_to(centre.x, centre.y)?;
        input::left_click()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(())
    }

    fn chord(&mut self, chord: &str) -> anyhow::Result<()> {
        let presses = chord_presses(chord).map_err(anyhow::Error::msg)?;
        input::keys(&presses)
    }

    fn stats(&mut self) -> Option<ClientStats> {
        let text = std::fs::read_to_string(&self.launch.stats_out).ok()?;
        ClientStats::from_json(&text).ok()
    }

    fn recent_hosts(&mut self) -> Vec<String> {
        let Some(dir) = &self.config_dir else {
            return Vec::new();
        };
        let (file, _warnings) = HistoryFile::load(&dir.join("recent-hosts.toml"), true);
        file.hosts()
            .hosts()
            .iter()
            .map(|h| h.as_str().to_owned())
            .collect()
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let hwnd = self.hwnd()?;
        // SAFETY: posting copies the integers.
        unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .context("PostMessageW(WM_CLOSE)")
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
}
