//! CPU decode: OpenH264 to tight I420 for Single420, and to tight 4:4:4
//! for Dual420 (two decoders, recombined here).
//!
//! This owns OpenH264 itself rather than `gliff_sw::Decoder`, whose only
//! output is BGRA, because the display path converts on the GPU
//! (see `docs/architecture.md`). It mirrors `gliff_sw::Decoder` (upstream
//! 089662fd `gliff-sw/src/lib.rs:390-477`) in everything else: the decoder
//! config, the held-picture counter, the flush rule, the even crop and the
//! Dual420 pairing rule (both decoders are fed every unit; one picture
//! without the other is an error).
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

use gliff_proto::msg::ChromaMode;

use crate::yuv::FrameError;
use crate::yuv::Sampling;
use crate::yuv::YuvFrame;
use crate::yuv::YuvPool;

/// A decode failure.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error(transparent)]
    OpenH264(#[from] openh264::Error),
    #[error("decoded picture: {0}")]
    Frame(#[from] FrameError),
    #[error("main and aux streams fell out of step")]
    OutOfStep,
    #[error("main picture is {main:?} but aux picture is {aux:?}")]
    AuxSize {
        main: (usize, usize),
        aux: (usize, usize),
    },
}

/// Idle plane buffers kept per decoder: three planes for each of the
/// picture in the slot, the one being uploaded and the one being decoded.
const POOL_KEEP: usize = 9;

/// The client's only decoder until hardware decode is built.
pub struct CpuDecoder {
    inner: Decoder,
    /// The second decoder of a Dual420 stream, fed the aux payloads.
    aux: Option<Decoder>,
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
            aux: None,
            pool: YuvPool::new(POOL_KEEP),
            held: 0,
        })
    }

    /// Feeds one access unit: `main`, and for Dual420 `aux` (a Single420
    /// decoder ignores `aux`, as upstream). `Some` is the oldest held
    /// picture: a High-profile stream releases only its first picture one
    /// unit late.
    #[must_use = "a released picture must be presented or dropped on purpose"]
    pub fn decode(&mut self, main: &[u8], aux: &[u8]) -> Result<Option<YuvFrame>, DecodeError> {
        self.held += 1;
        let frame = match &mut self.aux {
            None => match self.inner.decode(main)? {
                Some(picture) => Some(copy_planes(&self.pool, &picture)?),
                None => None,
            },
            Some(second) => match (self.inner.decode(main)?, second.decode(aux)?) {
                (Some(m), Some(a)) => Some(recombine(&self.pool, &m, &a)?),
                (None, None) => None,
                _ => return Err(DecodeError::OutOfStep),
            },
        };
        let Some(frame) = frame else {
            return Ok(None);
        };
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
        let frame = match &mut self.aux {
            None => match self.inner.flush_remaining()?.first() {
                Some(picture) => Some(copy_planes(&pool, picture)?),
                None => None,
            },
            Some(second) => {
                let main = self.inner.flush_remaining()?;
                let aux = second.flush_remaining()?;
                match (main.first(), aux.first()) {
                    (Some(m), Some(a)) => Some(recombine(&pool, m, a)?),
                    _ => None,
                }
            }
        };
        let Some(frame) = frame else {
            // Nothing came out, so nothing is drainable: correct any drift
            // the counter picked up from failed decodes (as upstream).
            self.held = 0;
            return Ok(None);
        };
        self.held = self.held.saturating_sub(1);
        Ok(Some(frame))
    }

    /// A fresh decoder for a `chroma` stream: one OpenH264 decoder for
    /// Single420, two for Dual420. The next IDR carries SPS and PPS
    /// in-band.
    pub fn reset(&mut self, chroma: ChromaMode) -> Result<(), DecodeError> {
        self.inner = new_openh264()?;
        // `session::Core` refuses any other chroma before a reset.
        self.aux = match chroma {
            ChromaMode::Dual420 => Some(new_openh264()?),
            _ => None,
        };
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

/// Puts a Dual420 pair back together as tight 4:4:4, the inverse of
/// upstream's `split_yuv444`: Y is main's luma; on even rows, U and V take
/// main's chroma at even columns and aux's chroma at odd columns; odd rows
/// of U are the top half of aux's luma, odd rows of V the bottom half.
fn recombine(
    pool: &YuvPool,
    main: &DecodedYUV<'_>,
    aux: &DecodedYUV<'_>,
) -> Result<YuvFrame, DecodeError> {
    let even = |(w, h): (usize, usize)| (w & !1, h & !1);
    let (w, h) = even(main.dimensions());
    if even(aux.dimensions()) != (w, h) {
        return Err(DecodeError::AuxSize {
            main: (w, h),
            aux: even(aux.dimensions()),
        });
    }
    let (my, mu, mv) = main.strides();
    let (ay, au, av) = aux.strides();
    let half = h / 2;
    let mut y = pool.get(w * h);
    let mut u = pool.get(w * h);
    let mut v = pool.get(w * h);
    copy_rows(main.y(), my, y.as_mut_slice(), w, h);
    let rows = u
        .as_mut_slice()
        .chunks_exact_mut(w)
        .zip(v.as_mut_slice().chunks_exact_mut(w));
    for (row, (u_row, v_row)) in rows.enumerate() {
        let by = row / 2;
        if row % 2 == 0 {
            interleave(&main.u()[by * mu..], &aux.u()[by * au..], u_row);
            interleave(&main.v()[by * mv..], &aux.v()[by * av..], v_row);
        } else {
            u_row.copy_from_slice(&aux.y()[by * ay..by * ay + w]);
            v_row.copy_from_slice(&aux.y()[(half + by) * ay..(half + by) * ay + w]);
        }
    }
    let size = (
        u32::try_from(w).expect("an H.264 picture is narrower than 2^32"),
        u32::try_from(h).expect("an H.264 picture is shorter than 2^32"),
    );
    Ok(YuvFrame::sampled(
        Sampling::Full,
        size,
        y,
        u,
        v,
        Instant::now(),
    )?)
}

/// One full-width chroma row from its even (`even`) and odd (`odd`)
/// columns.
fn interleave(even: &[u8], odd: &[u8], out: &mut [u8]) {
    for ((pair, e), o) in out.as_chunks_mut::<2>().0.iter_mut().zip(even).zip(odd) {
        pair[0] = *e;
        pair[1] = *o;
    }
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
