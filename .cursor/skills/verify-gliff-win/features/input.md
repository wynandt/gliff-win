# Input through the readout

The readout must not become a new input surface or disrupt remote grab.

## Sub-features

- INPUT-1: graph toggles only on matching left-button release, not cancellation.
- INPUT-2: pointer motion, clicks and scrolling reach the remote beneath text.
- INPUT-3: hover grab, focus, release chord and fullscreen strip remain usable.

## How to get to it (user POV)

Use the graph with a remote session active, then move and click over its text.
The strip itself remains local controls; the readout over video does not.

## Driving it with the native client

Preconditions: parent launch/doctor, authorized disposable Windows session,
remote application that visibly records pointer and scroll actions.

- Graph: press then slide off; cancel a held press; right-click; none toggles.
  A clean left press/release does. Repeat with overflow and address bar open.
- Overlay: place a remote clickable/scrollable control beneath the readout;
  capture motion/click/scroll and their remote effects, both enabled and disabled.
- Grab: test hover-grab and click-grab settings, focus away/back, release chord,
  and fullscreen reveal/hide. Capture state and remote input effects.
- Regression: use the documented `gwin-verify.exe keys` and `pointer` harnesses
  in their own sessions with the required remote recorder. No silent setup.
- Host actions: `cargo test -p gliff-windows --lib --locked strip`.

## Gotchas

No new HWND is created, but that design fact is not runtime input proof. The
native input journeys remain unexecuted. Existing harnesses send global input;
never run them on an occupied desktop or mistake their fake judges for UI proof.
