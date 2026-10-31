//! What the pointer does on the strip, as a state machine. The window passes
//! each mouse message in as an [`Input`] and runs the [`Action`]s that come
//! out; nothing here touches Win32, so a click, a drag, a middle click and a
//! wheel turn are tests of a list.
//!
//! The rules (section 3.2 of the tabs design): a click acts when the button
//! goes up on what it went down on, so a press that slides off cancels; a
//! press on a tab that moves past the drag distance becomes a drag, dropped
//! where the pointer is let go; the middle button acts on the whole tab as the
//! slot does; the right button opens the menu of a tab.

use super::Hit;
use super::Layout;
use super::Metrics;
use super::draw::Drag;
use crate::tabs::TabId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// A mouse event, in the strip's own pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Move(i32, i32),
    Down(Button, i32, i32),
    Up(Button, i32, i32),
    /// The pointer left the strip, with no button held.
    Leave,
    /// The capture ended without a button going up (focus lost, Escape).
    Cancel,
    /// The wheel turned: positive away from the person, in 120ths of a
    /// notch.
    Wheel(i32),
}

/// What the window does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A click on a tab: show it.
    Show(TabId),
    /// A click on a tab's dot slot, or a middle click on the tab: stop it or
    /// forget it, as [`super::menu::dot_command`] says.
    Dot(TabId),
    /// The right button on a tab, at this point of the strip.
    Menu(TabId, (i32, i32)),
    /// A tab was dragged: move it before `before`, or to the end.
    Move {
        id: TabId,
        before: Option<TabId>,
    },
    Add,
    /// The address bar's go button.
    Go,
    /// The address bar's recent-hosts button.
    List,
    /// Show the list of every tab.
    Chevron,
    Fullscreen,
    /// Scroll the tabs by this many pixels (positive: later tabs come into
    /// view).
    Scroll(i32),
}

#[derive(Clone, Copy, Debug)]
struct Press {
    button: Button,
    hit: Hit,
    at: (i32, i32),
    x: i32,
    dragging: bool,
}

/// The pointer's state on the strip.
#[derive(Clone, Copy, Debug)]
pub struct Ui {
    hover: Hit,
    press: Option<Press>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            hover: Hit::Nothing,
            press: None,
        }
    }
}

/// The tab a hit is on, slot or body.
fn tab_of(hit: Hit) -> Option<TabId> {
    match hit {
        Hit::Tab(id) | Hit::Slot(id) => Some(id),
        _ => None,
    }
}

impl Ui {
    /// What the pointer is over; nothing while a drag is under way.
    pub fn hover(&self) -> Hit {
        if self.dragging() {
            Hit::Nothing
        } else {
            self.hover
        }
    }

    /// Whether a button is held, so the window keeps the mouse captured.
    pub fn pressed(&self) -> bool {
        self.press.is_some()
    }

    pub fn dragging(&self) -> bool {
        self.press.is_some_and(|p| p.dragging)
    }

    /// The drag to draw, if one is under way.
    pub fn drag(&self, layout: &Layout) -> Option<Drag> {
        let press = self.press.filter(|p| p.dragging)?;
        let id = tab_of(press.hit)?;
        Some(Drag {
            id,
            before: layout.drop_before(id, press.x),
        })
    }

    pub fn input(&mut self, layout: &Layout, metrics: &Metrics, input: Input) -> Vec<Action> {
        match input {
            Input::Move(x, y) => {
                self.hover = layout.hit(x, y);
                if let Some(press) = &mut self.press {
                    press.x = x;
                    let far = (x - press.at.0).abs() >= metrics.drag;
                    if press.button == Button::Left && far && matches!(press.hit, Hit::Tab(_)) {
                        press.dragging = true;
                    }
                }
                Vec::new()
            }
            Input::Down(button, x, y) => {
                if self.press.is_none() {
                    let hit = layout.hit(x, y);
                    self.hover = hit;
                    self.press = Some(Press {
                        button,
                        hit,
                        at: (x, y),
                        x,
                        dragging: false,
                    });
                }
                Vec::new()
            }
            Input::Up(button, x, y) => {
                let Some(press) = self.press.filter(|p| p.button == button) else {
                    return Vec::new();
                };
                self.press = None;
                let hit = layout.hit(x, y);
                self.hover = hit;
                if press.dragging {
                    return match tab_of(press.hit) {
                        Some(id) => vec![Action::Move {
                            id,
                            before: layout.drop_before(id, x),
                        }],
                        None => Vec::new(),
                    };
                }
                released(button, press.hit, hit, (x, y))
                    .into_iter()
                    .collect()
            }
            Input::Leave => {
                if self.press.is_none() {
                    self.hover = Hit::Nothing;
                }
                Vec::new()
            }
            Input::Cancel => {
                self.press = None;
                Vec::new()
            }
            Input::Wheel(delta) => {
                if layout.max_scroll <= 0 || self.dragging() {
                    return Vec::new();
                }
                // A notch moves the strip by a fixed step.
                let step = metrics.scroll_step;
                vec![Action::Scroll(-delta * step / 120)]
            }
        }
    }
}

/// The action of a button that went down on `down` and up on `up`.
fn released(button: Button, down: Hit, up: Hit, at: (i32, i32)) -> Option<Action> {
    match button {
        Button::Left if down == up => match down {
            Hit::Tab(id) => Some(Action::Show(id)),
            Hit::Slot(id) => Some(Action::Dot(id)),
            Hit::Add => Some(Action::Add),
            Hit::Chevron => Some(Action::Chevron),
            Hit::Fullscreen => Some(Action::Fullscreen),
            Hit::Go => Some(Action::Go),
            Hit::List => Some(Action::List),
            Hit::Nothing => None,
        },
        Button::Middle => match (tab_of(down), tab_of(up)) {
            (Some(a), Some(b)) if a == b => Some(Action::Dot(a)),
            _ => None,
        },
        Button::Right => match (tab_of(down), tab_of(up)) {
            (Some(a), Some(b)) if a == b => Some(Action::Menu(a, at)),
            _ => None,
        },
        Button::Left => None,
    }
}

#[cfg(test)]
mod tests;
