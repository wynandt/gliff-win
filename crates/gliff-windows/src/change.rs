//! "Did it change?" for the parts of the window that act on a state only
//! when it is a new one: the strip lays out and repaints, and the address
//! field moves, only when what they show is different from last time. Doing
//! it every pass of the message loop keeps the thread busy and makes the
//! field blink.

/// The last value seen.
#[derive(Clone, Debug, Default)]
pub struct Last<T> {
    seen: Option<T>,
}

impl<T: PartialEq> Last<T> {
    pub fn new() -> Self {
        Self { seen: None }
    }

    /// Notes `value`; `true` when it is not the one noted before.
    pub fn update(&mut self, value: T) -> bool {
        if self.seen.as_ref() == Some(&value) {
            return false;
        }
        self.seen = Some(value);
        true
    }

    /// Forgets the value, so the next one counts as new.
    pub fn forget(&mut self) {
        self.seen = None;
    }
}

/// Counts the passes of the message loop over fixed windows of time, so a
/// loop that never sleeps shows in the log.
#[derive(Clone, Debug, Default)]
pub struct Passes {
    from_ms: u64,
    count: u32,
}

/// The length of one window of counting.
pub const PASS_WINDOW_MS: u64 = 5_000;

/// More passes than this in one window is a busy loop: the loop sleeps until
/// there is a message or a picture, so a window of video at 60 fps is about
/// 300 passes and an idle one a handful.
pub const BUSY_PASSES: u32 = 2_500;

impl Passes {
    /// One pass at `now_ms`. At the end of a window: how many passes it had.
    pub fn pass(&mut self, now_ms: u64) -> Option<u32> {
        self.count = self.count.saturating_add(1);
        if now_ms.saturating_sub(self.from_ms) < PASS_WINDOW_MS {
            return None;
        }
        let count = std::mem::take(&mut self.count);
        self.from_ms = now_ms;
        Some(count)
    }
}

#[cfg(test)]
mod tests;
