//! The gate's cases as data: what to do to the machine, and what the remote machine
//! log, the foreground window and the client's stats must then show.
//! Pure; [`super::run`] drives them.

use std::time::Duration;

use gliff_windows::keymap::Press;

use super::scan::Scan;
use super::scan::chord_presses;
use super::scan::scan_of;

/// Where keyboard focus goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Client,
    Notepad,
}

/// Where the pointer goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    OverClient,
    OffClient,
}

/// A keyboard layout step. Loaded for the run only and unloaded after; the
/// user's own layouts are never changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutStep {
    Load(&'static str),
    Activate(&'static str),
    Unload(&'static str),
    /// Back to the layout the session had before the run touched it.
    Restore,
}

/// A mouse button other than the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Middle,
    Back,
    Forward,
}

/// Which wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wheel {
    Vertical,
    Horizontal,
}

/// One thing the harness does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// `SendInput` with scancodes; the sequence is sent as given.
    Keys(Vec<(Scan, Press)>),
    Pause(Duration),
    Focus(Window),
    Pointer(Place),
    Layout(LayoutStep),
    ClearNotepad,
    /// Close whatever Windows opened over the client (a flyout, an overlay):
    /// the text sink takes the focus, then Escape goes to it, so Escape
    /// never reaches the remote machine. Best effort, and a no-op when the sink
    /// cannot be focused. The runner sends it after judging every risky
    /// case, so nothing a chord opened is left for the next one.
    Dismiss,
    /// A left click at a remote logical point (the client's picture is
    /// inverted back to a screen point).
    Click {
        remote: (f64, f64),
    },
    Button(Side),
    /// Ask the remote machine for the focus of its `wev` window (`gwin-keys focus`)
    /// and fail the case, with what Hyprland says, when it does not stay.
    /// Hyprland leaves the active window empty once the session's output
    /// exists, so the layout cases ask right before their keys.
    RemoteFocus,
    /// One wheel event of this many Windows units.
    WheelEvent {
        wheel: Wheel,
        delta: i16,
    },
}

/// What the remote log must show for the case.
#[derive(Clone, Debug, PartialEq)]
pub enum Remote {
    /// These entries and no others, in this order.
    Exactly(Vec<String>),
    /// No entry at all.
    Nothing,
    /// These entries in order (others may be between), and none of
    /// `never`.
    InOrder {
        entries: Vec<String>,
        never: Vec<String>,
    },
    /// One `mouse:272 x,y` entry per target, each within `tolerance`
    /// remote logical pixels of it, and no other entry.
    Clicks {
        targets: Vec<(f64, f64)>,
        tolerance: f64,
    },
}

/// What Notepad holds at the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notepad {
    Unchecked,
    Empty,
    Text(&'static str),
}

/// Whether the client window is still the foreground one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foreground {
    Unchecked,
    Client,
}

/// What the client's stats file must say.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ClientExpect {
    pub grab: Option<bool>,
    /// Every one of these must appear in a `keymap_sent` line.
    pub keymap_contains: Vec<&'static str>,
    /// At least this many hook reinstalls, which proves the hook really was
    /// dropped (0: not checked).
    pub min_hook_reinstalls: u32,
}

/// What the remote machine's raw key events (`key:down:<evdev>`) must show. Any key
/// the case did not send, other than `extra`, is a stray and fails the case.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyExpect {
    /// Each of these must go down on the remote machine.
    pub need: Vec<u16>,
    /// Each of these must go up on the remote machine, and its last event must be
    /// that up (a key still held at the end is a stuck key).
    pub need_up: Vec<u16>,
    /// None of these may go down on the remote machine.
    pub never: Vec<u16>,
    /// Keys the client sends that the case did not (the stand-in's Super).
    pub extra: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expect {
    /// The bind entries of the remote log (raw key and typed entries are
    /// judged on their own below).
    pub remote: Remote,
    pub keys: KeyExpect,
    /// The keysyms a focused remote machine client typed (`typed:<sym>`), in order;
    /// `None` leaves them unchecked.
    pub typed: Option<Vec<&'static str>>,
    pub notepad: Notepad,
    pub foreground: Foreground,
    pub client: ClientExpect,
}

impl Expect {
    pub fn remote(remote: Remote) -> Self {
        Self {
            remote,
            keys: KeyExpect::default(),
            typed: None,
            notepad: Notepad::Unchecked,
            foreground: Foreground::Unchecked,
            client: ClientExpect::default(),
        }
    }
}

/// One case: the client flags it needs, what to do, what must follow.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub name: String,
    /// Extra client arguments; the harness restarts the client when they
    /// change between cases.
    pub flags: Vec<String>,
    pub acts: Vec<Act>,
    pub expect: Expect,
    /// A chord Windows would act on (open a window) if the client's keyboard
    /// hook were not in place. When one of these reaches the remote machine with
    /// nothing, the run stops sending the rest of them.
    pub risky: bool,
}

/// How long the client and the remote machine get to react to the keys of a case.
pub const SETTLE: Duration = Duration::from_millis(600);

fn chord(chord: &str) -> Vec<(Scan, Press)> {
    chord_presses(chord).expect("the gate's chords use known keys")
}

/// `keys` pressed and released one after the other.
fn taps(keys: &[&str]) -> Vec<(Scan, Press)> {
    keys.iter()
        .flat_map(|k| {
            let s = scan_of(k).expect("the gate's keys are known");
            [(s, Press::Down), (s, Press::Up)]
        })
        .collect()
}

fn flags(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn pause() -> Act {
    Act::Pause(SETTLE)
}

/// A case whose client is the default one, focused and grabbed.
fn plain(name: &str, acts: Vec<Act>, expect: Expect) -> Case {
    Case {
        name: name.to_owned(),
        risky: false,
        flags: Vec::new(),
        acts,
        expect,
    }
}

fn risky(case: Case) -> Case {
    Case {
        risky: true,
        ..case
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// One row of `docs/phase4/super-chords.tsv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordRow {
    pub n: u32,
    /// The remote machine's name for it, as `gwin-keys` logs it.
    pub chord: String,
}

/// The rows of the chord table (header skipped).
pub fn parse_chord_table(text: &str) -> Result<Vec<ChordRow>, String> {
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let n = fields
                .next()
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| format!("no row number in `{line}`"))?;
            let chord = fields
                .next()
                .ok_or_else(|| format!("no chord in `{line}`"))?
                .to_owned();
            Ok(ChordRow { n, chord })
        })
        .collect()
}

/// The keys of a table row's chord: "SUPER alone" is Super, the rest are
/// already `+`-joined key names.
fn row_keys(row: &ChordRow) -> &str {
    row.chord.split(' ').next().unwrap_or(&row.chord)
}

/// Chords the gate does not send itself: when the hook is not in place Windows
/// opens a window the run cannot close hands-free (the input switcher, the
/// screen-snip overlay). They are on the manual list instead
/// (`manual::ITEMS`), where a person sees the Windows machine.
pub const MANUAL_CHORDS: [&str; 2] = ["SUPER+Space", "SUPER+SHIFT+S"];

/// The Super chords the gate sends while the client is grabbed (the table
/// less [`MANUAL_CHORDS`]).
pub fn super_chords(rows: &[ChordRow]) -> Result<Vec<Case>, String> {
    rows.iter()
        .filter(|row| !MANUAL_CHORDS.contains(&row.chord.as_str()))
        .map(|row| {
            let presses = chord_presses(row_keys(row))?;
            Ok(risky(plain(
                &format!("chord {} {}", row.n, row.chord),
                vec![Act::Focus(Window::Client), Act::Keys(presses), pause()],
                Expect {
                    foreground: Foreground::Client,
                    ..Expect::remote(Remote::Exactly(vec![row.chord.clone()]))
                },
            )))
        })
        .collect()
}

/// The chords Windows would otherwise act on.
pub fn system_chords() -> Vec<Case> {
    let one = |name: &str, keys: &str, logged: &str| {
        risky(plain(
            &format!("system {name}"),
            vec![
                Act::Focus(Window::Client),
                Act::Keys(chord(keys)),
                pause(),
                Act::Keys(taps(&["F10"])),
                pause(),
            ],
            Expect {
                foreground: Foreground::Client,
                ..Expect::remote(Remote::Exactly(names(&[logged, "probe:none"])))
            },
        ))
    };
    let mut cases = vec![
        one("Alt+Tab", "Alt+Tab", "ALT+Tab"),
        one("Alt+F4", "Alt+F4", "ALT+F4"),
        one("Alt+Space", "Alt+Space", "ALT+Space"),
        one("Alt+Esc", "Alt+Escape", "ALT+Escape"),
        one("Ctrl+Esc", "Ctrl+Escape", "CTRL+Escape"),
        one("Ctrl+Shift+Esc", "Ctrl+Shift+Escape", "CTRL+SHIFT+Escape"),
        one("PrtSc", "Print", "Print"),
    ];
    // Five Shifts open the Sticky Keys prompt when Windows gets them.
    cases.push(risky(plain(
        "system Shift x5",
        vec![
            Act::Focus(Window::Client),
            Act::Keys(taps(&["Shift"; 5])),
            pause(),
            Act::Keys(taps(&["F10"])),
            pause(),
        ],
        Expect {
            foreground: Foreground::Client,
            ..Expect::remote(Remote::Exactly(names(&[
                "SHIFT",
                "SHIFT",
                "SHIFT",
                "SHIFT",
                "SHIFT",
                "probe:none",
            ])))
        },
    )));
    cases
}

const GERMAN: &str = "00000407";
const US: &str = "00000409";

/// US first, then German (loaded for the run only), AltGr, and back.
///
/// A plain `Y` key is sent; on a US keymap a focused remote machine client types
/// `y`, on the German one the same key position types `z`. What it typed is
/// the `typed:` stream of the remote log, so the cases show the keymap the
/// client sent, not how the remote machine's binds resolve (they follow the remote machine's
/// own layout, whatever the client sends). The layout is set explicitly
/// before the first case and put back after the last, and by the harness
/// when the run fails.
pub fn layouts() -> Vec<Case> {
    let y = || Act::Keys(taps(&["Y"]));
    let typed = |case: Case, want: &[&'static str]| Case {
        expect: Expect {
            typed: Some(want.to_vec()),
            ..case.expect
        },
        ..case
    };
    vec![
        typed(
            plain(
                "layout us",
                vec![
                    Act::Layout(LayoutStep::Load(US)),
                    Act::Layout(LayoutStep::Activate(US)),
                    pause(),
                    Act::Focus(Window::Client),
                    pause(),
                    Act::RemoteFocus,
                    y(),
                    pause(),
                ],
                Expect {
                    client: ClientExpect {
                        keymap_contains: vec!["layout=us"],
                        ..ClientExpect::default()
                    },
                    ..Expect::remote(Remote::Nothing)
                },
            ),
            &["y"],
        ),
        // A right Shift is an extended key to Windows' hook; it must still
        // be sent, so right Shift+`/` is `?` as left Shift+`/` is.
        typed(
            plain(
                "layout us right shift",
                vec![
                    Act::Focus(Window::Client),
                    pause(),
                    Act::RemoteFocus,
                    Act::Keys(chord("RShift+Slash")),
                    pause(),
                ],
                Expect {
                    keys: KeyExpect {
                        need: vec![54, 53],
                        need_up: vec![54, 53],
                        ..KeyExpect::default()
                    },
                    ..Expect::remote(Remote::Nothing)
                },
            ),
            &["question"],
        ),
        typed(
            plain(
                "layout german",
                vec![
                    Act::Layout(LayoutStep::Load(GERMAN)),
                    Act::Layout(LayoutStep::Activate(GERMAN)),
                    pause(),
                    Act::Focus(Window::Client),
                    pause(),
                    Act::RemoteFocus,
                    y(),
                    pause(),
                ],
                Expect {
                    client: ClientExpect {
                        keymap_contains: vec!["layout=de", "reason=inputlang"],
                        ..ClientExpect::default()
                    },
                    ..Expect::remote(Remote::Nothing)
                },
            ),
            &["z"],
        ),
        // AltGr+Q is `@` on the German layout; Windows adds a left Ctrl to
        // AltGr, which the client sends as it is (evdev 29), and a real
        // Ctrl+Q is still the bind.
        typed(
            plain(
                "layout altgr",
                vec![
                    Act::Focus(Window::Client),
                    pause(),
                    Act::RemoteFocus,
                    Act::Keys(chord("AltGr+Q")),
                    pause(),
                    Act::Keys(chord("Ctrl+Q")),
                    pause(),
                ],
                Expect {
                    keys: KeyExpect {
                        extra: vec![29],
                        ..KeyExpect::default()
                    },
                    ..Expect::remote(Remote::Exactly(names(&["CTRL+Q"])))
                },
            ),
            &["at"],
        ),
        typed(
            plain(
                "layout back to us",
                vec![
                    Act::Layout(LayoutStep::Activate(US)),
                    pause(),
                    Act::Focus(Window::Client),
                    pause(),
                    Act::RemoteFocus,
                    y(),
                    pause(),
                    Act::Layout(LayoutStep::Unload(GERMAN)),
                    Act::Layout(LayoutStep::Restore),
                ],
                Expect {
                    client: ClientExpect {
                        keymap_contains: vec!["layout=us", "reason=inputlang"],
                        ..ClientExpect::default()
                    },
                    ..Expect::remote(Remote::Nothing)
                },
            ),
            &["y"],
        ),
    ]
}

/// `--injected-keys local`: keys sent by another program stay on the Windows machine,
/// so the remote machine sees no bind and no key at all (the raw key stream would
/// show a leak).
pub fn injected_local() -> Vec<Case> {
    vec![Case {
        name: "injected keys local".to_owned(),
        risky: false,
        flags: flags(&["--injected-keys", "local"]),
        acts: vec![
            Act::Focus(Window::Client),
            Act::Keys(chord("LWin+Y")),
            pause(),
        ],
        expect: Expect {
            // LWin and Y.
            keys: KeyExpect {
                never: vec![125, 21],
                ..KeyExpect::default()
            },
            ..Expect::remote(Remote::Nothing)
        },
    }]
}

/// The stand-in chord: one `SUPER+L`, nothing stuck, no lock.
pub fn standin_chord() -> Vec<Case> {
    vec![Case {
        name: "stand-in chord Ctrl+Alt+L".to_owned(),
        risky: false,
        flags: flags(&["--super-l", "chord"]),
        acts: vec![
            Act::Focus(Window::Client),
            Act::Keys(chord("Ctrl+Alt+L")),
            pause(),
            Act::Keys(taps(&["F10"])),
            pause(),
        ],
        expect: Expect {
            foreground: Foreground::Client,
            // The client sends one Super (evdev 125) for the Ctrl+Alt+L the
            // case presses.
            keys: KeyExpect {
                extra: vec![125],
                ..KeyExpect::default()
            },
            ..Expect::remote(Remote::Exactly(names(&["SUPER+L", "probe:none"])))
        },
    }]
}

/// Keys for the capture checks; the remote machine binds `A` and `B`.
fn ab() -> Act {
    Act::Keys(taps(&["A", "B"]))
}

fn capture(name: &str, mode: &[&str], acts: Vec<Act>, expect: Expect) -> Case {
    Case {
        name: name.to_owned(),
        risky: false,
        flags: flags(mode),
        acts,
        expect,
    }
}

/// Notepad and the client side by side, by focus.
pub fn capture_follows_focus() -> Vec<Case> {
    let mode: &[&str] = &["--grab-follows", "focus"];
    vec![
        capture(
            "capture focus: notepad focused",
            mode,
            vec![
                Act::ClearNotepad,
                Act::Focus(Window::Notepad),
                Act::Pointer(Place::OverClient),
                ab(),
                pause(),
            ],
            Expect {
                notepad: Notepad::Text("ab"),
                ..Expect::remote(Remote::Nothing)
            },
        ),
        capture(
            "capture focus: client focused",
            mode,
            vec![Act::ClearNotepad, Act::Focus(Window::Client), ab(), pause()],
            Expect {
                notepad: Notepad::Empty,
                ..Expect::remote(Remote::Exactly(names(&["A", "B"])))
            },
        ),
    ]
}

pub fn capture_follows_hover() -> Vec<Case> {
    let mode: &[&str] = &["--grab-follows", "hover"];
    vec![
        capture(
            "capture hover: over the client, notepad focused",
            mode,
            vec![
                Act::ClearNotepad,
                Act::Focus(Window::Notepad),
                Act::Pointer(Place::OverClient),
                ab(),
                pause(),
            ],
            Expect {
                notepad: Notepad::Empty,
                ..Expect::remote(Remote::Exactly(names(&["A", "B"])))
            },
        ),
        capture(
            "capture hover: pointer off, then the probe",
            mode,
            vec![
                Act::ClearNotepad,
                Act::Focus(Window::Notepad),
                Act::Pointer(Place::OffClient),
                ab(),
                pause(),
                Act::Pointer(Place::OverClient),
                Act::Keys(taps(&["F10"])),
                pause(),
            ],
            Expect {
                notepad: Notepad::Text("ab"),
                ..Expect::remote(Remote::Exactly(names(&["probe:none"])))
            },
        ),
        capture(
            "capture hover: after the release chord, ungrabbed until leave and enter",
            mode,
            vec![
                Act::ClearNotepad,
                Act::Focus(Window::Notepad),
                Act::Pointer(Place::OverClient),
                Act::Keys(chord("Ctrl+Alt+Home")),
                pause(),
                ab(),
                pause(),
                Act::Pointer(Place::OffClient),
                Act::Pointer(Place::OverClient),
                ab(),
                pause(),
            ],
            // The first `ab` is Notepad's, the second the remote machine's: the grab
            // is back after the pointer left and came in. The remote machine has no
            // CTRL or ALT bind (a release bind is shadowed while another
            // modifier is held), so Ctrl (29) and Alt (56) reaching it show
            // in the raw key stream, and Home (102) never does.
            Expect {
                notepad: Notepad::Text("ab"),
                client: ClientExpect {
                    grab: Some(true),
                    ..ClientExpect::default()
                },
                keys: KeyExpect {
                    need: vec![29, 56],
                    need_up: vec![29, 56],
                    never: vec![102],
                    ..KeyExpect::default()
                },
                ..Expect::remote(Remote::Exactly(names(&["A", "B"])))
            },
        ),
    ]
}

/// The release chords: the default, and Shift+Esc as an option.
pub fn release_chords() -> Vec<Case> {
    vec![
        plain(
            "release Ctrl+Alt+Home",
            vec![
                Act::Focus(Window::Client),
                Act::Keys(chord("Ctrl+Alt+Home")),
                pause(),
            ],
            Expect {
                client: ClientExpect {
                    grab: Some(false),
                    ..ClientExpect::default()
                },
                // The remote machine's CTRL and ALT release binds cannot show a
                // modifier let go while another is held, so the raw key
                // stream is the evidence: Ctrl (29) and Alt (56) go down
                // and Home (102) never does.
                keys: KeyExpect {
                    need: vec![29, 56],
                    need_up: vec![29, 56],
                    never: vec![102],
                    ..KeyExpect::default()
                },
                ..Expect::remote(Remote::InOrder {
                    entries: Vec::new(),
                    never: names(&["Home", "CTRL+ALT+Home"]),
                })
            },
        ),
        // Focus mode: the client stays focused after the chord but is no
        // longer grabbed; a click on the picture takes the keyboard back.
        // `C` has no bind, so the raw key stream shows whether it reached the
        // remote machine (it must not); `A` and `B` after the click must.
        //
        // The case names the focus mode so the client starts afresh: the case
        // before it released the grab with the same chord, and focusing a
        // window that already has the focus does not grab again, so on that
        // client this chord would reach Windows, not the hook's release
        // (and Ctrl and Alt would never be forwarded).
        Case {
            flags: flags(&["--grab-follows", "focus"]),
            ..plain(
                "release Ctrl+Alt+Home, then a click grabs again",
                vec![
                    Act::Focus(Window::Client),
                    Act::Keys(chord("Ctrl+Alt+Home")),
                    pause(),
                    Act::Keys(taps(&["C"])),
                    pause(),
                    Act::Click {
                        remote: (20.0, 20.0),
                    },
                    pause(),
                    ab(),
                    pause(),
                ],
                Expect {
                    client: ClientExpect {
                        grab: Some(true),
                        ..ClientExpect::default()
                    },
                    keys: KeyExpect {
                        need: vec![29, 56],
                        need_up: vec![29, 56],
                        never: vec![102, 46],
                        ..KeyExpect::default()
                    },
                    ..Expect::remote(Remote::InOrder {
                        entries: names(&["A", "B"]),
                        never: names(&["Home", "CTRL+ALT+Home"]),
                    })
                },
            )
        },
        Case {
            name: "release Shift+Esc".to_owned(),
            risky: false,
            flags: flags(&["--release-chord", "shift+esc"]),
            acts: vec![
                Act::Focus(Window::Client),
                Act::Keys(chord("Shift+Escape")),
                pause(),
            ],
            expect: Expect {
                client: ClientExpect {
                    grab: Some(false),
                    ..ClientExpect::default()
                },
                // Shift (42) goes down; Esc (1) never does.
                keys: KeyExpect {
                    need: vec![42],
                    need_up: vec![42],
                    never: vec![1],
                    ..KeyExpect::default()
                },
                ..Expect::remote(Remote::InOrder {
                    entries: names(&["SHIFT"]),
                    never: names(&["Escape", "SHIFT+Escape"]),
                })
            },
        },
    ]
}

/// The longest a low-level hook callback may take before Windows gives up
/// on it (the registry's `LowLevelHooksTimeout` is capped at one second on
/// current Windows 10 and 11).
const HOOK_TIMEOUT_CAP_MS: u64 = 1000;

/// The stall the client is asked for: well past the cap, so Windows drops
/// the hook.
pub const HOOK_STALL_MS: u64 = HOOK_TIMEOUT_CAP_MS * 5 / 2;
const _: () = assert!(
    HOOK_STALL_MS > HOOK_TIMEOUT_CAP_MS,
    "the stall must outlast the longest hook timeout"
);

/// A slow hook callback (the stall is on `F24` only): afterwards the
/// chords still reach the remote machine, and the client's stats show the hook was
/// found silent and put back (otherwise the stall did not bite and the case
/// proves nothing). A second case holds Ctrl across the stall and lets it go
/// after the hook is back: no Ctrl may stay down in Windows (needs a live run).
pub fn hook_stall() -> Vec<Case> {
    vec![
        Case {
            name: "hook stall then a chord".to_owned(),
            risky: false,
            flags: flags(&["--hook-stall-test-ms", &HOOK_STALL_MS.to_string()]),
            acts: vec![
                Act::Focus(Window::Client),
                Act::Keys(taps(&["F24"])),
                Act::Pause(Duration::from_millis(HOOK_STALL_MS + 2500)),
                // Ctrl+Q does nothing on Windows if the hook were still down.
                Act::Keys(chord("Ctrl+Q")),
                pause(),
            ],
            expect: Expect {
                client: ClientExpect {
                    min_hook_reinstalls: 1,
                    ..ClientExpect::default()
                },
                keys: KeyExpect {
                    // The stall key, F24, as the client maps its scancode.
                    extra: vec![85],
                    ..KeyExpect::default()
                },
                ..Expect::remote(Remote::Exactly(names(&["CTRL+Q"])))
            },
        },
        held_across_stall(),
    ]
}

/// Ctrl goes down together with the stall key, so Windows may see its Down
/// while no hook is looking, and comes up after the hook is back. Then the
/// release chord gives the keyboard to Windows and a plain `A` goes to
/// Notepad. A Ctrl that stayed down in Windows would make that Ctrl+A (select
/// all), and Notepad would stay empty. It needs a live run: the stall bites
/// only when Windows drops the hook at that moment, and the case says nothing
/// about the remote keys beyond the ones it sent.
fn held_across_stall() -> Case {
    let (ctrl, f24) = (
        scan_of("Ctrl").expect("known key"),
        scan_of("F24").expect("known key"),
    );
    Case {
        name: "hook stall with Ctrl held, then a plain key on Windows".to_owned(),
        risky: false,
        flags: flags(&["--hook-stall-test-ms", &HOOK_STALL_MS.to_string()]),
        acts: vec![
            Act::ClearNotepad,
            Act::Focus(Window::Client),
            Act::Keys(vec![
                (ctrl, Press::Down),
                (f24, Press::Down),
                (f24, Press::Up),
            ]),
            Act::Pause(Duration::from_millis(HOOK_STALL_MS + 2500)),
            Act::Keys(vec![(ctrl, Press::Up)]),
            pause(),
            Act::Keys(chord("Ctrl+Alt+Home")),
            pause(),
            Act::Focus(Window::Notepad),
            Act::Keys(taps(&["A"])),
            pause(),
        ],
        expect: Expect {
            // Ctrl (29), Alt (56) and the stall key (85) may reach the remote
            // machine; Home (102) never does.
            keys: KeyExpect {
                extra: vec![29, 56, 85],
                never: vec![102],
                ..KeyExpect::default()
            },
            notepad: Notepad::Text("a"),
            ..Expect::remote(Remote::InOrder {
                entries: Vec::new(),
                never: names(&["Home"]),
            })
        },
    }
}

/// The nine click targets in remote logical pixels for a remote output of
/// `size` logical pixels: the four corners inset 5, the centre, and four
/// fractional points.
pub fn click_targets(size: (u32, u32)) -> Vec<(f64, f64)> {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    vec![
        (5.0, 5.0),
        (w - 5.0, 5.0),
        (5.0, h - 5.0),
        (w - 5.0, h - 5.0),
        (w / 2.0, h / 2.0),
        (w / 3.0, h / 3.0),
        (w * 2.0 / 3.0, h / 3.0),
        (w / 3.0, h * 2.0 / 3.0),
        (w * 2.0 / 3.0, h * 2.0 / 3.0),
    ]
}

/// A click at each target, one case each, checked to 1 remote pixel.
pub fn clicks(size: (u32, u32)) -> Vec<Case> {
    click_targets(size)
        .into_iter()
        .enumerate()
        .map(|(i, target)| {
            plain(
                &format!("click {} at {:.1},{:.1}", i + 1, target.0, target.1),
                vec![
                    Act::Focus(Window::Client),
                    Act::Click { remote: target },
                    pause(),
                ],
                Expect::remote(Remote::Clicks {
                    targets: vec![target],
                    tolerance: 1.0,
                }),
            )
        })
        .collect()
}

/// The wait after a wheel event that makes the client send a step.
/// Hyprland skips a scroll bind whose event comes less than
/// `binds:scroll_event_delay` (300 ms by default) after the previous scroll
/// event, and a skipped event restarts the wait, so steps sent closer than
/// that reach the remote log as one.
const WHEEL_SPACING: Duration = Duration::from_millis(400);

/// Whole notches, which reach the remote as wheel steps, and a smooth
/// stream. Every step the client sends is followed by [`WHEEL_SPACING`].
pub fn wheel() -> Vec<Case> {
    let event = |wheel, delta| Act::WheelEvent { wheel, delta };
    let events =
        |wheel, delta, times: usize| (0..times).map(|_| event(wheel, delta)).collect::<Vec<_>>();
    // One event that is a whole notch, then the wait.
    let notches = |wheel, delta, times: usize| {
        (0..times)
            .flat_map(|_| [event(wheel, delta), Act::Pause(WHEEL_SPACING)])
            .collect::<Vec<_>>()
    };
    let with_pause = |mut acts: Vec<Act>| {
        acts.insert(0, Act::Focus(Window::Client));
        acts.push(pause());
        acts
    };
    vec![
        plain(
            "wheel 3 notches down then 3 up",
            with_pause(
                [
                    notches(Wheel::Vertical, -120, 3),
                    notches(Wheel::Vertical, 120, 3),
                ]
                .concat(),
            ),
            Expect::remote(Remote::Exactly(names(&[
                "wheel:down",
                "wheel:down",
                "wheel:down",
                "wheel:up",
                "wheel:up",
                "wheel:up",
            ]))),
        ),
        // Smaller than a notch: a smooth scroll. The client sends it as a
        // finger scroll, but the server sends the source before the axis and
        // Hyprland's virtual pointer then makes it a wheel event, so the
        // down bind fires once per `binds:scroll_event_delay` (once or twice
        // here, by timing). It never fires the other way.
        plain(
            "wheel 30 smooth events of 8 scroll down",
            with_pause(events(Wheel::Vertical, -8, 30)),
            Expect::remote(Remote::InOrder {
                entries: names(&["wheel:down"]),
                never: names(&["wheel:up"]),
            }),
        ),
        // Horizontal keeps its sign: right is positive on both sides.
        plain(
            "wheel horizontal one notch right then left",
            with_pause(
                [
                    notches(Wheel::Horizontal, 120, 1),
                    notches(Wheel::Horizontal, -120, 1),
                ]
                .concat(),
            ),
            Expect::remote(Remote::Exactly(names(&["wheel:right", "wheel:left"]))),
        ),
    ]
}

/// Right, middle, back and forward with the default `--side-buttons`
/// (upstream: back is 278 `BTN_BACK`, forward 277 `BTN_FORWARD`), then the
/// two thumb buttons again with `--side-buttons linux-mouse` (275 `BTN_SIDE`,
/// 276 `BTN_EXTRA`).
pub fn buttons() -> Vec<Case> {
    let case = |side: Side, bind: &str, extra: &[&str]| {
        let name = if extra.is_empty() {
            format!("button {side:?}")
        } else {
            format!("button {side:?} as a Linux mouse")
        };
        Case {
            flags: flags(extra),
            ..plain(
                &name,
                vec![Act::Focus(Window::Client), Act::Button(side), pause()],
                Expect::remote(Remote::Exactly(vec![bind.to_owned()])),
            )
        }
    };
    let linux: &[&str] = &["--side-buttons", "linux-mouse"];
    vec![
        case(Side::Right, "mouse:273", &[]),
        case(Side::Middle, "mouse:274", &[]),
        case(Side::Back, "mouse:278", &[]),
        case(Side::Forward, "mouse:277", &[]),
        case(Side::Back, "mouse:275", linux),
        case(Side::Forward, "mouse:276", linux),
    ]
}

#[cfg(test)]
mod tests;
