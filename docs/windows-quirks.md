# Windows quirks

What Windows does that the client has to work around, and where each workaround
lives. Each entry says what Windows does, what the code does about it, and
where it has been tried. How the code is laid out is in
[architecture.md](architecture.md); known limits and open issues are in
[KNOWN-ISSUES.md](../KNOWN-ISSUES.md).

Read [Tested where](#tested-where) first: the client has run on one Windows
machine, and most entries below are true of that machine until someone shows
otherwise.

## Tested where

- **Windows:** one Windows 11 machine with one high-resolution display, at 200%
  scale for most runs and 150% for one. No other Windows version, no second
  display, no touch, pen or IME.
- **Remote machine:** one Hyprland machine. The final run used `gliff-server`
  0.4.1-1; the first full run used v0.4.0 with the Vulkan Video encoding fix
  applied (the exact build was not recorded).
- **Keyboard layouts:** US and German.
- **Results:** summarised in the table below. The raw logs are not part of
  this repository.

| Check | Result |
|---|---|
| `gwin-verify keys` | 40/40 on the first full run; 40/41 on the final build, whose one extra case failed from a test-order bug in the check, fixed with a replay test; the release group then passed 3/3 |
| `gwin-verify pointer` at 200% | 20/20, twice |
| `gwin-verify pointer` at 150% | 19/20 on the first run (all 9 clicks passed, one wheel notch dropped); 20/20 on the final build |
| `gwin-verify keys --only lock` (Super+L) | 9/9 (an earlier attempt was 8/9 from a human timing slip) |
| `gwin-verify keys --only manual` | 8/8 |
| Clipboard (text, images, files, both ways) | text, image and files passed on the second run (the first run failed files on a colon in a name, fixed); html and rtf have no gate case yet; see [clipboard.md](clipboard.md#hardware-gate) |

## Not tested anywhere

- Windows 10. It has not run there.
- More than one monitor, monitors with different scales, and a scale change
  while the client runs (planned).
- Touch, pen, an IME, sleep and resume, a Remote Desktop session, ARM.
- Keyboard layouts other than US and German. The layout table has about 25
  rows and is checked against xkeyboard-config, but only two layouts were typed
  on real hardware.
- Any other GPU or driver. The video path is CPU decode (or GPU decode when
  a machine's choice is on) and a D3D11 present, and the adapter choice (`--adapter`) is untested beyond the default.
- Third-party clipboard managers.

## Windows versions

- **A low-level hook gets at most 1000 ms.** `LowLevelHooksTimeout` is capped at
  1000 ms from Windows 10 1709. A hook that is slower is removed without notice.
  The client runs the hook on its own thread, does as little as possible there,
  and detects a silent removal: a key that arrives as a window message instead
  is forwarded and the hook is reinstalled (`hook_reinstalls` and `legacy_keys`
  in the stats file). `gwin-verify keys` has a check that stalls the hook for
  2.5 s on purpose; it reports "inconclusive", not a pass, when Windows did not
  drop the hook that time.
- **Keys that go down while no hook is looking.** For the moment between the
  removal and the new hook, Windows sees every key itself. A key that went down
  then has its Down counted by Windows, and if the new hook swallowed its Up,
  Windows would keep it down for good: a Win or Ctrl that never comes up turns
  the next plain key into a shortcut. The client therefore keeps a ledger of
  each key: whether Windows saw its Down, and whether the remote machine did
  (`key_ledger`). The ledger outlives the hook. The new hook starts from the keys
  Windows holds down, so an Up of a key Windows saw go down goes to Windows (and
  to the remote machine as well when it saw the Down), and the release chord
  works even if Ctrl and Alt went down in the gap. A key the hook lets through
  because no session is there, or because the session could not take it, is
  recorded the same way. When the grab ends, the hook asks the session to let go
  of every key it still holds. At a reinstall the client tells the remote that
  Win, Ctrl, Alt and Shift are up, except the keys the user holds, and presses
  an Up on Windows only for a modifier Windows counts as down although the hook
  saw it come up. It presses no Up for a key the hook never saw move: Windows
  cannot tell a key that is held from one that is stuck, and a release would
  cut a chord the person is still holding. At the end of a grab, and in the
  sweep after a return from the lock screen or a secure prompt, the client lets
  go of every modifier Windows counts as down except one the hook saw go down
  and not come up, so a Ctrl+click or Shift+click on the window you switch to is
  not cut: the person's own Up reaches Windows. A modifier whose Down the hook
  never saw is still released at a grab end. When the client exits it lets go of
  every modifier Windows counts as down, held or not.
- **What counts as a silent hook.** A key message that reaches the window
  while the window grabs and the hook is also in grab mode, and that the hook
  did not note passing. A key the hook passes after the release chord, or with no
  session to send it to, is not a sign of a silent hook.
- **The log holds no keystrokes.** The line written when the hook was lost
  says how many keys were down and which of Win, Ctrl, Alt and Shift were among
  them, in that fixed order. It never names another key. A test checks that
  no other key can change that line.
- **Why the hook looked silent.** The same line adds facts that tell the causes
  apart, none of them a key:

  ```text
  keyboard hook silent: key sent from the window (1 so far), hook installed again; 0 keys down; message extra info: none; hook calls 12, last 85 us, slowest 310 us, last ran 412 ms ago, injected keys seen 0; hook noted this key within 5 s: no
  ```

  - `message extra info`: `set` when the key message carried extra info. A
    program that sends keys (an on-screen keyboard or an input method) usually
    sets it; a keyboard does not. Windows tells a window nothing more about
    where a key came from.
  - `hook calls` is the number of callbacks since the hook was installed, with
    how long the last and the slowest took in microseconds and how long ago
    the last one ended. A slowest time near the Windows limit (300 ms by
    default), or a long time since the last call, points at a hook that was
    slow or dropped. The line says `hook never called since installed` when no
    key reached it.
  - `injected keys seen` counts keys the hook saw flagged as sent by another
    program.
  - `hook noted this key within 5 s`: whether the hook had noted passing a key
    of the same kind (left and right Ctrl count as one) within 5 seconds
    either way. `yes` means the key's time did not match its note; `no` means
    the hook never handled it.
- **OpenSSH.** The client needs the OpenSSH client (`ssh.exe`). It starts
  `%SystemRoot%\System32\OpenSSH\ssh.exe` by full path. Only when that file
  does not exist does it fall back to the bare name `ssh.exe`, which Windows
  looks up in the client's own folder first. `--ssh PATH`, or `ssh` in
  `config.toml`, names another one, and it always wins.
- **Programs and DLLs the client starts.** The Open action on a file, folder or
  web address starts `%SystemRoot%\explorer.exe` by full path. The client
  is linked with `/DEPENDENTLOADFLAG:0x800` (`LOAD_LIBRARY_SEARCH_SYSTEM32`),
  so the DLLs it imports, and any it loads by name without a path, come from
  the system folder, not from the client's folder or the current folder.
  The client imports the Visual C++ runtime (`VCRUNTIME140.dll`), so that
  runtime must be installed in the system folder; a copy placed next to the
  client is not used.
- **What Windows' ssh says when a connection fails.** The failure classes in
  `connect/diag.rs` were first made from OpenSSH on Linux. The same cases were
  then captured from OpenSSH for Windows 9.5p2 (`ssh.exe` from Windows itself),
  and live in `crates/gliff-windows/tests/ssh_corpus/windows-openssh-9.5p2/`;
  `scripts/capture-ssh-corpus-windows.ps1` makes them again. Only the cases that
  need no server could be captured that way. An unknown or changed host key, a
  refused key and a missing server program have no Windows capture yet. The
  differences:

  | Case | Linux (OpenSSH 10.0p2) | Windows (OpenSSH for Windows 9.5p2) |
  |---|---|---|
  | Nothing listening, `ConnectTimeout` set (the client always sets it) | `ssh: connect to host H port P: Connection refused` | `banner exchange: Connection to UNKNOWN port -1: Connection refused` (no host or port) |
  | Nothing listening, no `ConnectTimeout` | as above | `ssh: connect to host H port P: Connection refused` |
  | Name not found | `ssh: Could not resolve hostname H: Name or service not known` | `ssh: Could not resolve hostname H: No such host is known. ` (trailing space) |
  | No answer | `ssh: connect to host H port 22: Connection timed out` | the same |
  | Address with no route (IPv6) | `Network is unreachable` | `ssh: connect to host H port 22: Unknown error`, so the client reads an `Unknown error` on a failed connect as unreachable |
  | Closed after connecting | `kex_exchange_identification: read: Connection reset by peer`, then `Connection reset by H port P` | `kex_exchange_identification: read: Connection reset` (no "by peer"), then the same last line |
  | Config file ssh cannot parse | `PATH: line N: Bad configuration option: X` | the same, with every backslash of PATH doubled |

  ssh ends its own messages with CRLF on both systems, so the classifier reads
  lines with `str::lines`, which drops the CR. A message to the person never
  holds a CR or a path.

## Keyboard

- **Only a low-level keyboard hook can swallow Win chords and Alt+Tab.** Raw
  Input cannot. The hook sees the scan code, the extended flag, the injected
  flag and the time, before Windows updates its key state.
- **Not blockable: Ctrl+Alt+Del, and the secure desktop.** A UAC prompt or Ctrl+Alt+Del
  switches to the secure desktop, where the hook sees nothing, so a key released
  there is never seen as released. While grabbed, the client polls the input
  desktop every 250 ms; when it is not the default one, the client ungrabs and
  sends a release for every held key to the remote machine. An elevated window
  that has focus blocks input to the client (UIPI).
- **A window can take the foreground without telling the grabbing window.**
  The grab follows the `WM_SETFOCUS` and `WM_KILLFOCUS` messages. A full-screen
  game that starts while the client launches took the foreground and the client
  saw no `WM_KILLFOCUS`: the hook went on swallowing every key and sending it to
  the remote machine, and the game got none, until Ctrl+Alt+Del dropped the grab
  (about 74 s in the log). So, with `--grab-follows focus`, while the keyboard
  is grabbed the client asks `GetForegroundWindow` about once a second. A
  window of its own process counts as the client's (its menus, tooltips, the
  address bar and dialogs); no window at all, or one that cannot be read, counts
  as no evidence. Two looks in a row at another program's window are taken as a
  lost focus, and the grab ends by the normal path (held keys are released on
  both machines). One log line says so; it names no key. If the foreground comes
  back to the client without a focus message, the same look takes the focus
  back. `--grab-follows hover` is not checked: there the keyboard follows the
  pointer, not the foreground. The check was not run against a real game: that
  needs a live test.
- **Pause and NumLock share scan code 0x45.** Only the virtual key tells them
  apart, and a catch-all on 0x45 swallowed E0 45 (the `KEY_SHOP` key) in a
  prototype. The key table names `VK_PAUSE` and `VK_NUMLOCK` only; the case is a
  test.
- **Right Shift is an extended key.** A probe of the keyboard hook on a built-in keyboard (Num Lock off)
  measured the real keys:

  | Key | `vkCode` | `scanCode` | extended flag | injected |
  |---|---|---|---|---|
  | Left Shift | 0xA0 | 0x2A | no | no |
  | Right Shift | 0xA1 | 0x36 | **yes** | no |
  | Up | 0x26 | 0x48 | yes | no |
  | Delete | 0x2E | 0x53 | yes | no |

  A real right Shift therefore arrives as E0 36 and goes to the remote machine as
  `KEY_RIGHTSHIFT` (54). The old PS/2 fake right Shift is also E0 36, so scan
  code and extended flag alone can not tell a fake from a real one. What the
  client relies on is that Windows tags the fake shifts it makes: when
  Num Lock is on, Windows inserts a Shift-up before a dual-state keypad key and a
  Shift-down after it, and those events carry the "simulated" bit (0x200) in
  `scanCode`: 0x22A for the left Shift and 0x236 for the right one. The client drops
  every key whose scan code is above one byte (the one-byte conversion in
  `keymap::evdev`, which was there before right Shift was forwarded), so the
  tagged fake shifts never reach the remote machine. E0 2A is also dropped (a real left Shift is never
  extended). The fake-shift rule is documented behaviour that AutoHotkey relies on
  (`SC_FAKE_LSHIFT` 0x22A, `SC_FAKE_RSHIFT` 0x236), not something measured on
  this keyboard: the probe ran with Num Lock off, where Windows makes no fake
  shifts, and no Shift event appeared around Shift+Up. A probe with Num Lock on
  is still open (see [KNOWN-ISSUES.md](../KNOWN-ISSUES.md)). Until it runs, a fake right
  Shift that is E0 36 without the simulated bit would be forwarded as a Shift,
  never dropped, so a real right Shift can not be lost. The cost would be
  Shift going up and down around the extended key (no key stays stuck: the
  Up and the Down are both sent, and the grab end lets go of what was sent). The fallback for a silent hook (key
  messages, which carry only the low byte of the scan code) can not see the
  simulated bit.
- **AltGr.** AltGr arrives as a left Ctrl (scan
  0x1D, not extended) and a right Alt (E0 38) with the same `time`. The client
  holds a left Ctrl for up to 10 ms and drops it, with its key-up, if the right Alt
  follows in that time; otherwise it forwards the Ctrl late. A
  click or a wheel step releases a waiting Ctrl at once, so a real Ctrl+click
  keeps its order.
- **Other set-1 quirks the table absorbs:** PrtSc arrives as E0 37, Ctrl+Break as
  E0 46, and injected virtual-key-only events carry scan 0.
- **Injected keys.** Password-manager auto-type arrives as injected keys. They are
  forwarded by default (`--injected-keys local` leaves them to Windows).
- **The release chord is Ctrl+Alt+Home.** On a compact keyboard, Home is often
  Fn+Left, which sends an extended Home and works. A keypad 7 key, even one
  with Home printed on it, sends scan code 0x47 with no extended prefix and the
  client reads it as keypad 7, so it does not release (tested by hand). On a layout with
  AltGr, release with the left Alt, not AltGr. `--release-chord` takes another
  chord, for example `shift+esc`.
- **Win+Shift+S stays on Windows.** See [Local chords](#local-chords).

### Win+L and the lock setting

Windows handles Win+L before any hook, so the hook never sees it while locking
is enabled. The only route without admin rights is to switch locking off per
user: `HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\System\DisableLockWorkstation`
set to 1. Then the hook sees the `L`, and every other way to lock (the Start
menu, the Ctrl+Alt+Del screen, `LockWorkStation()`) stops too.

The client sets the value only while the window has the keyboard (it is
focused, or with `--grab-follows hover` the pointer has grabbed it), and only
for a machine whose *Send Win+L to this machine* switch is on (or with
`--super-l registry`, below). A pointer resting on a window that does not have
the keyboard never sets it. The value goes back 300 ms after the window loses
the keyboard (so a quick focus flicker does not write the registry twice), and
at once when the switch is turned off, the window shows a tab whose switch is
off or no tab, or the computer goes to sleep. A Win tap that the swallowed
`L` would leave behind is masked with a tagged tap of the unassigned `VK_E8`, so
Start does not open. Every way the value could be left on has a way back:

- a marker `HKCU\Software\gliff-windows\LockPolicy`, written before the value,
  keeps the prior state (absent, or its exact value) and the owner process;
- the exit paths (drop guard, a panic on the window thread, `WM_DESTROY`,
  `WM_ENDSESSION`, a console control handler) restore it;
- a helper process (`gliff-windows.exe --lock-watchdog`) restores it if the client is
  killed, and also releases any Shift, Ctrl, Alt or Win key that Windows still
  counts as down;
- on start, a marker whose owner is gone is restored and removed, in every
  `--super-l` mode and whatever the switches say. So a crash, or a kill that
  takes the helper too (ending the whole process tree), leaves the setting on
  only until the next start or `--restore-lock-policy`;
- a value that was already 1 with no marker of ours is not ours and is never
  cleared;
- `gliff-windows.exe --restore-lock-policy` does the same by hand.

By default no machine's switch is on and `--super-l` is `chord`, so the client
does not change the setting (it only repairs a dead client's leftover,
above): Win+L locks
Windows as usual, even while the window has the keyboard, and Ctrl+Alt+L
(`--super-l-chord` changes the chord) sends Super+L to the remote machine. The
client first releases the modifiers the remote machine holds, including Ctrl
and Alt, then sends Super down, L down, L up and Super up, and presses the
modifiers again. On a tiling window manager such as Hyprland, Super+L is a
focus binding, not a lock, so on the remote machine the chord moves focus.
`--super-l off` forwards nothing.

**Send Win+L to this machine** is the opt-in, one machine at a time. Right-click
a tab, open *Connection settings* and tick it. It is off by default, saved as
`send_win_l = true` in that machine's `[overrides."user@host"]` table, and
applies at once, with no reconnect. While the window shows that tab and has
the keyboard, Windows locking is switched off for your user. That covers Win+L, the Lock entry on the Ctrl+Alt+Del screen
and the Lock entry in the Start menu. Win+L then goes to the remote machine, and
with no live session (a failed tab) it does nothing at all. Whether the
inactivity lock and the password-protected screen saver still work during that
time has not been tested on Windows (see
[KNOWN-ISSUES.md](../KNOWN-ISSUES.md)). Until that is
settled, do not rely on either to lock a machine whose window has the
keyboard.
`super_l = "registry"` in `config.toml` turns the switch on for every machine
that has no choice of its own; a machine's own choice beats it.
`--super-l registry` on the command line turns it on for every machine, and
`--super-l chord` or `--super-l off` turns it off for every machine; either
flag greys the switch in the menu, with the reason under it.

`gwin-verify` starts its other checks with `--super-l chord`, so only the lock
step (which starts the client with `--super-l registry`, the same route as the
switch) writes the setting, and it should only be run by the person at the
machine. An injected Win+L does not lock, so only a real key tests it.

### Layouts

The client reads the active layout with `GetKeyboardLayoutNameW` (a KLID such as
`00000407`) and sends an XKB keymap built from it: an exact table first, then
the language's default, then `us` with a warning in the log. The layout is
per thread, so the client re-reads it on `WM_INPUTLANGCHANGE` and `WM_SETFOCUS`
and sends a new keymap when the text changes. While grabbed, the layout hotkeys
(Win+Space, Alt+Shift, Ctrl+Shift) go to the remote machine like any key, so the
Windows layout cannot change then. A Windows IME is switched off for the window;
the remote machine composes from raw keys.

The keymap decides what a focused remote program types, not which Hyprland bind
fires: with `input:resolve_binds_by_sym` off (the default), a bind follows the
key's place under the remote machine's own layout. The first hardware run showed
it: a German keymap was sent and the bind for the US `Y` still fired where `Z`
was expected. `gwin-verify keys` therefore reads the typed keysym, not a bind.

### Local chords

A grab sends every key to the remote machine, so Win+Shift+S would never reach
Windows. The hook keeps a list of local chords (`--local-chords`, default
`win+shift+s`). When the last key of one goes down with exactly its modifiers
held, the client tells the remote machine to release everything and replays the
chord to Windows, which never saw the modifiers go down. The remote machine sees
Win and Shift go down and come up for a moment. The replay is covered by host
tests and the build, and it worked in the clipboard hand test (the snip was taken
on Windows and pasted on Linux). See
[clipboard.md](clipboard.md#pictures-and-snips) for the clipboard side.

## Pointer and DPI

- **The window is per-monitor DPI aware (v2),** so every size is in physical
  pixels, and the protocol scale is `dpi / 96`. Mouse positions are client
  pixels and become remote pixels through the layout.
- **A resize waits for the size to hold.** `ResizeGate` sends 200 ms after the size
  stops changing, at once when a drag ends, and after `WM_DPICHANGED` once the
  window takes the suggested rectangle. Sizes are rounded down to even and are never under 64.
- **Leaving the window sends no more moves.** The client asks for `TME_LEAVE` and
  holds the capture on a button down, so a drag outside the window keeps going.
- **A touchpad scrolls smoothly.** A notch is 120; a precision touchpad sends
  many smaller deltas through the same wheel messages. A delta of whole notches
  goes out as wheel steps, any other as a smooth scroll that ends 150 ms after
  the last message. Windows already adds the touchpad's inertia, so the remote
  sees it as more smooth deltas.

  The client marks the smooth scroll as a finger scroll (`discrete` empty),
  but the remote does not see it as one today. The server's virtual pointer
  sends the axis source before the axis (`crates/hypr-input/src/thread.rs`,
  the `InputCmd::Axis` arm), and Hyprland's virtual pointer starts a new axis
  event, whose source is the wheel, on every axis request
  (`src/protocols/VirtualPointer.cpp`, `setAxis`). So each smooth delta
  reaches the remote as a wheel event with no step, Hyprland makes up a
  high-resolution wheel value for it (8 per pixel), and applications that read
  that value scroll smoothly. Two things follow: Hyprland's wheel binds
  (`mouse_up`, `mouse_down` and so on) can fire during a touchpad scroll, at
  most once per `binds:scroll_event_delay`, and applications see no finger
  scroll, so they add no kinetic scrolling of their own. The upstream Linux
  client sends its touchpad the same way and is affected the same way.
  Sending the source after the axis would fix it on the server.
- **Thumb buttons** are `XBUTTON1` (back) and `XBUTTON2` (forward). `--side-buttons`
  chooses which Linux codes they send; the default is upstream's, and which default
  to keep is not decided yet.
- Tested at 200% and 150% only. A scale change while the client runs has not been
  tried.
- **With Own screen, the remote scale is the Windows scale.** Change it on
  Windows and reconnect. Changing the scale of the private screen in the remote
  desktop's own display settings is not supported; see
  [The remote compositor reloads](#the-remote-compositor-reloads).

## Clipboard

The details are in [clipboard.md](clipboard.md). The parts that are Windows
behaviour:

- **Delayed rendering.** `SetClipboardData(format, NULL)` promises a format.
  `WM_RENDERFORMAT` arrives when something pastes, and the owner must answer
  without opening the clipboard, because the requester holds it open.
  `WM_RENDERALLFORMATS` arrives when the owner exits. The client runs its
  clipboard window on its own thread, so a render that waits for the network
  blocks that thread and the program that asked, not the picture.
- **Change notification** comes from `AddClipboardFormatListener` on a
  message-only window.
- **Synthesized formats.** Windows makes `CF_TEXT` from `CF_UNICODETEXT`, and
  the DIB variants and `CF_BITMAP` from each other. A snip is `CF_DIB` alone,
  so the client reads `CF_DIBV5` or `CF_DIB` (a registered `PNG` is preferred when
  there is one). In the other direction it sets `PNG` and `CF_DIBV5`, and Windows
  adds `CF_DIB` and `CF_BITMAP`.
- **History and cloud sync read every format at copy time.** That would force a
  whole remote copy, up to 4 GiB for files, so remote copies carry the three
  privacy marks (`CanIncludeInClipboardHistory` 0, `CanUploadToCloudClipboard` 0,
  `ExcludeClipboardContentFromMonitorProcessing`) unless `--clipboard-history`.
  A local copy that carries one of those marks (a password manager sets
  them) is sent as an empty offer.
- **File names Windows refuses.** The spool lives in
  `%LOCALAPPDATA%\gliff-windows\clipboard`. `<>:"\|?*` and control characters
  become `_`, a trailing dot or space is removed, device names (`CON`, `NUL`,
  `COM1` and so on) get a `_` in front, and names that collide in case get a
  number. Path parts over 255 UTF-16
  units are shortened and the `\\?\` prefix is used from 240 characters; a
  path below the top level over 259 characters is spooled but only opened by
  programs that support long paths.
- **Explorer trims a leading space** from a folder name when it pastes; the
  client does not rename such a name. A pasted dot-file does not get the
  hidden attribute; the client does not set it.
- **Explorer's cut is a copy.** The source is never removed.
- **Virtual files** (files inside an archive Explorer shows as a folder, a mail
  attachment, a browser download) are not on the clipboard as paths and need OLE; they are
  not built.

## Video

- **Decode is on the CPU by default.** OpenH264 decodes the 4:2:0 stream; a D3D11
  shader converts to RGB. H.264 levels up to 5.2 decode; a level above that makes
  every access unit fail, and `gwin-verify` reads the SPS to say so.
- **The swap chain is flip-model,** two buffers, with `Present(1, 0)`; a lost device
  makes a new presenter on the same window.
- **GPU decode is a choice per machine.** It uses the Windows H.264 decoder
  (Media Foundation) with D3D11 video and falls back to OpenH264 on its own; see
  [settings.md](settings.md#gpu-decode).

### Measured on the Windows machine

Three runs on the Windows 11 machine, with one high-DPI laptop screen at 200%,
against a real `gliff-server` on the remote machine over a LAN. The result files
are not part of this repository, and the exact build of the client that ran is
not recorded. The CPU and GPU of the machine are not recorded either.

**Decode of a 3840x2400 clip.** `gwin-verify decode --kill-rule`
with libx264 High level 5.2 clips of a synthetic desktop, 300 frames each, 0 errors. The
limit is p95 at most 33.3 ms (30 fps):

| Size | p50 ms | p95 ms | max ms | decode fps |
|---|---|---|---|---|
| 1920x1080 | 4.33 | 6.53 | 35.23 | 196.0 |
| 2560x1440 | 7.32 | 11.24 | 46.14 | 116.9 |
| 3840x2400 | 18.42 | 31.09 | 92.36 | 45.5 |

The margin at 3840x2400 is 2.2 ms of p95, which is thin. The first access unit
(the IDR) is slow and sits above p95, and a short clip makes p95 worse.

**A real 30-second stream at 3840x2400.**
`gwin-verify stream --host <target> --size 3840x2400 --seconds 30`, second run.
The first run failed on its placement probe and on the fps rule (29.03 fps against
30.0); run 2 compares with 0.95 of the cap, and this is run 2. The server capped at
30 fps; H.264 High level 5.2:

| Measure | Value |
|---|---|
| Frames in the 30 s window | 870 pictures, 0 missing, 0 decode errors |
| Rate | 29.0 fps against a cap of 30 (the check passes at 0.95 of the cap) |
| Decode | p50 5.69 ms, p95 7.17 ms, max 16.18 ms |
| Longest gap, stalls | 43.39 ms, 0 stalls |
| Over the whole run | 1022 frames read, 1022 acks |
| Pattern check | 32 of 32 cells matched, mean error 0.69 (limit 40) |

The stream decodes in a third of the time of the clip because it is a different
picture; these two numbers are not comparable, and this page does not say why.

**The window: present.**
`gwin-verify present --host <target> --client .\gliff-windows.exe --seconds 20`, second run, 3840x2400
at 200%, flip-model swap chain, 2 buffers, no tearing, `Present(1, 0)`, frame latency 1. The
source was the 60 fps moving pattern; the server cap was 60:

| Stage | Measured | Presented / decoded | Dropped in the slot | Presented fps | Latency p50 / p95 / max | Longest gap |
|---|---|---|---|---|---|---|
| Windowed, 2560x1600 | 20.95 s | 1175 / 1187 | 12 | 56.08 | 0.5 / 1.0 / 10.05 ms | 200 ms |
| Full screen, 3840x2400 | 21.33 s | 1127 / 1127 | 0 | 52.85 | 0.9 / 1.0 / 2.01 ms | 37 ms |

Latency here is from the decoded picture to `Present` returning; the gate is
p95 at most 16.7 ms. A resize to 2560x1600 at scale 2 was applied 69 ms after it was
sent and showed a picture after 115 ms; the resize at the switch to full screen
took 68 ms and 137 ms. The session had 2883 frames and 2883 acks, 0 decode
errors, decode p50 3.3 ms, p95 7.1 ms, max 31.6 ms, and 13 pictures replaced in the
slot before they were shown. Both stages decoded below the cap of 60, which the
check reports as a warning: gliff sends frames on damage, so these rates measure
the source as much as the client. The first run of this check failed with 1.84
presented fps on a desktop with nothing moving, which is why the check needs the
moving pattern.

**Not measured:** the delay from a server frame to the pixel on screen, CPU and
GPU load, memory use, anything over Wi-Fi or a WAN, a second Windows machine, and
the time to copy a clipboard item of any kind. The clipboard gate checks that
data arrives and is correct, not how long it takes.

## Server facts that matter here

- The server must be Gliff v0.4.1 or later. Some drivers refuse the Vulkan Video
  encode profile when it carries usage hints; the fix is upstream pull request #86,
  in v0.4.1. Without it the server falls back to CPU encoding, which stops at
  3840x2160.
- A headless output starts clamped to 1920x1080 until the first Resize, and a
  window shows there only after it is moved.
- A mirrored output is the other way round: it keeps its own size and scale, and
  the server only scales the stream down to fit the window, never up
  (`fit_mirror`, `gliff-server/src/session.rs:718-742`). A window larger than the
  remote screen shows it zoomed, and the server follows a change of the mirrored
  output on its own (`session.rs:767-800`). The client's default mirrors the
  focused screen; `--headless` asks for the private one the rest of this page
  describes.
- `--output auto` can pick a leftover private screen. The server takes the
  focused enabled output, else the leftmost enabled one, with no filter for a
  headless output (`session.rs:1777-1784`), and removes the private screen it
  created only when it exits on its own (`SessionOutput` drop,
  `session.rs:1575-1612`; the screen is named `gliff-<pid>`, `session.rs:1692-1710`).
  A killed server leaves it behind, and it can win `auto`. Name the display with
  `--output NAME`. This is separate from the mode loss after a compositor reload.

**Why 0.4.1: a measurement on the remote machine.** A mirror of a real
2560x1440 output at 59.95 Hz on localhost, `gliff-probe stream-bench --no-decode`,
motion from a fullscreen test pattern, no client and no ssh. The remote machine
had an older discrete GPU. This is a server-side measurement and no Windows is
involved:

| Server | Result |
|---|---|
| v0.4.0, default | 20.7 fps, 14.8 Mbit/s, 0 stalls; CPU encode because VA-API failed to initialise |
| v0.4.0, `--video cpu` | 20.4 fps, 6 stalls |
| v0.4.0, `--capture shm` | 20.7 fps, 0 stalls |
| v0.4.0 with a VA-API driver package for the GPU installed | 21.7 fps; VA-API opened but the Vulkan capabilities refused the usage hints, so CPU again |
| v0.4.0 with a build that drops the usage hints | **56.3 fps**, 41.8 Mbit/s, 0 stalls, Vulkan Video encode |

The last row is the basis of the fix that shipped in v0.4.1 (upstream pull request #86). The logs are not part
of this repository.

## The remote compositor reloads

This is a fault of a private (headless) screen: the mode a reload discards is
one the server set with a runtime keyword. A mirrored screen is a real output
whose mode comes from the compositor's own config, so the client's resync
(below) runs only for a private screen. That a mirrored session needs no resync
is reasoned from the server's code, not yet tried on hardware.

A Hyprland config reload on the remote machine (`hyprctl reload`, which many
desktop tools run when they apply a theme) leaves a headless session with a
window that shows nothing new and does not react. The link stays up: no
`Error`, no end of session, no `StreamConfig`. Closing and reopening the client
gives a good session.

**Cause, in server v0.4.1 (`crates/` paths are upstream's).** The server sets a
headless output's mode with `keyword monitor` (`hypr-ipc/src/lib.rs:214`), a
runtime keyword that a reload discards. After the reload the output is back at
the compositor's default mode (1920x1080 at the default scale), while the
server's record of the output (`SessionOutput` width, height and scale,
`gliff-server/src/session.rs:1655-1776`) still holds the mode the client asked
for. Nothing watches the compositor's events, so the record is never
corrected. `matches_output` (`session.rs:924-926`) compares each captured
frame's extent with that record, and `pump_encoder` (`session.rs:861`, `:873`)
drops every frame that does not match, so no picture is encoded and no message
tells the client why. The mismatch handling in `on_capture`
(`session.rs:767-800`) runs only for a mirrored real output, not a headless
one.

A Resize with the size and scale the client last asked for cannot repair it.
The client gates it (`ResizeGate`, and `Core`'s `last_resize`), and the server
returns early when the size and scale equal its own record (`session.rs:665-667`).
The upstream Linux client has the same gate (`crates/gliff/src/main.rs:2191-2219`)
and no stall detection (`net.rs`), so it is affected the same way.

**What the client does about it.** It cannot see the compositor, so it
watches for the symptom: input that gets no answer. When a key press, a button
press or a wheel message has gone out, at least three have gone out unanswered, and
no `VideoFrame` or `StreamConfig` has arrived for 3 s, the session sends a
*resync*: a Resize to the same size less 32 px of width (more when the window
is under 352 px wide), then at once the real Resize again. A size that differs
from the server's record makes it set the mode again and send a new
`StreamConfig` (`session.rs:649-715`), so the output matches its record
again and frames flow. The window title reads "gliff - re-syncing the display"
until the first picture after it arrives, and the log says why and counts it. At most one
resync runs per 30 s. With several sessions each has its own watch and limit,
and the title shows the state of the shown session only; the window logs
`session <n>: resync: started` and `... pictures are flowing again` (the
driver's own `resync:` line with the counters comes just before the first).

**What it does after a stall.** The remote may have missed a key's Up while it
stalled or while the keyboard hook was installed again, and then it goes on
holding that key (a stuck Win, Ctrl, Alt or Shift). So right after a resync,
and after the hook is installed again while the keyboard is grabbed, the client
tells the remote that every Win, Ctrl, Alt and Shift key (left and right) is up,
except the ones the user is holding at that moment. The log says so with
a count only, for example `resync: told the remote Win, Ctrl, Alt and Shift are
up, except the 1 key(s) held`; it never names a key.

- A static remote screen sends no frames, so silence alone is not a stall. That
  is why the trigger needs input, and three inputs, not one: a click on a quiet
  desktop must not re-size anything.
- A resync that was not needed costs a 32 px change of the remote output width
  for under a second (a terminal may reflow by a column or two) and a decoder
  reset. It is rate limited so that cannot repeat.
- A stalled stream is re-synced, not reconnected: a new session would remove the
  headless output and move every window on it. A session that died is retried
  (see Reconnect in architecture.md). If a resync does not help (the log shows
  `resync` lines and still no pictures), close and reopen the client.
- The fix for the cause is in the server (watch the output's real mode and
  apply it again, or correct its record and send a `StreamConfig`). The client's
  resync is a workaround until then.

Tested against `gwin-fake-server` with `GWIN_FAKE_SERVER=reload`, which models
the above: after a few seconds it stops sending frames and ignores a Resize
that equals its record, and recovers on one that differs
(`cargo test -p gwin-verify --test driver`).

### Scale picked on the remote

Own screen sizes the remote screen to the window and gives it the client's scale (the Windows display scale, `dpi / 96`). A scale chosen in the remote desktop's own display settings does not stick. The likely cause is that the desktop's scale menu rewrites its monitor config and the compositor reloads it, which discards the mode the server set (the reload failure above); the remote's own logs were not read. The picture goes black until the client resyncs, which takes at least 3 inputs, 3 s of silence and the 30 s gap between resyncs. The resync then sends the client's scale, so the remote screen is back at the Windows scale. In the log, every stream config after each resync says the Windows scale, and the client sends no Resize of its own.

To change the scale, change the Windows display scale and reconnect. A client setting that sends a chosen scale is in the backlog (see [KNOWN-ISSUES.md](../KNOWN-ISSUES.md)). Not tried: whether the pointer and the layout are right with a remote scale that differs from the Windows scale.

## Known limitations

- **A click between the fake Ctrl and the right Alt of an AltGr press** carries the
  fake Ctrl. The two events come from one keystroke, microseconds apart. Not
  changed: holding the Ctrl back for a click would put a real Ctrl+click out in
  the wrong order. Once the right Alt is in, a click gets no Ctrl (tested).
- **Ctrl+Alt+Del and UAC** stay with Windows, as above.
- The other limits of the clipboard are the
  [known gaps](clipboard.md#known-gaps).
