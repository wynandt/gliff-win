//! H.264 clips for the decode check: a synthetic desktop encoded here with
//! OpenH264 (Baseline, at most 3840x2160), or a clip file of access units
//! made elsewhere (`scripts/make-clips`: the same desktop through libx264
//! High profile, which also reaches the 3840x2400 panel).
//!
//! File format (`.gwclip`): the 8 magic bytes `GWCLIP01`, width and height
//! as u32 LE, then for each access unit its length as u32 LE and its bytes.

use std::io::Read;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use anyhow::bail;
use anyhow::ensure;
use gliff_windows::h264;
use gliff_windows::h264::LevelVerdict;
use openh264::OpenH264API;
use openh264::encoder::BitRate;
use openh264::encoder::Encoder;
use openh264::encoder::EncoderConfig;
use openh264::encoder::FrameRate;
use openh264::encoder::IntraFramePeriod;
use openh264::encoder::Profile;
use openh264::encoder::RateControlMode;
use openh264::encoder::UsageType;
use openh264::formats::YUVSlices;

use crate::size::Size;

const MAGIC: &[u8; 8] = b"GWCLIP01";
/// One access unit larger than this is refused as corrupt.
const MAX_AU: u32 = 64 << 20;

/// Encoded access units of one size, in decode order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub size: Size,
    pub units: Vec<Vec<u8>>,
    /// Where the units came from, for the result line.
    pub source: String,
}

impl Clip {
    pub fn bytes(&self) -> u64 {
        self.units.iter().map(|u| u.len() as u64).sum()
    }

    pub fn write_to(&self, out: &mut impl Write) -> anyhow::Result<()> {
        out.write_all(MAGIC)?;
        out.write_all(&self.size.width.to_le_bytes())?;
        out.write_all(&self.size.height.to_le_bytes())?;
        for unit in &self.units {
            let len = u32::try_from(unit.len()).context("access unit over 4 GiB")?;
            out.write_all(&len.to_le_bytes())?;
            out.write_all(unit)?;
        }
        Ok(())
    }

    pub fn read_from(input: &mut impl Read, source: String) -> anyhow::Result<Self> {
        let mut magic = [0u8; 8];
        input.read_exact(&mut magic).context("clip header")?;
        ensure!(&magic == MAGIC, "not a GWCLIP01 file");
        let width = read_u32(input)?.context("clip width")?;
        let height = read_u32(input)?.context("clip height")?;
        let mut units = Vec::new();
        while let Some(len) = read_u32(input)? {
            ensure!(
                len > 0 && len <= MAX_AU,
                "access unit length {len} out of range"
            );
            let mut unit = vec![0u8; len as usize];
            input
                .read_exact(&mut unit)
                .context("truncated access unit")?;
            units.push(unit);
        }
        ensure!(!units.is_empty(), "clip has no access units");
        Ok(Self {
            size: Size { width, height },
            units,
            source,
        })
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::read_from(
            &mut std::io::BufReader::new(file),
            format!("file:{}", path.display()),
        )
    }
}

/// A u32 LE, or `None` at a clean end of input.
fn read_u32(input: &mut impl Read) -> anyhow::Result<Option<u32>> {
    let mut b = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = input.read(&mut b[got..])?;
        if n == 0 {
            if got == 0 {
                return Ok(None);
            }
            bail!("truncated length field");
        }
        got += n;
    }
    Ok(Some(u32::from_le_bytes(b)))
}

/// Encodes `frames` frames of a synthetic desktop at `size`: a static
/// text-like background, a scrolling text pane, and a moving gradient
/// window. One IDR first, then P frames only, as the server sends after
/// a keyframe request. OpenH264 Baseline at `bitrate` bits/s, 30 fps.
pub fn synthetic(size: Size, frames: u32, bitrate: u32) -> anyhow::Result<Clip> {
    ensure!(
        size.width.max(size.height) <= OPENH264_MAX.0
            && size.width.min(size.height) <= OPENH264_MAX.1,
        "{size}: the OpenH264 encoder stops at 3840x2160; use a clip from scripts/make-clips"
    );
    let config = EncoderConfig::new()
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .bitrate(BitRate::from_bps(bitrate))
        .max_frame_rate(FrameRate::from_hz(30.0))
        .skip_frames(false)
        .intra_frame_period(IntraFramePeriod::from_num_frames(0))
        .profile(Profile::Baseline);
    let mut encoder =
        Encoder::with_api_config(OpenH264API::from_source(), config).context("OpenH264 encoder")?;
    let mut desktop = Desktop::new(size);
    let mut units = Vec::with_capacity(frames as usize);
    for f in 0..frames {
        desktop.draw(f);
        let (w, h) = (size.width as usize, size.height as usize);
        let yuv = YUVSlices::new(
            (&desktop.y, &desktop.u, &desktop.v),
            (w, h),
            (w, w / 2, w / 2),
        );
        let unit = encoder
            .encode(&yuv)
            .with_context(|| format!("encode frame {f} at {size}"))?
            .to_vec();
        ensure!(!unit.is_empty(), "encoder skipped frame {f} at {size}");
        units.push(unit);
    }
    Ok(Clip {
        size,
        units,
        source: format!("synthetic-openh264-baseline@{bitrate}bps"),
    })
}

/// OpenH264's encoder limit (level 5.2, README "3840x2160 or 2160x3840").
const OPENH264_MAX: (u32, u32) = (3840, 2160);

/// Writes `frames` raw I420 frames of the synthetic desktop, for an
/// external encoder.
pub fn render_i420(size: Size, frames: u32, out: &mut impl Write) -> anyhow::Result<()> {
    let mut desktop = Desktop::new(size);
    for f in 0..frames {
        desktop.draw(f);
        out.write_all(&desktop.y)?;
        out.write_all(&desktop.u)?;
        out.write_all(&desktop.v)?;
    }
    out.flush()?;
    Ok(())
}

/// Splits an Annex B stream into access units at its access unit
/// delimiters (NAL type 9; libx264 `aud=1`). Bytes before the first
/// delimiter are refused rather than guessed at.
pub fn from_annexb(data: &[u8], size: Size, source: String) -> anyhow::Result<Clip> {
    let starts = nal_starts(data);
    let auds: Vec<usize> = starts
        .iter()
        .filter(|&&(_, payload)| data.get(payload).is_some_and(|b| b & 0x1f == 9))
        .map(|&(code, _)| code)
        .collect();
    ensure!(
        !auds.is_empty(),
        "no access unit delimiters; encode with aud=1"
    );
    ensure!(auds[0] == 0, "{} bytes before the first delimiter", auds[0]);
    let first_end = auds.get(1).copied().unwrap_or(data.len());
    let first: Vec<u8> = starts
        .iter()
        .filter(|&&(code, _)| code < first_end)
        .filter_map(|&(_, payload)| data.get(payload).map(|b| b & 0x1f))
        .collect();
    for (nal, name) in [(7, "SPS"), (8, "PPS"), (5, "IDR slice")] {
        ensure!(
            first.contains(&nal),
            "the first access unit has no {name} (NAL types {first:?}); a decoder cannot start"
        );
    }
    // libx264 picks 6.0 for 3840x2400 at 60 fps; OpenH264 stops at 5.2.
    for nal in h264::nal_units(data)
        .into_iter()
        .filter(|nal| nal.first().is_some_and(|b| b & 0x1f == 7))
    {
        let sps = h264::parse_sps(nal).context("unreadable SPS")?;
        ensure!(
            sps.decoder_accepts() == LevelVerdict::Accepted,
            "SPS level_idc {} is above 5.2; the OpenH264 decoder rejects it \
             (dsNoParamSets on every unit), so encode with -level 5.2",
            sps.level_idc
        );
    }
    let mut units = Vec::with_capacity(auds.len());
    for (i, &start) in auds.iter().enumerate() {
        let end = auds.get(i + 1).copied().unwrap_or(data.len());
        units.push(data[start..end].to_vec());
    }
    Ok(Clip {
        size,
        units,
        source,
    })
}

/// (start code offset, first NAL byte offset) for each NAL unit.
fn nal_starts(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let code = if i > 0 && data[i - 1] == 0 { i - 1 } else { i };
            out.push((code, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// I420 planes of the synthetic desktop.
struct Desktop {
    size: Size,
    background: Vec<u8>,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

/// Text-like texture: glyph cells 6x10 px on 16 px lines, ~35% lit.
fn text_pixel(x: u32, y: u32) -> bool {
    let (line, row) = (y / 16, y % 16);
    if row >= 10 {
        return false;
    }
    let (cell, col) = (x / 6, x % 6);
    if col == 5 {
        return false;
    }
    let glyph = hash(cell, line);
    let bit = (row / 2) * 5 + col; // a 5x5 glyph drawn 1x2
    (glyph >> (bit % 25)) & 1 == 1 && hash(cell ^ 0x55, line) % 100 < 70
}

fn hash(a: u32, b: u32) -> u32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^ (h >> 13)
}

impl Desktop {
    fn new(size: Size) -> Self {
        let (w, h) = (size.width, size.height);
        let mut background = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let base = 40 + (y * 40 / h) as u8;
                background[(y * w + x) as usize] = if text_pixel(x, y) { 220 } else { base };
            }
        }
        let chroma = (w / 2 * (h / 2)) as usize;
        Self {
            size,
            y: background.clone(),
            background,
            u: vec![128; chroma],
            v: vec![128; chroma],
        }
    }

    fn draw(&mut self, frame: u32) {
        let (w, h) = (self.size.width, self.size.height);
        self.y.copy_from_slice(&self.background);
        self.u.fill(128);
        self.v.fill(128);
        // A terminal pane in the right half scrolls one text line per frame.
        let (pane_x, pane_w, pane_h) = (w / 2, w / 2, h / 2);
        let scroll = frame * 16;
        for y in 0..pane_h {
            for x in 0..pane_w {
                let lit = text_pixel(x, y + scroll);
                self.y[(y * w + pane_x + x) as usize] = if lit { 200 } else { 20 };
            }
        }
        // A gradient window (a quarter of the height) bounces across.
        let side = (h / 4).max(2) & !1;
        let span_x = w.saturating_sub(side).max(1);
        let span_y = h.saturating_sub(side).max(1);
        let ox = bounce(frame * 24, span_x) & !1;
        let oy = bounce(frame * 14, span_y) & !1;
        for y in 0..side {
            for x in 0..side {
                let value = ((x + y + frame * 4) % 256) as u8;
                self.y[((oy + y) * w + ox + x) as usize] = value;
            }
        }
        for y in 0..side / 2 {
            for x in 0..side / 2 {
                let i = ((oy / 2 + y) * (w / 2) + ox / 2 + x) as usize;
                self.u[i] = 90;
                self.v[i] = 170;
            }
        }
    }
}

/// A position that moves back and forth over `0..=span`.
fn bounce(t: u32, span: u32) -> u32 {
    let period = span * 2;
    let p = t % period;
    if p <= span { p } else { period - p }
}

#[cfg(test)]
mod tests {
    use super::Clip;
    use super::bounce;
    use super::synthetic;
    use crate::size::Size;

    #[test]
    fn bounce_stays_in_range_and_turns() {
        assert_eq!(bounce(0, 10), 0);
        assert_eq!(bounce(10, 10), 10);
        assert_eq!(bounce(15, 10), 5);
        assert_eq!(bounce(20, 10), 0);
    }

    #[test]
    fn clip_file_round_trips() {
        let clip = synthetic(
            Size {
                width: 320,
                height: 240,
            },
            3,
            1_000_000,
        )
        .expect("encode");
        assert_eq!(clip.units.len(), 3);
        let mut bytes = Vec::new();
        clip.write_to(&mut bytes).expect("write");
        let back = Clip::read_from(&mut &bytes[..], clip.source.clone()).expect("read");
        assert_eq!(back, clip);
    }

    #[test]
    fn truncated_clip_is_refused() {
        let clip = synthetic(
            Size {
                width: 320,
                height: 240,
            },
            2,
            1_000_000,
        )
        .expect("encode");
        let mut bytes = Vec::new();
        clip.write_to(&mut bytes).expect("write");
        bytes.truncate(bytes.len() - 1);
        assert!(Clip::read_from(&mut &bytes[..], String::new()).is_err());
    }

    #[test]
    fn panel_size_is_refused_with_a_pointer_to_make_clips() {
        let err = synthetic(
            Size {
                width: 3840,
                height: 2400,
            },
            1,
            20_000_000,
        )
        .expect_err("OpenH264 cannot encode 3840x2400");
        assert!(err.to_string().contains("make-clips"), "{err}");
    }

    #[test]
    fn annexb_splits_at_delimiters_and_keeps_four_byte_codes() {
        let aud = [0, 0, 0, 1, 0x09, 0xf0];
        // A real libx264 SPS, level rewritten to 5.2, then a PPS.
        let sps = [
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x34, 0xac, 0xd9, 0x40, 0x3c, 0x00, 0x4b, 0x6c, 0x04,
            0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x0c, 0x83, 0xc2, 0x01, 0x06, 0x58,
            0x00, 0x00, 0x01, 0x68, 0xee,
        ];
        let idr = [0, 0, 1, 0x65, 0x88, 0x00, 0x00, 0x03, 0x01];
        let p = [0, 0, 1, 0x41, 0x9a];
        let mut data = Vec::new();
        for part in [&aud[..], &sps, &idr, &aud, &p] {
            data.extend_from_slice(part);
        }
        let size = Size {
            width: 2,
            height: 2,
        };
        let clip = super::from_annexb(&data, size, String::new()).expect("split");
        assert_eq!(clip.units.len(), 2);
        assert_eq!(clip.units[0], [&aud[..], &sps, &idr].concat());
        assert_eq!(clip.units[1], [&aud[..], &p].concat());
    }

    #[test]
    fn annexb_without_parameter_sets_up_front_is_refused() {
        // What libx264 `repeat-headers=0` into a raw .h264 produced: AUD + IDR.
        let data = [0, 0, 0, 1, 0x09, 0x10, 0, 0, 1, 0x65, 0x88];
        let size = Size {
            width: 2,
            height: 2,
        };
        let err = super::from_annexb(&data, size, String::new()).expect_err("no SPS");
        assert!(err.to_string().contains("SPS"), "{err}");
    }

    #[test]
    fn annexb_above_level_5_2_is_refused() {
        let data = [
            // A real libx264 High 6.0 SPS (see gliff_windows::h264 tests).
            0, 0, 0, 1, 0x09, 0x10, 0, 0, 1, 0x67, 0x64, 0x00, 0x3c, 0xac, 0xd9, 0x40, 0x3c, 0x00,
            0x4b, 0x6c, 0x04, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x0c, 0x83, 0xc2,
            0x01, 0x06, 0x58, 0, 0, 1, 0x68, 0xee, 0, 0, 1, 0x65, 0x88,
        ];
        let size = Size {
            width: 2,
            height: 2,
        };
        let err = super::from_annexb(&data, size, String::new()).expect_err("level 6.0");
        assert!(err.to_string().contains("5.2"), "{err}");
        let mut ok = data;
        ok[12] = 52;
        assert!(super::from_annexb(&ok, size, String::new()).is_ok());
    }

    #[test]
    fn annexb_without_delimiters_is_refused() {
        let size = Size {
            width: 2,
            height: 2,
        };
        assert!(super::from_annexb(&[0, 0, 1, 0x65, 1], size, String::new()).is_err());
    }
}
