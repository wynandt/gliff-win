//! Decoded pictures as tight I420, and the pool their planes come from.
//!
//! The display path uploads these planes as-is and converts on the GPU.
//! [`YuvFrame::to_bgra`] is upstream's CPU conversion,
//! kept as the oracle for the shader and for check PNGs, never for display.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::time::Instant;

use gliff_sw::BgraFrame;

/// Recycles plane buffers so a 3840x2400 picture does not fault in 13.8 MB
/// of fresh pages each time. Holds at most `keep` idle buffers; a buffer
/// of another length is dropped, not kept, so a resize frees the old size.
#[derive(Debug, Clone)]
pub struct YuvPool {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    idle: Mutex<Vec<Vec<u8>>>,
    keep: usize,
}

impl YuvPool {
    /// A pool that keeps up to `keep` idle buffers.
    pub fn new(keep: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                idle: Mutex::new(Vec::new()),
                keep,
            }),
        }
    }

    /// A buffer of exactly `len` bytes; its contents
    /// are whatever the last user left, so callers overwrite all of it.
    pub fn get(&self, len: usize) -> PooledBuf {
        let reused = {
            let mut idle = self
                .shared
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            idle.iter()
                .position(|b| b.len() == len)
                .map(|i| idle.swap_remove(i))
        };
        PooledBuf {
            buf: reused.unwrap_or_else(|| vec![0; len]),
            home: Arc::downgrade(&self.shared),
        }
    }

    /// Idle buffers waiting for reuse.
    pub fn idle(&self) -> usize {
        self.shared
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// A plane buffer that goes back to its pool when dropped (on the UI
/// thread, after the upload).
#[derive(Debug)]
pub struct PooledBuf {
    buf: Vec<u8>,
    home: Weak<Shared>,
}

impl PooledBuf {
    /// A buffer that belongs to no pool (tests, one-off frames).
    pub fn detached(buf: Vec<u8>) -> Self {
        Self {
            buf,
            home: Weak::new(),
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}

impl std::ops::Deref for PooledBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buf
    }
}

impl Drop for PooledBuf {
    fn drop(&mut self) {
        let Some(home) = self.home.upgrade() else {
            return;
        };
        let mut idle = home
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if idle.len() < home.keep {
            idle.push(std::mem::take(&mut self.buf));
        }
    }
}

/// Why a plane set cannot be a [`YuvFrame`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("odd or empty picture {0}x{1}")]
    Size(u32, u32),
    #[error("plane {plane} has {got} bytes, expected {want}")]
    Plane {
        plane: &'static str,
        got: usize,
        want: usize,
    },
}

/// One decoded picture, tight I420: Y is `width * height` bytes, U and V
/// are `width / 2 * height / 2`, rows without padding. Even size only.
#[derive(Debug)]
pub struct YuvFrame {
    width: u32,
    height: u32,
    y: PooledBuf,
    u: PooledBuf,
    v: PooledBuf,
    /// When the decoder released it; present latency is measured from here.
    decoded_at: Instant,
}

impl YuvFrame {
    /// Checks the size and the plane lengths.
    pub fn new(
        (width, height): (u32, u32),
        y: PooledBuf,
        u: PooledBuf,
        v: PooledBuf,
        decoded_at: Instant,
    ) -> Result<Self, FrameError> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(FrameError::Size(width, height));
        }
        let luma = width as usize * height as usize;
        for (plane, got, want) in [
            ("y", y.len(), luma),
            ("u", u.len(), luma / 4),
            ("v", v.len(), luma / 4),
        ] {
            if got != want {
                return Err(FrameError::Plane { plane, got, want });
            }
        }
        Ok(Self {
            width,
            height,
            y,
            u,
            v,
            decoded_at,
        })
    }

    /// A frame of one flat colour, for tests and fakes.
    pub fn solid((width, height): (u32, u32), yuv: [u8; 3]) -> Result<Self, FrameError> {
        let luma = width as usize * height as usize;
        Self::new(
            (width, height),
            PooledBuf::detached(vec![yuv[0]; luma]),
            PooledBuf::detached(vec![yuv[1]; luma / 4]),
            PooledBuf::detached(vec![yuv[2]; luma / 4]),
            Instant::now(),
        )
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn y(&self) -> &[u8] {
        &self.y
    }

    pub fn u(&self) -> &[u8] {
        &self.u
    }

    pub fn v(&self) -> &[u8] {
        &self.v
    }

    pub fn decoded_at(&self) -> Instant {
        self.decoded_at
    }

    /// Upstream's CPU conversion (`gliff_sw::convert::i420_to_bgra`, BT.709
    /// limited range, chroma repeated over each 2x2 block): the oracle for
    /// the shader and the source of check PNGs.
    pub fn to_bgra(&self) -> BgraFrame {
        let (w, h) = (self.width as usize, self.height as usize);
        let pixels =
            gliff_sw::convert::i420_to_bgra(&self.y, &self.u, &self.v, (w, w / 2, w / 2), w, h);
        BgraFrame {
            width: self.width,
            height: self.height,
            pixels,
        }
    }
}

impl PartialEq for YuvFrame {
    /// Same size and planes; `decoded_at` is not part of the picture.
    fn eq(&self, other: &Self) -> bool {
        self.size() == other.size()
            && self.y() == other.y()
            && self.u() == other.u()
            && self.v() == other.v()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::FrameError;
    use super::PooledBuf;
    use super::YuvFrame;
    use super::YuvPool;

    #[test]
    fn a_dropped_buffer_is_reused_for_the_same_length() {
        let pool = YuvPool::new(4);
        let mut a = pool.get(16);
        a.as_mut_slice()[0] = 7;
        drop(a);
        assert_eq!(pool.idle(), 1, "the buffer went back");
        let b = pool.get(16);
        assert_eq!(b[0], 7, "the same allocation came out again");
        assert_eq!(pool.idle(), 0, "taken out of the pool");
    }

    #[test]
    fn another_length_allocates_and_the_pool_stays_bounded() {
        let pool = YuvPool::new(2);
        let bufs: Vec<_> = (0..3).map(|_| pool.get(8)).collect();
        drop(bufs);
        assert_eq!(pool.idle(), 2, "at most `keep` idle buffers");
        let c = pool.get(32);
        assert_eq!(c.len(), 32, "a new length is a new buffer");
        assert_eq!(pool.idle(), 2, "the 8-byte buffers are untouched");
    }

    #[test]
    fn a_buffer_outliving_its_pool_is_just_freed() {
        let pool = YuvPool::new(2);
        let a = pool.get(8);
        drop(pool);
        drop(a);
    }

    #[test]
    fn frames_check_size_and_plane_lengths() {
        let plane = |n| PooledBuf::detached(vec![0; n]);
        assert_eq!(
            YuvFrame::new((3, 2), plane(6), plane(1), plane(1), Instant::now()).err(),
            Some(FrameError::Size(3, 2))
        );
        assert_eq!(
            YuvFrame::new((4, 2), plane(8), plane(1), plane(2), Instant::now()).err(),
            Some(FrameError::Plane {
                plane: "u",
                got: 1,
                want: 2
            })
        );
        assert!(YuvFrame::new((4, 2), plane(8), plane(2), plane(2), Instant::now()).is_ok());
    }

    #[test]
    fn to_bgra_is_upstreams_conversion() {
        // Limited-range white, black and BT.709 red (Y 63, U 102, V 240).
        for (yuv, bgr) in [
            ([235, 128, 128], [255, 255, 255]),
            ([16, 128, 128], [0, 0, 0]),
            ([63, 102, 240], [0, 0, 255]),
        ] {
            let frame = YuvFrame::solid((4, 2), yuv).expect("solid frame");
            let bgra = frame.to_bgra();
            assert_eq!((bgra.width, bgra.height), (4, 2));
            let px = &bgra.pixels[..4];
            for (got, want) in px[..3].iter().zip(bgr) {
                assert!(
                    got.abs_diff(want) <= 1,
                    "yuv {yuv:?}: got {px:?}, want about {bgr:?}"
                );
            }
            assert_eq!(px[3], 255, "opaque");
        }
    }
}
