//! Tests for the parts of the Windows harness that need no client, no
//! remote machine and no real input.

use super::*;

/// A stats file the client wrote before the case began, showing the
/// keyboard grabbed, and not rewritten since.
fn stale_stats(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gwin-harness-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the folder is created");
    let path = dir.join("stats.json");
    let mut stats = ClientStats::from_json(include_str!(
        "../../../present_check/fixtures/synthetic-good-stats.json"
    ))
    .expect("the fixture parses");
    stats.input.hook = "grab".to_owned();
    stats.input.grab = true;
    std::fs::write(&path, stats.to_json()).expect("the stats file is written");
    // The file's time must be before the moment `read_seen` asks.
    std::thread::sleep(Duration::from_millis(50));
    path
}

/// A harness with no client and no window that reads `stats_out`.
fn harness(stats_out: PathBuf) -> Win32Harness {
    Win32Harness {
        launch: Launch {
            client: PathBuf::new(),
            host: "user@example".parse().expect("the host parses"),
            server_path: String::new(),
            ssh: PathBuf::new(),
            log_dir: std::env::temp_dir(),
            stats_out,
            remote_keys: String::new(),
            prompt_enter: false,
        },
        child: None,
        hwnd: None,
        explorer_before: Vec::new(),
        explorer_watch: None,
        sink: None,
        layouts: Vec::new(),
        original_default: None,
        diag_files: 0,
        locked: std::sync::Arc::default(),
        stop_poller: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    }
}

#[test]
fn a_stats_file_older_than_the_case_is_not_read() {
    let path = stale_stats("observe");
    let h = harness(path.clone());
    let started = Instant::now();
    let seen = h.read_seen();
    let waited = started.elapsed();
    let _ = std::fs::remove_dir_all(path.parent().expect("a folder"));
    assert!(
        waited >= STATS_FRESH,
        "the wait ended after {waited:?}, before its timeout"
    );
    assert!(
        seen.input.is_none(),
        "the stats file was not rewritten, yet its input (grab = {:?}) was returned",
        seen.input.map(|i| i.grab)
    );
}
