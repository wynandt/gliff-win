//! A wait for a thread that gives up. The UI thread must not hang on a
//! helper thread that is stuck.

use std::convert::Infallible;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

/// Waits for the thread whose end drops the sender of `ended`. The channel
/// carries [`Infallible`], so nothing can be sent on it: the only way a wait
/// ends early is the sender going away. True if the thread ended within `limit`; false if it is
/// still running, in which case the caller lets it go.
pub fn ended_within(ended: &Receiver<Infallible>, limit: Duration) -> bool {
    match ended.recv_timeout(limit) {
        Err(RecvTimeoutError::Disconnected) => true,
        Err(RecvTimeoutError::Timeout) => false,
        Ok(never) => match never {},
    }
}

#[cfg(test)]
mod tests;
