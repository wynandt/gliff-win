//! The tab strip, as pure geometry: where each tab, its dot slot and the
//! buttons go at a given width and scale, what a click lands on, where a
//! dragged tab is dropped, and which dot a tab shows. No Win32 here, so the
//! rules run in tests on any machine; the window only draws what this says
//! and passes clicks to it.
//!
//! All pixel values are device pixels. The design's sizes are in logical
//! pixels (96 dpi) and [`Metrics::at`] scales them; 100, 125, 150 and 200%
//! give the strip heights 32, 40, 48 and 64 of the design.

use crate::connect::SshFailure;
use crate::tabs::Status;
use crate::tabs::TabId;

/// Where the address bar's edit field goes: over the bar's field while the bar
/// is open and the strip is shown, else nowhere (hidden).
pub fn edit_target(bar: Option<Rect>, shown: bool) -> Option<Rect> {
    bar.filter(|_| shown)
}

/// A rectangle: `left` and `top` are inside, `right` and `bottom` are not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Every size the strip uses, in device pixels at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub dpi: u32,
    /// The whole strip.
    pub strip_height: i32,
    /// One tab, centred in the strip.
    pub tab_height: i32,
    /// Space at both ends of the strip.
    pub pad: i32,
    /// Space between neighbours.
    pub gap: i32,
    /// How far one wheel notch scrolls the strip.
    pub scroll_step: i32,
    /// Room the open bar leaves the tabs when it has to give up width.
    pub bar_tab_room: i32,
    /// Space before a tab's name.
    pub label_pad: i32,
    /// Space between a tab's name and its dot, so a name that measures
    /// exactly its width is not cut by a pixel of rounding.
    pub label_gap: i32,
    /// The clickable square at a tab's right end that holds the dot.
    pub slot: i32,
    /// Space after the slot.
    pub slot_margin: i32,
    /// The dot's diameter.
    pub dot: i32,
    /// The height of the name's font at full size.
    pub text: i32,
    /// The smallest the name's font gets before the strip scrolls.
    pub text_floor: i32,
    /// The add, chevron and full-screen buttons.
    pub button: i32,
    /// The corner radius of a tab.
    pub radius: i32,
    /// How far the pointer moves before a press becomes a drag.
    pub drag: i32,
    /// The address bar's field, with its chevron inside the right end.
    pub bar_field: i32,
    /// The recent-hosts button at the field's right end.
    pub bar_list: i32,
    /// The go button.
    pub bar_go: i32,
    /// The least the field shrinks to when the strip is crowded.
    pub bar_field_min: i32,
    /// Space between the field's edge and its text.
    pub bar_inset: i32,
}

impl Metrics {
    /// The name font sizes the strip tries, largest first: the full size
    /// down to the floor, one logical pixel at a time.
    pub fn text_sizes(&self) -> Vec<i32> {
        let px = |logical: i32| (logical * self.dpi as i32 + 48) / 96;
        let mut sizes = vec![self.text];
        for logical in (9..13).rev() {
            let size = px(logical).clamp(self.text_floor, self.text);
            if sizes.last().is_some_and(|last| size < *last) {
                sizes.push(size);
            }
        }
        if sizes.last().is_some_and(|last| *last > self.text_floor) {
            sizes.push(self.text_floor);
        }
        sizes
    }

    pub fn at(dpi: u32) -> Self {
        let dpi = dpi.max(48);
        let px = |logical: i32| (logical * dpi as i32 + 48) / 96;
        Metrics {
            dpi,
            strip_height: px(32),
            tab_height: px(26),
            pad: px(6),
            gap: px(6),
            scroll_step: px(46),
            bar_tab_room: px(92),
            label_pad: px(12),
            label_gap: px(4),
            slot: px(22),
            slot_margin: px(4),
            dot: px(10),
            text: px(13),
            text_floor: px(9),
            button: px(28),
            radius: px(8),
            drag: px(4),
            bar_field: px(206),
            bar_list: px(24),
            bar_go: px(32),
            bar_field_min: px(96),
            bar_inset: px(10),
        }
    }
}

/// The width of a tab's name. The window measures with GDI; tests use a
/// fixed width per character.
pub trait Measure {
    /// The width of `text` in the name font of `size` device pixels.
    fn text_width(&self, text: &str, size: i32) -> i32;
}

/// What the layout needs to know about a tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabItem {
    pub id: TabId,
    pub label: String,
}

/// Where one tab went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabBox {
    pub id: TabId,
    pub rect: Rect,
    /// The click target that holds the dot.
    pub slot: Rect,
    /// Where the name is drawn.
    pub label: Rect,
}

/// What a point on the strip is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// The body of a tab: a click shows it.
    Tab(TabId),
    /// The dot slot of a tab: a click stops a live tab and forgets one that
    /// is not.
    Slot(TabId),
    Add,
    Chevron,
    Fullscreen,
    /// The address bar's recent-hosts button.
    List,
    /// The address bar's go button.
    Go,
    /// Nothing: the strip's background, a gap, or a tab scrolled out of
    /// view.
    Nothing,
}

/// The address bar, where the add button was, when it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    /// The field with its border: the edit field and the list button.
    pub field: Rect,
    /// Where the edit field (the Win32 field) sits, inside the border.
    pub edit: Rect,
    /// The recent-hosts button, at the field's right end.
    pub list: Rect,
    pub go: Rect,
}

/// The strip at one width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub tabs: Vec<TabBox>,
    /// The add button, or the whole address bar when `bar` is set.
    pub add: Rect,
    /// Set while the address bar is open.
    pub bar: Option<Bar>,
    /// Present only when the tabs do not fit.
    pub chevron: Option<Rect>,
    pub fullscreen: Rect,
    /// Where tabs are visible; a tab outside it is clipped.
    pub viewport: Rect,
    /// The height of the font the names are drawn in (device pixels).
    pub text: i32,
    /// The scroll actually applied (the request, clamped).
    pub scroll: i32,
    pub max_scroll: i32,
}

/// Lays the strip out. `scroll` is how far the tabs are moved left; it is
/// clamped to what the content allows.
///
/// Tabs take their own width: the whole name, the dot and the padding, with no
/// minimum. A name is never cut. When the tabs do not fit, the
/// name font shrinks for all of them, a pixel at a time down to
/// `text_floor`; only when they do not fit even then do the tabs keep
/// that width, the strip scrolls, and the add button and a chevron are
/// pinned at the right, left of the full-screen button.
pub fn layout(
    metrics: &Metrics,
    width: i32,
    tabs: &[TabItem],
    scroll: i32,
    bar: bool,
    measure: &dyn Measure,
) -> Layout {
    let m = metrics;
    let top = (m.strip_height - m.tab_height) / 2;
    let bottom = top + m.tab_height;
    let button = |right: i32| Rect {
        left: right - m.button,
        top,
        right,
        bottom,
    };
    let fullscreen = button(width - m.pad);
    // The add button, or the open address bar in its place.
    let add_width = if bar {
        m.bar_field + m.bar_list + m.bar_go
    } else {
        m.button
    };
    let count = tabs.len() as i32;

    let natural_at = |size: i32| -> Vec<i32> {
        tabs.iter()
            .map(|tab| {
                m.label_pad
                    + measure.text_width(&tab.label, size)
                    + m.label_gap
                    + m.slot
                    + m.slot_margin
            })
            .collect()
    };
    // Room for the tabs and the add button between the two paddings and the
    // full-screen button.
    let room = width - 2 * m.pad - m.button - m.gap;
    let fits = |natural: &[i32]| natural.iter().sum::<i32>() + count * m.gap + add_width <= room;
    let sizes = m.text_sizes();
    let fitting = sizes.iter().find_map(|size| {
        let natural = natural_at(*size);
        fits(&natural).then_some((*size, natural))
    });
    let (text, natural, cap) = match fitting {
        Some((size, natural)) => (size, natural, true),
        None => (m.text_floor, natural_at(m.text_floor), false),
    };

    let (widths, add, chevron, viewport) = match cap {
        true => {
            let widths = natural;
            let used: i32 = widths.iter().sum::<i32>() + count * m.gap;
            let add_left = m.pad + used;
            let viewport = Rect {
                left: m.pad,
                top,
                right: m.pad + used - m.gap.min(used),
                bottom,
            };
            let add = Rect {
                left: add_left,
                right: add_left + add_width,
                top,
                bottom,
            };
            (widths, add, None, viewport)
        }
        false => {
            let chevron = button(fullscreen.left - m.gap);
            // The bar gives up field width before it gives up its buttons, and
            // leaves the tabs about one tab of room.
            let right = chevron.left - m.gap;
            let least = m.bar_field_min + m.bar_list + m.bar_go;
            let width = if bar {
                add_width.min((right - m.pad - m.gap - m.bar_tab_room).max(least))
            } else {
                add_width
            };
            let add = Rect {
                left: right - width,
                right,
                top,
                bottom,
            };
            let viewport = Rect {
                left: m.pad,
                top,
                right: (add.left - m.gap).max(m.pad),
                bottom,
            };
            (natural, add, Some(chevron), viewport)
        }
    };

    let content = widths.iter().sum::<i32>() + (count - 1).max(0) * m.gap;
    let max_scroll = (content - viewport.width()).max(0);
    let scroll = scroll.clamp(0, max_scroll);
    let mut x = viewport.left - scroll;
    let boxes = tabs
        .iter()
        .zip(&widths)
        .map(|(tab, w)| {
            let rect = Rect {
                left: x,
                top,
                right: x + w,
                bottom,
            };
            x += w + m.gap;
            let slot = Rect {
                left: rect.right - m.slot_margin - m.slot,
                top,
                right: rect.right - m.slot_margin,
                bottom,
            };
            let label = Rect {
                left: rect.left + m.label_pad,
                top,
                right: slot.left,
                bottom,
            };
            TabBox {
                id: tab.id,
                rect,
                slot,
                label,
            }
        })
        .collect();
    Layout {
        tabs: boxes,
        add,
        bar: bar.then(|| bar_of(m, add)),
        chevron,
        fullscreen,
        viewport,
        text,
        scroll,
        max_scroll,
    }
}

/// The parts of the address bar inside the rectangle `whole`.
fn bar_of(m: &Metrics, whole: Rect) -> Bar {
    let go = Rect {
        left: whole.right - m.bar_go,
        ..whole
    };
    let list = Rect {
        left: go.left - m.bar_list,
        right: go.left,
        ..whole
    };
    let field = Rect {
        right: go.left,
        ..whole
    };
    // The edit field is as tall as one line of text, centred.
    let high = (m.text * 4 + 2) / 3;
    let top = whole.top + (whole.bottom - whole.top - high) / 2;
    let edit = Rect {
        left: field.left + m.bar_inset,
        top,
        right: list.left - 2,
        bottom: top + high,
    };
    Bar {
        field,
        edit,
        list,
        go,
    }
}

impl Layout {
    /// The scroll that brings tab `id` fully into view, changing the
    /// current scroll as little as possible. An unknown tab changes
    /// nothing.
    pub fn scroll_to_show(&self, id: TabId) -> i32 {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return self.scroll;
        };
        let view = self.viewport.width();
        // The tab's place in the unscrolled content.
        let left = tab.rect.left - self.viewport.left + self.scroll;
        let right = left + tab.rect.width();
        let wanted = if left < self.scroll {
            left
        } else if right > self.scroll + view {
            right - view
        } else {
            self.scroll
        };
        wanted.clamp(0, self.max_scroll)
    }

    pub fn hit(&self, x: i32, y: i32) -> Hit {
        if let Some(bar) = &self.bar {
            if bar.go.contains(x, y) {
                return Hit::Go;
            }
            if bar.list.contains(x, y) {
                return Hit::List;
            }
        }
        if self.bar.is_none() && self.add.contains(x, y) {
            return Hit::Add;
        }
        if self.chevron.is_some_and(|chevron| chevron.contains(x, y)) {
            return Hit::Chevron;
        }
        if self.fullscreen.contains(x, y) {
            return Hit::Fullscreen;
        }
        if !self.viewport.contains(x, y) {
            return Hit::Nothing;
        }
        for tab in &self.tabs {
            if tab.slot.contains(x, y) {
                return Hit::Slot(tab.id);
            }
            if tab.rect.contains(x, y) {
                return Hit::Tab(tab.id);
            }
        }
        Hit::Nothing
    }

    /// Where a dragged tab dropped at horizontal position `x` goes: before
    /// the first other tab whose middle is right of `x`, or at the end
    /// (`None`). The answer is what [`crate::tabs::Tabs::move_tab`] takes.
    pub fn drop_before(&self, dragged: TabId, x: i32) -> Option<TabId> {
        self.tabs
            .iter()
            .filter(|tab| tab.id != dragged)
            .find(|tab| (tab.rect.left + tab.rect.right) / 2 > x)
            .map(|tab| tab.id)
    }
}

/// How a tab's dot looks (the design's section 3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DotState {
    /// A hollow ring: no session and none wanted.
    Stopped,
    /// A ring with a turning arc.
    Connecting,
    /// A filled dot.
    Live,
    /// A half-filled ring: the session ended and a retry is coming or
    /// running.
    Reconnecting,
    /// A filled dot with a cross: gave up.
    Failed,
}

/// The dot a tab with this status shows.
///
/// The failure does not change the dot: a tab that gave up is failed
/// whatever the reason. The panel, not the dot, tells the reasons apart.
pub fn dot_of(status: &Status, _failure: Option<&SshFailure>) -> DotState {
    match status {
        Status::Stopped => DotState::Stopped,
        Status::Connecting => DotState::Connecting,
        Status::Streaming => DotState::Live,
        Status::Reconnecting { .. } => DotState::Reconnecting,
        Status::GaveUp(_) => DotState::Failed,
    }
}

pub mod address;
pub mod draw;
pub mod edit_menu;
pub mod menu;
pub mod raster;
pub mod reveal;
pub mod ui;

#[cfg(test)]
mod tests;
