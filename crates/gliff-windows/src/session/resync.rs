//! When to resync: the pure watch behind `Core::resync`.
//!
//! A server whose output was reset behind its back (a compositor reload
//! discards the mode a headless output was given at runtime) keeps the link
//! and sends nothing, so the client sees only that nothing answers its input.
//! A still screen also sends nothing, so silence alone is not a stall:
//! [`Watch`] needs [`MIN_INPUTS`] key presses, button presses or wheel
//! messages with no picture since, and the first of them [`SILENCE`] old. It owns no
//! clock; the driver passes `now`.

use std::time::Duration;
use std::time::Instant;

/// How long the first unanswered input must be old.
pub const SILENCE: Duration = Duration::from_secs(3);
/// The fewest unanswered inputs that count as a stall: one click on a quiet
/// desktop is not.
pub const MIN_INPUTS: u32 = 3;
/// The least time between two resyncs, so a server that cannot be helped is
/// not resized in a loop.
pub const BACKOFF: Duration = Duration::from_secs(30);

/// Unanswered input and the last resync; see the module docs.
#[derive(Debug, Clone, Default)]
pub struct Watch {
    /// When the first input with no answer since was sent, and how many.
    unanswered: Option<(Instant, u32)>,
    last_resync: Option<Instant>,
}

impl Watch {
    /// A key press, button press or wheel message was sent.
    pub fn input(&mut self, now: Instant) {
        self.unanswered = match self.unanswered {
            Some((since, n)) => Some((since, n.saturating_add(1))),
            None => Some((now, 1)),
        };
    }

    /// A `VideoFrame` or `StreamConfig` arrived: the server is answering.
    pub fn answered(&mut self) {
        self.unanswered = None;
    }

    /// When a resync becomes due, if the input so far calls for one.
    pub fn due(&self) -> Option<Instant> {
        let (since, n) = self.unanswered?;
        if n < MIN_INPUTS {
            return None;
        }
        let due = since + SILENCE;
        Some(self.last_resync.map_or(due, |at| due.max(at + BACKOFF)))
    }

    /// True once, when a resync is due at `now`: the caller must perform it.
    pub fn take(&mut self, now: Instant) -> bool {
        if self.due().is_none_or(|due| now < due) {
            return false;
        }
        self.unanswered = None;
        self.last_resync = Some(now);
        true
    }

    /// How many inputs are unanswered, for the log.
    pub fn unanswered(&self) -> u32 {
        self.unanswered.map_or(0, |(_, n)| n)
    }
}

#[cfg(test)]
mod tests;
