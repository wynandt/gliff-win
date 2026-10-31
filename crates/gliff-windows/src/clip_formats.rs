//! Clipboard formats: what Windows holds and Linux programs call it, and the
//! conversions between the two. Pure code; the Win32 calls that read and
//! write the formats live in `clip_win`.
//!
//! Outgoing (Windows to Linux): [`offer_mimes`] names what we can serve and
//! [`local_body`] reads and converts one flavour. Incoming (Linux to
//! Windows): [`plan_remote`] says which Windows formats to advertise and
//! [`render`] converts a fetched body into one.

mod html;
mod image;
mod text;

use gliff_proto::clipboard as proto;

pub use html::cf_html_to_fragment;
pub use html::html_to_cf_html;
pub use image::dib_to_png;
pub use image::png_to_dibv5;
pub use text::utf8_to_utf16_nul;
pub use text::utf16_to_utf8_lf;

/// The registered format names the Windows clipboard uses for flavours that
/// have no standard format.
pub const HTML_FORMAT: &str = "HTML Format";
pub const RTF_FORMAT: &str = "Rich Text Format";
pub const PNG_FORMAT: &str = "PNG";
pub const JFIF_FORMAT: &str = "JFIF";
pub const GIF_FORMAT: &str = "GIF";

/// Registered formats that keep a copy out of the clipboard history, the
/// cloud sync and monitors. A program that holds a secret sets them.
pub const EXCLUDE_FORMAT: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const VIEWER_IGNORE_FORMAT: &str = "Clipboard Viewer Ignore";
/// DWORD formats: 0 keeps the content out of the history, or the cloud.
pub const HISTORY_FORMAT: &str = "CanIncludeInClipboardHistory";
pub const CLOUD_FORMAT: &str = "CanUploadToCloudClipboard";
/// Set on every clipboard we write, so our own change is told from a user's.
pub const MARKER_FORMAT: &str = "application/x-gliff-windows-source";
/// The flavour a Linux password manager sets on a secret.
pub const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";

pub const RTF_MIME: &str = "text/rtf";
pub const RTF_APP_MIME: &str = "application/rtf";
pub const HTML_MIME: &str = "text/html";
pub const PNG_MIME: &str = "image/png";
pub const JPEG_MIME: &str = "image/jpeg";
pub const GIF_MIME: &str = "image/gif";

/// Most flavours advertised to Windows for one offer.
const MAX_FORMATS: usize = 32;
/// The longest registered format name Windows accepts.
const MAX_FORMAT_NAME: usize = 255;

/// A clipboard format as the pure code sees it. The Win32 side maps the
/// numeric ids and registered names to and from these.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WinFormat {
    /// `CF_UNICODETEXT`.
    UnicodeText,
    /// `CF_DIB`.
    Dib,
    /// `CF_DIBV5`.
    DibV5,
    /// `CF_HDROP`, a list of files.
    Hdrop,
    /// A registered format; the name is not one of the known ones below.
    Registered(String),
    /// "HTML Format".
    Html,
    /// "Rich Text Format".
    Rtf,
    /// "PNG".
    Png,
    /// "JFIF".
    Jfif,
    /// "GIF".
    Gif,
}

impl WinFormat {
    /// The format a registered name stands for.
    pub fn from_registered(name: &str) -> Self {
        match name {
            HTML_FORMAT => Self::Html,
            RTF_FORMAT => Self::Rtf,
            PNG_FORMAT => Self::Png,
            JFIF_FORMAT => Self::Jfif,
            GIF_FORMAT => Self::Gif,
            other => Self::Registered(other.to_owned()),
        }
    }

    /// The registered name of a format that has one.
    pub fn registered_name(&self) -> Option<&str> {
        match self {
            Self::Html => Some(HTML_FORMAT),
            Self::Rtf => Some(RTF_FORMAT),
            Self::Png => Some(PNG_FORMAT),
            Self::Jfif => Some(JFIF_FORMAT),
            Self::Gif => Some(GIF_FORMAT),
            Self::Registered(name) => Some(name),
            Self::UnicodeText | Self::Dib | Self::DibV5 | Self::Hdrop => None,
        }
    }
}

/// Why a body could not be read or converted.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    #[error("image: {0}")]
    Image(String),
    #[error("{0} bytes is over the {1} byte item limit")]
    TooLarge(usize, usize),
    #[error("the clipboard has no {0} to read")]
    Missing(String),
    #[error("the offer lists no flavour {0}")]
    Unknown(String),
}

/// Whether `mime` names rich text.
fn is_rtf(mime: &str) -> bool {
    matches!(base_mime(mime).as_str(), RTF_MIME | RTF_APP_MIME)
}

/// The type and subtype of a MIME name without parameters, lower case.
fn base_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// A registered name that looks like a MIME type; those are the
/// non-standard flavours worth offering.
fn is_mime_name(name: &str) -> bool {
    name != MARKER_FORMAT && name.contains('/') && !name.contains(char::is_whitespace)
}

/// The flavours to offer Linux for a Windows clipboard holding `formats`,
/// best first (text, HTML, RTF, images, then the rest), through upstream's
/// filter. A file list (`CF_HDROP`) is not here; files are offered
/// separately.
pub fn offer_mimes(formats: &[WinFormat]) -> Vec<String> {
    let has = |wanted: &WinFormat| formats.contains(wanted);
    let mut mimes: Vec<String> = Vec::new();
    if has(&WinFormat::UnicodeText) {
        // Upstream's filter adds the other text flavours behind this one.
        mimes.push(proto::TEXT_MIME.to_owned());
    }
    if has(&WinFormat::Html) {
        mimes.push(HTML_MIME.to_owned());
    }
    if has(&WinFormat::Rtf) {
        mimes.push(RTF_MIME.to_owned());
        mimes.push(RTF_APP_MIME.to_owned());
    }
    if has(&WinFormat::Png) || has(&WinFormat::DibV5) || has(&WinFormat::Dib) {
        mimes.push(PNG_MIME.to_owned());
    }
    if has(&WinFormat::Jfif) {
        mimes.push(JPEG_MIME.to_owned());
    }
    if has(&WinFormat::Gif) {
        mimes.push(GIF_MIME.to_owned());
    }
    for format in formats {
        if let WinFormat::Registered(name) = format
            && is_mime_name(name)
            && !mimes.contains(name)
        {
            mimes.push(name.clone());
        }
    }
    proto::forwardable_mimes(&mimes)
}

/// Whether the clipboard asks to stay out of history, sync and monitors.
/// `dword` reads the 4-byte value of the history and cloud formats.
pub fn is_private(formats: &[WinFormat], dword: &mut dyn FnMut(&str) -> Option<u32>) -> bool {
    let named = |wanted: &str| {
        formats
            .iter()
            .any(|f| f.registered_name().is_some_and(|name| name == wanted))
    };
    named(EXCLUDE_FORMAT)
        || named(VIEWER_IGNORE_FORMAT)
        || [HISTORY_FORMAT, CLOUD_FORMAT]
            .into_iter()
            .any(|name| named(name) && dword(name) == Some(0))
}

/// The most raw bytes worth copying off the clipboard for `format`, so that a
/// huge item is refused before it is copied. A text body is read as UTF-16,
/// twice the size of its UTF-8, and any other body is no larger than the item
/// limit, with room for a header. A bitmap is bounded by its pixel limit,
/// since its PNG is far smaller than the bitmap.
pub fn raw_limit(format: &WinFormat) -> usize {
    match format {
        WinFormat::Dib | WinFormat::DibV5 => image::MAX_DIB_BYTES,
        _ => 2 * usize::try_from(proto::MAX_ITEM).unwrap_or(usize::MAX / 4) + 4096,
    }
}

/// The body of the flavour `mime` from the clipboard in Linux's form. `read`
/// gives the raw bytes of a Windows format. A body over `limit` is refused.
pub fn local_body(
    mime: &str,
    formats: &[WinFormat],
    limit: usize,
    read: &mut dyn FnMut(&WinFormat) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, ConvertError> {
    let mut fetch = |format: WinFormat| -> Result<Vec<u8>, ConvertError> {
        read(&format).ok_or_else(|| ConvertError::Missing(format!("{format:?}")))
    };
    let base = base_mime(mime);
    let body = if proto::is_text_mime(mime) {
        let raw = fetch(WinFormat::UnicodeText)?;
        text::utf16_to_utf8_lf(&raw).into_bytes()
    } else if base == HTML_MIME {
        html::cf_html_to_fragment(&fetch(WinFormat::Html)?)
    } else if is_rtf(mime) {
        let mut raw = fetch(WinFormat::Rtf)?;
        while raw.last() == Some(&0) {
            raw.pop();
        }
        raw
    } else if base == PNG_MIME {
        // The registered PNG is the program's own encoding and keeps its
        // alpha; the bitmap formats are the fallback, also when the PNG
        // cannot be read.
        let png = if formats.contains(&WinFormat::Png) {
            fetch(WinFormat::Png).ok()
        } else {
            None
        };
        match png {
            Some(png) => png,
            None => {
                let dib = if formats.contains(&WinFormat::DibV5) {
                    fetch(WinFormat::DibV5).or_else(|_| fetch(WinFormat::Dib))
                } else {
                    fetch(WinFormat::Dib)
                }?;
                image::dib_to_png(&dib)?
            }
        }
    } else if base == JPEG_MIME {
        fetch(WinFormat::Jfif)?
    } else if base == GIF_MIME {
        fetch(WinFormat::Gif)?
    } else if formats.contains(&WinFormat::Registered(mime.to_owned())) {
        fetch(WinFormat::Registered(mime.to_owned()))?
    } else {
        return Err(ConvertError::Unknown(mime.to_owned()));
    };
    if body.len() > limit {
        return Err(ConvertError::TooLarge(body.len(), limit));
    }
    Ok(body)
}

/// How a fetched body is made into a Windows format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Render {
    Text,
    Html,
    Rtf,
    /// Taken as it is.
    Passthrough,
    /// PNG to `CF_DIBV5`.
    PngToDib,
}

/// One Windows format to advertise for a Linux offer, and where its bytes
/// come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePlan {
    pub format: WinFormat,
    /// The flavour to fetch from the Linux side, as the offer spelled it.
    pub mime: String,
    pub render: Render,
}

/// The Windows formats to advertise for a Linux offer. File lists are not
/// planned here; the caller handles `text/uri-list` and its kin.
pub fn plan_remote(offered: &[String]) -> Vec<RemotePlan> {
    let mut plans: Vec<RemotePlan> = Vec::new();
    let mut add = |format: WinFormat, mime: &str, render: Render| {
        if plans.len() < MAX_FORMATS && !plans.iter().any(|p| p.format == format) {
            plans.push(RemotePlan {
                format,
                mime: mime.to_owned(),
                render,
            });
        }
    };
    if let Some(mime) = proto::resolve_mime(proto::TEXT_MIME, offered) {
        add(WinFormat::UnicodeText, mime, Render::Text);
    }
    for mime in offered {
        let base = base_mime(mime);
        if proto::is_file_mime(mime) || proto::is_text_mime(mime) {
            continue;
        }
        if base == HTML_MIME {
            add(WinFormat::Html, mime, Render::Html);
        } else if is_rtf(mime) {
            add(WinFormat::Rtf, mime, Render::Rtf);
        } else if base == PNG_MIME {
            add(WinFormat::Png, mime, Render::Passthrough);
            add(WinFormat::DibV5, mime, Render::PngToDib);
        } else if base == JPEG_MIME {
            add(WinFormat::Jfif, mime, Render::Passthrough);
        } else if base == GIF_MIME {
            add(WinFormat::Gif, mime, Render::Passthrough);
        } else if !mime.is_empty() && mime.len() <= MAX_FORMAT_NAME {
            add(WinFormat::from_registered(mime), mime, Render::Passthrough);
        }
    }
    plans
}

/// Whether the Linux side marks its offer as a secret.
pub fn remote_is_sensitive(offered: &[String]) -> bool {
    offered.iter().any(|m| m == PASSWORD_HINT_MIME)
}

/// The bytes of a Windows format from a fetched body.
pub fn render(render: Render, body: &[u8]) -> Result<Vec<u8>, ConvertError> {
    match render {
        Render::Text => Ok(text::utf8_to_utf16_nul(body)),
        Render::Html => Ok(html::html_to_cf_html(body)),
        Render::Rtf => {
            let mut out = body.to_vec();
            out.push(0);
            Ok(out)
        }
        Render::Passthrough => Ok(body.to_vec()),
        Render::PngToDib => image::png_to_dibv5(body),
    }
}

#[cfg(test)]
mod tests;
