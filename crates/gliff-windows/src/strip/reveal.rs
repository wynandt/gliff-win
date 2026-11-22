//! Full screen: the strip is out of the layout and comes over the top of the
//! picture when the pointer asks for it. This is the rule and the slide, with no
//! Win32 in them, so both are tested on any machine. The window feeds them.
//! The edge and the leave margin use the strip's scale.

/// How long the strip takes to come out or go back.
pub const SLIDE_MS: u32 = 150;
/// Logical px from the top edge in which the pointer brings the strip out.
const EDGE: i32 = 2;
/// Logical px below the strip at which the pointer sends it back.
const LEAVE: i32 = 8;

/// The distances of the rule, in device px at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The pointer is at the edge when its y is below this.
    pub edge: i32,
    /// How far below the strip the pointer may go before the strip leaves.
    pub leave: i32,
    /// The strip's height.
    pub height: i32,
}

impl Rule {
    /// The edge, the leave margin and `height` at `dpi`, on the strip's scale.
    /// A dpi below 48 counts as 48. A dpi that does not fit in `i32` counts as 96.
    pub fn at(dpi: u32, height: i32) -> Self {
        Self {
            edge: super::device_px(EDGE, dpi),
            leave: super::device_px(LEAVE, dpi),
            height,
        }
    }
}

/// Where the pointer is, in the window's client coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    At(i32),
    /// Outside the window, or not known.
    Away,
}

/// Whether the strip should be out.
#[derive(Debug)]
pub struct Reveal {
    rule: Rule,
    out: bool,
    held: bool,
    pointer: Pointer,
}

impl Reveal {
    pub fn new(rule: Rule) -> Self {
        Self {
            rule,
            out: false,
            held: false,
            pointer: Pointer::Away,
        }
    }

    pub fn is_out(&self) -> bool {
        self.out
    }

    pub fn set_rule(&mut self, rule: Rule) {
        self.rule = rule;
    }

    /// The pointer moved. Whether the strip should now come out or go.
    pub fn pointer(&mut self, pointer: Pointer) -> bool {
        self.pointer = pointer;
        self.settle()
    }

    /// The address bar or a menu of the strip starts or stops being in use.
    /// Whether the strip should now come out or go.
    pub fn hold(&mut self, held: bool) -> bool {
        self.held = held;
        self.settle()
    }

    /// Back to hidden, with nothing held: the window left full screen.
    pub fn reset(&mut self) {
        self.out = false;
        self.held = false;
        self.pointer = Pointer::Away;
    }

    fn settle(&mut self) -> bool {
        let out = self.held
            || match self.pointer {
                Pointer::Away => false,
                Pointer::At(y) if self.out => y <= self.rule.height + self.rule.leave,
                Pointer::At(y) => (0..self.rule.edge).contains(&y),
            };
        std::mem::replace(&mut self.out, out) != out
    }
}

/// The strip's top edge over time: it is `-height` while hidden and 0 while out.
#[derive(Debug)]
pub struct Slide {
    height: i32,
    from: f64,
    to: f64,
    start: u32,
}

impl Slide {
    /// Hidden.
    pub fn new(height: i32) -> Self {
        Self {
            height,
            from: 0.0,
            to: 0.0,
            start: 0,
        }
    }

    pub fn set_height(&mut self, height: i32) {
        self.height = height;
    }

    /// Goes to out (`true`) or hidden from where it is at `now` (ms).
    pub fn retarget(&mut self, out: bool, now: u32) {
        let to = if out { 1.0 } else { 0.0 };
        if to == self.to {
            return;
        }
        self.from = self.progress(now);
        self.to = to;
        self.start = now;
    }

    /// Out or hidden at once.
    pub fn snap(&mut self, out: bool) {
        self.to = if out { 1.0 } else { 0.0 };
        self.from = self.to;
    }

    /// The strip's top at `now`, from `-height` to 0.
    pub fn top(&self, now: u32) -> i32 {
        let hidden = f64::from(self.height) * (1.0 - self.progress(now));
        -(hidden.round() as i32)
    }

    /// Nothing moves any more.
    pub fn at_rest(&self, now: u32) -> bool {
        self.from == self.to || now.saturating_sub(self.start) >= SLIDE_MS
    }

    fn progress(&self, now: u32) -> f64 {
        let t = (f64::from(now.saturating_sub(self.start)) / f64::from(SLIDE_MS)).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }
}

#[cfg(test)]
mod tests;
