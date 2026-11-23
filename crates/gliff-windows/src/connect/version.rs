//! The server version: what `gliff-server --version` prints, and the lowest
//! version this client accepts.

use std::fmt;

/// The lowest `gliff-server` this client accepts: the one it is tested with.
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

#[cfg(test)]
mod tests;
