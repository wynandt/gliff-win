//! The remote machine's `gwin-keys` log: `timestamp<TAB>n<TAB>what`, one line per
//! bind that fired. Pure.

/// One line of the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub ts: String,
    /// The log's own running number; the harness reads what is new by it.
    pub n: u64,
    pub what: String,
}

/// The entries, and how many lines could not be read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub entries: Vec<Entry>,
    pub malformed: usize,
}

pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.splitn(3, '\t');
        let entry = match (fields.next(), fields.next(), fields.next()) {
            (Some(ts), Some(n), Some(what)) => n.parse().ok().map(|n| Entry {
                ts: ts.to_owned(),
                n,
                what: what.to_owned(),
            }),
            _ => None,
        };
        match entry {
            Some(entry) => parsed.entries.push(entry),
            None => parsed.malformed += 1,
        }
    }
    parsed
}

/// The entries after number `mark`.
pub fn since(entries: &[Entry], mark: u64) -> Vec<Entry> {
    entries.iter().filter(|e| e.n > mark).cloned().collect()
}

/// The highest number in the log, or 0.
pub fn last_n(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| e.n).max().unwrap_or(0)
}

/// Whether a log entry is a bind that fired (`SUPER+L`, `probe:none`,
/// `mouse:272 x,y`), as against the raw key lines (`key:down:125`,
/// `typed:a`) the log also carries. A step that asks "which binds fired" is
/// not answered by those: Win+L is one bind however many keys went down.
pub fn is_bind(what: &str) -> bool {
    !what.starts_with("key:") && !what.starts_with("typed:")
}

/// The position of a `mouse:272 x,y` entry (relative to the session's
/// output), or `None` for any other entry.
pub fn cursor_of(what: &str, button_code: u32) -> Option<(f64, f64)> {
    let rest = what.strip_prefix(&format!("mouse:{button_code} "))?;
    let (x, y) = rest.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::Entry;
    use super::cursor_of;
    use super::is_bind;
    use super::last_n;
    use super::parse;
    use super::since;

    #[test]
    fn lines_split_on_tabs_and_keep_tabs_in_the_last_field() {
        let p = parse("12:00:01\t1\tSUPER+Y\n12:00:02\t2\tmouse:272 10,20\n");
        assert_eq!(p.malformed, 0, "both lines are well formed");
        assert_eq!(
            p.entries[1],
            Entry {
                ts: "12:00:02".into(),
                n: 2,
                what: "mouse:272 10,20".into()
            },
            "the second line"
        );
    }

    #[test]
    fn bad_lines_are_counted_not_dropped_silently() {
        let p = parse("garbage\n12:00:01\tx\tSUPER+Y\n\n12:00:02\t3\tCTRL+Q\n");
        assert_eq!(p.malformed, 2, "no tabs, and a non-number");
        assert_eq!(p.entries.len(), 1, "one good line");
    }

    #[test]
    fn since_takes_what_is_newer_than_the_mark() {
        let p = parse("a\t1\tx\nb\t2\ty\nc\t3\tz\n");
        assert_eq!(since(&p.entries, 1).len(), 2, "entries 2 and 3");
        assert_eq!(last_n(&p.entries), 3, "the highest number");
        assert_eq!(last_n(&[]), 0, "an empty log is 0");
    }

    #[test]
    fn cursor_entries_carry_the_position() {
        assert_eq!(cursor_of("mouse:272 100.5,200", 272), Some((100.5, 200.0)));
        assert_eq!(cursor_of("mouse:273 1,2", 272), None, "another button");
        assert_eq!(cursor_of("SUPER+Y", 272), None, "not a mouse entry");
    }

    #[test]
    fn raw_key_lines_are_not_binds_but_everything_else_is() {
        for raw in ["key:down:125", "key:up:38", "typed:a"] {
            assert!(!is_bind(raw), "{raw}");
        }
        for bind in ["SUPER+L", "probe:none", "mouse:272 10,20", "A"] {
            assert!(is_bind(bind), "{bind}");
        }
    }
}
