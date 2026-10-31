//! The real surface: shapes and names, as the window paints them.

use gliff_windows::palette::Palette;
use gliff_windows::strip_gdi::DcMeasure;
use gliff_windows::strip_gdi::Fonts;
use gliff_windows::strip_gdi::Surface;

use super::Options;
use super::file_name;
use super::judge;
use super::scene;
use super::scene::Scene;
use super::write;
use crate::line::Line;

pub fn run(options: &Options, scenes: &[Scene], scales: &[u32]) -> Line {
    let pal = Palette::DARK;
    let mut failures = Vec::new();
    let mut files = Vec::new();
    for scene in scenes {
        for &scale in scales {
            match one(options, scene, scale, &pal) {
                Ok((path, problems)) => {
                    files.push(path);
                    failures.extend(
                        problems
                            .into_iter()
                            .map(|p| format!("{} at {scale}%: {p}", scene.name)),
                    );
                }
                Err(e) => failures.push(format!("{} at {scale}%: {e:#}", scene.name)),
            }
        }
    }
    Line::new("strip-shot", failures.is_empty())
        .with("mode", "gdi")
        .with("pictures", files.len())
        .with("problems", failures)
        .with("files", files)
}

fn one(
    options: &Options,
    scene: &Scene,
    scale: u32,
    pal: &Palette,
) -> anyhow::Result<(String, Vec<String>)> {
    let fonts = Fonts::new();
    // A first surface measures the names; the strip is then laid out and
    // drawn on a surface of its size.
    let probe = Surface::new(None, 1, 1, pal.strip).ok_or_else(|| anyhow::anyhow!("no bitmap"))?;
    let built = scene::build(
        scene,
        scale,
        &DcMeasure {
            hdc: probe.dc(),
            fonts: &fonts,
        },
    );
    let (w, h) = (built.width, built.metrics.strip_height);
    let mut surface =
        Surface::new(None, w, h, pal.strip).ok_or_else(|| anyhow::anyhow!("no bitmap"))?;
    let ops = built.ops(pal);
    surface.draw(&fonts, &ops);
    let px = surface.pixels();
    let problems = judge::problems(
        scene,
        &built,
        &ops,
        &judge::Picture {
            px,
            width: w,
            height: h,
            text: true,
        },
        pal,
    );
    let path = options.out.join(file_name(scene, scale, false));
    write(&path, px, w, h)?;
    Ok((path.display().to_string(), problems))
}
