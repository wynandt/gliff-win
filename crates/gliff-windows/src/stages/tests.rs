use std::time::Duration;
use std::time::Instant;

use super::Screen;
use super::Stages;
use crate::present_log::Stage;
use crate::session::ResizeReq;

fn req(width: u32, height: u32) -> ResizeReq {
    ResizeReq {
        width,
        height,
        scale: 2.0,
    }
}

#[test]
fn starting_until_the_first_picture_then_windowed() {
    let t0 = Instant::now();
    let mut s = Stages::new();
    assert_eq!(s.current(t0), Stage::Starting, "nothing shown yet");
    s.on_shown((1920, 1080));
    assert_eq!(s.current(t0), Stage::Windowed, "after the first picture");
}

#[test]
fn a_resize_holds_resizing_until_a_picture_of_its_size() {
    let t0 = Instant::now();
    let mut s = Stages::new();
    s.on_shown((1920, 1080));
    s.on_resize(req(2560, 1600), t0);
    assert_eq!(s.current(t0), Stage::Resizing, "just sent");
    s.on_shown((1920, 1080));
    assert_eq!(
        s.current(t0 + Duration::from_secs(1)),
        Stage::Resizing,
        "a picture of the old size does not end it"
    );
    s.on_shown((2560, 1600));
    assert_eq!(s.current(t0), Stage::Windowed, "the new size ends it");
}

#[test]
fn fullscreen_after_its_resize_lands() {
    let t0 = Instant::now();
    let mut s = Stages::new();
    s.on_shown((2560, 1600));
    s.on_screen(Screen::Fullscreen);
    s.on_resize(req(3840, 2400), t0);
    assert_eq!(s.current(t0), Stage::Resizing, "the F11 resize in flight");
    s.on_shown((3840, 2400));
    assert_eq!(s.current(t0), Stage::Fullscreen, "landed");
}

#[test]
fn a_resize_the_server_ignores_gives_up_after_three_seconds() {
    let t0 = Instant::now();
    let mut s = Stages::new();
    s.on_shown((3840, 2400));
    s.on_resize(req(2560, 1600), t0);
    let almost = t0 + Stages::GIVE_UP - Duration::from_millis(1);
    assert_eq!(s.current(almost), Stage::Resizing, "inside the window");
    assert_eq!(
        s.current(t0 + Stages::GIVE_UP),
        Stage::Windowed,
        "mirror mode keeps its view; the stage moves on"
    );
}

#[test]
fn closing_ends_every_stage_for_good() {
    let t0 = Instant::now();
    let mut s = Stages::new();
    s.on_shown((3840, 2400));
    s.on_screen(Screen::Fullscreen);
    s.on_close();
    assert_eq!(
        s.current(t0),
        Stage::Closing,
        "WM_CLOSE ends the fullscreen stage"
    );
    s.on_resize(req(1920, 1080), t0);
    s.on_shown((1920, 1080));
    assert_eq!(
        s.current(t0 + Duration::from_secs(10)),
        Stage::Closing,
        "nothing after the close reopens a measured stage"
    );
}
