# Keyboard and chords

While the window has the keyboard, keys go to the remote machine, except
the local chords and the chord that gives the keyboard back to Windows.

## Sub-features

- KEYS-1: the keyboard follows the focused window (default) or the pointer;
  click the picture to take it; `Ctrl+Alt+Home` (default) gives it back; an
  optional timeout lets it go.
- KEYS-2: switch chords change tabs; `Ctrl+Alt+L` (default) sends Super+L;
  Win+L still locks Windows unless *Send Win+L to this machine* is on;
  `Win+Shift+S` stays on Windows by default.
- KEYS-3: Super chords reach Hyprland; when things settle, neither Windows
  nor the remote machine holds a key the user has let go of.
- KEYS-4: the client logs no keystrokes (counts, modifier names and the
  layout name only).

Pointer clicks and the wheel are [Input](input.md) and `gwin-verify pointer`.
Per-machine *Send Win+L to this machine* is also [Settings](settings.md).

## How to get to it (user POV)

Click the picture so the window has the keyboard, type, then press
`Ctrl+Alt+Home` to give it back. Chords are in the README ("Keyboard") and
can be changed in `config.toml` (`docs/settings.md`).

## Driving it with the native client

Preconditions: authorized disposable Windows session, a client built with
`--features test-hooks` for `gwin-verify keys`, and `gwin-keys` armed on the
remote machine for a bounded time. Do not touch the keyboard or mouse
during the harness. Read the safety rules in `docs/development.md` first.
If a lock-policy run is cut short, `gliff-windows.exe --restore-lock-policy`
puts the value back.

- Regression: `gwin-verify.exe keys` (chords, system keys, layouts, AltGr,
  capture, release, a slow hook), then `keys --only lock` (a person presses
  Win+L and unlocks once), then `keys --manual` (chords only a person can
  press). See `docs/gwin-verify.md`.
- Hand check: grab on focus and on hover, the release chord, a local chord
  (`Win+Shift+S` by default), and that the log names no key other than a
  modifier.

Host companions (not native proof):

- `formal/held-keys/run`: TLA+ model of holding and releasing keys. Run it
  when changing the hook, the grab, the key ledger, the local chords, or the
  release paths. See `formal/held-keys/README.md`.
- Unit tests of `grab_state`, `key_ledger`, `local_chords`, `chord_watch`,
  `altgr` and the other pure input modules in `crates/gliff-windows`.

## Gotchas

A released client rejects the hidden test flags, so `keys` needs a
`test-hooks` build. `keys --only lock` is the one step that changes a
Windows setting. The README records a hardware gate on an earlier build
(40 of 40 key checks, 9 of 9 Super+L lock steps, 8 of 8 manual chords; see
`docs/windows-quirks.md`). This map expansion did not re-run it. The model
check is not a live hook, IME, or pointer proof; its limits are in
`formal/held-keys/README.md`.
