//! The failure panel: what the picture area of a tab that gave up shows
//! (design 11.3). A title (the message's first sentence), the second
//! sentence, one primary button, **Details** beside it, and a link into the
//! key guide where one helps.
//!
//! This is pure: the content comes from [`crate::connect::message`], the
//! layout from the picture's size and a text measure, and the look is a draw
//! list ([`Op`]) the window runs on a GDI surface and a golden test checks.
//! Everything is in device pixels, from [`Metrics`].

use crate::connect::Button;
use crate::connect::Guide;
use crate::connect::SshFailure;
use crate::connect::message;
use crate::history::HostTarget;
use crate::palette::Palette;
use crate::palette::Rgb;
use crate::strip::Measure;
use crate::strip::Rect;
use crate::strip::draw::Op;

/// What the panel says, before it is laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    /// What happened, in one sentence naming the machine.
    pub title: String,
    /// What to do, in one more.
    pub body: String,
    /// The one primary button.
    pub primary: Button,
    /// The guide the link goes to, where one helps.
    pub guide: Option<Guide>,
}

/// The content for one failure of `target`.
pub fn content(failure: &SshFailure, target: &HostTarget) -> Content {
    let m = message(failure, target);
    Content {
        title: m.what,
        body: m.to_do,
        primary: m.button,
        guide: m.guide,
    }
}

/// The words on a button.
pub fn button_label(button: Button) -> &'static str {
    match button {
        Button::TryAgain => "Try again",
        Button::CheckAgain => "Check again",
        Button::EditAddress => "Edit address",
        Button::OpenOptionalFeatures => "Open that Settings page",
        Button::Close => "Close",
        Button::Details => "Details",
    }
}

/// The words of the link to a guide page.
pub fn guide_label(guide: Guide) -> &'static str {
    match guide {
        Guide::TrustMachine => "How to trust a machine",
        Guide::SetUpKey => "How to set up a key",
        Guide::KeyFileRights => "How to fix the file's rights",
        Guide::PickOneKey => "How to pick one key",
        Guide::ChangedIdentity => "What to check, and how to forget the old identity",
    }
}

/// Where the link of a guide goes: the page in the repository.
pub fn guide_url(guide: Guide) -> String {
    format!(
        "{}/blob/main/{}#{}",
        env!("CARGO_PKG_REPOSITORY"),
        crate::connect::GUIDE_FILE,
        guide.anchor()
    )
}

/// Every size of the panel, in device pixels at one scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub dpi: u32,
    /// The widest the text column gets.
    pub column: i32,
    /// Room kept at both sides of the picture.
    pub margin: i32,
    /// Text sizes: the title (bold), the second sentence, a button's label,
    /// and the link.
    pub title: i32,
    pub body: i32,
    pub label: i32,
    pub link: i32,
    pub button_height: i32,
    pub button_pad: i32,
    pub button_min: i32,
    pub button_gap: i32,
    pub radius: i32,
    /// Space between the title and the second sentence, between that and the
    /// buttons, and between the buttons and the link.
    pub gap_title: i32,
    pub gap_buttons: i32,
    pub gap_link: i32,
}

impl Metrics {
    pub fn at(dpi: u32) -> Self {
        let dpi = dpi.max(48);
        let px = |logical: i32| (logical * dpi as i32 + 48) / 96;
        Metrics {
            dpi,
            column: px(520),
            margin: px(16),
            title: px(16),
            body: px(14),
            label: px(14),
            link: px(13),
            button_height: px(32),
            button_pad: px(20),
            button_min: px(88),
            button_gap: px(10),
            radius: px(4),
            gap_title: px(6),
            gap_buttons: px(18),
            gap_link: px(14),
        }
    }

    /// The height of one line of text of `size`.
    fn line(size: i32) -> i32 {
        size * 5 / 4
    }
}

/// Which text a line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Title,
    Body,
}

/// One line of the title or the second sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub rect: Rect,
    pub text: String,
    pub kind: Kind,
}

/// What a click or the pointer can be on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Primary,
    Details,
    Guide,
}

/// Where everything goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub lines: Vec<Line>,
    pub primary: Rect,
    pub details: Option<Rect>,
    pub link: Option<Rect>,
    /// Where the words of the two buttons go (text is drawn from the left).
    labels: (Rect, Option<Rect>),
    metrics: Metrics,
    content: Content,
}

impl Layout {
    /// What is at (`x`, `y`) in the picture.
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        if self.primary.contains(x, y) {
            Some(Action::Primary)
        } else if self.details.is_some_and(|r| r.contains(x, y)) {
            Some(Action::Details)
        } else if self.link.is_some_and(|r| r.contains(x, y)) {
            Some(Action::Guide)
        } else {
            None
        }
    }

    /// What the primary button does.
    pub fn primary_button(&self) -> Button {
        self.content.primary
    }

    /// The guide the link opens.
    pub fn guide(&self) -> Option<Guide> {
        self.content.guide
    }
}

/// Bold text is a little wider than the measure, which has one weight.
fn bold_width(measure: &dyn Measure, text: &str, size: i32) -> i32 {
    measure.text_width(text, size) * 17 / 16
}

/// Breaks `text` at spaces so no line is wider than `column`. A word wider
/// than the column stays whole on its own line.
fn wrap(text: &str, column: i32, width_of: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(last) if width_of(&format!("{last} {word}")) <= column => {
                last.push(' ');
                last.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Lays the panel out in a picture of `width` by `height`.
pub fn layout(
    m: &Metrics,
    content: &Content,
    width: i32,
    height: i32,
    measure: &dyn Measure,
) -> Layout {
    let column = m.column.min(width - 2 * m.margin).max(1);
    let title_width = |t: &str| bold_width(measure, t, m.title);
    let body_width = |t: &str| measure.text_width(t, m.body);
    let titles = wrap(&content.title, column, &title_width);
    let bodies = wrap(&content.body, column, &body_width);

    let wants = |button: Button| {
        (measure.text_width(button_label(button), m.label) + 2 * m.button_pad).max(m.button_min)
    };
    let primary_width = wants(content.primary);
    let details_width = (content.primary != Button::Details).then(|| wants(Button::Details));
    let link_text = content.guide.map(guide_label);

    let title_h = Metrics::line(m.title);
    let body_h = Metrics::line(m.body);
    let link_h = Metrics::line(m.link);
    let mut total = titles.len() as i32 * title_h + m.gap_buttons + m.button_height;
    if !bodies.is_empty() {
        total += m.gap_title + bodies.len() as i32 * body_h;
    }
    if link_text.is_some() {
        total += m.gap_link + link_h;
    }
    // A little above the middle; a picture too small shows the top.
    let mut y = (height * 2 / 5 - total / 2).max(0);

    let centred = |w: i32| (width - w) / 2;
    let mut lines = Vec::new();
    for text in titles {
        let w = title_width(&text).min(width);
        lines.push(Line {
            rect: Rect {
                left: centred(w),
                top: y,
                right: centred(w) + w,
                bottom: y + title_h,
            },
            text,
            kind: Kind::Title,
        });
        y += title_h;
    }
    if !bodies.is_empty() {
        y += m.gap_title;
    }
    for text in bodies {
        let w = body_width(&text).min(width);
        lines.push(Line {
            rect: Rect {
                left: centred(w),
                top: y,
                right: centred(w) + w,
                bottom: y + body_h,
            },
            text,
            kind: Kind::Body,
        });
        y += body_h;
    }
    y += m.gap_buttons;

    let group = primary_width + details_width.map_or(0, |w| m.button_gap + w);
    let left = centred(group);
    let primary = Rect {
        left,
        top: y,
        right: left + primary_width,
        bottom: y + m.button_height,
    };
    let details = details_width.map(|w| Rect {
        left: primary.right + m.button_gap,
        top: y,
        right: primary.right + m.button_gap + w,
        bottom: y + m.button_height,
    });
    y += m.button_height;
    let label_in = |r: Rect, button: Button| {
        let w = measure.text_width(button_label(button), m.label);
        let left = r.left + (r.width() - w) / 2;
        Rect {
            left,
            top: r.top,
            right: left + w,
            bottom: r.bottom,
        }
    };
    let labels = (
        label_in(primary, content.primary),
        details.map(|r| label_in(r, Button::Details)),
    );

    let link = link_text.map(|text| {
        y += m.gap_link;
        let w = measure.text_width(text, m.link).min(width);
        Rect {
            left: centred(w),
            top: y,
            right: centred(w) + w,
            bottom: y + link_h,
        }
    });

    Layout {
        width,
        height,
        lines,
        primary,
        details,
        link,
        labels,
        metrics: *m,
        content: content.clone(),
    }
}

/// The picture area behind the panel: black in both themes.
const BACKDROP: Rgb = Rgb(0, 0, 0);

/// What the panel looks like, `hover` being what the pointer is on.
pub fn ops(l: &Layout, pal: &Palette, hover: Option<Action>) -> Vec<Op> {
    let m = &l.metrics;
    let mut out = vec![Op::Rect {
        rect: Rect {
            left: 0,
            top: 0,
            right: l.width,
            bottom: l.height,
        },
        radius: 0,
        fill: BACKDROP,
    }];
    for line in &l.lines {
        let title = line.kind == Kind::Title;
        out.push(Op::Text {
            rect: line.rect,
            text: line.text.clone(),
            size: if title { m.title } else { m.body },
            color: if title { pal.text } else { pal.text_dim },
            bold: title,
        });
    }
    let on = |a| hover == Some(a);
    let mut button = |rect: Rect, label: Rect, words: &str, fill: Rgb, bold: bool| {
        out.push(Op::Rect {
            rect,
            radius: m.radius,
            fill,
        });
        out.push(Op::Text {
            rect: label,
            text: words.to_owned(),
            size: m.label,
            color: pal.text,
            bold,
        });
    };
    button(
        l.primary,
        l.labels.0,
        button_label(l.content.primary),
        if on(Action::Primary) {
            pal.primary_hover
        } else {
            pal.primary
        },
        true,
    );
    if let (Some(rect), Some(label)) = (l.details, l.labels.1) {
        button(
            rect,
            label,
            button_label(Button::Details),
            if on(Action::Details) {
                pal.go_hover
            } else {
                pal.go
            },
            false,
        );
    }
    if let (Some(rect), Some(guide)) = (l.link, l.content.guide) {
        out.push(Op::Text {
            rect,
            text: guide_label(guide).to_owned(),
            size: m.link,
            color: pal.accent,
            bold: false,
        });
        out.push(Op::Line {
            from: (rect.left, rect.bottom - 1),
            to: (rect.right, rect.bottom - 1),
            width: (m.dpi as i32 / 96).max(1),
            color: pal.accent,
        });
    }
    out
}

#[cfg(test)]
mod tests;
