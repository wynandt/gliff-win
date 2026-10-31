//! The parts of upstream gliff-server (089662fd) the fake reproduces, as
//! pure functions with the file:line they come from. Paths are relative to
//! `crates/` in the upstream tree.

/// Upper bound of the OpenH264 CPU encoder (gliff-server/src/session.rs:1247-1252,
/// gliff-sw/src/lib.rs:238).
pub const CPU_ENCODER_MAX: (u32, u32) = (3840, 2160);

/// The ladder's top fps (`MAX_FPS`, session.rs:483) and its starting
/// level's cap (`START_LEVEL` = 1, 30 fps; rate.rs:324-337, session.rs:493).
pub const MAX_FPS: u32 = 60;
pub const START_FPS: u32 = 30;

/// The ladder steps up only after this long at a level (`HOLD_UP`, rate.rs:376).
pub const HOLD_UP_MS: u64 = 4_000;

/// The byte-cap floor (rate.rs:286-296): twice the largest frame seen,
/// at least 96 KiB. The fake has no RTT estimate, so the floor is the cap.
pub fn cap_bytes(largest_frame: usize) -> u64 {
    (2 * largest_frame).max(96 * 1024) as u64
}

/// A frame goes out only if nothing is in flight, or the next one still
/// fits under the cap (session.rs:882-890).
pub fn has_room(inflight: u64, est_frame: u64, cap: u64) -> bool {
    inflight == 0 || inflight + est_frame <= cap
}

/// The first headless mode: the client's declared maximum, clamped to
/// 320..1920 x 240..1080 and made even (session.rs:1579-1580).
pub fn initial_mode(max: (u32, u32)) -> (u32, u32) {
    (max.0.clamp(320, 1920) & !1, max.1.clamp(240, 1080) & !1)
}

/// What a Resize asks of the output (session.rs:625-645): the size
/// clamped to the caps maximum and made even, the scale clamped to
/// 0.5..4.0, `None` under 320x240, then `clean_mode`.
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
    Some((clean_mode(width, height, scale), scale))
}

/// hypr-ipc/src/lib.rs:197-209, the same arithmetic: round the mode down
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

/// gliff-va/src/settings.rs:34-42: the largest even size with the same
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
/// downscale of the output (session.rs:1494-1500).
pub fn scale_milli(output_scale: f32, stream_width: u32, output_width: u32) -> u32 {
    (output_scale * stream_width as f32 / output_width.max(1) as f32 * 1000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::CPU_ENCODER_MAX;
    use super::clean_mode;
    use super::fit_extent;
    use super::has_room;
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
        assert_eq!(scale_milli(1.0, 3456, 3840), 900, "scale_milli");
        assert_eq!(fit_extent(1280, 800, CPU_ENCODER_MAX), (1280, 800), "fits");
    }

    #[test]
    fn headless_starts_clamped_and_resize_follows_upstream() {
        assert_eq!(initial_mode((3840, 2400)), (1920, 1080), "initial clamp");
        assert_eq!(
            resize_mode((3840, 2400), 1.0, (3840, 2400)),
            Some(((3840, 2400), 1.0)),
            "resize"
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
}
