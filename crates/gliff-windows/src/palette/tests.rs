use super::Palette;
use super::Rgb;

#[test]
fn hex_splits_the_bytes_and_colorref_reverses_them_for_gdi() {
    let c = Rgb::hex(0x12_34_56);
    assert_eq!(c, Rgb(0x12, 0x34, 0x56));
    assert_eq!(c.colorref(), 0x00_56_34_12);
}

#[test]
fn translucent_fills_become_the_solid_colour_they_make_over_the_strip() {
    let white = Rgb(255, 255, 255);
    let p = Palette::DARK;
    // 0.14, 0.07 and 0.55 of 255, within one code of exact arithmetic.
    let near = |got: Rgb, alpha: f64| {
        let want = (48.0 + (255.0 - 48.0) * alpha).round() as i32;
        (i32::from(got.0) - want).abs() <= 1 && got.0 == got.1 && got.1 == got.2
    };
    assert!(near(p.tab_shown, 0.14), "{:?}", p.tab_shown);
    assert!(near(p.tab_hover, 0.07), "{:?}", p.tab_hover);
    assert!(near(p.text_dim, 0.55), "{:?}", p.text_dim);
    assert_eq!(white.over(p.strip, 0), p.strip);
    assert_eq!(white.over(p.strip, 256), white);
}
