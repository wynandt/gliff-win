---
name: verify-gliff-win
description: Verify the gliff-win Windows desktop client against the feature map, using repository checks and the real native client (driven on the Windows machine against a loopback server, or against an authorized remote machine). Read before claiming UI or protocol behavior works, and before reproducing a bug or proving a fix.
---

# Verify the client

Read [the feature map](features/README.md) before choosing an entry point.
Host checks, picture judges and cross-builds are not native UI evidence.
Native journeys that have not been run on an authorized Windows session are
marked unexecuted in the map; do not claim those. The loopback launch and
`scripts/gwin-native.ps1` were added without a Windows run: until a map
entry records one, treat their first run as the proof of the helper itself.

## Launch

On Linux, prerequisites and the MSVC cross-build recipe are in
`docs/development.md`. Host helpers (`scripts/gwv`, the `gwin-verify`
subcommands that run on the build machine, `scripts/verify-stats`) do not
launch a Windows window.

There are two native launches. Use the loopback unless the journey needs a
real remote machine (Hyprland binds, `gwin-keys`, `wl-clipboard`, the
moving pattern).

### Loopback: the real client, no remote machine

The real `gliff-windows.exe` runs a real session against
`gwin-fake-server.exe` on the same Windows machine. `gwin-fake-ssh.exe`
stands in for `ssh.exe` (`--ssh`), so there is no network, no sshd, no keys
and no host trust. The fake server writes what it received to
`input.jsonl` in the run folder: the remote half of the evidence.

Build the bundle on the build machine and copy the three programs into one
folder on the Windows machine (no installer):

```bash
cargo xwin build --release --locked --target x86_64-pc-windows-msvc -p gliff-windows -p gwin-verify
# bundle: target/x86_64-pc-windows-msvc/release/{gliff-windows,gwin-fake-ssh,gwin-fake-server}.exe
```

Then, in PowerShell on the Windows machine, with this skill's
`scripts\gwin-native.ps1` copied next to the bundle:

```powershell
$line = powershell -NoProfile -ExecutionPolicy Bypass -File .\gwin-native.ps1 launch -Bundle .
$line
$run = ($line | ConvertFrom-Json).run
```

`launch` makes `%TEMP%\gwin-verify-<stamp>-<id>`, starts the client with
that folder's own `--config-dir`, `--log-dir` and `--stats-out`,
`--ssh gwin-fake-ssh.exe`, `--super-l off`, `--headless` (the remote
output follows the window, as in the other gates), `--no-clipboard` (unless
`-Clipboard`) and the target `verify@loopback`, and records the owned PID,
binary path and SHA-256 in `run.json`. `-Fault NAME` passes a
`GWIN_FAKE_SERVER` fault (names in `docs/gwin-verify.md`). It prints one
JSON line; `ok` is true only when the window exists and `stats.json` shows
a streaming session. `-ExecutionPolicy Bypass` applies to that one process
and changes no setting.

### Remote machine: an authorized target

Native verification requires an authorized, unlocked, disposable Windows
session and a built client in the current folder. Any journey that opens a
session also needs an already configured SSH target in `GLIFF_VERIFY_HOST`.
Never provision trust or change SSH policy for a test. Do not drive someone
else's desktop. Always give the client its own config and log directories.
From PowerShell:

```powershell
$run = Join-Path $env:TEMP ('gwin-verify-' + [guid]::NewGuid())
New-Item -ItemType Directory $run | Out-Null
$exe = (Resolve-Path .\gliff-windows.exe).Path
# Connected journey: require a configured target and write stats for doctor.
if (-not $env:GLIFF_VERIFY_HOST) { throw 'Set an authorized test target first' }
$arguments = '--config-dir "{0}\config" --log-dir "{0}\logs" --stats-out "{0}\stats.json" "{1}"' -f $run,$env:GLIFF_VERIFY_HOST
$client = Start-Process $exe -ArgumentList $arguments -PassThru
```

Omit the host argument to open the empty address bar. Omit `--stats-out`
when the map entry does not read the JSON.

Ready for a connected journey means the native window shows the remote
picture and, when `--stats-out` is used, `sessions` in the JSON contains a
streaming session with frames increasing. An SSH error or an empty black
window is not readiness. Ready with no target means the address bar is
shown and the process is still the owned binary. For rate measurements use
a moving remote source, then a static source for idle behavior, and record
which was used.

## Doctor

Loopback:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\gwin-native.ps1 doctor -Run $run
```

It fails (exit 1, `ok: false`, named `reasons`) unless the owned PID is
alive with the recorded path and hash, its window is visible, a session is
streaming with frames increasing over 1.5 s, and the fake server logged a
`hello`. It lists the owned child PIDs (fake ssh and server). Run it before
the first drive and after any drive that failed.

Remote machine:

```powershell
$client.Refresh()
if ($client.HasExited) { throw 'Owned client exited' }
if ((Get-Process -Id $client.Id).Path -ne $exe) { throw 'Wrong binary' }
Get-FileHash $exe
```

Record the checkout commit, binary hash, owned PID and run directory. When
`--stats-out` was used:

```powershell
Get-Content "$run\stats.json" -Raw | ConvertFrom-Json | Select-Object complete,sessions
```

A JSON read during file replacement can be retried; persistent missing
stats is a failure. This check does not establish input or visual
correctness.

## Drive

Follow the mapped journeys. On the loopback, `gwin-native.ps1` is the
bundled native control. Each verb takes `-Run $run`, prints one JSON line
and exits 1 on failure:

| Verb | User action | What it judges |
|---|---|---|
| `shot -Name N` | none | PNG of the owned window, `evidence\N.png` |
| `type -Text gliff` | click the picture, type, `Ctrl+Alt+Home` | the remote side received each key down and up in order, and holds no key afterwards |
| `resize -Width W -Height H` | resize the window | the remote side received a new Resize |
| `drop` | none (the fake server it started is killed) | the tab reconnects: a second `hello` and a streaming session |

`type` and `resize` send real input with `SendInput` and take the keyboard
while they run. Use them only on an unlocked session that nobody is using,
and do not touch the keyboard or mouse meanwhile. `type` sends only `a-z`,
`0-9` and space. Elsewhere, use approved native desktop control or an
attending operator; if neither is available, stop and report the native
journey blocked. Never substitute an internal setter
for a user-visible click. The custom strip has no accessibility selectors:
identify the control just before you use it and record the actual click.

`gwin-verify` subcommands are specified in `docs/gwin-verify.md`. Those that
drive the real window (`present`, `tabs`, `keys`, `pointer`) take over
input; `clipboard` overwrites the clipboards of both machines. Run them
only in the explicitly authorized disposable session. They are regression
evidence for their own plans, not proof of a different journey in the map.
Safety rules for real-machine runs are in `docs/development.md`.

## Evidence

On the loopback, the run folder is the evidence and survives cleanup:
`run.json` (owned PID, binary hash), `input.jsonl` (what the remote side
received; the `bye` line lists keys and buttons still held), `stats.json`,
`logs\` (client and ssh logs) and `evidence\` (PNGs and each verb's JSON).
Most input bugs show only on the remote side, so a keyboard, pointer or
resize claim needs the `input.jsonl` lines, not only a screenshot.

Keep command transcripts and exit codes. For a native window, keep
before/action/after captures cropped to the owned window, and any
`stats.json` in `$run`. Record DPI, client size, source, tab/session ID,
and, when comparing a readout to JSON, the stream sample sequence and a
copy of that snapshot taken before the next tick. Report races rather than
inventing a match.

On Linux, retain transcripts under a proof directory. Examples:

```bash
proof=$(mktemp -d "${TMPDIR:-/tmp}/gwin-verify-proof.XXXXXX")
set -o pipefail
# pick the helper the map entry names, for example:
#   scripts/gwv run panel -- panel-shot --out "$proof/panel"
#   scripts/gwv run strip -- strip-shot --out "$proof/strip"
#   scripts/gwv run config -- config
#   scripts/verify-stats
#   formal/held-keys/run
printf 'Retained proof: %s\n' "$proof"
```

Evidence from a host helper proves only that helper. Native journeys remain
unverified until their action and side effects are captured on Windows.

## Cleanup

Loopback:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\gwin-native.ps1 cleanup -Run $run
```

It closes the owned window (`WM_CLOSE`, then `Stop-Process` on that PID
only after 10 s), ends any owned fake ssh or server PID that is still
running after checking its path, removes the run's `config` folder, and
lists the evidence files it kept. Run it after every failed iteration too.

Remote machine: close only the window started above, then wait for that PID:

```powershell
$client.Refresh()
if (-not $client.HasExited) { $client.CloseMainWindow() | Out-Null; if (-not $client.WaitForExit(10000)) { Stop-Process -Id $client.Id } }
```

Retain the run directory and evidence. Never kill by process name or remove
proof during cleanup. A harness must use its own isolated config directory;
never run it on an existing user instance.

## Helpers

| Helper | What it proves | What it does not |
|---|---|---|
| `gwin-verify` / `scripts/gwv` | The subcommands in `docs/gwin-verify.md` | A different journey than the one named |
| `scripts/verify-stats` | Host stats logic and fake-server session identity | The native graph toggle or overlay |
| `formal/held-keys/run` | The held-keys model (see `formal/held-keys/README.md`) | Live hook, IME, or pointer behaviour |
| `scripts/gwin-native.ps1` (this skill) | On the Windows machine: the loopback launch, doctor, `shot`, `type`, `resize`, `drop`, cleanup | Hyprland binds, real ssh, clipboard data, decode timing on a real server |
| `gwin-fake-ssh` + `gwin-fake-server` with `GWIN_FAKE_INPUT_LOG` | What the remote side received, for the loopback and for `cargo test -p gwin-verify --test loopback` | A real `gliff-server` or compositor |

`scripts/gwv` is the Linux helper for building and running `gwin-verify` and
keeping transcripts. Run `scripts/check` separately for full gates; report
failures without skipping them. Use `/maintain-verification-skill` to keep
these recipes and the map current.
