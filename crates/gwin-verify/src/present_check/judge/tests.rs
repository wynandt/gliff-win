use gliff_windows::present_log::Stage;
use gliff_windows::stats_out::ClientStats;
use gliff_windows::stats_out::SwapChainInfo;
use serde_json::Value;

use super::Capture;
use super::ClientExit;
use super::Expect;
use super::Observed;
use super::judge;
use crate::pattern::Score;
use crate::present_check::fixtures::PANEL;
use crate::present_check::fixtures::good_score;
use crate::present_check::fixtures::good_stats;
use crate::present_check::fixtures::stage;

fn good() -> Observed {
    let last = good_stats();
    let mut fullscreen = last.clone();
    fullscreen.complete = false;
    fullscreen.end = String::new();
    fullscreen.ssh_exit = None;
    Observed {
        fullscreen,
        last: Some(last),
        window_rect: PANEL,
        capture: Capture::Scored(good_score()),
        exit: ClientExit::Exited(Some(0)),
    }
}

fn reasons(observed: &Observed) -> Vec<String> {
    let line = judge(observed, &Expect::panel(20));
    let reasons: Vec<String> = line.metrics["reasons"]
        .as_array()
        .expect("reasons is an array")
        .iter()
        .map(|v| v.as_str().expect("reasons are strings").to_owned())
        .collect();
    assert_eq!(
        line.ok,
        reasons.is_empty(),
        "ok must mean no reasons: {}",
        line.to_json()
    );
    reasons
}

fn warnings(observed: &Observed) -> Value {
    judge(observed, &Expect::panel(20)).metrics["warnings"].clone()
}

#[test]
fn a_good_run_passes_and_reports_every_section() {
    let line = judge(&good(), &Expect::panel(20));
    assert!(line.ok, "a good run must pass: {}", line.to_json());
    for key in [
        "windowed",
        "fullscreen",
        "resize",
        "fullscreen_geometry",
        "swap_chain",
        "adapter",
        "session",
        "pattern",
        "exit",
    ] {
        assert!(
            line.metrics.get(key).is_some_and(|v| !v.is_null()),
            "missing {key}: {}",
            line.to_json()
        );
    }
    assert_eq!(line.metrics["adapter"]["vendor_id"], "0x1234", "hex vendor");
    assert_eq!(
        line.metrics["windowed"]["decoded_fps"], 59.8,
        "longest windowed segment"
    );
}

#[test]
fn the_synthetic_fixture_file_passes() {
    let stats = ClientStats::from_json(include_str!("../fixtures/synthetic-good-stats.json"))
        .expect("the fixture parses as ClientStats");
    let observed = Observed {
        last: Some(stats),
        ..good()
    };
    assert_eq!(reasons(&observed), Vec::<String>::new(), "fixture run");
}

#[test]
fn the_presenter_losing_more_than_five_percent_fails() {
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.present.stages[3] = stage(Stage::Windowed, 2100, 56.0, 59.8);
    assert_eq!(reasons(&observed), ["present_fps_windowed"], "56 of 59.8");
}

#[test]
fn decoded_below_the_cap_only_warns() {
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.present.stages[5] = stage(Stage::Fullscreen, 27_900, 29.0, 29.5);
    assert_eq!(
        reasons(&observed),
        Vec::<String>::new(),
        "29.5 of 60 passes"
    );
    assert_eq!(
        warnings(&observed),
        serde_json::json!(["decoded_below_cap_fullscreen"]),
        "but warns"
    );
}

#[test]
fn latency_stalls_and_present_errors_are_named_per_stage() {
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.present.stages[3].latency_p95_ms = 17.0;
    last.present.stages[5].max_gap_ms = 251;
    last.present.stages[5].occluded = 1;
    assert_eq!(
        reasons(&observed),
        [
            "latency_windowed",
            "stall_fullscreen",
            "present_errors_fullscreen"
        ],
        "each gate by name"
    );
}

#[test]
fn a_missing_or_short_stage_fails() {
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.present.stages.truncate(5);
    last.present.stages[3].measured_ms = 15_000;
    assert_eq!(
        reasons(&observed),
        ["short_windowed", "no_fullscreen_stage"],
        "15 s of 20, and no fullscreen"
    );
}

#[test]
fn acks_and_decode_errors_are_exact() {
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.session.acks -= 1;
    last.session.decode_errors = 1;
    assert_eq!(
        reasons(&observed),
        ["acks", "decode_errors"],
        "exact counters"
    );
}

#[test]
fn a_tearing_swap_chain_fails() {
    for change in [
        |s: &mut SwapChainInfo| s.allow_tearing = true,
        |s: &mut SwapChainInfo| s.sync_interval = 0,
        |s: &mut SwapChainInfo| s.swap_effect = "discard".to_owned(),
    ] {
        let mut observed = good();
        change(&mut observed.fullscreen.swap_chain);
        assert_eq!(
            reasons(&observed),
            ["tearing"],
            "{:?}",
            observed.fullscreen.swap_chain
        );
    }
}

#[test]
fn fullscreen_needs_client_swap_chain_monitor_and_rect_to_agree() {
    let mut off_rect = good();
    off_rect.window_rect = (0, 0, 3840, 2360);
    assert_eq!(reasons(&off_rect), ["fullscreen"], "a taskbar-sized gap");
    let mut low_dpi = good();
    low_dpi.fullscreen.monitor.dpi = 96;
    assert_eq!(reasons(&low_dpi), ["fullscreen"], "96 DPI");
    let mut small_chain = good();
    small_chain.fullscreen.swap_chain.size = (2560, 1600);
    assert_eq!(reasons(&small_chain), ["fullscreen"], "stale swap chain");
    let mut windowed = good();
    windowed.fullscreen.window.fullscreen = false;
    assert_eq!(reasons(&windowed), ["fullscreen"], "not fullscreen");
}

#[test]
fn the_windowed_resize_must_land_within_three_seconds() {
    let mut slow = good();
    let last = slow.last.as_mut().expect("good has a last file");
    last.resizes[0].first_picture_ms = Some(3001);
    last.resizes[1].first_picture_ms = Some(4401);
    // The first matching record is judged.
    assert_eq!(reasons(&slow), ["resize"], "3001 ms to the picture");
    let mut never = good();
    let last = never.last.as_mut().expect("good has a last file");
    last.resizes[0].applied_ms = None;
    assert_eq!(reasons(&never), ["resize"], "never applied");
    let mut wrong_scale = good();
    let last = wrong_scale.last.as_mut().expect("good has a last file");
    for r in &mut last.resizes {
        r.scale = 1.0;
    }
    assert_eq!(reasons(&wrong_scale), ["resize"], "scale 1.0");
}

#[test]
fn the_screenshot_must_match_the_pattern() {
    let mut mismatch = good();
    mismatch.capture = Capture::Scored(Score {
        cells_matched: 30,
        ..good_score()
    });
    assert_eq!(reasons(&mismatch), ["pattern"], "30 of 32 cells");
    let mut failed = good();
    failed.capture = Capture::Failed("BitBlt failed".to_owned());
    assert_eq!(reasons(&failed), ["pattern"], "no capture");
}

#[test]
fn only_a_clean_bye_exit_passes() {
    let mut killed = good();
    killed.exit = ClientExit::Killed;
    assert_eq!(reasons(&killed), ["exit_after_bye"], "killed");
    let mut code = good();
    code.exit = ClientExit::Exited(Some(1));
    assert_eq!(reasons(&code), ["exit_after_bye"], "exit 1");
    let mut end = good();
    end.last.as_mut().expect("good has a last file").end = "server_error".to_owned();
    assert_eq!(reasons(&end), ["exit_after_bye"], "not bye");
    let mut ssh = good();
    ssh.last.as_mut().expect("good has a last file").ssh_exit = Some(255);
    assert_eq!(reasons(&ssh), ["exit_after_bye"], "ssh 255");
    let mut no_file = good();
    no_file.last = None;
    assert_eq!(
        reasons(&no_file),
        ["exit_after_bye"],
        "no final file (the heartbeat judges the stages)"
    );
}

#[test]
#[ignore = "regenerates fixtures/synthetic-good-stats.json"]
fn write_fixture() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/present_check/fixtures/synthetic-good-stats.json"
    );
    std::fs::write(path, good_stats().to_json() + "\n").expect("write the fixture");
}

#[test]
fn a_p95_on_the_budget_passes_after_widening_to_f64() {
    // Regression: 16.7f32 widens to 16.700000762.
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.present.stages[3].latency_p95_ms = 16.7;
    last.present.stages[5].latency_p95_ms = 16.7;
    assert_eq!(
        reasons(&observed),
        Vec::<String>::new(),
        "16.7 ms is in budget"
    );
    let last = observed.last.as_mut().expect("good has a last file");
    last.present.stages[5].latency_p95_ms = 16.8;
    assert_eq!(reasons(&observed), ["latency_fullscreen"], "16.8 ms is not");
}

#[test]
fn a_closing_stage_after_fullscreen_is_not_judged() {
    let mut observed = good();
    let last = observed.last.as_mut().expect("good has a last file");
    last.present
        .stages
        .push(stage(Stage::Closing, 60_000, 0.0, 0.0));
    assert_eq!(
        reasons(&observed),
        Vec::<String>::new(),
        "teardown is outside the gate"
    );
}
