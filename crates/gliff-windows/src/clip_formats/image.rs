//! Images between Windows device-independent bitmaps and PNG.
//!
//! `CF_DIB` and `CF_DIBV5` are a bitmap header, optional colour masks or a
//! palette, and the pixel rows (bottom-up unless the height is negative,
//! each row padded to 4 bytes). PNG is what Linux programs exchange.

use std::io::Cursor;

use super::ConvertError;

/// The largest picture converted, in pixels. A picture of 64 Mpx is 256 MiB
/// as 32-bit pixels; hostile sizes are refused before anything is
/// allocated.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// The largest bitmap read off the clipboard: [`MAX_PIXELS`] as 32-bit
/// pixels, plus a megabyte for the header, masks and palette.
pub const MAX_DIB_BYTES: usize = 4 * MAX_PIXELS as usize + (1 << 20);

/// The most memory the PNG decoder may use: a 64 Mpx picture of 8-bit RGBA
/// plus its row buffers. The decoder's default of 64 MiB would refuse a
/// large screenshot.
const MAX_DECODED_BYTES: usize = 2 * 4 * MAX_PIXELS as usize;

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;

/// `BITMAPINFOHEADER`, and the larger headers that start with it.
const INFO_HEADER: usize = 40;
const V2_HEADER: usize = 52;
const V3_HEADER: usize = 56;
const V5_HEADER: usize = 124;
/// `LCS_sRGB`, the colour space tag in a V4 and V5 header.
const LCS_SRGB: u32 = 0x7352_4742;
/// `LCS_GM_IMAGES`.
const INTENT_IMAGES: u32 = 4;

/// What the header says about the pixels.
struct Layout {
    width: usize,
    height: usize,
    top_down: bool,
    bits: u16,
    compression: u32,
    /// Where the pixel rows start in the payload.
    pixels_at: usize,
    /// Red, green, blue and alpha masks for 16 and 32 bit pixels.
    masks: [u32; 4],
    /// Palette entries (B, G, R) for 1, 4 and 8 bit pixels.
    palette: Vec<[u8; 3]>,
}

/// A DIB (`CF_DIB` or `CF_DIBV5` payload) as a PNG.
///
/// 32-bit pixels carry alpha only when the header has an alpha mask (a
/// `BI_BITFIELDS` bitmap); a plain `BI_RGB` bitmap is opaque, as its unused
/// byte is often zero. A bitmap whose alpha is zero everywhere is opaque too.
/// 1, 4, 8, 16, 24 and 32 bit uncompressed bitmaps are read; RLE and
/// embedded JPEG or PNG are not.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let layout = parse_header(dib)?;
    let stride = (layout.width * usize::from(layout.bits)).div_ceil(32) * 4;
    let needed = layout
        .pixels_at
        .checked_add(stride * layout.height)
        .ok_or_else(|| ConvertError::Image("bitmap size overflows".to_owned()))?;
    if dib.len() < needed {
        return Err(ConvertError::Image(format!(
            "bitmap has {} bytes, its header needs {needed}",
            dib.len()
        )));
    }
    let rows = &dib[layout.pixels_at..needed];
    let has_alpha_mask = layout.masks[3] != 0
        && matches!(layout.compression, BI_BITFIELDS | BI_ALPHABITFIELDS)
        && layout.bits == 32;
    let mut rgba = vec![0u8; layout.width * layout.height * 4];
    for y in 0..layout.height {
        let source_row = if layout.top_down {
            y
        } else {
            layout.height - 1 - y
        };
        let row = &rows[source_row * stride..(source_row + 1) * stride];
        let out = &mut rgba[y * layout.width * 4..(y + 1) * layout.width * 4];
        for (x, pixel) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *pixel = read_pixel(&layout, row, x, has_alpha_mask);
        }
    }
    let mut alpha = false;
    if has_alpha_mask {
        let pixels = rgba.as_chunks_mut::<4>().0;
        let all_zero = pixels.iter().all(|p| p[3] == 0);
        // Zero everywhere means the program did not use alpha; 255
        // everywhere means it is not needed. Either way the picture is
        // opaque and the PNG is smaller without a channel.
        if all_zero {
            for pixel in pixels.iter_mut() {
                pixel[3] = 255;
            }
        }
        alpha = !all_zero && pixels.iter().any(|p| p[3] != 255);
    }
    encode_png(&rgba, layout.width, layout.height, alpha)
}

fn parse_header(dib: &[u8]) -> Result<Layout, ConvertError> {
    let word = |at: usize| -> Result<u32, ConvertError> {
        dib.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| ConvertError::Image("bitmap header is cut short".to_owned()))
    };
    let size = word(0)? as usize;
    if ![INFO_HEADER, V2_HEADER, V3_HEADER, 108, V5_HEADER].contains(&size) {
        return Err(ConvertError::Image(format!(
            "bitmap header of {size} bytes is not supported"
        )));
    }
    let width = word(4)? as i32;
    let raw_height = word(8)? as i32;
    let bits =
        u16::try_from(word(12)? >> 16).map_err(|_| ConvertError::Image("bit count".to_owned()))?;
    let compression = word(16)?;
    let colors_used = word(32)? as usize;
    if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
        return Err(ConvertError::Image(format!(
            "bitmap size {width}x{raw_height} is not valid"
        )));
    }
    let (width, height) = (width as usize, raw_height.unsigned_abs() as usize);
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(ConvertError::Image(format!(
            "bitmap of {width}x{height} is over the {MAX_PIXELS} pixel limit"
        )));
    }
    if !matches!(compression, BI_RGB | BI_BITFIELDS | BI_ALPHABITFIELDS) {
        return Err(ConvertError::Image(format!(
            "bitmap compression {compression} is not supported"
        )));
    }
    if ![1, 4, 8, 16, 24, 32].contains(&bits) {
        return Err(ConvertError::Image(format!(
            "{bits} bits per pixel is not supported"
        )));
    }
    let uses_masks = matches!(compression, BI_BITFIELDS | BI_ALPHABITFIELDS) && bits >= 16;
    if matches!(compression, BI_BITFIELDS | BI_ALPHABITFIELDS) && !matches!(bits, 16 | 32) {
        return Err(ConvertError::Image(format!(
            "colour masks with {bits} bits per pixel"
        )));
    }
    let mut pixels_at = size;
    let mut masks = default_masks(bits);
    if uses_masks {
        masks = [word(40)?, word(44)?, word(48)?, 0];
        if size >= V3_HEADER {
            masks[3] = word(52)?;
        } else if size == INFO_HEADER {
            // The masks follow the header; alpha does only for ALPHABITFIELDS.
            let count = if compression == BI_ALPHABITFIELDS {
                4
            } else {
                3
            };
            if count == 4 {
                masks[3] = word(52)?;
            }
            pixels_at += count * 4;
        }
        if masks[..3].contains(&0) {
            return Err(ConvertError::Image("a colour mask is empty".to_owned()));
        }
    }
    let mut palette = Vec::new();
    if bits <= 8 {
        let entries = if colors_used == 0 {
            1usize << bits
        } else {
            colors_used
        };
        if entries > 256 {
            return Err(ConvertError::Image(format!("{entries} palette entries")));
        }
        for i in 0..entries {
            let at = pixels_at + i * 4;
            let entry = dib
                .get(at..at + 4)
                .ok_or_else(|| ConvertError::Image("palette is cut short".to_owned()))?;
            palette.push([entry[0], entry[1], entry[2]]);
        }
        pixels_at += entries * 4;
    } else if colors_used > 0 && colors_used <= 256 {
        // An optional colour table sits in front of the pixels.
        pixels_at += colors_used * 4;
    }
    Ok(Layout {
        width,
        height,
        top_down: raw_height < 0,
        bits,
        compression,
        pixels_at,
        masks,
        palette,
    })
}

/// Masks for bitmaps without their own: 5-5-5 for 16 bit, B-G-R for 32.
fn default_masks(bits: u16) -> [u32; 4] {
    match bits {
        16 => [0x7C00, 0x03E0, 0x001F, 0],
        _ => [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0],
    }
}

/// One pixel as RGBA.
fn read_pixel(layout: &Layout, row: &[u8], x: usize, has_alpha_mask: bool) -> [u8; 4] {
    match layout.bits {
        1 | 4 | 8 => {
            let bits = usize::from(layout.bits);
            let bit = x * bits;
            let byte = row[bit / 8];
            let shift = 8 - bits - (bit % 8);
            let index = usize::from((byte >> shift) & ((1u16 << bits) - 1) as u8);
            let [b, g, r] = layout.palette.get(index).copied().unwrap_or([0, 0, 0]);
            [r, g, b, 255]
        }
        16 => {
            let value = u32::from(u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]));
            masked(&layout.masks, value, false)
        }
        24 => [row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255],
        _ => {
            let at = x * 4;
            if layout.compression == BI_RGB {
                return [row[at + 2], row[at + 1], row[at], 255];
            }
            let value = u32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]]);
            masked(&layout.masks, value, has_alpha_mask)
        }
    }
}

/// The channels of `value` under `masks`, each scaled to 8 bits.
fn masked(masks: &[u32; 4], value: u32, has_alpha: bool) -> [u8; 4] {
    let channel = |mask: u32| -> u8 {
        if mask == 0 {
            return 0;
        }
        let shift = mask.trailing_zeros();
        let max = u64::from(mask >> shift);
        let part = u64::from((value & mask) >> shift);
        ((part * 255 + max / 2) / max) as u8
    };
    [
        channel(masks[0]),
        channel(masks[1]),
        channel(masks[2]),
        if has_alpha { channel(masks[3]) } else { 255 },
    ]
}

fn encode_png(
    rgba: &[u8],
    width: usize,
    height: usize,
    alpha: bool,
) -> Result<Vec<u8>, ConvertError> {
    let png_error = |e: png::EncodingError| ConvertError::Image(format!("png: {e}"));
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer;
    if alpha {
        encoder.set_color(png::ColorType::Rgba);
        writer = encoder.write_header().map_err(png_error)?;
        writer.write_image_data(rgba).map_err(png_error)?;
    } else {
        encoder.set_color(png::ColorType::Rgb);
        writer = encoder.write_header().map_err(png_error)?;
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        writer.write_image_data(&rgb).map_err(png_error)?;
    }
    writer.finish().map_err(png_error)?;
    Ok(out)
}

/// A PNG as a `CF_DIBV5` payload: a 32-bit `BI_BITFIELDS` bitmap with alpha,
/// bottom-up. Any PNG colour type and depth is read; 16-bit channels drop to
/// 8.
pub fn png_to_dibv5(data: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let png_error = |e: png::DecodingError| ConvertError::Image(format!("png: {e}"));
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    decoder.set_limits(png::Limits {
        bytes: MAX_DECODED_BYTES,
    });
    let mut reader = decoder.read_info().map_err(png_error)?;
    let (width, height) = (reader.info().width as usize, reader.info().height as usize);
    if width == 0 || height == 0 || (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(ConvertError::Image(format!(
            "picture of {width}x{height} is not convertible"
        )));
    }
    let Some(buf_len) = reader.output_buffer_size() else {
        return Err(ConvertError::Image(format!(
            "picture of {width}x{height} is not convertible"
        )));
    };
    let mut buf = vec![0u8; buf_len];
    let info = reader.next_frame(&mut buf).map_err(png_error)?;
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => {
            return Err(ConvertError::Image("palette left unexpanded".to_owned()));
        }
    };
    let mut out = dibv5_header(width, height);
    out.reserve(width * height * 4);
    for y in (0..height).rev() {
        let row = &buf[y * info.line_size..y * info.line_size + width * channels];
        for px in row.chunks_exact(channels) {
            let (r, g, b, a) = match channels {
                1 => (px[0], px[0], px[0], 255),
                2 => (px[0], px[0], px[0], px[1]),
                3 => (px[0], px[1], px[2], 255),
                _ => (px[0], px[1], px[2], px[3]),
            };
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Ok(out)
}

/// A `BITMAPV5HEADER` for a bottom-up 32-bit B-G-R-A bitmap.
fn dibv5_header(width: usize, height: usize) -> Vec<u8> {
    let mut h = Vec::with_capacity(V5_HEADER);
    let mut put = |value: u32| h.extend_from_slice(&value.to_le_bytes());
    put(V5_HEADER as u32);
    put(width as u32);
    put(height as u32);
    put(1 | (32 << 16)); // planes, bit count
    put(BI_BITFIELDS);
    put((width * height * 4) as u32);
    put(0); // x pixels per metre
    put(0); // y pixels per metre
    put(0); // colours used
    put(0); // colours important
    put(0x00FF_0000); // red
    put(0x0000_FF00); // green
    put(0x0000_00FF); // blue
    put(0xFF00_0000); // alpha
    put(LCS_SRGB);
    for _ in 0..9 {
        put(0); // CIEXYZ end points
    }
    for _ in 0..3 {
        put(0); // gamma
    }
    put(INTENT_IMAGES);
    put(0); // profile data
    put(0); // profile size
    put(0); // reserved
    h
}
