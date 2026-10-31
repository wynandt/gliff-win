//! [`LockHarness`] on the real machine. Registry access is the client's own
//! read-only view (`WinRegistry`'s two read methods); nothing here writes.

use std::io::BufRead;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::bail;
use gliff_windows::lock_policy::Registry;
use gliff_windows::lock_policy::StoredMarker;
use gliff_windows::lockreg::WinRegistry;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::StationsAndDesktops::CloseDesktop;
use windows::Win32::System::StationsAndDesktops::DESKTOP_CONTROL_FLAGS;
use windows::Win32::System::StationsAndDesktops::DESKTOP_SWITCHDESKTOP;
use windows::Win32::System::StationsAndDesktops::OpenInputDesktop;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::core::w;

use super::harness::Win32Harness;
use super::input;
use super::overlay::Overlay;
use crate::input_check::cases::Place;
use crate::input_check::cases::Window;
use crate::input_check::countdown::overlay_text;
use crate::input_check::countdown::seconds_left;
use crate::input_check::lock::Kill;
use crate::input_check::lock::LockHarness;
use crate::input_check::lock::Reading;
use crate::input_check::remote_log::Entry;
use crate::input_check::scan::chord_presses;

/// `VK_F23`, the client's panic test key.
const VK_F23: u16 = 0x86;

/// Watches whether the input desktop is the user's. While the Windows machine is
/// locked it is not, and this process cannot open it.
pub fn watch_session(locked: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new()
        .name("session-watch".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                // SAFETY: plain flags; the handle is closed below.
                match unsafe {
                    OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP)
                } {
                    Ok(desktop) => {
                        // SAFETY: the handle OpenInputDesktop just returned.
                        let _ = unsafe { CloseDesktop(desktop) };
                    }
                    Err(_) => locked.store(true, Ordering::Release),
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
}

/// Every top-level Explorer window now, as raw handle values.
fn explorer_windows() -> Vec<isize> {
    let mut found = Vec::new();
    let mut previous: Option<HWND> = None;
    // SAFETY: a static class name; each handle passed back is the one the
    // previous call returned.
    while let Ok(hwnd) = unsafe { FindWindowExW(None, previous, w!("CabinetWClass"), None) } {
        found.push(hwnd.0 as isize);
        previous = Some(hwnd);
    }
    found
}

/// Watches for Explorer windows that were not open at the start, from a
/// background thread, so one that opens and goes away during a long step is
/// still counted. Each new window is closed at once.
pub(super) struct ExplorerWatch {
    stop: Arc<AtomicBool>,
    seen: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ExplorerWatch {
    fn start(before: Vec<isize>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, seen) = (Arc::clone(&stop), Arc::clone(&seen));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    for hwnd in super::super::lock::unseen(&before, &explorer_windows()) {
                        seen.store(true, Ordering::Release);
                        // SAFETY: posting copies the integers; the handle came
                        // from FindWindowExW a moment ago and a stale one just
                        // fails.
                        let _ = unsafe {
                            PostMessageW(
                                Some(HWND(hwnd as *mut core::ffi::c_void)),
                                WM_CLOSE,
                                WPARAM(0),
                                LPARAM(0),
                            )
                        };
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
        };
        Self {
            stop,
            seen,
            thread: Some(thread),
        }
    }

    fn seen(&self) -> bool {
        self.seen.load(Ordering::Acquire)
    }

    /// Stops the thread; true when any new window was seen.
    fn finish(mut self) -> bool {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.seen.load(Ordering::Acquire)
    }
}

impl Drop for ExplorerWatch {
    /// A watch that is dropped without `finish` still stops, so it never
    /// goes on closing Explorer windows the maintainer opens.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn image_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "gliff-windows.exe".to_owned())
}

/// Process ids of every running image with this name.
fn pids_named(image: &str) -> Vec<u32> {
    let Ok(out) = Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split("\",\"").nth(1)?.parse().ok())
        .collect()
}

impl LockHarness for Win32Harness {
    fn read_policy(&mut self) -> anyhow::Result<Reading> {
        let mut registry = WinRegistry;
        let value = registry
            .read_value()
            .map_err(|e| anyhow::anyhow!("reading DisableLockWorkstation: {e:?}"))?;
        let marker = registry
            .read_marker()
            .map_err(|e| anyhow::anyhow!("reading the marker: {e:?}"))?
            != StoredMarker::Absent;
        Ok(Reading { value, marker })
    }

    fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()> {
        self.launch_client(flags)
    }

    fn focus(&mut self, window: Window) -> anyhow::Result<()> {
        Win32Harness::focus(self, window)
    }

    fn pointer(&mut self, place: Place) -> anyhow::Result<()> {
        Win32Harness::pointer(self, place)
    }

    fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>> {
        crate::input_check::run::Harness::remote_log(self)
    }

    fn arm_remote(&mut self) -> anyhow::Result<bool> {
        let (ok, text) = self.remote_command(&["check"])?;
        if ok {
            return Ok(false);
        }
        let (armed, said) = self.remote_command(&["arm", "--minutes", "30"])?;
        if !armed {
            let path = self.save_diag("remote-arm-failed", &format!("{text}\n{said}"));
            let first = said.lines().next().unwrap_or("no output").trim().to_owned();
            bail!(
                "the remote machine is not in the test submap and `gwin-keys arm` failed: {first} \
                 (details in {}; `gwin-keys disarm` there and run again)",
                path.display()
            );
        }
        let (ok, text) = self.remote_command(&["check"])?;
        if !ok {
            let _ = self.remote_command(&["disarm"]);
            let path = self.save_diag("remote-arm-unverified", &text);
            let first = text.lines().next().unwrap_or("no output").trim().to_owned();
            bail!(
                "the remote machine armed but is not in the test submap: {first} (details in {})",
                path.display()
            );
        }
        Ok(true)
    }

    fn remote_submap_ok(&mut self) -> bool {
        self.require_remote_submap().is_ok()
    }

    fn disarm_remote(&mut self) {
        if !self.remote_command(&["disarm"]).is_ok_and(|(ok, _)| ok) {
            eprintln!(
                "could not disarm the remote machine's test submap: run `gwin-keys disarm` there"
            );
        }
    }

    fn countdown(&mut self, instruction: &str, secs: u32) {
        eprintln!("\n✋ {instruction} ({secs} s)");
        if self.launch.prompt_enter {
            eprint!("   (Enter when done) ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
            return;
        }
        let overlay = match Overlay::open(&overlay_text(instruction, secs)) {
            Ok(o) => Some(o),
            Err(e) => {
                eprintln!("   (no countdown window: {e:#}; watch this console)");
                None
            }
        };
        let start = Instant::now();
        loop {
            let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            let left = seconds_left(secs, elapsed);
            if let Some(overlay) = &overlay {
                overlay.set_text(&overlay_text(instruction, left));
            }
            if left == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        input::release_modifiers("a step");
    }

    fn release_grab(&mut self) -> anyhow::Result<()> {
        let chord = chord_presses("Ctrl+Alt+Home").map_err(anyhow::Error::msg)?;
        let sent = input::keys(&chord);
        std::thread::sleep(Duration::from_millis(200));
        input::release_modifiers("the release chord");
        sent
    }

    fn wait_unlocked(&mut self, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        loop {
            // SAFETY: plain flags; the handle is closed right away.
            let opened =
                unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) };
            if let Ok(desktop) = opened {
                // SAFETY: the handle OpenInputDesktop just returned.
                let _ = unsafe { CloseDesktop(desktop) };
                // The desktop needs a moment before it takes input again.
                std::thread::sleep(Duration::from_secs(2));
                self.locked.store(false, Ordering::Release);
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn ask(&mut self, text: &str) -> bool {
        eprintln!("\n✋ {text}");
        eprint!("   [y/n] ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        line.trim().eq_ignore_ascii_case("y")
    }

    fn session_active(&mut self) -> bool {
        !self.locked.swap(false, Ordering::AcqRel)
    }

    fn foreground_is_client(&mut self) -> bool {
        Win32Harness::foreground_is_client(self)
    }

    fn client_running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }

    fn watchdog_running(&mut self) -> bool {
        let own = self.child.as_ref().map(std::process::Child::id);
        pids_named(&image_name(&self.launch.client))
            .into_iter()
            .any(|pid| Some(pid) != own)
    }

    fn close_client(&mut self) -> anyhow::Result<()> {
        self.stop();
        Ok(())
    }

    fn panic_client(&mut self) -> anyhow::Result<()> {
        input::vk_tap(VK_F23)?;
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        // The panic ends the process; give it a moment.
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn kill(&mut self, how: Kill) -> anyhow::Result<()> {
        let pid = self.child.as_ref().context("no client to kill")?.id();
        let mut cmd = Command::new("taskkill");
        cmd.arg("/F");
        if how == Kill::Tree {
            cmd.arg("/T");
        }
        let status = cmd
            .args(["/PID", &pid.to_string()])
            .output()
            .context("running taskkill")?
            .status;
        if !status.success() {
            anyhow::bail!("taskkill {how:?} exited {status}");
        }
        self.hwnd = None;
        // Reap the child and let go of its handle. While this process holds
        // it, Windows keeps the dead client's process object, and a check
        // that asks "is the owner still there" by opening it by pid would
        // find it.
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        Ok(())
    }

    fn restore_flag(&mut self) -> anyhow::Result<()> {
        let out = Command::new(&self.launch.client)
            .arg("--restore-lock-policy")
            .output()
            .context("running --restore-lock-policy")?;
        // Anything the helper printed goes in the run's output. (What it
        // restored, or found alive, is in the client log.)
        for line in String::from_utf8_lossy(&out.stdout)
            .lines()
            .chain(String::from_utf8_lossy(&out.stderr).lines())
        {
            println!("  --restore-lock-policy: {line}");
        }
        if !out.status.success() {
            anyhow::bail!("--restore-lock-policy exited {}", out.status);
        }
        Ok(())
    }

    fn note_explorer_windows(&mut self) {
        self.explorer_before = explorer_windows();
        // A watch left from an earlier call is stopped first.
        if let Some(old) = self.explorer_watch.take() {
            old.finish();
        }
        self.explorer_watch = Some(ExplorerWatch::start(self.explorer_before.clone()));
    }

    fn explorer_opened_and_closed(&mut self) -> bool {
        let Some(watch) = self.explorer_watch.take() else {
            return false;
        };
        // A short grace: a window that is still opening when the step ends.
        let deadline = Instant::now() + Duration::from_secs(1);
        while !watch.seen() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        watch.finish()
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}
