//! The server version: what `gliff-server --version` prints, the lowest
//! version this client accepts, and the one extra ssh call that asks.
//!
//! The call runs after a server-class failure or when the person presses
//! "Check again" (design 11.4). Nothing runs it at start.

use std::fmt;

use super::diag::Phase;
use super::diag::SshFailure;
use super::diag::classify;
use super::diag::from_spawn_error;
use super::runner::Cmd;
use super::runner::ToolError;
use super::runner::Tools;
use crate::ssh::SshSpec;

/// The lowest `gliff-server` this client accepts: the one it is tested with
/// (decision O8).
pub const MIN_SERVER: ServerVersion = ServerVersion::new(0, 4, 1);

/// A `major.minor.patch` version. Ordered the way versions are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ServerVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The version in what `gliff-server --version` printed: the first line
    /// is `gliff-server X.Y.Z`. A pre-release or build suffix (`-rc1`,
    /// `+abc`) is ignored. `None` when the text is anything else.
    pub fn parse(printed: &str) -> Option<Self> {
        let line = printed.lines().find(|line| !line.trim().is_empty())?;
        let number = line.trim().strip_prefix("gliff-server ")?.trim();
        let number = number.split(['-', '+']).next()?;
        let mut parts = number.split('.');
        let version = Self::new(
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for ServerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Asks the server for its version over its own ssh and judges the answer:
/// the version when it is new enough, else the failure to show.
///
/// A failed call is classified like a failed session ([`Phase::VersionCheck`]).
/// A call that ends but prints no version is `Other`, since the server ran
/// and said something this client does not know.
pub fn check_server(tools: &dyn Tools, spec: &SshSpec) -> Result<ServerVersion, SshFailure> {
    let cmd = Cmd::version(spec).map_err(|e| SshFailure::Other {
        last_line: e.to_string(),
    })?;
    let output = match tools.run(&cmd) {
        Ok(output) => output,
        Err(ToolError::Spawn(e)) => return Err(from_spawn_error(&e)),
        Err(ToolError::Timeout) => return Err(SshFailure::Unreachable),
    };
    if output.exit != Some(0) {
        return Err(classify(&output.stderr, output.exit, Phase::VersionCheck));
    }
    match ServerVersion::parse(&output.stdout) {
        Some(found) if found >= MIN_SERVER => Ok(found),
        Some(found) => Err(SshFailure::ServerOld { found }),
        None => Err(SshFailure::Other {
            last_line: "the server did not print a version".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests;
