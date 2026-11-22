# Settings

A flag beats `config.toml`, which beats the built-in default. There is no
settings window. Choices that apply to one machine live in that tab's
Connection settings menu.

## Sub-features

- SET-1: `--print-settings` prints each value and where it came from
  (`flag`, `file`, `machine` or `default`) and exits without starting a
  session; a chord that collides with another exits 2.
- SET-2: `recent-hosts.toml` keeps valid targets in order and drops a
  hostile line; at most ten hosts.
- SET-3: right-click a tab, Connection settings: Show as, Share clipboard,
  Sharp colour, GPU decode, and Send Win+L to this machine. A flag that
  applies to every machine greys the row and names the reason.
- SET-4: checks use their own config folder and do not write the registry
  for settings or history.

## How to get to it (user POV)

Pass flags on the command line, edit `config.toml` in the config folder, or
use Connection settings on a tab. See the README ("Options") and
`docs/settings.md`.

## Driving it with the native client

Preconditions: parent skill launch with an isolated `--config-dir`. Never
point a check at the person's real config folder.

- `--print-settings` with a temporary folder: a file value, then a flag
  that overrides it, then a colliding chord (exit 2).
- Connection settings: change Show as, Share clipboard, Sharp colour, GPU
  decode and Send Win+L; record what is saved and, for Reconnect-class
  rows, that *Reconnect now to apply* appears when a session is live.
  Start with `--headless` or `--no-clipboard` and check the matching row is
  greyed.

Host companions (not native proof):

- `gwin-verify config`: writes a scratch `config.toml` and
  `recent-hosts.toml`, runs `--print-settings` three times, and judges the
  JSON. It does not connect. Off Windows the client still runs
  `--print-settings`. See `docs/gwin-verify.md`.
- Unit tests of `settings`, `config`, `history` and `startup` in
  `crates/gliff-windows`. One `config` test makes the file unreadable after
  the save's first read: the save fails and the bytes stay. A native save of
  a read-denied file on a Windows machine was not run.

## Gotchas

The settings window described in `docs/settings.md` is not built. Native
Connection settings menu journeys have no recorded hardware gate. This map
expansion did not open that menu on Windows. `keep_in_tray` is read and
shown in `--print-settings` but nothing uses it until a tray exists.
