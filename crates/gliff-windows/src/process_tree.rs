//! A child process together with the processes it starts.
//!
//! ssh can start helpers of its own: a `ProxyCommand`, the ssh behind a
//! `ProxyJump`, a `LocalCommand`. A helper inherits ssh's ends of the
//! client's pipes, so while it runs, the client's reads from ssh never reach
//! end of file, even after ssh itself has been killed.
//!
//! On Windows the tree is a job object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: ending it, dropping it, or the
//! client's own exit ends ssh and every helper in it. A process the child
//! started before [`ProcessTree::adopt`] ran is not in the tree, so adopt
//! right after the spawn. On other hosts the tree is empty and ending it
//! does nothing.

use tokio::process::Child;

/// The processes a child started; see the module docs.
#[derive(Debug)]
pub struct ProcessTree {
    #[cfg(windows)]
    job: Option<win::Job>,
}

impl ProcessTree {
    /// Takes in `child` and every process it starts from now on. The `String`
    /// says why Windows refused, if it did; the tree then holds only the child.
    #[cfg(windows)]
    pub fn adopt(child: &Child) -> (Self, Option<String>) {
        match win::Job::with(child) {
            Ok(job) => (Self { job: Some(job) }, None),
            Err(why) => (Self { job: None }, Some(why)),
        }
    }

    /// Takes in `child` and every process it starts from now on. The `String`
    /// says why Windows refused, if it did; the tree then holds only the child.
    #[cfg(not(windows))]
    pub fn adopt(child: &Child) -> (Self, Option<String>) {
        let _ = child;
        (Self {}, None)
    }

    /// Ends every process still in the tree, the child too. Its owner still
    /// reaps the child.
    pub fn end(&self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.end();
        }
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows::Win32::System::JobObjects::CreateJobObjectW;
    use windows::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    use windows::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
    use windows::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
    use windows::Win32::System::JobObjects::SetInformationJobObject;
    use windows::Win32::System::JobObjects::TerminateJobObject;
    use windows::core::PCWSTR;

    use tokio::process::Child;

    /// The exit code a process ended through its tree gets.
    const ENDED: u32 = 1;

    /// A job object that ends its processes when it is closed.
    #[derive(Debug)]
    pub(super) struct Job {
        handle: HANDLE,
    }

    // SAFETY: a job handle is a kernel handle, usable from any thread.
    unsafe impl Send for Job {}
    // SAFETY: the job's calls are thread-safe kernel calls on the handle.
    unsafe impl Sync for Job {}

    impl Job {
        pub(super) fn with(child: &Child) -> Result<Self, String> {
            let process = child
                .raw_handle()
                .ok_or_else(|| "the child has already been reaped".to_owned())?;
            // SAFETY: no security attributes and no name.
            let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
                .map_err(|e| format!("CreateJobObjectW: {e}"))?;
            let job = Self { handle };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = u32::try_from(std::mem::size_of_val(&limits))
                .expect("the job limit structure fits in a u32");
            // SAFETY: `limits` is the structure the information class names
            // and outlives the call.
            unsafe {
                SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast(),
                    size,
                )
            }
            .map_err(|e| format!("SetInformationJobObject: {e}"))?;
            // SAFETY: both handles are open; the process handle stays owned
            // by `child`.
            unsafe { AssignProcessToJobObject(job.handle, HANDLE(process)) }
                .map_err(|e| format!("AssignProcessToJobObject: {e}"))?;
            Ok(job)
        }

        pub(super) fn end(&self) {
            // SAFETY: the handle is open until `drop`.
            let _ = unsafe { TerminateJobObject(self.handle, ENDED) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle `CreateJobObjectW` returned, closed once,
            // here; closing the last handle ends the job's processes.
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }
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
