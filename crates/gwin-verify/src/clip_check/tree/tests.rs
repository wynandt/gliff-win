use super::Found;
use super::Node;
use super::Spec;
use super::difference;
use super::hdrop;
use super::parse_hdrop;
use super::parse_uri_list;
use super::pattern;
use super::read_tar;
use super::uri_list;
use super::windows_name_ok;

fn dir(path: &str) -> Node {
    Node::Dir(path.to_owned())
}

fn file(path: &str, len: usize, seed: u32) -> Node {
    Node::File {
        path: path.to_owned(),
        len,
        seed,
    }
}

fn sample() -> Spec {
    Spec {
        nodes: vec![
            dir("日本語 ünï 🎉"),
            file("日本語 ünï 🎉/données.txt", 1234, 1),
            dir("日本語 ünï 🎉/deep"),
            file("日本語 ünï 🎉/deep/empty.bin", 0, 2),
            file("日本語 ünï 🎉/deep/exact.bin", 512, 3),
            dir("日本語 ünï 🎉/vide"),
            file("loose.txt", 5, 4),
        ],
    }
}

#[test]
fn the_pattern_is_stable_and_depends_on_its_seed() {
    assert_eq!(pattern(64, 7), pattern(64, 7));
    assert_ne!(pattern(64, 7), pattern(64, 8));
    assert!(pattern(0, 7).is_empty());
    let bytes = pattern(1 << 16, 7);
    let distinct = bytes.iter().collect::<std::collections::HashSet<_>>().len();
    assert!(distinct > 200, "{distinct} distinct byte values");
}

#[test]
fn the_roots_are_the_top_level_names_once_in_order() {
    assert_eq!(sample().roots(), ["日本語 ünï 🎉", "loose.txt"]);
}

#[test]
fn a_tar_written_here_reads_back_as_the_same_tree() {
    let spec = sample();
    assert_eq!(
        read_tar(&spec.tar().expect("tar")).expect("read"),
        spec.found()
    );
}

#[test]
fn a_name_past_100_bytes_goes_through_the_prefix_field() {
    let long = format!("{}/{}/{}", "d".repeat(60), "e".repeat(60), "f.txt");
    let spec = Spec {
        nodes: vec![file(&long, 3, 1)],
    };
    assert_eq!(
        read_tar(&spec.tar().expect("tar")).expect("read"),
        spec.found()
    );
    let too_long = Spec {
        nodes: vec![file(&"x".repeat(300), 1, 1)],
    };
    assert!(too_long.tar().is_err());
}

#[test]
fn a_truncated_or_odd_archive_is_an_error() {
    let tar = sample().tar().expect("tar");
    assert!(read_tar(&tar[..700]).is_err(), "cut inside an entry");
    let mut link = tar.clone();
    link[156] = b'2';
    assert!(read_tar(&link).is_err(), "a symlink is not a file");
}

#[test]
fn the_urls_of_a_list_read_back_as_the_paths() {
    let paths = vec![
        "/home/user/.cache/x/日本語 ünï 🎉".to_owned(),
        "/tmp/a%b#c?d&e'f.txt".to_owned(),
    ];
    let body = uri_list(&paths);
    assert!(
        body.starts_with("file:///home/user/.cache/x/%E6%97%A5"),
        "{body}"
    );
    assert!(body.ends_with("\r\n"));
    assert_eq!(parse_uri_list(&body).expect("parse"), paths);
    assert_eq!(
        parse_uri_list("# note\nfile://localhost/a%20b\n\n").expect("parse"),
        ["/a b"]
    );
    assert!(parse_uri_list("http://x/y").is_err());
    assert!(parse_uri_list("file://other/y").is_err());
    assert!(parse_uri_list("file:///a%2").is_err());
}

#[test]
fn a_drop_block_reads_back_as_the_paths() {
    let paths = vec![
        r"C:\Users\user\日本語 ünï 🎉".to_owned(),
        r"C:\t\a.txt".to_owned(),
    ];
    let block = hdrop(&paths);
    assert_eq!(&block[..4], &20u32.to_le_bytes());
    assert_eq!(&block[16..20], &1u32.to_le_bytes(), "wide");
    assert_eq!(parse_hdrop(&block).expect("parse"), paths);
    let mut narrow = block.clone();
    narrow[16] = 0;
    assert_eq!(parse_hdrop(&narrow), None);
    assert_eq!(parse_hdrop(&block[..10]), None);
}

#[test]
fn windows_names_the_gate_accepts() {
    for ok in [
        "a.txt",
        "日本語",
        "Docs",
        "readme.md",
        "CONSOLE",
        "com10",
        "x y",
        "a (2).txt",
    ] {
        assert!(windows_name_ok(ok), "{ok}");
    }
    for bad in [
        "",
        "CON",
        "con.txt",
        "NUL.tar.gz",
        "COM1",
        "lpt9.log",
        "a:b",
        "a?.txt",
        "q\"",
        "t|u",
        "trail.",
        "trail ",
        "a\u{1}b",
        "a<b",
        "x*",
        "back\\slash",
    ] {
        assert!(!windows_name_ok(bad), "{bad:?}");
    }
}

fn path_of(f: &Found) -> &str {
    &f.path
}

#[test]
fn equal_trees_have_no_difference_and_a_change_is_named() {
    let want = sample().found();
    assert_eq!(difference(&want, &want, true), None);
    assert_eq!(difference(&want, &want, false), None);

    let mut missing = want.clone();
    let gone = missing.remove(2);
    assert_eq!(
        difference(&want, &missing, true),
        Some(format!("`{}` is missing", path_of(&gone)))
    );
    let mut extra = want.clone();
    extra.push(Found {
        path: "zzz".to_owned(),
        file: None,
    });
    assert_eq!(
        difference(&want, &extra, true),
        Some("`zzz` is extra".to_owned())
    );

    let mut short = want.clone();
    let slot = short
        .iter_mut()
        .find(|f| f.path.ends_with("données.txt"))
        .expect("file");
    slot.file = slot.file.map(|(size, sha)| (size - 1, sha));
    assert!(
        difference(&want, &short, true)
            .expect("diff")
            .contains("has 1233 bytes, not 1234")
    );

    let mut flipped = want.clone();
    let slot = flipped
        .iter_mut()
        .find(|f| f.path == "loose.txt")
        .expect("file");
    slot.file = slot.file.map(|(size, mut sha)| {
        sha[0] ^= 1;
        (size, sha)
    });
    let said = difference(&want, &flipped, true).expect("diff");
    assert!(said.contains("`loose.txt` differs in content"), "{said}");

    let mut kind = want.clone();
    let slot = kind
        .iter_mut()
        .find(|f| f.path.ends_with("vide"))
        .expect("dir");
    slot.file = Some((0, [0; 32]));
    assert!(
        difference(&want, &kind, true)
            .expect("diff")
            .contains("is a file, not a folder")
    );
}

fn refused() -> Spec {
    Spec {
        nodes: vec![
            file("CON.txt", 10, 1),
            file("a:b?.txt", 20, 2),
            file("trailing.", 30, 3),
            file("Readme.md", 40, 4),
            file("readme.md", 50, 5),
            dir("Docs"),
            file("Docs/x.txt", 60, 6),
        ],
    }
}

#[test]
fn renamed_files_pass_when_the_names_open_and_the_contents_match() {
    let want = refused().found();
    let renamed = Spec {
        nodes: vec![
            file("_CON.txt", 10, 1),
            file("a_b_.txt", 20, 2),
            file("trailing", 30, 3),
            file("Readme.md", 40, 4),
            file("readme (2).md", 50, 5),
            dir("Docs"),
            file("Docs/x.txt", 60, 6),
        ],
    }
    .found();
    assert_eq!(difference(&want, &renamed, false), None);
    assert!(
        difference(&want, &renamed, true).is_some(),
        "strict sees the new names"
    );
    // The names as they were are not what Windows can open.
    let said = difference(&want, &want, false).expect("diff");
    assert!(said.contains("a name Windows cannot open"), "{said}");
}

#[test]
fn a_renamed_tree_still_fails_for_content_count_and_case() {
    let want = refused().found();
    let ok = |nodes: Vec<Node>| difference(&want, &Spec { nodes }.found(), false);
    let base = |extra: Vec<Node>| {
        let mut nodes = vec![
            file("_CON.txt", 10, 1),
            file("a_b_.txt", 20, 2),
            file("trailing", 30, 3),
            file("Readme.md", 40, 4),
            file("readme (2).md", 50, 5),
            dir("Docs"),
            file("Docs/x.txt", 60, 6),
        ];
        nodes.extend(extra);
        nodes
    };
    assert_eq!(ok(base(vec![])), None);
    let mut wrong = base(vec![]);
    wrong[1] = file("a_b_.txt", 20, 99);
    assert!(
        ok(wrong)
            .expect("diff")
            .contains("no file with the 20 bytes")
    );
    assert!(
        ok(base(vec![file("more.txt", 1, 1)]))
            .expect("diff")
            .contains("too many")
    );
    assert!(
        ok(base(vec![dir("Other")]))
            .expect("diff")
            .contains("folders")
    );
    let mut clash = base(vec![]);
    clash[4] = file("README.MD", 50, 5);
    assert!(ok(clash).expect("diff").contains("only in case"));
}

// The tools on the remote machine, against the gate's own reader and writer.
#[cfg(unix)]
mod with_tar {
    use std::process::Command;
    use std::process::Stdio;

    use super::super::Spec;
    use super::super::list_disk;
    use super::super::read_tar;
    use super::sample;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gwin-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_tar_on_this_machine_extracts_our_archive_to_the_same_tree() {
        let spec = sample();
        let dir = temp("extract");
        let mut child = Command::new("tar")
            .args(["-x", "-C"])
            .arg(&dir)
            .args(["-f", "-"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("tar");
        std::io::Write::write_all(
            &mut child.stdin.take().expect("stdin"),
            &spec.tar().expect("tar"),
        )
        .expect("write");
        assert!(child.wait().expect("wait").success());
        let roots: Vec<_> = spec.roots().iter().map(|r| dir.join(r)).collect();
        assert_eq!(list_disk(&roots).expect("walk"), spec.found());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn the_archives_of_this_tar_read_in_every_format() {
        let mut nodes = sample().nodes;
        // Names past 100 bytes: GNU long names, pax paths.
        let deep = format!("{}/{}", "g".repeat(90), "h".repeat(90));
        nodes.push(super::dir(deep.split('/').next().expect("first")));
        nodes.push(super::dir(&deep));
        nodes.push(super::file(
            &format!("{deep}/{}.txt", "i".repeat(40)),
            77,
            9,
        ));
        let spec = Spec { nodes };
        let dir = temp("formats");
        spec.write_to(&dir).expect("write");
        for format in ["gnu", "posix", "ustar", "oldgnu"] {
            let out = Command::new("tar")
                .arg(format!("--format={format}"))
                .arg("-c")
                .arg("-C")
                .arg(&dir)
                .args(spec.roots())
                .output()
                .expect("tar");
            if !out.status.success() {
                // ustar cannot hold the longest names: that is the format's limit.
                assert_eq!(format, "ustar");
                continue;
            }
            assert_eq!(
                read_tar(&out.stdout).expect(format),
                spec.found(),
                "{format}"
            );
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
