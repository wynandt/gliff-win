# gwin-verify

`gwin-verify` proves one behaviour of the client per subcommand and prints
one JSON object per line, `{"check": ..., "ok": ..., <metrics>}`. It exits
non-zero if any line has `"ok": false`. This page is the reference for each
check. For how to build it, where it runs and what to do first, read
[development.md](development.md).

## Checks

| Check | What it proves | Where it runs |
|---|---|---|
| `decode` | Decode time (p50/p95/max per access unit, fps) and correctness through the client's `CpuDecoder`, its GPU decoder, or both (`--decoder`), against the decode kill rule (p95 at most 33.3 ms, fps at least 30 at 3840x2400) | Anywhere; the kill rule is judged on the Windows machine |
| `ssh` | The server starts over ssh as the client starts it, the handshake completes, Bye ends it cleanly | Anywhere (stand-ins on Linux) |
| `make-clip`, `render` | Write `.gwclip` test clips and a synthetic desktop | Linux |
| `stream` | A headless session at the panel size: Resize applied, fps, decode p50/p95, acks equal frames, no stall, SPS level, last picture compared with the test pattern | Anywhere (stand-ins on Linux) |
| `present` | Drives the real `gliff-windows.exe` window and judges its stats file and a screenshot | Windows only |
| `tabs` | Several sessions in one window: three targets, the tab chords sent with `SendInput`, the right session shown after each, no key left down in any session, hidden sessions still acking, the recent hosts file has the targets. It takes over the keyboard and mouse | Windows only; never unattended |
| `keys` | Input: the 20 Super chords, system chords, layouts (a right Shift with `/` among them), AltGr, capture, release chord, no stuck keys, a slow hook (and a Ctrl held across it), the Super+L lock step and the manual chords, judged from the remote machine's `gwin-keys` log and the client's stats file | Windows only |
| `pointer` | Input: click on known pixels at the display's current scale, buttons and wheel | Windows only |
| `clipboard` | Text, pictures and files both ways between the Windows clipboard and the remote machine's, with the real client running; it **overwrites both clipboards** | Windows only |
| `panel-shot` | The failure panel for each of the thirteen failures at 100, 125, 150 and 200% into PNG files, each judged (no overlap, inside the picture, the buttons in their colours, words drawn). It uses no window, keyboard, mouse or clipboard | Anywhere; on Windows it draws the words with the real fonts, elsewhere shapes only |
| `strip-shot` | The tab strip's states drawn at 100, 125, 150 and 200% into PNG files, and each picture judged (no overlap, dot size, the Failed dot's red, the chevron only when narrow, names drawn). It uses no window, keyboard, mouse or clipboard | Anywhere; on Windows it draws the text with the real fonts, elsewhere shapes only |
| `config` | `gliff-windows --print-settings` with a temporary config folder: flag over file over default, and the recent hosts list (a hostile line dropped) | Anywhere the client runs |
| `gpu` | Not built yet: it prints `{"ok": false, "pending": ...}` and exits 1, so nothing reads as green before it exists | n/a |

Wire framing is proved by `cargo test -p gliff-windows --test framing`, not by
the command line.

## Starting the client

Every check that starts `gliff-windows` does it through one function,
`client_command`, which adds a `--config-dir` to the command line: a folder
named `client-config` inside the check's log folder, emptied before each start.
No check therefore reads or writes
the settings file or the recent hosts of the machine it runs on. A host test
fails if a launcher builds the client's command line another way.

## config

```
gwin-verify config [--client PATH] [--log-dir DIR]
```

- Writes a `config.toml` and a `recent-hosts.toml` into a scratch folder inside `--log-dir`, runs
  `PATH --print-settings --config-dir DIR` three times (plain, with a flag that
  overrides one file setting, and with a chord that collides with a tab chord,
  which must exit 2), and judges the JSON.
- With no flag, a setting in the file shows the file's value with source `file`,
  and a setting not in the file shows its default with source `default`.
- With a flag for the same setting, the flag's value shows with source `flag`.
- The recent hosts list shows the valid lines in order, and not the hostile line.
- Nothing else is touched: the client is not asked to connect, and the folder
  is checked to hold only the two files, unchanged. It runs where the client runs; off Windows the client
  binary only parses its arguments, and `--print-settings` is one of the things
  it does there.

## decode

```
gwin-verify decode --clip F [--clip F ...] | --sizes 1920x1080,2560x1440 [--frames 300]
                   [--max-p95-ms 33.3] [--min-fps 30] [--kill-rule]
                   [--decoder cpu|gpu|both] [--aux-clip F] [--pace-fps N]
                   [--idle-drain]
```

- `--decoder` picks the decoder (`cpu` by default). `gpu` is the client's
  GPU decoder and fails if it cannot start. `both` runs the CPU and the GPU
  decoder on the same clip, reports each, and fails if any GPU picture differs
  from the CPU one in any byte (H.264 decoding is exact).
- `--aux-clip` gives the aux half of a Dual420 stream, unit for unit with the
  single `--clip`, and decodes the pair as Dual420 (4:4:4).
- `--pace-fps` feeds the units at that rate instead of back to back, as a
  live stream arrives, so the line's `cpu_pct` (process CPU time over wall time,
  as a share of one core) is the cost at that frame rate. Every line also has
  `cpu_ms_per_frame`.
- `--idle-drain` drains the decoder after every unit that leaves a picture
  held, as the session does when the screen goes still, and reports the count
  as `drains` (`cpu_drains` and `gpu_drains` with `both`). A decoder that shows
  each picture as it decodes it holds none, so it never drains. The GPU
  decoder is never drained, as in a session, so pictures it still holds at
  the end of the clip are not counted.

- `--clip` decodes `.gwclip` files; `--sizes` encodes the synthetic desktop in
  process with OpenH264 Baseline (3840x2160 at most) and decodes it.
- A 3840x2400 line carries `"kill_rule": true` only at the default limits.
  `--kill-rule` adds a failing `kill-rule` line if no 3840x2400 clip was judged
  at those limits.
- A line fails on p95 over the limit, fps under the limit, any decode error, a
  missing picture or a wrong-size picture. It names the first error.
- Compare at 300 frames: the first unit is the slow IDR, and short clips make
  p95 worse.
- OpenH264's decoder rejects SPS level 6.0. libx264 picks 6.0 for 3840x2400
  unless told `-level 5.2`; `make-clip` refuses such streams, and streams
  without SPS/PPS/IDR up front or without access unit delimiters.

## Clips

A `.gwclip` holds the access units of one H.264 stream (magic `GWCLIP01`,
width, height, then length-prefixed units), so the same bytes decode on
Linux and on Windows.

- `scripts/make-clips [out_dir] [frames] [bitrate]` renders the synthetic
  desktop, encodes it with libx264 High (level 5.2, one IDR then P frames, an
  access unit delimiter per picture) and wraps it. It needs `ffmpeg` with
  `libx264` and writes 1080p, 1440p and 3840x2400 clips (each 5 to 9 MB; they
  are build artifacts and are not committed).
- `gwin-verify make-clip --size WxH --frames N --out F [--annexb A]` writes a
  clip in process, or wraps an Annex B file.
- `gwin-verify render` prints raw I420 frames on stdout (13.8 MB per frame at
  3840x2400, so always pipe it).

## Which screen the gates use

The client's built-in default is to mirror the remote machine's focused screen. Every check that starts the real client (`present`, `tabs`, `keys`, `pointer`, `clipboard`) builds its command line from one place, `client_config::client_command`, which gives it `--headless` as well as its own config folder. So the gates keep testing the private screen they were written and passed on, whatever the default is. The `stream` check and the session tests ask for the private screen in code. A mirror run of these gates is a separate step on the real pair (see the plan in the pull request that changed the default); until it is done, no gate result in this page is a mirror result.

## ssh

```
gwin-verify ssh <target> [--output NAME] [--server-path P] [--ssh PATH]
```

Starts the server as the client does, reads HelloAck, answers the Ping, reads
StreamConfig, says Bye, closes stdin, and waits for the server to exit. `ok`
needs protocol version 1, the right mode (headless, or a mirror), a
Pong, a stream size, and a clean exit after Bye inside `--timeout-s`.

Without `--output` the check starts the server with `--headless`, the private
screen the gates were built for. `--output NAME` mirrors that screen and needs
the server to report it. `--output auto` is the client's own default: the
check starts `--stdio --output auto` and passes when the server reports a
mirror (`headless` false) of whichever screen it chose, so one run proves the
command line the client now sends by default.

- A failure is classified from ssh's stderr log: `HostKey`, `Auth`,
  `ServerMissing`, `Network` or `Other`.
- A target starting with `-`, a server path that is neither absolute nor a bare
  command name, or a bad output name is refused before ssh starts.
- Upstream's `gliff-server` exits only at stdin EOF, even after Bye, so the
  client closes stdin after Bye. The fake server models this: a client that
  forgets fails on Linux too.
- Faults for the fake server: `wedge`, `wrong-mode`, `bad-exit`, `linger`
  (`crates/gwin-verify/tests/ssh.rs`).
- `GWIN_FAKE_TINT=r,g,b` is not a fault: the fake server draws one flat colour
  in place of the grid, so a test with several fake servers can tell their
  pictures apart. `crates/gwin-verify/tests/multi_session.rs` starts three
  sessions (through `Tabs` and `SessionSet`, each with its own tint), checks
  that every frame is acked in every session while hidden, and that after a
  switch the picture on screen is the new session's. A second test gives one
  machine the `reload` fault and checks that a re-sync starts only in the
  session that went quiet, and only once input reaches it (it is hidden at
  first, so it gets none). It also measures client
  CPU with one and with three busy sessions (see below).

Linux example:

```
scripts/gwv run ssh-headless -- ssh user@host --ssh "$PWD/scripts/fake-ssh" \
  --server-path "$PWD/target/release/gwin-fake-server"
```

## stream

```
gwin-verify stream --host <target> --size 3840x2400 --seconds 30
```

Every `--host` of gwin-verify is refused when it starts with `-` (ssh would
read it as an option), and the harness puts `--` before the target in the ssh
and client command lines it builds.

Runs the client's real session core headless: Resize to the panel size,
waits for a StreamConfig that implies that output, settles, then measures
decoded fps, decode p50/p95, frame gaps, frame id holes, acks against frames
and the SPS profile and level. A second ssh asks `hyprctl` where the
`gwin-pattern` window is at the start and end of the window. The last picture
is saved as a PNG and compared with the 32-cell test pattern.

`ok` needs every part to pass. Failures are named in `failures`:
`resize_ignored`, `resize_reverted`, `encoder_clamped`, `fps`, `fps_cap`,
`stall`, `acks`, `decode_errors`, `frames_missing`, `level_rejected`,
`no_sps`, `placement`, `pattern`, `png`, `exit`, `ended`, `link`.

- The server sets the frame rate. The ladder starts at 30 fps and steps to 60
  only after 4 s unblocked, so the gate is relative: fps must reach 0.95 of
  the cap in force, time-weighted (`--fps-of-cap`), and each cap in force must
  be at least `--min-fps` (30).
- `encoder_clamped` at 3840x2400 means the server fell back to its CPU encoder
  (3840x2160 at most). The panel size needs the Vulkan encoder.
- The end placement probe runs before Bye because upstream drops the output on
  Bye.
- `--placement skip` drops the hyprctl probe (non-Hyprland host); pattern
  placement is then unproven.
- `--present-ms 100` takes pictures from the frame slot only every 100 ms, so
  `slot_dropped > 0` while acks still equal frames.

Fault names for the fake server (`GWIN_FAKE_SERVER=<name>`), each failing by the
code shown: `stall` (`fps`, `stall`), `drop` (`frames_missing`,
`decode_errors`), `ignore-resize` (`resize_ignored`), `level60`
(`level_rejected`), `no-pattern` (`pattern`), `misplaced` (`placement`),
`cap15` (`fps_cap`). `reload` is for the driver test, not the stream check: it
stops sending frames and ignores a same-size Resize, as a server whose output
mode was reset by a compositor reload does. `hold30` passes at a 30 fps cap. `GWIN_FAKE_HYPR_INSTANCE=none`
makes the placement probe fail with the real hyprctl message. The Linux
integration test runs all of these at 640x400:
`cargo test -p gwin-verify --test stream`.

On the remote machine, start the pattern first:
`scripts/remote/gwin-pattern place 3840x2400` (waits for the output, shows the
pattern fullscreen, re-checks every 5 s, exits 1 if a check failed), and
`scripts/remote/gwin-pattern cleanup` afterwards.

## present

```
gwin-verify.exe present --host <target> --client .\gliff-windows.exe --seconds 20
```

Windows only; run it with the display at 200%, unlocked, with nothing covering
the screen, and do not touch the mouse or keyboard (about 70 s). On the remote
machine run `scripts/remote/gwin-pattern follow` first: it waits for the
session's output at any size, fullscreens the moving 60 fps pattern there and
re-places it on every Resize.

The line has a `windowed` and a `fullscreen` section (presented and decoded
fps, cap, latency p50/p95/max, longest gap, dropped, present errors), `resize`,
`fullscreen_geometry`, `swap_chain` (flip model, no tearing), `adapter`,
`session` (frames equal acks, no decode errors), `pattern`, `exit` and `png`.
Failures are named in `reasons`: `launch`, `no_first_picture`,
`short_<stage>`, `present_fps_<stage>`, `latency_<stage>` (p95 over 16.7 ms),
`stall_<stage>` (gap over 250 ms), `present_errors_<stage>`, `acks`,
`decode_errors`, `tearing`, `fullscreen`, `resize`, `pattern`, `capture_black`,
`exit_after_bye`, `no_damage_source`.

- gliff sends frames on damage, so fps and stall gates measure the source as
  much as the client. A static desktop gives a few frames a minute and fails the
  stall gate for the setup; the moving pattern is required, and
  `no_damage_source` stops the run early when hyprctl does not show it.
- At 200% the output's mode is the client's physical size and hyprctl reports
  it with scale 2: 2560x1600 is a 1280x800 logical output. Use `follow`, not
  `place`.
- The screenshot is a `BitBlt` of the screen, so anything on top of the window
  fails `pattern`.
- `exit_after_bye` needs the server and ssh to exit 0 within 5 s of `WM_CLOSE`.
- On Linux the judge and the step sequence run against a fake harness:
  `cargo test -p gwin-verify --bin gwin-verify present_check`. The Linux script
  test is `cargo test -p gwin-verify --test pattern_follow`.
- `crates/gwin-verify/src/present_check/fixtures/synthetic-good-stats.json` is
  synthetic; replace it with a real passing `present-stats.json` once one exists.

## tabs

`gwin-verify tabs --host a@one --host b@two --host c@three` starts the real
client with the three targets (a fresh config folder, like every check), waits
until all three sessions stream, then:

1. clicks the middle of the picture, so the window is in the foreground and
   grabbing the keyboard;
2. presses five switch chords with `SendInput`, in this order: next, next,
   previous, jump to the first tab, jump to the third (`Ctrl+Alt+PageDown`,
   `Ctrl+Alt+PageUp`, `Ctrl+Alt+1`, `Ctrl+Alt+3`, the defaults of
   `switch_next`, `switch_previous` and `switch_jump`; the plan stays inside
   the tabs, so it does not depend on whether next wraps);
3. after each chord waits up to 5 s for the stats file to name the expected
   session as shown, then 3 s more for the counters to move, and keeps that
   snapshot;
4. closes the window and waits 10 s for the client to exit.

It reads the `sessions` list of the stats file (one line per session: id,
target, shown, status, frames, acks, `keys_down`, `buttons_down`) and fails
with a named reason when:

| Reason | Meaning |
|---|---|
| `sessions`, `target_order`, `not_streaming` | At the start the list is not the targets in command-line order, all streaming |
| `shown_count`, `wrong_shown` | After a chord not exactly one session is shown, or it is the wrong one |
| `held_input` | After a chord any session, shown or hidden, has a key or button down |
| `not_acked` | A session, hidden ones included, acked nothing since the last snapshot |
| `history_missing` | A target is not in `recent-hosts.toml` of the check's config folder |
| `exit` | The client did not exit with code 0 after the window closed |
| `steps_missing` | Fewer chords were made than planned |

A run that cannot start (`launch`), a session that never streams
(`not_streaming`, before any chord is sent), a focus click that fails (`focus`)
or a chord that cannot be sent (`chord`) ends with that one reason and an
`error`.

**This check grabs the keyboard and mouse of whoever is at the machine** for
about a minute. It prints a warning and waits `--countdown-s` (10) seconds
before the first click, and must not be run unattended or by a script on a
machine a person is using. Its judge and its step sequence are tested on the
build machine against a fake client (`cargo test -p gwin-verify --bin
gwin-verify tabs_check`); the Windows harness is built and linted by
`scripts/check` but has not been run against a real client.

## panel-shot

`gwin-verify panel-shot --out DIR` draws the failure panel for every failure the same way `strip-shot`
draws the strip (no window, no input, no clipboard). It writes `panel-<failure>-<scale>pct.png`
(`-shapes` when it drew no words) and prints one JSON line with the files and the `problems`, and fails if
there is one. `--scale N` (repeat) picks scales; `--shapes-only` draws no words on Windows too.

## strip-shot

`gwin-verify strip-shot --out DIR` draws the tab strip with the same code the window
uses: the draw list, the rasteriser and, on Windows, GDI text on a bitmap in
memory. It **opens no window and sends and reads no input**, so it is safe to run
on a machine in use. It writes `strip-<scene>-<scale>pct.png`
(with `-shapes` in the name when it drew no text) and prints one JSON line with the
files and a list of `problems`. It fails if there is one.

| Option | Meaning |
|---|---|
| `--out DIR` | Where the pictures go (default `gwin-verify-logs/strip`) |
| `--scale N` | Only this scale in percent: 100, 125, 150 or 200. Repeat for more. Default all four |
| `--scene NAME` | Only this scene. Repeat for more. Default all |
| `--shapes-only` | Draw no text, also on Windows |

Scenes: `one-tab`, `three-tabs-connecting`, `ten-tabs`, `dots` (every dot state),
`failed` (the red Failed dot, set directly in the state, with no connection),
`hover-tab`, `hover-dot`, `hover-add`, `hover-fullscreen`, `dragging`, `long-name`,
`long-host` (a 40-character host), `near-identical-ips` and `narrow-near-identical-ips`
(twelve addresses that differ in the last digits), `near-identical-hosts` (long names that
differ only at the end),
`narrow-chevron` and `narrow-hover-chevron` (a width that needs the overflow
chevron), and the address bar: `bar-empty`, `bar-typed`, `bar-unfocused`, `bar-error`,
`bar-hover-go`, `bar-hover-list`, `narrow-bar` (the bar with ten tabs in a narrow window) and
`bar-no-tabs` (the bar open in a window with no tab).

What each picture is judged on: the strip is the design height at that scale
(32, 40, 48, 64); no two buttons or tabs overlap; the dot is at least 90% of its design
size; the Failed dot has red pixels and a cross; a chevron appears only when the
tabs do not fit; every name is whole: the full address, in the strip's font, in a place
as wide as the name, with no ellipsis, and no tab cut by the strip unless it scrolls; a name is
drawn for every tab and stops short of the edge of its place (not in shapes mode).

**Not covered.** The tab menu, the chevron menu and the recent-hosts menu are native popup
menus, which exist only inside a menu loop, so they are not in the pictures. The edit field
itself is a native EDIT control, so a picture shows its text and caret-less field as the
strip draws them, not the control. Their entries are
tested in the model's unit tests.

The pixel checks also run as unit tests on any machine:
`cargo test -p gwin-verify --bin gwin-verify strip_shot` and
`cargo test -p gliff-windows strip::raster`.

## keys and pointer

```
gwin-verify.exe keys    --host <target> --client .\gliff-windows.exe --remote-keys <path to gwin-keys>
                        [--only chords,system,...] [--manual]
gwin-verify.exe pointer --host <target> --client .\gliff-windows.exe --remote-keys <path to gwin-keys>
                        [--expect-scale-milli 2000]
```

`keys` starts the client with two hidden flags (`--test-panic` and
`--hook-stall-test-ms`), which exist only in a client built with
`--features test-hooks` ([development.md](development.md#build)). A released
client rejects them, so build the client for `keys` with that feature.

Windows only, with the display at 200% (`pointer --expect-scale-milli 1500` at
150%), unlocked and untouched. Both drive the real client with `SendInput` and
judge what the remote machine's `gwin-keys` logged. The design, every case and
its pass rule are in `crates/gwin-verify/src/input_check/`; the chord list is
[phase4/super-chords.tsv](phase4/super-chords.tsv).

Run order for a hardware gate:

1. On the remote machine: `scripts/remote/gwin-pattern follow`, then
   `scripts/remote/gwin-keys arm --minutes 15`. While armed, the remote
   machine's own keyboard is in the test submap, so nobody uses it.
2. On the Windows machine: `keys`, then `pointer`.
3. `keys --only lock`: the one step in which the client changes a Windows
   setting (the per-user `DisableLockWorkstation` value, with
   `--super-l registry`, which the step passes itself and which turns *Send
   Win+L to this machine* on for every machine; the other checks run the client
   in `chord` mode). It needs a person at the machine to press Win+L and
   unlock once. `gwin-verify` itself only reads the registry. If the run is cut
   short, `gliff-windows.exe --restore-lock-policy` puts the value back.
4. `keys --manual`: the chords only a person can press (Win+Space,
   Win+Shift+S, the full 20, Ctrl+Alt+Del then Esc, a UAC prompt then cancel).
5. The 150% step: a person changes Settings > System > Display > **Scale** (the
   list, not "Custom scaling") to 150%, `pointer --expect-scale-milli 1500`
   runs, then the scale goes back to 200% and `pointer` runs again, which sets
   `scale_restored`.
6. `scripts/remote/gwin-keys disarm`, and keep what it prints (the remote
   log, which `disarm` then deletes) and the JSON lines.

The CPU measurement is the second test in `multi_session.rs`. It counts the
test process's own user and system time (the fake servers are other processes)
over six seconds of steady streaming at 1280x800, with one session and then
three, and prints and saves `multi-session-cpu.txt` in the test's temporary
folder. It covers the session threads, the decode and the acks; it does not
cover painting. The numbers vary with the build machine; run
`cargo test --release -p gwin-verify --test multi_session -- --nocapture` to
read them. It asserts only that three sessions decode more frames a second than
one.

On Linux, `cargo test -p gwin-verify` runs the case list, the judges, the fake
harnesses and the `gwin-keys` script against the fake `hyprctl`.
`crates/gwin-verify/src/input_check/judge/fixtures/run1-remote.log` is a real
remote log from the first hardware run; its judge tests replay it. Win32 code is
checked by `cargo xwin clippy -D warnings` only. Bind names that depend on
Hyprland's key naming (`MOD5+Q` for AltGr, the `wheel:left` and `wheel:right`
binds) are settled by the first remote machine run; a wrong name fails its case with the
log shown.

## clipboard

```
gwin-verify.exe clipboard --host <target> [--client .\gliff-windows.exe]
                          [--only text,image,files] [--log-dir DIR]
```

Windows only, with the remote machine's `wl-clipboard` installed. It starts the
client with `--super-l off` (so it changes no Windows setting), puts content on
one clipboard, reads the other over ssh with `wl-copy` and `wl-paste`, and ends
the client with `WM_CLOSE`. It needs no focus and does not capture the
keyboard, but it **overwrites the clipboards of both machines**, so run it with
nothing else being copied.

- Cases: `text`, `image` and `files` are built. `html` and `rtf` are not and
  fail as `pending`, and they are in the default `--only` list, so a run
  without `--only` ends non-zero. Use `--only text,image,files`.
- Every text carries a per-run nonce, so an old clipboard cannot pass. A case
  ends at its first failing step, and a quiet period at the end checks that
  nothing changed.
- Pictures and file trees are compared by the gate's own PNG, bitmap, tar and
  SHA-256 code, not the client's, so the client does not vouch for itself.
- `files` writes a few MiB under the user's temp directory on Windows and under
  `~/.cache/gwin-verify-clip` on the remote machine, and removes both at the end.
- Steps, pass rules and the hand tests that have no gate case are in
  [clipboard.md](clipboard.md#test-matrix).
- On Linux the driver and the judge run against fake clipboards, and the
  remote half against `scripts/fake-ssh` and `scripts/fake-bin/wl-copy` and
  `wl-paste`: `cargo test -p gwin-verify`.
