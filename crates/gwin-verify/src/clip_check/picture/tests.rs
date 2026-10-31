use super::Picture;

#[test]
fn a_picture_survives_png() {
    for picture in [
        Picture::opaque(37, 11, 3),
        Picture::translucent(37, 11, 3),
        Picture::opaque(1, 1, 0),
    ] {
        let png = picture.to_png().expect("encode");
        assert_eq!(Picture::from_png(&png).expect("decode"), picture);
    }
}

/// A PNG that is only a signature and a header claiming `width` x `height`
/// RGBA pixels, with no image data.
fn header_only_png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    drop(encoder.write_header().expect("header"));
    out
}

#[test]
fn a_png_that_claims_an_impossible_size_is_an_error_not_a_panic() {
    let png = header_only_png(u32::MAX, u32::MAX);
    assert!(Picture::from_png(&png).is_err());
}

#[test]
fn an_opaque_picture_is_an_rgb_png_and_a_translucent_one_rgba() {
    let color = |p: &Picture| {
        let png = p.to_png().expect("encode");
        png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .expect("header")
            .info()
            .color_type
    };
    assert_eq!(color(&Picture::opaque(4, 4, 1)), png::ColorType::Rgb);
    assert_eq!(color(&Picture::translucent(4, 4, 1)), png::ColorType::Rgba);
}

#[test]
fn a_dib_survives_with_every_row_padding() {
    // 32-bit rows need no padding; the 24-bit hand-made one below does.
    for width in 1..=7 {
        let picture = Picture::opaque(width, 5, 9);
        assert_eq!(Picture::from_dib(&picture.to_dib()).expect("read"), picture);
    }
}

#[test]
fn a_24_bit_dib_with_padded_rows_is_read() {
    // 3x2, bottom-up, 9 bytes a row padded to 12: red, green, blue / white,
    // black, grey.
    let mut dib = Vec::new();
    for value in [40u32, 3, 2] {
        dib.extend_from_slice(&value.to_le_bytes());
    }
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&24u16.to_le_bytes());
    dib.extend_from_slice(&[0; 24]);
    dib.extend_from_slice(&[255, 255, 255, 0, 0, 0, 128, 128, 128, 0, 0, 0]);
    dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 255, 0, 0, 0, 0, 0]);
    let picture = Picture::from_dib(&dib).expect("read");
    assert_eq!((picture.width, picture.height), (3, 2));
    assert_eq!(
        picture.rgba,
        [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 128,
            128, 128, 255
        ]
    );
}

#[test]
fn a_v5_bitmap_with_an_alpha_mask_keeps_alpha() {
    let mut dib = vec![0u8; 124];
    dib[0..4].copy_from_slice(&124u32.to_le_bytes());
    dib[4..8].copy_from_slice(&1u32.to_le_bytes());
    dib[8..12].copy_from_slice(&(-1i32).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    dib[16..20].copy_from_slice(&3u32.to_le_bytes());
    dib[52..56].copy_from_slice(&0xFF00_0000u32.to_le_bytes());
    dib.extend_from_slice(&[30, 20, 10, 99]);
    let picture = Picture::from_dib(&dib).expect("read");
    assert_eq!(picture.rgba, [10, 20, 30, 99]);
}

#[test]
fn a_bitmap_that_is_cut_short_or_odd_is_an_error() {
    let dib = Picture::opaque(4, 4, 1).to_dib();
    assert!(Picture::from_dib(&dib[..dib.len() - 1]).is_err());
    assert!(Picture::from_dib(&dib[..10]).is_err());
    let mut rle = dib;
    rle[16..20].copy_from_slice(&1u32.to_le_bytes());
    assert!(Picture::from_dib(&rle).is_err());
}

#[test]
fn a_difference_names_the_first_pixel_or_the_size() {
    let a = Picture::opaque(8, 4, 1);
    assert_eq!(a.difference(&a), None);
    assert!(
        a.difference(&Picture::opaque(4, 8, 1))
            .expect("sizes")
            .starts_with("size 4x8")
    );
    let mut b = a.clone();
    b.rgba[(2 * 8 + 5) * 4] ^= 1;
    assert!(a.difference(&b).expect("pixel").starts_with("pixel 5,2"));
}

#[test]
fn the_pictures_differ_with_the_seed_and_hold_the_alpha_range() {
    assert_ne!(Picture::opaque(64, 64, 1), Picture::opaque(64, 64, 2));
    let p = Picture::translucent(64, 64, 1);
    assert_eq!(p.rgba[3], 0, "transparent at the top left");
    assert_eq!(p.rgba[p.rgba.len() - 1], 255, "opaque at the bottom right");
}
