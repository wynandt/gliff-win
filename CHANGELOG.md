# Changelog

Notable changes to gliff-win, newest first. The section for a version is the
body of its GitHub release: the release workflow prints it with
`scripts/release-notes` and refuses a version that has no section. To cut a
release, see [CONTRIBUTING.md](CONTRIBUTING.md#releasing).

## [Unreleased]

- With an ssh helper such as a `ProxyCommand` or `ProxyJump` in the ssh
  config, closing the window while a link had stopped answering hid the
  window, but the client kept running in the background until the helper
  ended, which could be never. The client now ends ssh together with the
  helpers it started, 5 s after Bye, and exits within about six seconds of
  the close. A helper also no longer outlives a `hyprctl` query that timed
  out. (Addresses [#85](https://github.com/wynandt/gliff-win/issues/85))
- When Windows refuses the message that stops the keyboard hook's thread,
  the client log names the refused post and the Windows error, such as
  `ERROR_NOT_ENOUGH_QUOTA`. Closing still waits at most 500 ms for that
  thread. Before, the log said only that the thread did not stop in time.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- The keys and pointer checks stop at start, with the reason, when Windows
  refuses to move the client window to the corner of its monitor's work area.
  Before, the cases ran with the window wherever it was.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- When the client does not rewrite its stats file within 2.5 times its
  one-second interval after a case, a keys case that checks the client's
  grab, keymap or hook reinstalls fails as inconclusive. Before, it was
  judged from the older copy of the file. The wait now follows the client's
  interval, and the 500 ms pause after the client window appeared is gone.
  ([#67](https://github.com/wynandt/gliff-win/issues/67))

## [0.5.0]

- Added a per-machine *Scale* setting for *Own screen* (`scale` in
  `[overrides."user@host"]`, accepting `"auto"` as the default or fixed `1.25`,
  `1.6`, or `2`). A tab's *Connection settings* submenu offers four radio
  choices (`Auto`, `125%`, `160%`, `200%`), which are greyed out and ignored
  in *Mirror* mode. On a tab with a live *Own screen* session a pick applies
  at once: the client sends a Resize on the running session, so the remote
  output and the windows on it stay. A hidden tab takes the new scale when it
  is shown next, and *Reconnect now to apply* is not offered for a
  Scale-only change. *Show as*, *Sharp colour* and *GPU decode* still need
  *Reconnect*. Hyprland may round the scale, and the output size so the
  scale divides it. Changing the scale in the remote desktop's own display
  settings is still not supported. (Part of [#3](https://github.com/wynandt/gliff-win/issues/3))
- Choosing *Reconnect* or *Reconnect now to apply* on a tab that already had
  a running or connecting session did nothing, and turning *Share clipboard*
  on for a session that started without it did not reconnect the tab.
  *Reconnect* now stops the active session while keeping the tab open and
  starts a fresh session with the current saved choices once the old session
  finishes closing.
- A window that could not use *Send Win+L to this machine* because another gliff window held the Windows lock setting now takes it over once that window has gone, without a restart. (Part of [#114](https://github.com/wynandt/gliff-win/issues/114))
- Listing a GPU's outputs ends only when Windows reports there are no more.
  Any other failure stops Direct3D setup instead of skipping that GPU or
  reporting that it does not drive the window's monitor.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- The lock watchdog is started again, inside the client's job, only when
  that job refuses to let it leave. Any other failure to start is reported
  as that error, and the lock setting stays off.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- A lock-policy change whose mutex cannot be created, or whose wait fails,
  is reported and does not read or write the setting. A turn-on that fails
  this way leaves the registry route off for that window. A restore that
  fails this way keeps the marker and tries again. A wait that reaches 2
  seconds still goes ahead, so a stuck holder cannot block a restore.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- If the single-instance lock cannot be created or checked, the client
  shows a message box that names the error, writes that error on the exit
  line, and stops before it opens a window. A second start while a client
  is already running still adds a target to the open window, or brings that
  window forward when there is no target.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- A failed request for per-monitor DPI awareness is written to the client
  log once the log is open, including when start is refused before the
  window exists. The request is still per-monitor v2, made before any
  window, and the client still starts. A successful request writes
  nothing. The program has no manifest that would already have set that
  awareness.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- The placement probe and `gwin-keys` pick one Hyprland instance: the newest
  `.socket.sock` under the runtime directory. They had been able to attach
  to different instances, because `gwin-keys` followed the newest directory.
  ([#66](https://github.com/wynandt/gliff-win/issues/66))
- PNG, JPEG, GIF, HTML and rich text read from the Windows clipboard stop
  at 32 MiB plus 4096 bytes. A file list and any other registered format
  stop there too. Text still stops at twice 32 MiB plus 4096 bytes, and a
  bitmap still stops at its pixel limit. A body over 32 MiB after conversion
  is still refused. An HTML Format payload larger than 32 MiB plus 4096
  bytes is no longer read, even when the copied fragment would have fit.
  ([#68](https://github.com/wynandt/gliff-win/issues/68))
- The stream check flushes a held picture after the client's idle drain
  (150 ms) and waits the client's 5 s for the server to exit after Bye.
  It had kept a second drain and a 10 s wait.
  ([#66](https://github.com/wynandt/gliff-win/issues/66))
- The lock step tries a refused placement again. A later try that puts the
  window in front still leaves what the failed try said, and that step fails.
  ([#67](https://github.com/wynandt/gliff-win/issues/67))
- When the keys check cannot dismiss a window a chord may have opened, that
  case fails. Escape still goes only to the text sink, so it is not sent to
  the client.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- A version check whose wait for ssh fails reports that wait error. A check
  that runs out of time still reports that the machine did not answer.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- A panic while a log line is being written leaves the later lines in the
  file. When the log was never opened, or a write fails, that line is absent
  and the client continues.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- In fullscreen, the distance that shows the strip and the strip's own
  sizes could use different scales when the display dpi was below 48, or
  when it did not fit in a signed 32-bit integer. Both use one scale now:
  a dpi below 48 counts as 48, and a dpi that does not fit counts as 96.
  ([#66](https://github.com/wynandt/gliff-win/issues/66))
- A save no longer replaces `config.toml` or `recent-hosts.toml` when the
  last read, just before the rename, cannot read the file. The save fails
  and the file stays, including text that read never copied. A missing file
  is still replaced.
  ([#65](https://github.com/wynandt/gliff-win/issues/65))
- Clicking a tab after the window had started closing could start a new
  session. A click then starts nothing, and the saved tabs stay as they were.
  ([#86](https://github.com/wynandt/gliff-win/issues/86))

## [0.4.0]

- The client log now says why each session and the client ended. Each
  session that ends gets one line with the reason (the client said Bye, the
  server sent an error, the server is incompatible, the link was lost, or ssh
  failed) and ssh's exit code. After an ssh error the line also has the last
  lines ssh printed: at most 6 lines of 200 characters. One more line says
  why the client exited (the window was closed, Windows is signing out or
  shutting down, another client was already running, or an error), with the
  exit code and how many sessions ended cleanly. Before any of this text goes
  in the log, file paths, host key fingerprints, key text, long hex digests,
  private key blocks and the values of password, token and secret settings
  are hidden, and control and invisible characters are removed. The client
  reads ssh's stderr file only when it is a plain file with one name. A
  symbolic link, junction, folder or hard link in its place is not read, and
  the line says "(not read: the file is not a plain file)". See the README
  section "Why a session or the client ended".
  ([#2](https://github.com/wynandt/gliff-win/issues/2))
- Clicking a tab just after disconnecting it could start a second session
  while the first was still closing. A click now only shows that tab until
  the old session has finished closing (usually under a second, at most about
  six seconds); click again to connect.
  ([#46](https://github.com/wynandt/gliff-win/issues/46))
- A key pressed while the keyboard hook was too slow could reach the remote
  as down, up, then down again, and stay held there. The slow hook call now
  drops a key the window has already sent.
  ([#56](https://github.com/wynandt/gliff-win/issues/56))
- With the default thumb button setting, Back and Forward could do nothing
  on the remote: they were sent as codes the server does not use for those
  buttons. The default (`--side-buttons upstream`) now sends `BTN_SIDE` and
  `BTN_EXTRA`, the codes the server uses, the same as `linux-mouse`.
  ([#60](https://github.com/wynandt/gliff-win/issues/60))
- The picture could stay frozen until the session ended when the decoder
  failed while catching up after a quiet spell. The client now drops the
  pictures stuck in the decoder and asks the remote for a full picture, as
  it already did for other decode errors.
  ([#58](https://github.com/wynandt/gliff-win/issues/58))
- When Windows could not set up COM for GPU decode, the client went on into
  Media Foundation anyway, and the errors that followed were hard to read.
  GPU decode now stops there with a COM error in the log, and the client
  decodes on the CPU.
  ([#61](https://github.com/wynandt/gliff-win/issues/61))
- A remote pointer shape whose click point lay outside its image was still
  shown, so a click could land away from the drawn pointer. The client now
  shows the system arrow for such a shape, as the upstream client does.
  ([#62](https://github.com/wynandt/gliff-win/issues/62))

## [0.3.0]

- Click the graph button beside fullscreen to show or hide connection
  statistics over the active tab's picture. They are off at startup.
  The readout shows white monospace text over 60% black: decoder tiers,
  FPS, video payload Mbit/s, mean decode time, stream size and the
  server cap. Switching tabs shows that session's statistics. The
  readout never takes the keyboard or pointer.
  ([#41](https://github.com/wynandt/gliff-win/issues/41))
- The program now uses upstream Gliff's application icon in the window
  title, the taskbar, Alt+Tab and Explorer, instead of the generic Windows
  icon.
- Resizing the window back to an earlier size could be lost if the remote
  had not applied the first resize yet, so the picture ended at the
  intermediate size. The client now still requests the final size.
  ([#44](https://github.com/wynandt/gliff-win/issues/44))
- A remote clipboard copy could be cleared when you switched to another tab
  and that tab announced an empty clipboard. The copy now stays with the
  tab that supplied it; only that tab can withdraw it.
  ([#43](https://github.com/wynandt/gliff-win/issues/43))

## [0.2.2]

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
