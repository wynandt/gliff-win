//! The remote machine's clipboard, over ssh: `wl-copy` and `wl-paste`.
//!
//! A non-interactive ssh has no Wayland environment, so the script finds the
//! compositor's socket in the runtime directory. It is plain `sh` inside
//! `sh -c '...'`, whatever the login shell is.

use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::bail;

use super::tree::Found;
use super::tree::Spec;
use super::tree::parse_uri_list;
use super::tree::read_tar;
use super::tree::uri_list;

/// Sets `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` when the shell has none:
/// the first `wayland-*` socket of the runtime directory.
macro_rules! environment {
    () => {
        concat!(
            r#"export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"; "#,
            r#"if [ -z "$WAYLAND_DISPLAY" ]; then "#,
            r#"for s in "$XDG_RUNTIME_DIR"/wayland-*; do "#,
            r#"case "$s" in *.lock) ;; *) if [ -S "$s" ]; then WAYLAND_DISPLAY="${s##*/}"; break; fi;; esac; "#,
            r#"done; fi; export WAYLAND_DISPLAY; "#,
        )
    };
}

/// Copy stdin. `wl-copy` stays in the background to serve the selection, so
/// its output goes nowhere: ssh would wait for it otherwise.
pub const COPY_COMMAND: &str = concat!(
    "sh -c '",
    environment!(),
    "exec wl-copy --type text/plain >/dev/null 2>&1'"
);

/// Paste text without a trailing newline; exit 1 when there is none.
pub const PASTE_COMMAND: &str = concat!(
    "sh -c '",
    environment!(),
    "exec timeout 5 wl-paste --no-newline --type text 2>/dev/null'"
);

/// Copy a PNG from stdin as `image/png`.
pub const COPY_IMAGE_COMMAND: &str = concat!(
    "sh -c '",
    environment!(),
    "exec wl-copy --type image/png >/dev/null 2>&1'"
);

/// Paste the `image/png`; exit 1 when the clipboard has none.
pub const PASTE_IMAGE_COMMAND: &str = concat!(
    "sh -c '",
    environment!(),
    "exec timeout 10 wl-paste --type image/png 2>/dev/null'"
);

/// Copy a `text/uri-list` from stdin.
pub const COPY_URIS_COMMAND: &str = concat!(
    "sh -c '",
    environment!(),
    "exec wl-copy --type text/uri-list >/dev/null 2>&1'"
);

/// Paste the `text/uri-list`; a paste of files waits for the server to have
/// them all, so it gets longer than the others. Exit 1 when there is none.
pub const PASTE_URIS_COMMAND: &str = concat!(
    "sh -c '",
    environment!(),
    "exec timeout 50 wl-paste --type text/uri-list 2>/dev/null'"
);

/// Where the trees go on the remote machine, as a shell word.
pub const DEFAULT_SCRATCH: &str = "\"$HOME/.cache/gwin-verify-clip\"";

/// How long one ordinary ssh command gets.
const SSH_TIMEOUT: Duration = Duration::from_secs(20);

/// How long one that moves files gets.
const FILES_TIMEOUT: Duration = Duration::from_secs(60);

/// The remote machine, reached with `ssh`.
pub struct Remote {
    pub ssh: PathBuf,
    pub target: String,
    /// The directory for trees, as a shell word (`DEFAULT_SCRATCH`).
    pub scratch: String,
}

/// `word` as one shell word.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Make the scratch directory afresh and unpack a tar from stdin into it.
/// Prints the directory's full path.
fn put_tree_command(scratch: &str) -> String {
    format!(
        "sh -c 'd=\"$1\"; [ -n \"$d\" ] && rm -rf \"$d\" && mkdir -p \"$d\" && tar -x -C \"$d\" -f - && cd \"$d\" && pwd' sh {scratch}"
    )
}

/// Remove the scratch directory.
fn clear_command(scratch: &str) -> String {
    format!("sh -c 'd=\"$1\"; [ -n \"$d\" ] && rm -rf \"$d\"' sh {scratch}")
}

/// A tar of the given `-C directory name` pairs on stdout. Exit 2 when one is
/// gone: the paste points at files that no longer exist.
fn read_tree_command(places: &[(String, String)]) -> String {
    let mut command = String::from("sh -c 'exec tar --format=posix -c -f - \"$@\"' sh");
    for (parent, name) in places {
        // `./` keeps a name that starts with `-` from being an option.
        command.push_str(&format!(
            " -C {} {}",
            quoted(parent),
            quoted(&format!("./{name}"))
        ));
    }
    command
}

/// What a finished remote command gave.
struct Finished {
    code: Option<i32>,
    stdout: Vec<u8>,
}

impl Remote {
    fn run(&self, command: &str, stdin: &[u8]) -> anyhow::Result<Finished> {
        self.run_for(command, stdin, SSH_TIMEOUT)
    }

    fn run_for(&self, command: &str, stdin: &[u8], timeout: Duration) -> anyhow::Result<Finished> {
        let mut cmd = Command::new(&self.ssh);
        cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "--"])
            .arg(&self.target)
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// `CREATE_NO_WINDOW` from WinBase.h: no console flashes up.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", self.ssh.display()))?;
        let mut pipe = child.stdin.take().context("ssh stdin")?;
        // A command that exits early closes the pipe; the exit code says why.
        let _ = pipe.write_all(stdin);
        drop(pipe);
        // A picture is larger than a pipe holds: read while it is written.
        let mut stdout = child.stdout.take().context("ssh stdout")?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + timeout;
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("ssh did not finish in {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = child.wait()?;
        let stdout = reader.join().map_err(|_| anyhow::anyhow!("ssh reader"))?;
        Ok(Finished {
            code: status.code(),
            stdout,
        })
    }

    /// Copies `text` on the remote machine.
    pub fn set_text(&self, text: &str) -> anyhow::Result<()> {
        let done = self.run(COPY_COMMAND, text.as_bytes())?;
        if done.code != Some(0) {
            bail!("wl-copy over ssh ended with {:?}", done.code);
        }
        Ok(())
    }

    /// The text on the remote clipboard; `None` when it holds none.
    pub fn text(&self) -> anyhow::Result<Option<String>> {
        let done = self.run(PASTE_COMMAND, &[])?;
        match done.code {
            Some(0) => Ok(Some(
                String::from_utf8(done.stdout).context("the text is not UTF-8")?,
            )),
            // wl-paste: no selection, or none of that type.
            Some(1) => Ok(None),
            // ssh itself, and a missing compositor.
            other => bail!("wl-paste over ssh ended with {other:?}"),
        }
    }

    /// Copies the PNG `png` on the remote machine as `image/png`.
    pub fn set_image(&self, png: &[u8]) -> anyhow::Result<()> {
        let done = self.run(COPY_IMAGE_COMMAND, png)?;
        if done.code != Some(0) {
            bail!("wl-copy over ssh ended with {:?}", done.code);
        }
        Ok(())
    }

    /// The `image/png` on the remote clipboard; `None` when it has none.
    pub fn image(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let done = self.run(PASTE_IMAGE_COMMAND, &[])?;
        match done.code {
            Some(0) => Ok(Some(done.stdout)),
            Some(1) => Ok(None),
            other => bail!("wl-paste over ssh ended with {other:?}"),
        }
    }

    /// Creates `spec` on the remote machine (replacing an earlier tree) and
    /// copies its top-level entries as files: a `text/uri-list`, as a file
    /// manager leaves it.
    pub fn set_files(&self, spec: &Spec) -> anyhow::Result<()> {
        let tar = spec.tar()?;
        let done = self.run_for(&put_tree_command(&self.scratch), &tar, FILES_TIMEOUT)?;
        if done.code != Some(0) {
            bail!("unpacking the tree over ssh ended with {:?}", done.code);
        }
        let base = String::from_utf8(done.stdout)
            .context("the scratch path is not UTF-8")?
            .trim_end()
            .to_owned();
        if !base.starts_with('/') {
            bail!("the scratch directory is not an absolute path: {base:?}");
        }
        let paths: Vec<String> = spec
            .roots()
            .iter()
            .map(|root| format!("{base}/{root}"))
            .collect();
        let done = self.run(COPY_URIS_COMMAND, uri_list(&paths).as_bytes())?;
        if done.code != Some(0) {
            bail!("wl-copy over ssh ended with {:?}", done.code);
        }
        Ok(())
    }

    /// The files the remote clipboard names, read as a tree: the paste of a
    /// file manager. `None` when it names none, or when what it names is not
    /// there (a paste from before, or one still on its way).
    pub fn files(&self) -> anyhow::Result<Option<Vec<Found>>> {
        let done = self.run_for(PASTE_URIS_COMMAND, &[], FILES_TIMEOUT)?;
        match done.code {
            Some(0) => {}
            // No selection, no uri-list in it, or no answer in time.
            Some(1 | 124) => return Ok(None),
            other => bail!("wl-paste over ssh ended with {other:?}"),
        }
        let body = String::from_utf8(done.stdout).context("the list is not UTF-8")?;
        let mut places = Vec::new();
        for path in parse_uri_list(&body)? {
            let (parent, name) = path
                .rsplit_once('/')
                .filter(|(_, name)| !name.is_empty())
                .with_context(|| format!("not a path to a file: {path}"))?;
            places.push((
                if parent.is_empty() { "/" } else { parent }.to_owned(),
                name.to_owned(),
            ));
        }
        if places.is_empty() {
            return Ok(None);
        }
        let done = self.run_for(&read_tree_command(&places), &[], FILES_TIMEOUT)?;
        match done.code {
            Some(0) => read_tar(&done.stdout).map(Some),
            // The files are gone.
            Some(1 | 2) => Ok(None),
            other => bail!("tar over ssh ended with {other:?}"),
        }
    }

    /// Removes the trees `set_files` made.
    pub fn clear_files(&self) -> anyhow::Result<()> {
        let done = self.run(&clear_command(&self.scratch), &[])?;
        if done.code != Some(0) {
            bail!("removing the tree over ssh ended with {:?}", done.code);
        }
        Ok(())
    }
}
