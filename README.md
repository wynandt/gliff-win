# gliff-win

A Windows client for [Gliff](https://github.com/omacom/gliff), the remote
desktop for Hyprland. gliff-win shows the Hyprland session of another machine
in a normal, resizable Windows window, with one tab for each machine. All it
needs between the two machines is ssh: no open ports, no accounts, no relay.

This is version 0.2. It works and is used daily, and has been tried on one
set of hardware. See [Not in 0.2](#not-in-02) and
[Verification](#verification).

## Status and contributing

gliff-win is a personal project that I use daily and share as-is. It is MIT
licensed. I fix what breaks for me and merge good changes when I have time.
There is no support and no promised response time.

Bug reports with a clear repro are welcome. Small, focused pull requests are
welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). Report security problems
privately through the Security tab, see [SECURITY.md](SECURITY.md).

## What you get

- **A tab for each machine.** The tab strip shows the full machine name, for
  example `user@203.0.113.7`, and a dot for the state of the session. Drag a
  tab to move it. You can open as many as you like and switch with a chord.
- **An address bar.** Type `user@host` or a name from your ssh config and press
  Enter. A list button shows the machines you used before.
- **A panel when a connection fails.** It says in plain words what went wrong,
  offers the buttons that make sense (try again, edit the address, details),
  and links to [docs/ssh-keys.md](docs/ssh-keys.md) when a key is the problem.
- **The remote screen, fitted into your window.** By default the window
  mirrors the remote machine's focused screen, as the Linux client does, and
  the picture keeps its shape. `--headless` asks the server for a private
  screen that follows your window instead, and `--output` mirrors a named
  screen.
- **Sharp video.** H.264 decoded on the CPU by default and presented with Direct3D 11 at
  vsync. *Sharp colour*, a choice per machine, asks for full-resolution
  colour (4:4:4), so coloured text stays crisp. *GPU decode*, another choice
  per machine, decodes on the GPU and falls back to the CPU on its
  own ([details](docs/settings.md#gpu-decode)). The strip's fullscreen button, the window menu or `F11` (while Windows has
  the keyboard) toggles borderless fullscreen.
- **Keyboard and mouse.** Every key, including Super chords, goes to Hyprland
  while the window has the keyboard. Your Windows layout is sent as an XKB
  keymap. A mouse wheel scrolls in notches; a precision touchpad scrolls
  smoothly. `Ctrl+Alt+Home` gives the keyboard back to Windows.
- **Clipboard.** Copy and paste text, images and files in both directions.
  Nothing is sent until something pastes. Remote copies stay out of Win+V
  history and cloud sync.
- **ssh is the only transport and the only authentication.** The server opens
  no port, and the client never asks for a password.
- **The stock `gliff-server` and upstream's protocol**, unchanged.

## Requirements

- **Windows machine:** Windows 11 with the OpenSSH client (`ssh.exe`), and an
  ssh key (for example in ssh-agent) that the remote machine accepts without a
  prompt. [docs/ssh-keys.md](docs/ssh-keys.md) has the commands. Windows 10
  has not been tried.
- **Remote machine:** Hyprland with a running session, `sshd`, and
  `gliff-server` from [omacom/gliff](https://github.com/omacom/gliff),
  **v0.4.1 or later**. Earlier versions lack the Vulkan Video encoding fix
  that some GPU drivers need. Without it the server falls back to CPU encoding,
  which stops at 3840x2160. For the clipboard, install `wl-clipboard` there
  too. See the upstream project for installing the server.
- **To build:** Rust 1.89 or later. The repository pins the exact toolchain in
  `rust-toolchain.toml`.

## Install

Download `gliff-win-vX.Y.Z-x86_64-pc-windows-msvc.zip` from the
[Releases page](https://github.com/wynandt/gliff-win/releases), unzip it
anywhere and run `gliff-windows.exe`. It needs no installer and no other
files. The zip also holds `gwin-verify.exe` (the check program, see
[docs/gwin-verify.md](docs/gwin-verify.md)), `LICENSE`, `NOTICE`, `README.md`
and `CHANGELOG.md`.

To check the download, compare its hash with the line for it in the release's
`SHA256SUMS` file. In PowerShell:

```
Get-FileHash .\gliff-win-vX.Y.Z-x86_64-pc-windows-msvc.zip
```

Each release also carries a build provenance attestation, which the GitHub CLI
checks with `gh attestation verify <file> --repo wynandt/gliff-win`.

**The programs are not code-signed.** The first time you run one, Windows
SmartScreen may show "Windows protected your PC" with "Unknown publisher".
If the hash matched, choose **More info**, then **Run anyway**. Ticking
**Unblock** in the zip's Properties before you unzip it skips the prompt.

### Build from source

On Windows, with the Visual Studio Build Tools (the C++ workload) and `nasm`
(`scripts/install-nasm-windows.ps1` installs it; CI builds this way):

```
cargo build --release --workspace --locked
```

The programs land in `target\release\`.

Or cross-build from Linux with
[cargo-xwin](https://github.com/rust-cross/cargo-xwin). This needs `nasm` and
LLVM 19 `clang-cl` on `PATH`:

```
cargo xwin build --workspace --release --locked --target x86_64-pc-windows-msvc
```

The programs land in `target/x86_64-pc-windows-msvc/release/`. Copy
`gliff-windows.exe` to the Windows machine.

## Use

Start the client with no arguments to get the address bar:

```
gliff-windows.exe
```

Or name the machines to open, one tab each:

```
gliff-windows.exe user@203.0.113.7 user@203.0.113.8
```

A target is `user@host` or a name from your ssh config. It goes to `ssh.exe`
unchanged after a check, so ssh config aliases work. A `host:port` and a
`ssh://` URL are refused; ssh takes a port from its own configuration (see
[settings](docs/settings.md#recent-hosts)). The first target is shown, the
others connect in the background. With no target, the client reopens the
machines the last run left open.

Starting the client again with a target adds it to the window that is already
open.

### The tab strip

Each tab shows the machine's full name and a dot:

| Dot | State |
|---|---|
| Stopped | No session. The tab stays so you can start it again. |
| Connecting | ssh and the server are starting. |
| Live | The session is running. |
| Reconnecting | The session ended for a reason that can pass, and the tab is trying again. |
| Failed | The tab gave up. The panel says why. |

- **Click a tab** to show it. **Click the `+`** to open the address bar, type
  a target and press Enter to connect. The list button next to it shows
  recent machines; its menu can forget one. Right-click in the address bar for
  Cut, Copy, Paste and Select All.
- **Right-click a tab** for Connect, Reconnect, Disconnect, Copy address,
  Connection settings and Forget. Forget removes the tab. Copy address puts
  `user@host` on the Windows clipboard. Connection settings is a submenu for
  that machine only: how it is shown (mirror or own screen), whether it shares
  the clipboard, Sharp colour, GPU decode, and whether Win+L goes to it. A row that a flag or `config.toml` has decided for every machine
  is greyed, with a line under it that says which, such as "Locked: this window was
  started with --output, which applies to every machine." See [Options](#options).
- **Middle-click a tab** to stop it if it has a session or is starting one,
  and to forget it if it has none (stopped or failed). The dot does the same when you click it.
- **Too many tabs** for the width? A chevron opens a menu of them.
- **Fullscreen** hides the strip. Move the pointer to the top edge and it
  slides in; move away and it slides out.

### When a connection fails

The panel names the problem in plain words, for example "The machine did not
accept a key from this computer.", and has the buttons that fit: **Try
again**, **Check again**, **Edit address**, **Details** (ssh's own text) and
**Close**. It reads ssh's own message, and the full text stays in
`ssh-stderr-<n>.log` in the log folder. For key problems it links to the
matching step in [docs/ssh-keys.md](docs/ssh-keys.md), which also has a table of
every message.

A session that ends on its own for a reason that can pass (the server exits,
ssh drops) is tried again by itself, up to five times with a growing wait. A
failure a retry cannot fix, such as a refused key or a changed host key, is
not retried.

### Keyboard

While the window has the keyboard, every key goes to the remote machine.

| Chord | What it does |
|---|---|
| `Ctrl+Alt+Home` | Gives the keyboard back to Windows. Click the picture to take it again. |
| `Ctrl+Alt+PageDown` | Next tab. |
| `Ctrl+Alt+PageUp` | Previous tab. |
| `Ctrl+Alt+1` to `9` | Tab by position. |
| `Ctrl+Alt+L` | Sends Super+L to the remote machine. Win+L still locks Windows, unless *Send Win+L to this machine* is on for the tab. |
| `Win+Shift+S` | Stays on Windows, so a snip goes to the Windows clipboard and from there to the remote machine. |

All of these can be changed in `config.toml`, and all but the release chord can
also be turned off; see
[docs/settings.md](docs/settings.md). On a compact keyboard, `Home` is
`Fn+Left`.

**The client logs no keystrokes.** Its log and its statistics file never say
which key was pressed or in what order. They hold counts (how many
keys were down, how many times the keyboard hook was installed again), the
names of modifier keys only (Win, Ctrl, Alt, Shift; left or right) when a
modifier was let go of or was down at an event, and the keyboard layout name. A clipboard
line holds sizes and times, never what was copied. The log says nothing about the
pointer's position or about other programs' windows. Every line has the time it
was written, so a line that says the release chord was used, or that a local
chord was kept on Windows (its modifiers, never its key), also says when. With
the default list the only local chord is `Win+Shift+S`. It is bounded: a file
stops growing at 4 MiB, and the client keeps the newest 14 daily files.

### Clipboard

Copy on one machine and paste on the other: text, pictures and files. The
other side only learns what kinds of data are available, and the data moves
when something pastes. The server trusts the machine it is connected to and
withholds nothing from the clipboard. Use `--no-clipboard` on a host you do
not trust. Limits, formats and the file rules are in
[docs/clipboard.md](docs/clipboard.md).

### Options

| Option | Meaning |
|---|---|
| `--server-path <PATH>` | The `gliff-server` to run: an absolute path, or a command name on the remote `PATH`. One word only: letters, digits and `/ . _ + -`. It cannot carry flags. Default `gliff-server`. |
| `--headless` | A private remote screen sized to the window, instead of mirroring the focused one. Every machine; it beats the tab menu's *Show as*. |
| `--output <NAME>` | Mirror this remote output (for example `DP-1`) instead of the focused one. Cannot be combined with `--headless`. |
| `--ssh <SSH>` | The ssh client to run. Default `%SystemRoot%\System32\OpenSSH\ssh.exe`, or `ssh.exe` when that file does not exist. |
| `--adapter <monitor\|high-performance\|minimum-power>` | Which GPU presents. Default `monitor`, the one that drives the window's monitor. |
| `--stats-out <FILE>` | Write session and present statistics as JSON about once a second and at exit. |
| `--log-dir <DIR>` | Where the client log and ssh's stderr go. Default `%LOCALAPPDATA%\gliff-windows\logs`. |
| `--release-chord <CHORD>` | The chord that gives the keyboard back. Default `ctrl+alt+home`; `shift+esc` is the upstream one. It cannot be turned off: `none` is refused. |
| `--grab-follows <focus\|hover>` | The keyboard follows the focused window (default) or the pointer. With `focus`, the client also lets go of the keyboard when it finds that another program's window is in front, even if Windows sent no focus message. |
| `--grab-timeout-secs <N>` | Let go of the keyboard after N seconds. Off by default. |
| `--injected-keys <forward\|local>` | Keys that other programs inject (auto-type): send them (default) or keep them on Windows. |
| `--forward-media-keys` | Send volume and media keys to the remote machine instead of keeping them on Windows. |
| `--super-l <chord\|registry\|off>` | How Super+L gets through. Default `chord`: the client changes no Windows setting, Win+L locks Windows and `Ctrl+Alt+L` sends Super+L. `registry` turns *Send Win+L to this machine* on for every machine; `chord` and `off` turn it off for every machine. Without the flag, each tab's menu decides; read [Win+L and the lock setting](docs/windows-quirks.md#winl-and-the-lock-setting) first. |
| `--super-l-chord <CHORD>` | The chord that sends Super+L. Default `ctrl+alt+l`. |
| `--local-chords <LIST>` | Chords that stay on Windows while the keyboard is captured. Default `win+shift+s`; `none` keeps nothing. |
| `--side-buttons <upstream\|linux-mouse>` | Which codes the two thumb buttons send. Default `upstream`. |
| `--no-clipboard` | Turn the clipboard off for every machine; it beats the tab menu's *Share clipboard*. |
| `--clipboard-history` | Let remote copies appear in Win+V history and cloud sync. |
| `--config-dir <DIR>` | Read settings from this folder instead of `%APPDATA%\gliff-windows\`. |
| `--print-settings` | Print the settings as JSON and exit. Needs no target. |

Most options can also be set in `config.toml` in the config folder
(`%APPDATA%\gliff-windows\`, or `--config-dir`, or the environment variable
`GLIFF_WINDOWS_CONFIG_DIR`). A flag beats the file, and the file beats the
default. `--print-settings` shows the result and where each value came from.
The keys, the defaults and the chord rules are in
[docs/settings.md](docs/settings.md).

**Connection choices for one machine.** Right-click a tab and open
*Connection settings* to choose, for that machine only, *Show as* (mirror the
remote screen, or its own screen), *Share clipboard*, *Sharp colour*
(full-resolution colour, off by default), *GPU decode* and *Send Win+L to
this machine* (off by default; it applies at once). The choice is saved in
`config.toml` at once; the screen, the colour and the decoder apply the next
time the tab connects. When a tab
has a session that started with something else, the menu shows *Reconnect now
to apply*, which reconnects that tab in one click. A flag (`--headless`,
`--output`, `--no-clipboard`, `--super-l`) applies to every machine and beats the menu: the
menu then shows the choice greyed, with the reason. There is no settings window
yet. See [docs/settings.md](docs/settings.md#connection-choices-for-one-machine).

With *Own screen*, the remote screen is sized to the window and takes the Windows
display scale. To change that scale, change the Windows display scale and
reconnect; changing it in the remote desktop's own display settings is not
supported and blanks the picture for 10 seconds or more
([details](docs/settings.md#which-remote-screen-the-window-shows)).

## Not in 0.2

- **System tray.** The window is the only place the client lives. A tray
  needs its own design for what closing the window means.
- **Settings window.** Settings live in `config.toml` and flags. A window that
  edits them is designed ([docs/settings.md](docs/settings.md)), not built.
- **A theme choice.** The strip and panel use one fixed dark palette. There
  is no light theme and no following the Windows setting yet. Dark mode
  that follows Windows is planned. See [What's missing](#whats-missing).
- **ssh key management.** gliff-win uses your own `ssh` and keys. It does not
  create, copy or store keys, and never asks for a password. The guide is
  [docs/ssh-keys.md](docs/ssh-keys.md).
- **HTML and rich text on the clipboard.** The code is built and tested on the
  build machine, but the hardware gate has no case for it yet, so it is not
  claimed. Text, images and files are.
- **Clipboard files that stream without a spool.**

## What's missing

Gaps against the Linux Gliff client (0.4.1), most noticeable first. "Not planned yet"
means no document in this repository plans it.

1. **Sound.** gliff-win plays no audio and sends no microphone. The Linux
   Gliff 0.4.1 has none either, so this is a gap in the protocol, not only in
   this client. Not planned yet.
2. **Sharp coloured text is off by default.** The Linux client asks for
   full-resolution colour (4:4:4) by default. gliff-win asks for it only for a
   machine whose *Sharp colour* choice is on; otherwise it asks for 4:2:0,
   which smears coloured text a little. A server that encodes on the CPU sends
   4:2:0 unless it runs with `--full-chroma`
   ([details](docs/settings.md#sharp-colour)).
3. **GPU video decode is off by default.** The Linux client chooses with
   `--video cpu|gpu`. gliff-win decodes on the CPU (OpenH264) unless a
   machine's *GPU decode* choice is on, and each decoded picture is copied
   back from the GPU before it is drawn
   ([details](docs/settings.md#gpu-decode)).
4. **System tray and start with Windows.** Not in the Linux client 0.4.1
   either. Here the tray is designed, not built. Planned for a
   later release.
5. **A settings window.** The five connection choices for one machine, "Show
   as" (mirror or own screen), "Share clipboard", "Sharp colour", "GPU
   decode" and "Send Win+L to this machine", are in a tab's right-click menu
   under *Connection settings*. Share clipboard and Send Win+L apply at once;
   the others apply the next time the tab connects.
   Everything else is flags and `config.toml`; there is no window that edits
   them. The Linux client 0.4.1 has no settings window either, but its open
   pull request [omacom/gliff#98](https://github.com/omacom/gliff/pull/98) adds
   one, and the design here follows it. Planned for a later release.
6. **Light and dark theme.** The strip and panel use a fixed dark palette. The
   Linux client follows its desktop theme. The plan is to follow the Windows app
   theme, in a later release.
7. **Several monitors and mixed scales.** The window is per-monitor DPI aware,
   but moving it between monitors of different scale, or a scale change while
   it runs, is untested. Planned, not started.
8. **A progress bar and Cancel for big pastes.** The Linux client shows one
   after the first second. Here progress is a log line and a slow paste shows
   nothing. Planned if real use shows a need.
9. **HTML and rich text on the clipboard.** Built and host-tested, but not
   proven on hardware. Copy and cut also act as copy only. Windows "virtual
   files" are not built: files go through a spool folder, as in the Linux client.
   Planned if real use shows a need.
10. **Touch and pen.** The Linux client turns the first finger into a left
    click. gliff-win has no touch code of its own and no pen support. Whether
    Windows' own touch-to-mouse conversion gives a usable result is untested.
    Not planned yet.
11. **Release hotkey choices.** The Linux client takes `double-escape` or
    `none`. gliff-win takes any chord of keys, such as `shift+esc`, and the
    default is `ctrl+alt+home`. It cannot be turned off: `--release-chord none`
    stops the client at start, and `release_chord = "none"` in `config.toml` is
    ignored and logged. Not planned yet.
12. **A package or installer.** The Linux client ships as a package.
    gliff-win ships as a zip of the programs on the Releases page, with no
    installer and no code signature.

The full list of limits and open issues is in [KNOWN-ISSUES.md](KNOWN-ISSUES.md).

Not in either client: HDR, gamepads, a UDP video path and AV1 for screens wider
than 4096 pixels (upstream lists the last two as not built).

## If something does not work

- **The connection fails.** Read the panel first, then
  [docs/ssh-keys.md](docs/ssh-keys.md). ssh's own message is in
  `ssh-stderr-<n>.log` in the log folder (one file per session; the first is
  1). A typical cause is that the host key is not in `known_hosts` yet: run
  `ssh user@host` once in a terminal.
- **`gliff-server` is not found.** A non-interactive ssh shell may not have it
  on its `PATH`. Pass `--server-path`, or set `server_path` in `config.toml`.
- **The keyboard is stuck in the window.** Press `Ctrl+Alt+Home`, then click
  the picture to take it again. On a compact keyboard press `Fn+Left`. A
  keypad 7 key does not work for this.
- **A Win, Ctrl, Alt or Shift key seems to stay down in Windows after the
  keyboard comes back** (a plain key acts as a shortcut). Press and release
  the key once. If it keeps happening, the log line
  `keyboard hook silent` says when the keyboard hook was lost and put back;
  see [docs/windows-quirks.md](docs/windows-quirks.md).
- **Win+L locks Windows, not the remote machine.** By default the client
  changes no Windows lock setting, so Win+L locks Windows even while the window
  has the keyboard. Press `Ctrl+Alt+L` to send Super+L to the remote machine
  (on a tiling window manager such as Hyprland, Super+L is a focus binding,
  not a lock). To send the real Win+L, right-click the tab, open *Connection
  settings* and tick *Send Win+L to this machine*: Windows lock shortcuts are
  then off while the window shows that tab and has the keyboard, and back
  300 ms after it loses the keyboard (at once if you untick it or show
  another tab). See
  [docs/windows-quirks.md](docs/windows-quirks.md#winl-and-the-lock-setting).
- **A paste does not arrive.** The log records each offer, request and
  transfer with its serial; see [docs/clipboard.md](docs/clipboard.md).
- **The screen stops answering after the remote compositor reloads.** The
  client asks the server for its output again after 3 s, and the title reads
  "re-syncing the display"
  ([docs/windows-quirks.md](docs/windows-quirks.md#the-remote-compositor-reloads)).

## Verification

What is proven, and where. Numbers are from this repository's records.

- **Unit and integration tests:** `cargo test --workspace --locked` runs about
  1,500 tests with no failures (6 more are ignored). About 1,100 of them are in the
  client's library. The counts are rounded because they change with every fix. They run on Linux against a fake server and need no second machine.
- **Pictures of the UI:** `gwin-verify strip-shot` draws the tab strip's scenes
  at 100, 125, 150 and 200%, 104 pictures, and judges each (no overlap, dot size, the
  Failed dot's red, names whole). `gwin-verify panel-shot` draws the failure
  panel for each of the thirteen failures at the same four scales, 52 pictures,
  and judges them. Neither opens a window or touches input. See
  [docs/gwin-verify.md](docs/gwin-verify.md).
- **Hardware gates** on a real Windows 11 machine against a real
  `gliff-server`:
  - Keys and pointer (an earlier build): 40 of 40 key checks, 20 of 20 pointer clicks at 200%,
    9 of 9 Super+L lock steps, 8 of 8 manual chords. See
    [docs/windows-quirks.md](docs/windows-quirks.md#tested-where).
  - Clipboard: `gwin-verify clipboard` passed for text, images and files.
    HTML and rich text have no case yet. See
    [docs/clipboard.md](docs/clipboard.md#hardware-gate).
- **Not yet shown on real machines:** the tab strip, address bar and failure
  panel are verified by tests and the pictures above, and by daily use, but
  have no recorded hardware gate of their own. The lists of what is not
  tested are in
  [docs/windows-quirks.md](docs/windows-quirks.md#not-tested-anywhere).

To run the checks yourself:

```
scripts/check              # lint, fmt, clippy, tests, then the Windows clippy and release build
scripts/check --host-only  # the same without the Windows steps
```

## More

- [docs/architecture.md](docs/architecture.md): crates, threads, data flow, and
  how it differs from upstream Gliff.
- [docs/windows-quirks.md](docs/windows-quirks.md): what Windows does that the
  client works around, and where each thing was tested.
- [docs/clipboard.md](docs/clipboard.md): what copies, how, and the limits.
- [docs/ssh-keys.md](docs/ssh-keys.md): the commands to set up ssh keys, and
  what each connect error means.
- [docs/settings.md](docs/settings.md): the settings file, its keys, and the
  recent hosts list.
- [docs/development.md](docs/development.md): the test suite and `gwin-verify`.
- [docs/gwin-verify.md](docs/gwin-verify.md): what each check proves.
- [KNOWN-ISSUES.md](KNOWN-ISSUES.md): what is off by default, not built, not
  tested, or still open.

## License

MIT, see [LICENSE](LICENSE), which names both copyright holders: Wynand
Theunissen for this project's changes and Kevin McConnell for upstream Gliff.
Parts of the code are derived from upstream Gliff, which is also MIT; see
[NOTICE](NOTICE). OpenH264 is compiled from source, so Cisco's binary patent
grant does not apply to it.
