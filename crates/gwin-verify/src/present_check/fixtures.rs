//! Shared test data: a run that passes every gate.

use gliff_windows::present_log::PresentSummary;
use gliff_windows::present_log::Stage;
use gliff_windows::present_log::StageSummary;
use gliff_windows::stats_out::AdapterInfo;
use gliff_windows::stats_out::ClientStats;
use gliff_windows::stats_out::MonitorInfo;
use gliff_windows::stats_out::ResizeRecord;
use gliff_windows::stats_out::SessionCounters;
use gliff_windows::stats_out::SwapChainInfo;
use gliff_windows::stats_out::WindowInfo;

use crate::pattern::Score;

pub const PANEL: (i32, i32, u32, u32) = (0, 0, 3840, 2400);

pub fn stage(stage: Stage, start_ms: u64, presented_fps: f64, decoded_fps: f64) -> StageSummary {
    StageSummary {
        stage,
        start_ms,
        end_ms: start_ms + 25_000,
        measured_ms: 20_000,
        presented_total: 1500,
        presented: (presented_fps * 20.0) as u64,
        decoded: (decoded_fps * 20.0) as u64,
        slot_dropped: ((decoded_fps - presented_fps) * 20.0) as u64,
        presented_fps,
        decoded_fps,
        latency_p50_ms: 4.0,
        latency_p95_ms: 9.5,
        latency_max_ms: 15.0,
        max_gap_ms: 40,
        occluded: 0,
        device_lost: 0,
    }
}

/// A run that passes every gate: the panel at 200%, 60 fps cap.
pub fn good_stats() -> ClientStats {
    ClientStats {
        version: "0.1.0".to_owned(),
        complete: true,
        session: SessionCounters {
            frames: 3000,
            acks: 3000,
            to_slot: 3000,
            slot_dropped: 12,
            configs: 3,
            resizes_sent: 3,
            decode_p50_ms: 9.0,
            decode_p95_ms: 14.0,
            decode_max_ms: 20.0,
            ..SessionCounters::default()
        },
        present: PresentSummary {
            shown: 2988,
            occluded: 0,
            device_lost: 0,
            stages: vec![
                StageSummary {
                    measured_ms: 0,
                    ..stage(Stage::Starting, 0, 0.0, 0.0)
                },
                StageSummary {
                    measured_ms: 0,
                    ..stage(Stage::Windowed, 1200, 0.0, 0.0)
                },
                StageSummary {
                    measured_ms: 0,
                    ..stage(Stage::Resizing, 1500, 0.0, 0.0)
                },
                stage(Stage::Windowed, 2100, 59.5, 59.8),
                StageSummary {
                    measured_ms: 0,
                    ..stage(Stage::Resizing, 27_200, 0.0, 0.0)
                },
                stage(Stage::Fullscreen, 27_900, 58.9, 59.6),
            ],
        },
        adapter: AdapterInfo {
            name: "Test GPU".to_owned(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            choice: "monitor".to_owned(),
            drives_monitor: true,
        },
        monitor: MonitorInfo {
            rect: PANEL,
            dpi: 192,
            refresh_hz: 240.0,
        },
        window: WindowInfo {
            client: (3840, 2400),
            rect: PANEL,
            fullscreen: true,
            dpi: 192,
        },
        swap_chain: SwapChainInfo {
            size: (3840, 2400),
            swap_effect: "flip_discard".to_owned(),
            buffers: 2,
            allow_tearing: false,
            sync_interval: 1,
            max_frame_latency: 1,
        },
        resizes: vec![
            ResizeRecord {
                width: 2560,
                height: 1600,
                scale: 2.0,
                sent_ms: 0,
                applied_ms: Some(900),
                first_picture_ms: Some(1150),
            },
            ResizeRecord {
                width: 2560,
                height: 1600,
                scale: 2.0,
                sent_ms: 1400,
                applied_ms: Some(1900),
                first_picture_ms: Some(2100),
            },
            ResizeRecord {
                width: 3840,
                height: 2400,
                scale: 2.0,
                sent_ms: 27_400,
                applied_ms: Some(27_700),
                first_picture_ms: Some(27_900),
            },
        ],
        fps_cap: 60,
        end: "bye".to_owned(),
        ssh_exit: Some(0),
        input: gliff_windows::stats_out::InputInfo::default(),
        sessions: Vec::new(),
        chords: gliff_windows::stats_out::ChordsInfo::default(),
    }
}

pub fn good_score() -> Score {
    Score {
        mae_mean: 3.2,
        mae_max: 7.9,
        worst_cell: 5,
        cells_matched: 32,
        tolerance: 40.0,
    }
}
