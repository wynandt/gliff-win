---
name: verify-gliff-win
description: Verify the Windows desktop client against the feature map, using repository checks and the native client. Read before claiming UI or protocol behavior works.
---

# Verify the client

Read [the feature map](features/README.md) before choosing an entry point.
Host checks, picture judges and cross-builds are not native UI evidence.
Native journeys that have not been run on an authorized Windows session are
marked unexecuted in the map; do not claim those.

## Launch

On Linux, prerequisites and the MSVC cross-build recipe are in
`docs/development.md`. Host helpers (`scripts/gwv`, the `gwin-verify`
subcommands that run on the build machine, `scripts/verify-stats`) do not
launch a Windows window.

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

Follow the mapped journeys. Use approved native desktop control or an
attending operator; none is bundled with this skill. If unavailable, stop
and report the native journey blocked. Never substitute an internal setter
for a user-visible click. The custom strip has no accessibility selectors:
identify the control just before you use it and record the actual click.

`gwin-verify` subcommands are specified in `docs/gwin-verify.md`. Those that
drive the real window (`present`, `tabs`, `keys`, `pointer`) take over
input; `clipboard` overwrites the clipboards of both machines. Run them
only in the explicitly authorized disposable session. They are regression
evidence for their own plans, not proof of a different journey in the map.
Safety rules for real-machine runs are in `docs/development.md`.

## Evidence

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

Close only the window started above, then wait for that PID:

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

`scripts/gwv` is the Linux helper for building and running `gwin-verify` and
keeping transcripts. Run `scripts/check` separately for full gates; report
failures without skipping them. Use `/maintain-verification-skill` to keep
these recipes and the map current.
