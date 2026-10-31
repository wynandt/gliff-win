//! `gwin-verify panel-shot`: draws the failure panel for every failure at
//! every display scale into PNG files and judges each picture. No window, no
//! keyboard or mouse, no clipboard. On Windows the pictures use the client's
//! own surface code and so show the words; elsewhere only the shapes.

#[cfg(test)]
mod tests;

use std::path::Path;
use std::path::PathBuf;

use gliff_sw::BgraFrame;
use gliff_windows::connect::ServerVersion;
use gliff_windows::connect::SshFailure;
use gliff_windows::failure_panel;
use gliff_windows::failure_panel::Layout;
use gliff_windows::failure_panel::Metrics;
use gliff_windows::palette::Palette;
use gliff_windows::palette::Rgb;
use gliff_windows::strip::Measure;
use gliff_windows::strip::Rect;
use gliff_windows::strip::draw::Op;
use gliff_windows::strip::raster::Canvas;

use crate::line::Line;

/// The scales in percent.
pub const SCALES: [u32; 4] = [100, 125, 150, 200];

/// The picture area drawn, in logical px.
const AREA: (i32, i32) = (640, 400);

pub struct Options {
    pub out: PathBuf,
    pub scales: Vec<u32>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub shapes_only: bool,
}

/// One failure to draw, with the name its file gets.
pub struct Case {
    pub name: &'static str,
    pub failure: SshFailure,
}

/// Every failure the panel can show.
pub fn cases() -> Vec<Case> {
    let case = |name, failure| Case { name, failure };
    vec![
        case("client-missing", SshFailure::ClientMissing),
        case("name-not-found", SshFailure::NameNotFound),
        case("refused", SshFailure::Refused),
        case("unreachable", SshFailure::Unreachable),
        case("host-unknown", SshFailure::HostUnknown),
        case("host-key-changed", SshFailure::HostKeyChanged),
        case("key-refused", SshFailure::KeyRefused),
        case("key-file-permissions", SshFailure::KeyFilePermissions),
        case("too-many-keys", SshFailure::TooManyKeys),
        case("server-missing", SshFailure::ServerMissing),
        case(
            "server-old",
            SshFailure::ServerOld {
                found: ServerVersion::new(0, 3, 9),
            },
        ),
        case("protocol-mismatch", SshFailure::ProtocolMismatch),
        case(
            "other",
            SshFailure::Other {
                last_line: "Connection closed by remote host.".to_owned(),
            },
        ),
    ]
}

/// Names as wide as the GDI font makes them, near enough for a machine with
/// no font.
pub struct Approx;

impl Measure for Approx {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        (text.chars().count() as i32 * size * 11 + 10) / 20
    }
}

/// One panel, laid out and ready to draw.
pub struct Built {
    pub layout: Layout,
    pub ops: Vec<Op>,
    pub width: i32,
    pub height: i32,
}

pub fn build(case: &Case, scale: u32, measure: &dyn Measure, pal: &Palette) -> Built {
    let dpi = scale * 96 / 100;
    let (width, height) = (AREA.0 * scale as i32 / 100, AREA.1 * scale as i32 / 100);
    let target = "me@lab-pc".parse().expect("a target");
    let content = failure_panel::content(&case.failure, &target);
    let layout = failure_panel::layout(&Metrics::at(dpi), &content, width, height, measure);
    let ops = failure_panel::ops(&layout, pal, None);
    Built {
        layout,
        ops,
        width,
        height,
    }
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn colour(px: &[u32], width: i32, x: i32, y: i32) -> Rgb {
    let p = px[(y * width + x) as usize];
    Rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
}

fn has_ink(px: &[u32], width: i32, r: Rect, behind: Rgb) -> bool {
    (r.top..r.bottom).any(|y| (r.left..r.right).any(|x| colour(px, width, x, y) != behind))
}

/// Every way the picture falls short; empty when it is right. `text` says
/// whether words were drawn, so their ink can be looked for.
pub fn problems(built: &Built, px: &[u32], text: bool, pal: &Palette) -> Vec<String> {
    let mut out = Vec::new();
    let area = Rect {
        left: 0,
        top: 0,
        right: built.width,
        bottom: built.height,
    };
    if px.len() != (built.width * built.height) as usize {
        out.push("the picture is not the size of the area".to_owned());
        return out;
    }
    let l = &built.layout;
    let mut parts: Vec<(&str, Rect)> = l
        .lines
        .iter()
        .map(|line| ("a line of text", line.rect))
        .collect();
    parts.push(("the primary button", l.primary));
    parts.extend(l.details.map(|r| ("Details", r)));
    parts.extend(l.link.map(|r| ("the link", r)));
    for (i, (name, r)) in parts.iter().enumerate() {
        let inside = r.left >= area.left
            && r.top >= area.top
            && r.right <= area.right
            && r.bottom <= area.bottom;
        if !inside {
            out.push(format!("{name} {r:?} leaves the picture"));
        }
        for (other, o) in &parts[i + 1..] {
            if overlap(*r, *o) {
                out.push(format!("{name} and {other} overlap"));
            }
        }
    }
    let black = Rgb(0, 0, 0);
    let (cx, cy) = ((l.primary.left + 2), (l.primary.top + l.primary.bottom) / 2);
    if colour(px, built.width, cx, cy) != pal.primary {
        out.push("the primary button is not drawn in its colour".to_owned());
    }
    if let Some(d) = l.details
        && colour(px, built.width, d.left + 2, (d.top + d.bottom) / 2) != pal.go
    {
        out.push("Details is not drawn in its colour".to_owned());
    }
    if text {
        for line in &l.lines {
            if !has_ink(px, built.width, line.rect, black) {
                out.push(format!("no ink where {:?} should be", line.text));
            }
        }
        if let Some(link) = l.link
            && !has_ink(px, built.width, link, black)
        {
            out.push("no ink where the link should be".to_owned());
        }
    }
    out
}

pub fn file_name(case: &Case, scale: u32, shapes_only: bool) -> String {
    let kind = if shapes_only { "-shapes" } else { "" };
    format!("panel-{}{kind}-{scale}pct.png", case.name)
}

pub fn write(path: &Path, px: &[u32], width: i32, height: i32) -> anyhow::Result<()> {
    let frame = BgraFrame {
        width: width as u32,
        height: height as u32,
        pixels: px.iter().flat_map(|p| p.to_le_bytes()).collect(),
    };
    crate::png_out::write_png(path, &frame)
}

/// The shapes of `built`, no words.
pub fn shapes(built: &Built) -> Vec<u32> {
    let mut px = vec![0; (built.width * built.height) as usize];
    let mut canvas = Canvas::new(&mut px, built.width, built.height);
    for op in &built.ops {
        canvas.shape(op);
    }
    px
}

pub fn run(options: &Options) -> Line {
    let scales = if options.scales.is_empty() {
        SCALES.to_vec()
    } else {
        options.scales.clone()
    };
    if let Some(bad) = scales.iter().find(|s| !SCALES.contains(s)) {
        return Line::new("panel-shot", false).with(
            "error",
            format!("no scale {bad}; scales are 100, 125, 150, 200"),
        );
    }
    let pal = Palette::DARK;
    let mut failures = Vec::new();
    let mut files = Vec::new();
    #[cfg(windows)]
    let real = !options.shapes_only;
    #[cfg(not(windows))]
    let real = false;
    for case in cases() {
        for &scale in &scales {
            let drawn: anyhow::Result<(Built, Vec<u32>)> = if real {
                #[cfg(windows)]
                {
                    win32::draw(&case, scale, &pal)
                }
                #[cfg(not(windows))]
                {
                    unreachable!("no GDI off Windows")
                }
            } else {
                let built = build(&case, scale, &Approx, &pal);
                let px = shapes(&built);
                Ok((built, px))
            };
            match drawn {
                Ok((built, px)) => {
                    for p in problems(&built, &px, real, &pal) {
                        failures.push(format!("{} at {scale}%: {p}", case.name));
                    }
                    let path = options.out.join(file_name(&case, scale, !real));
                    match write(&path, &px, built.width, built.height) {
                        Ok(()) => files.push(path.display().to_string()),
                        Err(e) => failures.push(format!("{}: {e:#}", path.display())),
                    }
                }
                Err(e) => failures.push(format!("{} at {scale}%: {e:#}", case.name)),
            }
        }
    }
    Line::new("panel-shot", failures.is_empty())
        .with("mode", if real { "gdi" } else { "shapes-only" })
        .with("pictures", files.len())
        .with("problems", failures)
        .with("files", files)
}

#[cfg(windows)]
mod win32 {
    use gliff_windows::strip_gdi::DcMeasure;
    use gliff_windows::strip_gdi::Fonts;
    use gliff_windows::strip_gdi::Surface;

    use super::Built;
    use super::Case;
    use super::Palette;
    use super::build;

    /// The panel drawn on the client's own surface, words and all.
    pub fn draw(case: &Case, scale: u32, pal: &Palette) -> anyhow::Result<(Built, Vec<u32>)> {
        let fonts = Fonts::new();
        let probe =
            Surface::new(None, 1, 1, pal.backdrop).ok_or_else(|| anyhow::anyhow!("no bitmap"))?;
        let built = build(
            case,
            scale,
            &DcMeasure {
                hdc: probe.dc(),
                fonts: &fonts,
            },
            pal,
        );
        let mut surface = Surface::new(None, built.width, built.height, pal.backdrop)
            .ok_or_else(|| anyhow::anyhow!("no bitmap"))?;
        surface.draw(&fonts, &built.ops);
        let px = surface.pixels().to_vec();
        Ok((built, px))
    }
}
