//! What the strip looks like, as a list of drawing operations. The window
//! runs the list on a GDI surface; tests check the list as text, so the look is
//! checked on any machine without a bitmap.
//!
//! Everything is in device pixels, from [`Metrics`] and [`Layout`].

use super::Bar;
use super::DotState;
use super::Hit;
use super::Layout;
use super::Metrics;
use super::Rect;
use crate::palette::Palette;
use crate::palette::Rgb;
use crate::tabs::TabId;

/// One drawing operation. The window turns each into GDI calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// A filled rounded rectangle (`radius` 0: a plain one).
    Rect {
        rect: Rect,
        radius: i32,
        fill: Rgb,
    },
    /// An ellipse in `rect`, filled, stroked or both. The stroke is drawn
    /// inside the rectangle.
    Ellipse {
        rect: Rect,
        fill: Option<Rgb>,
        stroke: Option<(Rgb, i32)>,
    },
    /// An arc of the ellipse in `rect`: `start` degrees clockwise from
    /// three o'clock, `sweep` degrees clockwise, stroked inside the
    /// rectangle.
    Arc {
        rect: Rect,
        start: i32,
        sweep: i32,
        width: i32,
        color: Rgb,
    },
    /// A filled slice of the ellipse in `rect`, angles as for `Arc`.
    Pie {
        rect: Rect,
        start: i32,
        sweep: i32,
        color: Rgb,
    },
    Line {
        from: (i32, i32),
        to: (i32, i32),
        width: i32,
        color: Rgb,
    },
    /// One line of text of `size` device pixels, at the left of `rect` and
    /// centred vertically. It is never cut with an ellipsis: the layout gives
    /// it the room its whole width needs.
    Text {
        rect: Rect,
        text: String,
        size: i32,
        color: Rgb,
        bold: bool,
    },
    /// Drawing after this is cut to `rect`, until `Unclip`.
    Clip(Rect),
    Unclip,
}

/// What the drawing needs to know about one tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPaint {
    pub id: TabId,
    pub label: String,
    pub dot: DotState,
}

/// A tab being dragged to a new place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drag {
    pub id: TabId,
    /// The tab it would be dropped before; `None`: at the end.
    pub before: Option<TabId>,
}

/// What the address bar shows, when it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarPaint<'a> {
    /// What is typed. The Win32 edit field sits over it and draws the same
    /// text; the list is what a picture of the bar shows.
    pub text: &'a str,
    /// The edit field has the keyboard.
    pub focused: bool,
    /// The last Enter was refused.
    pub error: bool,
    /// The edit field is on screen over the bar's field and draws its own
    /// text and hint, so the strip draws none under it.
    pub edit_live: bool,
}

/// The hint in an empty edit field.
pub const PLACEHOLDER: &str = "user@host";

/// Everything the strip is drawn from.
pub struct Paint<'a> {
    pub metrics: &'a Metrics,
    pub palette: &'a Palette,
    pub layout: &'a Layout,
    /// The strip's width.
    pub width: i32,
    /// In the layout's order.
    pub tabs: &'a [TabPaint],
    pub shown: Option<TabId>,
    /// What the pointer is over.
    pub hover: Hit,
    pub drag: Option<Drag>,
    /// Set while the address bar is open.
    pub bar: Option<BarPaint<'a>>,
    /// The turning arc's step, counted in tenths of a second.
    pub phase: u32,
}

/// The whole strip.
pub fn draw(p: &Paint<'_>) -> Vec<Op> {
    let m = p.metrics;
    let pal = p.palette;
    let mut ops = vec![Op::Rect {
        rect: Rect {
            left: 0,
            top: 0,
            right: p.width,
            bottom: m.strip_height,
        },
        radius: 0,
        fill: pal.strip,
    }];

    ops.push(Op::Clip(p.layout.viewport));
    for (tab, boxed) in p.tabs.iter().zip(&p.layout.tabs) {
        if boxed.rect.right <= p.layout.viewport.left || boxed.rect.left >= p.layout.viewport.right
        {
            continue;
        }
        let dragged = p.drag.is_some_and(|d| d.id == tab.id);
        let on_tab = matches!(p.hover, Hit::Tab(id) | Hit::Slot(id) if id == tab.id);
        let tint = if p.shown == Some(tab.id) {
            Some(pal.tab_shown)
        } else if on_tab || dragged {
            Some(pal.tab_hover)
        } else {
            None
        };
        if let Some(fill) = tint {
            ops.push(Op::Rect {
                rect: boxed.rect,
                radius: m.radius,
                fill,
            });
        }
        ops.push(Op::Text {
            rect: boxed.label,
            text: tab.label.clone(),
            size: p.layout.text,
            color: if tab.dot == DotState::Stopped {
                pal.text_dim
            } else {
                pal.text
            },
            bold: true,
        });
        let centre = (
            (boxed.slot.left + boxed.slot.right) / 2,
            (boxed.slot.top + boxed.slot.bottom) / 2,
        );
        if p.hover == Hit::Slot(tab.id) {
            ops.extend(slot_button(boxed.slot, centre, m, pal));
        } else {
            ops.extend(dot(centre, m.dot, tab.dot, p.phase, pal));
        }
    }
    if let Some(drag) = p.drag {
        ops.extend(drop_marker(p, drag));
    }
    ops.push(Op::Unclip);

    match (p.bar, p.layout.bar) {
        (Some(paint), Some(bar)) => ops.extend(address_bar(p, &bar, paint)),
        _ => ops.extend(button(
            p.layout.add,
            p.hover == Hit::Add,
            Glyph::Plus,
            m,
            pal,
        )),
    }
    if let Some(chevron) = p.layout.chevron {
        ops.extend(button(
            chevron,
            p.hover == Hit::Chevron,
            Glyph::Chevron,
            m,
            pal,
        ));
    }
    ops.extend(button(
        p.layout.fullscreen,
        p.hover == Hit::Fullscreen,
        Glyph::Fullscreen,
        m,
        pal,
    ));
    ops
}

/// The width of the strokes the glyphs use.
fn stroke(m: &Metrics) -> i32 {
    (m.dpi as i32 * 3 / 2 + 48) / 96
}

/// The dot of a tab, a pure function of its state. `phase` turns the arc of a
/// connecting tab: one step is 36 degrees, ten steps a turn.
pub fn dot(centre: (i32, i32), d: i32, state: DotState, phase: u32, pal: &Palette) -> Vec<Op> {
    let rect = Rect {
        left: centre.0 - d / 2,
        top: centre.1 - d / 2,
        right: centre.0 - d / 2 + d,
        bottom: centre.1 - d / 2 + d,
    };
    let thin = (d / 5).max(1);
    match state {
        DotState::Stopped => vec![Op::Ellipse {
            rect,
            fill: None,
            stroke: Some((pal.ring, thin)),
        }],
        DotState::Connecting => vec![
            Op::Ellipse {
                rect,
                fill: None,
                stroke: Some((pal.ring, thin)),
            },
            Op::Arc {
                rect,
                start: (phase % 10) as i32 * 36,
                sweep: 90,
                width: thin + 1,
                color: pal.accent,
            },
        ],
        DotState::Live => vec![Op::Ellipse {
            rect,
            fill: Some(pal.ok),
            stroke: None,
        }],
        DotState::Reconnecting => vec![
            Op::Ellipse {
                rect,
                fill: None,
                stroke: Some((pal.warn, thin)),
            },
            // The left half, from nine o'clock round the top is 90 to 270
            // degrees: from six o'clock clockwise through nine to twelve.
            Op::Pie {
                rect,
                start: 90,
                sweep: 180,
                color: pal.warn,
            },
        ],
        DotState::Failed => {
            let inset = (d / 4).max(1);
            let cross = |a: (i32, i32), b: (i32, i32)| Op::Line {
                from: a,
                to: b,
                width: thin,
                color: pal.backdrop,
            };
            vec![
                Op::Ellipse {
                    rect,
                    fill: Some(pal.err),
                    stroke: None,
                },
                cross(
                    (rect.left + inset, rect.top + inset),
                    (rect.right - inset, rect.bottom - inset),
                ),
                cross(
                    (rect.right - inset, rect.top + inset),
                    (rect.left + inset, rect.bottom - inset),
                ),
            ]
        }
    }
}

/// The dot under the pointer: a round pad with a cross. A click on it stops
/// a live tab and forgets one that is not.
fn slot_button(slot: Rect, centre: (i32, i32), m: &Metrics, pal: &Palette) -> Vec<Op> {
    let pad = slot.width().min(slot.bottom - slot.top);
    let left = centre.0 - pad / 2;
    let top = centre.1 - pad / 2;
    let arm = m.dot / 2 + 1;
    let line = |a: (i32, i32), b: (i32, i32)| Op::Line {
        from: a,
        to: b,
        width: stroke(m),
        color: pal.text,
    };
    vec![
        Op::Ellipse {
            rect: Rect {
                left,
                top,
                right: left + pad,
                bottom: top + pad,
            },
            fill: Some(pal.tab_hover),
            stroke: None,
        },
        line(
            (centre.0 - arm, centre.1 - arm),
            (centre.0 + arm, centre.1 + arm),
        ),
        line(
            (centre.0 + arm, centre.1 - arm),
            (centre.0 - arm, centre.1 + arm),
        ),
    ]
}

#[derive(Clone, Copy)]
enum Glyph {
    Plus,
    Chevron,
    Fullscreen,
    /// The address bar's go arrow.
    Go,
}

/// A button of the strip's right side: a pill under the pointer, and a
/// glyph.
fn button(rect: Rect, hovered: bool, glyph: Glyph, m: &Metrics, pal: &Palette) -> Vec<Op> {
    let mut ops = Vec::new();
    if hovered {
        ops.push(Op::Rect {
            rect,
            radius: m.radius,
            fill: pal.tab_hover,
        });
    }
    ops.extend(glyph_in(rect, glyph, m, pal));
    ops
}

/// The address bar, drawn where the add button was: an outline, the text
/// field, the recent-hosts button and the go button.
///
/// The Win32 edit field covers the edit field and draws the same text, so the
/// list is complete without it: that is what the pictures show.
fn address_bar(p: &Paint<'_>, bar: &Bar, paint: BarPaint<'_>) -> Vec<Op> {
    let m = p.metrics;
    let pal = p.palette;
    let radius = m.radius * 3 / 4;
    let inset = |r: Rect| Rect {
        left: r.left,
        top: r.top + 1,
        right: r.right,
        bottom: r.bottom - 1,
    };
    let outline = if paint.error {
        pal.err
    } else if paint.focused {
        pal.accent
    } else {
        pal.field_border
    };
    let whole = p.layout.add;
    let mut ops = vec![
        Op::Rect {
            rect: whole,
            radius,
            fill: outline,
        },
        // The edit field runs under the go button, whose square left edge
        // covers the rounded end.
        Op::Rect {
            rect: Rect {
                left: whole.left + 1,
                right: bar.go.left + radius,
                ..inset(whole)
            },
            radius: radius - 1,
            fill: pal.field,
        },
    ];
    let go = Rect {
        right: bar.go.right - 1,
        ..inset(bar.go)
    };
    let go_fill = if p.hover == Hit::Go {
        pal.go_hover
    } else {
        pal.go
    };
    ops.push(Op::Rect {
        rect: Rect {
            right: go.left + go.width() / 2,
            ..go
        },
        radius: 0,
        fill: go_fill,
    });
    ops.push(Op::Rect {
        rect: go,
        radius: radius - 1,
        fill: go_fill,
    });
    // The seam between the edit field and the go button.
    ops.push(Op::Rect {
        rect: Rect {
            right: bar.go.left + 1,
            ..inset(bar.go)
        },
        radius: 0,
        fill: outline,
    });
    ops.extend(glyph_in(bar.go, Glyph::Go, m, pal));

    if p.hover == Hit::List {
        ops.push(Op::Rect {
            rect: Rect {
                left: bar.list.left + 1,
                top: bar.list.top + 2,
                right: bar.list.right - 1,
                bottom: bar.list.bottom - 2,
            },
            radius: radius - 1,
            fill: pal.tab_hover,
        });
    }
    ops.extend(glyph_in(bar.list, Glyph::Chevron, m, pal));

    let (text, color) = if paint.text.is_empty() {
        (PLACEHOLDER, pal.placeholder)
    } else {
        (paint.text, pal.text)
    };
    if paint.edit_live {
        return ops;
    }
    ops.push(Op::Clip(bar.edit));
    ops.push(Op::Text {
        rect: bar.edit,
        text: text.to_owned(),
        size: m.text,
        color,
        bold: false,
    });
    ops.push(Op::Unclip);
    ops
}

fn glyph_in(rect: Rect, glyph: Glyph, m: &Metrics, pal: &Palette) -> Vec<Op> {
    let mut ops = Vec::new();
    let (cx, cy) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
    let w = stroke(m);
    let arm = m.button / 5;
    let line = |from: (i32, i32), to: (i32, i32)| Op::Line {
        from,
        to,
        width: w,
        color: pal.text,
    };
    match glyph {
        Glyph::Plus => {
            ops.push(line((cx - arm, cy), (cx + arm, cy)));
            ops.push(line((cx, cy - arm), (cx, cy + arm)));
        }
        Glyph::Chevron => {
            let h = arm / 2 + 1;
            ops.push(line((cx - arm, cy - h), (cx, cy + h)));
            ops.push(line((cx, cy + h), (cx + arm, cy - h)));
        }
        Glyph::Go => {
            let h = arm + 1;
            let w = arm / 2 + 1;
            ops.push(line((cx - w, cy - h), (cx + w, cy)));
            ops.push(line((cx + w, cy), (cx - w, cy + h)));
        }
        Glyph::Fullscreen => {
            // Four corners of a square, each an L.
            let tick = (arm * 2 / 3).max(2);
            for (sx, sy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let corner = (cx + sx * arm, cy + sy * arm);
                ops.push(line(corner, (corner.0 - sx * tick, corner.1)));
                ops.push(line(corner, (corner.0, corner.1 - sy * tick)));
            }
        }
    }
    ops
}

/// The mark that shows where a dragged tab would land: a bar in the gap
/// before the tab it would go in front of, or after the last tab.
fn drop_marker(p: &Paint<'_>, drag: Drag) -> Option<Op> {
    let m = p.metrics;
    let others: Vec<_> = p.layout.tabs.iter().filter(|t| t.id != drag.id).collect();
    let x = match drag.before {
        Some(id) => others.iter().find(|t| t.id == id)?.rect.left - m.gap / 2,
        None => others.last()?.rect.right + m.gap / 2,
    };
    let width = (m.dpi as i32 * 2 / 96).max(2);
    let top = p.layout.viewport.top;
    Some(Op::Rect {
        rect: Rect {
            left: x - width / 2,
            top,
            right: x - width / 2 + width,
            bottom: p.layout.viewport.bottom,
        },
        radius: 0,
        fill: p.palette.accent,
    })
}

#[cfg(test)]
mod tests;
