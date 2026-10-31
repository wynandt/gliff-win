//! The visible countdown that tells the maintainer when to press a key
//! during the lock step: the pure parts (the seconds left and the words),
//! which the Windows overlay and the console both use.

/// Whole seconds left of `total_secs` after `elapsed_ms`, rounded up, so the
/// overlay reads `20s` at the start and `0s` only at the very end.
pub fn seconds_left(total_secs: u32, elapsed_ms: u64) -> u32 {
    let total_ms = u64::from(total_secs) * 1000;
    let left_ms = total_ms.saturating_sub(elapsed_ms);
    // At most `total_secs`, so it fits.
    u32::try_from(left_ms.div_ceil(1000)).unwrap_or(total_secs)
}

/// What the overlay shows, e.g. `Press Win+L now, 20s`.
pub fn overlay_text(instruction: &str, left_secs: u32) -> String {
    format!("{instruction}, {left_secs}s")
}

#[cfg(test)]
mod tests {
    use super::overlay_text;
    use super::seconds_left;

    #[test]
    fn the_overlay_starts_at_the_full_time_and_counts_down() {
        assert_eq!(seconds_left(20, 0), 20);
        assert_eq!(seconds_left(20, 1), 20);
        assert_eq!(seconds_left(20, 999), 20);
        assert_eq!(seconds_left(20, 1000), 19);
        assert_eq!(seconds_left(20, 19_001), 1);
    }

    #[test]
    fn it_reads_zero_at_the_end_and_never_goes_below() {
        assert_eq!(seconds_left(20, 20_000), 0);
        assert_eq!(seconds_left(20, 9_000_000), 0);
        assert_eq!(seconds_left(0, 0), 0);
    }

    #[test]
    fn the_text_names_the_press_and_the_seconds_left() {
        assert_eq!(overlay_text("Press Win+L now", 20), "Press Win+L now, 20s");
        assert_eq!(overlay_text("Press Win+L now", 0), "Press Win+L now, 0s");
    }
}
