use super::Canvas;
use crate::palette::Palette;
use crate::palette::Rgb;
use crate::strip::DotState;
use crate::strip::Metrics;
use crate::strip::Rect;
use crate::strip::draw::Op;
use crate::strip::draw::dot;

const PAL: Palette = Palette::DARK;
const DPIS: [u32; 5] = [96, 120, 144, 168, 192];

fn blank(w: i32, h: i32) -> Vec<u32> {
    let c = PAL.strip;
    vec![u32::from(c.0) << 16 | u32::from(c.1) << 8 | u32::from(c.2); (w * h) as usize]
}

fn rgb(px: u32) -> Rgb {
    Rgb((px >> 16) as u8, (px >> 8) as u8, px as u8)
}

/// A dot of `state` drawn at `dpi` on a square canvas, and its size.
fn draw_dot(state: DotState, dpi: u32, phase: u32) -> (Vec<u32>, i32) {
    let d = Metrics::at(dpi).dot;
    let size = d + 8;
    let mut px = blank(size, size);
    let mut canvas = Canvas::new(&mut px, size, size);
    for op in dot((size / 2, size / 2), d, state, phase, &PAL) {
        assert!(canvas.shape(&op));
    }
    (px, size)
}

fn at(px: &[u32], size: i32, x: i32, y: i32) -> Rgb {
    rgb(px[(y * size + x) as usize])
}

#[test]
fn a_live_dot_has_smooth_edges_and_the_area_of_a_circle() {
    for dpi in DPIS {
        let (px, size) = draw_dot(DotState::Live, dpi, 0);
        let d = f64::from(Metrics::at(dpi).dot);
        let mid = size / 2;
        assert_eq!(at(&px, size, mid, mid), PAL.ok, "centre at {dpi} dpi");
        assert_eq!(at(&px, size, 0, 0), PAL.strip, "corner at {dpi} dpi");
        // Pixels that are neither the dot's colour nor the strip's.
        let blended = px
            .iter()
            .filter(|p| rgb(**p) != PAL.ok && rgb(**p) != PAL.strip)
            .count();
        assert!(blended >= 8, "{blended} blended pixels at {dpi} dpi");
        // The strip's green channel runs from 0x30 to the dot's 0xf0: the
        // lit amount of all pixels is the dot's area.
        let lit: f64 = px
            .iter()
            .map(|p| f64::from(rgb(*p).1.saturating_sub(PAL.strip.1)))
            .sum::<f64>()
            / f64::from(PAL.ok.1 - PAL.strip.1);
        let circle = std::f64::consts::PI * d * d / 4.0;
        assert!(
            (lit - circle).abs() / circle < 0.03,
            "area {lit} against {circle} at {dpi} dpi"
        );
    }
}

#[test]
fn a_failed_dot_is_red_with_a_cross_you_can_see() {
    for dpi in DPIS {
        let (px, size) = draw_dot(DotState::Failed, dpi, 0);
        let d = Metrics::at(dpi).dot;
        let mid = size / 2;
        // The crossing is the cross's colour; a point of the red disc off
        // the cross is red.
        assert_eq!(at(&px, size, mid, mid), PAL.backdrop, "{dpi} dpi");
        let off = at(&px, size, mid + d / 2 - 2, mid);
        assert_eq!(off, PAL.err, "{dpi} dpi");
        let reds = px.iter().filter(|p| rgb(**p) == PAL.err).count();
        let crossed = px.iter().filter(|p| rgb(**p) == PAL.backdrop).count();
        assert!(reds > 0 && crossed >= 2, "{dpi} dpi");
    }
}

#[test]
fn a_connecting_arc_turns_and_a_stopped_ring_is_hollow() {
    let (a, size) = draw_dot(DotState::Connecting, 96, 0);
    let (b, _) = draw_dot(DotState::Connecting, 96, 5);
    assert_ne!(a, b);
    assert!(a.iter().any(|p| rgb(*p) == PAL.accent));
    let (ring, size_ring) = draw_dot(DotState::Stopped, 96, 0);
    assert_eq!(size, size_ring);
    // Hollow: the middle is the strip's colour.
    assert_eq!(at(&ring, size, size / 2, size / 2), PAL.strip);
    assert!(ring.iter().any(|p| rgb(*p) == PAL.ring));
}

#[test]
fn a_rounded_rectangle_keeps_its_corners_off_and_its_middle_on() {
    let (w, h) = (40, 20);
    let mut px = blank(w, h);
    let fill = Rgb::hex(0xffffff);
    Canvas::new(&mut px, w, h).shape(&Op::Rect {
        rect: Rect {
            left: 4,
            top: 2,
            right: 36,
            bottom: 18,
        },
        radius: 6,
        fill,
    });
    assert_eq!(at(&px, w, 20, 10), fill);
    assert_eq!(at(&px, w, 4, 2), PAL.strip, "the corner pixel is cut off");
    assert_eq!(at(&px, w, 20, 2), fill, "the straight edge is whole");
    assert_eq!(at(&px, w, 3, 10), PAL.strip, "nothing outside");
}

#[test]
fn a_clip_keeps_drawing_inside_it() {
    let (w, h) = (20, 10);
    let mut px = blank(w, h);
    let mut canvas = Canvas::new(&mut px, w, h);
    canvas.shape(&Op::Clip(Rect {
        left: 5,
        top: 0,
        right: 10,
        bottom: 10,
    }));
    let white = Rgb::hex(0xffffff);
    canvas.shape(&Op::Rect {
        rect: Rect {
            left: 0,
            top: 0,
            right: 20,
            bottom: 10,
        },
        radius: 0,
        fill: white,
    });
    canvas.shape(&Op::Unclip);
    assert_eq!(at(&px, w, 4, 5), PAL.strip);
    assert_eq!(at(&px, w, 5, 5), white);
    assert_eq!(at(&px, w, 9, 5), white);
    assert_eq!(at(&px, w, 10, 5), PAL.strip);
}

#[test]
fn text_is_left_to_the_caller() {
    let mut px = blank(4, 4);
    let op = Op::Text {
        rect: Rect::default(),
        text: "x".into(),
        size: 13,
        color: PAL.text,
        bold: false,
    };
    assert!(!Canvas::new(&mut px, 4, 4).shape(&op));
}

#[test]
fn a_line_is_whole_pixels_at_an_even_width_and_soft_when_slanted() {
    let (w, h) = (20, 20);
    let mut px = blank(w, h);
    let white = Rgb::hex(0xffffff);
    let mut canvas = Canvas::new(&mut px, w, h);
    canvas.shape(&Op::Line {
        from: (4, 10),
        to: (16, 10),
        width: 2,
        color: white,
    });
    canvas.shape(&Op::Line {
        from: (4, 2),
        to: (16, 8),
        width: 2,
        color: white,
    });
    assert_eq!(at(&px, w, 10, 9), white);
    assert_eq!(at(&px, w, 10, 10), white);
    assert_eq!(at(&px, w, 10, 8), PAL.strip);
    assert_eq!(at(&px, w, 10, 11), PAL.strip);
    let soft = (0..w)
        .flat_map(|x| (0..8).map(move |y| (x, y)))
        .filter(|(x, y)| {
            let c = at(&px, w, *x, *y);
            c != white && c != PAL.strip
        })
        .count();
    assert!(soft >= 10, "{soft} soft pixels on the slanted line");
}
