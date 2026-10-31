use std::path::Path;
use std::path::PathBuf;

use gliff_proto::ClipboardFile;
use gliff_proto::clipboard as proto;
use gliff_proto::clipboard::FilesError;
use gliff_transport::clipboard::files::ListError;

use super::Spool;
use super::extended;
use super::hdrop_bytes;
use super::list_files;
use super::parse_hdrop;
use super::sanitize_component;
use super::spool_paths;
use super::top_level_units;
use super::validate_offer;

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gwin-clip-files-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        std::fs::write(&path, bytes).expect("write the file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file(path: &str, size: u64) -> ClipboardFile {
    ClipboardFile {
        path: path.to_owned(),
        size,
        dir: false,
    }
}

fn dir(path: &str) -> ClipboardFile {
    ClipboardFile {
        path: path.to_owned(),
        size: 0,
        dir: true,
    }
}

// Sanitizer.

#[test]
fn forbidden_characters_become_underscores() {
    assert_eq!(
        sanitize_component("a<b>c:d\"e|f?g*h\\i/j"),
        "a_b_c_d_e_f_g_h_i_j"
    );
    assert_eq!(sanitize_component("tab\there\u{7F}"), "tab_here_");
    assert_eq!(sanitize_component("日本 😀.txt"), "日本 😀.txt");
}

#[test]
fn trailing_dots_and_spaces_go_and_empty_becomes_underscore() {
    assert_eq!(sanitize_component("name. . "), "name");
    assert_eq!(sanitize_component(".hidden"), ".hidden");
    assert_eq!(sanitize_component(".."), "_");
    assert_eq!(sanitize_component("."), "_");
    assert_eq!(sanitize_component(""), "_");
    assert_eq!(sanitize_component("   "), "_");
}

#[test]
fn reserved_device_names_get_a_prefix_with_or_without_an_extension() {
    for name in [
        "CON",
        "con",
        "Prn.txt",
        "AUX.tar.gz",
        "NUL ",
        "com1",
        "LPT9.log",
        "COM\u{B9}",
        "CONIN$",
        "conout$.x",
        "nul .txt",
    ] {
        let clean = sanitize_component(name);
        assert!(clean.starts_with('_'), "{name:?} became {clean:?}");
    }
    for name in [
        "CONSOLE",
        "COM10",
        "COM",
        "LPT",
        "com0x",
        "xCON",
        "CON.txt.d",
    ] {
        let clean = sanitize_component(name);
        assert_eq!(
            clean.starts_with('_'),
            name == "CON.txt.d",
            "{name:?} became {clean:?}"
        );
    }
    assert_eq!(sanitize_component("com0"), "_com0");
}

#[test]
fn long_parts_are_shortened_to_255_units_and_keep_their_extension() {
    let long = "x".repeat(400) + ".jpeg";
    let clean = sanitize_component(&long);
    assert_eq!(clean.encode_utf16().count(), 255);
    assert!(clean.ends_with(".jpeg"));
    let astral = "😀".repeat(200);
    let clean = sanitize_component(&astral);
    assert!(clean.encode_utf16().count() <= 255);
    assert!(
        clean.chars().all(|c| c == '😀'),
        "never cut inside a character"
    );
    let no_dot = "y".repeat(300);
    assert_eq!(sanitize_component(&no_dot).len(), 255);
    let huge_extension = format!("a.{}", "e".repeat(300));
    assert!(sanitize_component(&huge_extension).encode_utf16().count() <= 255);
}

#[test]
fn spool_paths_keep_indexes_and_sanitize_every_part() {
    let files = [
        dir("d*r"),
        file("d*r/CON.txt", 1),
        dir("d*r/sub."),
        file("d*r/sub./a?b", 2),
        file("plain", 3),
    ];
    proto::validate_files(&files).expect("offer is valid");
    let paths = spool_paths(&files);
    let expected: Vec<PathBuf> = ["d_r", "d_r/_CON.txt", "d_r/sub", "d_r/sub/a_b", "plain"]
        .iter()
        .map(|p| p.split('/').collect())
        .collect();
    assert_eq!(paths, expected);
}

#[test]
fn colliding_names_get_numbers_before_the_extension() {
    let files = [
        file("a.txt", 1),
        file("A.TXT", 1),
        file("a?.txt", 1),
        file("a_.txt", 1),
        file("noext", 1),
        file("NoExt", 1),
    ];
    let paths = spool_paths(&files);
    let names: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "a.txt",
            "A (2).TXT",
            "a_.txt",
            "a_ (2).txt",
            "noext",
            "NoExt (2)"
        ]
    );
}

#[test]
fn directories_that_collide_keep_their_children_apart() {
    let files = [
        dir("d"),
        dir("D"),
        file("d/x", 1),
        file("D/x", 1),
        file("D/y", 1),
    ];
    let paths = spool_paths(&files);
    let names: Vec<String> = paths
        .iter()
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    assert_eq!(names, ["d", "D (2)", "d/x", "D (2)/x", "D (2)/y"]);
}

#[test]
fn a_child_listed_before_its_directory_still_lands_inside_it() {
    let files = [file("d/a", 1), dir("d")];
    let paths = spool_paths(&files);
    assert_eq!(paths[0], PathBuf::from("d").join("a"));
    assert_eq!(paths[1], PathBuf::from("d"));
}

#[test]
fn the_sanitized_offer_has_distinct_names_whatever_the_input() {
    let files: Vec<ClipboardFile> = ["a?", "a*", "A_", "a_", "CON", "_CON", "_con"]
        .iter()
        .map(|name| file(name, 0))
        .collect();
    let paths = spool_paths(&files);
    let lower: std::collections::HashSet<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().to_lowercase())
        .collect();
    assert_eq!(lower.len(), paths.len(), "{paths:?}");
}

// Long paths.

#[test]
fn long_drive_and_unc_paths_get_the_extended_prefix() {
    let tail = "a".repeat(250);
    let drive = PathBuf::from(format!(r"C:\Users\user\{tail}"));
    assert_eq!(
        extended(&drive),
        PathBuf::from(format!(r"\\?\C:\Users\user\{tail}"))
    );
    let unc = PathBuf::from(format!(r"\\server\share\{tail}"));
    assert_eq!(
        extended(&unc),
        PathBuf::from(format!(r"\\?\UNC\server\share\{tail}"))
    );
    let done = PathBuf::from(format!(r"\\?\C:\{tail}"));
    assert_eq!(extended(&done), done);
}

#[test]
fn short_relative_and_unix_paths_are_left_alone() {
    for path in [r"C:\short", r"relative\path", "/unix/path", ""] {
        assert_eq!(extended(Path::new(path)), PathBuf::from(path));
    }
    let long_unix = PathBuf::from(format!("/{}", "a".repeat(300)));
    assert_eq!(extended(&long_unix), long_unix);
    let long_relative = PathBuf::from("a".repeat(300));
    assert_eq!(extended(&long_relative), long_relative);
}

// Walker.

#[test]
fn a_tree_is_listed_in_name_order_with_slash_paths() {
    let tmp = TempDir::new("tree");
    tmp.write("top/b.txt", b"bb");
    tmp.write("top/a.txt", b"a");
    tmp.write("top/sub/c.bin", &[0; 10]);
    std::fs::create_dir_all(tmp.path().join("top/empty")).expect("empty dir");
    let single = tmp.write("single.txt", b"xyz");
    let listed = list_files(&[tmp.path().join("top"), single]).expect("list");
    let summary: Vec<(&str, u64, bool)> = listed
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.size, e.dir))
        .collect();
    assert_eq!(
        summary,
        [
            ("top", 0, true),
            ("top/a.txt", 1, false),
            ("top/b.txt", 2, false),
            ("top/empty", 0, true),
            ("top/sub", 0, true),
            ("top/sub/c.bin", 10, false),
            ("single.txt", 3, false),
        ]
    );
    assert_eq!(listed.paths.len(), listed.entries.len());
    assert_eq!(listed.paths[1], tmp.path().join("top").join("a.txt"));
    assert_eq!(proto::top_level(&listed.entries).count(), 2);
}

#[cfg(unix)]
#[test]
fn symbolic_links_inside_a_directory_are_skipped() {
    let tmp = TempDir::new("links");
    tmp.write("d/real.txt", b"1");
    std::os::unix::fs::symlink(tmp.path().join("d"), tmp.path().join("d/loop")).expect("link dir");
    std::os::unix::fs::symlink(tmp.path().join("d/real.txt"), tmp.path().join("d/alias"))
        .expect("link file");
    let listed = list_files(&[tmp.path().join("d")]).expect("list");
    let names: Vec<&str> = listed.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["d", "d/real.txt"]);
}

#[test]
fn a_root_without_a_name_or_that_is_gone_fails_the_whole_listing() {
    let tmp = TempDir::new("errors");
    let ok = tmp.write("ok.txt", b"1");
    assert!(matches!(
        list_files(&[PathBuf::from("/")]),
        Err(ListError::NoName(_))
    ));
    assert!(matches!(
        list_files(&[ok.clone(), tmp.path().join("gone.txt")]),
        Err(ListError::Io(..))
    ));
}

#[test]
fn roots_with_one_name_are_offered_numbered() {
    let tmp = TempDir::new("dupes");
    let first = tmp.write("one/same.txt", b"1");
    let second = tmp.write("two/same.txt", b"2");
    let third = tmp.write("three/same.txt", b"3");
    let listed = list_files(&[first.clone(), second.clone(), third]).expect("listed");
    let names: Vec<&str> = listed.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["same.txt", "same (2).txt", "same (3).txt"]);
    assert_eq!(
        listed.paths[1],
        extended(&second),
        "the content is the second's"
    );
    // Offered twice, the same path is the same file.
    let twice = list_files(&[first.clone(), first]).expect("listed");
    assert_eq!(twice.entries[1].path, "same (2).txt");
}

#[test]
fn two_folders_with_one_name_keep_their_children_apart() {
    let tmp = TempDir::new("dupe-dirs");
    tmp.write("one/pics/a.png", b"1");
    tmp.write("two/pics/b.png", b"2");
    let listed =
        list_files(&[tmp.path().join("one/pics"), tmp.path().join("two/pics")]).expect("listed");
    let names: Vec<&str> = listed.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["pics", "pics/a.png", "pics (2)", "pics (2)/b.png"]);
    proto::validate_files(&listed.entries).expect("the server accepts it");
}

#[test]
fn a_folder_with_a_dot_is_numbered_after_its_whole_name() {
    let tmp = TempDir::new("dot-dirs");
    tmp.write("one/v1.2/a", b"1");
    tmp.write("two/v1.2/b", b"2");
    let listed =
        list_files(&[tmp.path().join("one/v1.2"), tmp.path().join("two/v1.2")]).expect("listed");
    assert_eq!(listed.entries[2].path, "v1.2 (2)");
}

#[test]
fn too_many_entries_fail_the_listing() {
    let tmp = TempDir::new("many");
    std::fs::create_dir_all(tmp.path().join("d")).expect("dir");
    for i in 0..=proto::MAX_FILE_ENTRIES {
        std::fs::write(tmp.path().join(format!("d/{i}")), b"").expect("file");
    }
    assert!(matches!(
        list_files(&[tmp.path().join("d")]),
        Err(ListError::Files(_))
    ));
}

// Spool.

#[test]
fn spools_are_separate_directories_and_vanish_on_drop() {
    let tmp = TempDir::new("spool");
    let base = tmp.path().join("clipboard");
    let first = Spool::create_in(&base).expect("first");
    let second = Spool::create_in(&base).expect("second");
    assert_ne!(first.dir(), second.dir());
    std::fs::write(first.dir().join("f"), b"x").expect("write into spool");
    let first_dir = first.dir().to_path_buf();
    drop(first);
    assert!(!first_dir.exists());
    assert!(second.dir().exists());
}

/// A spool directory with a file in it, named as a run of `pid` would.
fn leave(base: &Path, pid: u32, n: u32) -> PathBuf {
    let dir = base.join(format!("{pid}-{n}"));
    std::fs::create_dir_all(&dir).expect("a spool directory");
    std::fs::write(dir.join("a.txt"), b"x").expect("a file");
    dir
}

#[test]
fn the_sweep_removes_the_directories_of_ended_runs_only() {
    let tmp = TempDir::new("sweep");
    let base = tmp.path().join("clipboard");
    Spool::sweep_stale(&base, |_| false);
    let ended = leave(&base, 4_000_001, 0);
    let ended_too = leave(&base, 4_000_001, 7);
    let running = leave(&base, 4_000_002, 0);
    let own = leave(&base, std::process::id(), 99);
    let stray_dir = base.join("notes");
    std::fs::create_dir_all(&stray_dir).expect("a stray directory");
    let stray_file = base.join("5-6");
    std::fs::write(&stray_file, b"x").expect("a file that looks like a spool");
    Spool::sweep_stale(&base, |pid| pid == 4_000_002);
    assert!(!ended.exists() && !ended_too.exists(), "ended runs go");
    assert!(running.exists(), "a running client keeps its files");
    assert!(own.exists(), "so does this one, whatever `alive` says");
    assert!(
        stray_dir.exists() && stray_file.exists(),
        "unknown names stay"
    );
}

#[test]
fn removing_own_spools_leaves_the_others() {
    let tmp = TempDir::new("own");
    let base = tmp.path().join("clipboard");
    Spool::remove_own(&base);
    let mine = Spool::create_in(&base).expect("a spool");
    let mine_dir = mine.dir().to_path_buf();
    let other = leave(&base, 4_000_003, 0);
    Spool::remove_own(&base);
    assert!(!mine_dir.exists());
    assert!(other.exists());
    drop(mine);
}

#[test]
fn a_file_list_round_trips_through_the_hdrop_layout() {
    let paths = vec![
        PathBuf::from(r"C:\spool\1-1\a.txt"),
        PathBuf::from("D:\\dossier\\r\u{e9}sum\u{e9}.pdf"),
    ];
    let bytes = hdrop_bytes(&paths);
    assert_eq!(&bytes[..4], &20u32.to_le_bytes(), "the offset of the names");
    assert_eq!(&bytes[16..20], &1u32.to_le_bytes(), "wide characters");
    assert_eq!(
        &bytes[bytes.len() - 4..],
        &[0, 0, 0, 0],
        "the list ends in two NULs"
    );
    assert_eq!(parse_hdrop(&bytes), Some(paths), "the same paths");
}

#[test]
fn a_bad_hdrop_is_refused() {
    assert_eq!(parse_hdrop(&[0; 10]), None, "shorter than the header");
    let mut narrow = hdrop_bytes(&[PathBuf::from("a")]);
    narrow[16] = 0;
    assert_eq!(parse_hdrop(&narrow), None, "narrow characters");
    let mut early = hdrop_bytes(&[PathBuf::from("a")]);
    early[0] = 4;
    assert_eq!(parse_hdrop(&early), None, "an offset inside the header");
    assert_eq!(
        parse_hdrop(&hdrop_bytes(&[])),
        Some(Vec::new()),
        "an empty list"
    );
}

#[test]
fn the_top_level_budget_follows_the_spool_directory_length() {
    // 259 units in all, less the directory, a separator, within 32..=255.
    assert_eq!(
        top_level_units(Path::new("")),
        255,
        "capped at one name's limit"
    );
    assert_eq!(top_level_units(Path::new(r"C:\s")), 254);
    let dir = PathBuf::from(format!(r"C:\{}", "x".repeat(200)));
    assert_eq!(top_level_units(&dir), 259 - 203 - 1);
    let huge = PathBuf::from(format!(r"C:\{}", "x".repeat(400)));
    assert_eq!(top_level_units(&huge), 32, "never below a usable name");
}

// Checking an offer. These read the paths as text, so they say the same on
// every OS: Windows reads `a:b?.txt` as a drive and a name, and upstream's
// check, which uses the host's path parser, refused the whole offer there.

fn offer_of(paths: &[&str]) -> Vec<ClipboardFile> {
    paths.iter().map(|p| file(p, 1)).collect()
}

#[test]
fn names_that_are_legal_on_linux_are_not_refused() {
    for good in [
        "a:b?.txt",
        "C:evil",
        "C:",
        "d:r/x",
        "con",
        "CON.txt",
        "aux.txt",
        "COM1",
        "trail.",
        "trail ",
        "dir./file",
        " lead",
        "a<b>c\"d|e*f",
        "back\\slash",
        "..\\x",
        "a\\..\\b",
        "\\\\server\\share",
        "...",
        ".hidden",
        "日本/😀.txt",
        "a\u{7F}b",
    ] {
        assert_eq!(validate_offer(&offer_of(&[good])), Ok(()), "{good:?}");
    }
}

#[test]
fn the_hardware_gate_names_are_accepted_together() {
    let files = offer_of(&[
        "CON.txt",
        "a:b?.txt",
        "trail.",
        "Docs",
        "Docs/Readme.md",
        "Docs/readme.md",
        "docs",
        "docs/x",
        "Readme.md",
        "readme.md",
    ]);
    assert_eq!(validate_offer(&files), Ok(()));
}

#[test]
fn traversal_and_absolute_paths_are_refused_on_every_os() {
    for bad in [
        "",
        "/",
        "/abs",
        "/etc/passwd",
        "../x",
        "..",
        ".",
        "./a",
        "a/./b",
        "a/../b",
        "a/..",
        "a/",
        "a//b",
        "//a",
        "a\0b",
        "\0",
        "a/\0",
    ] {
        assert!(
            matches!(
                validate_offer(&offer_of(&[bad])),
                Err(FilesError::BadPath(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn backslashes_are_name_characters_that_the_sanitizer_removes() {
    // A Linux name is bytes between `/`; a backslash in it is not a Windows
    // separator once the sanitizer has run, so none of these can leave the
    // spool or form a drive or a parent part.
    let paths = [
        "..\\x",
        "a\\..\\b",
        "C:\\evil",
        "\\\\server\\share\\f",
        "C:evil",
        "d/C:evil",
    ];
    let files = offer_of(&paths);
    assert_eq!(validate_offer(&files), Ok(()));
    for out in spool_paths(&files) {
        let text = out.to_str().expect("utf-8").replace('/', "|");
        let parts: Vec<&str> = out.iter().map(|p| p.to_str().expect("utf-8")).collect();
        assert!(!parts.is_empty(), "{text}");
        for part in parts {
            assert!(part != "." && part != ".." && !part.is_empty(), "{text}");
            assert!(!part.contains([':', '\\', '?', '*']), "{text}");
            assert!(!part.ends_with(['.', ' ']), "{text}");
        }
    }
}

#[test]
fn the_caps_and_duplicates_still_apply() {
    use gliff_proto::clipboard::MAX_FILE_ENTRIES;
    use gliff_proto::clipboard::MAX_FILE_PATH_BYTES;
    use gliff_proto::clipboard::MAX_FILES_TOTAL;

    assert!(matches!(
        validate_offer(&offer_of(&["x", "x"])),
        Err(FilesError::BadPath(_))
    ));
    assert!(matches!(
        validate_offer(&[dir("d"), file("d/x", 1)]),
        Ok(())
    ));
    let sized_dir = ClipboardFile {
        path: "d".into(),
        size: 1,
        dir: true,
    };
    assert!(matches!(
        validate_offer(&[sized_dir]),
        Err(FilesError::BadPath(_))
    ));
    assert!(matches!(
        validate_offer(&[file("x", MAX_FILES_TOTAL + 1)]),
        Err(FilesError::TooLarge(_))
    ));
    let many: Vec<_> = (0..=MAX_FILE_ENTRIES)
        .map(|i| file(&format!("f{i}"), 0))
        .collect();
    assert!(matches!(validate_offer(&many), Err(FilesError::TooMany(_))));
    let name = "x".repeat(200);
    let long: Vec<_> = (0..MAX_FILE_PATH_BYTES / 200 + 1)
        .map(|i| file(&format!("{i}{name}"), 0))
        .collect();
    assert!(matches!(
        validate_offer(&long),
        Err(FilesError::PathsTooLong(_))
    ));
}

#[test]
fn the_offer_check_does_not_use_the_host_path_parser() {
    let source = include_str!("offer.rs");
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in [
        "std::path",
        "Path::",
        "PathBuf",
        "&Path",
        "components(",
        "validate_files(",
    ] {
        assert!(!code.contains(banned), "offer.rs uses {banned}");
    }
}
