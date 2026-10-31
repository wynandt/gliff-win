//! The gate, one rule per test, each against upstream's behaviour.

use std::time::Duration;
use std::time::Instant;

use super::Moving;
use super::ResizeGate;
use super::Settled;
use super::request_for;
use crate::session::ResizeReq;

const DPI: u32 = 192;

fn req(width: u32, height: u32, scale: f32) -> ResizeReq {
    ResizeReq {
        width,
        height,
        scale,
    }
}

fn gate() -> (ResizeGate, Instant) {
    let mut g = ResizeGate::new(req(1920, 1200, 2.0));
    g.on_view((1920, 1200));
    (g, Instant::now())
}

#[test]
fn a_size_change_outside_a_drag_sends_after_it_holds_for_200ms() {
    let (mut g, t0) = gate();
    assert_eq!(
        g.on_size((2560, 1600), DPI, Moving::Free, t0),
        Some(ResizeGate::SETTLE),
        "arms the settle timer"
    );
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(req(2560, 1600, 2.0))
    );
    assert_eq!(g.requested(), req(2560, 1600, 2.0));
    assert_eq!(
        g.on_settle(t0 + 2 * ResizeGate::SETTLE),
        Settled::Nothing,
        "sent once"
    );
}

#[test]
fn a_size_that_changes_again_restarts_the_settle() {
    let (mut g, t0) = gate();
    let _ = g.on_size((2400, 1500), DPI, Moving::Free, t0);
    let t1 = t0 + Duration::from_millis(150);
    assert_eq!(
        g.on_size((2560, 1600), DPI, Moving::Free, t1),
        Some(ResizeGate::SETTLE),
        "a new size re-arms"
    );
    // The first timer fires 200 ms after t0: 150 ms of the new size only.
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Wait(Duration::from_millis(150))
    );
    assert_eq!(
        g.on_settle(t1 + ResizeGate::SETTLE),
        Settled::Send(req(2560, 1600, 2.0)),
        "only the final size is sent"
    );
}

#[test]
fn the_same_size_again_does_not_re_arm() {
    let (mut g, t0) = gate();
    let _ = g.on_size((2560, 1600), DPI, Moving::Free, t0);
    assert_eq!(
        g.on_size(
            (2560, 1600),
            DPI,
            Moving::Free,
            t0 + Duration::from_millis(50)
        ),
        None,
        "WM_SIZE repeats at the same size do not delay the send"
    );
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(req(2560, 1600, 2.0))
    );
}

#[test]
fn nothing_is_sent_during_a_drag_and_once_at_its_end() {
    let (mut g, t0) = gate();
    for (i, w) in [2000, 2200, 2400, 2561].into_iter().enumerate() {
        let t = t0 + Duration::from_millis(16 * i as u64);
        assert_eq!(
            g.on_size((w, 1600), DPI, Moving::InSizeMove, t),
            None,
            "w {w}"
        );
    }
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Nothing,
        "no timer send mid-drag"
    );
    assert_eq!(
        g.on_exit_size_move((2561, 1601), DPI),
        Some(req(2560, 1600, 2.0)),
        "odd sides round down to even"
    );
}

#[test]
fn a_drag_cancels_a_pending_settle() {
    let (mut g, t0) = gate();
    let _ = g.on_size((2400, 1500), DPI, Moving::Free, t0);
    let _ = g.on_size((2500, 1500), DPI, Moving::InSizeMove, t0);
    assert_eq!(g.on_settle(t0 + ResizeGate::SETTLE), Settled::Nothing);
}

#[test]
fn a_size_matching_the_server_view_is_not_sent() {
    let (mut g, t0) = gate();
    g.on_view((2560, 1600));
    let _ = g.on_size((2560, 1600), DPI, Moving::Free, t0);
    assert_eq!(g.on_settle(t0 + ResizeGate::SETTLE), Settled::Nothing);
    assert_eq!(g.on_exit_size_move((2560, 1600), DPI), None);
}

#[test]
fn the_last_request_is_not_repeated() {
    let (mut g, _) = gate();
    assert_eq!(
        g.on_exit_size_move((2560, 1600), DPI),
        Some(req(2560, 1600, 2.0))
    );
    assert_eq!(
        g.on_exit_size_move((2560, 1600), DPI),
        None,
        "already asked for"
    );
}

#[test]
fn a_scale_change_at_the_same_size_is_sent() {
    // Upstream gates size == view only when the scale is unchanged too.
    let (mut g, _) = gate();
    assert_eq!(
        g.on_dpi_changed((1920, 1200), 96),
        Some(req(1920, 1200, 1.0)),
        "same px, new scale"
    );
}

#[test]
fn sizes_under_64_are_skipped() {
    let (mut g, t0) = gate();
    assert_eq!(g.on_exit_size_move((63, 400), DPI), None);
    assert_eq!(g.on_exit_size_move((400, 62), DPI), None);
    // A minimised window reports 0x0.
    let _ = g.on_size((0, 0), DPI, Moving::Free, t0);
    assert_eq!(g.on_settle(t0 + ResizeGate::SETTLE), Settled::Nothing);
    assert_eq!(g.requested(), req(1920, 1200, 2.0), "nothing changed");
}

#[test]
fn the_dpi_change_sends_at_once_and_cancels_the_settle() {
    let (mut g, t0) = gate();
    let _ = g.on_size((1000, 700), 120, Moving::Free, t0);
    assert_eq!(
        g.on_dpi_changed((1200, 750), 120),
        Some(req(1200, 750, 1.25))
    );
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Nothing,
        "superseded"
    );
}

#[test]
fn request_for_makes_sides_even_at_the_protocol_scale() {
    assert_eq!(request_for((3841, 2399), 192), req(3840, 2398, 2.0));
    assert_eq!(request_for((1366, 768), 120), req(1366, 768, 1.25));
}
