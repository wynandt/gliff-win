//! The test pattern, which the remote-side script (`scripts/remote/gwin-pattern`),
//! the fake server and the check all share. The top 4/5 of the output is an
//! 8x4 grid of 32 flat colours. The bottom 1/5 is a black strip with a
//! moving white square, so the server always has damage to send. Every
//! coordinate is a fraction of the picture size, so the pattern compares
//! at any stream size.

use gliff_sw::BgraFrame;

pub const COLUMNS: u32 = 8;
pub const ROWS: u32 = 4;

/// Row-major RGB. Levels 0, 128 and 255 in every combination, then five
/// more; the closest pair is 63 apart on one channel, far above what
/// H.264 4:2:0 does to a flat cell. Keep in sync with the script's
/// PALETTE (a test checks it).
pub const PALETTE: [[u8; 3]; 32] = [
    [0, 0, 0],
    [0, 0, 128],
    [0, 0, 255],
    [0, 128, 0],
    [0, 128, 128],
    [0, 128, 255],
    [0, 255, 0],
    [0, 255, 128],
    [0, 255, 255],
    [128, 0, 0],
    [128, 0, 128],
    [128, 0, 255],
    [128, 128, 0],
    [128, 128, 128],
    [128, 128, 255],
    [128, 255, 0],
    [128, 255, 128],
    [128, 255, 255],
    [255, 0, 0],
    [255, 0, 128],
    [255, 0, 255],
    [255, 128, 0],
    [255, 128, 128],
    [255, 128, 255],
    [255, 255, 0],
    [255, 255, 128],
    [255, 255, 255],
    [64, 64, 64],
    [192, 192, 192],
    [255, 192, 0],
    [0, 64, 192],
    [192, 0, 64],
];

/// The default worst-cell tolerance, in mean absolute error per channel
/// out of 255.
pub const DEFAULT_TOLERANCE: f64 = 40.0;

/// The share of each cell, centred, that is compared (edges blur under
/// scaling and chroma subsampling).
const INNER: f64 = 0.7;

/// Height of the grid: the top 4/5 of the picture.
fn grid_height(height: u32) -> u32 {
    height * 4 / 5
}

/// Cell (column, row) as `x0..x1, y0..y1`, the same integer maths as the
/// script's drawbox expressions.
pub fn cell_rect(width: u32, height: u32, column: u32, row: u32) -> (u32, u32, u32, u32) {
    let grid = grid_height(height);
    (
        column * width / COLUMNS,
        (column + 1) * width / COLUMNS,
        row * grid / ROWS,
        (row + 1) * grid / ROWS,
    )
}

// The drawing half serves gwin-fake-server and the tests.
#[cfg_attr(not(test), allow(dead_code))]
/// What [`render`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// The grid and the moving square.
    Pattern,
    /// A grey desktop with the moving square only (fake `no-pattern`).
    Desktop,
    /// One flat colour (R, G, B) where the grid would be, with the moving
    /// square: a fake server a test can tell from another by its picture
    /// (`GWIN_FAKE_TINT`).
    Flat([u8; 3]),
}

#[cfg_attr(not(test), allow(dead_code))]
/// Draws frame `n` as BGRA into `out` (resized to fit).
pub fn render(width: u32, height: u32, n: u64, content: Content, out: &mut Vec<u8>) {
    background(width, height, content, out);
    square(width, height, n, out);
}

#[cfg_attr(not(test), allow(dead_code))]
/// Everything but the moving square. The fake server draws this once per
/// size and copies it, since only the square changes.
pub fn background(width: u32, height: u32, content: Content, out: &mut Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    out.resize(w * h * 4, 0);
    let grid = grid_height(height) as usize;
    for y in 0..h {
        let row = out[y * w * 4..(y + 1) * w * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut();
        for (x, px) in row.enumerate() {
            let rgb = match (content, y < grid) {
                (Content::Pattern, true) => {
                    let column = (x as u32 * COLUMNS / width).min(COLUMNS - 1);
                    let r = (y as u32 * ROWS / grid as u32).min(ROWS - 1);
                    PALETTE[(r * COLUMNS + column) as usize]
                }
                (Content::Desktop, true) => [96, 96, 104],
                (Content::Flat(rgb), true) => rgb,
                (_, false) => [0, 0, 0],
            };
            px.copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
/// The square for frame `n`: 60% of the strip high, moving 1/120 of the
/// width per frame (the script's overlay expression).
pub fn square(width: u32, height: u32, n: u64, out: &mut [u8]) {
    let (w, h) = (width as usize, height as usize);
    let grid = grid_height(height) as usize;
    let strip = h - grid;
    let side = (strip * 3 / 5).max(2);
    let span = w.saturating_sub(side).max(1);
    let step = (w / 120).max(1) as u64;
    let x0 = ((n * step) % span as u64) as usize;
    let y0 = grid + (strip - side) / 2;
    for y in y0..(y0 + side).min(h) {
        let start = (y * w + x0) * 4;
        let end = (y * w + (x0 + side).min(w)) * 4;
        out[start..end].fill(255);
    }
}

/// How close a picture is to the pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    /// Mean absolute error per channel of each cell's inner area, averaged
    /// over the 32 cells.
    pub mae_mean: f64,
    pub mae_max: f64,
    /// Index of the worst cell (row-major).
    pub worst_cell: usize,
    /// Cells whose mean colour is nearest to their own palette entry.
    pub cells_matched: usize,
    pub tolerance: f64,
}

impl Score {
    pub fn verdict(&self) -> Verdict {
        if self.cells_matched == PALETTE.len() && self.mae_max <= self.tolerance {
            Verdict::Match
        } else {
            Verdict::Mismatch
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Match,
    Mismatch,
}

/// Compares a BGRA picture with the pattern; the moving strip is ignored.
pub fn compare(frame: &BgraFrame, tolerance: f64) -> Score {
    let (w, h) = (frame.width, frame.height);
    let mut maes = Vec::with_capacity(PALETTE.len());
    let mut matched = 0;
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let expected = PALETTE[(row * COLUMNS + column) as usize];
            let mean = inner_mean(frame, cell_rect(w, h, column, row));
            let mae = (0..3)
                .map(|c| (mean[c] - f64::from(expected[c])).abs())
                .sum::<f64>()
                / 3.0;
            if nearest(mean) == (row * COLUMNS + column) as usize {
                matched += 1;
            }
            maes.push(mae);
        }
    }
    let (worst_cell, mae_max) =
        maes.iter().copied().enumerate().fold(
            (0, 0.0),
            |best, (i, m)| if m > best.1 { (i, m) } else { best },
        );
    Score {
        mae_mean: maes.iter().sum::<f64>() / maes.len() as f64,
        mae_max,
        worst_cell,
        cells_matched: matched,
        tolerance,
    }
}

/// Mean RGB of the centred [`INNER`] share of a cell.
fn inner_mean(frame: &BgraFrame, (x0, x1, y0, y1): (u32, u32, u32, u32)) -> [f64; 3] {
    let trim = |a: u32, b: u32| {
        let margin = (f64::from(b - a) * (1.0 - INNER) / 2.0) as u32;
        (a + margin, (b - margin).max(a + margin + 1))
    };
    let (x0, x1) = trim(x0, x1);
    let (y0, y1) = trim(y0, y1);
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for y in y0..y1.min(frame.height) {
        for x in x0..x1.min(frame.width) {
            let i = ((y * frame.width + x) * 4) as usize;
            // BGRA.
            sum[0] += u64::from(frame.pixels[i + 2]);
            sum[1] += u64::from(frame.pixels[i + 1]);
            sum[2] += u64::from(frame.pixels[i]);
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    [sum[0] as f64 / n, sum[1] as f64 / n, sum[2] as f64 / n]
}

fn nearest(rgb: [f64; 3]) -> usize {
    let distance = |p: &[u8; 3]| {
        (0..3)
            .map(|c| (rgb[c] - f64::from(p[c])).powi(2))
            .sum::<f64>()
    };
    PALETTE
        .iter()
        .enumerate()
        .min_by(|a, b| distance(a.1).total_cmp(&distance(b.1)))
        .map_or(0, |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use gliff_sw::BgraFrame;
    use gliff_sw::Decoder;
    use gliff_sw::Encoder;
    use gliff_sw::EncoderSettings;

    use super::Content;
    use super::PALETTE;
    use super::Verdict;
    use super::compare;
    use super::render;

    fn frame(width: u32, height: u32, content: Content) -> BgraFrame {
        let mut pixels = Vec::new();
        render(width, height, 3, content, &mut pixels);
        BgraFrame {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn the_rendered_pattern_matches_exactly() {
        let score = compare(&frame(1280, 800, Content::Pattern), 40.0);
        assert_eq!(score.cells_matched, 32, "{score:?}");
        assert!(score.mae_max < 1e-9, "exact render: {score:?}");
        assert_eq!(score.verdict(), Verdict::Match, "{score:?}");
    }

    #[test]
    fn a_flat_tint_fills_the_grid_and_is_not_the_pattern() {
        let flat = frame(1280, 800, Content::Flat([40, 120, 220]));
        assert_eq!(&flat.pixels[..4], &[220, 120, 40, 255], "BGRA of the tint");
        let score = compare(&flat, 40.0);
        assert_eq!(score.verdict(), Verdict::Mismatch, "{score:?}");
    }

    #[test]
    fn a_desktop_without_the_pattern_mismatches() {
        let score = compare(&frame(1280, 800, Content::Desktop), 40.0);
        assert_eq!(score.verdict(), Verdict::Mismatch, "{score:?}");
        assert!(
            score.cells_matched < 4,
            "a grey desktop matches almost nothing: {score:?}"
        );
    }

    #[test]
    fn the_pattern_survives_openh264_and_bt709() {
        let (w, h) = (1280, 800);
        let source = frame(w, h, Content::Pattern);
        let mut encoder = Encoder::new(
            EncoderSettings {
                width: w,
                height: h,
                bitrate: 8_000_000,
                framerate: 30,
            },
            false,
        )
        .expect("encoder");
        let mut decoder = Decoder::new(false, w, h).expect("decoder");
        let mut out = None;
        for _ in 0..3 {
            let au = encoder
                .encode_bgra(&source.pixels, out.is_none())
                .expect("encode");
            if let Some(f) = decoder.decode(&au.main, &[]).expect("decode") {
                out = Some(f);
            }
        }
        let decoded = out
            .or_else(|| decoder.flush().expect("flush"))
            .expect("a picture");
        let score = compare(&decoded, 40.0);
        assert_eq!(score.verdict(), Verdict::Match, "round trip: {score:?}");
        assert!(score.mae_max < 15.0, "flat cells stay close: {score:?}");
    }

    #[test]
    fn the_remote_script_draws_the_same_palette() {
        let script = include_str!("../../../scripts/remote/gwin-pattern");
        let line = script
            .lines()
            .find(|l| l.starts_with("PALETTE=("))
            .expect("the script defines PALETTE=(...)");
        let hex: Vec<&str> = line
            .trim_start_matches("PALETTE=(")
            .trim_end_matches(')')
            .split_whitespace()
            .collect();
        let ours: Vec<String> = PALETTE
            .iter()
            .map(|[r, g, b]| format!("{r:02x}{g:02x}{b:02x}"))
            .collect();
        assert_eq!(
            hex, ours,
            "scripts/remote/gwin-pattern PALETTE must match pattern::PALETTE"
        );
    }

    /// `frame` scaled up `k` times by pixel repeat, as Hyprland scales the
    /// script's small source to the output (filtering only softens edges,
    /// which the inner 70% skips).
    fn upscaled(frame: &BgraFrame, k: u32) -> BgraFrame {
        let (width, height) = (frame.width * k, frame.height * k);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let i = (((y / k) * frame.width + x / k) * 4) as usize;
                pixels.extend_from_slice(&frame.pixels[i..i + 4]);
            }
        }
        BgraFrame {
            width,
            height,
            pixels,
        }
    }

    /// A real present run at 200%: the output is a 3840x2400 mode at
    /// scale 2 (1920x1200 logical) and the client shows the 3840x2400
    /// stream. The script's 960x600 source fills it 4x; the cells are
    /// fractions of the picture, so it scores as the pattern.
    #[test]
    fn a_960x600_source_on_the_scale_2_panel_matches() {
        let panel = upscaled(&frame(960, 600, Content::Pattern), 4);
        assert_eq!((panel.width, panel.height), (3840, 2400), "the panel size");
        let score = compare(&panel, 40.0);
        assert_eq!(score.cells_matched, 32, "every cell at 4x: {score:?}");
        assert_eq!(score.verdict(), Verdict::Match, "{score:?}");
    }

    /// ffplay -fs letterboxes, so a 16:9 source (the clamped first mode's)
    /// on the 16:10 panel shifts the rows by up to 120 px. The inner-70%
    /// compare still passes it (the margin shrinks), so the script's
    /// aspect-following source is for an exact picture, not to pass.
    #[test]
    fn a_16_9_source_letterboxed_on_16_10_still_matches_with_less_margin() {
        let wide = upscaled(&frame(960, 540, Content::Pattern), 4);
        let bar = (2400 - wide.height) / 2;
        let row = (wide.width * 4) as usize;
        let mut pixels = vec![0u8; row * bar as usize];
        let (black, _) = pixels.as_chunks_mut::<4>();
        black.iter_mut().for_each(|px| px[3] = 255);
        let mut boxed = pixels.clone();
        boxed.extend_from_slice(&wide.pixels);
        boxed.extend_from_slice(&pixels);
        let panel = BgraFrame {
            width: 3840,
            height: 2400,
            pixels: boxed,
        };
        let score = compare(&panel, 40.0);
        assert_eq!(score.verdict(), Verdict::Match, "letterboxed: {score:?}");
        let fitted = compare(&upscaled(&frame(960, 600, Content::Pattern), 4), 40.0);
        assert!(
            score.mae_max > fitted.mae_max + 10.0,
            "the letterbox costs margin: letterboxed {score:?}, fitted {fitted:?}"
        );
    }

    /// The script's real graph through ffmpeg (build machine: `cargo test -p
    /// gwin-verify -- --ignored ffmpeg`): at the 960x600 source `follow`
    /// uses for a 16:10 output it is the pattern at 4x, it runs at 60 fps,
    /// and consecutive frames differ (continuous damage).
    #[test]
    #[ignore = "needs bash and ffmpeg on the build machine"]
    fn ffmpeg_renders_the_follow_source_as_moving_pattern_frames() {
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/remote/gwin-pattern"
        );
        let graph = std::process::Command::new("bash")
            .args([script, "graph", "960x600"])
            .output()
            .expect("run gwin-pattern graph");
        assert!(graph.status.success(), "gwin-pattern graph: {graph:?}");
        let graph = String::from_utf8(graph.stdout).expect("utf-8 graph");
        assert!(graph.contains("s=960x600:r=60"), "60 fps source: {graph}");
        let raw = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", graph.trim()])
            .args(["-frames:v", "3", "-vf", "scale=3840:2400:flags=bilinear"])
            .args(["-pix_fmt", "bgra", "-f", "rawvideo", "-"])
            .output()
            .expect("run ffmpeg");
        assert!(
            raw.status.success(),
            "ffmpeg: {}",
            String::from_utf8_lossy(&raw.stderr)
        );
        let size = 3840 * 2400 * 4;
        assert_eq!(raw.stdout.len(), 3 * size, "three 3840x2400 BGRA frames");
        let frames: Vec<&[u8]> = raw.stdout.chunks_exact(size).collect();
        for (n, pixels) in frames.iter().enumerate() {
            let score = compare(
                &BgraFrame {
                    width: 3840,
                    height: 2400,
                    pixels: pixels.to_vec(),
                },
                40.0,
            );
            println!("frame {n}: {score:?}");
            assert_eq!(
                score.verdict(),
                Verdict::Match,
                "frame {n} at 4x: {score:?}"
            );
        }
        assert_ne!(
            frames[0], frames[1],
            "frame 1 differs from frame 0: the square moves"
        );
        assert_ne!(
            frames[1], frames[2],
            "frame 2 differs from frame 1: the square moves"
        );
    }

    /// Build-machine-only cross-check of the script's lavfi graph: render it with
    /// ffmpeg to raw BGRA and point `GWV_PATTERN_RAW=path:WxH` at it
    /// (the commands are in docs/development.md).
    #[test]
    #[ignore = "needs a frame rendered by ffmpeg from pattern_graph"]
    fn an_ffmpeg_render_of_the_script_graph_matches() {
        let spec = std::env::var("GWV_PATTERN_RAW").expect("GWV_PATTERN_RAW=path:WxH");
        let (path, size) = spec.rsplit_once(':').expect("path:WxH");
        let (w, h) = size.split_once('x').expect("WxH");
        let frame = BgraFrame {
            width: w.parse().expect("width"),
            height: h.parse().expect("height"),
            pixels: std::fs::read(path).expect("raw frame"),
        };
        let score = compare(&frame, 40.0);
        println!("{score:?}");
        assert_eq!(score.verdict(), Verdict::Match, "ffmpeg render: {score:?}");
    }
}
