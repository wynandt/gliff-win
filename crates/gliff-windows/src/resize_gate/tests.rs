use std::time::Duration;
use std::time::Instant;

use super::Moving;
use super::ResizeGate;
use super::Settled;
use super::apply_live_scale;
use super::request_for;
use super::request_for_scale;
use crate::cli::Screen;
use crate::machine_choices::Connection;
use crate::machine_choices::Origin;
use crate::machine_choices::Running;
use crate::machine_choices::Scale;
use crate::session::ResizeReq;
use crate::strip::menu::Command;
use crate::strip::menu::Entry;
use crate::strip::menu::run;
use crate::strip::menu::tab_menu;
use crate::tabs::Effect;
use crate::tabs::Status;
use crate::tabs::Tabs;

const DPI: u32 = 192;

fn req(width: u32, height: u32, scale: f32) -> ResizeReq {
    ResizeReq {
        width,
        height,
        scale,
    }
}

fn gate() -> (ResizeGate, Instant) {
    let g = ResizeGate::new(req(1920, 1200, 2.0), Scale::Auto);
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
fn a_resize_back_supersedes_a_request_for_another_size() {
    let (mut g, t0) = gate();
    assert_eq!(
        g.on_exit_size_move((2560, 1600), DPI),
        Some(req(2560, 1600, 2.0))
    );
    let _ = g.on_size((1920, 1200), DPI, Moving::Free, t0);
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(req(1920, 1200, 2.0))
    );
    assert_eq!(g.on_exit_size_move((1920, 1200), DPI), None);
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
    let (mut g, _) = gate();
    assert_eq!(
        g.on_dpi_changed((1920, 1200), 96, Scale::Auto),
        Some(req(1920, 1200, 1.0)),
        "same px, new scale"
    );
}

#[test]
fn sizes_under_64_are_skipped() {
    let (mut g, t0) = gate();
    assert_eq!(g.on_exit_size_move((63, 400), DPI), None);
    assert_eq!(g.on_exit_size_move((400, 62), DPI), None);
    let _ = g.on_size((0, 0), DPI, Moving::Free, t0);
    assert_eq!(g.on_settle(t0 + ResizeGate::SETTLE), Settled::Nothing);
    assert_eq!(g.requested(), req(1920, 1200, 2.0), "nothing changed");
}

#[test]
fn the_dpi_change_sends_at_once_and_cancels_the_settle() {
    let (mut g, t0) = gate();
    let _ = g.on_size((1000, 700), 120, Moving::Free, t0);
    assert_eq!(
        g.on_dpi_changed((1200, 750), 120, Scale::Auto),
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

#[test]
fn request_for_scale_sends_physical_pixels_with_auto_or_fixed_scale() {
    assert_eq!(
        request_for_scale((2881, 1801), 192, Scale::Auto),
        req(2880, 1800, 2.0)
    );
    assert_eq!(
        request_for_scale((2881, 1801), 192, Scale::Percent125),
        req(2880, 1800, 1.25)
    );
    assert_eq!(
        request_for_scale((2881, 1801), 192, Scale::Percent160),
        req(2880, 1800, 1.6)
    );
    assert_eq!(
        request_for_scale((2881, 1801), 144, Scale::Percent200),
        req(2880, 1800, 2.0)
    );
}

#[test]
fn fixed_scale_is_preserved_across_resizes_on_the_same_screen() {
    let t0 = Instant::now();
    let initial = request_for_scale((2880, 1800), 192, Scale::Percent160);
    assert_eq!(initial, req(2880, 1800, 1.6));
    let mut g = ResizeGate::new(initial, Scale::Percent160);
    assert_eq!(
        g.on_exit_size_move((2400, 1500), 192),
        Some(req(2400, 1500, 1.6)),
        "fixed scale 1.6 must not revert to dpi/96 (2.0)"
    );
    assert_eq!(
        g.on_size((2000, 1250), 192, Moving::Free, t0),
        Some(ResizeGate::SETTLE)
    );
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(req(2000, 1250, 1.6))
    );
}

#[test]
fn fixed_scale_stays_fixed_when_moving_to_a_screen_with_another_dpi() {
    let initial = request_for_scale((2880, 1800), 192, Scale::Percent160);
    let mut g = ResizeGate::new(initial, Scale::Percent160);
    assert_eq!(
        g.on_dpi_changed((1920, 1080), 96, Scale::Percent160),
        Some(req(1920, 1080, 1.6))
    );
    assert_eq!(
        g.on_dpi_changed((1920, 1080), 144, Scale::Percent160),
        None,
        "same physical size and same fixed scale"
    );
}

#[test]
fn a_saved_scale_change_takes_effect_on_screen_move_not_on_same_screen_resize() {
    let t0 = Instant::now();
    let initial = request_for_scale((2880, 1800), 192, Scale::Auto);
    let mut g = ResizeGate::new(initial, Scale::Auto);

    assert_eq!(
        g.on_exit_size_move((2560, 1600), 192),
        Some(req(2560, 1600, 2.0)),
        "same screen keeps the session's active Auto scale"
    );
    assert_eq!(
        g.on_dpi_changed((2560, 1600), 192, Scale::Percent160),
        Some(req(2560, 1600, 1.6)),
        "screen move adopts the new saved scale even at the same size and dpi"
    );
    assert_eq!(g.scale(), Scale::Percent160);
    assert_eq!(
        g.on_size((2000, 1200), 144, Moving::Free, t0),
        Some(ResizeGate::SETTLE)
    );
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(req(2000, 1200, 1.6))
    );
}

#[test]
fn hidden_tab_scale_update_takes_effect_when_size_catches_up() {
    let t0 = Instant::now();
    let initial = request_for_scale((1920, 1200), 192, Scale::Auto);
    let mut g = ResizeGate::new(initial, Scale::Auto);
    g.set_scale(Scale::Percent125);
    assert_eq!(g.requested(), req(1920, 1200, 2.0), "no immediate send");
    assert_eq!(
        g.on_size((1920, 1200), 192, Moving::Free, t0),
        Some(ResizeGate::SETTLE)
    );
    assert_eq!(
        g.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(req(1920, 1200, 1.25))
    );
}

fn own(scale: Scale) -> Connection {
    Connection {
        screen: Screen::Own,
        screen_origin: Origin::Machine,
        screen_lock: None,
        scale,
        scale_origin: Origin::Machine,
        share_clipboard: true,
        share_origin: Origin::Default,
        share_lock: None,
        sharp_colour: false,
        sharp_origin: Origin::Default,
        gpu_decode: true,
        gpu_origin: Origin::Default,
        send_win_l: false,
        send_win_l_origin: Origin::Default,
        send_win_l_lock: None,
    }
}

fn settings_last(connection: &Connection, live: &Running) -> String {
    let entries = tab_menu(&Status::Streaming, connection, Some(live), None);
    let rows = entries
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Submenu {
                label: "Connection settings",
                rows,
            } => Some(rows),
            _ => None,
        })
        .expect("Connection settings");
    match rows.last().expect("a last row") {
        Entry::Item { label, .. } => (*label).to_owned(),
        Entry::Note(text) => text.clone(),
        other => panic!("unexpected last row: {other:?}"),
    }
}

fn snapshot(running: Running, gate: &ResizeGate) -> (Running, Scale, ResizeReq) {
    (running, gate.scale(), gate.requested())
}

#[test]
fn a_shown_own_screen_scale_pick_sends_a_resize_and_does_not_restart() {
    let started = own(Scale::Auto);
    let mut running = started.running();
    let dpi = 192;
    let client = (2881, 1801);
    let initial = request_for_scale((1920, 1200), dpi, Scale::Auto);
    let mut gate = ResizeGate::new(initial, running.effective_scale());
    let t0 = Instant::now();
    assert_eq!(
        gate.on_size((2000, 1000), dpi, Moving::Free, t0),
        Some(ResizeGate::SETTLE)
    );

    let req = apply_live_scale(
        true,
        &mut running,
        &mut gate,
        true,
        client,
        dpi,
        Scale::Percent160,
    )
    .expect("a shown Own screen scale pick sends a resize");
    assert_eq!(
        req,
        request_for_scale(client, dpi, Scale::Percent160),
        "the resize is the window size at the new scale"
    );
    assert_eq!(gate.requested(), req);
    assert_eq!(gate.scale(), Scale::Percent160);
    assert_eq!(
        gate.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Nothing,
        "the scale pick supersedes a pending settle"
    );
    let mut expected = started.running();
    expected.scale = Scale::Percent160;
    assert_eq!(running, expected);

    let saved = own(Scale::Percent160);
    assert!(
        !saved.pending(&running),
        "a Scale-only change is not pending once the session runs it"
    );
    assert_eq!(
        settings_last(&saved, &running),
        "Applies the next time this tab connects"
    );

    let mut tabs = Tabs::new();
    let _ = tabs.open(&"u@one".parse().expect("a host"));
    let id = tabs.iter().next().expect("a tab").id();
    let old = tabs.tab(id).expect("tab").session().expect("a session");
    tabs.connected(old, Instant::now());
    let effects = run(Command::Scale(Scale::Percent160), id, &mut tabs);
    assert!(
        effects.is_empty(),
        "a Scale pick must not stop or start a session: {effects:?}"
    );
    assert!(
        effects
            .iter()
            .all(|effect| !matches!(effect, Effect::Stop(_) | Effect::Start { .. }))
    );
    assert_eq!(tabs.tab(id).expect("tab").session(), Some(old));
    assert_eq!(tabs.tab(id).expect("tab").status(), &Status::Streaming);
}

#[test]
fn show_as_sharp_colour_and_gpu_decode_still_offer_reconnect_after_a_live_scale() {
    let mut running = own(Scale::Auto).running();
    let dpi = 192;
    let client = (1920, 1200);
    let mut gate = ResizeGate::new(
        request_for_scale(client, dpi, Scale::Auto),
        running.effective_scale(),
    );
    let req = apply_live_scale(
        true,
        &mut running,
        &mut gate,
        true,
        client,
        dpi,
        Scale::Percent125,
    );
    assert!(req.is_some());
    let saved = own(Scale::Percent125);
    assert!(!saved.pending(&running));

    let show_as = Connection {
        screen: Screen::Mirror,
        ..saved.clone()
    };
    let sharp = Connection {
        sharp_colour: true,
        ..saved.clone()
    };
    let gpu = Connection {
        gpu_decode: false,
        ..saved
    };
    for (label, changed) in [
        ("show as", show_as),
        ("sharp colour", sharp),
        ("gpu decode", gpu),
    ] {
        assert!(changed.pending(&running), "{label}");
        assert_eq!(
            settings_last(&changed, &running),
            "Reconnect now to apply",
            "{label}"
        );
    }
}

#[test]
fn a_hidden_own_screen_session_sends_nothing_until_its_next_resize() {
    let mut running = own(Scale::Auto).running();
    let client = (1920, 1200);
    let dpi = 192;
    let initial = request_for_scale(client, dpi, Scale::Auto);
    let mut gate = ResizeGate::new(initial, Scale::Auto);
    assert_eq!(
        apply_live_scale(
            true,
            &mut running,
            &mut gate,
            false,
            client,
            dpi,
            Scale::Percent125,
        ),
        None
    );
    assert_eq!(running.scale, Scale::Percent125);
    assert_eq!(gate.scale(), Scale::Percent125);
    assert_eq!(
        gate.requested(),
        initial,
        "nothing is sent while the tab is hidden"
    );
    assert!(!own(Scale::Percent125).pending(&running));
    let t0 = Instant::now();
    assert_eq!(
        gate.on_size(client, dpi, Moving::Free, t0),
        Some(ResizeGate::SETTLE)
    );
    assert_eq!(
        gate.on_settle(t0 + ResizeGate::SETTLE),
        Settled::Send(request_for_scale(client, dpi, Scale::Percent125))
    );
}

#[test]
fn a_mirror_session_and_a_session_that_is_not_the_tabs_are_unchanged() {
    let mut mirror = Running {
        screen: Screen::Mirror,
        scale: Scale::Auto,
        share_clipboard: true,
        sharp_colour: false,
        gpu_decode: true,
    };
    let initial = request_for_scale((1920, 1200), 192, Scale::Auto);
    let mut mirror_gate = ResizeGate::new(initial, Scale::Auto);
    let before = snapshot(mirror, &mirror_gate);
    assert_eq!(
        apply_live_scale(
            true,
            &mut mirror,
            &mut mirror_gate,
            true,
            (1920, 1200),
            192,
            Scale::Percent200,
        ),
        None
    );
    assert_eq!(snapshot(mirror, &mirror_gate), before);

    let mut other = own(Scale::Auto).running();
    let mut other_gate = ResizeGate::new(initial, Scale::Auto);
    let other_before = snapshot(other, &other_gate);
    assert_eq!(
        apply_live_scale(
            false,
            &mut other,
            &mut other_gate,
            true,
            (2880, 1800),
            192,
            Scale::Percent125,
        ),
        None,
        "a session that is not this tab's sends nothing"
    );
    assert_eq!(snapshot(other, &other_gate), other_before);
}

#[test]
fn picking_the_scale_the_session_already_runs_sends_nothing() {
    let mut running = own(Scale::Percent160).running();
    let client = (2560, 1600);
    let dpi = 144;
    let initial = request_for_scale(client, dpi, Scale::Percent160);
    let mut gate = ResizeGate::new(initial, Scale::Percent160);
    let before = snapshot(running, &gate);
    assert_eq!(
        apply_live_scale(
            true,
            &mut running,
            &mut gate,
            true,
            client,
            dpi,
            Scale::Percent160,
        ),
        None
    );
    assert_eq!(snapshot(running, &gate), before);
}

#[test]
fn a_two_hundred_percent_pick_at_a_matching_windows_scale_sends_nothing() {
    let mut running = own(Scale::Auto).running();
    let client = (2560, 1600);
    let dpi = 192;
    let initial = request_for_scale(client, dpi, Scale::Auto);
    assert_eq!(initial.scale, Scale::Percent200.factor(dpi));
    let mut gate = ResizeGate::new(initial, Scale::Auto);
    assert_eq!(
        apply_live_scale(
            true,
            &mut running,
            &mut gate,
            true,
            client,
            dpi,
            Scale::Percent200,
        ),
        None,
        "the wire scale already matches, so no Resize is sent"
    );
    assert_eq!(running.scale, Scale::Percent200);
    assert_eq!(gate.scale(), Scale::Percent200);
    assert_eq!(gate.requested(), initial);
    assert!(!own(Scale::Percent200).pending(&running));
    assert_eq!(
        settings_last(&own(Scale::Percent200), &running),
        "Applies the next time this tab connects"
    );
}
