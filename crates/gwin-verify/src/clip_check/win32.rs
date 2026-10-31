//! The real machines for `gwin-verify clipboard`: the Windows clipboard of
//! this process's session, the client as a child process, and the remote
//! machine over ssh. Windows only.

use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::bail;
use gliff_windows::history::HostTarget;
use gliff_windows::window::CLASS_NAME;
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::DataExchange::CloseClipboard;
use windows::Win32::System::DataExchange::EmptyClipboard;
use windows::Win32::System::DataExchange::GetClipboardData;
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::DataExchange::OpenClipboard;
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::DataExchange::SetClipboardData;
use windows::Win32::System::Memory::GMEM_MOVEABLE;
use windows::Win32::System::Memory::GlobalAlloc;
use windows::Win32::System::Memory::GlobalLock;
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::GlobalUnlock;
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::System::Ole::CF_DIBV5;
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::core::PCWSTR;

use super::Harness;
use super::WindowsForm;
use super::picture::Picture;
use super::remote::Remote;
use super::tree::Found;
use super::tree::Spec;
use super::tree::hdrop;
use super::tree::list_disk;
use super::tree::parse_hdrop;

/// Where and how the client runs.
pub struct Launch {
    pub client: PathBuf,
    pub host: HostTarget,
    pub server_path: String,
    pub ssh: PathBuf,
    pub log_dir: PathBuf,
}

pub struct Win32Harness {
    launch: Launch,
    remote: Remote,
    child: Option<Child>,
}

impl Win32Harness {
    pub fn new(launch: Launch) -> Self {
        let remote = Remote {
            ssh: launch.ssh.clone(),
            target: launch.host.to_string(),
            scratch: super::remote::DEFAULT_SCRATCH.to_owned(),
        };
        Self {
            launch,
            remote,
            child: None,
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
    }
}

/// The clipboard, opened for this process and closed on drop.
struct Open;

impl Open {
    /// Other programs hold the clipboard briefly; try for about a second.
    fn new() -> anyhow::Result<Self> {
        for _ in 0..50 {
            // SAFETY: no owner window: the clipboard is held by this thread.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Self);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        bail!("the Windows clipboard stayed busy");
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard.
        let _ = unsafe { CloseClipboard() };
    }
}

fn set_text(text: &str) -> anyhow::Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * size_of::<u16>();
    let _open = Open::new()?;
    // SAFETY: the clipboard is open; the memory is filled before it is
    // handed over, and freed when the hand-over fails.
    unsafe {
        EmptyClipboard().context("EmptyClipboard")?;
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes).context("GlobalAlloc")?;
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer.cast::<u16>(), wide.len());
        let _ = GlobalUnlock(memory);
        if let Err(e) = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(e).context("SetClipboardData");
        }
    }
    Ok(())
}

/// Puts `bytes` on the open clipboard as `format`.
fn put(format: u32, bytes: &[u8]) -> anyhow::Result<()> {
    // SAFETY: the clipboard is open; the memory is filled before it is
    // handed over, and freed when the hand-over fails.
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).context("GlobalAlloc")?;
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(memory);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(e).context("SetClipboardData");
        }
    }
    Ok(())
}

/// The registered format called `name`.
fn registered(name: &str) -> anyhow::Result<u32> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
    if id == 0 {
        bail!("RegisterClipboardFormat {name}");
    }
    Ok(id)
}

/// The bytes the open clipboard holds as `format`; `None` when it has none.
fn get(format: u32) -> anyhow::Result<Option<Vec<u8>>> {
    // SAFETY: the clipboard is open; the memory is the clipboard's own and
    // is locked only while it is copied.
    unsafe {
        if IsClipboardFormatAvailable(format).is_err() {
            return Ok(None);
        }
        let handle = GetClipboardData(format).context("GetClipboardData")?;
        let memory = HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            bail!("GlobalLock failed");
        }
        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(memory);
        Ok(Some(bytes))
    }
}

fn set_picture(picture: &Picture, form: WindowsForm) -> anyhow::Result<()> {
    let dib = picture.to_dib();
    let png = match form {
        WindowsForm::Dib => None,
        WindowsForm::PngAndDib => Some((registered("PNG")?, picture.to_png()?)),
    };
    let _open = Open::new()?;
    // SAFETY: the clipboard is open.
    unsafe { EmptyClipboard() }.context("EmptyClipboard")?;
    if let Some((id, png)) = png {
        put(id, &png)?;
    }
    put(u32::from(CF_DIB.0), &dib)
}

/// The picture on the clipboard, from the V5 bitmap, which Windows makes
/// from any other bitmap format; the plain bitmap when there is none.
fn picture() -> anyhow::Result<Option<Picture>> {
    let _open = Open::new()?;
    for format in [CF_DIBV5, CF_DIB] {
        if let Some(dib) = get(u32::from(format.0))? {
            return Picture::from_dib(&dib).map(Some);
        }
    }
    Ok(None)
}

/// The history format is there and zero.
fn history_excluded() -> anyhow::Result<bool> {
    let id = registered(gliff_windows::clip_formats::HISTORY_FORMAT)?;
    let _open = Open::new()?;
    Ok(get(id)?.is_some_and(|b| b.len() >= 4 && b[..4] == [0; 4]))
}

/// Where the gate makes the trees it copies from Windows.
fn files_dir() -> PathBuf {
    std::env::temp_dir().join("gwin-verify-clip-files")
}

/// Makes the tree and copies its top-level entries as files, as Explorer
/// does: `CF_HDROP` and a `Preferred DropEffect` of copy.
fn set_files(spec: &Spec) -> anyhow::Result<()> {
    let dir = files_dir();
    remove_files_dir()?;
    spec.write_to(&dir).context("making the tree")?;
    let paths: Vec<String> = spec
        .roots()
        .iter()
        .map(|root| dir.join(root).to_string_lossy().into_owned())
        .collect();
    let effect = registered("Preferred DropEffect")?;
    let _open = Open::new()?;
    // SAFETY: the clipboard is open.
    unsafe { EmptyClipboard() }.context("EmptyClipboard")?;
    put(u32::from(CF_HDROP.0), &hdrop(&paths))?;
    put(effect, &1u32.to_le_bytes())
}

fn remove_files_dir() -> anyhow::Result<()> {
    match std::fs::remove_dir_all(files_dir()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).context("removing the tree"),
        _ => Ok(()),
    }
}

/// The files on the clipboard, walked. Asking for the list makes the
/// client's delayed render run, so a paste of a pasted tree is waited for
/// here as it is in Explorer.
fn files() -> anyhow::Result<Option<Vec<Found>>> {
    let block = {
        let _open = Open::new()?;
        get(u32::from(CF_HDROP.0))?
    };
    let Some(block) = block else {
        return Ok(None);
    };
    let paths = parse_hdrop(&block).context("CF_HDROP is not a list of wide paths")?;
    let roots: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    list_disk(&roots).map(Some)
}

/// The directories a client process left in its spool.
fn spool_left(pid: u32) -> anyhow::Result<Vec<String>> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("gliff-windows")
        .join("clipboard");
    let prefix = format!("{pid}-");
    let entries = match std::fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", base.display())),
    };
    let mut left = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) {
            left.push(name);
        }
    }
    left.sort();
    Ok(left)
}

fn text() -> anyhow::Result<Option<String>> {
    let _open = Open::new()?;
    let id = u32::from(CF_UNICODETEXT.0);
    // SAFETY: the clipboard is open; the memory is the clipboard's own and
    // is locked only while it is copied.
    unsafe {
        if IsClipboardFormatAvailable(id).is_err() {
            return Ok(None);
        }
        let handle = GetClipboardData(id).context("GetClipboardData")?;
        let memory = HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            bail!("GlobalLock failed");
        }
        let units = std::slice::from_raw_parts(pointer.cast::<u16>(), size / size_of::<u16>());
        let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..end]);
        let _ = GlobalUnlock(memory);
        Ok(Some(text))
    }
}

impl Harness for Win32Harness {
    fn launch(&mut self) -> anyhow::Result<()> {
        let l = &self.launch;
        std::fs::create_dir_all(&l.log_dir)?;
        // `--super-l registry` changes a Windows setting; this gate has no
        // use for it, and names its mode so it never depends on a default.
        let child = crate::client_config::client_command(&l.client, &l.log_dir)
            .context("preparing the client's config folder")?
            .args(["--super-l", "off"])
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

    fn client_running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    fn windows_set_text(&mut self, text: &str) -> anyhow::Result<()> {
        set_text(text)
    }

    fn windows_text(&mut self) -> anyhow::Result<Option<String>> {
        text()
    }

    fn remote_set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.remote.set_text(text)
    }

    fn remote_text(&mut self) -> anyhow::Result<Option<String>> {
        self.remote.text()
    }

    fn windows_set_picture(&mut self, picture: &Picture, form: WindowsForm) -> anyhow::Result<()> {
        set_picture(picture, form)
    }

    fn windows_picture(&mut self) -> anyhow::Result<Option<Picture>> {
        picture()
    }

    fn windows_history_excluded(&mut self) -> anyhow::Result<bool> {
        history_excluded()
    }

    fn remote_set_picture(&mut self, picture: &Picture) -> anyhow::Result<()> {
        self.remote.set_image(&picture.to_png()?)
    }

    fn remote_picture(&mut self) -> anyhow::Result<Option<Picture>> {
        self.remote
            .image()?
            .map(|png| Picture::from_png(&png))
            .transpose()
    }

    fn windows_set_files(&mut self, spec: &Spec) -> anyhow::Result<()> {
        set_files(spec)
    }

    fn windows_files(&mut self) -> anyhow::Result<Option<Vec<Found>>> {
        files()
    }

    fn windows_drop_effect(&mut self) -> anyhow::Result<Option<u32>> {
        let id = registered("Preferred DropEffect")?;
        let _open = Open::new()?;
        Ok(get(id)?.and_then(|b| {
            b.get(..4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }))
    }

    fn remote_set_files(&mut self, spec: &Spec) -> anyhow::Result<()> {
        self.remote.set_files(spec)
    }

    fn remote_files(&mut self) -> anyhow::Result<Option<Vec<Found>>> {
        self.remote.files()
    }

    fn windows_spool_left(&mut self) -> anyhow::Result<Vec<String>> {
        let pid = self
            .child
            .as_ref()
            .map(Child::id)
            .context("no client was started")?;
        spool_left(pid)
    }

    fn clear_files(&mut self) -> anyhow::Result<()> {
        remove_files_dir()?;
        self.remote.clear_files()
    }

    /// WM_CLOSE to the client's window, so it says Bye and the server ends;
    /// killed when it does not end in 10 s.
    fn close(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let pid = child.id();
        let mut after = None;
        loop {
            // SAFETY: a static class name; no window title filter.
            let Ok(hwnd) = (unsafe { FindWindowExW(None, after, CLASS_NAME, PCWSTR::null()) })
            else {
                break;
            };
            let mut owner = 0;
            // SAFETY: a handle FindWindowExW just returned; `owner` is live.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            if owner == pid {
                // SAFETY: posting copies the integers; nothing is borrowed.
                let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
                break;
            }
            after = Some(hwnd);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while matches!(child.try_wait(), Ok(None)) {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}
