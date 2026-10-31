//! The remote cursor image and upstream's rule for using it
//! (gliff 089662fd `crates/gliff/src/main.rs:1266-1330`).

use bytes::Bytes;

/// A `CursorShape` with its ARGB payload (B, G, R, A bytes per pixel, rows
/// top down, straight alpha).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub hot: (i32, i32),
    pub argb: Bytes,
}

/// What the session's desktop is. Only the log line tells them apart: the
/// pointer decision is the same on both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    /// A private headless output.
    Headless,
    Mirror,
}

impl Desktop {
    pub fn from_headless(headless: bool) -> Self {
        if headless {
            Self::Headless
        } else {
            Self::Mirror
        }
    }
}

/// The pointer the window shows over the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    /// The remote image.
    Shape,
    /// The system arrow.
    Default,
}

/// Why an image is or is not a usable pointer shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Usable,
    /// Not 1 to 1024 px a side.
    BadSize,
    /// The payload holds fewer bytes than the size says.
    Short,
    /// Every pixel has no alpha.
    Transparent,
    /// Every pixel is the same colour (what Hyprland sends with no image).
    Flat,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::BadSize => "bad size",
            Self::Short => "short payload",
            Self::Transparent => "transparent",
            Self::Flat => "one flat colour",
        }
    }
}

/// Keeps the cursor log to one line per change: shapes arrive on every
/// hover, and the same decision twice in a row says nothing new.
#[derive(Debug, Default)]
pub struct LogGate {
    last: Option<String>,
}

impl LogGate {
    pub fn should_log(&mut self, line: &str) -> bool {
        if self.last.as_deref() == Some(line) {
            return false;
        }
        self.last = Some(line.to_owned());
        true
    }

    /// A new connection: say the next shape again.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

impl CursorImage {
    /// Upstream's `remote_cursor` test: 1 to 1024 px a side, the whole
    /// image present, and a visible shape.
    pub fn usable(&self) -> bool {
        self.verdict() == Verdict::Usable
    }

    pub fn verdict(&self) -> Verdict {
        let needed = u64::from(self.width) * u64::from(self.height) * 4;
        if !(1..=1024).contains(&self.width) || !(1..=1024).contains(&self.height) {
            return Verdict::BadSize;
        }
        if (self.argb.len() as u64) < needed {
            return Verdict::Short;
        }
        let (pixels, _) = self.argb.as_chunks::<4>();
        if pixels.iter().all(|p| p[3] == 0) {
            return Verdict::Transparent;
        }
        if has_visible_shape(&self.argb) {
            Verdict::Usable
        } else {
            Verdict::Flat
        }
    }

    /// One log line for a shape and what the window does with it: sizes
    /// and the decision only, never the pixels.
    pub fn log_line(&self, desktop: Desktop) -> String {
        let name = match desktop {
            Desktop::Headless => "headless",
            Desktop::Mirror => "mirror",
        };
        let what = match self.pointer() {
            Pointer::Shape => "shown as the remote shape",
            Pointer::Default => "system arrow (the shape cannot be used)",
        };
        format!(
            "cursor shape: {}x{} hot {},{} {}, {name}: {what}",
            self.width,
            self.height,
            self.hot.0,
            self.hot.1,
            self.verdict().word()
        )
    }

    /// The image if usable, else the system arrow. Hiding the pointer on a
    /// headless session when the image is unusable lost it for good: the
    /// compositor sends a transparent or flat image over some remote
    /// windows and the video does not draw a cursor there either. The
    /// cost is a possible second pointer when the video does draw one.
    pub fn pointer(&self) -> Pointer {
        if self.usable() {
            Pointer::Shape
        } else {
            Pointer::Default
        }
    }
}

/// Some pixel has alpha and not every pixel equals the first (Hyprland
/// sends one flat colour when it has no image to share).
fn has_visible_shape(argb: &[u8]) -> bool {
    let (pixels, _) = argb.as_chunks::<4>();
    let Some(first) = pixels.first() else {
        return false;
    };
    let opaque = pixels.iter().any(|p| p[3] != 0);
    let uniform = pixels.iter().all(|p| p == first);
    opaque && !uniform
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::CursorImage;
    use super::Desktop;
    use super::Pointer;

    fn image(width: u32, height: u32, argb: Vec<u8>) -> CursorImage {
        CursorImage {
            id: 1,
            width,
            height,
            hot: (0, 0),
            argb: Bytes::from(argb),
        }
    }

    fn arrow() -> CursorImage {
        let mut argb = vec![0u8; 2 * 2 * 4];
        argb[..4].copy_from_slice(&[0, 0, 0, 255]);
        image(2, 2, argb)
    }

    #[test]
    fn a_shaped_image_is_used() {
        assert_eq!(arrow().pointer(), Pointer::Shape);
    }

    #[test]
    fn the_verdict_names_why_an_image_is_or_is_not_usable() {
        use super::Verdict;
        assert_eq!(arrow().verdict(), Verdict::Usable);
        assert_eq!(image(2, 2, vec![0; 16]).verdict(), Verdict::Transparent);
        assert_eq!(
            image(2, 2, [9u8, 9, 9, 255].repeat(4)).verdict(),
            Verdict::Flat
        );
        assert_eq!(image(2, 2, vec![0; 8]).verdict(), Verdict::Short);
        assert_eq!(image(0, 0, Vec::new()).verdict(), Verdict::BadSize);
        assert_eq!(image(1025, 1, vec![0; 4100]).verdict(), Verdict::BadSize);
    }

    #[test]
    fn the_log_line_has_sizes_and_the_decision_but_no_pixels() {
        let line = arrow().log_line(Desktop::Headless);
        assert_eq!(
            line,
            "cursor shape: 2x2 hot 0,0 usable, headless: shown as the remote shape"
        );
        let unusable = image(2, 2, vec![0; 16]).log_line(Desktop::Headless);
        assert_eq!(
            unusable,
            "cursor shape: 2x2 hot 0,0 transparent, headless: system arrow (the shape cannot be used)"
        );
        let mirror = image(2, 2, vec![0; 16]).log_line(Desktop::Mirror);
        assert_eq!(
            mirror,
            "cursor shape: 2x2 hot 0,0 transparent, mirror: system arrow (the shape cannot be used)"
        );
    }

    #[test]
    fn the_log_gate_passes_a_changed_decision_and_swallows_repeats() {
        use super::LogGate;
        let mut gate = LogGate::default();
        let a = arrow().log_line(Desktop::Headless);
        let b = image(2, 2, vec![0; 16]).log_line(Desktop::Headless);
        assert!(gate.should_log(&a));
        assert!(!gate.should_log(&a));
        assert!(gate.should_log(&b));
        assert!(gate.should_log(&a));
        gate.reset();
        assert!(gate.should_log(&a));
    }

    #[test]
    fn transparent_flat_short_or_huge_images_fall_back() {
        let cases = [
            ("transparent", image(2, 2, vec![0; 16])),
            ("flat", image(2, 2, [9u8, 9, 9, 255].repeat(4))),
            ("short", image(2, 2, vec![0, 0, 0, 255, 0, 0, 0, 0])),
            ("empty", image(0, 0, Vec::new())),
            (
                "huge",
                image(1025, 1, {
                    let mut v = vec![0u8; 1025 * 4];
                    v[3] = 255;
                    v
                }),
            ),
        ];
        for (name, img) in cases {
            assert!(!img.usable(), "{name}");
            assert_eq!(img.pointer(), Pointer::Default, "{name}");
        }
    }
}
