//! The Windows programs the client starts, by full path.
//!
//! A bare program name is looked up in more places than the system folders, so
//! the client names the Windows copy instead. An `--ssh` or `ssh` setting from
//! the user is never rewritten; only the default is.

use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

/// Where Windows keeps its OpenSSH client, under the Windows folder.
const SSH_UNDER_ROOT: &str = r"System32\OpenSSH\ssh.exe";

/// The name used when the OpenSSH client is not at the Windows location.
const SSH_FALLBACK: &str = "ssh.exe";

/// The default ssh client: `%SystemRoot%\System32\OpenSSH\ssh.exe`, or the bare
/// name `ssh.exe` only when that file does not exist.
pub fn default_ssh() -> PathBuf {
    choose_ssh(std::env::var_os("SystemRoot"), Path::is_file)
}

/// `%SystemRoot%\explorer.exe`, which opens files, folders and web addresses.
#[cfg(windows)]
pub fn explorer() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    Path::new(&root).join("explorer.exe")
}

fn choose_ssh(system_root: Option<OsString>, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let Some(root) = system_root else {
        return PathBuf::from(SSH_FALLBACK);
    };
    let full = Path::new(&root).join(SSH_UNDER_ROOT);
    if exists(&full) {
        full
    } else {
        PathBuf::from(SSH_FALLBACK)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::choose_ssh;

    #[test]
    fn the_default_ssh_is_the_system_copy_and_the_bare_name_only_when_it_is_missing() {
        let root = || Some(OsString::from(r"C:\Windows"));
        let found = choose_ssh(root(), |_| true).to_string_lossy().into_owned();
        assert!(found.starts_with(r"C:\Windows"), "{found}");
        assert!(found.ends_with(r"System32\OpenSSH\ssh.exe"), "{found}");
        assert_eq!(choose_ssh(root(), |_| false).to_string_lossy(), "ssh.exe");
        assert_eq!(choose_ssh(None, |_| true).to_string_lossy(), "ssh.exe");
    }
}
