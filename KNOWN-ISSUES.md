# Known issues

What gliff-win does not do yet, what is off by default, what has not been
tested, and what is still open. Each item links to its issue or to the page
with the details. If you hit something that is not listed here, please open an
issue with a way to reproduce it.

## Choices per machine

These are built and can be switched per machine in the tab's *Connection
settings*. All are off by default except *GPU decode*, which is on, and
*Scale*, which defaults to *Auto*.

- **Scale** (for *Own screen*). Defaults to *Auto* (the Windows display scale);
  can be set per machine to `125%`, `160%`, or `200%` (`scale` in
  `[overrides."user@host"]`). Greyed out and ignored in *Mirror* mode. On a
  live Own screen tab a change applies at once with a Resize on the running
  session; a hidden tab takes it when next shown. *Show as*, *Sharp colour*
  and *GPU decode* still need a reconnect. See
  [docs/settings.md](docs/settings.md#scale). Part of
  [#3](https://github.com/wynandt/gliff-win/issues/3) (native Windows check
  pending).
- **Sharp colour.** Without it the client asks for a 4:2:0 stream, so coloured
  text is a little soft. A server that encodes on the CPU still sends 4:2:0
  unless it runs with `--full-chroma`; see
  [docs/settings.md](docs/settings.md#sharp-colour).
- **GPU decode** is on by default. Unticked, the client decodes H.264 on the
  CPU, and with *Sharp colour* it decodes two streams, so the decode work at
  least doubles. With it on, each picture is still copied back from the GPU and uploaded
  again to draw; see [docs/settings.md](docs/settings.md#gpu-decode).
- **Send Win+L to this machine.** Without it Win+L locks Windows and
  `Ctrl+Alt+L` sends Super+L. With it on, Windows lock shortcuts are off while
  the window shows that tab and has the keyboard; see
  [docs/settings.md](docs/settings.md#send-winl-to-this-machine).

## Not built yet

- **Sound.** No audio and no microphone. Gliff's protocol has none either.
- **System tray and start with Windows.**
- **A settings window.** The six choices for one machine (Show as, Scale,
  Share clipboard, Sharp colour, GPU decode and Send Win+L) are in a tab's
  right-click menu. Everything else is a flag or a key in `config.toml`; see
  [docs/settings.md](docs/settings.md).
- **A light theme.** The tab strip and the failure panel are always dark.
- **Progress and Cancel for big pastes.** Progress is written to the log only.
- **Virtual files on the clipboard** (zip contents from Explorer, mail
  attachments), and streaming a big file without a spool folder. HTML and rich
  text are built but not proven on real machines. See
  [docs/clipboard.md](docs/clipboard.md#known-gaps).
- **Touch and pen input.**
- **The reason a session or the client ended, in one log line**
  ([#2](https://github.com/wynandt/gliff-win/issues/2)). Connect failures and
  session ends are logged, but the client's exit and the end of ssh's error
  output are not.
- **An installer.** Releases are a zip of the programs, with no installer.
  Releases from 0.2.1 on are code-signed.
- **Automatic updates**
  ([#40](https://github.com/wynandt/gliff-win/issues/40)). Each new version
  is a zip on the Releases page. The client does not check for updates or
  install them.
- **Send the monitor refresh rate in Resize**
  ([#29](https://github.com/wynandt/gliff-win/issues/29)), after upstream
  [omacom/gliff#117](https://github.com/omacom/gliff/pull/117) merges.
- **Send the stale clipboard abort reason**
  ([#28](https://github.com/wynandt/gliff-win/issues/28)), after upstream
  [omacom/gliff#118](https://github.com/omacom/gliff/pull/118) merges.

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
- **A lone Super key-up after a stall could open a launcher**
  ([#6](https://github.com/wynandt/gliff-win/issues/6), closed as not
  reproduced). When the client puts the keyboard hook back or re-syncs a
  private screen, it sends a key-up for every modifier you are not holding,
  Super included. Hyprland 0.56.2 ignores such a release. A desktop that acts
  on a lone Super release could open its launcher.
- **Keypad keys can arrive as arrow keys**
  ([#53](https://github.com/wynandt/gliff-win/issues/53)). The server starts
  with Num Lock and Caps Lock off, and the client does not send its lock
  state. With Num Lock on in Windows, keypad digits can reach the remote as
  Home, End, arrows and Page keys. Not reproduced yet. The fix waits for an
  upstream protocol message.
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
- **The client can keep running after the window closes**
  ([#85](https://github.com/wynandt/gliff-win/issues/85)). If a process left
  over from a session still holds that session's link, the window goes away
  but the process stays in the background until the leftover process ends.
  Seen with a test server that never exits. Not yet seen with a real ssh
  setup.

gliff-win needs `gliff-server` 0.4.1 or later on the remote machine.
