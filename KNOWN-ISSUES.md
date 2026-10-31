# Known issues

What gliff-win does not do yet, what has not been tested, and what is still
open. Each item links to the page with the details. If you hit something that
is not listed here, please open an issue with a way to reproduce it.

## Not built yet

- **Sound.** No audio and no microphone. Gliff's protocol has none either.
- **Sharp coloured text is off by default** ([#9](https://github.com/wynandt/gliff-win/issues/9)). Without *Sharp
  colour* the client asks for a 4:2:0 stream, so coloured text is a little
  soft. Turn it on per machine in the tab's *Connection settings*. A server
  that encodes on the CPU still sends 4:2:0 unless it runs with
  `--full-chroma`; see [docs/settings.md](docs/settings.md#sharp-colour).
- **GPU video decoding.** The client decodes H.264 on the CPU. With *Sharp
  colour* it decodes two streams, so the decode work at least doubles.
- **System tray and start with Windows.**
- **A settings window.** The three choices for one machine (Show as, Share
  clipboard and Sharp colour) are in a tab's right-click menu. Everything else is a flag or a
  key in `config.toml`; see [docs/settings.md](docs/settings.md).
- **A light theme.** The tab strip and the failure panel are always dark.
- **Progress and Cancel for big pastes.** Progress is written to the log only.
- **Virtual files on the clipboard** (zip contents from Explorer, mail
  attachments), and streaming a big file without a spool folder. HTML and rich
  text are built but not proven on real machines. See
  [docs/clipboard.md](docs/clipboard.md#known-gaps).
- **Smooth touchpad scrolling.** The client sends whole wheel steps.
- **Touch and pen input.**
- **Choosing the remote scale.** The remote screen always follows the Windows
  display scale. To change it, change the Windows scale and reconnect.
- **Release binaries and an installer.** Build from source for now.

## Not tested

The client has run on one Windows machine. Not tried yet
([details](docs/windows-quirks.md#not-tested-anywhere)):

- Windows 10, ARM, a Remote Desktop session, sleep and resume.
- More than one monitor, monitors with different scales, or a scale change
  while the client runs.
- Keyboard layouts other than US and German, and any IME.
- Other GPUs and drivers, and `--adapter` beyond the default.
- *Sharp colour* on a Windows machine: the 4:4:4 picture is proved on the
  build machine only (decode, recombine and a CPU model of the shader).
- Third-party clipboard managers.
- With `--super-l registry`: whether the inactivity lock and a
  password-protected screen saver still lock Windows while the window is
  focused or hovered. Until this is tested, do not rely on either to lock the
  machine while the window has focus. The default (`--super-l chord`) changes
  no Windows setting.

## Open issues

- **A fake right Shift with Num Lock on** has not been probed. Until it is, such
  a Shift is forwarded rather than dropped, so a real Shift is never lost; see
  [docs/windows-quirks.md](docs/windows-quirks.md#keyboard).
- **One wheel notch was dropped once at 150% scale** and has not been seen
  again. If it happens to you, `wheel_steps_sent` in `input-stats.json` tells a
  dropped send from a dropped scroll.
- **A click between the two halves of an AltGr press** carries a Ctrl; see
  [docs/windows-quirks.md](docs/windows-quirks.md#known-limitations).
- **`--output auto` can pick a leftover private screen** left by a server that
  was killed. Name the display with `--output NAME`.
- **After the remote compositor reloads**, a private (headless) screen can
  stop updating until the client re-syncs it; see
  [docs/windows-quirks.md](docs/windows-quirks.md#the-remote-compositor-reloads).
- **A spool folder of a client that crashed** stays until the next client
  starts.
- **The release chord cannot be turned off.** `--release-chord none` is refused
  at start.

gliff-win needs `gliff-server` 0.4.1 or later on the remote machine.
