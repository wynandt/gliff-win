use super::Action;
use super::Content;
use super::Layout;
use super::Metrics;
use super::button_label;
use super::content;
use super::guide_url;
use super::layout;
use super::ops;
use crate::connect::Button;
use crate::connect::Guide;
use crate::connect::ServerVersion;
use crate::connect::SshFailure;
use crate::palette::Palette;
use crate::strip::Measure;
use crate::strip::Rect;
use crate::strip::draw::Op;

struct Fixed;

impl Measure for Fixed {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        text.chars().count() as i32 * size / 2
    }
}

const PAL: Palette = Palette::DARK;
const DPIS: [u32; 4] = [96, 120, 144, 192];

fn every_failure() -> Vec<SshFailure> {
    vec![
        SshFailure::ClientMissing,
        SshFailure::NameNotFound,
        SshFailure::Refused,
        SshFailure::Unreachable,
        SshFailure::HostUnknown,
        SshFailure::HostKeyChanged,
        SshFailure::KeyRefused,
        SshFailure::KeyFilePermissions,
        SshFailure::TooManyKeys,
        SshFailure::ServerMissing,
        SshFailure::ServerOld {
            found: ServerVersion::new(0, 4, 0),
        },
        SshFailure::ProtocolMismatch,
        SshFailure::Other {
            last_line: "Connection closed by remote host.".to_owned(),
        },
    ]
}

fn target() -> crate::history::HostTarget {
    "me@lab-pc".parse().expect("a target")
}

fn panel(failure: &SshFailure) -> Content {
    content(failure, &target())
}

fn lay(content: &Content, dpi: u32, width: i32, height: i32) -> Layout {
    layout(&Metrics::at(dpi), content, width, height, &Fixed)
}

fn inside(r: Rect, width: i32, height: i32) -> bool {
    r.left >= 0 && r.top >= 0 && r.right <= width && r.bottom <= height
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn every_rect(l: &Layout) -> Vec<Rect> {
    let mut all: Vec<Rect> = l.lines.iter().map(|line| line.rect).collect();
    all.push(l.primary);
    all.extend(l.details);
    all.extend(l.link);
    all
}

fn plain(text: &str) -> Content {
    Content {
        title: "Can't find \"lab-pc\".".to_owned(),
        body: text.to_owned(),
        primary: Button::TryAgain,
        guide: None,
    }
}

#[test]
fn the_column_is_centred_in_the_picture() {
    for dpi in DPIS {
        for failure in every_failure() {
            let l = lay(&panel(&failure), dpi, 1280, 720);
            for line in &l.lines {
                assert!(
                    ((line.rect.left + line.rect.right) - 1280).abs() <= 2,
                    "{line:?}"
                );
            }
            if let Some(link) = l.link {
                assert!(((link.left + link.right) - 1280).abs() <= 2);
            }
            let left = l.primary.left.min(l.details.map_or(i32::MAX, |d| d.left));
            let right = l.primary.right.max(l.details.map_or(0, |d| d.right));
            assert!((left + right - 1280).abs() <= 2, "{failure:?} at {dpi}");
        }
    }
}

#[test]
fn text_wraps_at_word_breaks_and_loses_nothing() {
    let text = "Signing in works, but ssh cannot find the server program on that machine, so \
                install it there and try again afterwards.";
    let l = lay(&plain(text), 96, 1280, 720);
    let column = Metrics::at(96).column;
    let body: Vec<&str> = l
        .lines
        .iter()
        .filter(|line| line.kind == super::Kind::Body)
        .map(|line| line.text.as_str())
        .collect();
    assert!(body.len() >= 2, "a long text takes more than one line");
    assert_eq!(body.join(" "), text);
    for line in &l.lines {
        assert!(
            line.rect.width() <= column,
            "{:?} is wider than the column",
            line.text
        );
    }
}

#[test]
fn a_word_wider_than_the_column_is_kept_whole() {
    let word = "x".repeat(200);
    let l = lay(&plain(&word), 96, 1280, 720);
    let lines: Vec<&str> = l.lines.iter().map(|line| line.text.as_str()).collect();
    assert!(lines.contains(&word.as_str()));
}

#[test]
fn a_narrow_picture_narrows_the_column() {
    let m = Metrics::at(96);
    let l = lay(
        &plain("one two three four five six seven eight nine ten eleven"),
        96,
        300,
        400,
    );
    for line in &l.lines {
        assert!(line.rect.width() <= 300 - 2 * m.margin, "{:?}", line.text);
    }
}

#[test]
fn details_sits_beside_the_primary_button_unless_it_is_the_primary() {
    let l = lay(&panel(&SshFailure::Refused), 96, 1280, 720);
    let details = l.details.expect("a Details button");
    assert_eq!(
        (details.top, details.bottom),
        (l.primary.top, l.primary.bottom)
    );
    assert_eq!(details.left - l.primary.right, Metrics::at(96).button_gap);

    let only = lay(
        &panel(&SshFailure::Other {
            last_line: "Connection closed by remote host.".to_owned(),
        }),
        96,
        1280,
        720,
    );
    assert_eq!(only.details, None);
}

#[test]
fn the_link_is_under_the_buttons_and_only_where_a_guide_helps() {
    let with = lay(&panel(&SshFailure::HostUnknown), 96, 1280, 720);
    let link = with.link.expect("a link");
    assert!(link.top >= with.primary.bottom);
    assert_eq!(lay(&panel(&SshFailure::Refused), 96, 1280, 720).link, None);
}

#[test]
fn nothing_overlaps_or_leaves_the_picture_at_any_scale() {
    for dpi in DPIS {
        for failure in every_failure() {
            for (lw, lh) in [(1920, 1080), (1280, 720), (800, 600), (480, 400)] {
                let (w, h) = (lw * dpi as i32 / 96, lh * dpi as i32 / 96);
                let l = lay(&panel(&failure), dpi, w, h);
                let all = every_rect(&l);
                for (i, a) in all.iter().enumerate() {
                    assert!(inside(*a, w, h), "{failure:?} {dpi} {w}x{h}: {a:?}");
                    for b in &all[i + 1..] {
                        assert!(!overlap(*a, *b), "{failure:?} {dpi} {w}x{h}: {a:?} {b:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_buttons_are_wide_enough_for_their_words() {
    for dpi in DPIS {
        let m = Metrics::at(dpi);
        for failure in every_failure() {
            let c = panel(&failure);
            let l = lay(&c, dpi, 1280, 720);
            let need = |button| Fixed.text_width(button_label(button), m.label) + 2 * m.button_pad;
            assert!(l.primary.width() >= need(c.primary));
            if let Some(d) = l.details {
                assert!(d.width() >= need(Button::Details));
            }
        }
    }
}

#[test]
fn a_click_lands_on_what_is_under_it() {
    let l = lay(&panel(&SshFailure::HostUnknown), 96, 1280, 720);
    let at = |r: Rect| ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    let (x, y) = at(l.primary);
    assert_eq!(l.hit(x, y), Some(Action::Primary));
    let (x, y) = at(l.details.expect("details"));
    assert_eq!(l.hit(x, y), Some(Action::Details));
    let (x, y) = at(l.link.expect("link"));
    assert_eq!(l.hit(x, y), Some(Action::Guide));
    assert_eq!(l.hit(2, 2), None);
}

#[test]
fn a_hovered_button_is_lighter() {
    let l = lay(&panel(&SshFailure::Refused), 96, 1280, 720);
    let fill = |hover| {
        ops(&l, &PAL, hover)
            .into_iter()
            .find_map(|op| match op {
                Op::Rect { rect, fill, .. } if rect == l.primary => Some(fill),
                _ => None,
            })
            .expect("the primary button is drawn")
    };
    assert_eq!(fill(None), PAL.primary);
    assert_eq!(fill(Some(Action::Primary)), PAL.primary_hover);
    assert_eq!(fill(Some(Action::Details)), PAL.primary);
}

fn hex(c: crate::palette::Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn render(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        let r = |r: &Rect| format!("{},{},{},{}", r.left, r.top, r.right, r.bottom);
        let line = match op {
            Op::Rect { rect, radius, fill } => format!("rect {} r{radius} {}", r(rect), hex(*fill)),
            Op::Text {
                rect,
                text,
                size,
                color,
                bold,
            } => format!(
                "text {} {text:?} {size}px {}{}",
                r(rect),
                hex(*color),
                if *bold { " bold" } else { "" }
            ),
            Op::Line {
                from,
                to,
                width,
                color,
            } => format!(
                "line {},{} {},{} w{width} {}",
                from.0,
                from.1,
                to.0,
                to.1,
                hex(*color)
            ),
            other => format!("{other:?}"),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

const GOLDEN: &str = include_str!("panel.golden");

#[test]
fn the_panel_for_a_refused_key_matches_its_golden() {
    let l = lay(&panel(&SshFailure::KeyRefused), 96, 640, 400);
    let text = render(&ops(&l, &PAL, None));
    if std::env::var_os("GLIFF_UPDATE_GOLDEN").is_some() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/failure_panel/panel.golden"
            ),
            &text,
        )
        .expect("write the golden");
    }
    assert_eq!(text, GOLDEN);
}

#[test]
fn a_guide_link_names_the_page_and_its_heading() {
    let url = guide_url(Guide::SetUpKey);
    assert!(url.starts_with("https://"), "{url}");
    assert!(url.ends_with("/docs/ssh-keys.md#2-create-a-key"), "{url}");
}
