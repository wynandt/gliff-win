//! The Windows keyboard layout as an XKB keymap for the server (decision 2:
//! any layout, then a language fallback, then `us`). Pure.
//!
//! The text is a small template with includes; the server compiles it
//! against its own xkeyboard-config. Evdev codes stay positions, so nothing
//! in the key path depends on the layout.

use std::fmt;

/// An XKB layout and variant, as xkeyboard-config names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XkbLayout {
    pub layout: &'static str,
    pub variant: Option<&'static str>,
}

const US: XkbLayout = XkbLayout::new("us", None);

impl XkbLayout {
    pub const fn new(layout: &'static str, variant: Option<&'static str>) -> Self {
        Self { layout, variant }
    }
}

impl fmt::Display for XkbLayout {
    /// `layout` or `layout(variant)`, the form `xkb_symbols` includes take.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.variant {
            Some(variant) => write!(f, "{}({variant})", self.layout),
            None => f.write_str(self.layout),
        }
    }
}

/// How a KLID was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The whole KLID is in the table.
    Exact,
    /// Only the language (the low 16 bits) is known; its default layout.
    Language,
    /// Nothing known: `us`.
    Fallback,
}

/// The result of `from_klid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub layout: XkbLayout,
    pub source: Source,
}

/// KLID (the eight hex digits `GetKeyboardLayoutNameW` returns, lower case)
/// to a layout. Rows are checked against xkeyboard-config by a test.
pub const KLID_TABLE: &[(&str, XkbLayout)] = &[
    ("00000409", US),
    ("00020409", XkbLayout::new("us", Some("intl"))),
    ("00010409", XkbLayout::new("us", Some("dvorak"))),
    ("00000407", XkbLayout::new("de", None)),
    ("00000807", XkbLayout::new("ch", None)),
    ("0000100c", XkbLayout::new("ch", Some("fr"))),
    ("0000040c", XkbLayout::new("fr", None)),
    ("00000809", XkbLayout::new("gb", None)),
    ("0000040a", XkbLayout::new("es", None)),
    ("00000410", XkbLayout::new("it", None)),
    ("00000816", XkbLayout::new("pt", None)),
    ("00000416", XkbLayout::new("br", None)),
    ("0000041d", XkbLayout::new("se", None)),
    ("00000414", XkbLayout::new("no", None)),
    ("00000406", XkbLayout::new("dk", None)),
    ("0000040b", XkbLayout::new("fi", None)),
    ("00000415", XkbLayout::new("pl", None)),
    ("00000405", XkbLayout::new("cz", None)),
    ("0000040e", XkbLayout::new("hu", None)),
    ("00000419", XkbLayout::new("ru", None)),
    ("00000813", XkbLayout::new("be", None)),
    ("00000c0c", XkbLayout::new("ca", None)),
    ("00001009", XkbLayout::new("ca", Some("multix"))),
    ("00000411", XkbLayout::new("jp", None)),
    ("00000412", XkbLayout::new("kr", None)),
];

/// The default layout of a language, by LANGID (low 16 bits of the KLID)
/// first, then by primary language (its low 10 bits).
pub const LANGUAGE_TABLE: &[(u16, XkbLayout)] = &[
    (0x0809, XkbLayout::new("gb", None)),
    (0x0409, US),
    (0x0407, XkbLayout::new("de", None)),
    (0x0c0c, XkbLayout::new("ca", None)),
    (0x040c, XkbLayout::new("fr", None)),
    (0x040a, XkbLayout::new("es", None)),
    (0x0410, XkbLayout::new("it", None)),
    (0x0816, XkbLayout::new("pt", None)),
    (0x0416, XkbLayout::new("br", None)),
    (0x041d, XkbLayout::new("se", None)),
    (0x0414, XkbLayout::new("no", None)),
    (0x0406, XkbLayout::new("dk", None)),
    (0x040b, XkbLayout::new("fi", None)),
    (0x0415, XkbLayout::new("pl", None)),
    (0x0405, XkbLayout::new("cz", None)),
    (0x040e, XkbLayout::new("hu", None)),
    (0x0419, XkbLayout::new("ru", None)),
    (0x0413, XkbLayout::new("nl", None)),
    (0x0411, XkbLayout::new("jp", None)),
    (0x0412, XkbLayout::new("kr", None)),
    (0x0408, XkbLayout::new("gr", None)),
    (0x041f, XkbLayout::new("tr", None)),
    (0x0422, XkbLayout::new("ua", None)),
    (0x040d, XkbLayout::new("il", None)),
];

/// LANGID primary language ids and their layouts, for a LANGID not in
/// `LANGUAGE_TABLE` (for example German (Austria), 0x0c07).
const PRIMARY_TABLE: &[(u16, XkbLayout)] = &[
    (0x09, US),
    (0x07, XkbLayout::new("de", None)),
    (0x0c, XkbLayout::new("fr", None)),
    (0x0a, XkbLayout::new("es", None)),
    (0x10, XkbLayout::new("it", None)),
    (0x16, XkbLayout::new("pt", None)),
    (0x1d, XkbLayout::new("se", None)),
    (0x14, XkbLayout::new("no", None)),
    (0x06, XkbLayout::new("dk", None)),
    (0x0b, XkbLayout::new("fi", None)),
    (0x15, XkbLayout::new("pl", None)),
    (0x05, XkbLayout::new("cz", None)),
    (0x0e, XkbLayout::new("hu", None)),
    (0x19, XkbLayout::new("ru", None)),
    (0x13, XkbLayout::new("nl", None)),
    (0x11, XkbLayout::new("jp", None)),
    (0x12, XkbLayout::new("kr", None)),
    (0x08, XkbLayout::new("gr", None)),
    (0x1f, XkbLayout::new("tr", None)),
    (0x22, XkbLayout::new("ua", None)),
    (0x0d, XkbLayout::new("il", None)),
];

/// A KLID to a layout: the exact table, then the language, then `us`.
/// The case of the digits does not matter; anything that is not eight hex
/// digits is the fallback.
pub fn from_klid(klid: &str) -> Resolved {
    let klid = klid.to_ascii_lowercase();
    if let Some((_, layout)) = KLID_TABLE.iter().find(|(k, _)| *k == klid) {
        return Resolved {
            layout: *layout,
            source: Source::Exact,
        };
    }
    let langid = (klid.len() == 8)
        .then(|| u32::from_str_radix(&klid, 16).ok())
        .flatten()
        .map(|value| (value & 0xffff) as u16);
    let Some(langid) = langid else {
        return Resolved {
            layout: US,
            source: Source::Fallback,
        };
    };
    let by_langid = LANGUAGE_TABLE.iter().find(|(l, _)| *l == langid);
    let by_primary = PRIMARY_TABLE.iter().find(|(p, _)| *p == langid & 0x3ff);
    match by_langid.or(by_primary) {
        Some((_, layout)) => Resolved {
            layout: *layout,
            source: Source::Language,
        },
        None => Resolved {
            layout: US,
            source: Source::Fallback,
        },
    }
}

/// The keymap text for a layout: the shape `setxkbmap -print` emits, without
/// the X11-only geometry. About 300 bytes.
pub fn keymap_text(layout: &XkbLayout) -> String {
    format!(
        "xkb_keymap {{\n  xkb_keycodes {{ include \"evdev+aliases(qwerty)\" }};\n  \
         xkb_types    {{ include \"complete\" }};\n  \
         xkb_compat   {{ include \"complete\" }};\n  \
         xkb_symbols  {{ include \"pc+{layout}+inet(evdev)\" }};\n}};\n"
    )
}

/// Why a keymap was sent, for the log line the gate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Start,
    InputLang,
    Focus,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Start => "start",
            Self::InputLang => "inputlang",
            Self::Focus => "focus",
        })
    }
}

/// A keymap to send after the session start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeymapSend {
    pub text: String,
    /// The `keymap_sent` log line (no trailing newline).
    pub log: String,
}

/// Remembers the text the server has, so a `Keymap` is sent only when it
/// changes (the server ignores identical text, but the line is noise).
#[derive(Debug, Default)]
pub struct KeymapTracker {
    last: Option<String>,
}

impl KeymapTracker {
    /// The text and log line for a KLID. `reason` is how the check came
    /// about; `Source::Fallback` is reported as `fallback` instead.
    fn describe(klid: &str, reason: Reason) -> (String, String) {
        let resolved = from_klid(klid);
        let text = keymap_text(&resolved.layout);
        let why = match resolved.source {
            Source::Fallback => "fallback".to_owned(),
            Source::Exact | Source::Language => reason.to_string(),
        };
        let log = format!(
            "keymap_sent klid={klid} layout={} variant={} bytes={} reason={why}",
            resolved.layout.layout,
            resolved.layout.variant.unwrap_or("-"),
            text.len(),
        );
        (text, log)
    }

    /// The `Hello.keymap` text and its log line, and records it as sent.
    pub fn start(&mut self, klid: &str) -> KeymapSend {
        let (text, log) = Self::describe(klid, Reason::Start);
        self.last = Some(text.clone());
        KeymapSend { text, log }
    }

    /// The `Keymap` to send for the layout now active, or `None` when the
    /// server already has that text.
    pub fn check(&mut self, klid: &str, reason: Reason) -> Option<KeymapSend> {
        let (text, log) = Self::describe(klid, reason);
        if self.last.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last = Some(text.clone());
        Some(KeymapSend { text, log })
    }
}

#[cfg(test)]
mod tests;
