//! Tests for the hook thread's teardown. They install no hook: the thread
//! under test is a plain message loop like the one in `run`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use windows::Win32::Foundation::ERROR_NOT_ENOUGH_QUOTA;

use super::*;

/// The most messages the test posts before it expects the queue to be full.
/// Windows' default per-thread quota is 10,000.
const FILL_LIMIT: u32 = 100_000;

/// The log file both tests read. The log opens once per process.
fn log_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("gliff-hook-teardown-{}", std::process::id()));
        crate::logging::init(&dir).expect("the log opens")
    })
}

fn log_text() -> String {
    std::fs::read_to_string(log_path()).unwrap_or_default()
}

/// A thread with a message queue that does not read it until told to.
struct BusyLoop {
    thread_id: u32,
    join: JoinHandle<()>,
    go: std::sync::mpsc::Sender<()>,
    ended: Receiver<Infallible>,
    stopped: Arc<AtomicBool>,
}

fn busy_loop() -> BusyLoop {
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let (ended_tx, ended) = std::sync::mpsc::channel::<Infallible>();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_in = stopped.clone();
    let join = std::thread::spawn(move || {
        let mut msg = MSG::default();
        // SAFETY: `msg` is a valid MSG; PM_NOREMOVE only creates the queue,
        // as `run` does.
        let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
        // SAFETY: no preconditions.
        tid_tx.send(unsafe { GetCurrentThreadId() }).unwrap();
        // Busy, as when the hook callback runs: the queue is not read.
        let _ = go_rx.recv();
        // SAFETY: `msg` is a valid MSG.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
        stopped_in.store(true, Ordering::SeqCst);
        drop(ended_tx);
    });
    BusyLoop {
        thread_id: tid_rx.recv().unwrap(),
        join,
        go,
        ended,
        stopped,
    }
}

/// Posts WM_NULL until Windows refuses one, and returns that error.
fn fill_queue(thread_id: u32) -> windows::core::Error {
    for _ in 0..FILL_LIMIT {
        // SAFETY: PostThreadMessageW takes any thread id.
        if let Err(e) = unsafe { PostThreadMessageW(thread_id, 0, WPARAM(0), LPARAM(0)) } {
            return e;
        }
    }
    panic!("the queue took {FILL_LIMIT} messages without filling");
}

/// Windows refuses WM_QUIT because the thread's queue is full while the
/// thread is busy. Needs Windows' posted-message quota, which Wine lacks.
#[test]
fn drop_logs_a_quit_post_refused_by_a_full_queue() {
    log_path();
    let busy = busy_loop();
    let full = fill_queue(busy.thread_id);
    assert_eq!(full.code(), ERROR_NOT_ENOUGH_QUOTA.to_hresult(), "{full}");

    drop(HookThread {
        thread_id: busy.thread_id,
        join: Some(busy.join),
        ended: busy.ended,
    });

    // Let the loop drain and end, so the test leaves no thread behind.
    busy.go.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !busy.stopped.load(Ordering::SeqCst) && Instant::now() < deadline {
        // SAFETY: PostThreadMessageW takes any thread id.
        let _ = unsafe { PostThreadMessageW(busy.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = log_text();
    assert!(
        text.contains("PostThreadMessageW(WM_QUIT) failed")
            && text.contains("ERROR_NOT_ENOUGH_QUOTA"),
        "the log does not name the failed quit post:\n{text}"
    );
}

/// Windows refuses WM_QUIT because the thread has no message queue. The
/// thread keeps running past the bounded wait, as a hook thread would.
#[test]
fn drop_logs_a_quit_post_to_a_thread_without_a_queue() {
    log_path();
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let (ended_tx, ended) = std::sync::mpsc::channel::<Infallible>();
    let join = std::thread::spawn(move || {
        // SAFETY: no preconditions.
        tid_tx.send(unsafe { GetCurrentThreadId() }).unwrap();
        let _ = go_rx.recv();
        drop(ended_tx);
    });
    let thread_id = tid_rx.recv().unwrap();

    let started = Instant::now();
    drop(HookThread {
        thread_id,
        join: Some(join),
        ended,
    });
    let waited = started.elapsed();
    go.send(()).unwrap();

    let text = log_text();
    assert!(
        waited >= STOP_WAIT,
        "Drop returned after {waited:?}, before its bounded wait"
    );
    assert!(
        text.contains("PostThreadMessageW(WM_QUIT) failed")
            && text.contains("ERROR_INVALID_THREAD_ID"),
        "the log does not name the failed quit post:\n{text}"
    );
}
