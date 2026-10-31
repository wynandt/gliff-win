# `scripts/remote/gwin-keys`

It runs on the Hyprland remote machine (0.55 or later, lua config), only as a
maintainer-approved step. It is tested on the build machine against
`scripts/fake-bin/hyprctl` (`crates/gwin-verify/tests/keys_script.rs`). It uses
`hyprctl eval 'hl.define_submap(...)'` with `hl.bind(...)` callbacks that only
`exec` (Hyprland's wiki warns that a callback must not block), as
`scripts/remote/gwin-pattern` uses `hyprctl dispatch`.

| Command | Does |
|---|---|
| `gwin-keys arm [--minutes 15]` | Sends one lua chunk that defines a submap `gwin-test-<epoch>` and enters it. In it, each row of `super-chords.tsv` has a bind that logs the row's chord name (`SUPER alone` is a release bind on `SUPER + SUPER_L`). Also: the system chords `ALT+Tab`, `ALT+F4`, `ALT+Space`, `ALT+Escape`, `CTRL+Escape`, `CTRL+SHIFT+Escape`, `Print`; a release bind `SHIFT` (Shift ×5 and the release-chord check); `Home`, `CTRL+ALT+Home`, `Escape`, `SHIFT+Escape` (which must never fire for a release chord); `CTRL+Q`; `SUPER+L`; plain `A` and `B`; the F10 probes `probe:none/shift/ctrl/alt/super`; `mouse:272..278` and the wheel binds `wheel:up/down/left/right`. There is **no catch-all bind**. Then it adds a global key listener, `hl.on("input.keyboard.key", ...)`, that runs `gwin-keys _key down|up <evdev>` (key repeat is ignored), and starts `wev -f wl_keyboard:key`, read by `gwin-keys _typed`. `SUPER+SHIFT+F12` and the timer disarm. A second `arm` while armed is refused. |
| `gwin-keys log [--since N]` | Prints the log lines with number above N (gwin-verify reads it over ssh). |
| `gwin-keys disarm` | Leaves the submap, takes the key listener off, stops `wev`, takes the binds out again (best effort; they are inert outside the submap), then prints the log and deletes it. |
| `gwin-keys check` | Exits 0 and prints `check: the submap <name> is active` when a state file exists and `hyprctl submap` returns that submap; otherwise prints what it found and exits 1 (not armed, or the submap was left). gwin-verify runs it before each client start and each lock-step press. |
| `gwin-keys focus [--tries 10]` | Dispatches `hl.dsp.focus({ window = "class:wev" })` and checks `hyprctl -j activewindow` until its class is `wev` (0.2 s apart). Prints `focus: wev is the active window` and exits 0, or prints `focus FAILED` with the diagnosis and exits 1. |
| `gwin-keys diag` | Prints `hyprctl -j` of `activewindow`, `activeworkspace`, `workspaces`, `monitors all` and `clients`, the `binds:scroll_event_delay` option and the state file. |
| `gwin-keys selftest` | Checks the lua it would send (bind count, the disarm bind, an unbind for every bind). Needs no `hyprctl`. |

**Over ssh.** `RUN_DIR` is `$XDG_RUNTIME_DIR`, or `/run/user/<uid>` when that is unset (no desktop session over ssh), and when `HYPRLAND_INSTANCE_SIGNATURE` is not set the newest directory under `$RUN_DIR/hypr` is used, so `hyprctl` finds the running Hyprland.

**The log is a record of every key pressed on the remote machine while armed.** It is private to the user (mode 0600), `disarm` deletes it after printing it (keep the printed copy), and the timer deletes it when it ends the run. `SUPER+SHIFT+F12` stops the recording but leaves the log until `disarm` prints and deletes it. `selftest` runs `disarm` against a stand-in `hyprctl` to check this.

**Log format.** `$XDG_RUNTIME_DIR/gwin-keys.log` (`GWIN_KEYS_LOG` overrides),
one line per event: `ts<TAB>n<TAB>what`. `what` is a bind name, `key:down:<evdev>` or `key:up:<evdev>` (the evdev code, from the listener, only while armed), or `typed:<keysym>` (what a focused remote machine client typed for a key press under the keymap the client sent; modifiers and releases are skipped; needs `wev`, and arm warns if it is missing). `n` is the line number, so a
reader asks for what is new by number. A mouse line is
`mouse:272 x,y`: the cursor relative to the `gliff-*` output, in that output's
logical pixels (the remote's pixels). The chord table is read from
`docs/phase4/super-chords.tsv` next to the script's repo, or `GWIN_KEYS_CHORDS`,
or `~/gwin-p4/super-chords.tsv`.

Why no catch-all: Hyprland's modifier mask is the state *before* the key, so a
lone modifier press matches an empty-mask catch-all and logged `other` ahead of
every chord, and a catch-all also fires a second time for a plain-key bind.
Why a listener: the `CTRL` and `ALT` release binds are not run while another
modifier is still held, so a raw key stream is the only evidence for them.
Binds follow the remote machine's own layout, not the keymap the client sends, so the
layout check reads `typed:` lines.

**Focus.** The layout cases read what `wev` types, and `wev` only gets keys
while it is the active window. Hyprland gives a new headless output (the gliff
session) an empty workspace and the pointer moving onto it moves the focus
there, so the active window is none and a `wev` focused beforehand is lost
once the session starts. `gwin-verify keys --remote-keys <cmd>` therefore runs
`<cmd> focus` over ssh right before each layout case's keys (after the client
has the focus and its pointer has settled) and stops that case with the output
when `wev` does not get the focus. It also saves `<cmd> diag` before the first
client starts and after its first picture, as `remote-diag-*.txt` in the log
directory.

**Mouse buttons.** Linux numbers: 272 left, 273 right, 274 middle, 275
`BTN_SIDE`, 276 `BTN_EXTRA`, 277 `BTN_FORWARD`, 278 `BTN_BACK`. The client's
default `--side-buttons upstream` sends Back as 278 and Forward as 277;
`linux-mouse` sends 275 and 276.

**Wheel.** Hyprland skips a scroll bind whose event comes less than
`binds:scroll_event_delay` ms (300 by default) after the previous scroll event,
and a skipped event restarts that wait. The wheel cases wait 400 ms after every
step the client sends; `diag` shows the option's value.

While it is armed, the remote machine's own keyboard is in the submap too, so nobody
should use the remote machine during a run. If a run is stuck, `hyprctl dispatch
'hl.dsp.submap("reset")'` from a tty leaves the submap.
