//! The latest-wins, one-picture-deep hand-off from the session thread to
//! whoever shows pictures (the UI thread in the client, a consumer thread in
//! `gwin-verify stream`). A picture replaced before it was taken is a drop:
//! it was decoded and acked, just never shown.

use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use crate::session::Picture;

/// What [`FrameSlot::put`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// The slot was empty.
    Filled,
    /// An untaken picture was replaced and counted as dropped.
    Replaced,
}

#[derive(Debug, Default)]
pub struct FrameSlot {
    picture: Mutex<Option<Picture>>,
    dropped: AtomicU64,
}

impl FrameSlot {
    pub fn put(&self, picture: Picture) -> Put {
        let old = self
            .picture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(picture);
        match old {
            Some(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                Put::Replaced
            }
            None => Put::Filled,
        }
    }

    pub fn take(&self) -> Option<Picture> {
        self.picture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    /// Pictures replaced before they were taken.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::FrameSlot;
    use super::Put;
    use crate::session::Geometry;
    use crate::session::Picture;
    use crate::yuv::YuvFrame;

    fn picture(frame_id: u64) -> Picture {
        Picture {
            frame: YuvFrame::solid((2, 2), [16, 128, 128]).expect("2x2 picture"),
            geometry: Geometry {
                stream: (2, 2),
                view: (2, 2),
                scale_milli: 1000,
            },
            frame_id,
        }
    }

    #[test]
    fn latest_wins_and_replacements_count_as_drops() {
        let slot = FrameSlot::default();
        assert_eq!(slot.put(picture(1)), Put::Filled, "empty slot");
        assert_eq!(
            slot.put(picture(2)),
            Put::Replaced,
            "untaken picture replaced"
        );
        assert_eq!(slot.put(picture(3)), Put::Replaced, "again");
        assert_eq!(slot.take().map(|p| p.frame_id), Some(3), "the latest one");
        assert!(slot.take().is_none(), "taken: empty");
        assert_eq!(slot.put(picture(4)), Put::Filled, "after a take");
        assert_eq!(slot.dropped(), 2, "two drops");
    }
}
