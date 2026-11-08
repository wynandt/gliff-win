//! Device-pixel text layout. It has no input target or video geometry effect.

use crate::palette::Rgb;
use crate::strip::draw::Op;
use crate::strip::{Measure, Rect};

pub struct OverlayLayout {
    pub size: (u32, u32),
    pub ops: Vec<Op>,
}

pub fn layout(
    text: &str,
    dpi: u32,
    available: (u32, u32),
    measure: &dyn Measure,
) -> Option<OverlayLayout> {
    let px = |n: i32| (n * dpi.clamp(48, 768) as i32 + 48) / 96;
    let pad = px(6);
    let font = px(13);
    let line_height = px(20);
    let width = i32::try_from(available.0).ok()?;
    let height = i32::try_from(available.1).ok()?;
    let room = width - 2 * pad;
    if text.is_empty() || room < font || height < line_height + 2 * pad {
        return None;
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for field in text.split("  ") {
        let joined = if line.is_empty() {
            field.to_owned()
        } else {
            format!("{line}  {field}")
        };
        if measure.text_width(&joined, font) <= room {
            line = joined;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        for ch in field.chars() {
            let mut next = line.clone();
            next.push(ch);
            if !line.is_empty() && measure.text_width(&next, font) > room {
                lines.push(std::mem::take(&mut line));
            }
            line.push(ch);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    let max_lines = (height - 2 * pad) / line_height;
    lines.truncate(max_lines as usize);
    let used = lines
        .iter()
        .map(|s| measure.text_width(s, font))
        .max()
        .unwrap_or(0);
    let size = (
        (used + 2 * pad).min(width),
        lines.len() as i32 * line_height + 2 * pad,
    );
    let mut ops = vec![Op::Rect {
        rect: Rect {
            left: 0,
            top: 0,
            right: size.0,
            bottom: size.1,
        },
        radius: 0,
        fill: Rgb(0, 0, 0),
    }];
    for (i, text) in lines.into_iter().enumerate() {
        let top = pad + i as i32 * line_height;
        ops.push(Op::Text {
            rect: Rect {
                left: pad,
                top,
                right: size.0 - pad,
                bottom: top + line_height,
            },
            text,
            size: font,
            color: Rgb(255, 255, 255),
            bold: false,
        });
    }
    Some(OverlayLayout {
        size: (size.0 as u32, size.1 as u32),
        ops,
    })
}

/// White text coverage over 60%-black, stored as premultiplied BGRA.
#[cfg(any(windows, test))]
pub(crate) fn premultiply_mask(mask: &[u32]) -> Vec<u32> {
    mask.iter()
        .map(|pixel| {
            let [b, g, r, _] = pixel.to_le_bytes();
            let coverage = u32::from(r.max(g).max(b));
            let alpha = 153 + (102 * coverage + 127) / 255;
            (alpha << 24) | (coverage << 16) | (coverage << 8) | coverage
        })
        .collect()
}

#[cfg(windows)]
pub(crate) struct Bitmap {
    pub size: (u32, u32),
    pub premultiplied_bgra: Vec<u32>,
}

#[cfg(windows)]
pub(crate) fn raster(text: &str, dpi: u32, available: (u32, u32)) -> Option<Bitmap> {
    use crate::strip_gdi::{DcMeasure, Fonts, Surface};
    let fonts = Fonts::monospace();
    let measure_surface = Surface::new(None, 1, 1, Rgb(0, 0, 0))?;
    let measure = DcMeasure {
        hdc: measure_surface.dc(),
        fonts: &fonts,
    };
    let l = layout(text, dpi, available, &measure)?;
    let mut bitmap = Surface::new(None, l.size.0 as i32, l.size.1 as i32, Rgb(0, 0, 0))?;
    bitmap.draw(&fonts, &l.ops);
    Some(Bitmap {
        size: l.size,
        premultiplied_bgra: premultiply_mask(bitmap.pixels()),
    })
}

#[cfg(test)]
mod tests {
    use super::layout;
    use crate::strip::Measure;
    use crate::strip::draw::Op;
    struct Mono;
    impl Measure for Mono {
        fn text_width(&self, text: &str, size: i32) -> i32 {
            text.chars().count() as i32 * size * 3 / 5
        }
    }
    #[test]
    fn layout_wraps_all_fields_at_four_dpis_without_exceeding_bounds() {
        let text = "GPU→CPU  60 fps  12.3 Mbit/s  decode 5.0 ms  1920x1080 → 3840x2160 @60";
        for dpi in [96, 120, 144, 192] {
            for width in [160, 320, 1200] {
                let l = layout(text, dpi, (width, 900), &Mono).expect("fits");
                assert!(l.size.0 <= width && l.size.1 <= 900);
                let mut drawn = String::new();
                for op in &l.ops {
                    if let Op::Text {
                        rect, text, size, ..
                    } = op
                    {
                        assert!(rect.left >= 0 && rect.right <= l.size.0 as i32);
                        assert!(Mono.text_width(text, *size) <= rect.width());
                        drawn.push_str(text);
                    }
                }
                assert_eq!(
                    drawn.split_whitespace().collect::<String>(),
                    text.split_whitespace().collect::<String>()
                );
            }
        }
    }
    #[test]
    fn mask_is_white_text_over_black_not_the_strip_palette() {
        let l = layout("CPU  30 fps", 96, (800, 600), &Mono).expect("layout");
        assert!(matches!(
            l.ops[0],
            Op::Rect {
                fill: crate::palette::Rgb(0, 0, 0),
                ..
            }
        ));
        assert!(l.ops.iter().any(|op| matches!(
            op,
            Op::Text {
                color: crate::palette::Rgb(255, 255, 255),
                ..
            }
        )));
    }
    #[test]
    fn premultiplied_bgra_keeps_background_translucent_and_solid_text_opaque() {
        let pixels = super::premultiply_mask(&[0xaa000000, 0x00ffffff, 0x00808080, 0xff0000ff]);
        assert_eq!(pixels[0].to_le_bytes(), [0, 0, 0, 153]);
        assert_eq!(pixels[1].to_le_bytes(), [255, 255, 255, 255]);
        assert_eq!(pixels[2].to_le_bytes(), [128, 128, 128, 204]);
        assert_eq!(pixels[3].to_le_bytes(), [255, 255, 255, 255]);
        for c in 0..=255u32 {
            let pixel = super::premultiply_mask(&[c * 0x010101])[0].to_le_bytes();
            assert_eq!(&pixel[..3], &[c as u8; 3]);
            assert!(pixel[0] <= pixel[3]);
        }
    }

    #[test]
    fn source_over_changes_with_video_without_dimming_solid_glyphs() {
        let composite = |source: u32, video: [u8; 3]| {
            let [b, g, r, a] = source.to_le_bytes();
            [b, g, r]
                .map(|v| v as f64)
                .into_iter()
                .zip(video)
                .map(|(s, d)| (s + f64::from(d) * (1.0 - f64::from(a) / 255.0)).round() as u8)
                .collect::<Vec<_>>()
        };
        let pixels = super::premultiply_mask(&[0, 0xffffff, 0x808080]);
        assert_eq!(composite(pixels[0], [40, 80, 120]), [16, 32, 48]);
        assert_eq!(composite(pixels[0], [200, 150, 100]), [80, 60, 40]);
        assert_eq!(composite(pixels[1], [40, 80, 120]), [255, 255, 255]);
        assert_eq!(composite(pixels[2], [200, 200, 200]), [168, 168, 168]);
    }
    #[test]
    fn empty_and_unusable_areas_allocate_nothing() {
        assert!(layout("", 96, (800, 600), &Mono).is_none());
        assert!(layout("CPU", 192, (10, 10), &Mono).is_none());
        assert!(layout("CPU", 96, (0, 0), &Mono).is_none());
    }
}
