//! Draws the strip's shapes ([`Op`]) into a pixel buffer with smooth edges.
//! GDI has no anti-aliasing for shapes, so the window runs every shape here
//! and keeps GDI for text only. The code is pure: it works on any machine,
//! and tests look at the pixels.
//!
//! A pixel is `0x00RRGGBB`, which is the memory order of a BGRA DIB. A
//! shape's coverage of a pixel is a number from 0 to 1; the colour is
//! blended over what is there in that proportion.

use super::Rect;
use super::draw::Op;
use crate::palette::Rgb;

/// Samples per pixel side for shapes with angles.
const SUB: i32 = 8;

/// A buffer of pixels and the clip that limits drawing into it.
pub struct Canvas<'a> {
    px: &'a mut [u32],
    width: i32,
    height: i32,
    clip: Rect,
}

impl<'a> Canvas<'a> {
    /// `px` holds `width * height` pixels, row by row. A shorter slice is a
    /// bug of the caller; drawing outside the slice is skipped.
    pub fn new(px: &'a mut [u32], width: i32, height: i32) -> Self {
        Self {
            px,
            width,
            height,
            clip: Rect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
        }
    }

    /// Runs one operation. `Text` is not drawn here: the result is `false`
    /// for it and `true` for every other operation.
    pub fn shape(&mut self, op: &Op) -> bool {
        match op {
            Op::Rect { rect, radius, fill } => self.rect(*rect, *radius, *fill),
            Op::Ellipse { rect, fill, stroke } => {
                if let Some(fill) = fill {
                    self.ellipse(*rect, 0, *fill);
                }
                if let Some((color, width)) = stroke {
                    self.ellipse(*rect, *width, *color);
                }
            }
            Op::Arc {
                rect,
                start,
                sweep,
                width,
                color,
            } => self.wedge(*rect, *width, *start, *sweep, *color),
            Op::Pie {
                rect,
                start,
                sweep,
                color,
            } => self.wedge(*rect, 0, *start, *sweep, *color),
            Op::Line {
                from,
                to,
                width,
                color,
            } => self.line(*from, *to, *width, *color),
            Op::Clip(rect) => {
                self.clip = Rect {
                    left: rect.left.max(0),
                    top: rect.top.max(0),
                    right: rect.right.min(self.width),
                    bottom: rect.bottom.min(self.height),
                };
            }
            Op::Unclip => {
                self.clip = Rect {
                    left: 0,
                    top: 0,
                    right: self.width,
                    bottom: self.height,
                };
            }
            Op::Text { .. } => return false,
        }
        true
    }

    /// The pixels `area` covers, cut to the clip.
    fn span(&self, area: Rect) -> Rect {
        Rect {
            left: area.left.max(self.clip.left),
            top: area.top.max(self.clip.top),
            right: area.right.min(self.clip.right),
            bottom: area.bottom.min(self.clip.bottom),
        }
    }

    fn blend(&mut self, x: i32, y: i32, color: Rgb, coverage: f32) {
        if coverage <= 0.0 {
            return;
        }
        let Some(px) = self.px.get_mut((y * self.width + x) as usize) else {
            return;
        };
        if coverage >= 1.0 {
            *px = pack(color);
            return;
        }
        let mix = |top: u8, below: u32| -> u32 {
            (f32::from(top) * coverage + below as f32 * (1.0 - coverage)).round() as u32
        };
        *px = mix(color.0, *px >> 16 & 0xff) << 16
            | mix(color.1, *px >> 8 & 0xff) << 8
            | mix(color.2, *px & 0xff);
    }

    fn rect(&mut self, r: Rect, radius: i32, color: Rgb) {
        let area = self.span(r);
        if radius <= 0 {
            for y in area.top..area.bottom {
                for x in area.left..area.right {
                    self.blend(x, y, color, 1.0);
                }
            }
            return;
        }
        let (hx, hy) = (
            f64::from(r.width()) / 2.0,
            f64::from(r.bottom - r.top) / 2.0,
        );
        let (cx, cy) = (f64::from(r.left) + hx, f64::from(r.top) + hy);
        let radius = f64::from(radius).min(hx).min(hy);
        for y in area.top..area.bottom {
            for x in area.left..area.right {
                let qx = (f64::from(x) + 0.5 - cx).abs() - (hx - radius);
                let qy = (f64::from(y) + 0.5 - cy).abs() - (hy - radius);
                let outside = qx.max(0.0).hypot(qy.max(0.0));
                let d = outside + qx.max(qy).min(0.0) - radius;
                self.blend(x, y, color, edge(d));
            }
        }
    }

    /// The ellipse in `r`; `inset` 0 fills it, otherwise it is the ring that
    /// wide, inside the rectangle.
    fn ellipse(&mut self, r: Rect, inset: i32, color: Rgb) {
        let area = self.span(r);
        let (rx, ry) = (
            f64::from(r.width()) / 2.0,
            f64::from(r.bottom - r.top) / 2.0,
        );
        let (cx, cy) = (f64::from(r.left) + rx, f64::from(r.top) + ry);
        let width = f64::from(inset);
        let (ix, iy) = (rx - width, ry - width);
        for y in area.top..area.bottom {
            for x in area.left..area.right {
                let (dx, dy) = (f64::from(x) + 0.5 - cx, f64::from(y) + 0.5 - cy);
                let outer = ((dx / rx).hypot(dy / ry) - 1.0) * rx.min(ry);
                let mut cover = edge(outer);
                if inset > 0 && ix > 0.0 && iy > 0.0 {
                    let inner = ((dx / ix).hypot(dy / iy) - 1.0) * ix.min(iy);
                    cover *= 1.0 - edge(inner);
                }
                self.blend(x, y, color, cover);
            }
        }
    }

    /// A slice (`inset` 0) or an arc stroked `inset` wide inside `r`, from
    /// `start` degrees clockwise from three o'clock through `sweep`.
    fn wedge(&mut self, r: Rect, inset: i32, start: i32, sweep: i32, color: Rgb) {
        let area = self.span(r);
        let (rx, ry) = (
            f64::from(r.width()) / 2.0,
            f64::from(r.bottom - r.top) / 2.0,
        );
        let (cx, cy) = (f64::from(r.left) + rx, f64::from(r.top) + ry);
        let width = f64::from(inset);
        let (ix, iy) = (rx - width, ry - width);
        let inside = |px: f64, py: f64| {
            let (dx, dy) = (px - cx, py - cy);
            if (dx / rx).hypot(dy / ry) > 1.0 {
                return false;
            }
            if inset > 0 && ix > 0.0 && iy > 0.0 && (dx / ix).hypot(dy / iy) < 1.0 {
                return false;
            }
            let angle = dy.atan2(dx).to_degrees();
            (angle - f64::from(start)).rem_euclid(360.0) <= f64::from(sweep)
        };
        let step = 1.0 / f64::from(SUB);
        for y in area.top..area.bottom {
            for x in area.left..area.right {
                let mut hits = 0;
                for sy in 0..SUB {
                    for sx in 0..SUB {
                        let px = f64::from(x) + (f64::from(sx) + 0.5) * step;
                        let py = f64::from(y) + (f64::from(sy) + 0.5) * step;
                        hits += i32::from(inside(px, py));
                    }
                }
                self.blend(x, y, color, hits as f32 / (SUB * SUB) as f32);
            }
        }
    }

    /// A line with round ends. The points are on the pixel grid's corners,
    /// so an even width covers whole pixels.
    fn line(&mut self, from: (i32, i32), to: (i32, i32), width: i32, color: Rgb) {
        let half = f64::from(width) / 2.0;
        let reach = half.ceil() as i32 + 1;
        let area = self.span(Rect {
            left: from.0.min(to.0) - reach,
            top: from.1.min(to.1) - reach,
            right: from.0.max(to.0) + reach,
            bottom: from.1.max(to.1) + reach,
        });
        let (ax, ay) = (f64::from(from.0), f64::from(from.1));
        let (bx, by) = (f64::from(to.0) - ax, f64::from(to.1) - ay);
        let length_sq = bx * bx + by * by;
        for y in area.top..area.bottom {
            for x in area.left..area.right {
                let (px, py) = (f64::from(x) + 0.5 - ax, f64::from(y) + 0.5 - ay);
                let t = if length_sq == 0.0 {
                    0.0
                } else {
                    ((px * bx + py * by) / length_sq).clamp(0.0, 1.0)
                };
                let d = (px - t * bx).hypot(py - t * by) - half;
                self.blend(x, y, color, edge(d));
            }
        }
    }
}

/// Coverage of a pixel whose centre is `d` pixels outside a shape's edge:
/// 1 well inside, 0 well outside, a ramp one pixel wide across the edge.
fn edge(d: f64) -> f32 {
    (0.5 - d).clamp(0.0, 1.0) as f32
}

fn pack(c: Rgb) -> u32 {
    u32::from(c.0) << 16 | u32::from(c.1) << 8 | u32::from(c.2)
}

#[cfg(test)]
mod tests;
