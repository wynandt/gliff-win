//! One interval per connection attempt, shared by the readout and JSON.

use std::time::Instant;

use crate::decode::DecoderKind;
use crate::session::SessionStats;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StreamSample {
    pub sequence: u64,
    pub elapsed_secs: f64,
    pub received_fps: f64,
    #[serde(default)]
    pub fps: f64,
    pub video_mbit: f64,
    pub decode_ms: f64,
    #[serde(default)]
    pub decode_call_ms: f64,
    pub video: String,
    pub stream: (u32, u32),
    pub view: (u32, u32),
    pub fps_cap: u32,
}

impl StreamSample {
    pub fn text(&self) -> String {
        let stream = if self.stream == self.view {
            format!("{}x{}", self.stream.0, self.stream.1)
        } else {
            format!(
                "{}x{} → {}x{}",
                self.stream.0, self.stream.1, self.view.0, self.view.1
            )
        };
        format!(
            "{}  {:.0} fps  {:.1} Mbit/s  decode {:.1} ms  {stream} @{}",
            self.video, self.fps, self.video_mbit, self.decode_ms, self.fps_cap
        )
    }
}

pub fn video_label(stats: &SessionStats) -> String {
    let server = stats
        .stream_info
        .and_then(|info| info.pipeline)
        .map_or("?", |p| match p {
            gliff_proto::msg::VideoPipeline::Cpu => "CPU",
            gliff_proto::msg::VideoPipeline::Gpu => "GPU",
        });
    let decoder = match stats.decoder {
        Some(DecoderKind::Cpu) => "CPU",
        Some(DecoderKind::Gpu) => "GPU",
        None => "?",
    };
    format!("{server}→{decoder}")
}

#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    frames: u64,
    bytes: u64,
    calls: u64,
    immediate: u64,
    immediate_ms: f64,
    ms: f64,
}

impl Counts {
    fn of(stats: &SessionStats) -> Self {
        Self {
            frames: stats.frames,
            bytes: stats.video_bytes,
            calls: stats.decode_ms.count(),
            immediate: stats.immediate_frames,
            immediate_ms: stats.immediate_decode_total_ms,
            ms: stats.decode_total_ms,
        }
    }
}

#[derive(Debug)]
pub struct StreamSampler {
    since: Instant,
    counts: Counts,
    latest: Option<StreamSample>,
}

impl StreamSampler {
    pub fn new(now: Instant) -> Self {
        Self {
            since: now,
            counts: Counts::default(),
            latest: None,
        }
    }

    pub fn latest(&self) -> Option<&StreamSample> {
        self.latest.as_ref()
    }

    pub fn observe(&mut self, now: Instant, stats: &SessionStats) {
        let secs = now.saturating_duration_since(self.since).as_secs_f64();
        if secs < 1.0 {
            return;
        }
        let counts = Counts::of(stats);
        let old = self.counts;
        self.counts = counts;
        self.since = now;
        let Some(info) = stats.stream_info else {
            return;
        };
        let calls = counts.calls.saturating_sub(old.calls);
        let immediate = counts.immediate.saturating_sub(old.immediate);
        self.latest = Some(StreamSample {
            sequence: self.latest.as_ref().map_or(1, |s| s.sequence + 1),
            elapsed_secs: secs,
            received_fps: counts.frames.saturating_sub(old.frames) as f64 / secs,
            fps: immediate as f64 / secs,
            video_mbit: counts.bytes.saturating_sub(old.bytes) as f64 * 8.0 / 1_000_000.0 / secs,
            decode_ms: if immediate == 0 {
                0.0
            } else {
                (counts.immediate_ms - old.immediate_ms).max(0.0) / immediate as f64
            },
            decode_call_ms: if calls == 0 {
                0.0
            } else {
                (counts.ms - old.ms).max(0.0) / calls as f64
            },
            video: video_label(stats),
            stream: info.geometry.stream,
            view: info.geometry.view,
            fps_cap: info.fps_cap,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::StreamSampler;
    use crate::decode::DecoderKind;
    use crate::session::SessionStats;
    use crate::session::{DecoderChange, Geometry, StreamInfo};
    use gliff_proto::msg::ChromaMode;
    use std::time::Duration;
    use std::time::Instant;

    fn stats() -> SessionStats {
        SessionStats {
            frames: 60,
            video_bytes: 1_000_000,
            decode_total_ms: 10.0,
            immediate_frames: 2,
            immediate_decode_total_ms: 10.0,
            decoder: Some(DecoderKind::Cpu),
            stream_info: Some(StreamInfo {
                geometry: Geometry {
                    stream: (1920, 1080),
                    view: (3840, 2160),
                    scale_milli: 2000,
                },
                fps_cap: 60,
                chroma: ChromaMode::Single420,
                pipeline: None,
                decoder: DecoderChange::Reset,
            }),
            ..SessionStats::default()
        }
    }

    #[test]
    fn delayed_tick_uses_elapsed_time_and_call_mean_not_a_percentile() {
        let now = Instant::now();
        let mut sampler = StreamSampler::new(now);
        let mut stats = stats();
        stats.decode_ms.record(1.0);
        stats.decode_ms.record(9.0);
        sampler.observe(now + Duration::from_millis(500), &stats);
        assert!(sampler.latest().is_none());
        sampler.observe(now + Duration::from_secs(2), &stats);
        let sample = sampler.latest().expect("sample");
        assert_eq!((sample.sequence, sample.elapsed_secs), (1, 2.0));
        assert_eq!(
            (sample.received_fps, sample.video_mbit, sample.decode_ms),
            (30.0, 4.0, 5.0)
        );
        assert_eq!(
            sample.text(),
            "?→CPU  1 fps  4.0 Mbit/s  decode 5.0 ms  1920x1080 → 3840x2160 @60"
        );
    }

    #[test]
    fn idle_ticks_zero_rates_and_fresh_attempts_do_not_reuse_a_sample() {
        let now = Instant::now();
        let mut sampler = StreamSampler::new(now);
        sampler.observe(now + Duration::from_secs(1), &stats());
        sampler.observe(now + Duration::from_secs(2), &stats());
        let sample = sampler.latest().expect("idle sample");
        assert_eq!(
            (
                sample.received_fps,
                sample.fps,
                sample.video_mbit,
                sample.decode_ms,
                sample.decode_call_ms
            ),
            (0.0, 0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(sample.sequence, 2);
        assert!(StreamSampler::new(now).latest().is_none());
    }

    #[test]
    fn independent_sessions_keep_their_own_identity_and_current_config() {
        let now = Instant::now();
        let mut a = StreamSampler::new(now);
        let mut b = StreamSampler::new(now);
        let mut second = stats();
        second.frames = 10;
        second.immediate_frames = 7;
        second.decoder = Some(DecoderKind::Gpu);
        second.stream_info.as_mut().expect("config").fps_cap = 30;
        second.stream_info.as_mut().expect("config").pipeline =
            Some(gliff_proto::msg::VideoPipeline::Gpu);
        a.observe(now + Duration::from_secs(1), &stats());
        b.observe(now + Duration::from_secs(1), &second);
        assert_eq!(a.latest().expect("a").received_fps, 60.0);
        assert_eq!(b.latest().expect("b").received_fps, 10.0);
        assert_eq!(a.latest().expect("a").fps, 2.0);
        assert_eq!(b.latest().expect("b").fps, 7.0);
        assert_eq!(b.latest().expect("b").fps_cap, 30);
        assert_eq!(b.latest().expect("b").video, "GPU→GPU");
        second.decoder = Some(DecoderKind::Cpu);
        b.observe(now + Duration::from_secs(2), &second);
        assert_eq!(b.latest().expect("fallback").video, "GPU→CPU");
    }

    #[test]
    fn no_config_never_fabricates_dimensions_and_equal_sizes_omit_arrow() {
        let now = Instant::now();
        let mut sampler = StreamSampler::new(now);
        sampler.observe(now + Duration::from_secs(1), &SessionStats::default());
        assert!(sampler.latest().is_none());
        let mut stats = stats();
        stats.stream_info.as_mut().expect("config").geometry.view = (1920, 1080);
        sampler.observe(now + Duration::from_secs(2), &stats);
        assert!(
            sampler
                .latest()
                .expect("sample")
                .text()
                .ends_with("1920x1080 @60")
        );
    }
}
