# Changelog

Notable changes to gliff-win, newest first. The section for a version is the
body of its GitHub release: the release workflow prints it with
`scripts/release-notes` and refuses a version that has no section. To cut a
release, see [CONTRIBUTING.md](CONTRIBUTING.md#releasing).

## [Unreleased]

- *GPU decode* is now on by default for every machine. If the GPU cannot
  decode, the client still falls back to the CPU (OpenH264) on its own, at
  the start or during a session. To decode on the CPU, untick *GPU decode* in
  a tab's *Connection settings* or set `gpu_decode = false` in that machine's
  table in `config.toml`. If the picture glitches, untick it and reconnect.
- A Win or Ctrl key could stay down after the keyboard hook was put back,
  most often with *Send Win+L to this machine* on: on the remote machine
  (Super held, so plain keys acted as shortcuts) or in Windows (Ctrl held
  after the keyboard came back). The client now files a Shift, Ctrl or Alt
  that reached the window as a key message under its left or right key, as
  the hook does, so its Up is not lost; tells the remote machine at every hook
  reinstall that each Win, Ctrl, Alt and Shift key the user does not hold is
  up, including one it still counted as down; and forgets what an old hook
  saw when a new one comes back after a time with no grab. The
  `keyboard hook silent` log line now says whether the key that showed it was
  a Win, Ctrl, Alt or Shift key, and Down or Up, and still names no other key.
  ([#35](https://github.com/wynandt/gliff-win/issues/35))
- With *Send Win+L to this machine* on, Win+L or another Win chord in a grab
  could leave Windows acting as if Win were held after the keyboard came back
  (a plain `F` opened Feedback Hub). Windows takes a Win key as down before
  the keyboard hook runs, and the grab kept the Up from it; the release at
  the grab end only looked at `GetAsyncKeyState`, which never showed that Win
  as down. When the hook keeps a Win key's Up for the remote machine, it now
  gives Windows an Up for that key as well (with the masked tap, so Start
  does not open), and the grab end, a hook reinstall and the exit release an
  owed Win key the user no longer holds. The reinstall log line names the
  Win keys still owed. ([#35](https://github.com/wynandt/gliff-win/issues/35))

## [0.2.1]

The first signed release: the programs are code-signed with Azure Artifact
Signing, so Windows shows a verified publisher. SmartScreen may still warn
until the signature builds reputation.

- *Send Win+L to this machine*, a switch per machine in a tab's *Connection
  settings*, off by default. With it on, a real Win+L goes to that machine:
  Windows lock shortcuts are off while the window shows that tab and has the
  keyboard, back at once when the switch goes off or another tab is shown,
  and 300 ms after the window loses the keyboard. It applies at once and is saved as
  `send_win_l` in `config.toml`. `--super-l registry` now turns it on for
  every machine, and `--super-l chord` or `off` turns it off for every
  machine. ([#21](https://github.com/wynandt/gliff-win/issues/21))
- A precision touchpad scrolls the remote smoothly, as on the Linux client.
  Before, small finger movements did nothing until they added up to a whole
  wheel notch, and then the remote jumped a step. A mouse wheel still scrolls
  one step per notch.
  ([#18](https://github.com/wynandt/gliff-win/issues/18))
- `gwin-verify gpu` has been removed. To check GPU decode, use
  `gwin-verify decode --decoder gpu` or `--decoder both`.
  ([#26](https://github.com/wynandt/gliff-win/issues/26))

## [0.2.1-rc.1]

First signed prerelease: the programs are now code-signed with Azure Artifact
Signing, so Windows shows a verified publisher. SmartScreen may still warn
until the signature builds reputation.

- *Send Win+L to this machine*, a switch per machine in a tab's *Connection
  settings*, off by default. With it on, a real Win+L goes to that machine:
  Windows lock shortcuts are off while the window shows that tab and has the
  keyboard, back at once when the switch goes off or another tab is shown,
  and 300 ms after the window loses the keyboard. It applies at once and is saved as
  `send_win_l` in `config.toml`. `--super-l registry` now turns it on for
  every machine, and `--super-l chord` or `off` turns it off for every
  machine. ([#21](https://github.com/wynandt/gliff-win/issues/21))
- A precision touchpad scrolls the remote smoothly, as on the Linux client.
  Before, small finger movements did nothing until they added up to a whole
  wheel notch, and then the remote jumped a step. A mouse wheel still scrolls
  one step per notch.
  ([#18](https://github.com/wynandt/gliff-win/issues/18))

## [0.2.0]

Decoding on the GPU, sharper coloured text, and a picture that keeps moving
while a menu is open or the window is dragged.

- GPU decode, a choice per machine in a tab's *Connection settings*, off by
  default. It decodes H.264 with the Windows decoder (Media Foundation) and
  Direct3D 11 video, shows the same picture pixel for pixel, and falls back to
  the CPU on its own, saying why in the log.
  ([#7](https://github.com/wynandt/gliff-win/issues/7))
- Sharp colour, a choice per machine, off by default. It asks the server for
  full-resolution colour (4:4:4, sent as two 4:2:0 streams), so coloured text
  stays crisp. The server decides: one that encodes on the CPU sends 4:4:4
  only when it runs with `--full-chroma`.
  ([#9](https://github.com/wynandt/gliff-win/issues/9))
- The picture keeps updating while a right-click menu or the window menu is
  open. Before, it froze until the menu closed.
  ([#11](https://github.com/wynandt/gliff-win/issues/11))
- The picture keeps the stream's full frame rate while the window is dragged
  or resized. Before, it dropped well below it.
  ([#14](https://github.com/wynandt/gliff-win/issues/14))

Both new choices apply the next time the tab connects, or at once with
*Reconnect now to apply*. [docs/settings.md](https://github.com/wynandt/gliff-win/blob/main/docs/settings.md)
has the details.

## [0.1.0]

First release: a Windows client for [Gliff](https://github.com/omacom/gliff)
that shows the Hyprland session of another machine in a normal, resizable
window over ssh.

- A tab for each machine, an address bar, and a panel that says in plain words
  why a connection failed.
- The remote's focused screen fitted into the window, a private screen that
  follows the window with `--headless`, or a named screen with `--output`.
  H.264 is decoded on the CPU and shown with Direct3D 11. `F11` toggles
  borderless fullscreen.
- Keyboard and mouse, including Super chords. The Windows layout is sent as an
  XKB keymap. `Ctrl+Alt+Home` gives the keyboard back to Windows.
- Clipboard in both directions for text, images and files. Remote copies stay
  out of Win+V history and cloud sync.
- ssh is the only transport and the only authentication. Works with the stock
  `gliff-server` v0.4.1 or later.

Not in 0.1: a system tray, a settings window, sound, and hardware decode. The
README's "Not in 0.1" and "What's missing" sections list them all.
