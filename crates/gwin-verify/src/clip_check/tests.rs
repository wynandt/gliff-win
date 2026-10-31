use std::time::Duration;

use super::ClipCase;
use super::Harness;
use super::Timing;
use super::WindowsForm;
use super::picture::Picture;
use super::run;
use super::tree::Found;
use super::tree::Spec;
use super::tree::windows_name_ok;
use crate::line::Line;

const TIMING: Timing = Timing {
    startup: Duration::from_millis(10),
    arrive: Duration::from_millis(100),
    poll: Duration::from_millis(25),
    quiet: Duration::from_millis(10),
    arrive_files: Duration::from_millis(200),
};

/// What the fake sync gets wrong.
#[derive(Default)]
struct Faults {
    /// The client ends at once.
    client_exits: bool,
    /// Nothing goes from Windows to the remote machine.
    no_windows_to_remote: bool,
    /// CRLF is not converted to LF on the way to the remote machine.
    keeps_crlf: bool,
    /// The Windows clipboard gets a foreign text after the quiet time.
    echo_after_quiet: bool,
    /// A picture from Windows arrives upside down.
    flips_pictures: bool,
    /// A picture from the remote machine loses its alpha on Windows.
    drops_alpha: bool,
    /// Nothing from the remote machine is kept out of the history.
    history_open: bool,
    /// The remote machine's picture is replaced after the quiet time.
    picture_echo_after_quiet: bool,
    /// A file from the remote machine arrives with one byte changed.
    corrupts_files: bool,
    /// The files from the remote machine come without a drop effect.
    no_drop_effect: bool,
    /// The Windows clipboard names other files after the quiet time.
    files_echo_after_quiet: bool,
    /// Names Windows cannot hold are kept as they are.
    keeps_refused_names: bool,
    /// The client leaves a spool directory behind.
    leaves_spool: bool,
}

/// What the client makes of a tree that came from the remote machine: names
/// Windows cannot hold are changed, and the later of two names that differ
/// only in case gets a number.
fn on_windows(found: &[Found], keep_names: bool) -> Vec<Found> {
    use std::collections::HashMap;
    use std::collections::HashSet;
    if keep_names {
        return found.to_vec();
    }
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for entry in found {
        let (parent, name) = match entry.path.rsplit_once('/') {
            Some((parent, name)) => (renamed[parent].clone() + "/", name),
            None => (String::new(), entry.path.as_str()),
        };
        let mut name = name.replace(['<', '>', ':', '"', '|', '?', '*'], "_");
        while !windows_name_ok(&name) {
            name = if name.ends_with(['.', ' ']) {
                name.trim_end_matches(['.', ' ']).to_owned() + "_"
            } else {
                format!("_{name}")
            };
        }
        let base = name.clone();
        let mut n = 1;
        while !seen.insert(format!("{parent}{name}").to_lowercase()) {
            n += 1;
            name = format!("{base} ({n})");
        }
        let path = format!("{parent}{name}");
        renamed.insert(entry.path.clone(), path.clone());
        out.push(Found {
            path,
            file: entry.file,
        });
    }
    out.sort();
    out
}

/// Two clipboards that follow each other, as the client makes them, plus
/// the faults above. `sleep` only counts: no real time passes.
#[derive(Default)]
struct Linked {
    faults: Faults,
    windows: Option<String>,
    remote: Option<String>,
    windows_picture: Option<Picture>,
    remote_picture: Option<Picture>,
    forms: Vec<WindowsForm>,
    windows_tree: Option<Vec<Found>>,
    remote_tree: Option<Vec<Found>>,
    cleared: bool,
    launched: bool,
    closed: bool,
    slept: Duration,
}

impl Harness for Linked {
    fn launch(&mut self) -> anyhow::Result<()> {
        self.launched = true;
        Ok(())
    }
    fn client_running(&mut self) -> bool {
        !self.faults.client_exits
    }
    fn windows_set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.windows = Some(text.to_owned());
        if !self.faults.no_windows_to_remote {
            self.remote = Some(if self.faults.keeps_crlf {
                text.to_owned()
            } else {
                text.replace("\r\n", "\n")
            });
        }
        Ok(())
    }
    fn windows_text(&mut self) -> anyhow::Result<Option<String>> {
        Ok(self.windows.clone())
    }
    fn remote_set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.remote = Some(text.to_owned());
        self.windows = Some(text.to_owned());
        Ok(())
    }
    fn remote_text(&mut self) -> anyhow::Result<Option<String>> {
        Ok(self.remote.clone())
    }
    fn windows_set_picture(&mut self, picture: &Picture, form: WindowsForm) -> anyhow::Result<()> {
        self.forms.push(form);
        self.windows_picture = Some(picture.clone());
        if !self.faults.no_windows_to_remote {
            let mut sent = picture.clone();
            if self.faults.flips_pictures {
                let row = sent.width as usize * 4;
                let rows: Vec<&[u8]> = picture.rgba.chunks_exact(row).rev().collect();
                sent.rgba = rows.concat();
            }
            self.remote_picture = Some(sent);
        }
        Ok(())
    }
    fn windows_picture(&mut self) -> anyhow::Result<Option<Picture>> {
        Ok(self.windows_picture.clone())
    }
    fn windows_history_excluded(&mut self) -> anyhow::Result<bool> {
        Ok(!self.faults.history_open)
    }
    fn remote_set_picture(&mut self, picture: &Picture) -> anyhow::Result<()> {
        self.remote_picture = Some(picture.clone());
        let mut seen = picture.clone();
        if self.faults.drops_alpha {
            seen.rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(|p| p[3] = 255);
        }
        self.windows_picture = Some(seen);
        Ok(())
    }
    fn remote_picture(&mut self) -> anyhow::Result<Option<Picture>> {
        Ok(self.remote_picture.clone())
    }
    fn windows_set_files(&mut self, spec: &Spec) -> anyhow::Result<()> {
        self.windows_tree = Some(spec.found());
        if !self.faults.no_windows_to_remote {
            self.remote_tree = Some(spec.found());
        }
        Ok(())
    }
    fn windows_files(&mut self) -> anyhow::Result<Option<Vec<Found>>> {
        Ok(self.windows_tree.clone())
    }
    fn windows_drop_effect(&mut self) -> anyhow::Result<Option<u32>> {
        Ok((!self.faults.no_drop_effect).then_some(1))
    }
    fn remote_set_files(&mut self, spec: &Spec) -> anyhow::Result<()> {
        self.remote_tree = Some(spec.found());
        let mut seen = on_windows(&spec.found(), self.faults.keeps_refused_names);
        if self.faults.corrupts_files
            && let Some((_, sha)) = seen.iter_mut().find_map(|f| f.file.as_mut())
        {
            sha[0] ^= 1;
        }
        self.windows_tree = Some(seen);
        Ok(())
    }
    fn remote_files(&mut self) -> anyhow::Result<Option<Vec<Found>>> {
        Ok(self.remote_tree.clone())
    }
    fn windows_spool_left(&mut self) -> anyhow::Result<Vec<String>> {
        Ok(if self.faults.leaves_spool {
            vec!["4242-1".to_owned()]
        } else {
            Vec::new()
        })
    }
    fn clear_files(&mut self) -> anyhow::Result<()> {
        self.cleared = true;
        Ok(())
    }
    fn close(&mut self) {
        self.closed = true;
    }
    fn sleep(&mut self, d: Duration) {
        self.slept += d;
        if self.faults.files_echo_after_quiet && d == TIMING.quiet {
            self.windows_tree = Some(vec![Found {
                path: "somebody else".to_owned(),
                file: None,
            }]);
        }
        if self.faults.echo_after_quiet && d == TIMING.quiet {
            self.windows = Some("somebody else".to_owned());
        }
        if self.faults.picture_echo_after_quiet && d == TIMING.quiet {
            self.remote_picture = Some(Picture::opaque(2, 2, 0));
        }
    }
}

fn text_run(h: &mut Linked) -> Vec<Line> {
    run(h, &[ClipCase::Text], "n1", TIMING)
}

fn field<'a>(line: &'a Line, key: &str) -> Option<&'a serde_json::Value> {
    line.metrics.get(key)
}

fn summary(lines: &[Line]) -> &Line {
    lines.last().expect("a summary line")
}

#[test]
fn linked_clipboards_pass_every_text_step() {
    let mut h = Linked::default();
    let lines = text_run(&mut h);
    assert!(lines.iter().all(|l| l.ok), "{lines:?}");
    let steps: Vec<_> = lines
        .iter()
        .filter_map(|l| field(l, "step").and_then(|s| s.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            "ascii-windows-to-remote",
            "ascii-remote-to-windows",
            "unicode-windows-to-remote",
            "unicode-remote-to-windows",
            "crlf-windows-to-remote",
            "lf-remote-to-windows",
            "quiet"
        ],
        "every step ran, in order"
    );
    assert_eq!(
        field(summary(&lines), "passed"),
        Some(&1.into()),
        "one case"
    );
    assert!(h.launched && h.closed, "the client was started and ended");
}

#[test]
fn a_copy_that_never_arrives_fails_its_step_and_ends_the_case() {
    let mut h = Linked {
        faults: Faults {
            no_windows_to_remote: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = text_run(&mut h);
    assert!(!summary(&lines).ok, "the run fails");
    assert_eq!(lines.len(), 2, "the failing step and the summary only");
    assert_eq!(
        field(&lines[0], "step"),
        Some(&"ascii-windows-to-remote".into()),
        "the first step is named"
    );
    assert_eq!(field(&lines[0], "got"), Some(&serde_json::Value::Null));
    assert!(h.closed, "the client is ended after a failure too");
}

#[test]
fn crlf_that_is_kept_is_named() {
    let mut h = Linked {
        faults: Faults {
            keeps_crlf: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = text_run(&mut h);
    let bad = lines.iter().find(|l| !l.ok).expect("a failed step");
    assert_eq!(field(bad, "step"), Some(&"crlf-windows-to-remote".into()));
    assert_eq!(
        field(bad, "expected"),
        Some(&"n1 a\nb\nc".into()),
        "what LF would have been"
    );
}

#[test]
fn a_clipboard_that_changes_in_the_quiet_time_fails() {
    let mut h = Linked {
        faults: Faults {
            echo_after_quiet: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = text_run(&mut h);
    let bad = lines.iter().find(|l| !l.ok).expect("a failed step");
    assert_eq!(field(bad, "step"), Some(&"quiet".into()));
    assert!(!summary(&lines).ok);
}

#[test]
fn a_client_that_ends_during_startup_fails_without_a_copy() {
    let mut h = Linked {
        faults: Faults {
            client_exits: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = text_run(&mut h);
    assert!(!summary(&lines).ok);
    assert!(h.windows.is_none(), "nothing was copied");
    assert_eq!(
        field(&lines[0], "error"),
        Some(&"the client ended during startup".into())
    );
}

#[test]
fn cases_that_are_not_built_are_pending_and_fail() {
    let mut h = Linked::default();
    let lines = run(&mut h, &ClipCase::ALL, "n1", TIMING);
    let pending: Vec<_> = lines
        .iter()
        .filter(|l| field(l, "pending").is_some())
        .filter_map(|l| field(l, "case").and_then(|c| c.as_str()))
        .collect();
    assert_eq!(pending, ["html", "rtf"]);
    assert!(!summary(&lines).ok, "nothing reads green before it exists");
    assert_eq!(field(summary(&lines), "passed"), Some(&3.into()));
}

#[test]
fn only_the_cases_asked_for_are_run() {
    let mut h = Linked::default();
    let lines = run(&mut h, &[ClipCase::Html], "n1", TIMING);
    assert!(!h.launched, "no client for a case that needs none");
    assert_eq!(lines.len(), 2, "the pending case and the summary");
}

fn image_run(h: &mut Linked) -> Vec<Line> {
    run(h, &[ClipCase::Image], "n1", TIMING)
}

fn failed_step(lines: &[Line]) -> &str {
    let bad = lines.iter().find(|l| !l.ok).expect("a failed step");
    field(bad, "step").and_then(|s| s.as_str()).unwrap_or("")
}

#[test]
fn linked_clipboards_pass_every_image_step() {
    let mut h = Linked::default();
    let lines = image_run(&mut h);
    assert!(lines.iter().all(|l| l.ok), "{lines:?}");
    let steps: Vec<_> = lines
        .iter()
        .filter_map(|l| field(l, "step").and_then(|s| s.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            "snip-windows-to-remote",
            "png-alpha-windows-to-remote",
            "picture-remote-to-windows",
            "alpha-remote-to-windows",
            "remote-copy-stays-out-of-history",
            "4k-windows-to-remote",
            "quiet"
        ]
    );
    assert_eq!(
        h.forms,
        [WindowsForm::Dib, WindowsForm::PngAndDib, WindowsForm::Dib],
        "a snip as a bitmap alone, a PNG copy with its bitmap"
    );
    assert!(h.launched && h.closed);
}

fn files_run(h: &mut Linked) -> Vec<Line> {
    run(h, &[ClipCase::Files], "n1", TIMING)
}

#[test]
fn linked_clipboards_pass_every_files_step() {
    let mut h = Linked::default();
    let lines = files_run(&mut h);
    assert!(lines.iter().all(|l| l.ok), "{lines:?}");
    let steps: Vec<_> = lines
        .iter()
        .filter_map(|l| field(l, "step").and_then(|s| s.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            "tree-windows-to-remote",
            "tree-remote-to-windows",
            "refused-names-remote-to-windows",
            "drop-effect-and-history",
            "quiet",
            "spool-removed-at-exit"
        ]
    );
    assert_eq!(field(summary(&lines), "passed"), Some(&1.into()));
    assert!(h.closed && h.cleared, "the client ended and the trees went");
}

#[test]
fn files_that_never_arrive_fail_their_step_and_are_still_cleaned_up() {
    let mut h = Linked {
        faults: Faults {
            no_windows_to_remote: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = files_run(&mut h);
    assert_eq!(failed_step(&lines), "tree-windows-to-remote");
    assert_eq!(field(&lines[0], "got"), Some(&"no files".into()));
    assert!(!summary(&lines).ok);
    assert!(h.closed && h.cleared);
}

#[test]
fn a_file_with_a_changed_byte_is_named() {
    let mut h = Linked {
        faults: Faults {
            corrupts_files: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = files_run(&mut h);
    assert_eq!(failed_step(&lines), "tree-remote-to-windows");
    let got = field(&lines[1], "got")
        .and_then(|g| g.as_str())
        .unwrap_or("");
    assert!(
        got.contains("differs in content") && got.contains("sha256"),
        "{got}"
    );
}

#[test]
fn names_windows_cannot_open_must_have_been_changed() {
    let mut h = Linked {
        faults: Faults {
            keeps_refused_names: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = files_run(&mut h);
    assert_eq!(failed_step(&lines), "refused-names-remote-to-windows");
}

#[test]
fn a_missing_drop_effect_or_history_mark_fails() {
    for faults in [
        Faults {
            no_drop_effect: true,
            ..Faults::default()
        },
        Faults {
            history_open: true,
            ..Faults::default()
        },
    ] {
        let mut h = Linked {
            faults,
            ..Linked::default()
        };
        let lines = files_run(&mut h);
        assert_eq!(failed_step(&lines), "drop-effect-and-history");
    }
}

#[test]
fn files_that_change_in_the_quiet_time_fail() {
    let mut h = Linked {
        faults: Faults {
            files_echo_after_quiet: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = files_run(&mut h);
    assert_eq!(failed_step(&lines), "quiet");
}

#[test]
fn a_spool_left_after_exit_fails_the_case_and_is_named() {
    let mut h = Linked {
        faults: Faults {
            leaves_spool: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = files_run(&mut h);
    assert_eq!(failed_step(&lines), "spool-removed-at-exit");
    let bad = lines.iter().find(|l| !l.ok).expect("a failed step");
    assert_eq!(field(bad, "left"), Some(&"4242-1".into()));
    assert_eq!(field(summary(&lines), "passed"), Some(&0.into()));
}

#[test]
fn a_picture_that_never_arrives_fails_its_step() {
    let mut h = Linked {
        faults: Faults {
            no_windows_to_remote: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = image_run(&mut h);
    assert_eq!(failed_step(&lines), "snip-windows-to-remote");
    assert_eq!(lines.len(), 2, "the failing step and the summary only");
    assert_eq!(field(&lines[0], "got"), Some(&serde_json::Value::Null));
}

#[test]
fn an_upside_down_picture_is_named_by_its_first_wrong_pixel() {
    let mut h = Linked {
        faults: Faults {
            flips_pictures: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = image_run(&mut h);
    assert_eq!(failed_step(&lines), "snip-windows-to-remote");
    let got = field(&lines[0], "got")
        .and_then(|g| g.as_str())
        .expect("a note");
    assert!(got.starts_with("pixel 0,0 "), "{got}");
}

#[test]
fn lost_alpha_fails_the_alpha_step_only() {
    let mut h = Linked {
        faults: Faults {
            drops_alpha: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = image_run(&mut h);
    assert_eq!(failed_step(&lines), "alpha-remote-to-windows");
}

#[test]
fn a_remote_copy_in_the_history_fails() {
    let mut h = Linked {
        faults: Faults {
            history_open: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = image_run(&mut h);
    assert_eq!(failed_step(&lines), "remote-copy-stays-out-of-history");
    assert!(
        lines
            .iter()
            .all(|l| field(l, "step") != Some(&"4k-windows-to-remote".into())),
        "the case ended there"
    );
}

#[test]
fn a_picture_that_changes_in_the_quiet_time_fails() {
    let mut h = Linked {
        faults: Faults {
            picture_echo_after_quiet: true,
            ..Faults::default()
        },
        ..Linked::default()
    };
    let lines = image_run(&mut h);
    assert_eq!(failed_step(&lines), "quiet");
    let bad = lines.iter().find(|l| !l.ok).expect("a failed step");
    assert!(field(bad, "remote").is_some());
}

#[test]
fn the_nonce_makes_every_picture_unique() {
    let a = super::picture_steps("one");
    let b = super::picture_steps("two");
    for (x, y) in a.iter().zip(&b) {
        assert_ne!(x.picture, y.picture, "{}", x.name);
    }
}

#[test]
fn the_nonce_makes_every_text_unique() {
    let a = super::text_steps("one");
    let b = super::text_steps("two");
    for (x, y) in a.iter().zip(&b) {
        assert_ne!(x.send, y.send, "{}", x.name);
    }
}

// The remote half, over scripts/fake-ssh and the fake wl-clipboard.
#[cfg(unix)]
mod over_ssh {
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use crate::clip_check::picture::Picture;
    use crate::clip_check::remote::Remote;
    use crate::clip_check::tree::Spec;
    use crate::clip_check::tree::difference;

    fn fake_ssh() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh")
    }

    /// The fake clipboard is one file per user: the tests that use it take turns.
    static FAKE_CLIPBOARD: Mutex<()> = Mutex::new(());

    fn fake_file() -> PathBuf {
        let uid = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m));
        std::env::temp_dir().join(format!("gwin-fake-clipboard-{}", uid.expect("own uid")))
    }

    fn remote() -> Remote {
        Remote {
            ssh: fake_ssh(),
            target: "user@host".to_owned(),
            scratch: crate::clip_check::remote::DEFAULT_SCRATCH.to_owned(),
        }
    }

    /// A remote with its own scratch directory, so no test touches the home
    /// directory it runs in.
    fn scratch_remote(name: &str) -> (Remote, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("gwin-verify-test-{name}-{}", std::process::id()));
        let remote = Remote {
            scratch: format!("'{}'", dir.display()),
            ..remote()
        };
        (remote, dir)
    }

    fn tree() -> Spec {
        use crate::clip_check::tree::Node;
        let file = |path: &str, len, seed| Node::File {
            path: path.to_owned(),
            len,
            seed,
        };
        Spec {
            nodes: vec![
                Node::Dir("\u{65e5}\u{672c} \u{fc}n\u{ef} \u{1f389}".to_owned()),
                file(
                    "\u{65e5}\u{672c} \u{fc}n\u{ef} \u{1f389}/donn\u{e9}es.txt",
                    1234,
                    1,
                ),
                Node::Dir("\u{65e5}\u{672c} \u{fc}n\u{ef} \u{1f389}/deep".to_owned()),
                file(
                    "\u{65e5}\u{672c} \u{fc}n\u{ef} \u{1f389}/deep/empty.bin",
                    0,
                    2,
                ),
                file(
                    "\u{65e5}\u{672c} \u{fc}n\u{ef} \u{1f389}/deep/big.bin",
                    6 << 20,
                    3,
                ),
                Node::Dir("\u{65e5}\u{672c} \u{fc}n\u{ef} \u{1f389}/vide".to_owned()),
                file("-dash 'q' \"d\" $HOME `t`.txt", 77, 4),
            ],
        }
    }

    #[test]
    fn a_tree_copied_over_ssh_reads_back_by_path_size_and_hash() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (remote, dir) = scratch_remote("tree");
        let spec = tree();
        remote.set_files(&spec).expect("copy");
        let got = remote.files().expect("paste").expect("files");
        assert_eq!(difference(&spec.found(), &got, true), None);
        assert!(
            got.iter()
                .any(|f| f.file.is_some_and(|(len, _)| len == 6 << 20))
        );
        // What was copied is a uri-list of the top-level entries, which a
        // reader sees as text.
        let list = remote.text().expect("text").expect("a list");
        assert_eq!(list.lines().count(), 2, "{list}");
        assert!(list.starts_with("file://"), "{list}");
        remote.clear_files().expect("clear");
        assert!(!dir.exists(), "the scratch directory is gone");
        assert_eq!(remote.files().expect("paste"), None, "its files are gone");
    }

    #[test]
    fn a_replaced_tree_leaves_nothing_of_the_old_one() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (remote, _dir) = scratch_remote("replace");
        remote.set_files(&tree()).expect("copy");
        let small = Spec {
            nodes: vec![crate::clip_check::tree::Node::File {
                path: "only.txt".to_owned(),
                len: 5,
                seed: 9,
            }],
        };
        remote.set_files(&small).expect("copy");
        let got = remote.files().expect("paste").expect("files");
        assert_eq!(difference(&small.found(), &got, true), None);
        remote.clear_files().expect("clear");
    }

    #[test]
    fn a_remote_clipboard_with_text_only_has_no_files() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (remote, _dir) = scratch_remote("text");
        remote.set_text("just text").expect("copy");
        assert_eq!(remote.files().expect("paste"), None);
    }

    #[test]
    fn text_copied_over_ssh_reads_back_exactly() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remote = remote();
        let text = "caf\u{e9} \u{1f389}\nline two \"quoted\" 'single' $HOME `tick`";
        remote.set_text(text).expect("copy");
        assert_eq!(remote.text().expect("paste"), Some(text.to_owned()));
    }

    #[test]
    fn an_empty_remote_clipboard_reads_as_none() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = std::fs::remove_file(fake_file());
        assert_eq!(remote().text().expect("paste"), None);
        assert_eq!(remote().image().expect("paste"), None);
    }

    /// Noise does not compress: the PNG is far larger than a pipe holds.
    fn noisy_png() -> Vec<u8> {
        let mut picture = Picture::opaque(600, 400, 5);
        let mut state = 12345u32;
        for byte in &mut picture.rgba {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = state.to_le_bytes()[3];
        }
        picture.to_png().expect("encode")
    }

    #[test]
    fn a_picture_copied_over_ssh_reads_back_byte_for_byte() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remote = remote();
        let png = noisy_png();
        assert!(png.len() > 300_000, "{}", png.len());
        remote.set_image(&png).expect("copy");
        assert_eq!(remote.image().expect("paste"), Some(png));
    }

    #[test]
    fn a_remote_clipboard_holds_one_type_at_a_time() {
        let _turn = FAKE_CLIPBOARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remote = remote();
        remote.set_text("words").expect("copy");
        assert_eq!(remote.image().expect("paste"), None, "text is no picture");
        remote.set_image(&noisy_png()).expect("copy");
        assert_eq!(remote.text().expect("paste"), None, "a picture is no text");
    }

    #[test]
    fn a_remote_that_cannot_run_ssh_is_an_error() {
        let remote = Remote {
            ssh: PathBuf::from("/nonexistent/ssh"),
            target: "user@host".to_owned(),
            scratch: crate::clip_check::remote::DEFAULT_SCRATCH.to_owned(),
        };
        assert!(remote.set_text("x").is_err(), "no ssh, no copy");
    }

    #[test]
    fn the_remote_commands_are_one_quoted_sh_script() {
        for command in [
            crate::clip_check::remote::COPY_COMMAND,
            crate::clip_check::remote::PASTE_COMMAND,
            crate::clip_check::remote::COPY_IMAGE_COMMAND,
            crate::clip_check::remote::PASTE_IMAGE_COMMAND,
            crate::clip_check::remote::COPY_URIS_COMMAND,
            crate::clip_check::remote::PASTE_URIS_COMMAND,
        ] {
            assert!(
                command.starts_with("sh -c '") && command.ends_with('\''),
                "{command}"
            );
            assert_eq!(
                command.matches('\'').count(),
                2,
                "no quote inside the script: {command}"
            );
        }
    }
}
