//! Text between a Windows `CF_UNICODETEXT` and UTF-8.

/// UTF-16LE text from the clipboard to UTF-8 for Linux: stops at the first
/// NUL, makes a lone surrogate U+FFFD, and turns CRLF into LF (a lone CR
/// stays). A trailing odd byte is dropped.
pub fn utf16_to_utf8_lf(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0);
    let text: String = char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    text.replace("\r\n", "\n")
}

/// UTF-8 from Linux to NUL-terminated UTF-16LE for the clipboard. Line feeds
/// stay as they are. Invalid UTF-8 becomes U+FFFD.
pub fn utf8_to_utf16_nul(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::with_capacity(text.len() * 2 + 2);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&[0, 0]);
    out
}
