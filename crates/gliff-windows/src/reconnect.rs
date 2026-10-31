//! When a tab whose session ended should try again, and how long it waits.
//!
//! Pure: no clock of its own (the caller passes `now`), no timer, no
//! session. [`Tabs`](crate::tabs::Tabs) keeps one [`Backoff`] per tab and
//! asks it what to do each time a session ends; the window arms the timer.
//!
//! Upstream retries five times at a fixed 1500 ms and resets its count as
//! soon as a session connects. This policy keeps the five retries and the
//! give-up, and differs in two places (design 5.10): the wait doubles up to
//! a cap, and the count resets only after a session has stayed up for
//! [`Policy::stable_after`], so a server that connects and drops at once
//! ends in a give-up instead of a loop.

use std::time::Duration;
use std::time::Instant;

use crate::connect::SshFailure;
use crate::tabs::End;

/// The numbers of the policy. A value, so a test can use small ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The wait before the first retry; each later one doubles it.
    pub first_delay: Duration,
    /// The longest wait.
    pub max_delay: Duration,
    /// Retries before giving up (the first try is not counted).
    pub max_retries: u8,
    /// How long a session must stay up for the count to start again.
    pub stable_after: Duration,
}

impl Policy {
    /// 1.5 s, 3, 6, 12, 24 (46.5 s in all), then a give-up; 30 s of uptime
    /// is a fresh start (longer than any one wait, so a link that only
    /// lasted a wait does not count).
    pub const STANDARD: Self = Self {
        first_delay: Duration::from_millis(1500),
        max_delay: Duration::from_secs(30),
        max_retries: 5,
        stable_after: Duration::from_secs(30),
    };

    /// The wait before retry number `attempt`, counted from 1.
    pub fn delay(&self, attempt: u8) -> Duration {
        let doublings = u32::from(attempt.saturating_sub(1));
        2u32.checked_pow(doublings)
            .and_then(|factor| self.first_delay.checked_mul(factor))
            .map_or(self.max_delay, |wait| wait.min(self.max_delay))
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// What to do with a session that ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Try again after this wait; `attempt` counts from 1.
    Retry { attempt: u8, after: Duration },
    /// Stop trying until the user asks. `exhausted` says it was the retry
    /// limit and not the kind of failure.
    GiveUp { exhausted: bool },
}

/// One tab's retry count and uptime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backoff {
    retries: u8,
    up_since: Option<Instant>,
}

impl Backoff {
    /// Retries counted since the last fresh start.
    pub fn retries(&self) -> u8 {
        self.retries
    }

    /// A session is up as of `now`. Does not reset the count: only uptime
    /// does, and that is judged when the session ends.
    pub fn up(&mut self, now: Instant) {
        self.up_since.get_or_insert(now);
    }

    /// A session ended at `now`, for a reason that is or is not worth
    /// retrying ([`retryable`]).
    pub fn ended(&mut self, policy: &Policy, now: Instant, retryable: bool) -> Next {
        let lasted = self
            .up_since
            .take()
            .map(|since| now.saturating_duration_since(since));
        if lasted.is_some_and(|lasted| lasted >= policy.stable_after) {
            self.retries = 0;
        }
        if !retryable {
            return Next::GiveUp { exhausted: false };
        }
        if self.retries >= policy.max_retries {
            return Next::GiveUp { exhausted: true };
        }
        self.retries += 1;
        Next::Retry {
            attempt: self.retries,
            after: policy.delay(self.retries),
        }
    }

    /// The user asked for a fresh start (a click), or the tab was stopped.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Whether trying again can help. Exhaustive over [`End`] and
/// [`SshFailure`], so a new class cannot be added without a choice here.
///
/// Retried: what a reboot, a network drop or a late network can cause. Not
/// retried: what the person has to fix, since under `BatchMode` the same
/// attempt fails the same way (a key, a host key, a missing program, a
/// version).
pub fn retryable(end: &End) -> bool {
    match end {
        End::Closed | End::Error(_) => true,
        End::Incompatible => false,
        End::Ssh(failure) => match failure {
            SshFailure::Refused
            | SshFailure::Unreachable
            | SshFailure::NameNotFound
            | SshFailure::Other { .. } => true,
            SshFailure::ClientMissing
            | SshFailure::HostUnknown
            | SshFailure::HostKeyChanged
            | SshFailure::KeyRefused
            | SshFailure::KeyFilePermissions
            | SshFailure::TooManyKeys
            | SshFailure::ServerMissing
            | SshFailure::ServerOld { .. }
            | SshFailure::ProtocolMismatch => false,
        },
    }
}

#[cfg(test)]
mod tests;
