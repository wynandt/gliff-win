# On-video statistics

The graph button turns a live readout on and off without opening log files.

## Sub-features

- STATS-1: default off; a graph click enables it and a second click disables it.
- STATS-2: successful-immediate decode FPS, payload Mbit/s, matching decode
  mean, pipeline and dimensions.
- STATS-3: idle refresh, narrow windows, 100, 125, 150 and 200% DPI, and
  fullscreen.
- STATS-4: the visible interval matches the shown session's JSON sample.

## How to get to it (user POV)

Click the graph beside fullscreen. In fullscreen first reveal the strip at
the top edge. The selection is window-wide, unsaved, and follows the active
tab.

## Driving it with the native client

Preconditions: parent skill launch and doctor passed; moving remote source.

- Toggle: record the default screen, a graph click and the visible readout,
  then a second click and its absence. Close and relaunch in the same
  config; the default is still off.
- Transparency: use recognizably changing video beneath the readout. The
  black background passes 40% of the video and solid white glyphs stay
  opaque. Toggle repeatedly; later video frames and pixels outside the
  readout must not become dim or translucent. This checks native
  blend-state isolation.
- Readout: enable it, keep a screenshot and the contemporaneous JSON;
  compare rounded fields with `sessions`' shown `stream` object and record
  `sequence`.
- Idle: stop remote damage; after two heartbeat intervals the rates reach
  zero.
- Geometry: resize narrow, change each listed DPI, reveal and hide the
  fullscreen strip; record legible bounded text and an unchanged remote
  picture mapping.
- Recovery: only with approved native device-loss tooling, recreate the
  graphics device and verify the selected readout returns. Otherwise report
  blocked.

Host companion: `scripts/verify-stats`; it does not exercise the native
toggle. Session identity across tabs is also covered by
`cargo test -p gwin-verify --test multi_session --locked`.

## Gotchas

Visible `fps` and `decode_ms` count successful immediate decode outputs, not
packets, idle drains, or actual presentations. JSON `received_fps` and
`decode_call_ms` preserve packet rate and the all-call mean. Backend timings
need not equal Linux numerically. Payload excludes SSH, protocol and
non-video traffic. Dimensions are remote stream and view dimensions, not
client size. Extremely short windows clip lines to the available height.
No Windows run or device-loss tool was available when this map was written;
these journeys have no claimed native proof. This expansion did not add
one. Fullscreen itself is [Fullscreen](fullscreen.md).
