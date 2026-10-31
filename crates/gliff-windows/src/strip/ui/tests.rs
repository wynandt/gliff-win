use super::Action;
use super::Button;
use super::Input;
use super::Ui;
use crate::strip::Hit;
use crate::strip::Layout;
use crate::strip::Measure;
use crate::strip::Metrics;
use crate::strip::TabItem;
use crate::strip::draw::Drag;
use crate::strip::layout;
use crate::tabs::TabId;
use crate::tabs::Tabs;

struct Fixed;

impl Measure for Fixed {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        text.chars().count() as i32 * size / 2
    }
}

struct Strip {
    m: Metrics,
    layout: Layout,
    ids: Vec<TabId>,
}

fn strip(count: usize, width: i32) -> Strip {
    build(count, width, false)
}

fn strip_with_bar(count: usize, width: i32) -> Strip {
    build(count, width, true)
}

fn build(count: usize, width: i32, bar: bool) -> Strip {
    let mut tabs = Tabs::new();
    for n in 1..=count {
        tabs.open(&format!("u@host-{n}").parse().expect("a host"));
    }
    let items: Vec<TabItem> = tabs
        .iter()
        .map(|t| TabItem {
            id: t.id(),
            label: t.target().to_string(),
        })
        .collect();
    let m = Metrics::at(96);
    let layout = layout(&m, width, &items, 0, bar, &Fixed);
    Strip {
        m,
        layout,
        ids: items.iter().map(|i| i.id).collect(),
    }
}

impl Strip {
    fn send(&self, ui: &mut Ui, input: Input) -> Vec<Action> {
        ui.input(&self.layout, &self.m, input)
    }

    /// A point inside tab `n` (0-based), off its slot.
    fn body(&self, n: usize) -> (i32, i32) {
        let t = &self.layout.tabs[n];
        (t.rect.left + 6, (t.rect.top + t.rect.bottom) / 2)
    }

    fn slot(&self, n: usize) -> (i32, i32) {
        let s = self.layout.tabs[n].slot;
        ((s.left + s.right) / 2, (s.top + s.bottom) / 2)
    }

    fn click(&self, ui: &mut Ui, button: Button, at: (i32, i32)) -> Vec<Action> {
        assert!(self.send(ui, Input::Down(button, at.0, at.1)).is_empty());
        self.send(ui, Input::Up(button, at.0, at.1))
    }
}

#[test]
fn a_left_click_on_a_tab_shows_it() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    assert_eq!(
        s.click(&mut ui, Button::Left, s.body(1)),
        [Action::Show(s.ids[1])]
    );
    assert!(!ui.pressed());
}

#[test]
fn a_left_click_on_a_slot_is_the_dot_button() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    assert_eq!(
        s.click(&mut ui, Button::Left, s.slot(2)),
        [Action::Dot(s.ids[2])]
    );
}

#[test]
fn the_middle_button_is_the_dot_button_on_the_whole_tab() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    assert_eq!(
        s.click(&mut ui, Button::Middle, s.body(0)),
        [Action::Dot(s.ids[0])]
    );
    assert_eq!(
        s.click(&mut ui, Button::Middle, s.slot(0)),
        [Action::Dot(s.ids[0])]
    );
    // A press on one tab and a release on another does nothing.
    let (a, b) = (s.body(0), s.body(1));
    s.send(&mut ui, Input::Down(Button::Middle, a.0, a.1));
    assert!(
        s.send(&mut ui, Input::Up(Button::Middle, b.0, b.1))
            .is_empty()
    );
}

#[test]
fn the_right_button_opens_the_menu_where_it_was_let_go() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let at = s.body(1);
    assert_eq!(
        s.click(&mut ui, Button::Right, at),
        [Action::Menu(s.ids[1], at)]
    );
    // Not on empty strip.
    assert!(
        s.click(&mut ui, Button::Right, (s.layout.fullscreen.left - 40, 16))
            .is_empty()
    );
}

#[test]
fn a_press_that_slides_off_cancels_the_click() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let (a, b) = (s.body(0), s.slot(0));
    s.send(&mut ui, Input::Down(Button::Left, a.0, a.1));
    // Onto the slot of the same tab: not the same thing.
    assert!(
        s.send(&mut ui, Input::Up(Button::Left, b.0, b.1))
            .is_empty()
    );
    assert!(!ui.pressed());
}

#[test]
fn the_buttons_of_the_right_side_act_on_release() {
    let s = strip(3, 600);
    let mid = |r: crate::strip::Rect| ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    let mut ui = Ui::default();
    assert_eq!(
        s.click(&mut ui, Button::Left, mid(s.layout.add)),
        [Action::Add]
    );
    assert_eq!(
        s.click(&mut ui, Button::Left, mid(s.layout.fullscreen)),
        [Action::Fullscreen]
    );
    let many = strip(12, 600);
    let chevron = many.layout.chevron.expect("a chevron");
    assert_eq!(
        many.click(&mut ui, Button::Left, mid(chevron)),
        [Action::Chevron]
    );
}

#[test]
fn a_click_on_empty_strip_does_nothing() {
    let s = strip(2, 600);
    let mut ui = Ui::default();
    assert!(
        s.click(&mut ui, Button::Left, (s.layout.fullscreen.left - 40, 16))
            .is_empty()
    );
}

#[test]
fn hover_follows_the_pointer_and_clears_when_it_leaves() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let at = s.body(1);
    s.send(&mut ui, Input::Move(at.0, at.1));
    assert_eq!(ui.hover(), Hit::Tab(s.ids[1]));
    let at = s.slot(1);
    s.send(&mut ui, Input::Move(at.0, at.1));
    assert_eq!(ui.hover(), Hit::Slot(s.ids[1]));
    s.send(&mut ui, Input::Leave);
    assert_eq!(ui.hover(), Hit::Nothing);
}

#[test]
fn a_press_that_moves_far_enough_is_a_drag_dropped_where_it_is_let_go() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let from = s.body(0);
    s.send(&mut ui, Input::Down(Button::Left, from.0, from.1));
    // Under the distance: not yet.
    s.send(&mut ui, Input::Move(from.0 + s.m.drag - 1, from.1));
    assert!(!ui.dragging());
    assert_eq!(ui.drag(&s.layout), None);
    // Past it.
    let over = s.layout.tabs[2].rect.right - 4;
    s.send(&mut ui, Input::Move(over, from.1));
    assert!(ui.dragging());
    assert_eq!(
        ui.drag(&s.layout),
        Some(Drag {
            id: s.ids[0],
            before: None
        })
    );
    assert_eq!(ui.hover(), Hit::Nothing);
    assert_eq!(
        s.send(&mut ui, Input::Up(Button::Left, over, from.1)),
        [Action::Move {
            id: s.ids[0],
            before: None
        }]
    );
    assert!(!ui.pressed() && !ui.dragging());
}

#[test]
fn a_drag_between_tabs_drops_before_the_next_tab() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let from = s.body(2);
    s.send(&mut ui, Input::Down(Button::Left, from.0, from.1));
    // Left of the middle of the second tab: before it.
    let to = s.layout.tabs[1].rect.left + 4;
    s.send(&mut ui, Input::Move(to, from.1));
    assert_eq!(
        ui.drag(&s.layout),
        Some(Drag {
            id: s.ids[2],
            before: Some(s.ids[1])
        })
    );
    assert_eq!(
        s.send(&mut ui, Input::Up(Button::Left, to, from.1)),
        [Action::Move {
            id: s.ids[2],
            before: Some(s.ids[1])
        }]
    );
}

#[test]
fn a_press_on_a_slot_or_a_button_never_drags() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let at = s.slot(0);
    s.send(&mut ui, Input::Down(Button::Left, at.0, at.1));
    s.send(&mut ui, Input::Move(at.0 + 100, at.1));
    assert!(!ui.dragging());
    assert!(
        s.send(&mut ui, Input::Up(Button::Left, at.0 + 100, at.1))
            .is_empty()
    );
}

#[test]
fn a_cancelled_drag_moves_nothing() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let from = s.body(0);
    s.send(&mut ui, Input::Down(Button::Left, from.0, from.1));
    s.send(&mut ui, Input::Move(from.0 + 80, from.1));
    assert!(ui.dragging());
    s.send(&mut ui, Input::Cancel);
    assert!(!ui.pressed() && !ui.dragging());
    assert!(
        s.send(&mut ui, Input::Up(Button::Left, from.0 + 80, from.1))
            .is_empty()
    );
}

#[test]
fn only_one_button_is_held_at_a_time() {
    let s = strip(3, 600);
    let mut ui = Ui::default();
    let at = s.body(0);
    s.send(&mut ui, Input::Down(Button::Left, at.0, at.1));
    // A second button goes down and up: ignored; the left one still counts.
    s.send(&mut ui, Input::Down(Button::Right, at.0, at.1));
    assert!(
        s.send(&mut ui, Input::Up(Button::Right, at.0, at.1))
            .is_empty()
    );
    assert_eq!(
        s.send(&mut ui, Input::Up(Button::Left, at.0, at.1)),
        [Action::Show(s.ids[0])]
    );
}

#[test]
fn the_wheel_scrolls_only_when_the_tabs_overflow() {
    let few = strip(2, 600);
    let mut ui = Ui::default();
    assert!(few.send(&mut ui, Input::Wheel(-120)).is_empty());
    let many = strip(12, 600);
    assert!(many.layout.max_scroll > 0);
    let step = many.m.scroll_step;
    // Away from the person: back toward the first tab.
    assert_eq!(
        many.send(&mut ui, Input::Wheel(120)),
        [Action::Scroll(-step)]
    );
    assert_eq!(
        many.send(&mut ui, Input::Wheel(-120)),
        [Action::Scroll(step)]
    );
    assert_eq!(
        many.send(&mut ui, Input::Wheel(-60)),
        [Action::Scroll(step / 2)]
    );
}

mod bar {
    use super::*;
    use crate::strip::Rect;

    fn mid(r: Rect) -> (i32, i32) {
        ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
    }

    #[test]
    fn a_click_on_the_go_button_goes() {
        let s = strip_with_bar(2, 900);
        let bar = s.layout.bar.expect("open");
        let mut ui = Ui::default();
        assert_eq!(s.click(&mut ui, Button::Left, mid(bar.go)), [Action::Go]);
    }

    #[test]
    fn a_click_on_the_recent_button_opens_the_list() {
        let s = strip_with_bar(2, 900);
        let bar = s.layout.bar.expect("open");
        let mut ui = Ui::default();
        assert_eq!(
            s.click(&mut ui, Button::Left, mid(bar.list)),
            [Action::List]
        );
    }

    #[test]
    fn a_press_on_go_that_slides_off_does_nothing() {
        let s = strip_with_bar(2, 900);
        let bar = s.layout.bar.expect("open");
        let mut ui = Ui::default();
        let at = mid(bar.go);
        assert!(
            s.send(&mut ui, Input::Down(Button::Left, at.0, at.1))
                .is_empty()
        );
        assert!(s.send(&mut ui, Input::Up(Button::Left, 2, at.1)).is_empty());
    }

    #[test]
    fn the_add_button_still_adds_when_the_bar_is_folded() {
        let s = strip(2, 900);
        let mut ui = Ui::default();
        assert_eq!(
            s.click(&mut ui, Button::Left, mid(s.layout.add)),
            [Action::Add]
        );
    }
}
