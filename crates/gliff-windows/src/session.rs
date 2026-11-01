//! The session: the handshake, the decode-time histogram, and
//! the pure protocol state machine [`Core`] in [`machine`].

pub mod driver;
pub mod input_side;
pub mod machine;
pub mod resync;

pub use machine::Action;
pub use machine::ColourCounts;
pub use machine::Core;
pub use machine::Decoded;
pub use machine::DecoderChange;
pub use machine::EndReason;
pub use machine::Geometry;
pub use machine::Picture;
pub use machine::ResizeReq;
pub use machine::SessionStats;
pub use machine::StreamInfo;
pub use machine::UiCmd;
pub use machine::UiEvent;

use gliff_proto::msg::ChromaMode;
use gliff_proto::msg::ClientCaps;
use gliff_proto::msg::ClientMsg;
use gliff_proto::msg::Codec;
use gliff_proto::msg::PROTOCOL_VERSION;

/// The smallest size the client advertises, so a later panel or a
/// fullscreen window never needs a new handshake.
pub const ADVERTISED_FLOOR: (u32, u32) = (3840, 2400);

/// The optional protocol features this client implements, sent in `Hello`.
/// This is not upstream's `gliff_proto::msg::FEATURES`, which lists what
/// upstream's own client and server implement, so bumping the upstream pin
/// cannot advertise a feature this client ignores. Add a name here only in
/// the change that implements it, and use the feature only when the server's
/// `HelloAck` lists it too: a behaviour is used only when both ends list it.
/// `Core` does not keep the `HelloAck` features yet, so the change that adds
/// the first name must also keep them and check them.
pub const FEATURES: &[&str] = &[];

/// The colour a session asks for: a machine's *Sharp colour* choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Colour {
    /// 4:2:0 only, as before the choice existed.
    #[default]
    Standard,
    /// Full-resolution colour (Dual420), with 4:2:0 for the server to fall
    /// back to.
    Sharp,
}

impl Colour {
    /// `Sharp` when the choice is on.
    pub fn sharp(on: bool) -> Self {
        if on { Self::Sharp } else { Self::Standard }
    }

    /// The chroma modes the client offers, preferred first.
    pub fn offered(self) -> &'static [ChromaMode] {
        match self {
            Self::Standard => &[ChromaMode::Single420],
            Self::Sharp => &[ChromaMode::Dual420, ChromaMode::Single420],
        }
    }
}

/// What the client advertises: H.264, the chroma modes of
/// `colour`, a maximum of the component-wise max of `panel` and
/// [`ADVERTISED_FLOOR`], and [`FEATURES`].
pub fn client_caps(panel: (u32, u32), colour: Colour) -> ClientCaps {
    ClientCaps {
        codecs: vec![Codec::H264],
        max_width: panel.0.max(ADVERTISED_FLOOR.0),
        max_height: panel.1.max(ADVERTISED_FLOOR.1),
        chroma: colour.offered().to_vec(),
        features: FEATURES.iter().map(|f| (*f).to_owned()).collect(),
    }
}

/// The first message. The keymap is empty (the server's default) until
/// keyboard input decides how to send the Windows layout.
pub fn hello(caps: ClientCaps, keymap: String) -> ClientMsg {
    ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        keymap,
        caps,
    }
}

/// Width of one histogram bucket in microseconds (0.1 ms), so the
/// 16.7 ms present budget sits on a bucket edge.
const BUCKET_US: u64 = 100;
/// Buckets up to 100 ms, then one overflow bucket.
const BUCKETS: usize = 1001;

/// Fixed 0.1 ms buckets to 100 ms plus overflow, so recording never
/// allocates. A quantile is the upper edge of its bucket (it overstates by
/// at most 0.1 ms), capped at the largest sample seen, which is also the
/// answer in the overflow bucket. Samples are binned in whole
/// microseconds, so a value on an edge never slips into the next bucket
/// through float error.
#[derive(Debug, Clone)]
pub struct Histogram {
    buckets: Box<[u32; BUCKETS]>,
    count: u64,
    max_ms: f32,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: Box::new([0; BUCKETS]),
            count: 0,
            max_ms: 0.0,
        }
    }
}

impl Histogram {
    /// Records one sample; negative and NaN samples count as 0 ms.
    pub fn record(&mut self, ms: f32) {
        let ms = if ms.is_nan() { 0.0 } else { ms.max(0.0) };
        // Bucket i holds (i, i + 1] * 0.1 ms; 0 ms goes in bucket 0. The
        // float-to-int cast saturates.
        let us = (f64::from(ms) * 1000.0).round() as u64;
        let index = usize::try_from(us.div_ceil(BUCKET_US))
            .unwrap_or(usize::MAX)
            .saturating_sub(1)
            .min(BUCKETS - 1);
        self.buckets[index] += 1;
        self.count += 1;
        self.max_ms = self.max_ms.max(ms);
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn max(&self) -> f32 {
        self.max_ms
    }

    /// The smallest bucket edge with at least `q` of the samples at or
    /// below it; 0 when empty. `q` is clamped to 0..=1.
    pub fn quantile(&self, q: f32) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        let q = q.clamp(0.0, 1.0);
        let rank = ((q as f64 * self.count as f64).ceil() as u64).max(1);
        let mut seen = 0u64;
        for (i, n) in self.buckets.iter().enumerate() {
            seen += u64::from(*n);
            if seen >= rank {
                // The overflow bucket has no upper edge, and no edge may
                // claim more than was seen.
                return if i == BUCKETS - 1 {
                    self.max_ms
                } else {
                    (((i as u64 + 1) * BUCKET_US) as f32 / 1000.0).min(self.max_ms)
                };
            }
        }
        self.max_ms
    }
}

#[cfg(test)]
mod tests {
    use gliff_proto::msg::ChromaMode;
    use gliff_proto::msg::ClientMsg;
    use gliff_proto::msg::Codec;
    use gliff_proto::msg::PROTOCOL_VERSION;

    use super::ADVERTISED_FLOOR;
    use super::Colour;
    use super::Histogram;
    use super::client_caps;
    use super::hello;

    #[test]
    fn caps_never_advertise_below_the_floor() {
        let caps = client_caps((1920, 1080), Colour::Standard);
        assert_eq!((caps.max_width, caps.max_height), ADVERTISED_FLOOR);
        assert_eq!(caps.codecs, [Codec::H264]);
        assert_eq!(caps.chroma, [ChromaMode::Single420]);
        let sharp = client_caps((1920, 1080), Colour::Sharp);
        assert_eq!(
            sharp.chroma,
            [ChromaMode::Dual420, ChromaMode::Single420],
            "Sharp colour offers Dual420 and leaves Single420 to fall back to"
        );
        assert_eq!(
            (sharp.max_width, sharp.max_height, sharp.codecs),
            (caps.max_width, caps.max_height, caps.codecs),
            "only the chroma differs"
        );
    }

    #[test]
    fn caps_grow_with_a_larger_panel_per_axis() {
        let caps = client_caps((5120, 1440), Colour::Standard);
        assert_eq!((caps.max_width, caps.max_height), (5120, 2400));
    }

    #[test]
    fn caps_advertise_no_protocol_features_yet() {
        for colour in [Colour::Standard, Colour::Sharp] {
            assert_eq!(
                client_caps((1920, 1080), colour).features,
                Vec::<String>::new(),
                "this client implements no optional feature, whatever the upstream pin lists"
            );
        }
    }

    #[test]
    fn hello_carries_this_protocol_version() {
        let ClientMsg::Hello {
            version, keymap, ..
        } = hello(
            client_caps((3840, 2400), Colour::Standard),
            "xkb text".to_owned(),
        )
        else {
            panic!("hello builds a Hello");
        };
        assert_eq!(version, PROTOCOL_VERSION);
        assert_eq!(keymap, "xkb text", "the layout rides in the Hello");
    }

    #[test]
    fn empty_histogram_reports_zero() {
        assert_eq!(Histogram::default().quantile(0.95), 0.0);
    }

    #[test]
    fn quantiles_are_bucket_upper_edges() {
        let mut h = Histogram::default();
        for ms in 1..=100 {
            h.record(ms as f32 / 10.0); // 0.1 ..= 10.0 ms
        }
        assert_eq!(h.count(), 100);
        assert_eq!(h.quantile(0.5), 5.0);
        assert_eq!(h.quantile(0.95), 9.5);
        assert_eq!(h.quantile(1.0), 10.0);
    }

    #[test]
    fn a_sample_inside_a_bucket_reports_its_upper_edge() {
        let mut h = Histogram::default();
        h.record(0.74);
        h.record(33.2);
        assert_eq!(h.quantile(0.5), 0.8);
    }

    #[test]
    fn a_quantile_never_exceeds_the_largest_sample() {
        // Seen on the build machine: p95 0.5 ms next to a max of 0.24 ms.
        let mut h = Histogram::default();
        h.record(0.24);
        assert_eq!(h.quantile(0.95), 0.24);
        h.record(0.21);
        assert_eq!(h.quantile(0.5), 0.24, "edge 0.3 is capped at the max");
        h.record(33.2);
        assert_eq!(h.quantile(1.0), 33.2);
    }

    #[test]
    fn exact_edges_stay_in_their_bucket() {
        let mut h = Histogram::default();
        h.record(33.0);
        assert_eq!(h.quantile(0.5), 33.0);
        h.record(0.0);
        assert_eq!(h.quantile(0.0), 0.1);
    }

    #[test]
    fn overflow_reports_the_largest_sample() {
        let mut h = Histogram::default();
        h.record(20.0);
        h.record(250.0);
        assert_eq!(h.quantile(0.95), 250.0);
        assert_eq!(h.max(), 250.0);
    }

    #[test]
    fn a_p95_just_under_the_present_budget_stays_under_it() {
        // Regression: with 0.5 ms buckets 16.6 ms was
        // stored as 17.0 and failed the 16.7 ms gate.
        let mut h = Histogram::default();
        for _ in 0..100 {
            h.record(16.6);
        }
        for _ in 0..5 {
            h.record(20.0);
        }
        assert_eq!(h.quantile(0.95), 16.6, "p95 within 0.1 ms of the samples");
        h.record(16.7);
        assert_eq!(h.quantile(1.0), 20.0, "max");
        let mut edge = Histogram::default();
        edge.record(16.7);
        edge.record(30.0);
        assert_eq!(edge.quantile(0.5), 16.7, "16.7 sits on a bucket edge");
    }

    #[test]
    fn nan_and_negative_count_as_zero() {
        let mut h = Histogram::default();
        h.record(f32::NAN);
        h.record(-3.0);
        assert_eq!(h.count(), 2);
        assert_eq!(h.quantile(1.0), 0.0);
    }
}
