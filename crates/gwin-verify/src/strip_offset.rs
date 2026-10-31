//! The client's tab strip sits at the top of its client area. The checks
//! talk about the picture, which is the client area below the strip: these
//! two functions convert between the two. They are pure so a build machine
//! tests them; the Windows harnesses read the strip's height from the
//! client (`gliff_windows::strip_win::height_of`).

/// The picture's screen origin and size, from the client area's and the
/// strip's height.
pub fn picture_in_client(
    origin: (i32, i32),
    size: (u32, u32),
    strip: u32,
) -> ((i32, i32), (u32, u32)) {
    (
        (
            origin.0,
            origin.1 + i32::try_from(strip).unwrap_or(i32::MAX),
        ),
        (size.0, size.1.saturating_sub(strip)),
    )
}

/// The client height that shows a picture `picture_height` tall under the
/// strip.
pub fn client_height_for(picture_height: u32, strip: u32) -> u32 {
    picture_height.saturating_add(strip)
}

#[cfg(test)]
mod tests {
    use super::client_height_for;
    use super::picture_in_client;

    #[test]
    fn the_picture_starts_below_the_strip_and_loses_its_height() {
        assert_eq!(
            picture_in_client((100, 50), (1920, 1080), 64),
            ((100, 114), (1920, 1016))
        );
    }

    #[test]
    fn without_a_strip_nothing_moves() {
        assert_eq!(
            picture_in_client((7, 9), (800, 600), 0),
            ((7, 9), (800, 600))
        );
        assert_eq!(client_height_for(600, 0), 600);
    }

    #[test]
    fn a_picture_fits_a_client_as_tall_as_both() {
        let client = client_height_for(1080, 64);
        assert_eq!(
            picture_in_client((0, 0), (1920, client), 64).1,
            (1920, 1080)
        );
    }

    #[test]
    fn a_client_shorter_than_the_strip_has_no_picture() {
        assert_eq!(picture_in_client((0, 0), (10, 20), 64).1, (10, 0));
    }
}
