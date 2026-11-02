# Known issues

What gliff-win does not do yet, what is off by default, what has not been
tested, and what is still open. Each item links to its issue or to the page
with the details. If you hit something that is not listed here, please open an
issue with a way to reproduce it.

## Off by default

These are built and can be turned on per machine in the tab's *Connection
settings*.

- **Sharp colour.** Without it the client asks for a 4:2:0 stream, so coloured
  text is a little soft. A server that encodes on the CPU still sends 4:2:0
  unless it runs with `--full-chroma`; see
  [docs/settings.md](docs/settings.md#sharp-colour).
- **GPU decode.** Without it the client decodes H.264 on the CPU, and with
  *Sharp colour* it decodes two streams, so the decode work at least doubles.
  With it on, each picture is still copied back from the GPU and uploaded
  again to draw; see [docs/settings.md](docs/settings.md#gpu-decode).
- **Send Win+L to this machine.** Without it Win+L locks Windows and
  `Ctrl+Alt+L` sends Super+L. With it on, Windows lock shortcuts are off while
  the window shows that tab and has the keyboard; see
  [docs/settings.md](docs/settings.md#send-winl-to-this-machine).

## Not built yet

- **Sound.** No audio and no microphone. Gliff's protocol has none either.
- **System tray and start with Windows.**
- **A settings window.** The five choices for one machine (Show as, Share
  clipboard, Sharp colour, GPU decode and Send Win+L) are in a tab's
  right-click menu. Everything else is a flag or a key in `config.toml`; see
  [docs/settings.md](docs/settings.md).
- **A light theme.** The tab strip and the failure panel are always dark.
- **Progress and Cancel for big pastes.** Progress is written to the log only.
- **Virtual files on the clipboard** (zip contents from Explorer, mail
  attachments), and streaming a big file without a spool folder. HTML and rich
  text are built but not proven on real machines. See
  [docs/clipboard.md](docs/clipboard.md#known-gaps).
- **Touch and pen input.**
- **Choosing the remote scale**
  ([#3](https://github.com/wynandt/gliff-win/issues/3)). With *Own screen*,
  the remote screen follows the Windows display scale. To change it, change
  the Windows scale and reconnect. *Mirror* keeps the remote screen's own
  scale.
- **The reason a session or the client ended, in one log line**
  ([#2](https://github.com/wynandt/gliff-win/issues/2)). Connect failures and
  session ends are logged, but the client's exit and the end of ssh's error
  output are not.
- **An installer.** Releases are a zip of the programs, with no installer
  and no code signature.

## Not tested

The client has run on one Windows machine. Not tried yet
([details](docs/windows-quirks.md#not-tested-anywhere)):

- Windows 10, ARM, a Remote Desktop session.
- Sleep and resume. With *Send Win+L to this machine* on, this includes
  whether Windows locks on wake after a sleep that began while the window had
  the keyboard (the client clears the setting when told the computer is going
  to sleep).
- More than one monitor, monitors with different scales, or a scale change
  while the client runs.
- Keyboard layouts other than US and German, and any IME.
- Other GPUs and drivers, and `--adapter` beyond the default.
- *Sharp colour* on screen. On a Windows machine its decode was checked:
  the decode time, and GPU pictures against CPU pictures. The recombine and
  the shader are proved on the build machine only (a CPU model of the
  shader), not on a Windows screen.
- Third-party clipboard managers.
- With *Send Win+L to this machine* on (or `--super-l registry`, or
  `super_l = "registry"` in `config.toml`): whether
  the inactivity lock and a password-protected screen saver still lock Windows
  while the window has the keyboard. Until this is tested, do not rely on
  either to lock the machine while the window has the keyboard. The default
  (the switch off) changes no Windows setting.

## Open issues

- **Launching again may not take the keyboard grab**
  ([#4](https://github.com/wynandt/gliff-win/issues/4)). A second launch hands
  its targets to the open window and brings it to the front. If the window
  misses the focus change, keys go to Windows until you click away and back.
- **The keyboard hook could time out during a local chord**
  ([#5](https://github.com/wynandt/gliff-win/issues/5)). The hook sends a kept
  chord to Windows while it holds a lock, and Windows drops a hook that is too
  slow. Not seen yet; it needs a check under heavy load.
- **A lone Super key-up after a stall can open the launcher**
  ([#6](https://github.com/wynandt/gliff-win/issues/6)). When the client
  re-syncs a private screen after a stall, it sends a key-up for every
  modifier you are not holding, Super included. Some Linux desktops open
  their launcher on a lone Super release.
- **The client reconnects when the host ends the session on purpose**
  ([#19](https://github.com/wynandt/gliff-win/issues/19)). Upstream is adding
  an error code for this. Until the client knows it, it treats that end like
  any server error and tries again.
- **GPU decode could show a black or wrong-colour picture without falling
  back** ([#20](https://github.com/wynandt/gliff-win/issues/20)). Not seen on
  Windows. The client falls back to the CPU only when the GPU decoder reports
  an error. If the picture looks wrong, untick *GPU decode* and reconnect.
- **A fake right Shift with Num Lock on** has not been probed. Until it is, such
  a Shift is forwarded rather than dropped, so a real Shift is never lost; see
  [docs/windows-quirks.md](docs/windows-quirks.md#keyboard).
- **A touchpad scroll reaches the remote as a wheel scroll.** It scrolls
  smoothly, but Hyprland's wheel binds can fire during it and applications add
  no kinetic scrolling. The cause is the order in which the server sends the
  scroll; see [docs/windows-quirks.md](docs/windows-quirks.md#pointer-and-dpi).
- **A click between the two halves of an AltGr press** carries a Ctrl; see
  [docs/windows-quirks.md](docs/windows-quirks.md#known-limitations).
- **`--output auto` can pick a leftover private screen** left by a server that
  was killed. Name the display with `--output NAME`; see
  [docs/settings.md](docs/settings.md#which-remote-screen-the-window-shows).
- **After the remote compositor reloads**, a private (headless) screen can
  stop updating until the client re-syncs it; see
  [docs/windows-quirks.md](docs/windows-quirks.md#the-remote-compositor-reloads).
- **A spool folder of a client that crashed** stays until the next client
  starts; see [docs/clipboard.md](docs/clipboard.md#known-gaps).
- **The release chord cannot be turned off.** `--release-chord none` is refused
  at start.

gliff-win needs `gliff-server` 0.4.1 or later on the remote machine.
