use std::collections::BTreeSet;

use super::KLID_TABLE;
use super::KeymapTracker;
use super::LANGUAGE_TABLE;
use super::Reason;
use super::Source;
use super::XkbLayout;
use super::from_klid;
use super::keymap_text;

fn known_names() -> BTreeSet<(String, String)> {
    include_str!("../../tests/fixtures/xkb-layouts.tsv")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let (layout, variant) = l.split_once('\t').expect("a tab-separated row");
            (layout.to_owned(), variant.to_owned())
        })
        .collect()
}

fn assert_known(names: &BTreeSet<(String, String)>, layout: &XkbLayout, what: &str) {
    let key = (
        layout.layout.to_owned(),
        layout.variant.unwrap_or("-").to_owned(),
    );
    assert!(
        names.contains(&key),
        "{what}: {layout} is not in xkeyboard-config's evdev.lst"
    );
}

#[test]
fn us_is_exact() {
    let r = from_klid("00000409");
    assert_eq!(r.layout, XkbLayout::new("us", None));
    assert_eq!(r.source, Source::Exact);
}

#[test]
fn the_table_rows() {
    for (klid, layout, variant) in [
        ("00020409", "us", Some("intl")),
        ("00010409", "us", Some("dvorak")),
        ("00000407", "de", None),
        ("00000807", "ch", None),
        ("0000100c", "ch", Some("fr")),
        ("00000809", "gb", None),
    ] {
        let r = from_klid(klid);
        assert_eq!(
            (r.layout.layout, r.layout.variant),
            (layout, variant),
            "{klid}"
        );
        assert_eq!(r.source, Source::Exact, "{klid}");
    }
}

#[test]
fn an_unlisted_klid_falls_back_to_its_language() {
    // German (Austria) keyboard: not in the table, German language.
    let r = from_klid("00000c07");
    assert_eq!(r.layout, XkbLayout::new("de", None));
    assert_eq!(r.source, Source::Language);
    // A layout variant id in the high half, English (US) language.
    let r = from_klid("a0010409");
    assert_eq!(r.layout, XkbLayout::new("us", None));
    assert_eq!(r.source, Source::Language);
}

#[test]
fn an_unknown_klid_or_language_falls_back_to_us() {
    for klid in ["", "zz", "0000", "00000436", "00000fff", "not-a-klid!"] {
        let r = from_klid(klid);
        assert_eq!(r.layout, XkbLayout::new("us", None), "{klid:?}");
        assert_eq!(r.source, Source::Fallback, "{klid:?}");
    }
}

#[test]
fn every_table_row_exists_in_xkeyboard_config() {
    let names = known_names();
    for (klid, layout) in KLID_TABLE {
        assert_known(&names, layout, klid);
    }
    for (langid, layout) in LANGUAGE_TABLE {
        assert_known(&names, layout, &format!("language {langid:#06x}"));
    }
}

#[test]
fn klids_are_unique_eight_lowercase_hex_digits() {
    let mut seen = BTreeSet::new();
    for (klid, _) in KLID_TABLE {
        assert_eq!(klid.len(), 8, "{klid}");
        assert!(
            klid.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{klid} is lower-case hex"
        );
        assert!(seen.insert(*klid), "{klid} is listed twice");
    }
}

#[test]
fn the_text_names_the_layout_and_is_small() {
    let text = keymap_text(&XkbLayout::new("de", Some("nodeadkeys")));
    assert!(text.contains("\"pc+de(nodeadkeys)+inet(evdev)\""), "{text}");
    assert!(text.starts_with("xkb_keymap {"), "{text}");
    assert!(text.contains("include \"evdev+aliases(qwerty)\""), "{text}");
    assert!(text.len() < 400, "{} bytes", text.len());
    let us = keymap_text(&XkbLayout::new("us", None));
    assert!(us.contains("\"pc+us+inet(evdev)\""), "{us}");
}

#[test]
fn start_records_the_text_and_logs_it() {
    let mut t = KeymapTracker::default();
    let sent = t.start("00000407");
    assert_eq!(sent.text, keymap_text(&XkbLayout::new("de", None)));
    assert_eq!(
        sent.log,
        format!(
            "keymap_sent klid=00000407 layout=de variant=- bytes={} reason=start",
            sent.text.len()
        )
    );
    assert!(
        t.check("00000407", Reason::Focus).is_none(),
        "the server has it"
    );
}

#[test]
fn a_changed_layout_is_sent_once_with_its_reason() {
    let mut t = KeymapTracker::default();
    t.start("00000409");
    let sent = t
        .check("00000407", Reason::InputLang)
        .expect("de differs from us");
    assert!(sent.log.contains("layout=de"), "{}", sent.log);
    assert!(sent.log.ends_with("reason=inputlang"), "{}", sent.log);
    assert!(
        t.check("00000407", Reason::Focus).is_none(),
        "no change, no send"
    );
    let back = t.check("00000409", Reason::Focus).expect("back to us");
    assert!(back.log.ends_with("reason=focus"), "{}", back.log);
}

#[test]
fn two_klids_with_one_layout_send_once() {
    let mut t = KeymapTracker::default();
    t.start("00000409");
    // Same text: an unlisted English (US) KLID resolves to us.
    assert!(t.check("a0010409", Reason::InputLang).is_none());
}

#[test]
fn a_fallback_is_logged_as_one() {
    let mut t = KeymapTracker::default();
    t.start("00000407");
    let sent = t
        .check("00000fff", Reason::InputLang)
        .expect("us differs from de");
    assert!(sent.log.ends_with("reason=fallback"), "{}", sent.log);
    assert!(sent.log.contains("layout=us variant=-"), "{}", sent.log);
}
