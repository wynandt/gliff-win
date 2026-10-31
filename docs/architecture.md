# Architecture

What gliff-win is made of and why. It describes the code on `main`: window,
video and session, keyboard, layout and pointer input, and the clipboard
(text, images and files). How to build and test it is in
[development.md](development.md); what Windows does that shaped it is in
[windows-quirks.md](windows-quirks.md); known limits and open issues are in
[KNOWN-ISSUES.md](../KNOWN-ISSUES.md).

gliff-win shows one remote Hyprland desktop in one Windows window. The client
starts `gliff-server --stdio` on the remote machine through `ssh.exe` and speaks
the stock [gliff](https://github.com/omacom/gliff) protocol over the ssh
child's stdin and stdout. Nothing runs on the remote machine except the stock
server.

## Data flow

```
 UI thread (Windows)                           session thread "gliff-session"
 +---------------------------------+   UiCmd   +------------------------------------+
 | window procedure, message loop  | --------> | tokio current-thread runtime       |
 | ResizeGate, Stages, PresentLog  |           | Core: protocol state machine       |
 | Presenter (D3D11, flip swap)    |           | CpuDecoder (OpenH264, to I420)     |
 | RemoteCursor, fullscreen        | <-------- | writer task, reader task           |
 +-------^-----------------^-------+  UiEvent  +----^---------------^-----+--------+
         |                 | FrameSlot (latest wins) |               |     | stdin / stdout
         |                 +---- Picture + Waker ----+               |     v ssh.exe child
 keyboard-hook thread          clipboard messages <-------------------+
 (WH_KEYBOARD_LL, swallows,    in and out of the session            gliff-server --stdio
  sends keys to the session)                |
                                            v
  gliff-clipboard thread  <--- Command / HostEvent --->  gliff-clipboard-win thread
  (transfer engine, spool)                               (message-only window, the
                                                          Windows clipboard)
```

1. `app::run` parses `Args`, opens the log (`logging`), builds the window and
   starts a session (`session::driver::start`).
2. `ssh::spawn` starts the child: `ssh -T -o Compression=no -o IPQoS=lowdelay
   -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=10 -- <target> <server_path>
   --stdio (--output auto | --headless | --output NAME)`. It uses `CREATE_NO_WINDOW` on
   Windows, writes ssh's stderr to `<log_dir>/ssh-stderr.log` (or
   `ssh-stderr-<n>.log` when the spec carries a session tag, so tabs do not
   overwrite each other's), and sets
   `kill_on_drop`. `validate` refuses a target that starts with `-`, a server
   path that is neither absolute nor one bare word of `[A-Za-z0-9._+-]`, and an
   output name outside `[A-Za-z0-9._-]`: the remote shell sees the command
   verbatim, so the check is a whitelist, not quoting.
3. The session thread sends `Hello` (protocol version, the keymap built from the
   Windows keyboard layout, and `client_caps`: H.264, a maximum of at least
   3840x2400, and the chroma modes: Single420 only, or Dual420 and Single420
   when the machine's *Sharp colour* choice is on), then runs the message loop.
4. The server answers `HelloAck`, `StreamConfig`, then `VideoFrame`s. `link`
   reads each message with its payloads. `Core` turns each message into
   `Action`s that the driver performs.
5. A decoded picture goes into the `FrameSlot` and the UI thread is woken with
   a posted message. The UI thread takes the newest picture and presents it.
6. When the window changes size, `ResizeGate` decides when to send `Resize`.
   The server answers with a new `StreamConfig` and the stream changes size.
7. Key and pointer events become protocol messages on the session thread. With
   the clipboard on, the clipboard messages go to the clipboard engine thread
   and its answers come back into the same writer.
8. Closing the window sends `Bye`, stops the clipboard engine, closes the ssh
   child's stdin and waits for the server and ssh to exit. The window waits up
   to 3 s for this, then closes anyway.

## Crates and modules

```
crates/gliff-windows/   library and the `gliff-windows` binary (the client)
crates/gwin-verify/     the `gwin-verify` check CLI and the `gwin-fake-server` binary
scripts/                check, lint, stand-ins and helpers (see development.md)
```

Most modules have no Win32 imports, so they build and test on Linux. The
modules marked **W** use Win32 and are `#[cfg(windows)]`: they are checked with
`cargo xwin clippy` and built with `cargo xwin build`, and run only on a real
Windows machine. Off Windows the `gliff-windows` binary only parses its
arguments and exits 2.

| Area | Modules | Role |
|---|---|---|
| Startup | `app` **W**, `cli`, `startup`, `logging`, `main` | Arguments, the config folder and settings at start, the log (one file per UTC day), the window and session, the two helper modes of the exe. |
| Settings | `settings`, `config`, `history` | Described under [Settings](#settings). |
| Session | `session` (`machine`, `driver`, `input_side`), `link`, `ssh` | The protocol state machine with no I/O, the task that owns the I/O, and the ssh child. |
| Video | `decode`, `h264`, `yuv`, `frame_slot`, `present` **W**, `present.hlsl`, `present_math`, `layout` | CPU decode to I420, the latest-wins hand-off, the D3D11 conversion and present, and where the picture goes in the window. |
| Window | `window` **W**, `window_input` **W**, `fullscreen` **W**, `cursor` **W**, `cursor_image`, `waker` **W**, `resize_gate`, `stages`, `present_log`, `stats_out` | The window procedure, resize rules, fullscreen, the remote cursor, and the `--stats-out` record. |
| Keyboard | `hook` **W**, `grab` **W**, `grab_state`, `chord_watch`, `keymap`, `xkb`, `altgr`, `input`, `local_chords`, `key_ledger`, `pass_log`, `modifiers`, `bounded_wait` | Described under [Input](#input). |
| Lock setting | `lockreg` **W**, `lock_policy`, `super_l` | The per-user `DisableLockWorkstation` value and every way back from it. |
| Tabs | `tabs`, `reconnect`, `route`, `clip_hub`, `session_set`, `single_instance`, `strip`, `palette` | Described under [Tabs and sessions](#tabs-and-sessions). |
| Pointer | `pointer`, `window_input` **W** | Mapping to the remote pixel, buttons, wheel. |
| Clipboard | `clip_win` **W**, `clip_link`, `clip_engine`, `clip_core`, `clip_formats`, `clip_files` | Described under [Clipboard](#clipboard). |

`gwin-verify` has one module per check; see [gwin-verify](#gwin-verify) below.

## Key design choices

Each choice names the alternative that was rejected.

- **ssh is the only transport and the only authentication.** The client starts
  `ssh.exe` with `BatchMode=yes` and no console window. Rejected: a pairing
  step, an open port, or a relay; each needs something installed or exposed on
  the remote machine. The cost is that a host key or a key with a passphrase
  must already work in a terminal. Because the remote shell sees the command
  verbatim, the target, server path and output name are checked against a
  whitelist instead of being quoted.
- **A separate repository, not a fork of Gliff.** The client touches neither the
  server nor the GTK client, so a fork would only add rebase work. The
  protocol, transport and software-codec crates are pinned by git revision and
  bumped deliberately, one release at a time.
- **The protocol is not changed.** The stock `gliff-server` is the server. A
  feature the stock server cannot do, such as streaming virtual files, waits for
  upstream or for a client-side design that needs no wire change.
- **CPU decode, GPU conversion.** OpenH264 decodes the 4:2:0 stream the
  client asks for (two of them for Dual420); a D3D11 shader converts the planes
  to RGB. Rejected for now:
  hardware decode (more drivers to test; it is on the not-built list) and converting to BGRA on the CPU the way `gliff_sw::Decoder`
  does. `to_bgra` is kept, and its output is checked byte for byte against
  upstream's, as the oracle for the shader.
- **Raw D3D11 through the `windows` crate, not wgpu.** A prototype of both was
  built on the build machine (cross-compiled and not run, because that machine has
  no Windows). The release exe was 140,288 bytes against 4,414,976, and 16 crates
  against 78. With wgpu the frame-latency wait happens inside
  `get_current_texture`, so there is no handle to add to the message loop's wait,
  and `GetFrameStatistics` is not exposed. D3D11 also keeps a later hardware
  decoder's NV12 textures on the same device. See [Measurements](#measurements).
- **The session core is a pure state machine with one rule: every frame read is
  acked,** including a frame that fails to decode and a frame the `FrameSlot`
  later replaces, as upstream's client does. All I/O is in the driver, so the
  protocol is tested on Linux with no ssh and no window.
- **The keyboard hook has its own thread, and it does as little as possible:
  look at the key, swallow or pass, hand it to the session.** A slow UI thread
  must not make Windows give up on the hook. The release chord is decided on
  the hook thread too (`chord_watch`). It used to be seen only by the session,
  so while a slow ssh was still connecting the hook already swallowed every key
  and the chord did nothing.
- **While the keyboard is grabbed, every key is swallowed, with an
  input-desktop poll.** Rejected: swallowing only the Win key and Alt+Tab
  (what some clients do). In a model of both policies the minimal one left a
  stuck key and caused three local side effects (menu mode, the Alt+Shift
  layout switch, a Sticky Keys prompt); swallowing everything plus the poll left
  no stuck keys and no side effects. The poll exists because the secure
  desktop may not deactivate the window.
- **The keyboard follows focus by default, and hover is an option.** Upstream is
  hover-only. Rejected as the default: with hover the Win key goes to Windows the
  moment the pointer is over the taskbar, and keystrokes meant for another window
  go to the remote machine while the pointer rests on gliff. (`--grab-follows`.)
- **Super+L reaches the remote machine through the per-user lock setting,
  held only while the window is focused or hovered, with crash safety.** Windows
  handles Win+L before any hook. Rejected: dropping the `L` while Win is held
  (the first design: Super+L would never reach the remote machine), and a
  machine-wide policy (needs admin). The setting is written behind a marker, a
  watchdog process restores it if the client is killed, and
  `--restore-lock-policy` does it by hand. `Ctrl+Alt+L` is the stand-in chord;
  `--super-l chord`, the default, uses only that and changes nothing on
  Windows (a start in any mode still repairs a dead client's leftover marker). The lock setting is the opt-in `--super-l registry`, because it
  switches Windows locking off while the window is focused or hovered
  (what is not yet tested about it is in [KNOWN-ISSUES.md](../KNOWN-ISSUES.md)).
- **Win+Shift+S stays on Windows as a local chord.** A grab forwards every key,
  so the snipping overlay would reach the remote machine and Windows would never
  see it. A local chord is kept for Windows: when its trigger goes down with
  exactly its modifiers held, the remote machine is told to let go of everything
  it holds and the hook replays the chord to Windows, which never saw the
  modifiers go down. The list is `--local-chords`.
- **The wheel is an accumulator.** Rejected: upstream's one step per wheel
  event, which is right for a notched wheel but, for a precision touchpad's 30
  small events, sent 30 steps where 2 were wanted in a model; the accumulator
  sent 2. Whole steps go out and the remainder carries over.
- **The clipboard uses the classic Win32 clipboard and delayed rendering, one
  thread per owner.** Nothing is read or sent until something pastes. Rejected
  for now: OLE (`IDataObject`) streaming, which would allow virtual files and no
  spool, at much higher risk; the spool covers parity.
  See [clipboard.md](clipboard.md).
- **Verification is part of the product.** `gwin-verify` proves each behaviour
  with JSON lines, against stand-ins on Linux and the real server on Windows,
  and the gate's own code judges results rather than the client's. See
  [development.md](development.md).
- **What the server sends is bounded where it is read.** The transport allows
  a payload of 64 MiB; the client refuses a message that declares more than its
  kind can carry (a clipboard chunk of 256 KiB, a cursor picture of 4 MiB, a
  video frame of 16 MiB) before it reads the payload, and ends the session. At
  most 32 server messages wait for the session; the reader then waits, so a
  fast sender is slowed by the pipe. The server's error text is cut to 512
  characters, with control characters shown as `?`, before it reaches the log
  or the screen. Reading the local clipboard stops at a size no legal body
  needs (twice the item limit; for a bitmap, its pixel limit as 32-bit pixels)
  instead of copying the whole item first.

## The session core

`session::Core` (`session/machine.rs`) has no I/O. It takes a server message,
a decode result or a UI command and returns the `Action`s the driver must
perform, in order: `Send`, `Decode`, `ResetDecoder`, `Present` and `Ui`.

- Phases: `AwaitHelloAck`, `AwaitStreamConfig`, `Streaming`, `Ended`.
- The first `Resize` is sent right after `HelloAck`, because a headless output
  starts clamped to 1920x1080 (a mirrored output keeps its own size, and the
  server fits it into the window). A stalled stream is resynced only on a
  headless output.
- `Ping` is answered with `Pong` in every phase.
- **Ack rule:** every `VideoFrame` read gets exactly one `FrameAck`, including
  frames that fail to decode and frames the slot later replaces. The ack is
  sent right after the decode call. A decode error also sends
  `RequestKeyframe`.
- **Chroma.** `Core` knows which chroma modes the client offered and refuses a
  `StreamConfig` for any other. A Single420 frame that carries an aux payload,
  and a Dual420 frame that carries none, are acked, not decoded, and answered
  with `RequestKeyframe`. A Dual420 frame is decoded only as a pair, so the two
  decoders always see the same access units.
- A `StreamConfig` resets the decoder only when the coded width, height or
  chroma changes (`DecoderChange::Reset` or `Kept`); a rate-only change keeps
  it. `ResetDecoder` carries the new chroma, so a switch between Single420 and
  Dual420 in the middle of a session (the server's rate ladder) builds or drops
  the second decoder. The server starts each new stream with a keyframe.
- The geometry of each access unit (stream size, view, scale) is queued and
  paired with the picture the decoder releases for it, so a picture is always
  shown with the geometry it was encoded under. The CPU decoder holds one
  picture back; the idle drain (150 ms with nothing to feed) flushes it, and a
  drained picture is never acked again.
- `UiCmd` is `Resize` (the same request twice sends one message), `Bye`, or the
  input commands (`Key`, pointer motion, buttons, wheel, a keymap change and
  `ReleaseInput`, which lets go of every held key and button).
  `UiEvent` is `Connected`, `StreamConfig`, `Cursor`, `Resyncing`, `Resynced` or
  `Ended`.
- `EndReason` is `Bye`, `Incompatible` (version or capability mismatch),
  `ServerError` or `Link` (the server broke the protocol). Streams above
  4096x4096 are refused.
- `SessionEnd` (from `SessionHandle::join`) carries the reason, ssh's exit
  code, any non-protocol errors, the counters and the number of pictures the
  slot dropped.
- **Resync (per session).** `Core::resync` sends a Resize 32 px narrower than the last one and
  then the last one again (`ResizeReq::nudged`), and reports `Resynced` with the
  first picture after it. It is the client's answer to a server that kept
  its link but stopped sending pictures after the remote compositor reloaded
  (see [windows-quirks.md](windows-quirks.md#the-remote-compositor-reloads)).
  When to do it is the pure `session::resync::Watch`: it counts key presses,
  button presses and wheel steps with no `VideoFrame` or `StreamConfig` since,
  and is due 3 s after the first of at least three, no sooner than 30 s after
  the last resync. The driver owns its clock. Each session's driver has its own
  `Watch` and its own 30 s limit, so a hidden tab whose server went quiet is
  only resynced when input reached *it*: input goes to the shown session alone,
  so a hidden session is never resynced, and one session's resync does not
  hold back another's.
  After the Resizes, `Core::resync` also sends an Up for every Win, Ctrl, Alt and Shift
  key (both sides) the key pipeline does not hold, so a modifier the remote kept after a
  missed Up is cleared. `UiCmd::ClearModifiers` does the same when the keyboard hook is
  installed again in the middle of a grab.

`session::driver` owns the I/O: a writer task fed by a channel, a reader task
that reads whole messages, the decoder, the 150 ms idle drain, and the Bye
sequence (Bye, then stdin EOF, then a 5 s wait for ssh). `gwin-verify stream`
keeps its own instrumented copy of this loop.

## Decode and present

- **Decode.** `decode::CpuDecoder` wraps OpenH264 and returns tight I420
  (`yuv::YuvFrame`, planes taken from a `YuvPool`). For Dual420 it runs a
  second OpenH264 decoder on the aux stream and puts full-resolution U and V
  back together from the two pictures (the inverse of upstream's
  `split_yuv444`), so the frame is 4:4:4 (`Sampling::Full`). A picture from one
  decoder without one from the other is an error (out of step), the same rule
  as `gliff_sw::Decoder`. It mirrors
  `gliff_sw::Decoder` in its configuration, flush rule and even crop, but keeps
  I420 instead of converting to BGRA, because the display converts on the GPU.
  `YuvFrame::to_bgra` is upstream's CPU conversion, kept as the oracle for the
  shader and for check images. OpenH264 accepts H.264 levels up to 5.2
  (`h264::OPENH264_MAX_LEVEL_IDC`); above that every unit fails, and
  `gwin-verify` reads the SPS to report it.
- **Frame slot.** `FrameSlot` holds one picture. A new picture replaces an
  untaken one, which counts as a drop: it was decoded and acked, never shown.
- **Present.** `present::Presenter` uploads the three planes as `R8_UNORM`
  textures (U and V at half size for 4:2:0, full size for 4:4:4) and runs two
  passes of `present.hlsl`. `ps_yuv` reads the chroma texture size, so one
  shader serves both. `ps_yuv` converts to RGB at the
  stream size (BT.709 limited range, with upstream's 13-bit constants over
  8192). `ps_blit` draws that into the picture's rectangle in the back buffer,
  point-sampled at an integer zoom and linear otherwise (`layout`). The swap
  chain is flip-model, two BGRA8 buffers, `Present(1, 0)` (vsync), frame
  latency 1 with a waitable object. `present_math` is a CPU model of `ps_yuv`
  used to test the shader's arithmetic against upstream's conversion.
- **Adapter.** `--adapter` is `monitor` (the GPU that drives the window's
  monitor, the default), `high-performance` or `minimum-power` (DXGI GPU preferences). A lost device is
  reported to the app, which builds a new `Presenter` on the same window.

## Window

`window.rs` has one method per message arm of the window procedure. State is in
a thread-local `RefCell`. Of the messages that arrive while it is borrowed, only
`WM_SIZE`, `WM_DESTROY` and an ending `WM_ENDSESSION` are queued and handled right
after; any other goes to `DefWindowProc` and is dropped. `AppState` is `Connecting`, `Streaming`, `Closing` or
`Ended` for the shown session (see [the window with several sessions](#the-window-with-several-sessions)). Timers: settle, size-move (16 ms), close (3 s) and stats (1 s).

- **Resize.** `ResizeGate` waits until the client size has held for 200 ms,
  sends at once when a drag ends (`WM_EXITSIZEMOVE`), and sends after
  `WM_DPICHANGED` once the window takes the suggested rectangle. Sizes are
  client pixels rounded down to even, never under 64, at the protocol scale
  `dpi / 96`. The window is per-monitor DPI aware (v2), so every size is in
  physical pixels.
- **Fullscreen.** F11 or the window menu toggles borderless fullscreen on the
  window's monitor (no exclusive mode, so DWM can use independent flip).
- **Cursor.** `RemoteCursor` turns the server's `CursorShape` into a Win32 cursor
  (`CreateIconIndirect`). `CursorImage::pointer` decides: a usable
  shape is shown; otherwise the system arrow, on a private screen and a mirrored one
  alike. Upstream hides the pointer on a private screen instead, trusting the video to
  draw the cursor, but the compositor sends a transparent or flat image over some remote
  windows (the workspace buttons of a status bar) and the video draws none there, so the
  pointer was lost. The cost is a possible second pointer where the video does draw one.
  The log gets one `cursor shape:` line each time the decision changes (sizes, hotspot
  and why the image was or was not usable; never pixels), so a missing pointer can be
  traced to the shape the server sent.
- **Stages and stats.** `Stages` tracks Starting, Resizing, Windowed, Fullscreen
  and Closing. `PresentLog` records what was presented per stage, and
  `stats_out` writes `--stats-out` as JSON about once a second and at exit.
  Besides the shown session's counters it lists every live session (`sessions`:
  id, target, shown, status, retries, frames, acks, keys and buttons held) and
  the chords in force (`chords`), so a hidden session can be judged too. A
  session's held keys and buttons are counted by its own state machine
  (`SessionStats::keys_down`, `buttons_down`) and are zero after a switch away.
  When the shown tab's session ends on its own, the title says what the tab does
  next (see [Reconnect](#reconnect-when-a-tab-tries-again)) and the window stays
  open.

## Tabs and sessions

The tab strip and reconnect are built and described
below; there is no tray yet. The client holds a set
of tabs, one per machine; a tab has at most one live session, and one tab is
shown. Every session keeps decoding and acking while hidden, so a switch is
instant.

Status in this tree: the whole core is built and wired into the window:
the `tabs` model, the input route, the clipboard router, the switch chords'
recogniser, the command line (zero or more targets), the session set, single
instance and the per-session stats. The tab strip with its address bar
([below](#the-tab-strip)) is built; the tray is not. Tabs change by a click on the strip, by an address typed in its bar, by a
switch chord, by a target given on the command line (also by a second launch,
through single instance), and by their own reconnect.

### The tab strip

The problem in plain words. The client can already hold several machines as
tabs, but a person can only move between them with a chord or the command
line. There is no way to see them, click one, add one by typing its address,
or learn why a connection failed except from a log file. The strip
shows the tabs and takes clicks, and has an address field (built). The failure panel replaces the
bare "session ended" text with a panel that says in plain words why a connect
failed and what to do next.

**Placement (decided by a spike).** The strip sits below the native title bar,
32 logical pixels high, as a child window of the main window. The picture is
still drawn by one flip-model swap chain on the main window, and the strip is
a child above it. The spike drew a strip child over a swap chain, at 200% scale
and after a resize, and also tried tabs in the title bar. Both rendered
correctly. Tabs in the title bar needs a custom caption, which loses the
caption buttons, the snap-layout flyout, the resize border and the dark title
bar unless each is rebuilt, so it is not shipped. The `Presenter` is not
touched: the viewport and the pointer mapping are offset by the strip height,
and the strip returns `HTTRANSPARENT` in its empty area so the main window
handles those clicks.

**Pure first.** Everything a person can observe about the strip is decided by
code that has no Win32 in it, so it is tested on any machine.

| Module | What it holds |
|---|---|
| `palette` | The colours as a struct of `Rgb` values. `Palette::DARK` is the only one; a light theme would add a second and a choice. Nothing else holds a colour. |
| `strip` | The metrics at a DPI, the layout (tab rectangles, the dot slot, the add button, the chevron, the full-screen button, scrolling), the hit test, the drag target, and the dot state of a tab. Text width is measured through a trait (name and font size), so tests use fixed widths and the window uses GDI. A tab is as wide as its whole name: the layout tries the font from 13 px down to 9 px (logical) and takes the largest at which every tab fits, then scrolls; a name is never cut. |
| `strip::draw` | The strip as a list of draw commands (rounded rectangles, ellipses, arcs, lines, text with its font size, clip), from the layout and the state to draw. Tests compare the list as text at four scales. |
| `strip::raster` | Turns every command except text into pixels: coverage from distance to the shape's edge, 8x8 samples for arcs and pies, so edges are smooth. It writes into a plain pixel array, with no Win32. |
| `tabs` (changed) | A click on a tab whose failure needs the person to act (a key, a host key, a missing server) only selects it; `reconnect` starts a new session at once and resets the retry count. |

**Drawing.** The window and the test tool draw the same way. `strip_gdi::Surface`
is a 32-bit bitmap in memory with a device context. The paint code builds the
draw list, the rasteriser fills the shapes into the bitmap, GDI draws the text
(ClearType) on the same bitmap, and one `BitBlt` puts it in the window. There is no
flicker, because nothing reaches the window until the picture is whole. The status
dot is 10 logical px (10, 13, 15, 20 device px at 100, 125, 150, 200%), a metric
with a test at every scale.

**One height.** `strip_win::height_of(hwnd)` says how tall the strip is (0 when it
is hidden or absent). The viewport, the pointer mapping, the resize gate and the
`gwin-verify` harness all take their offset from it, so the picture area is the
client area below the strip.

**Full-screen reveal.** In full screen the strip is a child drawn over the top
of the picture, not above it in the layout, so `height_of` is 0 and the picture
does not move. `strip::reveal` holds the rule, with no Win32: the strip comes out
when the pointer is within 2 logical px of the top edge, goes when it is more than
8 logical px below the strip or has left the window, and stays out while the
address bar or one of its menus is in use. `Slide` gives the strip's top for a
time (150 ms, eased). The window feeds the pointer from `WM_MOUSEMOVE` and, while
the strip is out, from a timer that reads the cursor position, so a pointer that
leaves without a message still hides it.

**Menus are native.** The tab menu and the chevron menu are Windows popup menus,
which the offscreen tool cannot draw. Their entries are checked in the model's
tests.

**The address bar** has a pure model (`strip::address`: events in, acts out) and a thin Win32
shell. `strip_edit` makes the EDIT child and replaces its window procedure to take Enter, Escape
and Ctrl+A, report focus, and paint the hint. `strip_win` places the field over the bar's field,
colours it (`WM_CTLCOLOREDIT`), reports `EN_CHANGE`, and keeps a queue of keyboard changes (give the
field the keyboard, set its text, give the picture the keyboard back). The queue is run from a
posted message because the window state is busy while it handles a message, and `SetFocus` made
there would send focus messages that find the state busy and are dropped. While the field has the
keyboard the grab's `typing` input is set. The recent-hosts menu is built from
`strip::menu::recent_menu` and is a native popup. A right-click in the field shows
our own popup (`strip::edit_menu`: Cut, Copy, Paste, Select All, each enabled by the
field's selection, its text and the clipboard) instead of the stock edit menu, which
also offers reading order, Unicode control characters and IME items.

**The failure panel** has a pure model (`failure_panel`: the message from `connect::diag` in, a layout and a
draw list out, with a golden) and a thin Win32 shell (`panel_win`). The shell is a child window over the
picture area, shown while the shown tab has given up with a failure and hidden otherwise. It paints the draw
list on the same GDI surface as the strip, reports the pointer, and hands a click to `App::panel_click`,
which runs the button: Try again and Check again start the tab again, Edit address opens the address bar
with the target filled in, Open Optional Features opens that Settings page, Details opens the folder with
the logs, Close hides the panel for that tab. The guide link opens the heading of `docs/ssh-keys.md` in the
browser. A child window over a picture presented by a flip-model swap chain on its parent is not yet
checked live.

Dot states and the tab menu are added in the units that build them.

### `tabs`: the model

`Tabs` is a pure state machine with events in and [`Effect`]s out. The UI
thread owns it. It calls one method per event and runs the returned effects in
order; the model starts nothing itself.

| Event | Method | What it does |
|---|---|---|
| A tab is clicked, or a chord selects it | `click`, `switch` | Shows the tab; starts a session first when the tab has none (stopped, gave up, or waiting to retry). |
| A host is typed or named | `open` | Clicks the tab with the same target (`HostTarget::same`: the user exactly, the host name without case), else adds a tab and clicks it. |
| The session is up | `connected(session, now)` | Idempotent. At the first one: `RecordHost` for a session the user started, `Persist` when the machine was not remembered, `ClipOffer` if the tab is shown. The tab's uptime clock starts (see Reconnect). |
| The session ended on its own | `ended(session, why, now)` | A failure a retry can fix: the tab waits (`Reconnecting`) and the retry's time is set. One the person must fix, or too many retries: the tab gives up with the reason. A shown tab stays shown and the route points at nothing. An end from a session that is no longer the tab's is dropped. |
| A retry is due | `due(now)` | Starts the tab's new session (cause `Retry`); a shown tab also takes the route and the picture back. Calling it early or twice does nothing. `next_due` says when to call it. |
| The stop button | `stop` | Ends the session, keeps the tab, and keeps it closed at the next start. |
| The forget button | `forget` | Stops, then removes the tab. |
| A drag | `move_tab` | Reorders. |
| Start | `start_up` | A tab per remembered machine; the open ones start (cause `Restore`); then each command-line target opens, the first one is shown. |

Ids: a `TabId` lives as long as the tab; a `SessionId` names one connection
attempt and is never reused, so a late event of a stopped session finds no
tab.

**Order of effects.** On any switch the first effect is `InputTo(new)`: the
input route moves before anything is released, so no key reaches the old
session after it. Then, for the old session, `ReleaseInput`, `ClipRevoke`,
`ClipCancel`; then, for the new one, `ClipOffer` (only when it is streaming)
and `Present`. When the click starts a session, `Start` comes first, because the
route needs the new session's sender. Stopping the shown tab is the same
sequence, followed by `ClipGone` and `Stop`; the new tab is the nearest running
tab, right first, then left, or nothing.

**What is saved.** `Tabs::tab_set` is the `machines`, `open` and `shown` of
`config.toml`: a machine is remembered after its first successful connect (or
because the file had it), `open` is every remembered machine not stopped by
hand, and a tab that gave up stays open. `Persist` asks the UI thread to write
it.

### Reconnect: when a tab tries again

`reconnect.rs` holds the policy and the count; `tabs` applies them. Both are
pure: the time comes in as an argument.

- **Which ends are retried.** A link that dropped, a server error, an order
  close, a machine that refused or was unreachable, a name that did not
  resolve, and ssh text this client has no class for (which includes a
  connection reset): retry. A refused key, a key file with bad permissions,
  too many keys, a host key that is new or changed, a missing ssh program or
  server, an old or mismatched server: no retry, because the same attempt
  fails the same way. `reconnect::retryable` is the one exhaustive list, and
  the table in 5.10 of multi-machine-and-tray gives the reasons.
- **Delay.** 1.5 s, then doubling to a cap of 30 s: 1.5, 3, 6, 12, 24 s.
- **Cap.** Five retries, then the tab gives up and says so ("gave up after 5
  retries"). The count starts again only when a session stayed up for 30 s, so
  a server that connects and drops at once ends in a give-up, not a loop. A
  click, or the stop button, also starts the count again.
- **Where the time comes from.** `SessionSet` owns a clock (`Instant::now`,
  or a test's fake) and starts what is due at the end of every `poll`. The
  window sets one timer from `Tabs::next_due` after every event and polls
  when it fires; a closing window sets none.
- **What it leaves alone.** A hidden tab retries like a shown one but takes
  no route and no clipboard. Each session has its own re-sync watch, which
  neither sees nor affects the retry. The state shows in the tab's status
  and the title only.

### `route` and `clip_hub`: where traffic goes

**`InputRoute`** is the one place that says which session keys and pointer
events go to. The hook thread and the window call `send`; only the UI thread
calls `switch`. `send` keeps the lock while it sends, and `switch` returns the
session it left. So when the UI thread then sends `ReleaseInput` to that
session, every key sent before the switch is already in its channel and the
release is the last thing it reads. A sender copied out of the lock would let a
key arrive behind the release and stay down; a test builds that wrong version
and shows the test can tell the two apart.

**`ClipRouter`** (pure) holds the clipboard rules for several sessions. There
is one Windows clipboard, so only the shown session shares it:

| Request | Goes to |
|---|---|
| The local clipboard changed (revoke) | Every running session, so none serves a copy that is gone. |
| The new local offer | The shown session only. |
| A paste (fetch) | The *owner*: the session whose remote offer the clipboard holds. This is the shown session unless the user has switched since copying. |
| A remote offer | Windows, only from the shown session; it becomes the owner. An empty offer from the owner clears it. |
| A session reads the local clipboard | Allowed for the shown session only. |
| Cancel of a transfer | The engine that numbered it. Engines each count from 1, so the router gives each transfer an id of its own for the Windows side. |

When the owner leaves (stopped, or its session ended), the clipboard's promise
is withdrawn with an empty offer and the engine is told to stop, so a paste
waiting on it fails at once instead of at its timeout.

**`ClipHub`** applies those rules between the one Windows clipboard thread and
the engines. The Windows thread keeps its single channel and its single host;
it does not know there are several sessions. The hub has one thread that sends
the Windows thread's commands on, and a host for each engine that filters its
events. The Windows thread's echo guard is unchanged: what a remote offer wrote
to the clipboard is never sent back out, so a copy made in one tab is not
offered to the next tab on a switch.

### `session_set`: running the effects

`SessionSet` owns every live session's handle, the `InputRoute` and the
clipboard side, and runs each [`Effect`] in order. It decides nothing: which
tab is shown and what a switch releases come from `Tabs`, and which engine gets
which clipboard request from `ClipRouter`. It reaches sessions and the
clipboard through two small traits (`Link`, `ClipSide`), which the real
`SessionHandle` and `ClipHub` implement, so every rule here is tested with
fakes.

| Effect | What the runner does |
|---|---|
| `Start` | Registers the session with the clipboard (its engine port), calls the starter. A failed start is reported to the model as an end with the reason, so the tab shows it. |
| `InputTo` | Points the route at that session's command channel, or at none. |
| `ReleaseInput` | Sends `UiCmd::ReleaseInput` to that session. |
| `ClipRevoke`, `ClipCancel` | Revokes that engine's offer; cancels that session's transfers, each by its own id. |
| `ClipOffer` | Tells the router the session is shown, then has the Windows clipboard thread re-read the clipboard and offer it. |
| `Present`, `PresentNothing` | Tells the router, and returns what the view should show. |
| `Stop` | Sends `Bye` and moves the handle to a retiring list. Nothing waits for the thread; `poll` joins it once finished. |
| `RecordHost`, `Persist` | Returned to the UI thread (recent hosts, config). |

`poll` takes each session's queued events. A connect and an end go to the
model, and its answer runs at once; a connect is also returned, with the other
events (a picture size, a cursor, "grab released"), under its session id for
that session's view (the cursor needs the desktop size it carries). A
session whose thread finished without an `Ended` event is ended with an error,
so a tab never stays "connecting".

### `single_instance`: one client per configuration folder

`decide` says what a launch does from three facts: a client runs under this
name, the targets given, and `--autostart` (kept for a start at logon; always false today). The
first launch runs. A second one with targets hands them over, with none it hands
over "show" (it also brings back a window hidden in the tray), and with
`--autostart` it exits quietly. The name comes from the configuration folder
(`instance_name`: case, slashes and a trailing separator do not matter), so a
test with its own folder does not meet a real client.

On Windows `single_instance::win` takes a named mutex (the client only, after
`main` has dispatched `--lock-watchdog` and `--restore-lock-policy`, which run
the same executable), marks the main window with the name as a window property,
and sends the request with `WM_COPYDATA` to the marked window, waiting up to
five seconds for it while the first client is still starting. The receiving end
checks the tag, caps the size at 16 KiB, and parses each target as the command
line does (a target such as `-oProxyCommand=...` is refused). A hand-over that
fails is logged and the second launch exits with an error; it never starts a
second client.

### The window with several sessions

`App` (in `window.rs`) holds the `Tabs` model, the `SessionSet`, and one
`SessionView` per live session; `app::run` builds them in this order: load the
settings, take the single-instance name (hand over and exit when a client
already runs), `Tabs::start_up`, the log, the window, the presenter, the
`InputRoute`, the clipboard side, the `SessionSet` with a starter, the keyboard
grab and pointer input (both on the route), and then runs the start effects.

- **A starter, one set of settings.** The starter closure builds every session
  the same way (same ssh options, server path, desktop mode, keymap snapshot,
  `--log-dir`), with the session's id as the ssh stderr tag, and a `PostWaker`
  that carries the session id in `wparam` of `WM_APP_FRAME` and `WM_APP_EVENT`.
  The UI thread therefore knows which session a wake-up is for.
- **A view per session** (`SessionView`): its waker, the resize gate, the
  remote cursor, the first-picture parameters, the frame-rate cap, the view and
  geometry last shown, whether it is re-syncing (`Resyncing` and `Resynced`
  set and clear it), and the last picture it presented (`last`). Pictures of
  a hidden session are decoded and acked but not presented: `WM_APP_FRAME`
  presents only for the shown session, and the frame slot keeps the newest.
- **A switch** (`show`): the model's effects have already moved the input route
  and released the old session's keys. The window then calls `switched()` on
  the pointer input (so a held button or a half scroll of the old session does
  not follow), takes the new session's view, runs its resize gate on the
  window's current size (a hidden session was not told about a resize), and
  presents `last` again, marked as a repeat so it is not logged as a latency
  sample; a session with no picture yet gets a black redraw until its first one.
- **The switch chords** come from the hook as `WM_APP_SWITCH` (a word packed by
  `switch_word`). The UI thread asks the model; a chord that names a tab that
  does not exist does nothing, and the hook still swallowed it (`Taker`).
- **The title** follows the shown tab (`tabs::window_title`): `gliff`, `gliff -
  reconnecting, attempt 2 of 5 (reason)` while that tab waits to retry or is
  starting its retry, `gliff -
  session ended (reason)` when that tab gave up, or `gliff - re-syncing the
  display` while the shown session is re-syncing. A session that ends or
  resyncs in a hidden tab changes only that tab's state; switching to a tab
  that is still re-syncing brings the title with it, and the title goes back
  to `gliff` when that session's first picture after the resync arrives. An
  ended tab's reason wins over a re-sync.
- **Closing.** The close button (or `WM_CLOSE`) with sessions running sets
  `closing`, calls `SessionSet::close_all` (a `Bye` to each, without telling
  the model, so the tabs stay open for the next start) and waits until none
  runs, or 3 s; with none running it closes at once. The exit code is 0 only
  when at least one session ran, every session ended with a `Bye` and ssh
  exit 0, and none failed to start.
- **A second launch** (`single_instance`) sends its targets (or "show") to the
  first as `WM_COPYDATA`; the window opens each target as a tab and brings
  itself to the front.
- **Stats.** `--stats-out` carries `sessions` (one line per session, final ones
  included) and the top-level `session`, `input`, `present` and similar fields
  describe the *primary* session, the one with the lowest id that is live or
  final. That is a simplification: the design says it follows the shown
  session (see Open decisions). The per-session line has `shown`, so a check
  does not need the top level.
- **CPU cost.** Measured on the build machine's headless harness with the
  fake server at 3840x2400: one session used 3.8% of one core at 41 fps
  (0.93 ms of decode per frame), three used 11.7% at 123.7 fps in all (0.94 ms
  per frame). The cost is linear: a hidden session decodes every frame, so ten
  tabs cost about ten times one.

## Input

Input is a pure core (tested on Linux) under a thin Win32 shell. Keys go from
the low-level hook thread straight to the session thread, not through the UI
thread, which stalls in the modal size-move loop.

| Module | Role |
|---|---|
| `keymap` | The scan code (set 1, with the extended bit) to evdev table, generated by `scripts/gen-keymap.py` and checked against an oracle fixture; `Chord` parses `ctrl+alt+home`-style chords. |
| `altgr` | The AltGr filter: Windows sends a left Ctrl and a right Alt for AltGr; the filter holds a lone left Ctrl only until a short deadline, then releases it. A pointer button or a wheel step releases it at once, so the remote sees a Ctrl before the click it modifies; the session loop serves the deadline before it handles whatever woke it, so a stream of mouse motion cannot push it back. |
| `input` | `KeyPipeline` (held keys, the release chord, media and injected keys, the Ctrl+Alt+L stand-in chord for Super+L), `WheelAccumulator` and the client's input options. |
| `chord_watch` | The hook's own watch for the release chord and the tab-switch chords (Ctrl+Alt+PageDown next, Ctrl+Alt+PageUp previous, Ctrl+Alt+1..9 by position, main-row digits only), and the mode latch that stops a late grab request from undoing a release. The latch opens when the UI thread has handled the chord, normally on the session's answer; if no answer comes within three ticks (the session has ended, or its tab is not shown), the UI thread handles the chord itself, so the latch cannot stay shut. `push` returns `Release`, `Switch(to)` or nothing; release is tested first. AltGr is dropped two ways: a fake left Ctrl followed by the right Alt within 10 ms, and `seed`, which drops the left Ctrl when the right Alt is held. The release chord works before the session says hello and while the UI thread is busy. `Taker` decides what happens to each key once a chord fired: the trigger, its auto-repeats and its release are all swallowed, so a session never sees half a chord; with no session a switch chord is still taken (to leave a dead tab) but a release chord is not, and every other key passes to Windows. `switch_word` and `switch_of_word` pack a switch into the `WM_APP_SWITCH` word. |
| `bounded_wait` | A wait for a helper thread that gives up after a limit, so the UI thread never hangs on a stuck hook thread. |
| `pointer` | `ButtonState`, the wheel carry, the side-button codes, and the mapping to the remote pixel under the pointer (`layout::to_remote` and its inverse). |
| `xkb` | The Windows layout (a KLID table with a `us` fallback) to XKB keymap text, sent in `Hello` and again when the layout changes. |
| `local_chords` | Chords that stay on Windows while the keyboard is grabbed (default `win+shift+s`, the snipping overlay). |
| `key_ledger` | Per key: whether Windows saw its Down, and whether the remote machine did. Seeded with the keys Windows holds when a grab starts, and with the held keys again when the hook is installed again. Routes an Up to Windows when Windows saw the Down, and to the remote machine too when it saw the Down. `describe_down` is the one way keys reach the log: a count and the Win, Ctrl, Alt and Shift groups, never another key. |
| `pass_log`, `modifiers` | Keys the hook let through on purpose, so the window does not mistake them for a silent hook (`silent_hook_judge`: the window and the hook must both be grabbing, and the key not noted); and the check that no Shift, Ctrl, Alt or Super key stays down in Windows after a grab. |
| `grab_state` | The pure focus or hover state machine that says when the keyboard goes to the remote machine. |
| `foreground_watch` | The pure decision behind the once-a-second foreground check: whether the window in front is ours, and when two looks at another program's window count as a lost focus. |
| `super_l`, `lock_policy` | When the per-user `DisableLockWorkstation` value is on (only while the window is focused or hovered), as a state machine. |
| `session::input_side` | Turns key and pointer events into protocol messages on the session thread. |
| `hook` (Windows) | `WH_KEYBOARD_LL` on its own thread. While grabbed it swallows every key. A watchdog reinstalls it if Windows drops it. |
| `grab` (Windows) | The input-desktop poll (releases everything at a UAC or secure desktop), the foreground check (releases the grab when another program's window has been in front for two looks), session lock notifications, IME off, hover tracking. |
| `window_input` (Windows) | Pointer, button and wheel messages from the window procedure. |
| `lockreg` (Windows) | The registry value, a write-ahead marker, the `--lock-watchdog` helper process and every restore path (`--restore-lock-policy` by hand). |

Held keys and buttons are released on focus loss, on the release chord, when a
UAC prompt or the secure desktop takes the input, and when the session ends.
Unknown mouse buttons are dropped. Motion is coalesced to the latest value on
the session thread. The Super+L lock setting is changed only by the opt-in
`--super-l registry`; the default `--super-l chord` changes
nothing on Windows.

## Clipboard

Text, images and files both ways, with upstream's clipboard protocol (nothing
is sent until something pastes). The behaviour is in
[clipboard.md](clipboard.md); this is how it is built.

- **`clip_win`** (Windows thread, `gliff-clipboard-win`): a message-only window,
  `AddClipboardFormatListener`, `OpenClipboard` with retry, delayed rendering
  (`WM_RENDERFORMAT`, and `WM_RENDERALLFORMATS` when the owner goes away). A paste that waits
  for the network blocks only this thread, never the picture.
- **`clip_engine` and `clip_link`** (engine thread, `gliff-clipboard`):
  upstream's `Transfers` on a current-thread runtime, the bridge that keeps
  serials and the spool, and the channels. The session starts the thread when
  the clipboard is on; it takes `Command`s from the Windows thread and answers
  with `HostEvent`s. A paste returns whole bodies (at most 32 MiB) or spool
  paths, so the Windows thread never handles chunks. At the end the engine
  stops first, so stdin closes and the server exits.
- **`clip_core`** (pure): what a local change turns into, what a remote offer
  turns into, the privacy formats that keep a copy out of history, the echo guard
  and the debounce.
- **`clip_formats`, `clip_files`** (pure): format conversion with golden-byte
  tests, the file walker, the Windows file-name sanitizer and the spool. Upstream's
  walker and spool are Unix-shaped, so these are ours; the types are upstream's.

## Settings

Three pure modules, with no Win32 import, so they cannot reach the registry.

- `settings` is the list of settings. One table (a macro invocation) names each
  setting once, with its type, key, default and class (Live, Reconnect or
  Restart), and generates the typed `Settings`, the `Layer` of values one
  source gives (every field optional), and the keys. `resolve(flags, file)`
  merges two layers over the defaults, remembers where each value came from,
  and runs `validate`, which refuses chords that could fire together.
  `Settings::diff` lists what changed and its class.
- `config` owns the folder (`ConfigDir`), the file layer and the machine list
  (`TabSet`: `machines`, `open`, `shown`, kept consistent), and the file
  rules shared with `history`: tolerant load, a re-read at every save with a
  numbered `.broken` or `.replaced` copy of the text the save replaces (unless it
  is what was loaded and is in the client's own layout; one copy serves failed
  tries of the same text; a last look just before the rename keeps text written
  in between), an unreadable file left alone, unknown keys kept in
  `config.toml`, atomic save with a unique temporary name and a retry only for
  transient errors. `history` rewrites the `hosts` list only.
- `history` holds `HostTarget` (a `user@host`, alias or IPv6 address parsed once, where it enters; the
  command line, `SshSpec` and the harness hold the parsed value),
  `RecentHosts` (most recent first, at most 10, one per machine) and its file.

`startup` joins them at start: it resolves the folder, loads the file, merges
the flags and returns the `Settings` that `app` uses, or the text of the
error. `--print-settings` prints that result and exits. The format and rules
are in [settings.md](settings.md).

## Command line

`gliff-windows <TARGET> [--server-path P] [--headless | --output NAME] [--ssh PATH]
[--adapter monitor|high-performance|minimum-power] [--stats-out FILE]
[--log-dir DIR] [--release-chord CHORD] [--grab-follows focus|hover]
[--grab-timeout-secs N] [--injected-keys forward|local] [--forward-media-keys]
[--super-l chord|registry|off] [--super-l-chord CHORD] [--local-chords LIST]
[--side-buttons upstream|linux-mouse] [--no-clipboard] [--clipboard-history]
[--config-dir DIR] [--print-settings]`. `--print-settings` replaces the target.
A flag is only a value when it is given: the clap fields are optional, with no
default, so the file can tell a flag from a default.
Two helper modes of the same exe, `--lock-watchdog` and
`--restore-lock-policy`, look after the lock setting.
`--server-path` defaults to `gliff-server`, found on the remote `PATH`. Run
`gliff-windows --help` for the full list.

## gwin-verify

`main.rs` defines the subcommands and `line.rs` the JSON-line output. Each
check has its own module: `decode_check`, `ssh_check`, `stream_check`
(`measure.rs` judges the wire), `present_check` (`judge.rs` is pure;
`run.rs` is the step sequence behind a `Harness` trait; `win32.rs` drives the
real window), `clip` (`.gwclip` files), `pattern` (the 32-cell test pattern),
`placement` (reads `hyprctl` JSON), `png_out`, `input_check` (the `keys` and
`pointer` checks: `cases.rs` is the data, `judge.rs` is pure, `run.rs` is the
step sequence behind a harness trait, `lock.rs` is the lock step, `manual.rs`
asks for the chords only a person can press, and `win32/` drives the real
window with `SendInput`) and `clip_check` (the `clipboard` check: a
`Harness` trait, the remote half over ssh, and its own PNG, bitmap, tar and
SHA-256 code). `gpu` is a stub that fails with `pending`. `gwin-fake-server`
models the upstream server behaviour the checks rest on; see
[development.md](development.md) and [gwin-verify.md](gwin-verify.md).

## How it differs from upstream Gliff

Upstream's client is a GTK program for Linux. gliff-win is a separate Rust
client for Windows that uses the server and the protocol as they are.

- It reuses upstream's `gliff-proto`, `gliff-transport` and `gliff-sw` crates
  directly, pinned to one commit, and ports upstream's session loop, layout
  and resize rules as logic. The ported files are listed in [NOTICE](../NOTICE).
- It starts ssh itself, with a checked server path, `BatchMode=yes`, no console
  window and stderr to a log file.
- Video is CPU decode (OpenH264) and a D3D11 present. It asks for Single420
  unless the machine's *Sharp colour* choice is on; upstream asks for Dual420
  by default.
- The keyboard follows focus by default (upstream: hover). The release chord is
  Ctrl+Alt+Home (upstream: Shift+Esc; `--release-chord shift+esc` restores it).
  Keys that other programs inject are forwarded by default.
- The Windows keyboard layout is sent as an XKB keymap, with a fallback to `us`.
- Super+L reaches the remote machine through the stand-in chord by default.
  The real Win+L needs the opt-in per-user lock setting, because Windows keeps
  it from hooks; Linux has no equivalent.
- The wheel carries a remainder instead of sending one step per event, and
  unknown mouse buttons are dropped.
- The clipboard runs on its own thread on the classic Win32 clipboard, in place
  of upstream's GTK thread, and maps by MIME type.
- Reconnect differs from upstream's: see
  [Reconnect](#reconnect-when-a-tab-tries-again).

## Measurements

Every number here has a source. Timings of the real client on a
Windows machine are in [windows-quirks.md](windows-quirks.md#measured-on-the-windows-machine);
this section holds the build-machine numbers that decided the design. Their
result files are not part of this repository. The programs that produced them were throwaway and are not in the
repository, so they cannot be re-run; the decode path they describe can be
re-timed with `gwin-verify decode` (see [gwin-verify.md](gwin-verify.md#decode)).

**CPU cost of one 3840x2400 frame (build machine, 8 vCPU, release
build, 300 access units of a libx264 High clip).** The first access unit is left
out (299 frames). Milliseconds per frame:

| What is done to each frame | mean | p50 | p95 | bytes to upload per frame |
|---|---|---|---|---|
| OpenH264 decode, then `gliff_sw`'s parallel I420 to BGRA conversion | 26.45 | 23.34 | 42.53 | 36,864,000 |
| OpenH264 decode only, no copy | 19.20 | 17.23 | 28.97 | none |
| decode, then a tight I420 copy | 22.52 | 20.42 | 35.98 | 13,824,000 |
| decode, then an NV12 interleave | 22.66 | 20.57 | 34.52 | 13,824,000 |

At 1920x1080 the same four took 6.24, 5.49, 5.54 and 5.99 ms mean (p95 8.66, 8.06,
8.08, 8.49). What it showed: converting to BGRA on the CPU adds about 7 ms to the
19 ms of decoding at 3840x2400 (26.45 against 19.20 mean) and uploads 2.7 times
the bytes of I420. That is why the client keeps I420 and converts on the GPU.
It is a build-machine number, not a Windows one: these files do not record the
CPU of the Windows machine, whose own timings are in the link above. The NV12
interleave was no faster than the tight I420 copy, so the client keeps planes.

**Shader arithmetic.** A CPU emulation of the planned `ps_yuv`
(f32, saturate, round to nearest) was compared with `gliff_sw::convert::i420_to_bgra`
for all 2^24 (Y, U, V) triples, each twice for the 2x2 chroma. Of 100,663,296 output
channel values, 100,662,684 matched exactly, 612 differed by 1, and none differed by more. The
worst triple was (174, 0, 29). Only a GPU run can prove the real shader, and that is the
`gwin-verify present` run in windows-quirks.md.

**Sharp colour: Dual420 against Single420 (build machine, 8 vCPU, release
build).** A synthetic screen of coloured monospace text on a dark background,
with a band of red text on blue, went through the CPU server's encoder
(`gliff_sw::Encoder`, bitrate per stream at the server's ceiling for 30 fps) and
then through the client's own path: the session core, `CpuDecoder` and the
`YuvFrame` it presents. 31 frames each; the first is the keyframe, and the
P-frame columns leave it out. Quality is of the last frame against the source;
colour error is in 8-bit Cb/Cr steps, with 4:2:0 shown the way `ps_yuv`
samples it.

| Run | RGB PSNR | Cb, Cr PSNR | worst colour error | decode, keyframe | decode, P-frame p50 | bytes per P-frame |
|---|---|---|---|---|---|---|
| 2560x1440 still, Single420 | 23.11 dB | 26.60, 25.89 dB | 129 | 53.9 ms | 3.56 ms | 20,893 |
| 2560x1440 still, Dual420 | 34.04 dB | 40.60, 40.58 dB | 43 | 97.7 ms | 9.32 ms | 48,474 |
| 2560x1440 scrolling 4 rows a frame, Single420 | 22.92 dB | 26.67, 25.96 dB | 126 | 55.5 ms | 3.98 ms | 14,630 |
| 2560x1440 scrolling 4 rows a frame, Dual420 | 27.25 dB | 32.30, 31.95 dB | 93 | 97.8 ms | 17.61 ms | 201,228 |
| 3840x2160 still, Single420 | 23.08 dB | 26.58, 25.81 dB | 124 | 164.8 ms | 9.44 ms | 48,413 |
| 3840x2160 still, Dual420 | 33.92 dB | 40.45, 40.45 dB | 34 | 418.1 ms | 18.07 ms | 111,688 |

Luma is the same in both modes (39.29 dB still, 36.36 dB scrolling, at
2560x1440), so all of the gain is colour. Converting the last frame to BGRA on
the CPU, as a model of the shader's work, took 1.56 ms for 4:2:0 and 3.01 ms
for 4:4:4 at 2560x1440. What it showed: on dense coloured text, Dual420 cuts the
worst colour error about threefold and costs two to four and a half times the
decode time per frame and two to fourteen times the bytes; the scrolling case
is the most expensive. A Sharp colour session that the server answers with
Single420 gave a picture byte-identical to a session with the choice off, and a
switch from Dual420 to Single420 in the middle of a session decoded every frame
with no error, its first picture after the switch coming from the new keyframe.
OpenH264 cannot encode 3840x2400, so the CPU server's 4:4:4 tops out at its
3840x2160 limit.

What has no number: the time from a server frame to the pixel on screen (the
`present` check measures only decoded-to-`Present` returned), the CPU and GPU load
of a session, memory use, the clipboard's transfer time or throughput, and
anything over a network other than one LAN. None was recorded, and this page
does not estimate them.

## Not built yet

In rough priority order:

1. **Live resize and scaling hardening**, including a DPI change while
   running and monitors with different DPI.
3. **More hardware.** Long sessions have run on one set of hardware; a
   recorded session on other machines is still wanted.
4. **Hardware decode.** OpenH264 decodes on the CPU, and *Sharp colour*
   (Dual420) at least doubles that work.
5. **Virtual files on the clipboard, and streaming a big file without a spool**
   (an OLE `IDataObject`). The spool covers parity until then.
   HTML and RTF are written and unit-tested, but `gwin-verify clipboard` has no
   case for them yet.
6. **Pixel-smooth touchpad scrolling.** The protocol has it; the client sends
   whole steps and carries the remainder.
7. **Release binaries and an installer.** Build from source.
8. **The tray** and key setup from the window. The tab strip,
   its address bar, the ssh error panel and recent hosts are built.

Hardware that has not been tried is listed in [windows-quirks.md](windows-quirks.md#not-tested-anywhere).

## Dependencies

- Upstream `gliff-proto` (messages), `gliff-transport` (framing) and `gliff-sw`
  (the CPU conversion used as a test oracle) are pinned by git revision
  `58c742452ded3f10a2ce61f60eb4845f18205e29` (v0.4.1) in the workspace
  `Cargo.toml`. The server the client is tested against is Gliff v0.4.1 (0.4.1-1
  in the final hardware run). `Cargo.lock` is committed.
- `openh264` (built from source, which needs `nasm` and `clang-cl`; see
  [development.md](development.md)), `png`, `tokio` (a current-thread runtime
  with `fs`, `process` and `io-util`), `clap`, `serde`, `serde_json`,
  `toml` (parse and write the settings files), `thiserror`, `bytes`, and on Windows only the `windows` crate (0.62).
- The edition is 2024 and the minimum Rust version is 1.89. Workspace lints deny
  `unwrap_used`, `wildcard_imports`, `enum_glob_use`,
  `undocumented_unsafe_blocks` and `unsafe_op_in_unsafe_fn`.
- Binaries: `gliff-windows` (the client), `gwin-verify` and `gwin-fake-server`.

