use super::Pointer;
use super::Reveal;
use super::Rule;
use super::SLIDE_MS;
use super::Slide;

const DPIS: [u32; 4] = [96, 120, 144, 192];
const HEIGHT: i32 = 32;

fn rule() -> Rule {
    Rule::at(96, HEIGHT)
}

fn reveal() -> Reveal {
    Reveal::new(rule())
}

#[test]
fn the_trigger_and_the_leave_margin_scale_with_the_display() {
    for dpi in DPIS {
        let r = Rule::at(dpi, 32);
        assert_eq!(r.edge, (2 * dpi as i32 + 48) / 96, "{dpi}");
        assert_eq!(r.leave, (8 * dpi as i32 + 48) / 96, "{dpi}");
    }
}

#[test]
fn the_pointer_within_two_pixels_of_the_top_edge_brings_the_strip_out() {
    let mut r = reveal();
    assert!(
        !r.pointer(Pointer::At(2)),
        "two pixels down is not the edge"
    );
    assert!(!r.is_out());
    assert!(r.pointer(Pointer::At(1)), "the change is reported");
    assert!(r.is_out());
    let mut r = reveal();
    assert!(r.pointer(Pointer::At(0)));
    assert!(r.is_out());
}

#[test]
fn the_pointer_far_from_the_edge_or_outside_the_window_does_nothing() {
    let mut r = reveal();
    for y in [3, 10, 100, 1079] {
        assert!(!r.pointer(Pointer::At(y)), "{y}");
    }
    assert!(!r.pointer(Pointer::Away));
    assert!(!r.is_out());
}

#[test]
fn the_strip_stays_while_the_pointer_is_over_it_and_for_eight_pixels_below() {
    let mut r = reveal();
    r.pointer(Pointer::At(0));
    for y in [0, 10, HEIGHT - 1, HEIGHT, HEIGHT + 8] {
        assert!(!r.pointer(Pointer::At(y)), "{y}");
        assert!(r.is_out(), "{y}");
    }
    assert!(
        r.pointer(Pointer::At(HEIGHT + 9)),
        "more than 8 below hides"
    );
    assert!(!r.is_out());
}

#[test]
fn the_strip_goes_when_the_pointer_leaves_the_window() {
    let mut r = reveal();
    r.pointer(Pointer::At(0));
    assert!(r.pointer(Pointer::Away));
    assert!(!r.is_out());
}

#[test]
fn it_hides_again_and_comes_out_again() {
    let mut r = reveal();
    r.pointer(Pointer::At(0));
    r.pointer(Pointer::At(500));
    assert!(!r.is_out());
    assert!(r.pointer(Pointer::At(1)));
    assert!(r.is_out());
}

#[test]
fn the_strip_stays_out_while_it_is_held_whatever_the_pointer_does() {
    let mut r = reveal();
    assert!(r.hold(true), "holding brings it out at once");
    assert!(r.is_out());
    assert!(!r.pointer(Pointer::At(800)));
    assert!(!r.pointer(Pointer::Away));
    assert!(r.is_out());
}

#[test]
fn letting_go_hides_it_when_the_pointer_is_far_and_keeps_it_when_the_pointer_is_over_it() {
    let mut r = reveal();
    r.hold(true);
    r.pointer(Pointer::At(800));
    assert!(r.hold(false), "the pointer is far: it goes");
    assert!(!r.is_out());

    let mut r = reveal();
    r.hold(true);
    r.pointer(Pointer::At(10));
    assert!(!r.hold(false), "the pointer is on the strip: it stays");
    assert!(r.is_out());

    let mut r = reveal();
    r.hold(true);
    assert!(r.hold(false), "no pointer is known: it goes");
    assert!(!r.is_out());
}

#[test]
fn leaving_full_screen_resets_it() {
    let mut r = reveal();
    r.hold(true);
    r.pointer(Pointer::At(0));
    r.reset();
    assert!(!r.is_out());
    assert!(!r.pointer(Pointer::At(500)), "and no hold is left over");
}

#[test]
fn a_new_scale_changes_the_margins() {
    let mut r = reveal();
    r.set_rule(Rule::at(192, 64));
    assert!(r.pointer(Pointer::At(3)), "four device px at 200%");
    assert!(!r.pointer(Pointer::At(64 + 16)));
    assert!(r.pointer(Pointer::At(64 + 17)));
}

#[test]
fn a_hidden_slide_has_the_strip_above_the_window() {
    let s = Slide::new(HEIGHT);
    assert_eq!(s.top(0), -HEIGHT);
    assert_eq!(s.top(10_000), -HEIGHT);
    assert!(s.at_rest(0));
}

#[test]
fn sliding_out_takes_the_slide_time_and_never_goes_backward() {
    let mut s = Slide::new(HEIGHT);
    s.retarget(true, 1000);
    assert_eq!(s.top(1000), -HEIGHT);
    assert!(!s.at_rest(1000));
    let mut last = -HEIGHT;
    for t in (1000..=1000 + SLIDE_MS).step_by(5) {
        let top = s.top(t);
        assert!(top >= last, "{t}: {top} after {last}");
        assert!((-HEIGHT..=0).contains(&top));
        last = top;
    }
    assert_eq!(s.top(1000 + SLIDE_MS), 0);
    assert_eq!(s.top(1000 + SLIDE_MS + 500), 0);
    assert!(s.at_rest(1000 + SLIDE_MS));
    let mid = s.top(1000 + SLIDE_MS / 2);
    assert!(mid > -HEIGHT && mid < 0, "half way: {mid}");
}

#[test]
fn sliding_in_reverses_from_where_it_is() {
    let mut s = Slide::new(HEIGHT);
    s.retarget(true, 0);
    let half = s.top(SLIDE_MS / 2);
    s.retarget(false, SLIDE_MS / 2);
    assert_eq!(s.top(SLIDE_MS / 2), half, "no jump when it turns round");
    assert_eq!(s.top(SLIDE_MS / 2 + SLIDE_MS * 2), -HEIGHT);
    assert!(s.at_rest(SLIDE_MS / 2 + SLIDE_MS * 2));
}

#[test]
fn the_same_target_again_does_not_restart_the_slide() {
    let mut s = Slide::new(HEIGHT);
    s.retarget(true, 0);
    let at = s.top(SLIDE_MS / 3);
    s.retarget(true, SLIDE_MS / 3);
    assert_eq!(s.top(SLIDE_MS / 3), at);
    assert_eq!(s.top(SLIDE_MS), 0, "it still ends on time");
}

#[test]
fn a_snap_moves_it_at_once() {
    let mut s = Slide::new(HEIGHT);
    s.snap(true);
    assert_eq!(s.top(0), 0);
    assert!(s.at_rest(0));
    s.snap(false);
    assert_eq!(s.top(0), -HEIGHT);
}

#[test]
fn a_new_height_keeps_the_state() {
    let mut s = Slide::new(32);
    s.snap(true);
    s.set_height(64);
    assert_eq!(s.top(0), 0);
    s.snap(false);
    assert_eq!(s.top(0), -64);
}
