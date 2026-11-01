# Settings and the config folder

The client reads its settings from three places. For each setting the first of these that has a value wins:

1. a command-line flag,
2. the `[settings]` table of `config.toml`,
3. the built-in default.

The same rule holds for every setting, with one addition: `screen` and `share_clipboard` can also be chosen for one machine, in that machine's tab menu, and `sharp_colour` is chosen there only. A machine's choice sits between the flag and the `[settings]` table (see [Connection choices for one machine](#connection-choices-for-one-machine)). The client writes `config.toml` when the tab set changes and when a connection choice is made in a tab menu. Every other setting is edited by hand.

## The config folder

| Order | Source |
|---|---|
| 1 | `--config-dir DIR` |
| 2 | the environment variable `GLIFF_WINDOWS_CONFIG_DIR` (an empty value counts as unset) |
| 3 | `%APPDATA%\gliff-windows\` |

The folder holds two files:

- `config.toml`: the `[settings]` table, and the machine list (`machines`, `open`, `shown`; see below).
- `recent-hosts.toml`: the recent hosts history, kept apart so that a damaged file in one cannot lose the other.

Logs and the clipboard spool stay under `%LOCALAPPDATA%`. The client keeps the newest 14 daily log files (`gliff-windows-YYYY-MM-DD.log`, each stopping at 4 MiB) and deletes older ones from the log folder when it starts; other files there are left alone. The client never writes the Windows registry for settings or history. Tests and checks pass their own folder, so they never touch a real one.

## What a file does

- **Loading is tolerant.** Unknown keys are ignored. A missing key takes its default. A key with a value that does not parse is ignored on its own, and the client logs it. `local_chords` may be text (`"win+shift+s,win+v"`) or a list (`["win+shift+s", "win+v"]`). A file that is not valid TOML gives the defaults and a log line.
- **A save is the client's state replacing the file, and no text a person wrote is lost.** The client reads the file again at every save, whatever it found at the load, because a person or another client may have changed it since. If the text it is about to replace is not what it loaded, or loading had to repair or drop something, or the text is not in the layout the client writes (comments, spacing, key order, another spelling of a value), it first keeps that text as a numbered copy next to the file, and then writes. A file the client itself wrote has its own layout, so it is not copied again. A file that is not valid TOML (or is not text, or is over 1 MiB) is kept as `config.toml.broken` (or `recent-hosts.toml.broken`). A file that is valid but changed after the load (a hand fix, another client), or that loading repaired (for example a machine entry that is not a target, or a `settings` that is not a table), or that holds comments or another layout, is kept as `config.toml.replaced` (or `recent-hosts.toml.replaced`). The client's own last write is not copied again. The state the client writes is the state it holds: the file is not merged with what a person typed, and the copy is how to get that text back. A copy is created exclusively and never replaced: the next name is `.1`, then `.2`, up to `.99`. A save that fails and is tried again with the file unchanged uses the copy it already made, so five failed saves leave one copy, not five. The file is read once more just before the rename; if someone wrote in between, that text is kept as well (a small window remains between that read and the rename, which no file system call closes). When every name is taken, or a copy cannot be made, the save fails with a message and the file stays as it is.
- **A file that cannot be read is left alone.** A read that fails for another reason (for example another program holds the file) is not damage. The run uses the defaults and the load notes it. A save then writes only if the file is missing, or can be read and is valid TOML (it is kept as a `.replaced` copy first). If the file can be read and is damaged, or cannot be read still, the save fails with a message and the file stays. Once the lock clears, the next save works.
- **Saving is atomic.** A save writes a temporary file next to the file (`config.toml.<process id>.<number>.tmp`, so two saves never share a name) and renames it over. The rename is tried up to 5 times, 50 ms apart, only for an error that can go away by itself (another program holds the file, a permission); any other error fails at once. A permission that is still refused after the tries, as for a read-only file, is reported as an error that will not go away by itself, so a caller does not try again in a loop; a lock is reported as one that may (on Windows: sharing and lock violations, a delete pending, a user-mapped section). A failed save removes its temporary file and leaves the old file; a temporary file left by a crash or by a failed removal is not cleaned up and nothing reads it. A text over 1 MiB is not written, because the next load would call it damaged: the save fails and the file stays. On Windows a read-only `config.toml` cannot be replaced, and the save fails with an access error. What the client holds is updated only when a write succeeded, so a failed save can be tried again.
- **Keys the client does not know are kept.** `config.toml` keeps its other top-level keys and tables, unknown keys in `[settings]`, and a known setting whose value did not parse, through a save. A bad value stays until the setting is given a value or reset (`ConfigFile::reset_setting`). Comments and the order and layout of the text are not kept: the file is written again from the parsed values; the `.replaced` copy kept before the first save holds the original text. `recent-hosts.toml` holds the `hosts` list only: a save writes that and nothing else, and another key it had is in the copy.
- **Only choices are saved.** A save writes the settings the user chose, not the defaults, and never the hidden test flags (`--test-panic`, `--hook-stall-test-ms`, `--stats-out`).

## The `[settings]` keys

Each key has the spelling of its flag, with `_` for `-`. A flag can only switch a yes/no setting on (`--forward-media-keys`, `--clipboard-history`) or share the clipboard off (`--no-clipboard`); to turn a yes/no setting off that the file turned on, edit the file.

The class says what a change needs. **Live** takes effect without a restart, **Reconnect** at the next connection, **Restart** at the next start. The client reads every setting once at start today; the class is what the later settings window will act on. That window is a design only; it is not built (the tab menu's connection choices are the first part of it that is), and it would change where these keys live (shared options move to `[defaults]` and `[overrides]`, the Windows-only ones to `[windows]`, and this table is migrated on read).

| Key | Default | Class | Flag |
|---|---|---|---|
| `release_chord` | `ctrl+alt+home` | Live | `--release-chord` |
| `grab_follows` | `focus` (or `hover`) | Live | `--grab-follows` |
| `grab_timeout_secs` | not set (1 to 86400) | Live | `--grab-timeout-secs` |
| `injected_keys` | `forward` (or `local`) | Live | `--injected-keys` |
| `forward_media_keys` | `false` | Live | `--forward-media-keys` |
| `local_chords` | `win+shift+s` (a list, or `none`) | Live | `--local-chords` |
| `side_buttons` | `upstream` (or `linux-mouse`) | Live | `--side-buttons` |
| `super_l_chord` | `ctrl+alt+l` | Live | `--super-l-chord` |
| `switch_next` | `ctrl+alt+pagedown` (or `none`) | Live | none |
| `switch_previous` | `ctrl+alt+pageup` (or `none`) | Live | none |
| `switch_jump` | `ctrl+alt` (the modifiers of tab 1 to 9, or `none`) | Live | none |
| `keep_in_tray` | `false` | Live | none |
| `remember_recent_hosts` | `true` | Live | none |
| `clipboard_history` | `false` | Live | `--clipboard-history` |
| `share_clipboard` | `true` | Reconnect | `--no-clipboard` |
| `server_path` | `gliff-server` | Reconnect | `--server-path` |
| `ssh` | `%SystemRoot%\System32\OpenSSH\ssh.exe` (`ssh.exe` if that file does not exist) | Reconnect | `--ssh` |
| `screen` | `mirror` (or `own`) | Reconnect | `--headless` (sets `own`) |
| `output` | not set (the focused screen) | Reconnect | `--output` |
| `super_l` | `chord` (or `registry`, `off`) | Restart | `--super-l` |
| `log_dir` | not set (`%LOCALAPPDATA%\gliff-windows\logs`) | Restart | `--log-dir` |
| `adapter` | `monitor` (or `high-performance`, `minimum-power`) | Restart | `--adapter` |

`switch_next`, `switch_previous` and `switch_jump` (the modifiers of the position chords) are used: they are the chords the hook watches for switching tabs, and the overlap check keeps them apart from the release chord and the local chords. `remember_recent_hosts` is used: a machine that connects is added to `recent-hosts.toml` when it is on. `keep_in_tray` is read, checked and shown, but nothing uses it until the tray exists.

An example:

```toml
[settings]
release_chord = "shift+esc"
grab_follows = "hover"
local_chords = "win+shift+s,win+v"
switch_next = "none"
screen = "own"
```

## The machine list

The top of `config.toml` can hold the machines the user keeps as tabs. Each is a target, `user@host` (or a bare host or alias), as on the command line:

```toml
machines = ["user@one", "user@two"]
open = ["user@two"]
shown = "user@two"
```

`machines` is every machine remembered, in the user's order. `open` is the ones to reconnect at start, in tab order. `shown` is the one that was in front. The file is repaired on load: an entry that is not a target is dropped, a repeated entry is kept once, an open machine that is not in `machines` joins it, and a `shown` machine that is not open is not shown. Each repair is a note, which is logged and printed by `--print-settings`. An empty list is not written. The client reads this list at start, reopens the machines in `open`, shows `shown`, and offers `machines` in the address bar's list.

## Which remote screen the window shows

`screen` chooses what the server puts in the window, and it is part of the server's command line, so a change applies at the next connection.

| Setting | Server command line | What the window shows |
|---|---|---|
| `screen = "mirror"` (the built-in default) | `--output auto` | The remote machine's focused screen, fitted into the window. The remote screen keeps its size; the picture is scaled down to fit and never up, and keeps its shape (bars at the sides or the top). This is what the Linux client does by default. |
| `screen = "own"`, or the flag `--headless` | `--headless` | A private screen that the server creates and sizes to the window. |
| `output = "NAME"`, or the flag `--output NAME` | `--output NAME` | The named remote screen, fitted like the default. It beats `screen`. |

- **An existing file keeps what it says.** A file that sets `screen` keeps that value, and a file that sets `output` keeps mirroring that screen. A key that is absent now means `mirror`, so a file that sets neither used to give a private screen and now gives the focused screen. To keep the private screen, add `screen = "own"` under `[settings]` or start with `--headless`.
- **`--headless` beats the file.** It also overrides an `output` in the file. It cannot be combined with `--output` on the same command line.
- **What differs while mirroring.** The remote screen's own size and scale decide the picture, not the window. The first picture is the remote screen's size (a private screen starts at most 1920x1080), and the server follows the focused screen on its own, so there is nothing to recover when the remote compositor reloads and the client does not run its resync. A window larger than the remote screen shows it zoomed.
- **Edge case: `auto` can pick a leftover private screen.** The server's `auto` is the focused real output, else the leftmost enabled one, and it does not skip a private screen (`gliff-` and a number) that an earlier session left behind. That happens when a server is killed instead of ending cleanly, because the private screen is removed only when the server exits on its own. If the window shows an empty screen, name the display with `--output NAME` or `output = "NAME"` (the names are the remote compositor's, such as `DP-1` or `HDMI-A-1`), or use `--headless`. The client does not work around this; a fix belongs in the server.
- **Own screen follows the Windows display scale.** The client asks for a private screen at the size of the window and at the scale of the Windows display (200% is a scale of 2). So the scale of that screen is set on Windows: change the Windows display scale, then reconnect. Changing the scale of that screen in the remote desktop's own display settings (for example a scale menu in the desktop environment) is not supported. The remote compositor reloads its config, the picture goes black for 10 seconds or more, and the next resync puts the client's scale back. Choosing a scale in the client is not built yet (see [KNOWN-ISSUES.md](../KNOWN-ISSUES.md)). The client was tried at 200% and 150% only; see [windows-quirks](windows-quirks.md#pointer-and-dpi). Mirror has none of this: the remote screen keeps its own scale.
- **The tab menu offers the same choice per machine**, as **Connection settings, Show as: Mirror or Own screen** (below). The flag and the file keys still work, and the flag beats the menu.

## Connection choices for one machine

Right-click a tab and open **Connection settings**. It holds five choices for that machine only:

- **Show as:** *Mirror the remote screen* or *Own screen*. This is the `screen` setting.
- **Share clipboard:** ticked or not. This is the `share_clipboard` setting.
- **Sharp colour:** ticked or not, off by default. This is the `sharp_colour` key, which exists only in a machine's table ([below](#sharp-colour)).
- **GPU decode:** ticked or not, off by default. This is the `gpu_decode` key, which exists only in a machine's table ([below](#gpu-decode)).
- **Send Win+L to this machine:** ticked or not, off by default. This is the `send_win_l` key ([below](#send-winl-to-this-machine)).

A choice is saved at once. **Send Win+L to this machine** applies at once and never needs a reconnect. **Share clipboard** applies at once to the tab's session: turned off, nothing is copied or pasted either way, and if what that machine copied is still on the Windows clipboard it is taken off (anything copied on Windows since is left alone); turned back on, sharing resumes in the same session. A session that started without sharing has no clipboard to resume, so ticking it reconnects that tab. **Show as**, **Sharp colour** and **GPU decode** are Reconnect-class: they apply the next time the tab connects, and the menu says so under the choices. If the tab has a session and a saved Show as, Sharp colour or GPU decode choice differs from what that session started with, the line is replaced by **Reconnect now to apply**, which does what **Reconnect** does: it ends the session and starts a new one with the saved choices. A row that a flag or the file has locked never counts as a change. If you change a choice and then change it back, the line stays as it was. A tab with no session (stopped, waiting to retry, or given up) keeps the line, because its next connect applies the choice anyway. There is no settings window and no OK button.

**Where it is saved.** In `config.toml`, in one table per machine, named by the target as it is written in `machines`:

```toml
[overrides."user@host"]
screen = "own"
share_clipboard = false
sharp_colour = true
gpu_decode = true
send_win_l = true
```

A table holds only what the user changed for that machine, so a machine that was never changed has no table. The table is a layer between the flag and `[settings]`:

1. a command-line flag,
2. the machine's `[overrides."user@host"]` table,
3. the `[settings]` table,
4. the built-in default.

The same table name and keys are the ones the design for the later settings window uses (`[overrides."user@host"]` over `[defaults]`; the older `[settings]` table is migrated on read), so the file does not need to change when that window comes. The host part is compared without regard to case, as everywhere else. A machine that is forgotten keeps its table, so its choices are there if it is added again. Keys in a table that the client does not know, and a known key whose value does not parse, are kept through a save, like the keys of `[settings]`. A choice made in the menu is saved as written, even when it equals the default.

**A flag locks the choice.** A flag applies to every machine, so the menu cannot override it. The choice is shown greyed, with the reason on a line below:

| Locked by | Show as | Share clipboard | Send Win+L |
|---|---|---|---|
| `--headless` | Own screen, locked | | |
| `--output NAME` | Mirror, locked | | |
| `--no-clipboard` | | Off, locked | |
| `share_clipboard = false` in `[settings]` | | Off, locked (the clipboard is not started, so it is off for every machine) | |
| `--super-l registry` | | | On, locked |
| `--super-l chord` or `--super-l off` | | | Off, locked |

The reason is one plain sentence, one of these:

- `Locked: this window was started with --headless, which applies to every machine.`
- `Locked: this window was started with --output, which applies to every machine.`
- `Locked: this window was started with --no-clipboard, which turns the clipboard off for every machine.`
- `Locked: config.toml has share_clipboard = false, which turns the clipboard off for every machine.`
- `Locked: this window was started with --super-l, which applies to every machine.`

A locked row changes nothing and saves nothing. A choice saved earlier stays in the file and applies again when the flag is not given. `output = "NAME"` in `[settings]` does not lock: choosing *Mirror* for a machine mirrors that named screen, and choosing *Own screen* gives a private screen.

## Send Win+L to this machine

Windows handles Win+L itself, before any program sees it, so by default Win+L locks Windows even while the window has the keyboard, and `Ctrl+Alt+L` (`super_l_chord`) sends Super+L instead. With **Send Win+L to this machine** on, a real Win+L goes to that machine. The menu row says what it costs: *Windows lock shortcuts are off while this window has the keyboard*.

- **When locking is off.** Only while the window shows a tab whose switch is on and the window has the keyboard. A pointer resting on the window without the keyboard does not turn locking off. Windows locking is back 300 ms after the window loses the keyboard, and at once when the switch is turned off, another tab or no tab is shown, or the computer goes to sleep. While locking is off, the Lock entries in the Start menu and on the Ctrl+Alt+Del screen do nothing either. If this window cannot turn locking off (another gliff window holds the setting, the registry refused it, or its helper did not start), the menu says so under the row and Win+L locks Windows.
- **How.** The per-user `DisableLockWorkstation` value, with a marker, a watchdog helper and a repair at start, so an exit leaves it as it was, and a crash or a kill leaves it set at most until the next start (or `--restore-lock-policy`). See [Win+L and the lock setting](windows-quirks.md#winl-and-the-lock-setting).
- **Applies at once.** Ticking or unticking takes effect without a reconnect or a restart, and the menu never offers *Reconnect now to apply* for it.
- **With the file and the flag.** The order is the usual one: the `--super-l` flag (locks the row: `registry` on, `chord` or `off` off), the machine's `send_win_l`, then `super_l = "registry"` in `[settings]` (on for every machine with no choice of its own), then off.
- **The chord still works.** `Ctrl+Alt+L` sends Super+L whether the switch is on or off, unless `super_l` is `off`.

## Sharp colour

H.264 video normally carries colour at half the resolution in each direction (4:2:0), so the edges of coloured text are a little soft. With **Sharp colour** on, the client offers full-resolution colour (4:4:4) as well as 4:2:0 when the tab connects. The server then sends two 4:2:0 streams per frame, and the client decodes both and puts the full colour back together before it draws the frame. Off, the client offers 4:2:0 only, exactly as without the choice.

- **The server decides.** The client offers; the server picks. A server that encodes on the GPU picks 4:4:4. A server that encodes on the CPU picks 4:2:0, because the second stream doubles its encode work, unless it runs with `--full-chroma`. A server started with `--low-bandwidth` always picks 4:2:0.
- **Passing `--full-chroma`.** `server_path` takes one program, not a command line. To add the flag, put a small script on the remote machine and point `server_path` at it:

  ```sh
  #!/bin/sh
  exec gliff-server --full-chroma "$@"
  ```

- **Slow links.** The server can drop back to 4:2:0 during a session when the link is short of room, and go back to 4:4:4 when there is room again. Each switch starts with a full frame, so the picture does not glitch.
- **Cost.** The client decodes two streams instead of one. On dense coloured text that took two to four and a half times the decode time per frame, the server sent two to fourteen times the bytes (most when scrolling), and the frame the client uploads to the GPU is twice the size. [architecture.md](architecture.md#measurements) has the numbers.
- **Which one you got.** Each `stream config` line in the log ends with the chroma the server picked (`Dual420` or `Single420`) and where it encodes (`encoder gpu` or `encoder cpu`).
- **Whether it reaches the screen.** While Sharp colour is on, each session logs a `sharp colour` line about every 5 seconds while pictures arrive. It counts the frames received, how many carried their second (aux) half, the pictures shown at 4:4:4 and at 4:2:0, and decode errors by kind: `out of step` (one half gave a picture and the other did not), `size mismatch`, or `other`. On a working Dual420 stream, `shown 4:4:4` follows `with aux` and every other count stays at zero.
- **No flag and no `[settings]` key.** The choice is per machine only, and nothing locks it.

## GPU decode

With **GPU decode** on, the client decodes the H.264 video on the GPU instead of on the processor. It uses the Windows H.264 decoder (Media Foundation) with Direct3D 11 video acceleration, on the default graphics adapter. Each decoded picture is copied back from the GPU and drawn exactly as before, so the picture is the same pixel for pixel: H.264 decoding is exact, and the check in [gwin-verify](gwin-verify.md#decode) compares every picture with the processor's. Off, the client decodes on the processor (OpenH264), exactly as without the choice.

- **Applies at the next connect.** Like Sharp colour, the choice is read when the tab connects, and **Reconnect now to apply** applies it at once.
- **Nothing changes on the server.** The client offers the server the same things either way. The server cannot tell which decoder the client uses.
- **Falls back to the processor on its own.** If the GPU has no H.264 decoder, the Windows decoder is missing (some N editions of Windows ship without it), or it decodes in software only, the session decodes on the processor from the start. If the GPU fails during a session (for example a driver update or a removed device), the session switches to the processor for the rest of that session and asks the server for a full frame, so the picture holds still for a moment and then goes on. Either way the log says why in one line.
- **Sharp colour works with it.** With Sharp colour on and a 4:4:4 stream, the client runs two GPU decoders, one for each half of the stream. If either fails, both halves go back to the processor together.
- **Which decoder runs.** When GPU decode is on, each session logs one `decoder` line when it starts, such as `decoder: gpu (Media Foundation, D3D11 video)`, or `decoder: cpu, GPU decode unavailable: <reason>`. While pictures arrive it logs a `decode` line about every 5 seconds with the decoder, the pictures decoded and the decode time per picture (median and 95th percentile). A switch during a session logs `decoder: switched to cpu, GPU decode failed: <reason>`. With GPU decode off none of these lines appear.
- **To report a bug** in the picture, untick GPU decode and reconnect; if the bug goes away, say so in the report.
- **Cost.** Decoding on the GPU takes the decode work off the processor, but each picture is still copied back to the processor and uploaded again to draw. [architecture.md](architecture.md#measurements) has the numbers.
- **No flag and no `[settings]` key.** The choice is per machine only, and nothing locks it.

## Chords that overlap

A change that would let two chords fire on the same key press is an error at start, not a silent pick. The check covers the release chord, the Super+L chord (unless `super_l` is `off`), the three switch chords and the local chords.

- The release chord, the Super+L chord and the switch chords fire when their last key goes down and their modifiers are held. Extra modifiers do not stop them. So two of these with the same last key overlap, whatever their modifiers: `ctrl+1` and `alt+1` both fire on `ctrl+alt+1`.
- A local chord fires only when the held modifiers are exactly its own. It overlaps another chord only when they share the last key and the other chord's modifiers are among the local chord's.

The message names both chords, where each value came from (a flag, the file or the default), and the flag or file key to change.

## `--print-settings`

`gliff-windows --print-settings` prints the settings as JSON and exits. It starts no session, so it takes no target, and it changes nothing on disk or in Windows. The JSON holds the config folder (and whether it is the default one), each setting with its value, where the value came from (`flag`, `file` or `default`) and its class, the machine lists (`machines`, `open`, `shown`), the saved connection choices, the recent hosts, and the notes from loading. Its keys are `config_dir`, `config_dir_is_default`, `settings` (each key holding `value`, `source` and `class`), `machines`, `open`, `shown`, `connection`, `recent_hosts` and `notes`. `connection` has one entry for each machine that is listed or has a saved choice, named by its target. Each entry has `screen`, `share_clipboard`, `sharp_colour` and `gpu_decode`, each with the `value` that machine runs with, its `source` (`flag`, `machine`, `file` or `default`) and `locked` (`true` when a flag, or `share_clipboard = false` in the file, decides it for every machine); a saved choice shows as source `machine`. Chords print in lowercase, such as `ctrl+alt+pagedown`. The client is a Windows program without a console: redirect the output to a file or a pipe to read it.

## Recent hosts

`recent-hosts.toml` holds one list, `hosts = ["user@host", ...]`, most recent first. It stores the target string only: no password, key, port or time. The list keeps at most 10 hosts and one entry per machine; a repeat moves to the front and keeps its newest spelling, and the host part is compared without regard to case. Every line is checked on load, and dropped with a note if it is not a target. A target is `user@host` or a bare host or alias. The user is everything before the last `@`, so it may hold an `@` of its own (`user@domain.com@host`), a `\` (`DOMAIN\user@host`) and letters of any script (`Müller@host`). It is refused if it is empty, starts with `-`, or holds whitespace, a control or invisible character (a zero-width or direction-changing one), or one of `$` the backtick `; | & < > " ' ( ) { } [ ] * ? ! # ^ % : = / ~`. The host is a name or alias of letters of any script, digits and `._-` (`me@münchen.de`), or an IPv6 address (a `:` is allowed only there, so the host must parse as an IPv6 address) with at most one `%` and a zone of letters, digits and `._-` after it; it does not start with `-`. The whole target is at most 253 bytes. A port (`host:22`), a bracket, a `%` outside an IPv6 address (`user@%APPDATA%`, `a%`), a quote or a space is refused. The same rules apply to the target on the command line: it is parsed once, and the program that runs ssh gets the parsed value after a `--`, as one argument, on every path that runs ssh.

  **Narrowing, on purpose:** a `ssh://user@host` URL and a `host:port` are refused, though ssh itself reads a URL with a port. The error for a URL says to leave out the scheme. The client keeps one `user@host` text to compare, list and show, and a port, a jump host or any other option belongs to ssh's own configuration (a `Host` entry in the ssh config file), which an alias then names.

The zone is compared as written, the rest without regard to case. A machine that connects is recorded. With `remember_recent_hosts = false` the client reads no list, records nothing, and leaves the file as it is.
