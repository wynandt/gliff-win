# Failure panel

When a connection fails, a panel names the problem in plain words and offers
the buttons that fit.

## Sub-features

- PANEL-1: the wording matches the failure, with the buttons that make sense
  (Try again, Check again, Edit address, Details, Close).
- PANEL-2: Details shows ssh's own text; the full text stays in
  `ssh-stderr-<n>.log` in the log folder; a key problem links to
  `docs/ssh-keys.md`.
- PANEL-3: the layout holds at 100, 125, 150 and 200% (no overlap, buttons in
  their colours, words drawn).

## How to get to it (user POV)

Fail a connection (a missing key, a refused host key, a server that is not
on the remote `PATH`). The Failed tab shows the panel. See the README
("When a connection fails") and `docs/ssh-keys.md`.

## Driving it with the native client

Preconditions: parent skill launch with an isolated config. Do not weaken
ssh or add a host key in order to manufacture a case.

- Trigger one failure at a time; record the panel wording, the buttons, and
  the matching `ssh-stderr-<n>.log`.
- Press Try again, Edit address, Details and Close; record what each does.
- For a key failure, follow the panel's link and check it points at the
  matching step in `docs/ssh-keys.md`.

Host companions (not native proof):

- `gwin-verify panel-shot --out DIR`: draws each of the thirteen failures at
  100, 125, 150 and 200% (52 pictures) and judges them. It opens no window
  and touches no input. See `docs/gwin-verify.md`.
- The failure-panel unit tests in `crates/gliff-windows`.

## Gotchas

Picture judges prove layout and wording of the drawn panel, not that a real
ssh failure reaches the same panel or that a button works. The tab strip,
address bar and failure panel have no recorded hardware gate of their own
(README, "Verification"). This map expansion did not click the native
panel.
