# Tabs

Each machine has a tab. The strip shows the full name and a dot for the
session state. Several sessions can run in one window.

## Sub-features

- TABS-1: click a tab to show it; click `+` to open the address bar; when
  the tabs do not fit, a chevron opens a menu of them; drag a tab to move it.
  After a disconnect, a click only shows the tab until its old session has
  closed (at most about six seconds). Once the window is closing, a click
  starts no session.
- TABS-2: the dot is Stopped, Connecting, Live, Reconnecting or Failed;
  middle-click (or a click on the dot) stops a session or forgets a tab that
  has none.
- TABS-3: right-click a tab for Connect, Reconnect, Disconnect, Copy address,
  Connection settings and Forget. On a running or connecting tab, Reconnect
  stops the active session while keeping the tab open and starts a fresh
  session once the old session finishes closing; repeated clicks before it
  closes start one replacement session.
- TABS-4: `Ctrl+Alt+PageDown`, `Ctrl+Alt+PageUp` and `Ctrl+Alt+1` through `9`
  (the defaults) select the expected tab; hidden sessions keep acking;
  reconnect starts a fresh session.

Connection settings themselves are [Settings](settings.md). Independent
stream readouts after a switch are [Stats](stats.md).

## How to get to it (user POV)

Open more than one machine from the address bar or the command line, then
click tab bodies or use the switch chords. See the README ("The tab strip").

## Driving it with the native client

Preconditions: isolated native launch with at least two authorized distinct
targets, recognizably different sizes or pictures.

- Click, drag, `+`, chevron and the tab menu: record which tab is shown, the
  dot, and that only the selected source is drawn.
- Click while closing (*loopback*, `GWIN_FAKE_SERVER=linger`): disconnect a
  live tab and click it at once. Record that the click starts no second
  session and no second remote hello, and that a click about 8 s after the
  disconnect connects. Passed on Windows for PR #83. Once the window is
  closing, a click starts no session ([#86](https://github.com/wynandt/gliff-win/issues/86)).
  The host test covers that. The loopback journey for a click during
  whole-window close was not run for this change.
- Chords: switch by next, previous and jump; record the shown session after
  each. Reconnect from the tab menu on a running tab and confirm the client
  log records the old session ending and a fresh session starting; no stale
  session should stay on screen. Host tests cover running-tab reconnect,
  idempotent double-click, and closing while restart is pending; the native
  Windows run for this fix has not yet been executed.
- Regression: `gwin-verify.exe tabs` with three authorized targets. It uses
  a fresh config, sends real switch chords, and takes over the keyboard and
  mouse after a countdown. Never unattended. See `docs/gwin-verify.md`.

Host companions (not native proof):

- `gwin-verify strip-shot --out DIR`: strip scenes at 100, 125, 150 and 200%
  (104 pictures), including dots, overflow, dragging and the address bar. It
  opens no window. Native popup menus and the EDIT control are not in the
  pictures.
- `cargo test -p gwin-verify --test multi_session --locked`.
- Unit tests of `tabs` and `strip` in `crates/gliff-windows`.

## Gotchas

`gwin-verify tabs` and strip-shot do not exercise the tab menu, the chevron
menu, drag-to-reorder, or middle-click. The tab strip and address bar have
no recorded hardware gate of their own (README, "Verification"). This map
expansion did not run the native tabs harness or click the strip. Host
multi-session tests use a fake server, not Windows tab clicks.
