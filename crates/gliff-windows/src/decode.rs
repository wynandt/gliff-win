//! CPU decode for Single420 streams: OpenH264 to tight I420.
//!
//! This owns OpenH264 itself rather than `gliff_sw::Decoder`, whose only
//! output is BGRA, because the display path converts on the GPU
//! (see `docs/architecture.md`). It mirrors `gliff_sw::Decoder` (upstream
//! 089662fd `gliff-sw/src/lib.rs:390-477`) in everything else: the decoder
//! config, the held-picture counter, the flush rule and the even crop.
//! `to_bgra` of its output is byte-identical to `gliff_sw::Decoder`'s
//! (tests below).
//!
//! Geometry is not tracked here; `session::Core` owns the one
//! pending-geometry FIFO.

use std::time::Instant;

use openh264::OpenH264API;
use openh264::decoder::DecodedYUV;
use openh264::decoder::Decoder;
use openh264::decoder::DecoderConfig;
use openh264::decoder::Flush;
use openh264::formats::YUVSource;

use crate::yuv::FrameError;
use crate::yuv::YuvFrame;
use crate::yuv::YuvPool;

/// A decode failure.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error(transparent)]
    OpenH264(#[from] openh264::Error),
    #[error("decoded picture: {0}")]
    Frame(#[from] FrameError),
}

/// Idle plane buffers kept per decoder: three planes for each of the
/// picture in the slot, the one being uploaded and the one being decoded.
const POOL_KEEP: usize = 9;

/// The client's only decoder until hardware decode is built.
pub struct CpuDecoder {
    inner: Decoder,
    pool: YuvPool,
    /// Access units fed minus pictures returned, as upstream: zero for
    /// Baseline; a High-profile stream holds its first picture until the
    /// next unit or a [`CpuDecoder::flush`].
    held: u32,
}

impl CpuDecoder {
    /// A Single420 decoder with its own plane pool.
    pub fn new() -> Result<Self, DecodeError> {
        Ok(Self {
            inner: new_openh264()?,
            pool: YuvPool::new(POOL_KEEP),
            held: 0,
        })
    }

    /// Feeds one access unit. `Some` is the oldest held picture: a
    /// High-profile stream releases only its first picture one unit late.
    #[must_use = "a released picture must be presented or dropped on purpose"]
    pub fn decode(&mut self, main: &[u8]) -> Result<Option<YuvFrame>, DecodeError> {
        self.held += 1;
        let Some(picture) = self.inner.decode(main)? else {
            return Ok(None);
        };
        let frame = copy_planes(&self.pool, &picture)?;
        self.held = self.held.saturating_sub(1);
        Ok(Some(frame))
    }

    /// True when a picture is held that only another unit or a flush
    /// releases; the driver arms its idle drain only then.
    pub fn has_pending(&self) -> bool {
        self.held > 0
    }

    /// Idle drain. Each OpenH264 flush leaks a picture slot until the next
    /// IDR and the fourth in a row breaks decoding, so callers flush only
    /// when [`CpuDecoder::has_pending`] is true.
    #[must_use = "a drained picture must be presented or dropped on purpose"]
    pub fn flush(&mut self) -> Result<Option<YuvFrame>, DecodeError> {
        let pool = self.pool.clone();
        let remaining = self.inner.flush_remaining()?;
        let Some(picture) = remaining.first() else {
            // Nothing came out, so nothing is drainable: correct any drift
            // the counter picked up from failed decodes (as upstream).
            self.held = 0;
            return Ok(None);
        };
        let frame = copy_planes(&pool, picture)?;
        self.held = self.held.saturating_sub(1);
        Ok(Some(frame))
    }

    /// A fresh decoder; the next IDR carries SPS and PPS in-band.
    pub fn reset(&mut self) -> Result<(), DecodeError> {
        self.inner = new_openh264()?;
        self.held = 0;
        Ok(())
    }
}

/// Upstream's `new_h264_decoder`: no flush after decode, because every
/// OpenH264 flush leaks a picture slot in single-thread mode.
fn new_openh264() -> Result<Decoder, openh264::Error> {
    Decoder::with_api_config(
        OpenH264API::from_source(),
        DecoderConfig::new().flush_after_decode(Flush::NoFlush),
    )
}

/// Copies the planes out of OpenH264's buffers, cropped to even
/// dimensions as upstream's `decoded_to_bgra` does, rows made tight.
fn copy_planes(pool: &YuvPool, picture: &DecodedYUV<'_>) -> Result<YuvFrame, DecodeError> {
    let (w, h) = picture.dimensions();
    let (w, h) = (w & !1, h & !1);
    let (sy, su, sv) = picture.strides();
    let (cw, ch) = (w / 2, h / 2);
    let mut y = pool.get(w * h);
    let mut u = pool.get(cw * ch);
    let mut v = pool.get(cw * ch);
    copy_rows(picture.y(), sy, y.as_mut_slice(), w, h);
    copy_rows(picture.u(), su, u.as_mut_slice(), cw, ch);
    copy_rows(picture.v(), sv, v.as_mut_slice(), cw, ch);
    let size = (
        u32::try_from(w).expect("an H.264 picture is narrower than 2^32"),
        u32::try_from(h).expect("an H.264 picture is shorter than 2^32"),
    );
    Ok(YuvFrame::new(size, y, u, v, Instant::now())?)
}

fn copy_rows(src: &[u8], stride: usize, dst: &mut [u8], width: usize, rows: usize) {
    if width == 0 {
        return;
    }
    for (row, out) in dst.chunks_exact_mut(width).take(rows).enumerate() {
        let start = row * stride;
        out.copy_from_slice(&src[start..start + width]);
    }
}

#[cfg(test)]
mod tests;
