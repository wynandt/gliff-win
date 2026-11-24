# Resize

When the user resizes the window, goes fullscreen or moves it to a screen
at another scale, the client asks the remote machine for an output of the
new size, and the picture follows without a stretched or stale frame.

## Sub-features

- RESIZE-1: a window resize sends a Resize with the new client size and
  scale; the remote side applies it and sends a new StreamConfig.
- RESIZE-2: requests in quick succession end at the last size the user
  chose, including a return to an earlier size before the newer one is
  applied (the bug behind issue 44).
- RESIZE-3: a remote output lost after a compositor reload is set again by
  the client's resync (the `reload` fault on the fake server).
- RESIZE-4: per-machine Own screen `Scale` (`Auto`, `125%`, `160%`, `200%`,
  saved as `scale` in `[overrides."user@host"]`) sends physical pixel size
  with the chosen scale at session start and when the window moves to
  another screen; picking a radio causes no live mid-session rescale on the
  same screen, and Mirror mode greys out the radios and ignores the setting.

Fullscreen geometry is [Fullscreen](fullscreen.md). The readout at narrow
sizes is [Stats](stats.md).

## How to get to it (user POV)

Drag a window edge, maximize or restore, or press the fullscreen button.
On a high-DPI screen, moving the window to a screen at another scale
changes the scale too.

## Driving it with gwin-native.ps1

Preconditions: the parent skill's loopback launch and a passing doctor.

- **Resize.** Run `gwin-native.ps1 resize -Run $run -Width 1024 -Height 640`.
  Pass: `remote_new` holds at least one `resize` line that differs from
  `remote_before`, and `evidence\resize-1024x640.png` shows the picture
  filling the window.
- **Back and forth.** Run `resize` to 1280x800, then at once to 1024x640.
  Pass: the last `resize` line in `input.jsonl` equals the last entry of
  the second call's `remote_new`, and no line after it has the first
  call's size (RESIZE-2).
- **Reload.** Launch with `-Fault reload`, then `resize` once. One second
  later the fake server stops sending frames; the client's resync must
  send a different Resize. Pass: `doctor` reports frames increasing again
  within 10 s and `input.jsonl` shows the nudged `resize` (RESIZE-3).

- **Own screen Scale (RESIZE-4).** In Own screen mode (`--headless` or
  `Show as -> Own screen`), pick each radio (`Auto`, `125%`, `160%`,
  `200%`) in Connection settings. Pass: picking a radio sends no live
  `resize` line on the same screen and saves `scale` in `config.toml`;
  reconnecting (or moving the window to another screen) sends `resize` with
  the physical client size and the chosen scale (`dpi / 96`, `1.25`, `1.6`,
  or `2.0`) and `StreamConfig` reports the matching `scale_milli`. In
  Mirror mode, the four Scale radios are greyed out and Mirror sessions
  ignore any saved `scale`.

Host companions (not native proof):

- `cargo test -p gwin-verify --test loopback --locked`: a Resize sent
  through the session driver reaches the remote side.
- `cargo test -p gwin-verify --test stream --test multi_session --locked`
  for the clamp and the reload resync.
- `gwin-verify present` on a real remote machine judges the picture after
  resize and fullscreen (`docs/gwin-verify.md`).

## Gotchas

`resize` sets the outer window size; the Resize carries the client area
in physical pixels, so the remote numbers are smaller than `-Width` and
`-Height` and scale with DPI. The fake server clamps to the client's
advertised maximum, as upstream does.

Windows run: the single **Resize** journey (RESIZE-1) passed under Windows
PowerShell 5.1 on one high-DPI laptop screen at 200% scale. A 1024x640
outer window gave a remote `resize` of 998x504 at scale 2.0. **Back and
forth** (RESIZE-2), **Reload** (RESIZE-3), and **Own screen Scale**
(RESIZE-4) have not run on Windows yet.
