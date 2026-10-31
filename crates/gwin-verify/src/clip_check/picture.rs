//! The pictures the image cases copy, and the formats they travel in.
//!
//! This is the gate's own reading and writing of PNG and DIB, written
//! apart from the client's converters so that the gate does not vouch for
//! the code it tests with that code.

use anyhow::Context;
use anyhow::bail;

/// A picture as 8-bit RGBA rows, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const INFO_HEADER: usize = 40;
/// Where a V3 or later header keeps the alpha mask.
const ALPHA_MASK_AT: usize = 52;
const V3_HEADER: usize = 56;

impl Picture {
    /// An opaque picture: red grows to the right, green downward, and blue
    /// steps in tiles that depend on `seed`, so a flip, a swapped channel or
    /// a picture from an earlier run all show.
    pub fn opaque(width: u32, height: u32, seed: u32) -> Self {
        Self::build(width, height, |x, y| {
            let [r, g, b] = Self::colour(width, height, seed, x, y);
            [r, g, b, 255]
        })
    }

    /// A picture whose alpha runs from 0 at the top left corner to 255 at
    /// the bottom right.
    pub fn translucent(width: u32, height: u32, seed: u32) -> Self {
        Self::build(width, height, |x, y| {
            let [r, g, b] = Self::colour(width, height, seed, x, y);
            let last = (width - 1) + (height - 1);
            let a = ((x + y) * 255)
                .checked_div(last)
                .map_or(255, |a| u8::try_from(a).unwrap_or(255));
            [r, g, b, a]
        })
    }

    fn colour(width: u32, height: u32, seed: u32, x: u32, y: u32) -> [u8; 3] {
        let along = |at: u32, size: u32| u8::try_from(at * 255 / (size - 1).max(1)).unwrap_or(255);
        let tile = (x / 32)
            .wrapping_add(y / 32)
            .wrapping_mul(37)
            .wrapping_add(seed);
        [along(x, width), along(y, height), tile.to_le_bytes()[0]]
    }

    fn build(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Self {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&pixel(x, y));
            }
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    fn is_opaque(&self) -> bool {
        self.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
    }

    /// The picture as PNG: RGB when opaque, else RGBA.
    pub fn to_png(&self) -> anyhow::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let data: Vec<u8> = if self.is_opaque() {
            encoder.set_color(png::ColorType::Rgb);
            self.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect()
        } else {
            encoder.set_color(png::ColorType::Rgba);
            self.rgba.clone()
        };
        let mut writer = encoder.write_header().context("png header")?;
        writer.write_image_data(&data).context("png data")?;
        writer.finish().context("png end")?;
        Ok(out)
    }

    /// A PNG of any colour type, as RGBA.
    pub fn from_png(data: &[u8]) -> anyhow::Result<Self> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().context("not a PNG")?;
        let buf_len = reader
            .output_buffer_size()
            .context("PNG decoded size is too large")?;
        let mut buf = vec![0; buf_len];
        let info = reader.next_frame(&mut buf).context("PNG data")?;
        let channels = match info.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => bail!("a palette PNG after expansion"),
        };
        let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        for row in buf[..info.buffer_size()].chunks_exact(info.line_size) {
            for px in row.chunks_exact(channels).take(info.width as usize) {
                rgba.extend_from_slice(&match channels {
                    1 => [px[0], px[0], px[0], 255],
                    2 => [px[0], px[0], px[0], px[1]],
                    3 => [px[0], px[1], px[2], 255],
                    _ => [px[0], px[1], px[2], px[3]],
                });
            }
        }
        Ok(Self {
            width: info.width,
            height: info.height,
            rgba,
        })
    }

    /// The picture as a `CF_DIB`: a 32-bit `BI_RGB` bitmap, bottom-up, with
    /// the unused fourth byte zero, as a screen snip leaves it.
    pub fn to_dib(&self) -> Vec<u8> {
        let mut dib = Vec::with_capacity(INFO_HEADER + self.rgba.len());
        dib.extend_from_slice(&(INFO_HEADER as u32).to_le_bytes());
        dib.extend_from_slice(&self.width.to_le_bytes());
        dib.extend_from_slice(&self.height.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&BI_RGB.to_le_bytes());
        dib.extend_from_slice(&(self.rgba.len() as u32).to_le_bytes());
        dib.extend_from_slice(&[0; 16]);
        for row in self.rgba.chunks_exact(self.width as usize * 4).rev() {
            for p in row.as_chunks::<4>().0 {
                dib.extend_from_slice(&[p[2], p[1], p[0], 0]);
            }
        }
        dib
    }

    /// A 24-bit or 32-bit bitmap, top-down or bottom-up, as a `CF_DIB` or
    /// `CF_DIBV5` holds it. 32-bit pixels have alpha only with a V3 or later
    /// header, an alpha mask, and some alpha that is not zero.
    pub fn from_dib(dib: &[u8]) -> anyhow::Result<Self> {
        let word = |at: usize| -> anyhow::Result<u32> {
            let bytes = dib
                .get(at..at + 4)
                .context("the bitmap header is cut short")?;
            Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        };
        let size = word(0)? as usize;
        let width = word(4)? as i32;
        let raw_height = word(8)? as i32;
        let bits = word(12)? >> 16;
        let compression = word(16)?;
        if width <= 0 || raw_height == 0 || raw_height == i32::MIN {
            bail!("bitmap size {width}x{raw_height}");
        }
        if !matches!(bits, 24 | 32) || !matches!(compression, BI_RGB | BI_BITFIELDS) {
            bail!("{bits} bits, compression {compression}: not a bitmap the gate reads");
        }
        let (width, height) = (width as usize, raw_height.unsigned_abs() as usize);
        let bytes_per_pixel = (bits / 8) as usize;
        let stride = (width * bytes_per_pixel).div_ceil(4) * 4;
        let mut at = size;
        if size == INFO_HEADER && compression == BI_BITFIELDS {
            at += 12;
        }
        let rows = dib
            .get(at..at + stride * height)
            .context("the bitmap has fewer pixels than its header says")?;
        let alpha_mask = if bits == 32 && compression == BI_BITFIELDS && size >= V3_HEADER {
            word(ALPHA_MASK_AT)?
        } else {
            0
        };
        let mut rgba = vec![0; width * height * 4];
        for y in 0..height {
            let source = if raw_height < 0 { y } else { height - 1 - y };
            let row = &rows[source * stride..(source + 1) * stride];
            for x in 0..width {
                let p = &row[x * bytes_per_pixel..];
                let a = if alpha_mask == 0xFF00_0000 { p[3] } else { 255 };
                let out = (y * width + x) * 4;
                rgba[out..out + 4].copy_from_slice(&[p[2], p[1], p[0], a]);
            }
        }
        // A program that does not use alpha leaves it zero everywhere.
        if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            for p in rgba.as_chunks_mut::<4>().0 {
                p[3] = 255;
            }
        }
        Ok(Self {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    }

    /// A line for a failed step: how `other` differs from `self`, or `None`
    /// when they are the same picture.
    pub fn difference(&self, other: &Self) -> Option<String> {
        if (self.width, self.height) != (other.width, other.height) {
            return Some(format!(
                "size {}x{}, expected {}x{}",
                other.width, other.height, self.width, self.height
            ));
        }
        let at = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(other.rgba.as_chunks::<4>().0)
            .position(|(a, b)| a != b)?;
        let (x, y) = (at as u32 % self.width, at as u32 / self.width);
        let pixel = |p: &Self| p.rgba[at * 4..at * 4 + 4].to_vec();
        Some(format!(
            "pixel {x},{y} is {:?}, expected {:?}",
            pixel(other),
            pixel(self)
        ))
    }
}

#[cfg(test)]
mod tests;
