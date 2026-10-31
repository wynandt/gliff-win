use std::time::Duration;
use std::time::Instant;

use super::BACKOFF;
use super::MIN_INPUTS;
use super::SILENCE;
use super::Watch;

fn presses(watch: &mut Watch, now: Instant, n: u32) {
    for _ in 0..n {
        watch.input(now);
    }
}

#[test]
fn one_click_on_a_quiet_screen_never_resyncs() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    watch.input(t0);
    assert_eq!(watch.due(), None);
    assert!(!watch.take(t0 + Duration::from_secs(600)));
}

#[test]
fn enough_unanswered_input_is_due_three_seconds_after_the_first() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    watch.input(t0);
    watch.input(t0 + Duration::from_secs(1));
    watch.input(t0 + Duration::from_secs(2));
    assert_eq!(watch.unanswered(), MIN_INPUTS);
    assert_eq!(watch.due(), Some(t0 + SILENCE));
    assert!(!watch.take(t0 + SILENCE - Duration::from_millis(1)));
    assert!(watch.take(t0 + SILENCE));
    assert_eq!(watch.due(), None, "the input was used up");
}

#[test]
fn a_picture_clears_the_count() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    presses(&mut watch, t0, MIN_INPUTS);
    watch.answered();
    assert_eq!(watch.due(), None);
    presses(&mut watch, t0 + Duration::from_secs(10), MIN_INPUTS - 1);
    assert_eq!(watch.due(), None, "the count starts again");
}

#[test]
fn a_second_resync_waits_out_the_backoff() {
    let t0 = Instant::now();
    let mut watch = Watch::default();
    presses(&mut watch, t0, MIN_INPUTS);
    assert!(watch.take(t0 + SILENCE));
    let t1 = t0 + SILENCE + Duration::from_secs(1);
    presses(&mut watch, t1, MIN_INPUTS);
    assert_eq!(
        watch.due(),
        Some(t0 + SILENCE + BACKOFF),
        "not before the backoff has passed"
    );
    assert!(!watch.take(t1 + SILENCE));
    assert!(watch.take(t0 + SILENCE + BACKOFF));
}
