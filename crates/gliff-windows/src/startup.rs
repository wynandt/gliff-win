//! What the client knows before it opens a window: the config folder, the
//! files in it, and the settings after flag, file and default are merged.
//!
//! Pure of Win32, like the modules it joins. Reading changes nothing on
//! disk, so `--print-settings` can use the same path as a normal start.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli::Args;
use crate::config::ConfigDir;
use crate::config::ConfigFile;
use crate::config::NoConfigDir;
use crate::history::HistoryFile;
use crate::history::HostTarget;
use crate::machine_choices::Connection;
use crate::machine_choices::Locks;
use crate::settings::ChordError;
use crate::settings::Resolved;
use crate::settings::resolve;

/// Why the client cannot start from its settings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartupError {
    #[error(transparent)]
    NoConfigDir(#[from] NoConfigDir),
    #[error(transparent)]
    Chords(#[from] ChordError),
}

/// The loaded files and the merged settings.
#[derive(Debug)]
pub struct Startup {
    pub dir: ConfigDir,
    pub config: ConfigFile,
    pub history: HistoryFile,
    pub resolved: Resolved,
    /// What the flags and the file fix for every machine, so that a
    /// machine's own choice cannot change it.
    pub locks: Locks,
    /// What loading ignored or dropped, for the log.
    pub notes: Vec<String>,
}

impl Startup {
    /// From the command line and the process environment.
    pub fn load(args: &Args) -> Result<Self, StartupError> {
        Self::load_with(
            args,
            std::env::var_os(crate::config::CONFIG_DIR_ENV),
            std::env::var_os("APPDATA").map(PathBuf::from),
        )
    }

    /// With the environment given, so a test does not depend on the real one.
    pub fn load_with(
        args: &Args,
        env_dir: Option<OsString>,
        app_data: Option<PathBuf>,
    ) -> Result<Self, StartupError> {
        let dir = ConfigDir::resolve(args.config_dir.clone(), env_dir, app_data)?;
        let (config, mut notes) = ConfigFile::load(&dir.config_file());
        let resolved = resolve(&args.layer(), config.settings())?;
        let locks = Locks::new(&args.layer(), &resolved);
        let (history, history_notes) =
            HistoryFile::load(&dir.history_file(), resolved.settings.remember_recent_hosts);
        notes.extend(history_notes);
        Ok(Self {
            dir,
            config,
            history,
            resolved,
            locks,
            notes,
        })
    }

    /// The `--print-settings` report.
    pub fn report(&self) -> serde_json::Value {
        let tabs = self.config.tabs();
        serde_json::json!({
            "config_dir": self.dir.path().display().to_string(),
            "config_dir_is_default": self.dir.is_default(),
            "settings": self.resolved.settings.to_json(&self.resolved.sources),
            "connection": self.connection_report(),
            "machines": names_of(tabs.machines()),
            "open": names_of(tabs.open()),
            "shown": tabs.shown().map(HostTarget::as_str),
            "recent_hosts": names_of(self.history.hosts().hosts()),
            "notes": self.notes,
        })
    }
}

impl Startup {
    /// What each machine, with a tab or a saved choice, runs with.
    fn connection_report(&self) -> serde_json::Value {
        let choices = self.config.choices().borrow();
        let mut machines: Vec<HostTarget> = self.config.tabs().machines().to_vec();
        for (machine, _) in choices.machines() {
            if !machines.iter().any(|m| m.same(machine)) {
                machines.push(machine.clone());
            }
        }
        machines
            .iter()
            .map(|machine| {
                let connection = Connection::of(
                    &self.resolved.settings,
                    &self.resolved.sources,
                    &self.locks,
                    choices.of(machine),
                );
                (machine.to_string(), connection.to_json())
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }
}

fn names_of(hosts: &[HostTarget]) -> Vec<&str> {
    hosts.iter().map(HostTarget::as_str).collect()
}

/// `--print-settings`: the report on standard output, or the reason on
/// standard error. Starts no session and changes nothing.
pub fn print_settings(args: &Args) -> ExitCode {
    match Startup::load(args) {
        Ok(startup) => {
            let text = serde_json::to_string_pretty(&startup.report())
                .expect("a JSON value is always text");
            // A Windows GUI program may have no standard output; then there
            // is nowhere to print, and that is not an error.
            let _ = writeln!(std::io::stdout(), "{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "gliff-windows: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests;
