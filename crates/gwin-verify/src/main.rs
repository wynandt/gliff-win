//! `gwin-verify`: JSON-line checks for the gliff Windows client.
//!
//! Every check prints one JSON object per line on stdout,
//! `{"check": .., "ok": bool, ..metrics}`, and the process exits non-zero
//! when any line has `"ok": false`. Checks of features that are not
//! built yet print a `pending` line and fail, so nothing reads as green before it exists.

#[cfg_attr(not(windows), allow(dead_code))]
mod client_config;
mod clip;
#[cfg_attr(not(windows), allow(dead_code))]
mod clip_check;
mod config_check;
mod decode_check;
mod input_check;
mod line;
mod panel_shot;
mod pattern;
mod placement;
mod png_out;
mod present_check;
mod size;
mod ssh_check;
mod stream_check;
#[cfg_attr(not(windows), allow(dead_code))]
mod strip_offset;
mod strip_shot;
mod tabs_check;
mod target;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use clap::Subcommand;
use gliff_windows::cli::DEFAULT_SERVER_PATH;
use gliff_windows::cli::DesktopMode;
use gliff_windows::history::HostTarget;
use gliff_windows::ssh::SshSpec;

use crate::clip::Clip;
use crate::decode_check::KILL_RULE;
use crate::decode_check::Limits;
use crate::line::Line;
use crate::size::Size;
use crate::stream_check::PlacementProbe;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    check: Check,
}

#[derive(Debug, Subcommand)]
enum Check {
    /// CPU decode p50/p95 and fps per size (Single420).
    Decode {
        /// `.gwclip` files (repeatable), e.g. from `scripts/make-clips`;
        /// when given, no synthetic clips are encoded.
        #[arg(long)]
        clip: Vec<PathBuf>,
        /// Sizes of the in-process OpenH264 clips (at most 3840x2160; the
        /// 3840x2400 panel needs a clip from `scripts/make-clips`).
        #[arg(long, value_delimiter = ',', default_value = "1920x1080,2560x1440")]
        sizes: Vec<Size>,
        /// Frames per synthetic clip (one IDR, then P frames).
        #[arg(long, default_value_t = 300)]
        frames: u32,
        /// Synthetic clip bitrate, bits/s.
        #[arg(long, default_value_t = 20_000_000)]
        bitrate: u32,
        #[arg(long, default_value_t = KILL_RULE.max_p95_ms)]
        max_p95_ms: f64,
        #[arg(long, default_value_t = KILL_RULE.min_fps)]
        min_fps: f64,
        /// Also fail unless a 3840x2400 clip was judged at the kill-rule
        /// limits (the real-machine gate run).
        #[arg(long)]
        kill_rule: bool,
    },
    /// Start the server over ssh as the client does (BatchMode, no
    /// console), handshake, Bye; prints the remote path it launched.
    Ssh {
        target: HostTarget,
        #[arg(long, default_value = DEFAULT_SERVER_PATH)]
        server_path: String,
        /// A remote output to mirror, or `auto` for the focused one, which is
        /// the client's default. Without it the check asks for a private screen.
        #[arg(long)]
        output: Option<String>,
        #[arg(long, default_value = default_ssh())]
        ssh: PathBuf,
        /// Where ssh's stderr log goes.
        #[arg(long, default_value = "gwin-verify-logs")]
        log_dir: PathBuf,
        #[arg(long, default_value_t = 20)]
        timeout_s: u64,
    },
    /// The client's config file and recent hosts through
    /// `gliff-windows --print-settings` on a scratch folder: flag over file
    /// over default, a damaged history line dropped, a colliding chord
    /// refused, nothing written. Starts no session.
    Config {
        /// The client; default `gliff-windows` next to this exe.
        #[arg(long)]
        client: Option<PathBuf>,
        /// The scratch config folder is made inside this one.
        #[arg(long, default_value = "gwin-verify-logs")]
        log_dir: PathBuf,
    },
    /// Write a `.gwclip`: the synthetic desktop through OpenH264, or an
    /// Annex B file with access unit delimiters (`--annexb`).
    MakeClip {
        #[arg(long)]
        size: Size,
        #[arg(long)]
        annexb: Option<PathBuf>,
        #[arg(long, default_value_t = 300)]
        frames: u32,
        #[arg(long, default_value_t = 20_000_000)]
        bitrate: u32,
        #[arg(long)]
        out: PathBuf,
    },
    /// Raw I420 frames of the synthetic desktop on stdout, for an external
    /// encoder. The one subcommand whose result line goes to stderr.
    Render {
        #[arg(long)]
        size: Size,
        #[arg(long, default_value_t = 300)]
        frames: u32,
    },
    /// Headless session at the panel size: Resize applied, fps,
    /// decode p50/p95, acks == frames, no stall, the SPS level, and the
    /// last picture (saved as a PNG) compared with the test pattern.
    Stream {
        /// ssh target, e.g. `user@host`.
        #[arg(long, value_parser = target::parse_target)]
        host: HostTarget,
        /// The panel: sent as Resize once the session is up.
        #[arg(long, default_value = "3840x2400")]
        size: Size,
        /// Length of the measurement window.
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        /// The floor for the server's fps cap: a cap below it fails
        /// `fps_cap`.
        #[arg(long, default_value_t = 30.0)]
        min_fps: f64,
        /// The share of the server's fps cap (time-weighted over the
        /// window) the decoded fps must reach.
        #[arg(long, default_value_t = stream_check::measure::FPS_OF_CAP)]
        fps_of_cap: f64,
        /// A gap between frames longer than this is a stall.
        #[arg(long, default_value_t = 5)]
        stall_s: u64,
        /// Wait after the Resize took effect, before measuring.
        #[arg(long, default_value_t = 5)]
        settle_s: u64,
        /// How long the server gets to apply the Resize.
        #[arg(long, default_value_t = 10)]
        resize_timeout_s: u64,
        /// Where the last decoded picture is written.
        #[arg(long, default_value = "gwin-verify-logs/stream-last.png")]
        png: PathBuf,
        /// Worst-cell mean absolute error allowed (0-255 per channel).
        #[arg(long, default_value_t = pattern::DEFAULT_TOLERANCE)]
        pattern_tolerance: f64,
        /// Ask hyprctl (second ssh) where the pattern window is.
        #[arg(long, value_enum, default_value_t = PlacementProbe::Ssh)]
        placement: PlacementProbe,
        /// Take pictures from the frame slot only every N ms, so it drops
        /// (acks must still equal frames).
        #[arg(long)]
        present_ms: Option<u64>,
        #[arg(long, default_value = DEFAULT_SERVER_PATH)]
        server_path: String,
        #[arg(long, default_value = default_ssh())]
        ssh: PathBuf,
        #[arg(long, default_value = "gwin-verify-logs")]
        log_dir: PathBuf,
    },
    /// Run the real client (`gliff-windows.exe`), resize it to
    /// 2560x1600 then F11, and judge its stats file and a screenshot.
    /// Windows only.
    Present {
        /// ssh target, e.g. `user@host`.
        #[arg(long, value_parser = target::parse_target)]
        host: HostTarget,
        /// The client; default `gliff-windows.exe` next to this exe.
        #[arg(long)]
        client: Option<PathBuf>,
        /// Length of each measured stage (windowed, fullscreen).
        #[arg(long, default_value_t = 20)]
        seconds: u64,
        /// Where the fullscreen screenshot is written.
        #[arg(long, default_value = "gwin-verify-logs/present-3840x2400.png")]
        png: PathBuf,
        /// Worst-cell mean absolute error allowed (0-255 per channel).
        #[arg(long, default_value_t = pattern::DEFAULT_TOLERANCE)]
        pattern_tolerance: f64,
        /// Before each measured stage, ask hyprctl (second ssh) that the
        /// pattern window covers the session's output; fails
        /// `no_damage_source` if not.
        #[arg(long, value_enum, default_value_t = PlacementProbe::Ssh)]
        placement: PlacementProbe,
        #[arg(long, default_value = DEFAULT_SERVER_PATH)]
        server_path: String,
        #[arg(long, default_value = default_ssh())]
        ssh: PathBuf,
        /// The client's log, ssh's stderr and the stats file go here.
        #[arg(long, default_value = "gwin-verify-logs")]
        log_dir: PathBuf,
    },
    /// Start the client with three targets, switch between them with the
    /// tab chords (`SendInput`), and judge the stats file: the right session
    /// is shown, no key is left down in any session, hidden sessions keep
    /// acking, and the history file has the targets. It grabs the keyboard
    /// of the person at the machine: run it only at a machine you sit at,
    /// never unattended. Windows only.
    Tabs {
        /// ssh targets, in tab order (three or more; repeat the option).
        #[arg(long = "host", required = true, num_args = 1.., value_parser = target::parse_target)]
        hosts: Vec<HostTarget>,
        /// The client; default `gliff-windows.exe` next to this exe.
        #[arg(long)]
        client: Option<PathBuf>,
        /// Seconds the warning stays up before the first click.
        #[arg(long, default_value_t = 10)]
        countdown_s: u64,
        #[arg(long, default_value = DEFAULT_SERVER_PATH)]
        server_path: String,
        #[arg(long, default_value = default_ssh())]
        ssh: PathBuf,
        /// The client's log, ssh's stderr and the stats file go here.
        #[arg(long, default_value = "gwin-verify-logs")]
        log_dir: PathBuf,
    },
    /// Draw the tab strip's states (one tab, ten, connecting, failed, hover,
    /// dragging, narrow with the chevron, and more) at 100, 125, 150 and 200%
    /// into `strip-<scene>-<scale>pct.png` and judge each picture. Needs no
    /// input: no window, keyboard, mouse or clipboard is touched. On Windows
    /// it draws names too; elsewhere the shapes only.
    StripShot {
        /// The pictures go here.
        #[arg(long, default_value = "gwin-verify-logs/strip")]
        out: PathBuf,
        /// Only these scales in percent (repeat; default all four).
        #[arg(long = "scale")]
        scales: Vec<u32>,
        /// Only these scenes (repeat; default all).
        #[arg(long = "scene")]
        scenes: Vec<String>,
        /// Shapes only, no names, on Windows too.
        #[arg(long)]
        shapes_only: bool,
    },
    /// Draw the failure panel for every failure (thirteen) at 100, 125, 150
    /// and 200% into `panel-<failure>-<scale>pct.png` and judge each picture.
    /// Needs no input: no window, keyboard, mouse or clipboard is touched. On
    /// Windows it draws the words too; elsewhere the shapes only.
    PanelShot {
        /// The pictures go here.
        #[arg(long, default_value = "gwin-verify-logs/panel")]
        out: PathBuf,
        /// Only these scales in percent (repeat; default all four).
        #[arg(long = "scale")]
        scales: Vec<u32>,
        /// Shapes only, no words, on Windows too.
        #[arg(long)]
        shapes_only: bool,
    },
    /// Send the 20 Super chords, the system chords, layouts, capture
    /// and release to the remote machine and judge its `gwin-keys` log.
    /// Windows only.
    Keys {
        #[command(flatten)]
        input: InputArgs,
        /// Run only these groups (default: all but `lock`).
        #[arg(long, value_enum)]
        only: Vec<input_check::KeyGroup>,
        /// Run only the cases whose name contains this text (any case of
        /// letters; repeat for several). Narrows the groups chosen by
        /// `--only`; the `lock` steps are not cases and ignore it.
        #[arg(long = "case")]
        cases: Vec<String>,
        /// Also ask the chords only a person can press (Ctrl+Alt+Del, UAC),
        /// with a client running. `--only manual` asks them alone.
        #[arg(long)]
        manual: bool,
    },
    /// Click, wheel and buttons at the client's current scale,
    /// judged against the remote machine's log. Windows only.
    Pointer {
        #[command(flatten)]
        input: InputArgs,
        /// The display scale the Windows machine is at, in thousandths (2000 is
        /// 200%); the check fails if the client reports another.
        #[arg(long, default_value_t = 2000)]
        expect_scale_milli: u32,
    },
    /// Copy text both ways between the Windows clipboard and the
    /// remote machine's (wl-copy / wl-paste over ssh), with the real client
    /// running. Overwrites both clipboards. Windows only.
    #[command(alias = "clip")]
    Clipboard {
        /// ssh target of the remote machine, e.g. `user@host`.
        #[arg(long, value_parser = target::parse_target)]
        host: HostTarget,
        /// The client; default `gliff-windows.exe` next to this exe.
        #[arg(long)]
        client: Option<PathBuf>,
        /// Run only these cases (default: all; the ones not built yet fail
        /// as pending).
        #[arg(long, value_enum, value_delimiter = ',', default_values_t = clip_check::ClipCase::ALL)]
        only: Vec<clip_check::ClipCase>,
        #[arg(long, default_value = DEFAULT_SERVER_PATH)]
        server_path: String,
        #[arg(long, default_value = default_ssh())]
        ssh: PathBuf,
        /// The client's log and ssh's stderr go here.
        #[arg(long, default_value = "gwin-verify-logs")]
        log_dir: PathBuf,
    },
    /// D3D11 video decoder profiles (not built yet).
    Gpu,
}

/// What `keys` and `pointer` share.
#[derive(Debug, clap::Args)]
struct InputArgs {
    /// ssh target of the remote machine, e.g. `user@host`.
    #[arg(long, value_parser = target::parse_target)]
    host: HostTarget,
    /// The client; default `gliff-windows.exe` next to this exe.
    #[arg(long)]
    client: Option<PathBuf>,
    /// The remote machine's `gwin-keys` command, run as `<cmd> log` over ssh.
    #[arg(long, default_value = "gwin-keys")]
    remote_keys: String,
    #[arg(long, default_value = DEFAULT_SERVER_PATH)]
    server_path: String,
    #[arg(long, default_value = default_ssh())]
    ssh: PathBuf,
    /// The client's log, ssh's stderr and the stats file go here.
    #[arg(long, default_value = "gwin-verify-logs")]
    log_dir: PathBuf,
    /// In the lock step, wait for Enter at each press instead of a timed
    /// countdown on screen.
    #[arg(long)]
    prompt_enter: bool,
}

#[cfg(windows)]
fn input_harness(args: InputArgs) -> anyhow::Result<input_check::win32::Win32Harness> {
    let client = match args.client {
        Some(c) => c,
        None => std::env::current_exe()?.with_file_name("gliff-windows.exe"),
    };
    let log_dir = std::path::absolute(&args.log_dir)?;
    std::fs::create_dir_all(&log_dir)?;
    input_check::win32::Win32Harness::new(input_check::win32::Launch {
        client,
        host: args.host,
        server_path: args.server_path,
        ssh: args.ssh,
        stats_out: log_dir.join("input-stats.json"),
        log_dir,
        remote_keys: args.remote_keys,
        prompt_enter: args.prompt_enter,
    })
}

#[cfg(windows)]
fn keys(
    args: InputArgs,
    only: &[input_check::KeyGroup],
    cases: &[String],
    manual: bool,
    report: &mut Report,
) -> anyhow::Result<()> {
    use input_check::KeyGroup;
    let mut harness = input_harness(args)?;
    let mut results = Vec::new();
    let plan = input_check::keys_plan(only).map_err(anyhow::Error::msg)?;
    let plan = input_check::select_cases(plan, cases).map_err(anyhow::Error::msg)?;
    if !plan.is_empty() {
        results.extend(input_check::run::run_cases(&mut harness, &plan));
    }
    if only.contains(&KeyGroup::Lock) {
        results.extend(input_check::lock::run(&mut harness));
    }
    if manual || only.contains(&KeyGroup::Manual) {
        results.extend(input_check::manual::run(&mut harness, &mut |h, text| {
            eprintln!("\n(the client is in front: do the press, then click this window to answer)");
            input_check::lock::LockHarness::ask(h, text)
        }));
    }
    for r in &results {
        report.emit(r.line("keys"));
    }
    report.emit(input_check::run::summary("keys", &results));
    Ok(())
}

#[cfg(windows)]
fn pointer(args: InputArgs, expect_scale_milli: u32, report: &mut Report) -> anyhow::Result<()> {
    use input_check::run::Harness;
    let mut harness = input_harness(args)?;
    harness.start_client(&[])?;
    let size = harness.remote_size();
    let scale = harness
        .observe()
        .input
        .and_then(|i| i.geometry)
        .map(|g| g.scale_milli);
    harness.stop_client();
    let Some(size) = size else {
        report
            .emit(Line::new("pointer", false).with("error", "the client reported no picture size"));
        return Ok(());
    };
    // The step that changes the scale and puts it back runs this twice; the
    // second line, at 2000, is `scale_restored`.
    let at_scale = scale == Some(expect_scale_milli);
    report.emit(
        Line::new("pointer_scale", at_scale)
            .with("scale_milli", scale)
            .with("expected", expect_scale_milli)
            .with("scale_restored", at_scale && expect_scale_milli == 2000),
    );
    let results = input_check::run::run_cases(&mut harness, &input_check::pointer_plan(size));
    for r in &results {
        report.emit(r.line("pointer"));
    }
    report.emit(input_check::run::summary("pointer", &results));
    Ok(())
}

#[cfg(windows)]
fn clipboard(
    host: HostTarget,
    client: Option<PathBuf>,
    only: &[clip_check::ClipCase],
    server_path: String,
    ssh: PathBuf,
    log_dir: PathBuf,
    report: &mut Report,
) -> anyhow::Result<()> {
    let client = match client {
        Some(c) => c,
        None => std::env::current_exe()?.with_file_name("gliff-windows.exe"),
    };
    let mut harness = clip_check::win32::Win32Harness::new(clip_check::win32::Launch {
        client,
        host,
        server_path,
        ssh,
        log_dir: std::path::absolute(&log_dir)?,
    });
    let nonce = clip_check::nonce();
    for line in clip_check::run(&mut harness, only, &nonce, clip_check::Timing::default()) {
        report.emit(line);
    }
    Ok(())
}

#[cfg(not(windows))]
fn clipboard(
    _host: HostTarget,
    _client: Option<PathBuf>,
    _only: &[clip_check::ClipCase],
    _server_path: String,
    _ssh: PathBuf,
    _log_dir: PathBuf,
    report: &mut Report,
) -> anyhow::Result<()> {
    report.emit(Line::new("clipboard", false).with(
        "error",
        "clipboard drives the Windows clipboard: Windows only",
    ));
    Ok(())
}

#[cfg(not(windows))]
fn keys(
    _args: InputArgs,
    _only: &[input_check::KeyGroup],
    _cases: &[String],
    _manual: bool,
    report: &mut Report,
) -> anyhow::Result<()> {
    report.emit(Line::new("keys", false).with("error", "keys drives Win32 input: Windows only"));
    Ok(())
}

#[cfg(not(windows))]
fn pointer(_args: InputArgs, _expect_scale_milli: u32, report: &mut Report) -> anyhow::Result<()> {
    report.emit(
        Line::new("pointer", false).with("error", "pointer drives Win32 input: Windows only"),
    );
    Ok(())
}

const fn default_ssh() -> &'static str {
    if cfg!(windows) { "ssh.exe" } else { "ssh" }
}

/// Prints each line as soon as it exists and remembers any failure.
#[derive(Default)]
struct Report {
    failed: bool,
}

impl Report {
    fn emit(&mut self, line: Line) {
        use std::io::Write;
        self.failed |= !line.ok;
        let mut out = std::io::stdout().lock();
        // A closed stdout leaves nothing to report to; the exit code still
        // carries the result.
        let _ = writeln!(out, "{}", line.to_json());
        let _ = out.flush();
    }
}

/// Where and how `present` runs the client (read on Windows only).
#[cfg_attr(not(windows), allow(dead_code))]
struct PresentArgs {
    host: HostTarget,
    client: Option<PathBuf>,
    server_path: String,
    ssh: PathBuf,
    log_dir: PathBuf,
    placement: PlacementProbe,
}

#[cfg(windows)]
fn present(args: PresentArgs, plan: &present_check::run::Plan) -> anyhow::Result<Line> {
    let PresentArgs {
        host,
        client,
        server_path,
        ssh,
        log_dir,
        placement,
    } = args;
    let client = match client {
        Some(c) => c,
        None => std::env::current_exe()?.with_file_name("gliff-windows.exe"),
    };
    let log_dir = std::path::absolute(&log_dir)?;
    std::fs::create_dir_all(&log_dir)?;
    let mut harness = present_check::win32::Win32Harness::new(present_check::win32::Launch {
        client,
        host,
        server_path,
        ssh,
        stats_out: log_dir.join("present-stats.json"),
        log_dir,
        placement,
    })?;
    Ok(present_check::run::run(&mut harness, plan))
}

#[cfg(not(windows))]
fn present(_args: PresentArgs, _plan: &present_check::run::Plan) -> anyhow::Result<Line> {
    Ok(Line::new("present", false).with("error", "present drives a Win32 window: Windows only"))
}

#[cfg(windows)]
fn tabs(
    hosts: Vec<HostTarget>,
    client: Option<PathBuf>,
    countdown_s: u64,
    server_path: String,
    ssh: PathBuf,
    log_dir: PathBuf,
) -> anyhow::Result<Line> {
    use tabs_check::judge::Expect;
    use tabs_check::run::Plan;
    if hosts.len() < 3 {
        return Ok(Line::new("tabs", false).with("error", "tabs needs three or more --host"));
    }
    let client = match client {
        Some(c) => c,
        None => std::env::current_exe()?.with_file_name("gliff-windows.exe"),
    };
    let log_dir = std::path::absolute(&log_dir)?;
    std::fs::create_dir_all(&log_dir)?;
    let expect = Expect::standard(hosts.iter().map(|h| h.as_str().to_owned()).collect());
    eprintln!(
        "gwin-verify tabs will click the client's window and press Ctrl+Alt chords in {countdown_s} s.\n\
         It takes over this keyboard and mouse for about a minute: keep your hands off, and stop it with Ctrl+C now if this is not the machine you sit at."
    );
    std::thread::sleep(Duration::from_secs(countdown_s));
    let mut harness = tabs_check::win32::Win32Harness::new(tabs_check::win32::Launch {
        client,
        hosts,
        server_path,
        ssh,
        stats_out: log_dir.join("tabs-stats.json"),
        log_dir,
    })?;
    Ok(tabs_check::run::run(&mut harness, &Plan::new(expect)))
}

#[cfg(not(windows))]
fn tabs(
    _hosts: Vec<HostTarget>,
    _client: Option<PathBuf>,
    _countdown_s: u64,
    _server_path: String,
    _ssh: PathBuf,
    _log_dir: PathBuf,
) -> anyhow::Result<Line> {
    Ok(Line::new("tabs", false).with("error", "tabs drives a Win32 window: Windows only"))
}

fn pending(check: &'static str, what: &'static str) -> Line {
    Line::new(check, false).with("pending", what)
}

fn run(check: Check, report: &mut Report) -> anyhow::Result<()> {
    match check {
        Check::Decode {
            clip,
            sizes,
            frames,
            bitrate,
            max_p95_ms,
            min_fps,
            kill_rule,
        } => {
            let limits = Limits {
                max_p95_ms,
                min_fps,
            };
            let clips: Box<dyn Iterator<Item = anyhow::Result<Clip>>> = if clip.is_empty() {
                Box::new(
                    sizes
                        .into_iter()
                        .map(move |size| clip::synthetic(size, frames, bitrate)),
                )
            } else {
                Box::new(clip.into_iter().map(|path| Clip::load(&path)))
            };
            let mut judged = false;
            for clip in clips {
                let line = match clip.and_then(|c| {
                    decode_check::measure(&c).map(|m| decode_check::judge(&c, &m, limits))
                }) {
                    Ok(line) => line,
                    Err(e) => Line::new("decode", false).with("error", format!("{e:#}")),
                };
                judged |= line.metrics.get("kill_rule") == Some(&serde_json::Value::Bool(true));
                report.emit(line);
            }
            if kill_rule && !judged {
                report.emit(
                    Line::new("kill-rule", false)
                        .with("error", "no 3840x2400 clip judged at 33.3 ms p95 / 30 fps"),
                );
            }
        }
        Check::Ssh {
            target,
            server_path,
            output,
            ssh,
            log_dir,
            timeout_s,
        } => {
            let mode = match output {
                Some(output) => DesktopMode::Mirror { output },
                None => DesktopMode::Headless,
            };
            // Absolute, so the reported stderr_log can be found afterwards.
            let log_dir = std::path::absolute(&log_dir)?;
            let spec = SshSpec {
                ssh,
                target,
                server_path,
                mode,
                log_dir,
                log_tag: None,
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            report.emit(runtime.block_on(ssh_check::run(spec, Duration::from_secs(timeout_s))));
        }
        Check::Config { client, log_dir } => {
            let client = match client {
                Some(client) => client,
                None => std::env::current_exe()?
                    .with_file_name(format!("gliff-windows{}", std::env::consts::EXE_SUFFIX)),
            };
            report.emit(config_check::run(&config_check::Options {
                client,
                log_dir: std::path::absolute(&log_dir)?,
            }));
        }
        Check::MakeClip {
            size,
            annexb,
            frames,
            bitrate,
            out,
        } => {
            let clip = match annexb {
                Some(path) => {
                    let data = std::fs::read(&path)?;
                    clip::from_annexb(
                        &data,
                        size,
                        format!(
                            "annexb:{}",
                            path.file_name().map_or_else(
                                || path.display().to_string(),
                                |n| n.to_string_lossy().into_owned()
                            )
                        ),
                    )?
                }
                None => clip::synthetic(size, frames, bitrate)?,
            };
            let mut file = std::io::BufWriter::new(std::fs::File::create(&out)?);
            clip.write_to(&mut file)?;
            // Drop would flush too, but would swallow a failed last write.
            std::io::Write::flush(&mut file)?;
            report.emit(
                Line::new("make-clip", true)
                    .with("size", size.to_string())
                    .with("frames", clip.units.len())
                    .with("source", clip.source.clone())
                    .with("bytes", clip.bytes())
                    .with("out", out.display().to_string()),
            );
        }
        Check::Render { size, frames } => {
            let mut out = std::io::BufWriter::new(std::io::stdout().lock());
            let result = clip::render_i420(size, frames, &mut out);
            let line = Line::new("render", result.is_ok())
                .with("size", size.to_string())
                .with("frames", frames);
            let line = match result {
                Ok(()) => line,
                Err(e) => line.with("error", format!("{e:#}")),
            };
            report.failed |= !line.ok;
            eprintln!("{}", line.to_json());
        }
        Check::Stream {
            host,
            size,
            seconds,
            min_fps,
            fps_of_cap,
            stall_s,
            settle_s,
            resize_timeout_s,
            png,
            pattern_tolerance,
            placement,
            present_ms,
            server_path,
            ssh,
            log_dir,
        } => {
            let opts = stream_check::Options {
                spec: SshSpec {
                    ssh,
                    target: host,
                    server_path,
                    mode: DesktopMode::Headless,
                    log_dir: std::path::absolute(&log_dir)?,
                    log_tag: None,
                },
                size: (size.width, size.height),
                window: Duration::from_secs(seconds),
                min_fps,
                fps_of_cap,
                stall: Duration::from_secs(stall_s),
                settle: Duration::from_secs(settle_s),
                resize_timeout: Duration::from_secs(resize_timeout_s),
                png: std::path::absolute(&png)?,
                tolerance: pattern_tolerance,
                placement,
                present_every: present_ms.map(Duration::from_millis),
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            report.emit(runtime.block_on(stream_check::run(opts)));
        }
        Check::Present {
            host,
            client,
            seconds,
            png,
            pattern_tolerance,
            placement,
            server_path,
            ssh,
            log_dir,
        } => {
            let plan = present_check::run::Plan::panel(
                seconds,
                std::path::absolute(&png)?,
                pattern_tolerance,
            );
            let args = PresentArgs {
                host,
                client,
                server_path,
                ssh,
                log_dir,
                placement,
            };
            report.emit(present(args, &plan)?);
        }
        Check::Tabs {
            hosts,
            client,
            countdown_s,
            server_path,
            ssh,
            log_dir,
        } => report.emit(tabs(hosts, client, countdown_s, server_path, ssh, log_dir)?),
        Check::StripShot {
            out,
            scales,
            scenes,
            shapes_only,
        } => report.emit(strip_shot::run(&strip_shot::Options {
            out,
            scales,
            scenes,
            shapes_only,
        })),
        Check::PanelShot {
            out,
            scales,
            shapes_only,
        } => report.emit(panel_shot::run(&panel_shot::Options {
            out,
            scales,
            shapes_only,
        })),
        Check::Keys {
            input,
            only,
            cases,
            manual,
        } => keys(input, &only, &cases, manual, report)?,
        Check::Pointer {
            input,
            expect_scale_milli,
        } => pointer(input, expect_scale_milli, report)?,
        Check::Clipboard {
            host,
            client,
            only,
            server_path,
            ssh,
            log_dir,
        } => clipboard(host, client, &only, server_path, ssh, log_dir, report)?,
        Check::Gpu => report.emit(pending("gpu", "hardware decode")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut report = Report::default();
    if let Err(e) = run(cli.check, &mut report) {
        report.emit(Line::new("gwin-verify", false).with("error", format!("{e:#}")));
    }
    if report.failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
