use super::DotState;
use super::Hit;
use super::Layout;
use super::Measure;
use super::Metrics;
use super::Rect;
use super::TabItem;
use super::dot_of;
use super::layout;
use crate::connect::SshFailure;
use crate::tabs::Status;
use crate::tabs::Tabs;
use std::time::Instant;

/// Six logical pixels per character at the design's 13 px font, scaled with
/// the font.
struct Fixed;

impl Measure for Fixed {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        text.chars().count() as i32 * size / 2
    }
}

const DPIS: [u32; 4] = [96, 120, 144, 192];

/// Tabs 1..=n named `name`, `TabId`s from a real model so they are valid.
fn items(names: &[&str]) -> Vec<TabItem> {
    let mut tabs = Tabs::new();
    for name in names {
        tabs.open(&name.parse().expect("a host"));
    }
    tabs.iter()
        .map(|tab| TabItem {
            id: tab.id(),
            label: tab.target().to_string(),
        })
        .collect()
}

fn many(n: usize) -> Vec<TabItem> {
    let names: Vec<String> = (1..=n).map(|i| format!("user@host-{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    items(&refs)
}

fn lay(m: &Metrics, width: i32, tabs: &[TabItem], scroll: i32) -> Layout {
    layout(m, width, tabs, scroll, false, &Fixed)
}

fn lay_bar(m: &Metrics, width: i32, tabs: &[TabItem]) -> Layout {
    layout(m, width, tabs, 0, true, &Fixed)
}

#[test]
fn a_tab_fits_the_strip_and_the_wheel_step_scales() {
    for dpi in DPIS {
        let m = Metrics::at(dpi);
        assert_eq!(m.scroll_step, (46 * dpi as i32 + 48) / 96);
        assert!(m.tab_height < m.strip_height);
        assert!(m.slot >= m.dot, "the slot holds the dot");
    }
}

#[test]
fn an_empty_strip_has_the_add_button_first() {
    let m = Metrics::at(96);
    let l = lay(&m, 800, &[], 0);
    assert!(l.tabs.is_empty());
    assert_eq!(l.add.left, m.pad);
    assert_eq!(l.chevron, None);
    assert_eq!(l.fullscreen.right, 800 - m.pad);
    assert_eq!(l.max_scroll, 0);
}

#[test]
fn a_few_tabs_keep_their_own_width_and_the_add_button_follows_the_last() {
    let m = Metrics::at(96);
    let tabs = items(&["a@x", "bob@example.org"]);
    let l = lay(&m, 1200, &tabs, 0);
    assert_eq!(l.tabs.len(), 2);
    let [a, b] = [l.tabs[0], l.tabs[1]];
    assert_eq!(a.rect.left, m.pad);
    assert_eq!(b.rect.left, a.rect.right + m.gap);
    assert!(b.rect.width() > a.rect.width(), "a longer name is wider");
    // No minimum: a tab is its name and its padding, however short.
    let own = |text: &str| {
        m.label_pad + Fixed.text_width(text, l.text) + m.label_gap + m.slot + m.slot_margin
    };
    assert_eq!(a.rect.width(), own("a@x"));
    assert_eq!(b.rect.width(), own("bob@example.org"));
    assert!(
        a.rect.width() < (92 * m.dpi as i32 + 48) / 96,
        "narrower than the old minimum"
    );
    assert_eq!(l.add.left, b.rect.right + m.gap);
    assert_eq!(l.chevron, None);
    assert_eq!(l.max_scroll, 0);
}

#[test]
fn the_font_shrinks_together_before_the_chevron_appears() {
    let m = Metrics::at(96);
    let tabs = many(5);
    let wide = lay(&m, 2000, &tabs, 0);
    let narrow = lay(&m, 600, &tabs, 0);
    assert_eq!(wide.text, m.text, "room enough: the full-size font");
    assert!(narrow.text < m.text, "a tight strip: a smaller font");
    assert!(narrow.text >= m.text_floor);
    assert_eq!(narrow.chevron, None, "all five still fit");
    assert_eq!(narrow.max_scroll, 0);
    assert!(narrow.add.right <= narrow.fullscreen.left);
    // Every name has the room it needs, at the font the strip chose.
    for (tab, item) in narrow.tabs.iter().zip(&tabs) {
        assert!(tab.label.width() >= Fixed.text_width(&item.label, narrow.text));
    }
}

#[test]
fn when_even_the_smallest_font_does_not_fit_the_strip_scrolls_and_the_chevron_appears() {
    for dpi in DPIS {
        let m = Metrics::at(dpi);
        let tabs = many(12);
        let l = lay(&m, 900 * dpi as i32 / 96, &tabs, 0);
        let chevron = l.chevron.expect("overflow shows the chevron");
        assert_eq!(l.text, m.text_floor, "{dpi}: the smallest font");
        assert!(l.max_scroll > 0);
        assert!(l.viewport.right <= l.add.left, "the add button is pinned");
        assert!(l.add.right <= chevron.left);
        assert!(chevron.right <= l.fullscreen.left);
    }
}

/// Addresses that differ only at the end, and long ones, the way a
/// user with many similar machines meets them.
fn look_alikes() -> Vec<TabItem> {
    let mut names: Vec<String> = (1..=12).map(|i| format!("user@203.0.113.{i}")).collect();
    names.extend((1..=4).map(|i| format!("deploy@build-server-eng-west.example.net-{i}")));
    names.extend((1..=3).map(|i| format!("admin@a-host-name-of-forty-chars-or-so-x{i}")));
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    items(&refs)
}

#[test]
fn a_name_is_never_cut_at_any_width_scale_or_count() {
    let all = look_alikes();
    for dpi in DPIS {
        let m = Metrics::at(dpi);
        for count in [1, 2, 3, 6, 12, 16, 19] {
            let tabs = &all[..count.min(all.len())];
            for width in (360 * dpi as i32 / 96..3200).step_by(53) {
                let l = lay(&m, width, tabs, 0);
                assert!(
                    (m.text_floor..=m.text).contains(&l.text),
                    "{dpi} dpi, {count} tabs, {width} wide: font {}",
                    l.text
                );
                for (tab, item) in l.tabs.iter().zip(tabs) {
                    let need = Fixed.text_width(&item.label, l.text);
                    assert!(
                        tab.label.width() >= need,
                        "{} is cut: {} of {need} px at {dpi} dpi, {width} wide",
                        item.label,
                        tab.label.width()
                    );
                    assert!(
                        tab.rect.width()
                            >= m.label_pad + need + m.label_gap + m.slot + m.slot_margin,
                        "{} has no room for its slot",
                        item.label
                    );
                }
                if l.chevron.is_none() {
                    assert_eq!(l.max_scroll, 0, "{dpi} dpi, {count} tabs, {width} wide");
                    for tab in &l.tabs {
                        assert!(
                            tab.rect.left >= l.viewport.left && tab.rect.right <= l.viewport.right,
                            "a whole tab is in view when nothing scrolls"
                        );
                    }
                } else {
                    assert_eq!(l.text, m.text_floor, "scrolling comes after the font");
                }
            }
        }
    }
}

#[test]
fn a_name_too_long_for_the_smallest_font_makes_its_tab_wider_and_the_strip_scrolls() {
    let name = format!("user@{}.example.net", "a-very-long-machine-name-".repeat(5));
    let tabs = items(&[name.as_str(), "b@x"]);
    for dpi in DPIS {
        let m = Metrics::at(dpi);
        let l = lay(&m, 600 * dpi as i32 / 96, &tabs, 0);
        assert_eq!(l.text, m.text_floor, "{dpi}");
        let need = Fixed.text_width(&name, l.text);
        assert!(l.tabs[0].label.width() >= need, "{dpi}: the name is whole");
        assert!(
            l.tabs[0].rect.width() > 240 * dpi as i32 / 96,
            "{dpi}: wider than the old cap"
        );
        assert!(l.max_scroll > 0 && l.chevron.is_some(), "{dpi}");
    }
}

#[test]
fn nothing_leaves_the_strip_or_overlaps_at_any_width_scale_and_count() {
    for dpi in DPIS {
        let m = Metrics::at(dpi);
        for count in [0, 1, 2, 3, 5, 8, 20] {
            let tabs = many(count);
            for width in (400..2400).step_by(97) {
                let l = lay(&m, width, &tabs, 0);
                for t in &l.tabs {
                    assert_eq!(t.rect.top, (m.strip_height - m.tab_height) / 2);
                    assert!(t.label.right <= t.slot.left, "label meets slot");
                    assert!(t.slot.right <= t.rect.right);
                    assert!(t.label.left >= t.rect.left);
                }
                for pair in l.tabs.windows(2) {
                    assert!(pair[0].rect.right + m.gap <= pair[1].rect.left);
                }
                let buttons = [Some(l.add), l.chevron, Some(l.fullscreen)];
                let buttons: Vec<Rect> = buttons.into_iter().flatten().collect();
                for r in &buttons {
                    assert!(r.left >= 0 && r.right <= width, "{r:?} in {width}");
                }
                for pair in buttons.windows(2) {
                    assert!(pair[0].right <= pair[1].left, "{pair:?}");
                }
                assert!(l.viewport.right <= buttons[0].left || l.chevron.is_none());
            }
        }
    }
}

#[test]
fn the_slot_is_the_right_end_of_the_tab_and_the_dot_sits_in_it() {
    let m = Metrics::at(144);
    let tabs = items(&["a@x"]);
    let t = lay(&m, 1000, &tabs, 0).tabs[0];
    assert_eq!(t.slot.width(), m.slot);
    assert_eq!(t.slot.right, t.rect.right - m.slot_margin);
    assert_eq!(t.label.left, t.rect.left + m.label_pad);
}

#[test]
fn scroll_is_clamped_to_the_content() {
    let m = Metrics::at(96);
    let tabs = many(12);
    let at0 = lay(&m, 900, &tabs, 0);
    assert_eq!(lay(&m, 900, &tabs, -50).scroll, 0);
    assert_eq!(lay(&m, 900, &tabs, 10_000).scroll, at0.max_scroll);
    let mid = lay(&m, 900, &tabs, 100);
    assert_eq!(mid.scroll, 100);
    assert_eq!(
        mid.tabs[0].rect.left,
        at0.tabs[0].rect.left - 100,
        "tabs move left by the scroll"
    );
    assert_eq!(
        lay(&m, 3000, &tabs, 500).scroll,
        0,
        "no overflow, no scroll"
    );
}

#[test]
fn the_shown_tab_scrolls_into_view_and_a_visible_one_does_not_move() {
    let m = Metrics::at(96);
    let tabs = many(12);
    let l = lay(&m, 900, &tabs, 0);
    let last = tabs[11].id;
    let scroll = l.scroll_to_show(last);
    let after = lay(&m, 900, &tabs, scroll);
    let t = after.tabs[11];
    assert!(t.rect.left >= after.viewport.left && t.rect.right <= after.viewport.right);
    assert_eq!(scroll, after.max_scroll, "the last tab is at the end");
    assert_eq!(after.scroll_to_show(last), scroll, "already in view");
    let back = after.scroll_to_show(tabs[0].id);
    assert_eq!(back, 0);
    let first_visible = after
        .tabs
        .iter()
        .find(|t| t.rect.left >= after.viewport.left)
        .expect("one");
    assert_eq!(after.scroll_to_show(first_visible.id), scroll);
    let unknown = many(13)[12].id;
    assert_eq!(after.scroll_to_show(unknown), scroll, "unknown tab");
}

#[test]
fn a_click_lands_on_the_slot_the_body_a_button_or_nothing() {
    let m = Metrics::at(96);
    let tabs = items(&["a@x", "b@x"]);
    let l = lay(&m, 1000, &tabs, 0);
    let [a, b] = [l.tabs[0], l.tabs[1]];
    let mid_y = m.strip_height / 2;
    assert_eq!(l.hit(a.slot.left + 1, mid_y), Hit::Slot(a.id));
    assert_eq!(l.hit(a.rect.left + 2, mid_y), Hit::Tab(a.id));
    assert_eq!(l.hit(b.label.left + 2, mid_y), Hit::Tab(b.id));
    assert_eq!(
        l.hit(a.rect.right + m.gap / 2, mid_y),
        Hit::Nothing,
        "the gap"
    );
    assert_eq!(l.hit(l.add.left + 1, mid_y), Hit::Add);
    assert_eq!(l.hit(l.fullscreen.left + 1, mid_y), Hit::Fullscreen);
    assert_eq!(l.hit(a.rect.left + 2, 0), Hit::Nothing, "above the tab");
    assert_eq!(l.hit(-5, mid_y), Hit::Nothing);
    assert_eq!(l.hit(l.fullscreen.left - 1, mid_y), Hit::Nothing);
}

#[test]
fn a_tab_scrolled_out_of_view_cannot_be_hit_and_the_chevron_can() {
    let m = Metrics::at(96);
    let tabs = many(12);
    let l = lay(&m, 900, &tabs, 400);
    let mid_y = m.strip_height / 2;
    let hidden = l.tabs[0];
    assert!(hidden.rect.right <= l.viewport.left || hidden.rect.left < l.viewport.left);
    assert_eq!(l.hit(l.viewport.left - 1, mid_y), Hit::Nothing);
    let chevron = l.chevron.expect("chevron");
    assert_eq!(l.hit(chevron.left + 1, mid_y), Hit::Chevron);
    // A tab partly under the add button is not reachable through it.
    let at_edge = l.hit(l.viewport.right + 1, mid_y);
    assert!(matches!(at_edge, Hit::Nothing | Hit::Add));
    // A partly visible tab is hit only inside the viewport.
    for x in l.viewport.left..l.viewport.right {
        if let Hit::Tab(id) | Hit::Slot(id) = l.hit(x, mid_y) {
            assert!(tabs.iter().any(|t| t.id == id));
        }
    }
}

#[test]
fn a_dropped_tab_goes_before_the_first_other_tab_whose_middle_is_right_of_it() {
    let m = Metrics::at(96);
    let tabs = items(&["a@x", "b@x", "c@x"]);
    let l = lay(&m, 1000, &tabs, 0);
    let mid = |i: usize| (l.tabs[i].rect.left + l.tabs[i].rect.right) / 2;
    let (a, b, c) = (tabs[0].id, tabs[1].id, tabs[2].id);
    assert_eq!(l.drop_before(c, 0), Some(a), "far left");
    assert_eq!(l.drop_before(c, mid(1) - 1), Some(b));
    assert_eq!(
        l.drop_before(c, mid(1) + 1),
        None,
        "past b; c itself is skipped"
    );
    assert_eq!(
        l.drop_before(a, mid(1) + 1),
        Some(c),
        "a moves between b and c"
    );
    assert_eq!(l.drop_before(a, 5000), None, "past everything: the end");
    assert_eq!(l.drop_before(b, mid(0) - 1), Some(a));
}

#[test]
fn every_status_has_one_dot_and_a_gave_up_tab_is_failed_for_any_reason() {
    let now = Instant::now();
    assert_eq!(dot_of(&Status::Stopped, None), DotState::Stopped);
    assert_eq!(dot_of(&Status::Connecting, None), DotState::Connecting);
    assert_eq!(dot_of(&Status::Streaming, None), DotState::Live);
    for at in [Some(now), None] {
        let waiting = Status::Reconnecting {
            attempt: 1,
            of: 5,
            why: "x".into(),
            at,
        };
        assert_eq!(dot_of(&waiting, None), DotState::Reconnecting);
    }
    let gave_up = Status::GaveUp("x".into());
    assert_eq!(dot_of(&gave_up, None), DotState::Failed);
    assert_eq!(
        dot_of(&gave_up, Some(&SshFailure::KeyRefused)),
        DotState::Failed
    );
}

mod bar {
    use super::*;

    #[test]
    fn a_closed_strip_has_no_bar_and_the_add_button_is_one_button_wide() {
        let m = Metrics::at(96);
        let l = lay(&m, 900, &items(&["a@x"]), 0);
        assert_eq!(l.bar, None);
        assert_eq!(l.add.width(), m.button);
    }

    #[test]
    fn the_open_bar_takes_the_place_of_the_add_button_after_the_last_tab() {
        for dpi in DPIS {
            let m = Metrics::at(dpi);
            let tabs = items(&["a@x", "bob@example.org"]);
            let closed = lay(&m, 1400, &tabs, 0);
            let open = lay_bar(&m, 1400, &tabs);
            let bar = open.bar.expect("the bar is open");
            assert_eq!(open.add.left, closed.add.left, "{dpi}");
            assert_eq!(bar.field.left, open.add.left);
            assert_eq!(bar.field.right, bar.list.right);
            assert_eq!(bar.list.right, bar.go.left);
            assert_eq!(bar.go.right, open.add.right);
            assert_eq!(bar.field.width(), m.bar_field + m.bar_list);
            assert_eq!(bar.go.width(), m.bar_go);
            assert_eq!(open.tabs, closed.tabs, "the tabs do not move");
        }
    }

    #[test]
    fn the_field_and_the_buttons_are_as_tall_as_a_tab_and_the_text_box_sits_inside() {
        for dpi in DPIS {
            let m = Metrics::at(dpi);
            let l = lay_bar(&m, 1400, &items(&["a@x"]));
            let bar = l.bar.expect("open");
            for r in [bar.field, bar.list, bar.go] {
                assert_eq!((r.top, r.bottom), (l.fullscreen.top, l.fullscreen.bottom));
            }
            let e = bar.edit;
            assert!(e.left >= bar.field.left + m.bar_inset - 1, "{dpi}");
            assert!(e.right <= bar.list.left);
            assert!(e.top > bar.field.top && e.bottom < bar.field.bottom);
            assert!(e.width() > 0 && e.bottom > e.top);
            let centre = |r: Rect| r.top + r.bottom;
            assert!((centre(e) - centre(bar.field)).abs() <= 1, "centred");
        }
    }

    #[test]
    fn with_no_tab_the_bar_is_first() {
        let m = Metrics::at(96);
        let l = lay_bar(&m, 900, &[]);
        assert_eq!(l.bar.expect("open").field.left, m.pad);
        assert_eq!(l.chevron, None);
    }

    #[test]
    fn a_crowded_strip_pins_the_bar_shrinks_its_field_and_never_overlaps() {
        for dpi in DPIS {
            let m = Metrics::at(dpi);
            for count in [0, 1, 3, 6, 12, 20] {
                let tabs = many(count);
                for width in (360 * dpi as i32 / 96..2400).step_by(83) {
                    let l = lay_bar(&m, width, &tabs);
                    let bar = l.bar.expect("open");
                    let mut parts = vec![bar.field, bar.go];
                    parts.extend(l.chevron);
                    parts.push(l.fullscreen);
                    for r in &parts {
                        assert!(r.left >= 0 && r.right <= width, "{r:?} in {width}");
                    }
                    for pair in parts.windows(2) {
                        assert!(
                            pair[0].right <= pair[1].left,
                            "{pair:?} {dpi} {count} {width}"
                        );
                    }
                    assert!(
                        l.viewport.right <= bar.field.left || l.viewport.width() == 0,
                        "tabs stay left of the bar: {l:?}"
                    );
                    assert!(
                        bar.field.width() >= m.bar_field_min + m.bar_list,
                        "{dpi} {count} {width}"
                    );
                    assert!(bar.edit.right <= bar.list.left && bar.edit.width() > 0);
                }
            }
        }
    }

    #[test]
    fn the_bar_makes_the_tabs_overflow_sooner_than_the_add_button_does() {
        let m = Metrics::at(96);
        let tabs = many(4);
        // A width where four tabs and a button fit but not four tabs and the bar.
        let width = (60..2000)
            .find(|w| {
                lay(&m, *w, &tabs, 0).chevron.is_none() && lay_bar(&m, *w, &tabs).chevron.is_some()
            })
            .expect("such a width exists");
        assert!(width > 400);
    }

    #[test]
    fn a_point_on_the_bar_buttons_hits_them_and_the_add_button_is_gone() {
        let m = Metrics::at(96);
        let l = lay_bar(&m, 1000, &items(&["a@x"]));
        let bar = l.bar.expect("open");
        let y = (bar.go.top + bar.go.bottom) / 2;
        assert_eq!(l.hit(bar.go.left + 1, y), Hit::Go);
        assert_eq!(l.hit(bar.list.left + 1, y), Hit::List);
        // The edit field and the border are not a button.
        assert_eq!(l.hit(bar.edit.left + 1, y), Hit::Nothing);
        let closed = lay(&m, 1000, &items(&["a@x"]), 0);
        assert_eq!(closed.hit(closed.add.left + 1, y), Hit::Add);
        assert_eq!(closed.hit(bar.go.left + 1, y), Hit::Nothing);
    }
}
