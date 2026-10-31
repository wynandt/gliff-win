//! The Windows "HTML Format" (CF_HTML) and the plain HTML Linux uses.
//!
//! CF_HTML is UTF-8 with a header of byte offsets (`Version:0.9`), the HTML
//! document and, inside it, the copied fragment between two comments. Linux
//! programs take and give the fragment as `text/html`.

const START_COMMENT: &str = "<!--StartFragment-->";
const END_COMMENT: &str = "<!--EndFragment-->";
/// Put in front of a fragment going to Linux so it is read as UTF-8.
const META: &str = "<meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\">";
const PREFIX: &str = "<html>\r\n<body>\r\n";
const SUFFIX: &str = "\r\n</body>\r\n</html>";

/// The fragment of a CF_HTML payload as `text/html` for Linux: the bytes
/// between `StartFragment` and `EndFragment` when the header is sound, else
/// between the two comments, else the whole document. A UTF-8 `meta` tag
/// goes in front.
pub fn cf_html_to_fragment(payload: &[u8]) -> Vec<u8> {
    let payload = trim_nul(payload);
    let fragment = header_range(payload, "StartFragment", "EndFragment")
        .map(|range| &payload[range])
        .or_else(|| comment_range(payload).map(|range| &payload[range]))
        .or_else(|| header_range(payload, "StartHTML", "EndHTML").map(|range| &payload[range]))
        .unwrap_or_else(|| after_header(payload));
    let mut out = META.as_bytes().to_vec();
    out.extend_from_slice(fragment);
    out
}

/// A CF_HTML payload for Windows from the `text/html` Linux gave. A document
/// that has the fragment comments is kept whole; one with a `body` has the
/// body as the fragment; anything else is wrapped in a minimal document.
pub fn html_to_cf_html(html: &[u8]) -> Vec<u8> {
    let html = String::from_utf8_lossy(html);
    let (document, fragment) = split_document(&html);
    let header_len = header(0, 0, 0, 0).len();
    let start_html = header_len;
    let end_html = header_len + document.len();
    let start_fragment = header_len + fragment.start;
    let end_fragment = header_len + fragment.end;
    let mut out = header(start_html, end_html, start_fragment, end_fragment).into_bytes();
    out.extend_from_slice(document.as_bytes());
    out
}

/// The document to hand to Windows and the byte range of the fragment in it.
fn split_document(html: &str) -> (String, std::ops::Range<usize>) {
    if let (Some(start), Some(end)) = (html.find(START_COMMENT), html.find(END_COMMENT))
        && start + START_COMMENT.len() <= end
    {
        return (html.to_owned(), start + START_COMMENT.len()..end);
    }
    let lower = html.to_ascii_lowercase();
    if let Some(body) = lower.find("<body")
        && let Some(open_end) = lower[body..].find('>')
    {
        // The comments go around the body's content, so a program that
        // reads them finds the same fragment as one that reads offsets.
        let from = body + open_end + 1;
        let to = lower
            .rfind("</body")
            .filter(|&i| i >= from)
            .unwrap_or(html.len());
        let document = format!(
            "{}{START_COMMENT}{}{END_COMMENT}{}",
            &html[..from],
            &html[from..to],
            &html[to..]
        );
        let start = from + START_COMMENT.len();
        return (document, start..start + (to - from));
    }
    let document = format!("{PREFIX}{START_COMMENT}{html}{END_COMMENT}{SUFFIX}");
    let start = PREFIX.len() + START_COMMENT.len();
    (document, start..start + html.len())
}

/// The header with the four offsets as ten digits each, so its length does
/// not depend on them.
fn header(
    start_html: usize,
    end_html: usize,
    start_fragment: usize,
    end_fragment: usize,
) -> String {
    format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
         StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    )
}

fn trim_nul(payload: &[u8]) -> &[u8] {
    let end = payload.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    &payload[..end]
}

/// The range two header offsets name, when both are present and inside the
/// payload in order.
fn header_range(payload: &[u8], start_key: &str, end_key: &str) -> Option<std::ops::Range<usize>> {
    let start = header_value(payload, start_key)?;
    let end = header_value(payload, end_key)?;
    (start <= end && end <= payload.len()).then_some(start..end)
}

/// The number after `key:` in the header lines at the front of the payload.
fn header_value(payload: &[u8], key: &str) -> Option<usize> {
    // The header ends where the HTML starts; stop looking there.
    let limit = payload
        .iter()
        .position(|&b| b == b'<')
        .unwrap_or(payload.len());
    let head = std::str::from_utf8(&payload[..limit]).ok()?;
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == key).then(|| value.trim().parse().ok()).flatten()
    })
}

fn comment_range(payload: &[u8]) -> Option<std::ops::Range<usize>> {
    let text = std::str::from_utf8(payload).ok()?;
    let start = text.find(START_COMMENT)? + START_COMMENT.len();
    let end = text.find(END_COMMENT)?;
    (start <= end).then_some(start..end)
}

/// Everything from the first `<`, for a payload without a usable header.
fn after_header(payload: &[u8]) -> &[u8] {
    let start = payload.iter().position(|&b| b == b'<').unwrap_or(0);
    &payload[start..]
}
