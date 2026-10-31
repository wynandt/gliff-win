use std::collections::HashMap;
use std::io::Cursor;

use super::ConvertError;
use super::Render;
use super::WinFormat;
use super::cf_html_to_fragment;
use super::dib_to_png;
use super::html_to_cf_html;
use super::is_private;
use super::local_body;
use super::offer_mimes;
use super::plan_remote;
use super::png_to_dibv5;
use super::raw_limit;
use super::remote_is_sensitive;
use super::render;
use super::utf8_to_utf16_nul;
use super::utf16_to_utf8_lf;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

// Text.

#[test]
fn text_stops_at_the_nul_and_drops_carriage_returns() {
    let mut raw = utf16("one\r\ntwo\rthree\r\n");
    raw.extend_from_slice(&[0, 0]);
    raw.extend_from_slice(&utf16("ignored"));
    assert_eq!(utf16_to_utf8_lf(&raw), "one\ntwo\rthree\n");
}

#[test]
fn text_keeps_astral_characters_and_marks_lone_surrogates() {
    assert_eq!(utf16_to_utf8_lf(&utf16("héllo 😀 日本")), "héllo 😀 日本");
    let lone = [0x3D, 0xD8, b'a', 0]; // U+D83D, 'a'
    assert_eq!(utf16_to_utf8_lf(&lone), "\u{FFFD}a");
    assert_eq!(utf16_to_utf8_lf(&[b'a', 0, 7]), "a", "odd byte is dropped");
}

#[test]
fn text_to_windows_is_nul_terminated_and_keeps_line_feeds() {
    let raw = utf8_to_utf16_nul("a\nb 😀".as_bytes());
    assert_eq!(&raw[raw.len() - 2..], &[0, 0]);
    assert_eq!(utf16_to_utf8_lf(&raw), "a\nb 😀");
    assert_eq!(raw, [utf16("a\nb 😀"), vec![0, 0]].concat());
}

// HTML.

fn offsets(cf: &str) -> (usize, usize, usize, usize) {
    let value = |key: &str| -> usize {
        cf.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix(':')))
            .expect("header key")
            .parse()
            .expect("header number")
    };
    (
        value("StartHTML"),
        value("EndHTML"),
        value("StartFragment"),
        value("EndFragment"),
    )
}

#[test]
fn html_without_a_document_is_wrapped_with_exact_offsets() {
    let cf = html_to_cf_html("<b>héllo</b>".as_bytes());
    let text = std::str::from_utf8(&cf).expect("utf-8");
    assert!(text.starts_with("Version:0.9\r\nStartHTML:0000000105\r\n"));
    let (start_html, end_html, start, end) = offsets(text);
    assert_eq!(start_html, 105);
    assert_eq!(end_html, cf.len());
    assert_eq!(&cf[start..end], "<b>héllo</b>".as_bytes());
    assert_eq!(&cf[start_html..start_html + 6], b"<html>");
    assert!(cf[..start].ends_with(b"<!--StartFragment-->"));
    assert!(cf[end..].starts_with(b"<!--EndFragment-->"));
}

#[test]
fn html_with_a_body_gets_the_body_as_the_fragment() {
    let doc = "<HTML><head><title>t</title></head><BODY class=\"x\"><p>hi</p></BODY></HTML>";
    let cf = html_to_cf_html(doc.as_bytes());
    let (_, end_html, start, end) = offsets(std::str::from_utf8(&cf).expect("utf-8"));
    assert_eq!(&cf[start..end], b"<p>hi</p>");
    assert_eq!(end_html, cf.len());
    assert!(
        cf.windows(doc.len() - 9)
            .any(|w| w.starts_with(b"<HTML><head>"))
    );
}

#[test]
fn html_with_fragment_comments_is_kept_whole() {
    let doc = "<html><body>a<!--StartFragment-->mid<!--EndFragment-->z</body></html>";
    let cf = html_to_cf_html(doc.as_bytes());
    let (start_html, _, start, end) = offsets(std::str::from_utf8(&cf).expect("utf-8"));
    assert_eq!(&cf[start..end], b"mid");
    assert_eq!(&cf[start_html..], doc.as_bytes());
}

#[test]
fn html_round_trips_through_both_directions() {
    for fragment in ["<b>x</b>", "plain text", "<p>日本 😀</p>", ""] {
        let cf = html_to_cf_html(fragment.as_bytes());
        let back = cf_html_to_fragment(&cf);
        let expected = format!(
            "<meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\">{fragment}"
        );
        assert_eq!(String::from_utf8(back).expect("utf-8"), expected);
    }
}

#[test]
fn html_from_windows_follows_the_header_and_ignores_a_trailing_nul() {
    let mut cf = html_to_cf_html(b"<i>q</i>");
    cf.push(0);
    let back = cf_html_to_fragment(&cf);
    assert!(back.ends_with(b"<i>q</i>"));
}

#[test]
fn html_from_windows_survives_a_bad_header() {
    let lying = b"Version:0.9\r\nStartFragment:0000000900\r\nEndFragment:0000000100\r\n<html><!--StartFragment-->ok<!--EndFragment--></html>";
    assert!(cf_html_to_fragment(lying).ends_with(b">ok"));
    let bare = b"<p>no header</p>";
    assert!(cf_html_to_fragment(bare).ends_with(b"<p>no header</p>"));
    let start_html_only =
        b"Version:0.9\r\nStartHTML:0000000055\r\nEndHTML:0000000067\r\n<p>whole</p>";
    assert!(cf_html_to_fragment(start_html_only).ends_with(b"<p>whole</p>"));
    assert!(!cf_html_to_fragment(b"").is_empty(), "the meta tag alone");
}

// Images.

/// A DIB as `BITMAPINFOHEADER` plus pixel rows, built by hand.
fn info_header(width: i32, height: i32, bits: u16, compression: u32, colors: u32) -> Vec<u8> {
    let mut h = Vec::new();
    for value in [40u32, width as u32, height as u32] {
        h.extend_from_slice(&value.to_le_bytes());
    }
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&bits.to_le_bytes());
    for value in [compression, 0, 0, 0, colors, 0] {
        h.extend_from_slice(&value.to_le_bytes());
    }
    h
}

fn decode_png(png: &[u8]) -> (png::OutputInfo, Vec<u8>) {
    let mut reader = png::Decoder::new(Cursor::new(png))
        .read_info()
        .expect("png header");
    let buf_len = reader.output_buffer_size().expect("size fits in memory");
    let mut buf = vec![0; buf_len];
    let info = reader.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    (info, buf)
}

#[test]
fn dib_24_bit_bottom_up_is_flipped_and_swizzled() {
    // 2x2, rows padded to 8 bytes. Bottom row first: red, green; then blue, white.
    let mut dib = info_header(2, 2, 24, 0, 0);
    dib.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // B,G,R: red; green
    dib.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // blue; white
    let (info, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(
        pixels,
        [0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0],
        "top row is blue, white; bottom row is red, green"
    );
}

#[test]
fn dib_top_down_keeps_row_order() {
    let mut dib = info_header(1, -2, 24, 0, 0);
    dib.extend_from_slice(&[0, 0, 255, 0]); // red
    dib.extend_from_slice(&[0, 255, 0, 0]); // green
    let (_, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(pixels, [255, 0, 0, 0, 255, 0]);
}

#[test]
fn dib_32_bit_rgb_is_opaque_even_with_a_zero_fourth_byte() {
    let mut dib = info_header(1, 1, 32, 0, 0);
    dib.extend_from_slice(&[10, 20, 30, 0]);
    let (info, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(pixels, [30, 20, 10]);
}

#[test]
fn dib_bitfields_with_alpha_mask_keeps_alpha() {
    let mut dib = info_header(2, -1, 32, 6, 0); // BI_ALPHABITFIELDS
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        dib.extend_from_slice(&mask.to_le_bytes());
    }
    dib.extend_from_slice(&[1, 2, 3, 128, 4, 5, 6, 255]);
    let (info, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(pixels, [3, 2, 1, 128, 6, 5, 4, 255]);
}

#[test]
fn dib_bitfields_with_all_zero_alpha_is_opaque() {
    let mut dib = info_header(1, 1, 32, 6, 0);
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        dib.extend_from_slice(&mask.to_le_bytes());
    }
    dib.extend_from_slice(&[1, 2, 3, 0]);
    let (info, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(pixels, [3, 2, 1]);
}

#[test]
fn dib_16_bit_565_masks_scale_to_8_bits() {
    let mut dib = info_header(1, 1, 16, 3, 0); // BI_BITFIELDS
    for mask in [0xF800u32, 0x07E0, 0x001F] {
        dib.extend_from_slice(&mask.to_le_bytes());
    }
    dib.extend_from_slice(&0xF81Fu16.to_le_bytes()); // magenta
    dib.extend_from_slice(&[0, 0]);
    let (_, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(pixels, [255, 0, 255]);
}

#[test]
fn dib_4_bit_palette_is_read_high_nibble_first() {
    let mut dib = info_header(2, 1, 4, 0, 2);
    dib.extend_from_slice(&[0, 0, 255, 0]); // index 0: red (B,G,R,_)
    dib.extend_from_slice(&[255, 0, 0, 0]); // index 1: blue
    dib.extend_from_slice(&[0x10, 0, 0, 0]); // pixels: 1, 0
    let (_, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
    assert_eq!(pixels, [0, 0, 255, 255, 0, 0]);
}

#[test]
fn dib_errors_are_reported_not_panicked() {
    let image = |r: Result<Vec<u8>, ConvertError>| matches!(r, Err(ConvertError::Image(_)));
    assert!(image(dib_to_png(&[])));
    assert!(image(dib_to_png(&[0; 10])));
    // The header promises more pixels than there are.
    let mut short = info_header(10, 10, 24, 0, 0);
    short.extend_from_slice(&[0; 30]);
    assert!(image(dib_to_png(&short)));
    // RLE, huge and empty sizes.
    assert!(image(dib_to_png(&info_header(1, 1, 8, 1, 0))));
    assert!(image(dib_to_png(&info_header(100_000, 100_000, 24, 0, 0))));
    assert!(image(dib_to_png(&info_header(0, 1, 24, 0, 0))));
    assert!(image(dib_to_png(&info_header(1, i32::MIN, 24, 0, 0))));
    assert!(image(dib_to_png(&info_header(1, 1, 24, 3, 0))));
}

fn make_png(color: png::ColorType, width: u32, height: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    writer.write_image_data(data).expect("data");
    writer.finish().expect("finish");
    out
}

#[test]
fn png_to_dibv5_has_a_v5_header_and_bottom_up_bgra() {
    // Top row red, bottom row half-transparent blue.
    let png = make_png(
        png::ColorType::Rgba,
        1,
        2,
        &[255, 0, 0, 255, 0, 0, 255, 128],
    );
    let dib = png_to_dibv5(&png).expect("convert");
    assert_eq!(dib.len(), 124 + 8);
    let word = |at: usize| u32::from_le_bytes(dib[at..at + 4].try_into().expect("4 bytes"));
    assert_eq!(word(0), 124, "header size");
    assert_eq!(word(4), 1, "width");
    assert_eq!(word(8), 2, "positive height: bottom-up");
    assert_eq!(word(12), 1 | (32 << 16), "planes, bit count");
    assert_eq!(word(16), 3, "BI_BITFIELDS");
    assert_eq!(word(20), 8, "image size");
    assert_eq!(
        [word(40), word(44), word(48), word(52)],
        [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]
    );
    assert_eq!(word(56), 0x7352_4742, "sRGB");
    assert_eq!(
        &dib[124..],
        [255, 0, 0, 128, 0, 0, 255, 255],
        "bottom row first"
    );
}

#[test]
fn png_to_dibv5_expands_gray_and_rgb() {
    let gray = png_to_dibv5(&make_png(png::ColorType::Grayscale, 1, 1, &[7])).expect("gray");
    assert_eq!(&gray[124..], [7, 7, 7, 255]);
    let rgb = png_to_dibv5(&make_png(png::ColorType::Rgb, 1, 1, &[1, 2, 3])).expect("rgb");
    assert_eq!(&rgb[124..], [3, 2, 1, 255]);
    let ga = png_to_dibv5(&make_png(png::ColorType::GrayscaleAlpha, 1, 1, &[9, 40])).expect("ga");
    assert_eq!(&ga[124..], [9, 9, 9, 40]);
}

#[test]
fn png_to_dibv5_rejects_a_header_that_claims_an_impossible_size() {
    // Only a header, claiming u32::MAX x u32::MAX RGBA pixels: the decoded
    // size does not even fit in a usize.
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, u32::MAX, u32::MAX);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    drop(encoder.write_header().expect("header"));
    assert!(matches!(png_to_dibv5(&out), Err(ConvertError::Image(_))));
}

#[test]
fn png_to_dibv5_rejects_garbage() {
    assert!(matches!(
        png_to_dibv5(b"not a png"),
        Err(ConvertError::Image(_))
    ));
    assert!(matches!(png_to_dibv5(&[]), Err(ConvertError::Image(_))));
}

#[test]
fn a_picture_survives_png_to_dib_and_back() {
    let pixels: Vec<u8> = (0..4 * 3 * 4).map(|i| (i * 7) as u8).collect();
    let original = make_png(png::ColorType::Rgba, 4, 3, &pixels);
    let dib = png_to_dibv5(&original).expect("to dib");
    let back = dib_to_png(&dib).expect("to png");
    let (info, decoded) = decode_png(&back);
    assert_eq!((info.width, info.height), (4, 3));
    assert_eq!(decoded, pixels);
}

// Formats and offers.

#[test]
fn offers_put_text_first_and_dedupe() {
    let formats = [
        WinFormat::Png,
        WinFormat::Hdrop,
        WinFormat::Html,
        WinFormat::UnicodeText,
        WinFormat::Dib,
        WinFormat::DibV5,
        WinFormat::Registered("application/x-custom".to_owned()),
        WinFormat::Registered("Shell IDList Array".to_owned()),
        WinFormat::Registered(super::MARKER_FORMAT.to_owned()),
        WinFormat::Rtf,
        WinFormat::Jfif,
        WinFormat::Gif,
    ];
    assert_eq!(
        offer_mimes(&formats),
        strings(&[
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "STRING",
            "TEXT",
            "text/html",
            "text/rtf",
            "application/rtf",
            "image/png",
            "image/jpeg",
            "image/gif",
            "application/x-custom",
        ])
    );
    assert!(offer_mimes(&[WinFormat::Hdrop]).is_empty());
}

#[test]
fn privacy_is_read_from_the_marker_formats() {
    let plain = [WinFormat::UnicodeText];
    let mut none = |_: &str| None;
    assert!(!is_private(&plain, &mut none));

    let excluded = [
        WinFormat::UnicodeText,
        WinFormat::from_registered(super::EXCLUDE_FORMAT),
    ];
    assert!(is_private(&excluded, &mut none));
    let ignored = [WinFormat::from_registered(super::VIEWER_IGNORE_FORMAT)];
    assert!(is_private(&ignored, &mut none));

    let history = [WinFormat::from_registered(super::HISTORY_FORMAT)];
    assert!(is_private(&history, &mut |_| Some(0)));
    assert!(!is_private(&history, &mut |_| Some(1)));
    assert!(!is_private(&history, &mut none));
    let cloud = [WinFormat::from_registered(super::CLOUD_FORMAT)];
    assert!(is_private(&cloud, &mut |name| (name
        == super::CLOUD_FORMAT)
        .then_some(0)));
}

fn store(items: &[(WinFormat, Vec<u8>)]) -> (Vec<WinFormat>, HashMap<WinFormat, Vec<u8>>) {
    (
        items.iter().map(|(f, _)| f.clone()).collect(),
        items.iter().cloned().collect(),
    )
}

#[test]
fn local_bodies_are_converted_per_flavour() {
    let mut dib = info_header(1, 1, 24, 0, 0);
    dib.extend_from_slice(&[0, 0, 255, 0]);
    let (formats, data) = store(&[
        (
            WinFormat::UnicodeText,
            [utf16("a\r\nb"), vec![0, 0]].concat(),
        ),
        (WinFormat::Html, html_to_cf_html(b"<b>x</b>")),
        (WinFormat::Rtf, b"{\\rtf1 hi}\0".to_vec()),
        (WinFormat::Dib, dib),
        (WinFormat::Jfif, vec![0xFF, 0xD8]),
        (WinFormat::Gif, b"GIF89a".to_vec()),
        (
            WinFormat::Registered("application/x-custom".to_owned()),
            vec![1, 2],
        ),
    ]);
    let mut read = |f: &WinFormat| data.get(f).cloned();
    let get = |mime: &str, read: &mut dyn FnMut(&WinFormat) -> Option<Vec<u8>>| {
        local_body(mime, &formats, 1 << 20, read)
    };
    assert_eq!(
        get("text/plain;charset=utf-8", &mut read),
        Ok(b"a\nb".to_vec())
    );
    assert_eq!(get("text/plain", &mut read), Ok(b"a\nb".to_vec()));
    assert!(
        get("text/html", &mut read)
            .expect("html")
            .ends_with(b"<b>x</b>")
    );
    assert_eq!(get("text/rtf", &mut read), Ok(b"{\\rtf1 hi}".to_vec()));
    assert_eq!(
        get("application/rtf", &mut read),
        Ok(b"{\\rtf1 hi}".to_vec())
    );
    assert_eq!(get("image/jpeg", &mut read), Ok(vec![0xFF, 0xD8]));
    assert_eq!(get("image/gif", &mut read), Ok(b"GIF89a".to_vec()));
    assert_eq!(get("application/x-custom", &mut read), Ok(vec![1, 2]));
    let png = get("image/png", &mut read).expect("png from dib");
    assert_eq!(decode_png(&png).1, [255, 0, 0]);
    assert_eq!(
        get("application/unlisted", &mut read),
        Err(ConvertError::Unknown("application/unlisted".to_owned()))
    );
}

#[test]
fn local_png_prefers_the_png_format_then_v5_then_dib() {
    let formats = [WinFormat::Dib, WinFormat::Png, WinFormat::DibV5];
    let mut order = Vec::new();
    let mut read = |f: &WinFormat| {
        order.push(f.clone());
        Some(make_png(png::ColorType::Rgb, 1, 1, &[1, 2, 3]))
    };
    let body = local_body("image/png", &formats, 1 << 20, &mut read).expect("png");
    assert_eq!(order, [WinFormat::Png]);
    assert_eq!(decode_png(&body).1, [1, 2, 3]);
}

#[test]
fn local_body_over_the_limit_is_refused_and_missing_data_is_reported() {
    let formats = [WinFormat::Gif, WinFormat::UnicodeText];
    let mut big = |_: &WinFormat| Some(vec![0u8; 11]);
    assert_eq!(
        local_body("image/gif", &formats, 10, &mut big),
        Err(ConvertError::TooLarge(11, 10))
    );
    let mut empty = |_: &WinFormat| None;
    assert!(matches!(
        local_body("text/plain", &formats, 10, &mut empty),
        Err(ConvertError::Missing(_))
    ));
    // What is copied off the clipboard before that check is bounded too, but
    // not below what a body of the limit needs: text as UTF-16, or a bitmap
    // at the pixel limit.
    let item = usize::try_from(gliff_proto::clipboard::MAX_ITEM).expect("fits");
    assert!(raw_limit(&WinFormat::UnicodeText) >= 2 * item);
    assert!(raw_limit(&WinFormat::Dib) >= 4 * 64 * 1024 * 1024);
    assert!(raw_limit(&WinFormat::Png) < 1 << 28);
}

#[test]
fn remote_offers_plan_every_useful_windows_format_once() {
    let offered = strings(&[
        "text/uri-list",
        "TEXT",
        "text/plain",
        "text/html",
        "application/rtf",
        "image/png",
        "image/jpeg",
        "image/webp",
        "x-kde-passwordManagerHint",
        "text/plain",
    ]);
    let plans = plan_remote(&offered);
    let summary: Vec<(WinFormat, &str, Render)> = plans
        .iter()
        .map(|p| (p.format.clone(), p.mime.as_str(), p.render))
        .collect();
    assert_eq!(
        summary,
        [
            (WinFormat::UnicodeText, "text/plain", Render::Text),
            (WinFormat::Html, "text/html", Render::Html),
            (WinFormat::Rtf, "application/rtf", Render::Rtf),
            (WinFormat::Png, "image/png", Render::Passthrough),
            (WinFormat::DibV5, "image/png", Render::PngToDib),
            (WinFormat::Jfif, "image/jpeg", Render::Passthrough),
            (
                WinFormat::Registered("image/webp".to_owned()),
                "image/webp",
                Render::Passthrough
            ),
            (
                WinFormat::Registered("x-kde-passwordManagerHint".to_owned()),
                "x-kde-passwordManagerHint",
                Render::Passthrough
            ),
        ]
    );
    assert!(remote_is_sensitive(&offered));
    assert!(!remote_is_sensitive(&strings(&["text/plain"])));
}

#[test]
fn remote_planning_is_bounded() {
    let many: Vec<String> = (0..200).map(|i| format!("application/x-{i}")).collect();
    assert_eq!(plan_remote(&many).len(), 32);
    let long = vec!["a/".to_owned() + &"b".repeat(300)];
    assert!(plan_remote(&long).is_empty());
    assert!(plan_remote(&[]).is_empty());
    assert!(plan_remote(&strings(&["text/uri-list", "x-special/gnome-copied-files"])).is_empty());
}

#[test]
fn rendering_makes_the_windows_bytes() {
    assert_eq!(
        render(Render::Text, b"hi").expect("text"),
        [b'h', 0, b'i', 0, 0, 0]
    );
    assert_eq!(render(Render::Rtf, b"{x}").expect("rtf"), b"{x}\0");
    assert_eq!(render(Render::Passthrough, b"ab").expect("raw"), b"ab");
    assert!(
        render(Render::Html, b"<b>")
            .expect("html")
            .starts_with(b"Version:0.9")
    );
    let png = make_png(png::ColorType::Rgb, 1, 1, &[9, 8, 7]);
    assert_eq!(render(Render::PngToDib, &png).expect("dib").len(), 128);
    assert!(render(Render::PngToDib, b"junk").is_err());
}

// Images: padding, sizes and the preferred format.

/// A 24-bit bottom-up DIB of `width` x `height` whose pixel (x, y) from the
/// top is `pixel(x, y)` as (r, g, b); rows are padded to 4 bytes.
fn rgb_dib(width: usize, height: usize, mut pixel: impl FnMut(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let mut dib = info_header(width as i32, height as i32, 24, 0, 0);
    let stride = (width * 3).div_ceil(4) * 4;
    for y in (0..height).rev() {
        let start = dib.len();
        for x in 0..width {
            let [r, g, b] = pixel(x, y);
            dib.extend_from_slice(&[b, g, r]);
        }
        dib.resize(start + stride, 0);
    }
    dib
}

#[test]
fn every_padding_of_a_24_bit_row_is_read_exactly() {
    // Widths 1 to 4 pad a row with 1, 2, 3 or no bytes.
    for width in 1..=9usize {
        let pixel = |x: usize, y: usize| [(x * 20 + y) as u8, (y * 30) as u8, (x * 9) as u8];
        let dib = rgb_dib(width, 3, pixel);
        let (info, pixels) = decode_png(&dib_to_png(&dib).expect("convert"));
        assert_eq!((info.width as usize, info.height), (width, 3));
        let expected: Vec<u8> = (0..3)
            .flat_map(|y| (0..width).flat_map(move |x| pixel(x, y)))
            .collect();
        assert_eq!(pixels, expected, "width {width}");
    }
}

#[test]
fn alpha_survives_the_trip_in_both_directions() {
    let rgba = [10, 20, 30, 0, 40, 50, 60, 77, 70, 80, 90, 255];
    let dib = png_to_dibv5(&make_png(png::ColorType::Rgba, 3, 1, &rgba)).expect("dib");
    let (info, pixels) = decode_png(&dib_to_png(&dib).expect("png"));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(pixels, rgba);
}

#[test]
fn an_opaque_picture_comes_back_without_an_alpha_channel() {
    let rgb = [1, 2, 3, 4, 5, 6];
    let dib = png_to_dibv5(&make_png(png::ColorType::Rgb, 2, 1, &rgb)).expect("dib");
    let (info, pixels) = decode_png(&dib_to_png(&dib).expect("png"));
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(pixels, rgb);
}

#[test]
fn a_screen_sized_picture_converts_both_ways() {
    // 4K at 24 bits is 24 MiB; as 32-bit pixels it is over the item limit,
    // so a copy of it only fits as PNG.
    let (width, height) = (3840usize, 2160usize);
    let pixel = |x: usize, y: usize| [(x / 16) as u8, (y / 9) as u8, ((x ^ y) / 64) as u8];
    let dib = rgb_dib(width, height, pixel);
    let png = dib_to_png(&dib).expect("convert");
    assert!(
        png.len() < 8 << 20,
        "a smooth picture compresses: {}",
        png.len()
    );
    let back = png_to_dibv5(&png).expect("back to a bitmap");
    assert_eq!(back.len(), 124 + width * height * 4);
    // Spot-check corners: the DIB is bottom-up, B-G-R-A.
    let at = |x: usize, y: usize| {
        let offset = 124 + ((height - 1 - y) * width + x) * 4;
        [back[offset + 2], back[offset + 1], back[offset]]
    };
    for (x, y) in [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ] {
        assert_eq!(at(x, y), pixel(x, y), "pixel {x},{y}");
    }
}

#[test]
fn a_decoded_picture_over_the_old_decoder_limit_is_read() {
    // 8K by 4K as RGBA is 128 MiB, over the PNG crate's default of 64 MiB.
    let (width, height) = (8192u32, 4096u32);
    let pixels = vec![200u8; width as usize * height as usize * 4];
    let png = make_png(png::ColorType::Rgba, width, height, &pixels);
    let dib = png_to_dibv5(&png).expect("convert");
    assert_eq!(dib.len(), 124 + pixels.len());
}

#[test]
fn a_picture_whose_png_is_over_the_item_limit_is_refused() {
    // Noise does not compress, so the PNG is about as big as the bitmap.
    let mut seed = 0x1234_5678u32;
    let dib = rgb_dib(64, 64, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [a, b, c, _] = seed.to_le_bytes();
        [a, b, c]
    });
    let png = dib_to_png(&dib).expect("convert");
    let formats = [WinFormat::Dib];
    let mut read = |_: &WinFormat| Some(dib.clone());
    assert!(local_body("image/png", &formats, png.len(), &mut read).is_ok());
    assert_eq!(
        local_body("image/png", &formats, png.len() - 1, &mut read),
        Err(ConvertError::TooLarge(png.len(), png.len() - 1))
    );
}

#[test]
fn a_bitmap_over_the_pixel_limit_is_refused_before_it_is_allocated() {
    // 16384 x 16384 is 256 Mpx; the header alone says so.
    let header = info_header(16384, 16384, 24, 0, 0);
    assert!(matches!(dib_to_png(&header), Err(ConvertError::Image(_))));
    let mut too_big = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut too_big, 16384, 16384);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        let mut writer = encoder.write_header().expect("header");
        writer
            .write_image_data(&vec![0u8; 16384 / 8 * 16384])
            .expect("data");
        writer.finish().expect("finish");
    }
    assert!(matches!(
        png_to_dibv5(&too_big),
        Err(ConvertError::Image(_))
    ));
}

#[test]
fn an_unreadable_registered_png_falls_back_to_the_bitmap() {
    let dib = rgb_dib(2, 1, |x, _| [x as u8, 0, 0]);
    let formats = [WinFormat::Png, WinFormat::DibV5, WinFormat::Dib];
    let mut read = |f: &WinFormat| (*f == WinFormat::DibV5).then(|| dib.clone());
    let body = local_body("image/png", &formats, 1 << 20, &mut read).expect("converted");
    assert_eq!(decode_png(&body).1, [0, 0, 0, 1, 0, 0]);
    // The plain bitmap is the last resort.
    let mut only_dib = |f: &WinFormat| (*f == WinFormat::Dib).then(|| dib.clone());
    assert!(local_body("image/png", &formats, 1 << 20, &mut only_dib).is_ok());
}
