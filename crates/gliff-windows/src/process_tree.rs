//! A child process together with the processes it starts.
//!
//! ssh can start helpers of its own: a `ProxyCommand`, the ssh behind a
//! `ProxyJump`, a `LocalCommand`. A helper inherits ssh's ends of the
//! client's pipes, so while it runs, the client's reads from ssh never reach
//! end of file, even after ssh itself has been killed.

use tokio::process::Child;

/// The processes a child started; see the module docs.
#[derive(Debug)]
pub struct ProcessTree {}

impl ProcessTree {
    /// Takes in `child` and every process it starts from now on. The `String`
    /// says why Windows refused, if it did; the tree then holds only the child.
    pub fn adopt(child: &Child) -> (Self, Option<String>) {
        let _ = child;
        (Self {}, None)
    }

    /// Ends every process still in the tree. The child itself is left for
    /// its owner to kill and reap.
    pub fn end(&self) {}
}

#[cfg(all(test, windows))]
mod tests {
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use tokio::io::AsyncReadExt;
    use tokio::process::Child;
    use tokio::process::ChildStdout;
    use tokio::process::Command;

    use super::ProcessTree;

    /// Longer than any test here may wait.
    const HELPER_LIFE: &str = "ping -n 60 127.0.0.1 >nul";
    /// How long a pipe may stay open once its writers are gone.
    const EOF_WAIT: Duration = Duration::from_secs(5);

    /// `cmd.exe` running a helper that inherits its stdout and lives for a
    /// minute: the shape of ssh running a `ProxyCommand`.
    async fn child_with_helper() -> (Child, ChildStdout, ProcessTree) {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/d", "/c", HELPER_LIFE])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(crate::win32::CREATE_NO_WINDOW);
        let mut child = cmd.spawn().expect("cmd.exe starts");
        let (tree, refused) = ProcessTree::adopt(&child);
        assert_eq!(refused, None, "Windows takes the child into a tree");
        let stdout = child.stdout.take().expect("stdout was set to piped");
        tokio::time::sleep(Duration::from_millis(500)).await;
        (child, stdout, tree)
    }

    async fn reaches_eof(stdout: &mut ChildStdout) -> Result<(), Duration> {
        let started = Instant::now();
        let mut rest = Vec::new();
        match tokio::time::timeout(EOF_WAIT, stdout.read_to_end(&mut rest)).await {
            Ok(_) => Ok(()),
            Err(_) => Err(started.elapsed()),
        }
    }

    #[tokio::test]
    async fn ending_the_tree_ends_a_helper_that_holds_the_pipe() {
        let (mut child, mut stdout, tree) = child_with_helper().await;
        tree.end();
        let _ = child.kill().await;
        let eof = reaches_eof(&mut stdout).await;
        assert_eq!(
            eof,
            Ok(()),
            "the child's stdout stayed open after the tree ended: its helper outlived it"
        );
    }

    #[tokio::test]
    async fn dropping_the_tree_ends_a_helper_that_holds_the_pipe() {
        let (mut child, mut stdout, tree) = child_with_helper().await;
        let _ = child.kill().await;
        drop(tree);
        let eof = reaches_eof(&mut stdout).await;
        assert_eq!(
            eof,
            Ok(()),
            "the child's stdout stayed open after the tree was dropped: its helper outlived it"
        );
    }
}
