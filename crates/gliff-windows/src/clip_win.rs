//! The Windows clipboard thread.
//!
//! One thread owns a message-only window and everything that touches the
//! clipboard: the change listener, reads of what another program copied, and
//! the delayed rendering of what the other machine copied. The rules it
//! applies are in `clip_core` and `clip_formats`; the transfers are in
//! `clip_engine`, on its own thread, so a paste that waits for the network
//! blocks only this thread.
//!
//! What this thread does, in order of a copy:
//! - A program on Windows copies: `WM_CLIPBOARDUPDATE`, debounced 50 ms, the
//!   echo guard, the formats read, an offer sent to the engine.
//! - The other machine copies: a `HostEvent::RemoteOffer` writes the formats
//!   as promises (`SetClipboardData(format, NULL)`) plus the marker and, by
//!   default, the formats that keep the copy out of history and sync.
//! - A program on Windows pastes: `WM_RENDERFORMAT` asks the engine for the
//!   body and waits for the answer.
//!
//! State is thread-local and borrowed briefly: `EmptyClipboard` sends
//! `WM_DESTROYCLIPBOARD` to this very window, so a borrow is never held
//! across a clipboard call.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;

use gliff_proto::ClipboardFile;
use gliff_proto::clipboard::MAX_ITEM;
use gliff_transport::clipboard::progress::State as TransferState;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::System::DataExchange::AddClipboardFormatListener;
use windows::Win32::System::DataExchange::CloseClipboard;
use windows::Win32::System::DataExchange::EmptyClipboard;
use windows::Win32::System::DataExchange::EnumClipboardFormats;
use windows::Win32::System::DataExchange::GetClipboardData;
use windows::Win32::System::DataExchange::GetClipboardFormatNameW;
use windows::Win32::System::DataExchange::GetClipboardOwner;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::DataExchange::OpenClipboard;
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::DataExchange::RemoveClipboardFormatListener;
use windows::Win32::System::DataExchange::SetClipboardData;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::GMEM_MOVEABLE;
use windows::Win32::System::Memory::GlobalAlloc;
use windows::Win32::System::Memory::GlobalLock;
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::GlobalUnlock;
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::System::Ole::CF_DIBV5;
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE;
use windows::Win32::UI::WindowsAndMessaging::KillTimer;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use windows::Win32::UI::WindowsAndMessaging::RegisterClassExW;
use windows::Win32::UI::WindowsAndMessaging::SetTimer;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
use windows::Win32::UI::WindowsAndMessaging::WM_CLIPBOARDUPDATE;
use windows::Win32::UI::WindowsAndMessaging::WM_CLOSE;
use windows::Win32::UI::WindowsAndMessaging::WM_DESTROY;
use windows::Win32::UI::WindowsAndMessaging::WM_DESTROYCLIPBOARD;
use windows::Win32::UI::WindowsAndMessaging::WM_RENDERALLFORMATS;
use windows::Win32::UI::WindowsAndMessaging::WM_RENDERFORMAT;
use windows::Win32::UI::WindowsAndMessaging::WM_TIMER;
use windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW;
use windows::core::PCWSTR;
use windows::core::w;

use crate::clip_core;
use crate::clip_core::Debounce;
use crate::clip_core::EchoGuard;
use crate::clip_core::LocalChange;
use crate::clip_core::RemoteWrite;
use crate::clip_engine::Command;
use crate::clip_engine::HostEvent;
use crate::clip_files;
use crate::clip_formats;
use crate::clip_formats::WinFormat;
use crate::clip_link::EngineIn;
use crate::clip_link::Host;
use crate::clip_log;
use crate::clip_log::Applied;
use crate::clip_log::BusyAt;
use crate::clip_log::Cause;
use crate::clip_log::Event as Log;
use crate::clip_log::Final;
use crate::clip_log::Notice;
use crate::clip_log::Own;
use crate::clip_log::RevokeWhy;

/// Tells this thread that the engine queued a [`HostEvent`].
const WM_APP_HOST: u32 = WM_APP + 1;
/// Asks this thread to read the clipboard again and offer it, as if a
/// program had just copied (a tab switch: the session now shown has not seen
/// what Windows holds).
const WM_APP_RESCAN: u32 = WM_APP + 2;
/// The quiet period after a clipboard update, in milliseconds.
const DEBOUNCE_MS: u32 = 50;
/// The timer that ends the quiet period.
const DEBOUNCE_TIMER: usize = 1;
/// How many times a busy clipboard is tried again, 100 ms apart, after the
/// attempts inside one reaction failed.
const BUSY_RETRIES: u32 = 5;
/// Attempts to open the clipboard inside one reaction, 15 ms apart.
const OPEN_ATTEMPTS: u32 = 8;

/// A running clipboard thread.
pub struct ClipWin {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl ClipWin {
    /// A function that makes the thread offer the Windows clipboard again.
    /// It can be called from any thread, and does nothing once the window is
    /// gone.
    pub fn rescan_fn(&self) -> impl Fn() + Send + 'static {
        let hwnd = self.hwnd;
        move || {
            // SAFETY: PostMessageW takes any HWND value; a window that is
            // gone makes it fail, and nothing is left to wake.
            let _ =
                unsafe { PostMessageW(Some(hwnd_of(hwnd)), WM_APP_RESCAN, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Destroy the window (which renders what is promised, if it still can)
    /// and wait for the thread.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        // SAFETY: PostMessageW takes any HWND value; a window that is gone
        // makes it fail, and the thread is then ending anyway.
        let _ = unsafe { PostMessageW(Some(hwnd_of(self.hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if thread.join().is_err() {
            clip_log::emit(Log::Notice(Notice::WindowsThreadPanicked));
        }
    }
}

impl Drop for ClipWin {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Starts the thread and waits until its window exists. `input` is the
/// engine's channel (`Port::channel`); the returned [`Host`] is the engine's
/// way back to this thread. `allow_history` is `--clipboard-history`.
pub fn start(
    input: UnboundedSender<EngineIn>,
    allow_history: bool,
) -> std::io::Result<(ClipWin, Arc<dyn Host>)> {
    let host = Arc::new(WinHost::default());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("gliff-clipboard-win".to_owned())
        .spawn({
            let host = Arc::clone(&host);
            move || thread_main(input, allow_history, host, ready_tx)
        })?;
    match ready_rx.recv() {
        Ok(Ok(hwnd)) => Ok((
            ClipWin {
                hwnd,
                thread: Some(thread),
            },
            host,
        )),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(std::io::Error::other(error))
        }
        Err(_) => Err(std::io::Error::other("the clipboard thread ended early")),
    }
}

/// The engine's events, queued for this thread.
#[derive(Default)]
struct WinHost {
    hwnd: AtomicIsize,
    queue: Mutex<VecDeque<HostEvent>>,
}

impl WinHost {
    fn take(&self) -> Vec<HostEvent> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

impl Host for WinHost {
    fn event(&self, event: HostEvent) {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(event);
        let hwnd = hwnd_of(self.hwnd.load(Ordering::Acquire));
        // SAFETY: PostMessageW takes any HWND value; a window that is gone
        // makes it fail, and nothing is left to wake.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_HOST, WPARAM(0), LPARAM(0)) };
    }
}

fn hwnd_of(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

/// The remote offer this window promised.
struct Remote {
    serial: u32,
    write: RemoteWrite,
}

struct State {
    input: UnboundedSender<EngineIn>,
    host: Arc<WinHost>,
    allow_history: bool,
    echo: EchoGuard,
    debounce: Debounce,
    busy_retries: u32,
    remote: Option<Remote>,
    /// The generation of the offer made for the local clipboard, and the
    /// clipboard's sequence number when it was read.
    generation: u64,
    local_sequence: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Runs `f` on the state, or does nothing when it is borrowed (a re-entrant
/// message) or not yet set.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

fn thread_main(
    input: UnboundedSender<EngineIn>,
    allow_history: bool,
    host: Arc<WinHost>,
    ready: std::sync::mpsc::Sender<Result<isize, String>>,
) {
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "clipboard window: error {:#010x}",
                error.code().0
            )));
            return;
        }
    };
    // SAFETY: our own window, on its own thread.
    if let Err(error) = unsafe { AddClipboardFormatListener(hwnd) } {
        // SAFETY: our own window.
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = ready.send(Err(format!(
            "clipboard listener: error {:#010x}",
            error.code().0
        )));
        return;
    }
    host.hwnd.store(hwnd.0 as isize, Ordering::Release);
    STATE.with(|cell| {
        *cell.borrow_mut() = Some(State {
            input,
            host,
            allow_history,
            echo: EchoGuard::default(),
            debounce: Debounce::default(),
            busy_retries: 0,
            remote: None,
            generation: 0,
            local_sequence: 0,
        });
    });
    let _ = ready.send(Ok(hwnd.0 as isize));
    let mut msg = MSG::default();
    // SAFETY: a live MSG; every window of this thread.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: the message GetMessageW just filled in.
        unsafe { DispatchMessageW(&msg) };
    }
    STATE.with(|cell| *cell.borrow_mut() = None);
}

fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: a class with a static name and our own window procedure, and
    // a message-only window of that class.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("GliffClipboard"),
            ..Default::default()
        };
        // A second start finds the class registered; creating the window
        // reports any real problem.
        let _ = RegisterClassExW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("GliffClipboard"),
            w!("gliff clipboard"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE | WM_APP_RESCAN => on_update(hwnd),
        WM_TIMER if wparam.0 == DEBOUNCE_TIMER => on_timer(hwnd),
        WM_APP_HOST => on_host_events(hwnd),
        WM_RENDERFORMAT => render_requested(wparam.0 as u32),
        WM_RENDERALLFORMATS => render_everything(hwnd),
        WM_DESTROYCLIPBOARD => {
            with_state(|state| state.remote = None);
        }
        WM_CLOSE => {
            // SAFETY: our own window; destroying it renders what is promised.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        WM_DESTROY => {
            // SAFETY: our own window, which still exists in WM_DESTROY.
            let _ = unsafe { RemoveClipboardFormatListener(hwnd) };
            // SAFETY: posts WM_QUIT to this thread's queue.
            unsafe { PostQuitMessage(0) };
        }
        // SAFETY: the default handling of a message of our own window.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

// The clipboard itself.

/// The clipboard, opened; closed when dropped.
struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Opens the clipboard for this window, trying again for a short while: any
/// program that is reading or writing it holds it.
fn open(hwnd: HWND) -> Option<Open> {
    for attempt in 0..OPEN_ATTEMPTS {
        // SAFETY: our own window.
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return Some(Open);
        }
        if attempt + 1 < OPEN_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    None
}

/// The Windows id of a format; registers a named one.
fn format_id(format: &WinFormat) -> u32 {
    match format {
        WinFormat::UnicodeText => u32::from(CF_UNICODETEXT.0),
        WinFormat::Dib => u32::from(CF_DIB.0),
        WinFormat::DibV5 => u32::from(CF_DIBV5.0),
        WinFormat::Hdrop => u32::from(CF_HDROP.0),
        other => other.registered_name().map_or(0, registered_id),
    }
}

fn registered_id(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated string that outlives the call.
    unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
}

/// The formats now on the clipboard that the pure code knows. The clipboard
/// must be open.
fn formats_on_clipboard(_open: &Open) -> Vec<WinFormat> {
    let mut formats = Vec::new();
    let mut id = 0;
    loop {
        // SAFETY: the clipboard is open (`_open`).
        id = unsafe { EnumClipboardFormats(id) };
        if id == 0 {
            break;
        }
        let format = match id {
            _ if id == u32::from(CF_UNICODETEXT.0) => WinFormat::UnicodeText,
            _ if id == u32::from(CF_DIB.0) => WinFormat::Dib,
            _ if id == u32::from(CF_DIBV5.0) => WinFormat::DibV5,
            _ if id == u32::from(CF_HDROP.0) => WinFormat::Hdrop,
            0xC000..=0xFFFF => {
                let mut name = [0u16; 256];
                // SAFETY: a buffer of the length passed.
                let len = unsafe { GetClipboardFormatNameW(id, &mut name) };
                let Ok(len) = usize::try_from(len) else {
                    continue;
                };
                WinFormat::from_registered(&String::from_utf16_lossy(&name[..len.min(255)]))
            }
            _ => continue,
        };
        formats.push(format);
    }
    formats
}

/// The bytes of a format on the open clipboard, or `None` when it is not
/// there, cannot be read, or is over [`clip_formats::raw_limit`]: a huge item
/// is refused before it is copied.
fn read_format(_open: &Open, format: &WinFormat) -> Option<Vec<u8>> {
    let id = format_id(format);
    if id == 0 {
        return None;
    }
    // SAFETY: the clipboard is open (`_open`); the handle is the clipboard's
    // own memory, locked only while it is copied.
    unsafe {
        let handle = GetClipboardData(id).ok()?;
        let memory = HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        if size > clip_formats::raw_limit(format) {
            return None;
        }
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(memory);
        Some(bytes)
    }
}

/// Puts `bytes` on the open clipboard as `id`. Windows owns the memory
/// after a successful call.
fn set_bytes(_open: &Open, id: u32, bytes: &[u8]) -> bool {
    if id == 0 {
        return false;
    }
    // SAFETY: the clipboard is open (`_open`); the memory is filled before
    // it is handed over and freed when the hand-over fails.
    unsafe {
        let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else {
            return false;
        };
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(memory);
        if SetClipboardData(id, Some(HANDLE(memory.0))).is_err() {
            let _ = GlobalFree(Some(memory));
            return false;
        }
        true
    }
}

/// Promises `id` to be rendered on request.
fn promise(_open: &Open, id: u32) {
    if id != 0 {
        // SAFETY: the clipboard is open (`_open`); a NULL handle promises.
        let _ = unsafe { SetClipboardData(id, None) };
    }
}

fn owner_is(hwnd: HWND) -> bool {
    // SAFETY: no arguments; the owner may be none.
    unsafe { GetClipboardOwner() }.is_ok_and(|owner| owner == hwnd)
}

// A program on Windows copies.

fn on_update(hwnd: HWND) {
    if with_state(|state| state.debounce.update()) == Some(true) {
        arm_timer(hwnd, DEBOUNCE_MS);
    }
}

fn arm_timer(hwnd: HWND, millis: u32) {
    // SAFETY: our own window and no callback.
    let _ = unsafe { SetTimer(Some(hwnd), DEBOUNCE_TIMER, millis, None) };
}

fn on_timer(hwnd: HWND) {
    // SAFETY: our own window.
    let _ = unsafe { KillTimer(Some(hwnd), DEBOUNCE_TIMER) };
    if with_state(|state| state.debounce.settle()) == Some(true) {
        local_changed(hwnd);
    }
}

fn local_changed(hwnd: HWND) {
    // SAFETY: no arguments.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let marker = registered_id(clip_formats::MARKER_FORMAT);
    // SAFETY: a format id.
    let has_marker = unsafe { IsClipboardFormatAvailable(marker) }.is_ok();
    let Some(own) = with_state(|state| state.echo.verdict(sequence, owner_is(hwnd), has_marker))
    else {
        return;
    };
    clip_log::emit(Log::LocalChange { sequence, own });
    if own != Own::No {
        return;
    }
    let Some(clipboard) = open(hwnd) else {
        let retry = with_state(|state| {
            state.busy_retries += 1;
            state.busy_retries <= BUSY_RETRIES && state.debounce.update()
        });
        if retry == Some(true) {
            arm_timer(hwnd, 100);
        } else {
            clip_log::emit(Log::Busy {
                at: BusyAt::LocalCopy,
            });
        }
        return;
    };
    with_state(|state| state.busy_retries = 0);
    let formats = formats_on_clipboard(&clipboard);
    let private = clip_formats::is_private(&formats, &mut |name| {
        let bytes = read_format(&clipboard, &WinFormat::from_registered(name))?;
        Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
    });
    let change = clip_core::local_change(&formats, private);
    let dropped = match &change {
        LocalChange::Offer { files: true, .. } => read_format(&clipboard, &WinFormat::Hdrop),
        _ => None,
    };
    drop(clipboard);
    let (mime_types, files) = match change {
        LocalChange::Withdraw => {
            let cause = if private {
                Cause::Private
            } else {
                Cause::NothingForwardable
            };
            clip_log::emit(Log::LocalSkipped { sequence, cause });
            (Vec::new(), Default::default())
        }
        LocalChange::Offer { mimes, files } => {
            let listed = if files {
                dropped
                    .as_deref()
                    .and_then(clip_files::parse_hdrop)
                    .map(|paths| clip_files::list_files(&paths))
            } else {
                None
            };
            let files = match listed {
                Some(Ok(listed)) => listed,
                Some(Err(error)) => {
                    clip_log::emit(Log::FilesSkipped {
                        sequence,
                        cause: Cause::of_list(&error),
                    });
                    Default::default()
                }
                None => Default::default(),
            };
            (mimes, files)
        }
    };
    let Some(command) = with_state(|state| {
        state.generation += 1;
        state.local_sequence = sequence;
        (state.input.clone(), state.generation)
    }) else {
        return;
    };
    let (input, generation) = command;
    // A local change revokes the current offer first.
    let _ = input.send(EngineIn::Command(Command::RevokeLocal(
        RevokeWhy::LocalChange,
    )));
    let _ = input.send(EngineIn::Command(Command::LocalOffer {
        mime_types,
        files,
        generation,
    }));
}

/// The body of `mime` from the local clipboard, if it is still what was
/// offered.
fn read_local(hwnd: HWND, mime: &str, generation: u64) -> Result<Vec<u8>, Cause> {
    let (current, sequence) =
        with_state(|state| (state.generation, state.local_sequence)).ok_or(Cause::ThreadBusy)?;
    if current != generation {
        return Err(Cause::ClipboardChanged);
    }
    // SAFETY: no arguments.
    if unsafe { GetClipboardSequenceNumber() } != sequence {
        return Err(Cause::ClipboardSequenceMoved);
    }
    let clipboard = open(hwnd).ok_or(Cause::ClipboardBusy)?;
    let formats = formats_on_clipboard(&clipboard);
    let limit = usize::try_from(MAX_ITEM).unwrap_or(usize::MAX);
    clip_formats::local_body(mime, &formats, limit, &mut |format| {
        read_format(&clipboard, format)
    })
    .map_err(|error| Cause::of_convert(&error))
}

// The other machine copies.

fn on_host_events(hwnd: HWND) {
    let Some(host) = with_state(|state| Arc::clone(&state.host)) else {
        return;
    };
    for event in host.take() {
        match event {
            HostEvent::RemoteOffer {
                serial,
                mime_types,
                files,
            } => remote_offered(hwnd, serial, &mime_types, &files),
            HostEvent::ReadLocal {
                mime_type,
                generation,
                reply,
            } => {
                let _ = reply.send(read_local(hwnd, &mime_type, generation));
            }
            HostEvent::Transfer { id, progress } => {
                let state = match progress.state {
                    TransferState::Running => None,
                    TransferState::Done => Some(Final::Done),
                    TransferState::Cancelled => Some(Final::Cancelled),
                    TransferState::Failed(_) => Some(Final::Failed),
                };
                if let Some(state) = state {
                    clip_log::emit(Log::TransferFinal {
                        transfer: id,
                        state,
                    });
                }
            }
        }
    }
}

fn remote_offered(hwnd: HWND, serial: u32, mimes: &[String], files: &[ClipboardFile]) {
    let allow_history = with_state(|state| state.allow_history).unwrap_or(false);
    match clip_core::remote_write(mimes, files, allow_history) {
        Some(write) => put_remote(hwnd, serial, write),
        None => clear_if_ours(hwnd, serial),
    }
}

/// An offer with nothing Windows can use: take back the promises, if they
/// are still ours.
fn clear_if_ours(hwnd: HWND, serial: u32) {
    if !owner_is(hwnd) {
        return;
    }
    let Some(clipboard) = open(hwnd) else {
        clip_log::emit(Log::Busy {
            at: BusyAt::ClearRemote,
        });
        return;
    };
    // SAFETY: the clipboard is open.
    let _ = unsafe { EmptyClipboard() };
    drop(clipboard);
    clip_log::emit(Log::OfferApplied {
        serial,
        applied: Applied::Cleared,
    });
    with_state(|state| {
        state.remote = None;
        // SAFETY: no arguments.
        state.echo.wrote(unsafe { GetClipboardSequenceNumber() });
    });
}

/// Promises the formats of a remote offer on the clipboard.
fn put_remote(hwnd: HWND, serial: u32, write: RemoteWrite) {
    let Some(clipboard) = open(hwnd) else {
        clip_log::emit(Log::Busy {
            at: BusyAt::PutRemote,
        });
        return;
    };
    // The old promises end here: the window is told with
    // WM_DESTROYCLIPBOARD before this returns.
    // SAFETY: the clipboard is open.
    let _ = unsafe { EmptyClipboard() };
    let plans = write.plans.clone();
    let files = write.files;
    let private = write.private;
    with_state(|state| state.remote = Some(Remote { serial, write }));
    for plan in &plans {
        promise(&clipboard, format_id(&plan.format));
    }
    if files {
        promise(&clipboard, u32::from(CF_HDROP.0));
        set_bytes(
            &clipboard,
            registered_id(clip_files::DROP_EFFECT_FORMAT),
            &clip_files::DROPEFFECT_COPY.to_le_bytes(),
        );
    }
    set_bytes(&clipboard, registered_id(clip_formats::MARKER_FORMAT), &[1]);
    if private {
        let zero = 0u32.to_le_bytes();
        for name in [
            clip_formats::EXCLUDE_FORMAT,
            clip_formats::HISTORY_FORMAT,
            clip_formats::CLOUD_FORMAT,
            clip_formats::VIEWER_IGNORE_FORMAT,
        ] {
            set_bytes(&clipboard, registered_id(name), &zero);
        }
    }
    drop(clipboard);
    with_state(|state| {
        // SAFETY: no arguments.
        state.echo.wrote(unsafe { GetClipboardSequenceNumber() });
    });
    clip_log::emit(Log::OfferApplied {
        serial,
        applied: Applied::Promised,
    });
}

// A program on Windows pastes.

/// `WM_RENDERFORMAT`: the clipboard is open in the program that pastes, and
/// this thread answers with the body, waiting for the engine.
fn render_requested(id: u32) {
    let Some((input, serial, write)) = with_state(|state| {
        state
            .remote
            .as_ref()
            .map(|remote| (state.input.clone(), remote.serial, remote.write.clone()))
    })
    .flatten() else {
        return;
    };
    if let Some(bytes) = rendered(&input, serial, &write, id) {
        // The clipboard is open in the requester, not in this thread, so
        // the proof of an open clipboard must not close it.
        let held = std::mem::ManuallyDrop::new(Open);
        set_bytes(&held, id, &bytes);
    }
}

/// The bytes of format `id` of a remote offer, fetched through the engine.
fn rendered(
    input: &UnboundedSender<EngineIn>,
    serial: u32,
    write: &RemoteWrite,
    id: u32,
) -> Option<Vec<u8>> {
    if write.files && id == u32::from(CF_HDROP.0) {
        let (result, answer) = oneshot::channel();
        input
            .send(EngineIn::Command(Command::FetchFiles { serial, result }))
            .ok()?;
        return match answer.blocking_recv() {
            Ok(Ok(paths)) => Some(clip_files::hdrop_bytes(&paths)),
            // The engine logged why (`paste ended`).
            Ok(Err(_)) | Err(_) => None,
        };
    }
    let plan = write
        .plans
        .iter()
        .find(|plan| format_id(&plan.format) == id)?;
    let (result, answer) = oneshot::channel();
    input
        .send(EngineIn::Command(Command::FetchMime {
            mime_type: plan.mime.clone(),
            serial,
            result,
        }))
        .ok()?;
    match answer.blocking_recv() {
        Ok(Ok(body)) => match clip_formats::render(plan.render, &body) {
            Ok(bytes) => Some(bytes),
            Err(_) => {
                clip_log::emit(Log::Notice(Notice::PasteNotConverted));
                None
            }
        },
        // The engine logged why (`paste ended`).
        Ok(Err(_)) | Err(_) => None,
    }
}

/// `WM_RENDERALLFORMATS`: this window is going away while it still holds
/// promises. Text and pictures are fetched now when the session is still up;
/// files are not (a spool of up to 4 GiB would hold the exit).
fn render_everything(hwnd: HWND) {
    let Some((input, serial, write)) = with_state(|state| {
        state
            .remote
            .as_ref()
            .map(|remote| (state.input.clone(), remote.serial, remote.write.clone()))
    })
    .flatten() else {
        return;
    };
    if !owner_is(hwnd) {
        return;
    }
    let Some(clipboard) = open(hwnd) else {
        return;
    };
    for plan in &write.plans {
        let id = format_id(&plan.format);
        if let Some(bytes) = rendered(&input, serial, &write, id) {
            set_bytes(&clipboard, id, &bytes);
        }
    }
    if write.files {
        clip_log::emit(Log::Notice(Notice::FilesNotRenderedAtExit));
    }
}
