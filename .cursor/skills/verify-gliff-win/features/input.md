# Input through the readout

The statistics readout must not become a new input surface or disrupt
remote grab. Product keyboard and chords are [Keyboard](keyboard.md).

## Sub-features

- INPUT-1: the graph toggles only on a matching left-button release, not
  on cancellation.
- INPUT-2: pointer motion, clicks and scrolling reach the remote machine
  beneath the readout text.
- INPUT-3: hover grab, focus, the release chord and the fullscreen strip
  remain usable with the readout on or off.

## How to get to it (user POV)

Use the graph with a remote session active, then move and click over its
text. The strip itself remains local controls; the readout over video does
not.

## Driving it with the native client

Preconditions: parent launch and doctor, authorized disposable Windows
session, and a remote application that visibly records pointer and scroll
actions.

- Graph: press then slide off; cancel a held press; right-click; none
  toggles. A clean left press/release does. Repeat with overflow and the
  address bar open.
- Overlay: place a remote clickable or scrollable control beneath the
  readout; capture motion, click and scroll and their remote effects, both
  with the readout on and off.
- Grab: hover-grab and click-grab, focus away and back, the release chord,
  and fullscreen reveal and hide. Capture state and remote input effects.
- Regression: `gwin-verify.exe pointer` (and `keys` for grab and the
  release chord) in their own sessions with the required remote recorder.
  No silent setup. See `docs/gwin-verify.md`.

Host companions (not native proof):

- `cargo test -p gliff-windows --lib --locked strip`.
- Pointer and button unit tests in `crates/gliff-windows`.

## Gotchas

No new HWND is created, but that design fact is not runtime input proof.
The native overlay journeys remain unexecuted. Existing harnesses send
global input; never run them on an occupied desktop or mistake their
judges for overlay proof. The README records a pointer hardware gate on an
earlier build (20 of 20 clicks at 200%; see `docs/windows-quirks.md`). This
map expansion did not re-run it or click the graph.
