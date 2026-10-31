use super::file_name;
use super::timestamp;

#[test]
fn timestamps_are_utc_calendar_times() {
    assert_eq!(timestamp(0), "1970-01-01T00:00:00Z", "the epoch");
    // `date -u -d @1792789200` = Fri Oct 23 2026 21:00:00 UTC.
    assert_eq!(timestamp(1_792_789_200), "2026-10-23T21:00:00Z", "today");
    // 2024-02-29 12:00:00 UTC, a leap day.
    assert_eq!(timestamp(1_709_208_000), "2024-02-29T12:00:00Z", "leap day");
    assert_eq!(
        timestamp(951_868_799),
        "2000-02-29T23:59:59Z",
        "century leap"
    );
}

#[test]
fn one_file_per_utc_day() {
    assert_eq!(
        file_name(1_792_789_200),
        "gliff-windows-2026-10-23.log",
        "today's file"
    );
    assert_eq!(
        file_name(1_792_800_000),
        "gliff-windows-2026-10-24.log",
        "past midnight UTC is the next file"
    );
}

#[test]
fn the_log_is_bounded_in_files_kept_and_in_size() {
    use super::Budget;
    use super::Fate;
    use super::MAX_FILE_BYTES;

    let dir = crate::test_dir::TestDir::new();
    let names = [
        "gliff-windows-2026-10-01.log",
        "gliff-windows-2026-10-02.log",
        "gliff-windows-2026-10-03.log",
        "gliff-windows-2026-10-04.log",
        // Not the client's log files: never removed.
        "gliff-windows-2026-10-0x.log",
        "gliff-windows-2026-10-01.log.bak",
        "notes.txt",
    ];
    for name in names {
        std::fs::write(dir.path().join(name), "x").expect("a file");
    }
    super::prune(dir.path(), 2);
    let mut left: Vec<String> = std::fs::read_dir(dir.path())
        .expect("the folder")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .into_string()
                .expect("a name")
        })
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "gliff-windows-2026-10-01.log.bak",
            "gliff-windows-2026-10-03.log",
            "gliff-windows-2026-10-04.log",
            "gliff-windows-2026-10-0x.log",
            "notes.txt",
        ],
        "the newest two log files stay, the rest of the folder is not touched"
    );

    let mut budget = Budget {
        used: 0,
        told: false,
    };
    assert_eq!(budget.admit(MAX_FILE_BYTES), Fate::Write, "up to the limit");
    assert_eq!(budget.admit(1), Fate::Notice, "the first line over says so");
    assert_eq!(budget.admit(1), Fate::Drop, "then nothing more is written");
}
