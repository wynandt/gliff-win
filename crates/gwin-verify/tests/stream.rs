use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::RwLock;

use serde_json::Value;

fn fake_ssh() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh")
}

static STREAMS: RwLock<()> = RwLock::new(());

const FPS_GATES: [&str; 4] = ["--min-fps", "30", "--fps-of-cap", "0.95"];

fn with_defaults<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let fps_gated = extra.contains(&"--min-fps");
    let (settle, window) = if fps_gated { ("5", "3") } else { ("1", "1") };
    let mut args: Vec<&str> = Vec::new();
    for (flag, value) in [
        ("--size", "640x400"),
        ("--seconds", window),
        ("--settle-s", settle),
        ("--min-fps", "0"),
        ("--fps-of-cap", "0"),
    ] {
        if !extra.contains(&flag) {
            args.extend([flag, value]);
        }
    }
    args.extend_from_slice(extra);
    args
}

fn stream(name: &str, fake: &str, extra: &[&str]) -> (bool, Value, PathBuf) {
    stream_env(name, fake, &[], extra)
}

fn stream_env(
    name: &str,
    fake: &str,
    env: &[(&str, &str)],
    extra: &[&str],
) -> (bool, Value, PathBuf) {
    let _gated;
    let _ungated;
    if extra.contains(&"--min-fps") {
        _gated = Some(
            STREAMS
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        _ungated = None;
    } else {
        _gated = None;
        _ungated = Some(
            STREAMS
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stream-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let hypr = dir.join("hypr");
    std::fs::create_dir_all(&hypr).expect("hypr dir");
    let png = dir.join("last.png");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gwin-verify"));
    cmd.env("GWIN_FAKE_SERVER", fake)
        .env("GWIN_FAKE_HYPR_DIR", &hypr)
        .envs(env.iter().copied())
        .args(["stream", "--host", "user@host"])
        .args(["--server-path", env!("CARGO_BIN_EXE_gwin-fake-server")])
        .arg("--ssh")
        .arg(fake_ssh())
        .arg("--log-dir")
        .arg(&dir)
        .args(with_defaults(extra));
    if !extra.contains(&"--png") {
        cmd.arg("--png").arg(&png);
    }
    let out = cmd.output().expect("run gwin-verify");
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "one JSON line: {stdout} / stderr: {stderr}");
    let line: Value = serde_json::from_str(lines[0]).expect("JSON line");
    assert_eq!(
        out.status.success(),
        line["ok"] == true,
        "exit code follows ok: {line}"
    );
    (out.status.success(), line, png)
}

fn failures(line: &Value) -> Vec<&str> {
    line["failures"]
        .as_array()
        .expect("failures array")
        .iter()
        .map(|v| v.as_str().expect("failure code"))
        .collect()
}

#[test]
fn a_good_server_passes_with_the_pattern_and_acks_equal_frames() {
    let (success, line, png) = stream("pass", "", &FPS_GATES);
    assert!(success, "{line}");
    assert_eq!(line["resize"], "applied", "{line}");
    assert_eq!(line["stream_config"]["stream"], "640x400", "{line}");
    assert!(line["fps"].as_f64().expect("fps") >= 30.0, "{line}");
    assert!(
        line["fps"].as_f64().expect("fps")
            >= 0.95
                * line["fps_cap_effective"]
                    .as_f64()
                    .expect("fps_cap_effective"),
        "{line}"
    );
    assert_eq!(
        line["decoder_resets_in_window"], 0,
        "both resets (first config, Resize) come before the window: {line}"
    );
    assert_eq!(line["stalls"], 0, "{line}");
    assert_eq!(line["acks"], line["frames"], "{line}");
    assert_eq!(line["acks_written"], line["frames"], "{line}");
    assert_eq!(
        line["sps"]["size"], "640x400",
        "the SPS after Resize: {line}"
    );
    assert_eq!(line["sps"]["decoder"], "accepted", "{line}");
    assert_eq!(line["pattern"]["cells_matched"], 32, "{line}");
    assert_eq!(line["exit_after_bye"], true, "{line}");
    assert!(png.exists(), "PNG written at {}", png.display());
    let placement = line["placement"].as_array().expect("placement array");
    assert_eq!(placement.len(), 2, "start and end probes: {line}");
    for p in placement {
        assert!(p.as_str().expect("probe").starts_with("ok:"), "{line}");
    }
}

#[test]
fn a_server_held_at_a_30_fps_cap_passes_at_30() {
    let (success, line, _) = stream("hold30", "hold30", &FPS_GATES);
    assert!(success, "{line}");
    assert_eq!(line["stream_config"]["fps_cap"], 30, "{line}");
    assert_eq!(line["fps_cap_effective"], 30.0, "{line}");
    let fps = line["fps"].as_f64().expect("fps");
    assert!((28.5..=31.0).contains(&fps), "paced at the cap: {line}");
}

#[test]
fn a_server_that_caps_below_30_fails_by_name() {
    let (success, line, _) = stream("cap15", "cap15", &FPS_GATES);
    assert!(!success, "{line}");
    assert_eq!(failures(&line), ["fps_cap"], "{line}");
    assert!(
        line["errors"][0]
            .as_str()
            .expect("error")
            .contains("capped the stream at 15 fps"),
        "{line}"
    );
}

#[test]
fn a_remote_without_hyprland_says_what_hyprctl_said() {
    let (success, line, _) = stream_env(
        "no-instance",
        "",
        &[("GWIN_FAKE_HYPR_INSTANCE", "none")],
        &[],
    );
    assert!(!success, "{line}");
    assert_eq!(failures(&line), ["placement", "placement"], "{line}");
    for p in line["placement"].as_array().expect("placement array") {
        let p = p.as_str().expect("probe");
        assert!(
            p.contains("HYPRLAND_INSTANCE_SIGNATURE not set"),
            "hyprctl's own stdout is in the message: {line}"
        );
        assert!(
            p.contains("no Hyprland instance socket under"),
            "and the discovery's stderr: {line}"
        );
    }
}

#[test]
fn a_png_that_cannot_be_written_fails_the_run() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("stream-png-blocker");
    std::fs::create_dir_all(&dir).expect("blocker dir");
    let blocker = dir.join("not-a-dir");
    std::fs::write(&blocker, b"a file").expect("blocker file");
    let png = blocker.join("last.png");
    let png = png.to_str().expect("utf-8 path");
    let (success, line, _) = stream("png-unwritable", "", &["--png", png]);
    assert!(!success, "{line}");
    assert_eq!(failures(&line), vec!["png"], "{line}");
}

#[test]
fn frames_the_slot_drops_are_still_acked() {
    let (success, line, _) = stream("slot-drop", "", &["--present-ms", "100"]);
    assert!(success, "{line}");
    assert!(
        line["slot_dropped"].as_u64().expect("slot_dropped") > 0,
        "presenting every 100 ms drops: {line}"
    );
    assert_eq!(line["acks"], line["frames"], "{line}");
    assert_eq!(line["acks_written"], line["frames"], "{line}");
}

#[test]
fn a_server_that_stops_sending_is_a_stall() {
    let (success, line, _) = stream(
        "stall",
        "stall",
        &[
            "--settle-s",
            "1",
            "--seconds",
            "10",
            "--min-fps",
            "30",
            "--fps-of-cap",
            "0.95",
        ],
    );
    assert!(!success, "{line}");
    assert_eq!(failures(&line), ["fps", "stall"], "{line}");
    assert_eq!(line["stalls"], 1, "{line}");
    assert!(
        line["max_gap_ms"].as_f64().expect("max_gap_ms") > 5000.0,
        "{line}"
    );
}

#[test]
fn dropped_frames_are_missing_ids_and_break_decode() {
    let (success, line, _) = stream("drop", "drop", &[]);
    assert!(!success, "{line}");
    let codes = failures(&line);
    assert!(codes.contains(&"frames_missing"), "{line}");
    assert!(
        codes.contains(&"decode_errors"),
        "broken references: {line}"
    );
    assert!(
        codes
            .iter()
            .all(|c| ["frames_missing", "decode_errors"].contains(c)),
        "{line}"
    );
    assert!(
        line["keyframe_requests"]
            .as_u64()
            .expect("keyframe_requests")
            > 0,
        "the client asks for a keyframe: {line}"
    );
}

#[test]
fn a_server_that_ignores_resize_fails_with_the_size_it_kept() {
    let (success, line, _) = stream(
        "ignore-resize",
        "ignore-resize",
        &["--resize-timeout-s", "2"],
    );
    assert!(!success, "{line}");
    assert_eq!(failures(&line), ["resize_ignored"], "{line}");
    assert_eq!(
        line["stream_config"]["implied_output"], "1920x1080",
        "{line}"
    );
}

#[test]
fn a_level_the_decoder_rejects_fails_by_name() {
    let (success, line, _) = stream("level60", "level60", &[]);
    assert!(!success, "{line}");
    let codes = failures(&line);
    assert!(codes.contains(&"level_rejected"), "{line}");
    assert!(codes.contains(&"decode_errors"), "{line}");
    assert_eq!(line["sps"]["level"], "6.0", "{line}");
    assert_eq!(line["sps"]["decoder"], "rejected", "{line}");
    assert_eq!(
        line["acks"], line["frames"],
        "failed decodes are acked: {line}"
    );
}

#[test]
fn a_picture_that_is_not_the_pattern_fails_and_keeps_the_png() {
    let (success, line, png) = stream("no-pattern", "no-pattern", &[]);
    assert!(!success, "{line}");
    assert_eq!(failures(&line), ["pattern"], "{line}");
    assert!(png.exists(), "PNG kept for a look: {}", png.display());
}

#[test]
fn a_pattern_on_the_wrong_monitor_is_a_placement_failure() {
    let (success, line, _) = stream("misplaced", "misplaced", &[]);
    assert!(!success, "{line}");
    assert_eq!(
        failures(&line),
        ["placement", "placement"],
        "start and end: {line}"
    );
}

#[test]
fn the_cpu_encoder_clamp_is_named_not_passed() {
    let (success, line, _) = stream(
        "clamped",
        "",
        &["--size", "3840x2400", "--settle-s", "1", "--seconds", "2"],
    );
    assert!(!success, "{line}");
    assert_eq!(failures(&line), ["encoder_clamped"], "{line}");
    assert_eq!(line["stream_config"]["stream"], "3456x2160", "{line}");
    assert_eq!(line["stream_config"]["scale_milli"], 900, "{line}");
}
