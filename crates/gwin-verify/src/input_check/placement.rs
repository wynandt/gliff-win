//! Where the text sink goes so that the pointer can be *off the client*: on
//! the sink, which is a window of its own, and outside the client's frame.
//! A sink laid over the client's corner makes "off" mean "covered by another
//! window", which Windows reports differently from leaving the window (and
//! which a fault in the harness's own stacking can turn back into "over").

/// A rectangle in screen pixels, `right` and `bottom` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Space kept between the sink and the client's frame.
const GAP: i32 = 24;

/// The top-left corner for a window of `size` that lies wholly inside `work`
/// and clear of `client`: to its right, else left, below or above. `None`
/// when the client leaves no such room.
pub fn beside(client: Rect, work: Rect, size: (i32, i32)) -> Option<(i32, i32)> {
    let mid_y = ((client.top + client.bottom - size.1) / 2).clamp(work.top, work.bottom - size.1);
    let mid_x = ((client.left + client.right - size.0) / 2).clamp(work.left, work.right - size.0);
    let right = (client.right + GAP, mid_y);
    let left = (client.left - GAP - size.0, mid_y);
    let below = (mid_x, client.bottom + GAP);
    let above = (mid_x, client.top - GAP - size.1);
    [right, left, below, above].into_iter().find(|&(x, y)| {
        x >= work.left && y >= work.top && x + size.0 <= work.right && y + size.1 <= work.bottom
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (i32, i32) = (320, 160);

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The window the failing run had: 2906x1871 at the corner of a
    /// 3840x2400 monitor.
    #[test]
    fn the_sink_goes_to_the_strip_right_of_the_client() {
        let (x, y) = beside(rect(0, 0, 2906, 1871), rect(0, 0, 3840, 2360), SIZE)
            .expect("room beside the client");
        assert_eq!(x, 2906 + GAP);
        assert!((0..=2360 - SIZE.1).contains(&y));
    }

    #[test]
    fn the_sink_never_overlaps_the_client_or_leaves_the_work_area() {
        let work = rect(0, 0, 2000, 1200);
        for client in [
            rect(0, 0, 1000, 600),
            rect(1000, 600, 2000, 1200),
            rect(400, 0, 1700, 1100),
            rect(300, 300, 1500, 1000),
        ] {
            let (x, y) = beside(client, work, SIZE).expect("room beside the client");
            let apart = x + SIZE.0 <= client.left
                || x >= client.right
                || y + SIZE.1 <= client.top
                || y >= client.bottom;
            assert!(apart, "{client:?} -> {x},{y}");
            assert!(x >= 0 && y >= 0 && x + SIZE.0 <= 2000 && y + SIZE.1 <= 1200);
        }
    }

    #[test]
    fn a_client_that_fills_the_work_area_leaves_no_room() {
        assert_eq!(
            beside(rect(0, 0, 1920, 1040), rect(0, 0, 1920, 1040), SIZE),
            None
        );
    }

    #[test]
    fn the_left_side_is_used_when_the_right_is_too_narrow() {
        let (x, _) = beside(rect(500, 0, 1900, 1000), rect(0, 0, 1920, 1040), SIZE)
            .expect("room beside the client");
        assert_eq!(x, 500 - GAP - SIZE.0);
    }
}
