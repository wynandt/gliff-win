//! `gwin-verify clipboard` (docs/clipboard.md, "Test matrix"): start the real
//! client, copy on one machine, and read the clipboard of the other.
//!
//! The cases are pure: they drive a [`Harness`] and judge what it reads back.
//! Only `win32` touches the real Windows clipboard and window, and only on
//! Windows; `remote` runs `wl-copy` and `wl-paste` over ssh on the remote
//! machine and is proven against `scripts/fake-ssh` and the fake wl-clipboard.

// Only Windows drives a real machine; elsewhere these run in tests.
pub mod picture;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod remote;
pub mod sha256;
pub mod tree;

#[cfg(windows)]
pub mod win32;

#[cfg(test)]
mod tests;

mod files;

use std::time::Duration;

use self::picture::Picture;
use self::tree::Found;
use self::tree::Spec;
use crate::line::Line;

/// A group of cases (`--only`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ClipCase {
    /// Plain text both ways: ASCII, Unicode, line ends, and a quiet end.
    Text,
    /// HTML.
    Html,
    /// Rich text.
    Rtf,
    /// Pictures.
    Image,
    /// Files.
    Files,
}

impl ClipCase {
    pub const ALL: [Self; 5] = [Self::Text, Self::Html, Self::Rtf, Self::Image, Self::Files];

    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Html => "html",
            Self::Rtf => "rtf",
            Self::Image => "image",
            Self::Files => "files",
        }
    }
}

/// A short text that differs from run to run (the clock and the process).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn nonce() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{:x}-{:x}", millis % 0xff_ffff_ffff, std::process::id())
}

/// How long the cases wait; the tests shrink it.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// After the client starts, before the first copy.
    pub startup: Duration,
    /// How long a copy gets to arrive.
    pub arrive: Duration,
    /// How long a copy of files gets: they cross the link in full first.
    pub arrive_files: Duration,
    /// Between two reads of the other side.
    pub poll: Duration,
    /// The quiet time at the end of `text`.
    pub quiet: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            startup: Duration::from_secs(12),
            arrive: Duration::from_secs(15),
            arrive_files: Duration::from_secs(60),
            poll: Duration::from_millis(250),
            quiet: Duration::from_secs(3),
        }
    }
}

/// The two machines and the client between them.
pub trait Harness {
    /// Starts the client.
    fn launch(&mut self) -> anyhow::Result<()>;
    /// The client has not ended.
    fn client_running(&mut self) -> bool;
    /// Copies `text` on the Windows machine.
    fn windows_set_text(&mut self, text: &str) -> anyhow::Result<()>;
    /// The text on the Windows clipboard, or `None` when it holds none.
    fn windows_text(&mut self) -> anyhow::Result<Option<String>>;
    /// Copies `text` on the remote machine.
    fn remote_set_text(&mut self, text: &str) -> anyhow::Result<()>;
    /// The text on the remote clipboard, or `None` when it holds none.
    fn remote_text(&mut self) -> anyhow::Result<Option<String>>;
    /// Copies `picture` on the Windows machine, as `form` says.
    fn windows_set_picture(&mut self, picture: &Picture, form: WindowsForm) -> anyhow::Result<()>;
    /// The picture on the Windows clipboard, or `None` when it holds none.
    fn windows_picture(&mut self) -> anyhow::Result<Option<Picture>>;
    /// The Windows clipboard asks to stay out of the history and cloud sync.
    fn windows_history_excluded(&mut self) -> anyhow::Result<bool>;
    /// Copies `picture` on the remote machine as `image/png`.
    fn remote_set_picture(&mut self, picture: &Picture) -> anyhow::Result<()>;
    /// The `image/png` on the remote clipboard, or `None` when it has none.
    fn remote_picture(&mut self) -> anyhow::Result<Option<Picture>>;
    /// Copies the tree `spec` on the Windows machine as files, as Explorer
    /// does: `CF_HDROP` of its top-level entries, with the copy drop effect.
    fn windows_set_files(&mut self, spec: &Spec) -> anyhow::Result<()>;
    /// The files on the Windows clipboard, walked: `None` when it holds none.
    fn windows_files(&mut self) -> anyhow::Result<Option<Vec<Found>>>;
    /// The `Preferred DropEffect` on the Windows clipboard, if it has one.
    fn windows_drop_effect(&mut self) -> anyhow::Result<Option<u32>>;
    /// Copies the tree `spec` on the remote machine as files.
    fn remote_set_files(&mut self, spec: &Spec) -> anyhow::Result<()>;
    /// The files the remote clipboard holds, as a paste reads them.
    fn remote_files(&mut self) -> anyhow::Result<Option<Vec<Found>>>;
    /// The spool directories the client's process has left on Windows.
    fn windows_spool_left(&mut self) -> anyhow::Result<Vec<String>>;
    /// Removes the trees the gate made on both machines.
    fn clear_files(&mut self) -> anyhow::Result<()>;
    /// Ends the client.
    fn close(&mut self);
    fn sleep(&mut self, d: Duration);
}

/// How a picture is put on the Windows clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowsForm {
    /// `CF_DIB` alone, as the screen snip tools leave a copy; Windows adds
    /// the other bitmap formats.
    Dib,
    /// The registered "PNG" format and `CF_DIB`, as most editors leave it.
    PngAndDib,
}

/// Which way a copy goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    WindowsToRemote,
    RemoteToWindows,
}

/// One copy and what the other side must then hold.
struct Step {
    name: &'static str,
    way: Way,
    send: String,
    expect: String,
}

/// The `text` steps. `nonce` makes this run's texts differ from any text
/// an earlier run left on a clipboard.
fn text_steps(nonce: &str) -> Vec<Step> {
    let unicode = format!(
        "{nonce} na\u{ef}ve caf\u{e9} \u{2014} \u{65e5}\u{672c}\u{8a9e} \u{1f389} \u{5e2}\u{5d1}\u{5e8}\u{5d9}\u{5ea} e\u{301}"
    );
    let same = |name, way, text: String| Step {
        name,
        way,
        expect: text.clone(),
        send: text,
    };
    vec![
        same(
            "ascii-windows-to-remote",
            Way::WindowsToRemote,
            format!("gwin clip {nonce} one"),
        ),
        same(
            "ascii-remote-to-windows",
            Way::RemoteToWindows,
            format!("gwin clip {nonce} two"),
        ),
        same(
            "unicode-windows-to-remote",
            Way::WindowsToRemote,
            unicode.clone(),
        ),
        same("unicode-remote-to-windows", Way::RemoteToWindows, unicode),
        // CRLF becomes LF on the way to the remote machine.
        Step {
            name: "crlf-windows-to-remote",
            way: Way::WindowsToRemote,
            send: format!("{nonce} a\r\nb\r\nc"),
            expect: format!("{nonce} a\nb\nc"),
        },
        // LF stays LF on the way to Windows.
        same(
            "lf-remote-to-windows",
            Way::RemoteToWindows,
            format!("{nonce} a\nb\nc"),
        ),
    ]
}

/// What a step found.
#[derive(Debug)]
enum Outcome {
    Arrived { polls: u32 },
    Wrong { got: Option<String> },
    Error(String),
}

/// Reads with `read` until `wanted` accepts what it returns or
/// `timing.arrive` is over. Returns the number of reads when it arrived, or
/// the last thing read.
fn poll<H: Harness, T>(
    h: &mut H,
    timing: Timing,
    mut read: impl FnMut(&mut H) -> anyhow::Result<Option<T>>,
    wanted: impl Fn(&T) -> bool,
) -> Result<u32, Result<Option<T>, String>> {
    let mut waited = Duration::ZERO;
    let mut polls = 0;
    loop {
        polls += 1;
        let last = read(h).map_err(|e| format!("{e:#}"));
        if matches!(&last, Ok(Some(value)) if wanted(value)) {
            return Ok(polls);
        }
        if waited >= timing.arrive {
            return Err(last);
        }
        h.sleep(timing.poll);
        waited += timing.poll;
    }
}

/// Copies on one side and polls the other until it holds the expected text
/// or `timing.arrive` is over.
fn run_step<H: Harness>(h: &mut H, step: &Step, timing: Timing) -> Outcome {
    let set = match step.way {
        Way::WindowsToRemote => h.windows_set_text(&step.send),
        Way::RemoteToWindows => h.remote_set_text(&step.send),
    };
    if let Err(e) = set {
        return Outcome::Error(format!("the copy failed: {e:#}"));
    }
    let read = |h: &mut H| match step.way {
        Way::WindowsToRemote => h.remote_text(),
        Way::RemoteToWindows => h.windows_text(),
    };
    match poll(h, timing, read, |text| *text == step.expect) {
        Ok(polls) => Outcome::Arrived { polls },
        Err(Ok(got)) => Outcome::Wrong { got },
        Err(Err(e)) => Outcome::Error(format!("the read failed: {e}")),
    }
}

fn step_line(case: ClipCase, step: &Step, outcome: &Outcome, timing: Timing) -> Line {
    let line = Line::new("clipboard", matches!(outcome, Outcome::Arrived { .. }))
        .with("case", case.name())
        .with("step", step.name);
    match outcome {
        Outcome::Arrived { polls } => line.with(
            "ms",
            u64::try_from(timing.poll.as_millis()).unwrap_or(0)
                * u64::from(polls.saturating_sub(1)),
        ),
        Outcome::Wrong { got } => line
            .with("expected", step.expect.clone())
            .with("got", got.clone()),
        Outcome::Error(e) => line.with("error", e.clone()),
    }
}

/// The `text` case. Returns the lines and whether all steps passed. A step
/// that fails ends the case: the later steps would only report its damage.
fn run_text<H: Harness>(h: &mut H, nonce: &str, timing: Timing) -> (Vec<Line>, bool) {
    let mut lines = Vec::new();
    let mut last_expect = None;
    for step in text_steps(nonce) {
        let outcome = run_step(h, &step, timing);
        let ok = matches!(outcome, Outcome::Arrived { .. });
        lines.push(step_line(ClipCase::Text, &step, &outcome, timing));
        if !ok {
            return (lines, false);
        }
        last_expect = Some(step.expect);
    }
    // Both sides still hold the last copy after a quiet time: a copy that
    // came back as an echo, or a revoked offer, would have changed one.
    h.sleep(timing.quiet);
    let last = last_expect.unwrap_or_default();
    let windows = h.windows_text().map_err(|e| format!("{e:#}"));
    let remote = h.remote_text().map_err(|e| format!("{e:#}"));
    let held = |side: &Result<Option<String>, String>| matches!(side, Ok(Some(t)) if *t == last);
    let ok = held(&windows) && held(&remote);
    let mut line = Line::new("clipboard", ok)
        .with("case", ClipCase::Text.name())
        .with("step", "quiet");
    if !ok {
        line = line
            .with("expected", last.clone())
            .with("windows", format!("{windows:?}"))
            .with("remote", format!("{remote:?}"));
    }
    lines.push(line);
    (lines, ok)
}

/// One picture copied, and what the other side must then hold.
struct PictureStep {
    name: &'static str,
    way: Way,
    /// How Windows holds the picture; only a copy on Windows has one.
    form: WindowsForm,
    picture: Picture,
}

/// A number from the nonce, so this run's pictures differ from any picture
/// an earlier run left on a clipboard.
fn seed(nonce: &str) -> u32 {
    nonce
        .bytes()
        .fold(17u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)))
}

/// The `image` steps (the history check and the quiet end come after them).
/// A snip is a screen-sized bitmap on Windows; the large one is over the
/// item limit as a bitmap and fits only as PNG.
fn picture_steps(nonce: &str) -> Vec<PictureStep> {
    let seed = seed(nonce);
    let step = |name, way, form, picture| PictureStep {
        name,
        way,
        form,
        picture,
    };
    vec![
        step(
            "snip-windows-to-remote",
            Way::WindowsToRemote,
            WindowsForm::Dib,
            Picture::opaque(1920, 1080, seed),
        ),
        step(
            "png-alpha-windows-to-remote",
            Way::WindowsToRemote,
            WindowsForm::PngAndDib,
            Picture::translucent(64, 48, seed),
        ),
        step(
            "picture-remote-to-windows",
            Way::RemoteToWindows,
            WindowsForm::Dib,
            Picture::opaque(1920, 1080, seed.wrapping_add(1)),
        ),
        step(
            "alpha-remote-to-windows",
            Way::RemoteToWindows,
            WindowsForm::Dib,
            Picture::translucent(64, 48, seed.wrapping_add(1)),
        ),
    ]
}

/// The large picture, last: it is the copy both sides must still hold at
/// the quiet end.
fn large_picture(nonce: &str) -> PictureStep {
    PictureStep {
        name: "4k-windows-to-remote",
        way: Way::WindowsToRemote,
        form: WindowsForm::Dib,
        picture: Picture::opaque(3840, 2160, seed(nonce).wrapping_add(2)),
    }
}

fn picture_line(step: &PictureStep, outcome: &Outcome, timing: Timing) -> Line {
    let line = Line::new("clipboard", matches!(outcome, Outcome::Arrived { .. }))
        .with("case", ClipCase::Image.name())
        .with("step", step.name);
    match outcome {
        Outcome::Arrived { polls } => line.with(
            "ms",
            u64::try_from(timing.poll.as_millis()).unwrap_or(0)
                * u64::from(polls.saturating_sub(1)),
        ),
        Outcome::Wrong { got } => line.with("got", got.clone()),
        Outcome::Error(e) => line.with("error", e.clone()),
    }
}

/// Copies a picture on one side and polls the other until it holds the same
/// picture, pixel for pixel.
fn run_picture_step<H: Harness>(h: &mut H, step: &PictureStep, timing: Timing) -> Outcome {
    let set = match step.way {
        Way::WindowsToRemote => h.windows_set_picture(&step.picture, step.form),
        Way::RemoteToWindows => h.remote_set_picture(&step.picture),
    };
    if let Err(e) = set {
        return Outcome::Error(format!("the copy failed: {e:#}"));
    }
    let read = |h: &mut H| match step.way {
        Way::WindowsToRemote => h.remote_picture(),
        Way::RemoteToWindows => h.windows_picture(),
    };
    match poll(h, timing, read, |got| {
        step.picture.difference(got).is_none()
    }) {
        Ok(polls) => Outcome::Arrived { polls },
        Err(Ok(Some(got))) => Outcome::Wrong {
            got: step.picture.difference(&got),
        },
        Err(Ok(None)) => Outcome::Wrong { got: None },
        Err(Err(e)) => Outcome::Error(format!("the read failed: {e}")),
    }
}

/// The `image` case: pictures both ways, a copy from the remote machine
/// kept out of the Windows history, and a quiet end. A failed step ends the
/// case.
fn run_image<H: Harness>(h: &mut H, nonce: &str, timing: Timing) -> (Vec<Line>, bool) {
    let mut lines = Vec::new();
    let steps = picture_steps(nonce);
    let large = large_picture(nonce);
    // The history check reads the clipboard the last copy from the remote
    // machine left, so it runs before the large copy from Windows.
    for step in &steps {
        let outcome = run_picture_step(h, step, timing);
        let ok = matches!(outcome, Outcome::Arrived { .. });
        lines.push(picture_line(step, &outcome, timing));
        if !ok {
            return (lines, false);
        }
    }
    let excluded = h.windows_history_excluded().map_err(|e| format!("{e:#}"));
    let ok = matches!(excluded, Ok(true));
    let mut line = Line::new("clipboard", ok)
        .with("case", ClipCase::Image.name())
        .with("step", "remote-copy-stays-out-of-history");
    if !ok {
        line = line.with("got", format!("{excluded:?}"));
    }
    lines.push(line);
    if !ok {
        return (lines, false);
    }
    let outcome = run_picture_step(h, &large, timing);
    let ok = matches!(outcome, Outcome::Arrived { .. });
    lines.push(picture_line(&large, &outcome, timing));
    if !ok {
        return (lines, false);
    }
    let last = &large.picture;
    h.sleep(timing.quiet);
    let windows = h.windows_picture().map_err(|e| format!("{e:#}"));
    let remote = h.remote_picture().map_err(|e| format!("{e:#}"));
    let held = |side: &Result<Option<Picture>, String>| matches!(side, Ok(Some(p)) if last.difference(p).is_none());
    let ok = held(&windows) && held(&remote);
    let mut line = Line::new("clipboard", ok)
        .with("case", ClipCase::Image.name())
        .with("step", "quiet");
    if !ok {
        let say = |side: &Result<Option<Picture>, String>| match side {
            Ok(Some(p)) => last
                .difference(p)
                .unwrap_or_else(|| "the picture".to_owned()),
            Ok(None) => "no picture".to_owned(),
            Err(e) => e.clone(),
        };
        line = line
            .with("windows", say(&windows))
            .with("remote", say(&remote));
    }
    lines.push(line);
    (lines, ok)
}

/// Runs the cases in order. A case without its steps yet reports `pending`
/// and fails, so nothing reads green before it exists.
pub fn run<H: Harness>(h: &mut H, cases: &[ClipCase], nonce: &str, timing: Timing) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    let mut passed = 0usize;
    let runnable = [ClipCase::Text, ClipCase::Image, ClipCase::Files]
        .iter()
        .any(|case| cases.contains(case));
    let mut files_ok = None;
    let mut started = false;
    if runnable {
        match h.launch() {
            Ok(()) => {
                h.sleep(timing.startup);
                started = h.client_running();
                if !started {
                    lines.push(
                        Line::new("clipboard", false)
                            .with("case", "client")
                            .with("error", "the client ended during startup"),
                    );
                }
            }
            Err(e) => lines.push(
                Line::new("clipboard", false)
                    .with("case", "client")
                    .with("error", format!("the client did not start: {e:#}")),
            ),
        }
    }
    for case in cases {
        match case {
            ClipCase::Text if started => {
                let (case_lines, ok) = run_text(h, nonce, timing);
                lines.extend(case_lines);
                if ok {
                    passed += 1;
                } else {
                    failed.push(case.name());
                }
            }
            ClipCase::Image if started => {
                let (case_lines, ok) = run_image(h, nonce, timing);
                lines.extend(case_lines);
                if ok {
                    passed += 1;
                } else {
                    failed.push(case.name());
                }
            }
            ClipCase::Files if started => {
                // Its verdict waits for the spool check after the client ends.
                let (case_lines, ok) = files::run_files(h, nonce, timing);
                lines.extend(case_lines);
                files_ok = Some(ok);
            }
            ClipCase::Text | ClipCase::Image | ClipCase::Files => failed.push(case.name()),
            other => {
                lines.push(
                    Line::new("clipboard", false)
                        .with("case", other.name())
                        .with("pending", "not built yet"),
                );
                failed.push(other.name());
            }
        }
    }
    if runnable {
        h.close();
    }
    if let Some(ok) = files_ok {
        let (case_lines, ok) = files::after_exit(h, ok);
        lines.extend(case_lines);
        if ok {
            passed += 1;
        } else {
            failed.push(ClipCase::Files.name());
        }
    }
    lines.push(
        Line::new("clipboard-summary", failed.is_empty() && passed > 0)
            .with("cases", cases.len())
            .with("passed", passed)
            .with("failed", failed),
    );
    lines
}
