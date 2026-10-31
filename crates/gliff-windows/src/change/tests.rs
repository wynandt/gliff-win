use super::Last;
use super::PASS_WINDOW_MS;
use super::Passes;
use crate::strip::Rect;
use crate::strip::edit_target;

#[test]
fn the_first_value_is_new_and_the_same_value_again_is_not() {
    let mut last = Last::new();
    assert!(last.update(3));
    for _ in 0..5 {
        assert!(!last.update(3), "nothing changed");
    }
    assert!(last.update(4));
    assert!(!last.update(4));
}

#[test]
fn a_value_that_goes_back_is_new_again() {
    let mut last = Last::new();
    assert!(last.update("a"));
    assert!(last.update("b"));
    assert!(last.update("a"));
}

#[test]
fn a_forgotten_value_counts_as_new() {
    let mut last = Last::new();
    assert!(last.update(1));
    last.forget();
    assert!(last.update(1));
}

/// The edit field is moved only when its place is a new one: the strip asks
/// on every layout, which is every pass of the message loop.
#[test]
fn the_edit_field_is_placed_once_for_a_place_that_stays() {
    let bar = Rect {
        left: 40,
        top: 4,
        right: 300,
        bottom: 28,
    };
    let mut at = Last::new();
    let mut placed = 0;
    for _ in 0..100 {
        if at.update(edit_target(Some(bar), true)) {
            placed += 1;
        }
    }
    assert_eq!(placed, 1, "one move for a hundred layouts");
    let wider = Rect { right: 320, ..bar };
    assert!(at.update(edit_target(Some(wider), true)), "a new place");
    assert!(at.update(edit_target(Some(wider), false)), "hidden: new");
    assert!(!at.update(edit_target(None, true)), "hidden again, same");
}

#[test]
fn the_edit_field_has_a_place_only_while_the_bar_is_open_and_the_strip_shown() {
    let bar = Rect {
        left: 1,
        top: 2,
        right: 3,
        bottom: 4,
    };
    assert_eq!(edit_target(Some(bar), true), Some(bar));
    assert_eq!(edit_target(Some(bar), false), None);
    assert_eq!(edit_target(None, true), None);
}

#[test]
fn passes_are_counted_per_window_and_reported_at_its_end() {
    let mut passes = Passes::default();
    for ms in 0..PASS_WINDOW_MS - 1 {
        assert_eq!(passes.pass(ms), None);
    }
    // The 5000th pass (counted from 1) closes the window.
    assert_eq!(passes.pass(PASS_WINDOW_MS), Some(PASS_WINDOW_MS as u32));
    assert_eq!(passes.pass(PASS_WINDOW_MS + 1), None, "a new window");
    assert_eq!(passes.pass(2 * PASS_WINDOW_MS), Some(2));
}
