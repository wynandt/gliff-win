//! Waiting for a file the client rewrites about once a second.
//!
//! The client's `--stats-out` file is rewritten every second, so a case that
//! reads it right after its last key can read a copy that is older than the
//! key (the grab it shows is the one before the release chord). The harness
//! waits until the file is newer than the moment it asks.

use std::path::Path;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

/// How often the file's time is looked at.
const POLL: Duration = Duration::from_millis(25);

/// True once `path` was modified after `since`; false if that has not
/// happened within `timeout` (the client is gone, or too slow).
pub fn wait_modified_after(path: &Path, since: SystemTime, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let modified = std::fs::metadata(path).and_then(|m| m.modified());
        if modified.is_ok_and(|m| m > since) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gwin-fresh-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join("stats.json")
    }

    #[test]
    fn a_file_written_after_the_moment_is_fresh() {
        let path = scratch("after");
        std::fs::write(&path, "old").expect("write");
        let since = SystemTime::now();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                std::fs::write(&path, "new").expect("rewrite");
            })
        };
        assert!(wait_modified_after(&path, since, Duration::from_secs(5)));
        writer.join().expect("writer");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_older_than_the_moment_is_stale_until_the_timeout() {
        let path = scratch("stale");
        std::fs::write(&path, "old").expect("write");
        std::thread::sleep(Duration::from_millis(50));
        let since = SystemTime::now();
        let started = Instant::now();
        assert!(!wait_modified_after(
            &path,
            since,
            Duration::from_millis(200)
        ));
        assert!(started.elapsed() >= Duration::from_millis(200), "it waited");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_is_stale() {
        let path = scratch("missing");
        let _ = std::fs::remove_file(&path);
        assert!(!wait_modified_after(
            &path,
            SystemTime::now(),
            Duration::from_millis(60)
        ));
    }
}
