use super::BarPaint;
use super::DotState;
use super::Drag;
use super::Op;
use super::PLACEHOLDER;
use super::Paint;
use super::TabPaint;
use super::dot;
use super::draw;
use crate::palette::Palette;
use crate::palette::Rgb;
use crate::strip::Hit;
use crate::strip::Layout;
use crate::strip::Measure;
use crate::strip::Metrics;
use crate::strip::Rect;
use crate::strip::TabItem;
use crate::strip::layout;
use crate::tabs::TabId;
use crate::tabs::Tabs;

struct Fixed;

impl Measure for Fixed {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        text.chars().count() as i32 * size / 2
    }
}

const PAL: Palette = Palette::DARK;
const DPIS: [u32; 4] = [96, 120, 144, 192];

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn r(r: Rect) -> String {
    format!("{},{},{},{}", r.left, r.top, r.right, r.bottom)
}

/// One line per operation: the golden text.
fn render(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        let line = match op {
            Op::Rect { rect, radius, fill } => {
                format!("rect {} r{} {}", r(*rect), radius, hex(*fill))
            }
            Op::Ellipse { rect, fill, stroke } => format!(
                "ellipse {} fill {} stroke {}",
                r(*rect),
                fill.map_or("-".into(), hex),
                stroke.map_or("-".into(), |(c, w)| format!("{}/{}", hex(c), w)),
            ),
            Op::Arc {
                rect,
                start,
                sweep,
                width,
                color,
            } => {
                format!(
                    "arc {} {}+{} w{} {}",
                    r(*rect),
                    start,
                    sweep,
                    width,
                    hex(*color)
                )
            }
            Op::Pie {
                rect,
                start,
                sweep,
                color,
            } => {
                format!("pie {} {}+{} {}", r(*rect), start, sweep, hex(*color))
            }
            Op::Line {
                from,
                to,
                width,
                color,
            } => format!(
                "line {},{} {},{} w{} {}",
                from.0,
                from.1,
                to.0,
                to.1,
                width,
                hex(*color)
            ),
            Op::Text {
                rect,
                text,
                size,
                color,
                bold,
            } => {
                format!(
                    "text {} {:?} {}px {}{}",
                    r(*rect),
                    text,
                    size,
                    hex(*color),
                    if *bold { " bold" } else { "" }
                )
            }
            Op::Clip(rect) => format!("clip {}", r(*rect)),
            Op::Unclip => "unclip".to_string(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

struct Scene {
    ids: Vec<TabId>,
    paint: Vec<TabPaint>,
    items: Vec<TabItem>,
}

/// Tabs named `host-1`..; `dots` gives each one's dot.
fn scene(dots: &[DotState]) -> Scene {
    let mut tabs = Tabs::new();
    for i in 1..=dots.len() {
        tabs.open(&format!("u@host-{i}").parse().expect("a host"));
    }
    let ids: Vec<TabId> = tabs.iter().map(|t| t.id()).collect();
    let paint = ids
        .iter()
        .zip(dots)
        .enumerate()
        .map(|(i, (id, dot))| TabPaint {
            id: *id,
            label: format!("u@host-{}", i + 1),
            dot: *dot,
        })
        .collect();
    let items = ids
        .iter()
        .enumerate()
        .map(|(i, id)| TabItem {
            id: *id,
            label: format!("u@host-{}", i + 1),
        })
        .collect();
    Scene { ids, paint, items }
}

struct Frame {
    metrics: Metrics,
    layout: Layout,
    width: i32,
}

fn frame(s: &Scene, dpi: u32, width: i32, scroll: i32) -> Frame {
    let metrics = Metrics::at(dpi);
    let layout = layout(&metrics, width, &s.items, scroll, false, &Fixed);
    Frame {
        metrics,
        layout,
        width,
    }
}

fn ops(
    s: &Scene,
    f: &Frame,
    shown: Option<TabId>,
    hover: Hit,
    drag: Option<Drag>,
    phase: u32,
) -> Vec<Op> {
    draw(&Paint {
        metrics: &f.metrics,
        palette: &PAL,
        layout: &f.layout,
        width: f.width,
        tabs: &s.paint,
        shown,
        hover,
        drag,
        bar: None,
        phase,
    })
}

#[test]
fn each_dot_state_has_its_own_shape() {
    let golden = [
        (
            DotState::Stopped,
            "ellipse 7,7,13,13 fill - stroke #9a9a9a/1\n",
        ),
        (
            DotState::Connecting,
            "ellipse 7,7,13,13 fill - stroke #9a9a9a/1\n\
             arc 7,7,13,13 108+90 w2 #78aeed\n",
        ),
        (DotState::Live, "ellipse 7,7,13,13 fill #8ff0a4 stroke -\n"),
        (
            DotState::Reconnecting,
            "ellipse 7,7,13,13 fill - stroke #f8e45c/1\n\
             pie 7,7,13,13 90+180 #f8e45c\n",
        ),
        (
            DotState::Failed,
            "ellipse 7,7,13,13 fill #ff7b63 stroke -\n\
             line 8,8 12,12 w1 #202020\n\
             line 12,8 8,12 w1 #202020\n",
        ),
    ];
    for (state, text) in golden {
        assert_eq!(render(&dot((10, 10), 6, state, 3, &PAL)), text, "{state:?}");
    }
}

#[test]
fn the_background_comes_first_and_tabs_are_cut_to_the_viewport() {
    let s = scene(&[DotState::Live, DotState::Stopped]);
    let f = frame(&s, 96, 600, 0);
    let list = ops(&s, &f, None, Hit::Nothing, None, 0);
    assert_eq!(
        list[0],
        Op::Rect {
            rect: Rect {
                left: 0,
                top: 0,
                right: 600,
                bottom: 32
            },
            radius: 0,
            fill: PAL.strip,
        }
    );
    assert_eq!(list[1], Op::Clip(f.layout.viewport));
    let unclip = list.iter().position(|o| *o == Op::Unclip).expect("unclip");
    // Only the buttons come after the tabs.
    assert!(
        list[unclip + 1..]
            .iter()
            .all(|o| matches!(o, Op::Line { .. } | Op::Rect { .. }))
    );
}

#[test]
fn the_shown_tab_is_lit_and_a_hovered_tab_is_lit_less() {
    let s = scene(&[DotState::Live, DotState::Live, DotState::Live]);
    let f = frame(&s, 96, 600, 0);
    let fills = |list: &[Op]| -> Vec<(Rect, Rgb)> {
        list.iter()
            .filter_map(|o| match o {
                Op::Rect {
                    rect,
                    radius: 8,
                    fill,
                } => Some((*rect, *fill)),
                _ => None,
            })
            .collect()
    };
    let list = ops(&s, &f, Some(s.ids[0]), Hit::Tab(s.ids[1]), None, 0);
    assert_eq!(
        fills(&list),
        [
            (f.layout.tabs[0].rect, PAL.tab_shown),
            (f.layout.tabs[1].rect, PAL.tab_hover),
        ]
    );
    // The pointer on the dot slot lights the tab too; on the shown tab it
    // adds nothing.
    let list = ops(&s, &f, Some(s.ids[0]), Hit::Slot(s.ids[0]), None, 0);
    assert_eq!(fills(&list), [(f.layout.tabs[0].rect, PAL.tab_shown)]);
    let list = ops(&s, &f, Some(s.ids[0]), Hit::Slot(s.ids[2]), None, 0);
    assert_eq!(fills(&list)[1], (f.layout.tabs[2].rect, PAL.tab_hover));
}

#[test]
fn a_stopped_tab_has_a_dim_name() {
    let s = scene(&[DotState::Stopped, DotState::Live]);
    let f = frame(&s, 96, 600, 0);
    let colours: Vec<(String, Rgb, bool)> = ops(&s, &f, None, Hit::Nothing, None, 0)
        .into_iter()
        .filter_map(|o| match o {
            Op::Text {
                text, color, bold, ..
            } => Some((text, color, bold)),
            _ => None,
        })
        .collect();
    assert_eq!(
        colours,
        [
            ("u@host-1".to_string(), PAL.text_dim, true),
            ("u@host-2".to_string(), PAL.text, true),
        ]
    );
}

#[test]
fn under_the_pointer_the_dot_becomes_a_cross_button() {
    let s = scene(&[DotState::Live]);
    let f = frame(&s, 96, 600, 0);
    let slot = f.layout.tabs[0].slot;
    let plain = ops(&s, &f, None, Hit::Tab(s.ids[0]), None, 0);
    let over = ops(&s, &f, None, Hit::Slot(s.ids[0]), None, 0);
    let green = |list: &[Op]| {
        list.iter()
            .any(|o| matches!(o, Op::Ellipse { fill: Some(c), .. } if *c == PAL.ok))
    };
    assert!(green(&plain));
    assert!(!green(&over));
    let pad = over
        .iter()
        .find_map(|o| match o {
            Op::Ellipse {
                rect,
                fill: Some(c),
                ..
            } if *c == PAL.tab_hover => Some(*rect),
            _ => None,
        })
        .expect("a pad");
    // The pad sits inside the slot.
    assert!(pad.left >= slot.left && pad.right <= slot.right);
    assert!(pad.top >= slot.top && pad.bottom <= slot.bottom);
    let crosses = over
        .iter()
        .filter(|o| matches!(o, Op::Line { color, .. } if *color == PAL.text))
        .count();
    // Two strokes of the cross, plus the two glyph strokes of the add button
    // and the four strokes... the full-screen glyph has eight.
    assert_eq!(crosses, 2 + 2 + 8);
}

#[test]
fn the_drop_marker_sits_in_the_gap_it_would_drop_into() {
    let s = scene(&[DotState::Live, DotState::Live, DotState::Live]);
    let f = frame(&s, 96, 600, 0);
    let marker = |drag: Option<Drag>| -> Option<Rect> {
        ops(&s, &f, None, Hit::Nothing, drag, 0)
            .into_iter()
            .find_map(|o| match o {
                Op::Rect {
                    rect,
                    radius: 0,
                    fill,
                } if fill == PAL.accent => Some(rect),
                _ => None,
            })
    };
    assert_eq!(marker(None), None);
    let tabs = &f.layout.tabs;
    // Drag the first tab before the third: the bar is in the gap before the
    // third tab.
    let bar = marker(Some(Drag {
        id: s.ids[0],
        before: Some(s.ids[2]),
    }))
    .expect("a bar");
    assert!(
        bar.left >= tabs[1].rect.right && bar.right <= tabs[2].rect.left,
        "{bar:?}"
    );
    assert_eq!(
        (bar.top, bar.bottom),
        (f.layout.viewport.top, f.layout.viewport.bottom)
    );
    // Drag the first to the end: after the last tab.
    let bar = marker(Some(Drag {
        id: s.ids[0],
        before: None,
    }))
    .expect("a bar");
    assert!(bar.left >= tabs[2].rect.right, "{bar:?}");
    // The dragged tab is tinted as if hovered.
    let list = ops(
        &s,
        &f,
        None,
        Hit::Nothing,
        Some(Drag {
            id: s.ids[0],
            before: None,
        }),
        0,
    );
    assert!(list.contains(&Op::Rect {
        rect: tabs[0].rect,
        radius: 8,
        fill: PAL.tab_hover
    }));
}

#[test]
fn the_chevron_is_drawn_only_when_the_tabs_do_not_fit() {
    let few = scene(&[DotState::Live, DotState::Live]);
    let many = scene(&[DotState::Live; 12]);
    let glyph_lines = |s: &Scene, width| {
        let f = frame(s, 96, width, 0);
        ops(s, &f, None, Hit::Nothing, None, 0)
            .iter()
            .filter(|o| matches!(o, Op::Line { color, .. } if *color == PAL.text))
            .count()
    };
    // Add (2) and full screen (8); with the chevron, 2 more.
    assert_eq!(glyph_lines(&few, 800), 10);
    assert_eq!(glyph_lines(&many, 800), 12);
}

#[test]
fn a_tab_scrolled_out_of_view_is_not_drawn() {
    let s = scene(&[DotState::Live; 12]);
    let f = frame(&s, 96, 800, 0);
    let all = ops(&s, &f, None, Hit::Nothing, None, 0);
    let f2 = frame(&s, 96, 800, f.layout.max_scroll);
    assert!(f2.layout.scroll > 0);
    let scrolled = ops(&s, &f2, None, Hit::Nothing, None, 0);
    let names = |list: &[Op]| -> Vec<String> {
        list.iter()
            .filter_map(|o| match o {
                Op::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    assert!(names(&all).contains(&"u@host-1".to_string()));
    assert!(!names(&scrolled).contains(&"u@host-1".to_string()));
    assert!(names(&scrolled).contains(&"u@host-12".to_string()));
}

#[test]
fn nothing_is_drawn_outside_the_strip_at_any_scale() {
    let dots = [
        DotState::Live,
        DotState::Connecting,
        DotState::Reconnecting,
        DotState::Failed,
        DotState::Stopped,
    ];
    for dpi in DPIS {
        for count in [1, 5, 14] {
            let s = scene(&dots.iter().copied().cycle().take(count).collect::<Vec<_>>());
            let f = frame(&s, dpi, 900, 0);
            let all = [
                Hit::Nothing,
                Hit::Add,
                Hit::Fullscreen,
                Hit::Chevron,
                Hit::Tab(s.ids[0]),
                Hit::Slot(s.ids[0]),
            ];
            for hover in all {
                let drag = Some(Drag {
                    id: s.ids[0],
                    before: s.ids.get(1).copied(),
                });
                for op in ops(&s, &f, Some(s.ids[0]), hover, drag, 4) {
                    let rects: Vec<Rect> = match op {
                        Op::Rect { rect, .. }
                        | Op::Ellipse { rect, .. }
                        | Op::Arc { rect, .. }
                        | Op::Pie { rect, .. }
                        | Op::Text { rect, .. } => vec![rect],
                        Op::Line { from, to, .. } => vec![
                            Rect {
                                left: from.0,
                                top: from.1,
                                right: from.0 + 1,
                                bottom: from.1 + 1,
                            },
                            Rect {
                                left: to.0,
                                top: to.1,
                                right: to.0 + 1,
                                bottom: to.1 + 1,
                            },
                        ],
                        Op::Clip(_) | Op::Unclip => vec![],
                    };
                    for rect in rects {
                        assert!(
                            rect.left >= 0
                                && rect.top >= 0
                                && rect.right <= 900
                                && rect.bottom <= f.metrics.strip_height,
                            "{op:?} at {dpi} dpi, {count} tabs"
                        );
                    }
                }
            }
        }
    }
}

mod bar {
    use super::*;

    fn barred(s: &Scene, dpi: u32, width: i32, hover: Hit, bar: BarPaint<'_>) -> (Frame, Vec<Op>) {
        let metrics = Metrics::at(dpi);
        let layout = layout(&metrics, width, &s.items, 0, true, &Fixed);
        let f = Frame {
            metrics,
            layout,
            width,
        };
        let list = draw(&Paint {
            metrics: &f.metrics,
            palette: &PAL,
            layout: &f.layout,
            width,
            tabs: &s.paint,
            shown: s.ids.first().copied(),
            hover,
            drag: None,
            bar: Some(bar),
            phase: 0,
        });
        (f, list)
    }

    const EMPTY: BarPaint<'static> = BarPaint {
        text: "",
        focused: true,
        error: false,
        edit_live: false,
    };

    fn texts(list: &[Op]) -> Vec<(&str, Rgb)> {
        list.iter()
            .filter_map(|op| match op {
                Op::Text { text, color, .. } => Some((text.as_str(), *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn no_text_is_drawn_under_the_live_edit_field() {
        // The edit field draws its own text and hint. A second copy under it,
        // repainted by the strip on every pass, is what made the text blink.
        let s = scene(&[DotState::Live]);
        for text in ["", "me@host", "me@a-very-long-host-name.example.org"] {
            let bar = BarPaint {
                text,
                edit_live: true,
                ..EMPTY
            };
            let (f, list) = barred(&s, 96, 900, Hit::Nothing, bar);
            let field = f.layout.bar.expect("the bar is open").edit;
            let under: Vec<_> = list
                .iter()
                .filter(|op| {
                    matches!(op, Op::Text { rect, .. } if rect.left >= field.left
                    && rect.right <= field.right && rect.top >= field.top
                    && rect.bottom <= field.bottom)
                })
                .collect();
            assert!(
                under.is_empty(),
                "text {text:?} drawn under the field: {under:?}"
            );
        }
    }

    #[test]
    fn an_empty_bar_shows_the_hint_in_the_dim_colour_and_no_plus() {
        let s = scene(&[DotState::Live]);
        let (f, list) = barred(&s, 96, 900, Hit::Nothing, EMPTY);
        assert!(texts(&list).contains(&(PLACEHOLDER, PAL.placeholder)));
        let add = f.layout.add;
        let in_add = |op: &Op| match op {
            Op::Line { from, to, .. } => {
                let inside = |p: &(i32, i32)| add.contains(p.0, p.1);
                inside(from) && inside(to)
            }
            _ => false,
        };
        // The bar draws a chevron and an arrow, never the plus: a plus is
        // one horizontal and one vertical stroke through a centre.
        let bar = f.layout.bar.expect("open");
        let centre = (bar.go.left + bar.go.right) / 2;
        assert!(
            !list.iter().any(|op| matches!(op, Op::Line { from, to, .. }
                if in_add(op) && from.0 == to.0 && from.0 == centre)),
            "no vertical stroke through the go button's middle"
        );
    }

    #[test]
    fn typed_text_replaces_the_hint_and_is_clipped_to_the_text_box() {
        let s = scene(&[DotState::Live]);
        let bar = BarPaint {
            text: "me@a-very-long-host-name.example.org",
            ..EMPTY
        };
        let (f, list) = barred(&s, 96, 900, Hit::Nothing, bar);
        let found = texts(&list);
        assert!(found.contains(&(bar.text, PAL.text)));
        assert!(!found.iter().any(|(t, _)| *t == PLACEHOLDER));
        let edit = f.layout.bar.expect("open").edit;
        let at = list
            .iter()
            .position(|op| matches!(op, Op::Text { text, .. } if text == bar.text))
            .expect("the text");
        assert_eq!(list[at - 1], Op::Clip(edit), "clipped before");
        assert_eq!(list[at + 1], Op::Unclip, "and released after");
    }

    /// The outline is the first rounded rectangle of the bar, over the
    /// whole bar.
    fn outline(f: &Frame, list: &[Op]) -> Rgb {
        list.iter()
            .find_map(|op| match op {
                Op::Rect { rect, fill, .. } if *rect == f.layout.add => Some(*fill),
                _ => None,
            })
            .expect("the outline")
    }

    #[test]
    fn the_outline_is_dim_then_blue_with_the_keyboard_then_red_when_refused() {
        let s = scene(&[DotState::Live]);
        let colour = |focused, error| {
            let (f, list) = barred(
                &s,
                96,
                900,
                Hit::Nothing,
                BarPaint {
                    text: "x",
                    focused,
                    error,
                    edit_live: false,
                },
            );
            outline(&f, &list)
        };
        assert_eq!(colour(false, false), PAL.field_border);
        assert_eq!(colour(true, false), PAL.accent);
        assert_eq!(colour(true, true), PAL.err);
        assert_eq!(colour(false, true), PAL.err);
    }

    #[test]
    fn the_go_button_lights_under_the_pointer() {
        let s = scene(&[DotState::Live]);
        let go_fill = |hover| {
            let (f, list) = barred(&s, 96, 900, hover, EMPTY);
            let go = f.layout.bar.expect("open").go;
            list.iter()
                .filter_map(|op| match op {
                    Op::Rect { rect, fill, .. } if rect.right == go.right - 1 => Some(*fill),
                    _ => None,
                })
                .next_back()
                .expect("the go fill")
        };
        assert_eq!(go_fill(Hit::Nothing), PAL.go);
        assert_eq!(go_fill(Hit::Go), PAL.go_hover);
    }

    #[test]
    fn the_list_button_shows_a_chevron_and_lights_under_the_pointer() {
        let s = scene(&[DotState::Live]);
        let count = |hover| {
            let (f, list) = barred(&s, 96, 900, hover, EMPTY);
            let l = f.layout.bar.expect("open").list;
            let lines = list
                .iter()
                .filter(|op| matches!(op, Op::Line { from, .. } if l.contains(from.0, from.1)))
                .count();
            let lit = list
                .iter()
                .any(|op| matches!(op, Op::Rect { fill, .. } if *fill == PAL.tab_hover));
            (lines, lit)
        };
        assert_eq!(count(Hit::Nothing), (2, false));
        assert_eq!(count(Hit::List), (2, true));
    }

    #[test]
    fn nothing_is_drawn_outside_the_strip_at_any_scale_and_width() {
        let s = scene(&[DotState::Live, DotState::Failed, DotState::Stopped]);
        for dpi in DPIS {
            for width in (360 * dpi as i32 / 96..1800).step_by(131) {
                let (f, list) = barred(&s, dpi, width, Hit::Nothing, EMPTY);
                for op in &list {
                    let rect = match op {
                        Op::Rect { rect, .. }
                        | Op::Ellipse { rect, .. }
                        | Op::Arc { rect, .. }
                        | Op::Pie { rect, .. }
                        | Op::Text { rect, .. }
                        | Op::Clip(rect) => *rect,
                        Op::Line { .. } | Op::Unclip => continue,
                    };
                    assert!(
                        rect.left >= 0
                            && rect.right <= width
                            && rect.top >= 0
                            && rect.bottom <= f.metrics.strip_height,
                        "{op:?} at {dpi} dpi, width {width}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_name_is_drawn_at_the_font_the_layout_chose_and_in_a_rect_that_holds_it() {
    let names = ["user@203.0.113.1", "user@203.0.113.12", "user@203.0.113.2"];
    let mut tabs = Tabs::new();
    for name in names {
        tabs.open(&name.parse().expect("a host"));
    }
    let ids: Vec<TabId> = tabs.iter().map(|t| t.id()).collect();
    let s = Scene {
        paint: ids
            .iter()
            .zip(names)
            .map(|(id, name)| TabPaint {
                id: *id,
                label: name.to_string(),
                dot: DotState::Live,
            })
            .collect(),
        items: ids
            .iter()
            .zip(names)
            .map(|(id, name)| TabItem {
                id: *id,
                label: name.to_string(),
            })
            .collect(),
        ids,
    };
    for dpi in DPIS {
        for width in [420, 520, 700, 1000] {
            let f = frame(&s, dpi, width * dpi as i32 / 96, 0);
            let texts: Vec<(String, i32, Rect)> = ops(&s, &f, None, Hit::Nothing, None, 0)
                .iter()
                .filter_map(|o| match o {
                    Op::Text {
                        text, size, rect, ..
                    } => Some((text.clone(), *size, *rect)),
                    _ => None,
                })
                .collect();
            assert_eq!(texts.len(), 3, "{dpi} dpi, {width}");
            for (text, size, rect) in texts {
                assert_eq!(size, f.layout.text, "{dpi} dpi, {width}: {text}");
                assert!(rect.width() >= Fixed.text_width(&text, size), "{text}");
            }
        }
    }
}
