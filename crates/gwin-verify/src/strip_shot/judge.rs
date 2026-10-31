//! What a drawn strip must satisfy, judged from its layout, draw list and
//! pixels. Pure: tests run it on shapes drawn by the rasteriser on any
//! machine, and the Windows run runs it on the real surface with the names.

use gliff_windows::palette::Palette;
use gliff_windows::palette::Rgb;
use gliff_windows::strip::DotState;
use gliff_windows::strip::Hit;
use gliff_windows::strip::Rect;
use gliff_windows::strip::draw::Op;

use super::scene::Built;
use super::scene::Scene;

/// The pixels of one picture, `0x00RRGGBB`, row by row.
pub struct Picture<'a> {
    pub px: &'a [u32],
    pub width: i32,
    pub height: i32,
    /// Whether names are drawn, so their ink can be looked for.
    pub text: bool,
}

impl Picture<'_> {
    fn at(&self, x: i32, y: i32) -> Option<Rgb> {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        let p = self.px[(y * self.width + x) as usize];
        Some(Rgb((p >> 16) as u8, (p >> 8) as u8, p as u8))
    }

    fn has(&self, r: Rect, color: Rgb) -> bool {
        (r.top..r.bottom).any(|y| (r.left..r.right).any(|x| self.at(x, y) == Some(color)))
    }
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}

/// Every way the picture falls short; empty when it is right.
pub fn problems(
    scene: &Scene,
    built: &Built,
    ops: &[Op],
    pic: &Picture,
    pal: &Palette,
) -> Vec<String> {
    let m = &built.metrics;
    let l = &built.layout;
    let mut out = Vec::new();
    let strip = Rect {
        left: 0,
        top: 0,
        right: built.width,
        bottom: m.strip_height,
    };
    if (pic.width, pic.height) != (built.width, m.strip_height) {
        out.push(format!(
            "the picture is {}x{}, the strip {}x{}",
            pic.width, pic.height, built.width, m.strip_height
        ));
    }
    if l.chevron.is_some() != scene.chevron {
        out.push(format!(
            "the chevron is {}, expected {}",
            if l.chevron.is_some() {
                "shown"
            } else {
                "absent"
            },
            if scene.chevron { "shown" } else { "absent" }
        ));
    }

    if l.bar.is_some() != scene.bar.is_some() {
        out.push(format!(
            "the address bar is {}, expected {}",
            if l.bar.is_some() { "open" } else { "closed" },
            if scene.bar.is_some() {
                "open"
            } else {
                "closed"
            }
        ));
    }

    // The fixed parts: inside the strip, apart from each other. The
    // address bar's three parts sit inside `add`, so they are checked
    // against it.
    let mut parts = vec![("add", l.add), ("fullscreen", l.fullscreen)];
    if let Some(bar) = &l.bar {
        for (name, r) in [("edit field", bar.edit), ("list", bar.list), ("go", bar.go)] {
            if !inside(r, l.add) {
                out.push(format!("the address bar's {name} {r:?} leaves its bar"));
            }
        }
        if overlap(bar.edit, bar.list) || overlap(bar.list, bar.go) || overlap(bar.edit, bar.go) {
            out.push("the address bar's parts overlap".to_owned());
        }
    }
    if let Some(chevron) = l.chevron {
        parts.push(("chevron", chevron));
    }
    for (name, r) in &parts {
        if !inside(*r, strip) {
            out.push(format!("{name} {r:?} leaves the strip"));
        }
        if overlap(*r, l.viewport) {
            out.push(format!("{name} overlaps the tabs' area"));
        }
    }
    for (i, (a, ra)) in parts.iter().enumerate() {
        for (b, rb) in &parts[i + 1..] {
            if overlap(*ra, *rb) {
                out.push(format!("{a} overlaps {b}"));
            }
        }
    }

    // The tabs.
    let minimum_dot = 10 * m.dpi as i32 / 96 - 1;
    for (i, tab) in l.tabs.iter().enumerate() {
        for other in &l.tabs[i + 1..] {
            if overlap(tab.rect, other.rect) {
                out.push(format!("tab {} overlaps tab {}", i + 1, i + 2));
            }
        }
        // The name is complete: the full address, in the layout's font, in
        // a place as wide as it is.
        let full = &built.paints[i].label;
        let name = ops.iter().find_map(|op| match op {
            Op::Text {
                rect, text, size, ..
            } if *rect == tab.label => Some((text, *size)),
            _ => None,
        });
        if !inside(tab.label, tab.rect) || tab.label.width() < built.text_widths[i] {
            out.push(format!(
                "tab {}: the name is cut: {} px of room for {} px",
                i + 1,
                tab.label.width(),
                built.text_widths[i]
            ));
        }
        if let Some((text, size)) = name {
            if text != full || text.contains('\u{2026}') || text.contains("...") {
                out.push(format!("tab {}: the name is shortened to {text:?}", i + 1));
            }
            if size != l.text {
                out.push(format!(
                    "tab {}: the name is in a {size} px font, the strip's is {}",
                    i + 1,
                    l.text
                ));
            }
        }
        let whole = inside(tab.rect, l.viewport);
        if l.chevron.is_none() && !whole {
            out.push(format!(
                "tab {} is cut by the strip with nothing to scroll",
                i + 1
            ));
        }
        if !whole {
            continue;
        }
        if !inside(tab.rect, strip) || !inside(tab.slot, tab.rect) {
            out.push(format!("tab {} or its dot's slot leaves its place", i + 1));
        }
        if tab.label.right > tab.slot.left {
            out.push(format!("tab {}: the name runs into the dot's slot", i + 1));
        }
        let dot = ops.iter().find_map(|op| match op {
            Op::Ellipse { rect, .. } if inside(*rect, tab.slot) => Some(*rect),
            _ => None,
        });
        let Some(dot) = dot else {
            if built.hover != Hit::Slot(tab.id) {
                out.push(format!("tab {} draws no dot", i + 1));
            }
            continue;
        };
        if dot.width() < minimum_dot {
            out.push(format!(
                "tab {}: the dot is {} px, under {minimum_dot} px at {} dpi",
                i + 1,
                dot.width(),
                m.dpi
            ));
        }
        let state = built.paints[i].dot;
        let centre = ((dot.left + dot.right) / 2, (dot.top + dot.bottom) / 2);
        let here = pic.at(centre.0, centre.1);
        let seen = |color: Rgb| pic.has(dot, color);
        let ok = match state {
            DotState::Live => here == Some(pal.ok),
            DotState::Failed => {
                here == Some(pal.backdrop)
                    && pic.at(centre.0 + dot.width() / 2 - 2, centre.1) == Some(pal.err)
            }
            DotState::Connecting => seen(pal.accent) && seen(pal.ring),
            DotState::Reconnecting => seen(pal.warn),
            DotState::Stopped => seen(pal.ring) && here != Some(pal.ring),
        };
        if !ok && built.hover != Hit::Slot(tab.id) {
            out.push(format!("tab {}: the {state:?} dot does not show", i + 1));
        }
        if pic.text {
            let label = tab.label;
            let background = pic.at(label.left, label.top + 1);
            let inked = (label.top..label.bottom)
                .any(|y| (label.left..label.right).any(|x| pic.at(x, y) != background));
            if !inked {
                out.push(format!("tab {}: no name is drawn", i + 1));
            }
            // The room's last column stays clear: ink there is a name
            // running on, or cut at, the edge.
            let edge = label.right - 1;
            if (label.top..label.bottom).any(|y| pic.at(edge, y) != background) {
                out.push(format!(
                    "tab {}: the name reaches the edge of its room",
                    i + 1
                ));
            }
        }
    }
    out
}
