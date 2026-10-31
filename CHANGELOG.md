# Changelog

Notable changes to gliff-win, newest first. The section for a version is the
body of its GitHub release: the release workflow prints it with
`scripts/release-notes` and refuses a version that has no section. To cut a
release, see [CONTRIBUTING.md](CONTRIBUTING.md#releasing).

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
