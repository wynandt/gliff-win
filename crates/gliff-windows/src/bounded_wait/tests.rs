use std::convert::Infallible;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use super::ended_within;

#[test]
fn a_thread_that_ends_is_seen_to_end() {
    let (tx, rx) = mpsc::channel::<Infallible>();
    let handle = std::thread::spawn(move || drop(tx));
    handle.join().expect("thread");
    assert!(ended_within(&rx, Duration::from_secs(5)));
}

#[test]
fn a_thread_that_panics_counts_as_ended() {
    let (tx, rx) = mpsc::channel::<Infallible>();
    let handle = std::thread::spawn(move || {
        let _tx = tx;
        panic!("the helper thread failed");
    });
    let _ = handle.join();
    assert!(ended_within(&rx, Duration::from_secs(5)));
}

#[test]
fn a_stuck_thread_is_given_up_on_after_the_limit() {
    let (tx, rx) = mpsc::channel::<Infallible>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let _tx = tx;
        let _ = release_rx.recv();
    });
    let started = Instant::now();
    assert!(!ended_within(&rx, Duration::from_millis(100)));
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(release_tx);
    handle.join().expect("thread");
}
