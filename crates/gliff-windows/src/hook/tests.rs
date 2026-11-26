//! Tests for the hook thread's teardown. They install no hook: the thread
//! under test stands in for the one `run` starts. They call `stop`, which
//! returns the lines `Drop` logs, because the process log opens only once.

use std::sync::atomic::AtomicBool;

use super::*;

/// The most messages the test posts before it expects the queue to be full.
/// Windows' default per-thread quota is 10,000.
const FILL_LIMIT: u32 = 100_000;

/// A thread with a message queue that it does not read until told to, as a
/// hook thread whose callback is running.
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
        tid_tx
            .send(unsafe { GetCurrentThreadId() })
            .expect("the test is waiting for the id");
        let _ = go_rx.recv();
        // SAFETY: `msg` is a valid MSG.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
        stopped_in.store(true, Ordering::SeqCst);
        drop(ended_tx);
    });
    BusyLoop {
        thread_id: tid_rx.recv().expect("the thread sends its id"),
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
fn stop_names_a_quit_post_refused_by_a_full_queue() {
    let busy = busy_loop();
    let full = fill_queue(busy.thread_id);
    assert_eq!(full.code(), ERROR_NOT_ENOUGH_QUOTA.to_hresult(), "{full}");

    let mut hook = HookThread {
        thread_id: busy.thread_id,
        join: Some(busy.join),
        ended: busy.ended,
    };
    let lines = hook.stop();

    busy.go.send(()).expect("the thread is waiting");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !busy.stopped.load(Ordering::SeqCst) && Instant::now() < deadline {
        // SAFETY: PostThreadMessageW takes any thread id.
        let _ = unsafe { PostThreadMessageW(busy.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("PostThreadMessageW(WM_QUIT) failed")
                && l.contains("ERROR_NOT_ENOUGH_QUOTA")),
        "the failed quit post is not named: {lines:?}"
    );
    assert!(hook.stop().is_empty(), "a second stop does nothing");
}

/// Windows refuses WM_QUIT because the thread has no message queue. The
/// thread keeps running past the bounded wait, as a hook thread would.
#[test]
fn stop_names_a_quit_post_to_a_thread_without_a_queue() {
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel::<()>();
    let (ended_tx, ended) = std::sync::mpsc::channel::<Infallible>();
    let join = std::thread::spawn(move || {
        // SAFETY: no preconditions.
        tid_tx
            .send(unsafe { GetCurrentThreadId() })
            .expect("the test is waiting for the id");
        let _ = go_rx.recv();
        drop(ended_tx);
    });
    let thread_id = tid_rx.recv().expect("the thread sends its id");

    let started = Instant::now();
    let lines = HookThread {
        thread_id,
        join: Some(join),
        ended,
    }
    .stop();
    let waited = started.elapsed();
    go.send(()).expect("the thread is waiting");

    assert!(
        waited >= STOP_WAIT,
        "stop returned after {waited:?}, before its bounded wait"
    );
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("PostThreadMessageW(WM_QUIT) failed")
            && lines[0].contains("ERROR_INVALID_THREAD_ID"),
        "the failed quit post is not named: {lines:?}"
    );
    assert!(
        lines[1].contains("did not stop in time"),
        "the bounded wait is not reported: {lines:?}"
    );
}
