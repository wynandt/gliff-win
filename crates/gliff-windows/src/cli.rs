//! Command-line arguments of `gliff-windows`.

use std::path::PathBuf;

use crate::grab_state::Follow;
use crate::history::HostTarget;
use crate::input::InjectedKeys;
use crate::input::SuperL;
use crate::keymap::Chord;
use crate::local_chords::LocalChordList;
use crate::pointer::SideButtons;
use crate::settings::GrabTimeout;
use crate::settings::Layer;

/// `gliff-windows [user@host ...]`: one window with a tab for each remote
/// Hyprland desktop.
///
/// With no target, the machines the last run left open are reconnected
/// ([`crate::tabs::Tabs::start_up`]); with targets, those are added to them
/// and the first one named is shown.
///
/// A setting a flag does not give has no value here; the config file and then
/// the default fill it in ([`crate::settings::resolve`]). A flag that turns
/// something on or off only switches it the way its name says: leaving it
/// out leaves the file's choice.
#[derive(Debug, Clone, clap::Parser)]
#[command(version, about)]
pub struct Args {
    /// Each `user@host`, a bare host or alias, or an IPv6 address; passed to
    /// ssh unchanged after `--`. A `host:port` is refused: ssh takes a port
    /// with `-p` or from its own configuration, not in the destination. Every
    /// flag below applies to every tab.
    #[arg(value_name = "TARGET")]
    pub targets: Vec<HostTarget>,
    /// The folder of `config.toml` and `recent-hosts.toml`; default
    /// `%APPDATA%\gliff-windows`. Checks and tests always give a temporary
    /// one, so they never touch the real files.
    #[arg(long, value_name = "DIR")]
    pub config_dir: Option<PathBuf>,
    /// Print the settings in effect, with where each came from, as JSON, and
    /// exit. Starts no session and changes nothing. On Windows the program
    /// has no console: send its output to a file or a pipe.
    #[arg(long)]
    pub print_settings: bool,
    /// The `gliff-server` to run on the remote machine: an absolute path,
    /// or a bare command name that the remote shell finds on its `PATH`
    /// (default `gliff-server`).
    #[arg(long)]
    pub server_path: Option<String>,
    /// Mirror this remote output instead of the focused one. Cannot be
    /// combined with `--headless`.
    #[arg(long, conflicts_with = "headless")]
    pub output: Option<String>,
    /// Use a private screen that follows the window, instead of mirroring
    /// the remote machine's focused screen (the default).
    #[arg(long)]
    pub headless: bool,
    /// The ssh client to run (default `%SystemRoot%\System32\OpenSSH\ssh.exe`,
    /// or `ssh.exe` when that file does not exist).
    #[arg(long)]
    pub ssh: Option<PathBuf>,
    /// The chord that gives the keyboard back to Windows: `ctrl+alt+home`
    /// (the default), `shift+esc`, or any chord of keys joined by `+`.
    #[arg(long)]
    pub release_chord: Option<Chord>,
    /// What the keyboard follows: the focused window (default) or the
    /// pointer, so that hovering is enough.
    #[arg(long, value_enum)]
    pub grab_follows: Option<Follow>,
    /// Let go of the keyboard this many seconds (1 to 86400) after the grab
    /// starts or is armed again (a click in `focus` mode, the pointer leaving
    /// in `hover` mode), and say so in the log. The release chord still works
    /// first. Off by default: the grab then lasts until the chord or a focus
    /// or pointer change.
    #[arg(long, value_name = "N")]
    pub grab_timeout_secs: Option<GrabTimeout>,
    /// Keys that other programs inject (password-manager auto-type): send
    /// them to the remote machine (default) or leave them to Windows.
    #[arg(long, value_enum)]
    pub injected_keys: Option<InjectedKeys>,
    /// Send volume and media keys to the remote machine instead of keeping
    /// them on Windows.
    #[arg(long)]
    pub forward_media_keys: bool,
    /// How Super+L reaches the remote machine: the stand-in chord only,
    /// which changes nothing on Windows (`chord`, the default), the lock
    /// setting held while the window has the keyboard, which switches
    /// Windows locking off meanwhile (`registry`), or not at all (`off`).
    #[arg(long, value_enum)]
    pub super_l: Option<SuperL>,
    /// The chord that sends Super+L to the remote machine (default
    /// `ctrl+alt+l`; always on, except with `--super-l off`).
    #[arg(long)]
    pub super_l_chord: Option<Chord>,
    /// Chords that stay on Windows while the keyboard is grabbed, separated
    /// by commas, or `none`. The default keeps Win+Shift+S (the snipping
    /// overlay) local, so a screenshot goes to the Windows clipboard and from
    /// there to the remote machine. A chord is modifiers (ctrl, alt, shift,
    /// win) and one key, for example `win+shift+s,win+v`.
    #[arg(long, value_name = "LIST")]
    pub local_chords: Option<LocalChordList>,
    /// Which codes the two thumb buttons send (default `upstream`).
    #[arg(long, value_enum)]
    pub side_buttons: Option<SideButtons>,
    /// Do not share the clipboard with the remote machine.
    #[arg(long)]
    pub no_clipboard: bool,
    /// Let a copy made on the remote machine into the Windows clipboard
    /// history and cloud sync (Win+V). Off by default.
    #[arg(long)]
    pub clipboard_history: bool,
    /// Test only: make the keyboard hook wait this many milliseconds on the
    /// next F24 presses, to prove it recovers when Windows drops it. Exists
    /// only in a build with the `test-hooks` feature.
    #[cfg(feature = "test-hooks")]
    #[arg(long, hide = true)]
    pub hook_stall_test_ms: Option<u64>,
    /// Test only: make the keyboard hook thread panic on an injected F23,
    /// to prove the lock setting is put back after a panic. Exists only in a
    /// build with the `test-hooks` feature.
    #[cfg(feature = "test-hooks")]
    #[arg(long, hide = true)]
    pub test_panic: bool,
    /// Write the session and present statistics here (JSON), about once a
    /// second and at exit; `gwin-verify present` reads it.
    #[arg(long)]
    pub stats_out: Option<PathBuf>,
    /// Where the client log and ssh's stderr go; default
    /// `%LOCALAPPDATA%\gliff-windows\logs`.
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    /// Which GPU presents: the one driving the window's
    /// monitor (default), or a DXGI preference.
    #[arg(long, value_enum)]
    pub adapter: Option<AdapterChoice>,
}

/// A helper mode of the same exe, given instead of a target. The lock
/// setting's helper process and its by-hand recovery use these; they are
/// looked at before the normal arguments, which need a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Helper {
    /// `--lock-watchdog <pid> <start>`: wait for that client to end, then
    /// put the lock setting back if it still holds the client's marker.
    LockWatchdog { pid: u32, start: u64 },
    /// `--restore-lock-policy`: put the lock setting back after a client
    /// that did not get to.
    RestoreLockPolicy,
}

impl Helper {
    /// `None` when the arguments are a normal run. `Some(Err)` explains a
    /// helper flag with bad values.
    pub fn from_args(args: &[String]) -> Option<Result<Self, String>> {
        match args.get(1).map(String::as_str) {
            Some("--restore-lock-policy") => Some(Ok(Self::RestoreLockPolicy)),
            Some("--lock-watchdog") => {
                let parsed = (|| {
                    let pid = args.get(2)?.parse().ok()?;
                    let start = args.get(3)?.parse().ok()?;
                    Some(Self::LockWatchdog { pid, start })
                })();
                Some(parsed.ok_or_else(|| {
                    "--lock-watchdog needs a process id and a start time".to_owned()
                }))
            }
            _ => None,
        }
    }
}

/// `--adapter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AdapterChoice {
    /// The adapter whose output holds the window's monitor (no
    /// cross-adapter copy on a machine with two GPUs).
    Monitor,
    /// `DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE`.
    HighPerformance,
    /// `DXGI_GPU_PREFERENCE_MINIMUM_POWER`.
    MinimumPower,
}

impl AdapterChoice {
    /// The `--adapter` spelling, for the stats file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Monitor => "monitor",
            Self::HighPerformance => "high-performance",
            Self::MinimumPower => "minimum-power",
        }
    }
}

/// The default remote server command, found on the remote `PATH`, as upstream's
/// client finds it.
pub const DEFAULT_SERVER_PATH: &str = "gliff-server";

/// The `screen` setting: which remote screen a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Screen {
    /// A real output of the remote machine: the focused one unless `output`
    /// names another (the default, as upstream's client).
    Mirror,
    /// A private headless output that follows the window (`--headless`).
    Own,
}

/// What `--output` takes for "the focused output" (upstream's `auto`).
pub const FOCUSED_OUTPUT: &str = "auto";

/// Which remote desktop the session shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopMode {
    /// A private headless output sized to the window (`--headless`).
    Headless,
    /// An existing output, e.g. `eDP-1` (`--output`), or the focused one
    /// ([`FOCUSED_OUTPUT`]).
    Mirror { output: String },
}

impl DesktopMode {
    /// The focused output of the remote machine: the built-in default.
    pub fn mirror_focused() -> Self {
        Self::Mirror {
            output: FOCUSED_OUTPUT.to_owned(),
        }
    }
}

impl Args {
    /// The settings the flags give, for [`crate::settings::resolve`].
    pub fn layer(&self) -> Layer {
        Layer {
            release_chord: self.release_chord.clone(),
            grab_follows: self.grab_follows,
            grab_timeout_secs: self.grab_timeout_secs.map(Some),
            injected_keys: self.injected_keys,
            forward_media_keys: self.forward_media_keys.then_some(true),
            local_chords: self.local_chords.clone(),
            side_buttons: self.side_buttons,
            super_l_chord: self.super_l_chord.clone(),
            clipboard_history: self.clipboard_history.then_some(true),
            share_clipboard: self.no_clipboard.then_some(false),
            server_path: self.server_path.clone(),
            ssh: self.ssh.clone(),
            screen: self.headless.then_some(Screen::Own),
            // `--headless` also drops an output the file names.
            output: if self.headless {
                Some(None)
            } else {
                self.output.clone().map(Some)
            },
            super_l: self.super_l,
            log_dir: self.log_dir.clone().map(Some),
            adapter: self.adapter,
            ..Layer::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use std::path::PathBuf;

    use super::AdapterChoice;
    use super::Args;
    use super::Helper;
    use super::Screen;
    use crate::grab_state::Follow;
    use crate::input::InjectedKeys;
    use crate::input::SuperL;
    use crate::keymap::Chord;
    use crate::local_chords::LocalChordList;
    use crate::settings::GrabTimeout;
    use crate::settings::Layer;

    fn args(extra: &[&str]) -> Args {
        let mut all = vec!["gliff-windows", "user@host"];
        all.extend_from_slice(extra);
        Args::try_parse_from(all).expect("parse")
    }

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn the_test_flags_exist_only_in_a_test_hooks_build() {
        for flag in [&["--test-panic"][..], &["--hook-stall-test-ms", "100"]] {
            let mut all = vec!["gliff-windows", "user@host"];
            all.extend_from_slice(flag);
            assert_eq!(
                Args::try_parse_from(all).is_ok(),
                cfg!(feature = "test-hooks"),
                "{flag:?}"
            );
        }
    }

    #[test]
    fn no_flags_give_no_settings() {
        assert_eq!(
            args(&[]).layer(),
            Layer::default(),
            "the file and the defaults fill every setting"
        );
    }

    #[test]
    fn headless_asks_for_the_private_screen_and_drops_a_file_output() {
        let layer = args(&["--headless"]).layer();
        assert_eq!(layer.screen, Some(Screen::Own));
        assert_eq!(
            layer.output,
            Some(None),
            "an output named in the file does not survive --headless"
        );
    }

    #[test]
    fn headless_and_a_named_output_cannot_be_combined() {
        assert!(
            Args::try_parse_from([
                "gliff-windows",
                "user@host",
                "--headless",
                "--output",
                "DP-1"
            ])
            .is_err()
        );
    }

    #[test]
    fn the_headless_flag_beats_a_file_that_mirrors() {
        use crate::settings::resolve;
        let table: toml::Table = "screen = \"mirror\"\noutput = \"DP-1\""
            .parse()
            .expect("toml");
        let (file, notes) = Layer::from_table(&table);
        assert!(notes.is_empty(), "{notes:?}");
        let got = resolve(&args(&["--headless"]).layer(), &file).expect("resolves");
        assert_eq!(got.settings.desktop_mode(), super::DesktopMode::Headless);
    }

    #[test]
    fn the_targets_are_optional_and_may_be_several() {
        let names = |args: &Args| -> Vec<String> {
            args.targets.iter().map(|t| t.as_str().to_owned()).collect()
        };
        assert_eq!(names(&args(&[])), ["user@host"]);
        let bare = Args::try_parse_from(["gliff-windows"]).expect("a bare start parses");
        assert!(bare.targets.is_empty());
        let flags_only =
            Args::try_parse_from(["gliff-windows", "--super-l", "off"]).expect("parse");
        assert!(flags_only.targets.is_empty());
        let several =
            Args::try_parse_from(["gliff-windows", "a@one", "two", "--no-clipboard", "c@three"])
                .expect("parse");
        assert_eq!(names(&several), ["a@one", "two", "c@three"]);
        assert!(several.no_clipboard, "a flag between targets applies");
        let print = Args::try_parse_from(["gliff-windows", "--print-settings"]).expect("parse");
        assert!(print.print_settings);
        assert!(print.targets.is_empty());
    }

    #[test]
    fn a_bad_target_among_several_refuses_the_whole_line() {
        assert!(Args::try_parse_from(["gliff-windows", "a@one", "host:22"]).is_err());
    }

    #[test]
    fn the_config_dir_flag_is_a_path() {
        assert_eq!(
            args(&["--config-dir", "/tmp/c"]).config_dir,
            Some(PathBuf::from("/tmp/c"))
        );
        assert_eq!(args(&[]).config_dir, None);
    }

    #[test]
    fn the_grab_timeout_is_a_positive_number_of_seconds() {
        let layer = args(&["--grab-timeout-secs", "90"]).layer();
        assert_eq!(
            layer.grab_timeout_secs,
            Some(Some(GrabTimeout::new(90).expect("in range")))
        );
        for bad in ["0", "-1", "soon", "86401"] {
            assert!(
                Args::try_parse_from(["gliff-windows", "user@host", "--grab-timeout-secs", bad])
                    .is_err(),
                "{bad} is refused"
            );
        }
    }

    #[test]
    fn each_value_flag_lands_in_its_setting() {
        let layer = args(&[
            "--grab-follows",
            "hover",
            "--injected-keys",
            "local",
            "--super-l",
            "off",
            "--super-l-chord",
            "ctrl+alt+k",
            "--adapter",
            "minimum-power",
            "--server-path",
            "/opt/gliff-server",
            "--ssh",
            "C:/ssh.exe",
            "--output",
            "OUT-1",
            "--log-dir",
            "/x",
        ])
        .layer();
        assert_eq!(layer.grab_follows, Some(Follow::Hover));
        assert_eq!(layer.injected_keys, Some(InjectedKeys::Local));
        assert_eq!(layer.super_l, Some(SuperL::Off));
        assert_eq!(
            layer.super_l_chord,
            Some("ctrl+alt+k".parse().expect("chord"))
        );
        assert_eq!(layer.adapter, Some(AdapterChoice::MinimumPower));
        assert_eq!(layer.server_path.as_deref(), Some("/opt/gliff-server"));
        assert_eq!(layer.ssh, Some(PathBuf::from("C:/ssh.exe")));
        assert_eq!(layer.output, Some(Some("OUT-1".to_owned())));
        assert_eq!(layer.log_dir, Some(Some(PathBuf::from("/x"))));
    }

    #[test]
    fn a_switch_flag_only_switches_on_and_the_clipboard_flag_only_off() {
        let layer = args(&[
            "--forward-media-keys",
            "--clipboard-history",
            "--no-clipboard",
        ])
        .layer();
        assert_eq!(layer.forward_media_keys, Some(true));
        assert_eq!(layer.clipboard_history, Some(true));
        assert_eq!(
            layer.share_clipboard,
            Some(false),
            "--no-clipboard shares nothing"
        );
        let none = args(&[]).layer();
        assert_eq!(none.forward_media_keys, None, "left out: the file decides");
        assert_eq!(none.share_clipboard, None);
    }

    #[test]
    fn local_chords_take_a_list_or_none() {
        assert_eq!(
            args(&["--local-chords", "none"]).local_chords,
            Some(LocalChordList(vec![]))
        );
        assert_eq!(
            args(&["--local-chords", "win+shift+s,win+v"])
                .local_chords
                .expect("given")
                .0
                .len(),
            2
        );
        assert!(
            Args::try_parse_from(["gliff-windows", "t", "--local-chords", "s"]).is_err(),
            "a chord needs a modifier"
        );
    }

    #[test]
    fn the_release_chord_takes_shift_esc_and_other_chords() {
        let shift_esc = "shift+esc".parse::<Chord>().expect("a chord");
        assert_eq!(
            args(&["--release-chord", "shift+esc"]).release_chord,
            Some(shift_esc)
        );
        let other = "ctrl+alt+end".parse::<Chord>().expect("a chord");
        assert_eq!(
            args(&["--release-chord", "Ctrl+Alt+End"]).release_chord,
            Some(other)
        );
    }

    #[test]
    fn a_chord_that_does_not_parse_is_refused() {
        let err = Args::try_parse_from(["gliff-windows", "t", "--release-chord", "ctrl+banana"])
            .expect_err("unknown key");
        assert!(err.to_string().contains("banana"), "names the key: {err}");
    }

    #[test]
    fn the_adapter_and_the_stats_file_parse() {
        let args = args(&[
            "--stats-out",
            "C:/t/present-stats.json",
            "--adapter",
            "high-performance",
        ]);
        assert_eq!(
            args.stats_out,
            Some(PathBuf::from("C:/t/present-stats.json"))
        );
        assert_eq!(args.adapter, Some(AdapterChoice::HighPerformance));
        assert_eq!(AdapterChoice::HighPerformance.name(), "high-performance");
        assert!(
            Args::try_parse_from(["gliff-windows", "h", "--adapter", "discrete"]).is_err(),
            "unknown adapters are refused"
        );
    }

    #[test]
    fn the_helper_modes_are_told_apart_from_a_normal_run() {
        assert_eq!(
            Helper::from_args(&strings(&["gliff-windows", "user@host"])),
            None
        );
        assert_eq!(
            Helper::from_args(&strings(&["gliff-windows", "--restore-lock-policy"])),
            Some(Ok(Helper::RestoreLockPolicy))
        );
        assert_eq!(
            Helper::from_args(&strings(&[
                "gliff-windows",
                "--lock-watchdog",
                "412",
                "1337"
            ])),
            Some(Ok(Helper::LockWatchdog {
                pid: 412,
                start: 1337
            }))
        );
    }

    #[test]
    fn a_watchdog_without_its_values_says_so() {
        for bad in [
            vec!["gliff-windows", "--lock-watchdog"],
            vec!["gliff-windows", "--lock-watchdog", "412"],
            vec!["gliff-windows", "--lock-watchdog", "x", "1"],
        ] {
            let got = Helper::from_args(&strings(&bad)).expect("a helper mode");
            assert!(got.is_err(), "{bad:?} is refused");
        }
    }
}
