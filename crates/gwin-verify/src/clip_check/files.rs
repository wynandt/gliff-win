//! The `files` case: folders and files copied on one machine and pasted on
//! the other, both ways.
//!
//! The gate does not use the client's code to judge the client. The trees
//! are made, hashed, listed and compared by `tree` and `sha256`, which share
//! nothing with it, and the remote half is `tar` and `wl-copy` on that
//! machine.

use super::ClipCase;
use super::Harness;
use super::Outcome;
use super::Timing;
use super::Way;
use super::poll;
use super::seed;
use super::tree::Node;
use super::tree::Spec;
use super::tree::difference;
use crate::line::Line;

/// One copy and what must arrive.
struct FilesStep {
    name: &'static str,
    way: Way,
    spec: Spec,
    /// The same paths, not only the same contents: no name had to change.
    strict: bool,
}

fn file(path: String, len: usize, seed: u32) -> Node {
    Node::File { path, len, seed }
}

fn dir(path: String) -> Node {
    Node::Dir(path)
}

/// A folder with a unicode name, two levels down, an empty file, a folder
/// with nothing in it, a file of a few MiB and a loose file beside it. Both
/// the name and the contents are this run's own.
fn nested(nonce: &str, seed: u32) -> Spec {
    let root = format!("{nonce} \u{65e5}\u{672c}\u{8a9e} \u{fc}n\u{ef} \u{1f389}");
    let at = |rest: &str| format!("{root}/{rest}");
    Spec {
        nodes: vec![
            dir(root.clone()),
            file(at("donn\u{e9}es.txt"), 1234, seed),
            dir(at("deep")),
            dir(at("deep/deeper")),
            file(at("deep/deeper/empty.bin"), 0, seed),
            file(at("deep/big.bin"), 6 << 20, seed.wrapping_add(1)),
            dir(at("vide")),
            file(format!("loose-{nonce}.txt"), 77, seed.wrapping_add(2)),
        ],
    }
}

/// Names Windows cannot hold, or holds as the same: a device name, reserved
/// characters, a trailing dot, names that differ only in case, in files and
/// in folders.
fn refused(seed: u32) -> Spec {
    let names = [
        "CON.txt",
        "a:b?.txt",
        "trailing.",
        "Readme.md",
        "readme.md",
        "ok.txt",
        "Docs/x.txt",
        "docs/y.txt",
    ];
    let mut nodes = vec![dir("Docs".to_owned()), dir("docs".to_owned())];
    for (n, name) in (0u32..).zip(names) {
        nodes.push(file(
            name.to_owned(),
            100 + 7 * n as usize,
            seed.wrapping_add(n),
        ));
    }
    Spec { nodes }
}

fn steps(nonce: &str) -> Vec<FilesStep> {
    let seed = seed(nonce);
    vec![
        FilesStep {
            name: "tree-windows-to-remote",
            way: Way::WindowsToRemote,
            spec: nested(nonce, seed),
            strict: true,
        },
        FilesStep {
            name: "tree-remote-to-windows",
            way: Way::RemoteToWindows,
            spec: nested(nonce, seed.wrapping_add(10)),
            strict: true,
        },
        FilesStep {
            name: "refused-names-remote-to-windows",
            way: Way::RemoteToWindows,
            spec: refused(seed.wrapping_add(20)),
            strict: false,
        },
    ]
}

fn run_step<H: Harness>(h: &mut H, step: &FilesStep, timing: Timing) -> Outcome {
    let set = match step.way {
        Way::WindowsToRemote => h.windows_set_files(&step.spec),
        Way::RemoteToWindows => h.remote_set_files(&step.spec),
    };
    if let Err(e) = set {
        return Outcome::Error(format!("the copy failed: {e:#}"));
    }
    let expected = step.spec.found();
    let timing = Timing {
        arrive: timing.arrive_files,
        ..timing
    };
    let read = |h: &mut H| match step.way {
        Way::WindowsToRemote => h.remote_files(),
        Way::RemoteToWindows => h.windows_files(),
    };
    match poll(h, timing, read, |got| {
        difference(&expected, got, step.strict).is_none()
    }) {
        Ok(polls) => Outcome::Arrived { polls },
        Err(Ok(got)) => Outcome::Wrong {
            got: Some(match got {
                Some(got) => difference(&expected, &got, step.strict)
                    .unwrap_or_else(|| "the same tree".to_owned()),
                None => "no files".to_owned(),
            }),
        },
        Err(Err(e)) => Outcome::Error(format!("the read failed: {e}")),
    }
}

fn line(step: &str, ok: bool) -> Line {
    Line::new("clipboard", ok)
        .with("case", ClipCase::Files.name())
        .with("step", step.to_owned())
}

/// The steps, then the drop effect and the history, then a quiet end. A
/// failed step ends the case. Returns the lines and whether all passed; the
/// spool check after the client ends is `after_exit`.
pub(super) fn run_files<H: Harness>(h: &mut H, nonce: &str, timing: Timing) -> (Vec<Line>, bool) {
    let mut lines = Vec::new();
    let steps = steps(nonce);
    let mut ok = true;
    for step in &steps {
        let outcome = run_step(h, step, timing);
        let arrived = matches!(outcome, Outcome::Arrived { .. });
        let mut out = line(step.name, arrived);
        match &outcome {
            Outcome::Arrived { polls } => {
                out = out.with(
                    "ms",
                    u64::try_from(timing.poll.as_millis()).unwrap_or(0)
                        * u64::from(polls.saturating_sub(1)),
                );
            }
            Outcome::Wrong { got } => out = out.with("got", got.clone()),
            Outcome::Error(e) => out = out.with("error", e.clone()),
        }
        lines.push(out);
        if !arrived {
            ok = false;
            break;
        }
    }
    if !ok {
        return (lines, false);
    }
    // The clipboard now holds the last copy from the remote machine.
    let effect = h.windows_drop_effect().map_err(|e| format!("{e:#}"));
    let excluded = h.windows_history_excluded().map_err(|e| format!("{e:#}"));
    let good = matches!(effect, Ok(Some(1))) && matches!(excluded, Ok(true));
    let mut out = line("drop-effect-and-history", good);
    if !good {
        out = out
            .with("drop_effect", format!("{effect:?}"))
            .with("history_excluded", format!("{excluded:?}"));
    }
    lines.push(out);
    if !good {
        return (lines, false);
    }
    // Quiet: nothing is copied back, and both sides still hold the tree.
    let last = &steps[steps.len() - 1];
    let expected = last.spec.found();
    h.sleep(timing.quiet);
    let windows = h.windows_files().map_err(|e| format!("{e:#}"));
    let remote = h.remote_files().map_err(|e| format!("{e:#}"));
    let say = |side: &Result<Option<Vec<super::tree::Found>>, String>, strict| match side {
        Ok(Some(got)) => difference(&expected, got, strict),
        Ok(None) => Some("no files".to_owned()),
        Err(e) => Some(e.clone()),
    };
    // The remote side holds the paths it made; Windows may have renamed.
    let (windows, remote) = (say(&windows, last.strict), say(&remote, true));
    let good = windows.is_none() && remote.is_none();
    let mut out = line("quiet", good);
    if let Some(w) = windows {
        out = out.with("windows", w);
    }
    if let Some(r) = remote {
        out = out.with("remote", r);
    }
    lines.push(out);
    (lines, good)
}

/// After the client has ended: its spool on Windows is gone, and the gate's
/// own trees are removed on both machines. Returns the lines and whether
/// the case passed.
pub(super) fn after_exit<H: Harness>(h: &mut H, ok: bool) -> (Vec<Line>, bool) {
    let mut lines = Vec::new();
    let left = h.windows_spool_left().map_err(|e| format!("{e:#}"));
    let gone = matches!(&left, Ok(left) if left.is_empty());
    let mut out = line("spool-removed-at-exit", gone);
    match &left {
        Ok(left) if !left.is_empty() => out = out.with("left", left.join(", ")),
        Err(e) => out = out.with("error", e.clone()),
        Ok(_) => {}
    }
    lines.push(out);
    let cleared = h.clear_files();
    if let Err(e) = &cleared {
        lines.push(line("cleanup", false).with("error", format!("{e:#}")));
    }
    (lines, ok && gone && cleared.is_ok())
}
