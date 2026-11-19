//! The parts of upstream gliff-server, at the revision pinned in the
//! workspace `Cargo.toml`, that the fake reproduces, as pure functions
//! that name the upstream symbol they follow.

/// Upper bound of the OpenH264 CPU encoder (`gliff_sw::Encoder::MAX_SIZE`).
pub const CPU_ENCODER_MAX: (u32, u32) = (3840, 2160);

/// The ladder's top fps (`MAX_FPS`) and its starting
/// level's cap (`START_LEVEL` = 1, 30 fps in `rate::LEVELS`).
pub const MAX_FPS: u32 = 60;
pub const START_FPS: u32 = 30;

/// The ladder steps up only after this long at a level (`HOLD_UP`).
pub const HOLD_UP_MS: u64 = 4_000;

/// The byte-cap floor: twice the largest frame seen,
/// at least 96 KiB. The fake has no RTT estimate, so the floor is the cap.
pub fn cap_bytes(largest_frame: usize) -> u64 {
    (2 * largest_frame).max(96 * 1024) as u64
}

/// A frame goes out only if nothing is in flight, or the next one still
/// fits under the cap.
pub fn has_room(inflight: u64, est_frame: u64, cap: u64) -> bool {
    inflight == 0 || inflight + est_frame <= cap
}

/// The first headless mode: the client's declared maximum, clamped to
/// 320..1920 x 240..1080 and made even.
pub fn initial_mode(max: (u32, u32)) -> (u32, u32) {
    (max.0.clamp(320, 1920) & !1, max.1.clamp(240, 1080) & !1)
}

/// The server's headless output cap: 4096 on a side, and 3840x2160 pixels.
///
/// Upstream: `HEADLESS_LIMIT` at 58c7424.
const HEADLESS_SIDE: u32 = 4096;
const HEADLESS_PIXELS: u64 = 3840 * 2160;

/// Fit a requested headless size within the server's limits, keeping the
/// aspect ratio and the sides even. A window too narrow to scale stays at
/// the smallest size resize accepts.
///
/// Upstream: `headless_cap` at 58c7424.
pub fn headless_cap(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width), f64::from(height));
    let fit = (f64::from(HEADLESS_SIDE) / w)
        .min(f64::from(HEADLESS_SIDE) / h)
        .min((HEADLESS_PIXELS as f64 / (w * h)).sqrt())
        .min(1.0);
    let w = ((w * fit) as u32).clamp(320, HEADLESS_SIDE);
    let h = ((h * fit) as u32).clamp(240, HEADLESS_SIDE);
    (w & !1, h & !1)
}

/// The headless mode to create: the request within the server's cap,
/// rounded so the scale divides it.
///
/// Upstream: `headless_target` at 58c7424.
pub fn headless_target(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let (width, height) = headless_cap(width, height);
    clean_mode(width, height, scale)
}

/// What a Resize asks of the output: the size clamped to the caps maximum
/// and made even, the scale clamped to 0.5..4.0, `None` under 320x240, then
/// the pin's `headless_target`.
pub fn resize_mode(req: (u32, u32), scale: f32, caps_max: (u32, u32)) -> Option<((u32, u32), f32)> {
    let width = req.0.min(caps_max.0) & !1;
    let height = req.1.min(caps_max.1) & !1;
    let scale = if scale.is_finite() {
        scale.clamp(0.5, 4.0)
    } else {
        1.0
    };
    if width < 320 || height < 240 {
        return None;
    }
    Some((headless_target(width, height, scale), scale))
}

/// Upstream `hypr_ipc::clean_mode`, the same arithmetic: round the mode down
/// until the scale divides it into whole logical pixels, staying even.
pub fn clean_mode(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let s = f64::from(scale);
    let Some(q) = (1..=8u32).find(|q| {
        let numerator = s * f64::from(*q);
        (numerator - numerator.round()).abs() < 1e-4
    }) else {
        return (width, height);
    };
    let p = (s * f64::from(q)).round() as u32;
    let step = if p.is_multiple_of(2) { p } else { 2 * p };
    let fit = |v: u32| (v - v % step).max(step);
    (fit(width), fit(height))
}

/// Upstream `gliff_va::EncoderSettings::fit_extent`: the largest even size with the same
/// aspect whose 16-aligned coded size fits the encoder maximum.
pub fn fit_extent(width: u32, height: u32, max: (u32, u32)) -> (u32, u32) {
    let (max_w, max_h) = (max.0 & !15, max.1 & !15);
    if width <= max_w && height <= max_h {
        return (width, height);
    }
    let scale = (f64::from(max_w) / f64::from(width.max(1)))
        .min(f64::from(max_h) / f64::from(height.max(1)));
    let fit = |v: u32| (((f64::from(v) * scale).floor() as u32) & !1).max(2);
    (fit(width), fit(height))
}

/// `scale_milli` in StreamConfig: the output scale times the stream's
/// downscale of the output.
pub fn scale_milli(output_scale: f32, stream_width: u32, output_width: u32) -> u32 {
    (output_scale * stream_width as f32 / output_width.max(1) as f32 * 1000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::CPU_ENCODER_MAX;
    use super::clean_mode;
    use super::fit_extent;
    use super::has_room;
    use super::headless_cap;
    use super::headless_target;
    use super::initial_mode;
    use super::resize_mode;
    use super::scale_milli;

    #[test]
    fn the_3840x2400_panel_on_the_cpu_encoder_is_scaled_to_3456x2160() {
        assert_eq!(
            fit_extent(3840, 2400, CPU_ENCODER_MAX),
            (3456, 2160),
            "fit_extent"
        );
        assert_eq!(
            scale_milli(1.0, 3456, 3840),
            900,
            "scale_milli of an uncapped output"
        );
        assert_eq!(
            scale_milli(1.0, 3456, 3642),
            949,
            "scale_milli of the capped 3840x2400 output"
        );
        assert_eq!(fit_extent(1280, 800, CPU_ENCODER_MAX), (1280, 800), "fits");
        assert_eq!(
            fit_extent(3840, 2168, CPU_ENCODER_MAX),
            (3824, 2160),
            "floor-and-even, not rounded 3826x2160"
        );
    }

    #[test]
    fn headless_starts_clamped_and_resize_follows_upstream() {
        assert_eq!(initial_mode((3840, 2400)), (1920, 1080), "initial clamp");
        assert_eq!(
            resize_mode((3840, 2400), 1.0, (3840, 2400)),
            Some(((3642, 2276), 1.0)),
            "resize follows headless_target"
        );
        assert_eq!(
            resize_mode((319, 800), 1.0, (3840, 2400)),
            None,
            "too small is ignored"
        );
        assert_eq!(
            resize_mode((5000, 801), 9.0, (3840, 2400)),
            Some(((3840, 800), 4.0)),
            "clamped"
        );
        assert_eq!(
            clean_mode(1366, 768, 1.25),
            (1360, 760),
            "clean_mode at 5/4"
        );
    }

    #[test]
    fn the_byte_cap_blocks_only_with_frames_in_flight() {
        assert!(has_room(0, 1 << 30, 1), "nothing in flight always sends");
        assert!(!has_room(90_000, 10_000, 98_304), "over the cap waits");
    }

    #[test]
    fn headless_cap_matches_the_pin() {
        assert_eq!(headless_cap(1920, 1080), (1920, 1080), "within the limits");
        assert_eq!(headless_cap(3840, 2160), (3840, 2160), "encoder maximum");
        assert_eq!(
            headless_cap(3840, 2400),
            (3642, 2276),
            "over the pixel limit, same aspect"
        );
        assert_eq!(headless_cap(8192, 1000), (4096, 500), "a long side");
        assert_eq!(
            headless_target(3840, 2400, 1.0),
            (3642, 2276),
            "scale 1.0 does not round further"
        );
        assert_eq!(
            headless_target(1366, 768, 1.25),
            (1360, 760),
            "clean_mode after a size that already fits"
        );
    }
}
