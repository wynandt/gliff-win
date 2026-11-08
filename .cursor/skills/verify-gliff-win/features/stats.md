# On-video statistics

The graph button turns a live readout on and off without opening log files.

## Sub-features

- STATS-1: default off; graph click enables and a second click disables.
- STATS-2: successful-immediate decode FPS, payload Mbit/s, matching decode mean,
  pipeline and dimensions.
- STATS-3: idle refresh, narrow windows, 100/125/150/200% DPI and fullscreen.
- STATS-4: the visible interval matches the shown session's JSON sample.

## How to get to it (user POV)

Click the graph beside fullscreen. In fullscreen first reveal the strip at the
top edge. The selection is window-wide, unsaved, and follows the active tab.

## Driving it with the native client

Preconditions: parent skill launch/doctor passed; moving remote source.

- Toggle: record the default screen, graph click and visible readout, then second
  click and absence. Close and relaunch in the same config; default is still off.
- Transparency: use recognizably changing video beneath the readout. Verify
  the black background passes 40% of the video and solid white glyphs stay
  opaque. Toggle repeatedly; later video frames and pixels outside the readout
  must not become dim or translucent. This checks native blend-state isolation.
- Readout: enable it, preserve a screenshot and contemporaneous JSON; compare
  rounded fields with `sessions`' shown `stream` object and record `sequence`.
- Idle: stop remote damage; after two heartbeat intervals rates reach zero.
- Geometry: resize narrow, change each listed DPI, reveal/hide the fullscreen
  strip; record legible bounded text and unchanged remote picture mapping.
- Recovery: only with approved native device-loss tooling, recreate the graphics
  device and verify the selected readout returns. Otherwise report blocked.

Host companion: `scripts/verify-stats`; it does not exercise the native toggle.

## Gotchas

Visible `fps` and `decode_ms` count successful immediate decode outputs, not
packets, idle drains, or actual presentations. JSON `received_fps` and
`decode_call_ms` preserve packet rate and the all-call mean. Backend timings
need not equal Linux numerically. Payload excludes SSH/protocol and non-video
traffic. Dimensions are remote stream/view dimensions, not client size. Extremely short windows clip
lines to available height. No Windows run or device-loss tool was available
when this map was created; these journeys have no claimed native proof.
