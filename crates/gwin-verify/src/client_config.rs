//! Every check that starts the client gives it a fresh, empty config folder,
//! so a check never reads or writes the real `%APPDATA%\gliff-windows` files
//! (the settings, the machine list and the recent hosts of whoever runs it).
//!
//! The folder is `client-config` inside the check's own log folder and is
//! emptied before each start.

use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

/// The folder name inside a check's log folder.
const FOLDER: &str = "client-config";

/// An empty config folder for one client start.
pub fn fresh_config_dir(log_dir: &Path) -> io::Result<PathBuf> {
    let dir = log_dir.join(FOLDER);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The client's command line so far: the program, its `--config-dir` and
/// `--headless`. Every launcher starts from this, never from
/// `Command::new(client)`.
///
/// The client's own default is to mirror the remote machine's focused screen.
/// Every gate was written and passed on the private screen, so each one asks
/// for it by name and keeps testing what it did.
pub fn client_command(client: &Path, log_dir: &Path) -> io::Result<Command> {
    let mut command = Command::new(client);
    command.arg("--config-dir").arg(fresh_config_dir(log_dir)?);
    command.arg("--headless");
    Ok(command)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    use super::client_command;
    use super::fresh_config_dir;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gwin-verify-client-config-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary folder");
        dir
    }

    #[test]
    fn the_folder_is_empty_each_time_and_inside_the_log_folder() {
        let log = scratch("fresh");
        let first = fresh_config_dir(&log).expect("folder");
        assert!(first.starts_with(&log));
        fs::write(first.join("config.toml"), "[settings]\n").expect("write");
        let second = fresh_config_dir(&log).expect("folder");
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(&second).expect("read").count(), 0, "emptied");
        let _ = fs::remove_dir_all(&log);
    }

    #[test]
    fn the_command_starts_with_the_config_folder() {
        let log = scratch("command");
        let command = client_command(Path::new("gliff-windows"), &log).expect("command");
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args[0], "--config-dir");
        assert_eq!(Path::new(args[1]), log.join("client-config"));
        assert_eq!(
            args[2], "--headless",
            "the gates keep the private screen they were built for"
        );
        assert_eq!(args.len(), 3);
        let _ = fs::remove_dir_all(&log);
    }

    /// A launcher that built its own `Command::new(client)` would read the
    /// real config folder. Only [`client_command`] may, or a command that
    /// gives `--config-dir` itself; the lock helper (`--restore-lock-policy`)
    /// never reads the config.
    #[test]
    fn no_launcher_builds_a_client_command_without_the_config_folder() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut folders = vec![src];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).expect("folder") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension() != Some(OsStr::new("rs"))
                    || path.file_name() == Some(OsStr::new("client_config.rs"))
                {
                    continue;
                }
                let text = fs::read_to_string(&path).expect("source");
                let lines: Vec<&str> = text.lines().collect();
                for (n, line) in lines.iter().enumerate() {
                    if line.contains("Command::new(") && line.contains("client") {
                        let after = lines[n..lines.len().min(n + 3)].join(" ");
                        if !after.contains("--restore-lock-policy")
                            && !after.contains("--config-dir")
                        {
                            offenders.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "use client_command: {offenders:?}");
    }
}
