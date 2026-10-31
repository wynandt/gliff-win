//! The file format the gate reads.

use std::collections::BTreeMap;
use std::time::Instant;

use super::AdapterInfo;
use super::ChordsInfo;
use super::ClientStats;
use super::GeometryInfo;
use super::InputInfo;
use super::MonitorInfo;
use super::ResizeRecord;
use super::SessionCounters;
use super::SessionEntry;
use super::SwapChainInfo;
use super::WindowInfo;
use crate::present_log::PresentLog;
use crate::session::Histogram;
use crate::session::ResizeReq;
use crate::session::SessionStats;

fn sample() -> ClientStats {
    let mut stats = SessionStats {
        frames: 10,
        acks: 10,
        presented: 9,
        ..SessionStats::default()
    };
    let mut h = Histogram::default();
    h.record(3.0);
    stats.decode_ms = h;
    ClientStats {
        version: "0.1.0".into(),
        complete: true,
        session: SessionCounters::new(&stats, 2),
        present: PresentLog::new(Instant::now()).summary(Instant::now()),
        adapter: AdapterInfo {
            name: "Intel(R) Graphics".into(),
            vendor_id: 0x8086,
            device_id: 1,
            choice: "monitor".into(),
            drives_monitor: true,
        },
        monitor: MonitorInfo {
            rect: (0, 0, 3840, 2400),
            dpi: 192,
            refresh_hz: 240.0,
        },
        window: WindowInfo {
            client: (3840, 2400),
            rect: (0, 0, 3840, 2400),
            fullscreen: true,
            dpi: 192,
        },
        swap_chain: SwapChainInfo {
            size: (3840, 2400),
            swap_effect: "flip_discard".into(),
            buffers: 2,
            allow_tearing: false,
            sync_interval: 1,
            max_frame_latency: 1,
        },
        resizes: vec![ResizeRecord::sent(
            ResizeReq {
                width: 2560,
                height: 1600,
                scale: 2.0,
            },
            1200,
        )],
        end: "bye".into(),
        fps_cap: 60,
        ssh_exit: Some(0),
        input: InputInfo {
            hook: "grab".into(),
            grab: true,
            hook_reinstalls: 1,
            legacy_keys: 2,
            grab_timeouts: 1,
            modifiers_released: 3,
            modifiers_released_on_exit: 2,
            lock_setting_held: false,
            geometry: Some(GeometryInfo {
                stream: (3840, 2400),
                view: (3840, 2400),
                scale_milli: 2000,
            }),
            keymap_sent: vec!["keymap_sent us reason=start".into()],
            wheel_steps_sent: BTreeMap::new(),
            button_presses_sent: BTreeMap::new(),
        },
        sessions: vec![SessionEntry {
            id: 1,
            target: "user@host".into(),
            shown: true,
            status: "streaming".into(),
            retries: 0,
            frames: 10,
            acks: 10,
            keys_down: 0,
            buttons_down: 0,
        }],
        chords: ChordsInfo::default(),
    }
}

#[test]
fn input_info_names_the_fields_the_gate_reads() {
    let json = sample().to_json();
    for key in [
        "\"hook\": \"grab\"",
        "\"grab\": true",
        "\"hook_reinstalls\": 1",
        "\"grab_timeouts\": 1",
        "\"modifiers_released\": 3",
        "\"modifiers_released_on_exit\": 2",
        "\"scale_milli\": 2000",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
}

#[test]
fn counters_carry_the_session_and_the_slot() {
    let s = sample().session;
    assert_eq!(
        (s.frames, s.acks, s.to_slot, s.slot_dropped),
        (10, 10, 9, 2)
    );
    assert_eq!((s.decode_p95_ms, s.decode_max_ms), (3.0, 3.0));
}

#[test]
fn the_file_round_trips_and_names_its_fields() {
    let stats = sample();
    let json = stats.to_json();
    for key in [
        "\"allow_tearing\": false",
        "\"end\": \"bye\"",
        "\"ssh_exit\": 0",
        "\"fps_cap\": 60",
        "\"applied_ms\": null",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }
    assert_eq!(ClientStats::from_json(&json).expect("parse"), stats);
}

#[test]
fn write_replaces_the_file_whole() {
    let dir = std::env::temp_dir().join(format!("gwin-stats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("present-stats.json");
    let mut stats = sample();
    stats.write(&path).expect("first write");
    stats.complete = false;
    stats.write(&path).expect("second write");
    let back =
        ClientStats::from_json(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert!(!back.complete, "the second write won");
    assert!(!dir.join("present-stats.json.tmp").exists(), "no temp left");
    std::fs::remove_dir_all(&dir).expect("clean up");
}

fn req(width: u32, height: u32, scale: f32) -> super::ResizeReq {
    super::ResizeReq {
        width,
        height,
        scale,
    }
}

#[test]
fn a_resize_is_applied_by_its_config_then_shown() {
    let mut log = super::ResizeLog::default();
    log.sent(req(1280, 800, 1.0), 10);
    log.sent(req(2560, 1600, 2.0), 1000);
    // The first config is for the first Resize; a picture before the
    // second config is at the old view.
    log.on_config((1280, 800), 1000, 40);
    log.on_picture((1280, 800), 60);
    log.on_picture((1280, 800), 70);
    log.on_picture((1280, 800), 1100);
    // A config at the right view but another scale applies nothing.
    log.on_config((2560, 1600), 1000, 1150);
    log.on_config((2560, 1600), 2000, 1200);
    log.on_picture((2560, 1600), 1230);
    let r = log.records();
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(
        (r[0].applied_ms, r[0].first_picture_ms),
        (Some(40), Some(60)),
        "first: {r:?}"
    );
    assert_eq!(
        (r[1].applied_ms, r[1].first_picture_ms),
        (Some(1200), Some(1230)),
        "second waits for its own scale: {r:?}"
    );
}

#[test]
fn a_repeated_size_gets_its_own_record() {
    let mut log = super::ResizeLog::default();
    log.sent(req(1280, 800, 1.0), 0);
    log.on_config((1280, 800), 1000, 5);
    log.on_picture((1280, 800), 6);
    log.sent(req(2560, 1600, 1.0), 10);
    log.sent(req(1280, 800, 1.0), 20);
    log.on_config((1280, 800), 1000, 30);
    log.on_picture((1280, 800), 31);
    let r = log.records();
    assert_eq!(r[1].applied_ms, None, "2560x1600 never applied: {r:?}");
    assert_eq!(
        (r[2].applied_ms, r[2].first_picture_ms),
        (Some(30), Some(31)),
        "the second 1280x800 is applied by the later config: {r:?}"
    );
    assert_eq!(
        r[0].first_picture_ms,
        Some(6),
        "the first keeps its picture"
    );
}

#[test]
fn end_names_are_snake_case() {
    use super::EndReason;
    assert_eq!(super::end_name(&EndReason::Bye), "bye");
    assert_eq!(
        super::end_name(&EndReason::Incompatible(String::new())),
        "incompatible"
    );
    assert_eq!(
        super::end_name(&EndReason::ServerError {
            code: 300,
            message: String::new()
        }),
        "server_error"
    );
    assert_eq!(super::end_name(&EndReason::Link(String::new())), "link");
    assert_eq!(
        super::end_name(&EndReason::Ssh(crate::connect::SshFailure::Refused)),
        "ssh"
    );
}

#[test]
fn the_input_counters_round_trip_and_an_old_file_reads_with_none() {
    let mut info = InputInfo::default();
    info.wheel_steps_sent.insert("vertical+".to_owned(), 3);
    info.button_presses_sent.insert(278, 2);
    let json = serde_json::to_string(&info).expect("serialise");
    let back: InputInfo = serde_json::from_str(&json).expect("parse");
    assert_eq!(back, info, "round trip");
    assert!(json.contains("\"278\":2"), "codes are the keys: {json}");
    let old = r#"{"hook":"grab","grab":true,"hook_reinstalls":0,"legacy_keys":0,
        "lock_setting_held":false,"geometry":null,"keymap_sent":[]}"#;
    let old: InputInfo = serde_json::from_str(old).expect("old file");
    assert!(old.wheel_steps_sent.is_empty() && old.button_presses_sent.is_empty());
}

mod entries {
    use super::super::ChordsInfo;
    use super::super::session_entries;
    use crate::config::TabSet;
    use crate::history::HostTarget;
    use crate::session::SessionStats;
    use crate::session_set::Report;
    use crate::settings::Settings;
    use crate::tabs::End;
    use crate::tabs::Tabs;

    fn host(text: &str) -> HostTarget {
        text.parse().expect("a target")
    }

    fn report(tabs: &Tabs, name: &str, frames: u64, keys_down: u32) -> Report {
        let tab = tabs
            .iter()
            .find(|t| t.target().as_str() == name)
            .expect("tab");
        Report {
            session: tab.session().expect("a session"),
            target: host(name),
            stats: SessionStats {
                frames,
                acks: frames,
                keys_down,
                ..SessionStats::default()
            },
        }
    }

    #[test]
    fn every_live_session_has_a_line_and_only_one_is_shown() {
        let (mut tabs, _) = Tabs::start_up(&TabSet::default(), &[host("a@one"), host("b@two")]);
        let ids: Vec<_> = tabs.iter().filter_map(|t| t.session()).collect();
        tabs.connected(ids[1], std::time::Instant::now());
        let reports = [report(&tabs, "a@one", 5, 0), report(&tabs, "b@two", 7, 0)];
        let lines = session_entries(&reports, &tabs);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines.iter().map(|l| l.shown).collect::<Vec<_>>(),
            [true, false],
            "the first target is shown"
        );
        assert_eq!(
            lines.iter().map(|l| l.status.as_str()).collect::<Vec<_>>(),
            ["connecting", "streaming"]
        );
        assert_eq!(lines[1].target, "b@two");
        assert_eq!((lines[1].frames, lines[1].acks), (7, 7));
    }

    #[test]
    fn a_hidden_session_still_reports_its_held_keys() {
        let (tabs, _) = Tabs::start_up(&TabSet::default(), &[host("a@one"), host("b@two")]);
        let reports = [report(&tabs, "a@one", 1, 2), report(&tabs, "b@two", 1, 0)];
        let lines = session_entries(&reports, &tabs);
        assert_eq!(lines[0].keys_down, 2);
        assert_eq!(lines[1].keys_down, 0);
    }

    #[test]
    fn a_report_for_a_session_the_model_forgot_is_left_out() {
        let (mut tabs, _) = Tabs::start_up(&TabSet::default(), &[host("a@one")]);
        let reports = [report(&tabs, "a@one", 1, 0)];
        let id = reports[0].session;
        tabs.ended(id, End::Error("gone".into()), std::time::Instant::now());
        assert!(
            session_entries(&reports, &tabs).is_empty(),
            "the tab has no session now"
        );
        let (other, _) = Tabs::start_up(&TabSet::default(), &[]);
        assert!(session_entries(&reports, &other).is_empty());
    }

    #[test]
    fn the_chords_are_spelled_as_the_settings_spell_them() {
        let chords = ChordsInfo::of(&Settings::default());
        assert_eq!(chords.release, "ctrl+alt+home");
        assert_eq!(chords.switch_next, "ctrl+alt+pagedown");
        assert_eq!(chords.switch_jump, "ctrl+alt");
        assert_eq!(
            chords.local.len(),
            Settings::default().local_chords.0.len(),
            "one text per local chord"
        );
    }
}
