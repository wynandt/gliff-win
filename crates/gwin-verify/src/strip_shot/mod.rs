//! `gwin-verify strip-shot`: draws the tab strip's states at every display
//! scale into PNG files, judges each picture, and needs no input: it opens
//! no window, grabs no keyboard or mouse and touches no clipboard. The
//! pictures use the draw list the client's window runs, drawn on the same
//! surface code (`gliff_windows::strip_gdi::Surface`) on Windows. Elsewhere
//! only the shapes are drawn (no names), which is enough to look at dots,
//! buttons and tab shapes.
//!
//! The strip's right-click and chevron menus are native Windows menus and
//! cannot be drawn without a window; their entries are checked in the
//! menu model's tests (`strip::menu`).

pub mod judge;
pub mod scene;
#[cfg(test)]
mod tests;
#[cfg(windows)]
pub mod win32;

use std::path::Path;
use std::path::PathBuf;

use gliff_sw::BgraFrame;
use gliff_windows::palette::Palette;
use gliff_windows::strip::Measure;
use gliff_windows::strip::raster::Canvas;

use self::scene::Built;
use self::scene::Scene;
use crate::line::Line;

pub struct Options {
    /// The pictures go here, as `strip-<scene>-<scale>pct.png`.
    pub out: PathBuf,
    /// Scales in percent; empty: all of them.
    pub scales: Vec<u32>,
    /// Scene names; empty: all of them.
    pub scenes: Vec<String>,
    /// Draw shapes only, no names (the default off Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub shapes_only: bool,
}

/// Names as wide as the GDI font makes them, near enough for a build
/// machine that has no font.
pub struct Approx;

impl Measure for Approx {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        (text.chars().count() as i32 * size * 11 + 10) / 20
    }
}

/// A picture of the shapes of `built`, no names.
pub fn shapes(built: &Built, palette: &Palette) -> (Vec<u32>, i32, i32) {
    let (w, h) = (built.width, built.metrics.strip_height);
    let c = palette.strip;
    let mut px =
        vec![u32::from(c.0) << 16 | u32::from(c.1) << 8 | u32::from(c.2); (w * h) as usize];
    let mut canvas = Canvas::new(&mut px, w, h);
    for op in built.ops(palette) {
        canvas.shape(&op);
    }
    (px, w, h)
}

pub fn write(path: &Path, px: &[u32], width: i32, height: i32) -> anyhow::Result<()> {
    let frame = BgraFrame {
        width: width as u32,
        height: height as u32,
        pixels: px.iter().flat_map(|p| p.to_le_bytes()).collect(),
    };
    crate::png_out::write_png(path, &frame)
}

pub fn file_name(scene: &Scene, scale: u32, shapes_only: bool) -> String {
    let kind = if shapes_only { "-shapes" } else { "" };
    format!("strip-{}{kind}-{scale}pct.png", scene.name)
}

/// The scenes and scales an [`Options`] asks for, or why it asks for
/// something unknown.
pub fn selection(options: &Options) -> Result<(Vec<Scene>, Vec<u32>), String> {
    let all = scene::scenes();
    for name in &options.scenes {
        if !all.iter().any(|s| s.name == name) {
            let known: Vec<&str> = all.iter().map(|s| s.name).collect();
            return Err(format!(
                "no scene {name:?}; scenes are {}",
                known.join(", ")
            ));
        }
    }
    for scale in &options.scales {
        if !scene::SCALES.contains(scale) {
            return Err(format!("no scale {scale}; scales are 100, 125, 150, 200"));
        }
    }
    let scenes = all
        .into_iter()
        .filter(|s| options.scenes.is_empty() || options.scenes.iter().any(|n| n == s.name))
        .collect();
    let scales = if options.scales.is_empty() {
        scene::SCALES.to_vec()
    } else {
        options.scales.clone()
    };
    Ok((scenes, scales))
}

pub fn run(options: &Options) -> Line {
    let (scenes, scales) = match selection(options) {
        Ok(s) => s,
        Err(e) => return Line::new("strip-shot", false).with("error", e),
    };
    #[cfg(windows)]
    if !options.shapes_only {
        return win32::run(options, &scenes, &scales);
    }
    run_shapes(options, &scenes, &scales)
}

fn run_shapes(options: &Options, scenes: &[Scene], scales: &[u32]) -> Line {
    let pal = Palette::DARK;
    let mut failures = Vec::new();
    let mut files = Vec::new();
    for scene in scenes {
        for &scale in scales {
            let built = scene::build(scene, scale, &Approx);
            let (px, w, h) = shapes(&built, &pal);
            let ops = built.ops(&pal);
            let pic = judge::Picture {
                px: &px,
                width: w,
                height: h,
                text: false,
            };
            for p in judge::problems(scene, &built, &ops, &pic, &pal) {
                failures.push(format!("{} at {scale}%: {p}", scene.name));
            }
            let path = options.out.join(file_name(scene, scale, true));
            match write(&path, &px, w, h) {
                Ok(()) => files.push(path.display().to_string()),
                Err(e) => failures.push(format!("{}: {e:#}", path.display())),
            }
        }
    }
    Line::new("strip-shot", failures.is_empty())
        .with("mode", "shapes-only")
        .with("pictures", files.len())
        .with("problems", failures)
        .with("files", files)
}
