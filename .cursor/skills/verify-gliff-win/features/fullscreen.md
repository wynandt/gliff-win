# Fullscreen

The strip's fullscreen button, the window menu or `F11` (while Windows has
the keyboard) toggles borderless fullscreen. Fullscreen hides the strip;
moving the pointer to the top edge slides it in, and moving away slides it
out.

## Sub-features

- FULL-1: enter and leave borderless fullscreen; the remote picture mapping
  stays correct.
- FULL-2: in fullscreen, the strip reveals at the top edge and hides when
  the pointer leaves it.
- FULL-3: `F11` toggles fullscreen only while Windows has the keyboard, not
  while the remote machine has it.

## How to get to it (user POV)

Connect, then use the fullscreen button on the strip, the window menu, or
`F11` after giving the keyboard back (`Ctrl+Alt+Home` by default). See the
README ("What you get" and "The tab strip").

## Driving it with the native client

Preconditions: parent skill launch and doctor with a streaming session.

- Toggle by the button, by the window menu and by `F11` (keyboard released,
  then grabbed); record geometry and that the picture still maps to the
  remote screen.
- In fullscreen, reveal and hide the strip at the top edge; local strip
  controls still work; the picture is unchanged when the strip hides.

Host companions (not native proof):

- `gwin-verify strip-shot` includes the `hover-fullscreen` scene.
- Unit tests of `fullscreen` and `strip` reveal in `crates/gliff-windows`.
  `the_reveal_distances_use_the_strip_scale` checks that a dpi below 48, and a
  dpi that does not fit in a signed 32-bit integer, use one scale for the
  strip and the reveal edge. A Windows session at a dpi below 48 was not run.
- `cargo test -p gwin-verify --bin gwin-verify present_check`: the present
  judge and step sequence against a fake harness on Linux.

The native regression check is `gwin-verify.exe present`, which has
windowed and fullscreen sections and takes over input. A passing hardware
run is not in the repository records (`docs/gwin-verify.md` notes that the
checked-in present fixture is synthetic). This map expansion did not run
`present` or toggle fullscreen on Windows.
