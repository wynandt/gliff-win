---
name: verify-gliff-win
description: Verify the Windows desktop client, stream statistics, tabs and input using repository checks and the native client. Read before claiming UI behavior works.
---

# Verify the client

**Draft: the corrected native journey has not run.** Linux logic checks
and MSVC cross-builds do not prove corrected transparency, native blend-state
restoration, focus, or input routing.
Read [the feature map](features/README.md) before choosing an entry point.

## Launch

On Linux, prerequisites and the MSVC cross-build recipe are in
`docs/development.md`. Run `scripts/verify-stats` for the stats logic checks.
This does not launch a Windows window.

Native verification requires an authorized, unlocked, disposable Windows
session, a built client in the current folder, and an already configured SSH
target in `GLIFF_VERIFY_HOST`. Never provision trust or change SSH policy for
a test. Do not drive someone else's desktop. From PowerShell:

```powershell
if (-not $env:GLIFF_VERIFY_HOST) { throw 'Set an authorized test target first' }
$run = Join-Path $env:TEMP ('gliff-stats-' + [guid]::NewGuid())
New-Item -ItemType Directory $run | Out-Null
$exe = (Resolve-Path .\gliff-windows.exe).Path
$arguments = '--config-dir "{0}\config" --log-dir "{0}\logs" --stats-out "{0}\stats.json" "{1}"' -f $run,$env:GLIFF_VERIFY_HOST
$client = Start-Process $exe -ArgumentList $arguments -PassThru
```

Ready means the native window shows the remote picture and `sessions` in the
JSON contains a streaming session with frames increasing. An SSH error or an
empty black window is not readiness. Use a moving remote source for rates,
then a static source for idle behavior. Record which was used.

## Doctor

```powershell
$client.Refresh()
if ($client.HasExited) { throw 'Owned client exited' }
if ((Get-Process -Id $client.Id).Path -ne $exe) { throw 'Wrong binary' }
Get-FileHash $exe
Get-Content "$run\stats.json" -Raw | ConvertFrom-Json | Select-Object complete,sessions
```

Record the checkout commit, binary hash, owned PID and run directory. A JSON
read during file replacement can be retried; persistent missing stats is a
failure. This check does not establish input or visual correctness.

## Drive

Follow the mapped native journeys. The custom strip has no accessibility
selector for its graph button: identify the graph immediately before fullscreen,
record the actual click, and never substitute an internal visibility setter.
Use approved native desktop control or an attending operator; none is bundled
with this skill. If unavailable, stop and report the native journey blocked.

Existing `gwin-verify.exe tabs`, `keys`, `pointer`, and `present` checks are
specified in `docs/gwin-verify.md`. They take over input: run only in the
explicitly authorized disposable session. They are regression evidence, not
proof the new graph was toggled or the overlay is click-through.

## Evidence

Keep command transcripts and exit codes, before/action/after captures cropped
to the owned window, and the matching raw `stats.json` in `$run`. Record DPI,
client size, source, tab/session ID, stream sample sequence and timestamp for
each capture. A sequence identifies a JSON interval; the rounded readout must
match that interval, not a later file overwrite. Copy each snapshot before the
next tick. Report races rather than inventing a match.

On Linux:

```bash
proof=$(mktemp -d "${TMPDIR:-/tmp}/gliff-stats-proof.XXXXXX")
set -o pipefail
scripts/verify-stats 2>&1 | tee "$proof/checks.log"
printf 'Retained proof: %s\n' "$proof"
```
Evidence from this helper proves only the named tests. Native journeys below
remain unverified until their action and side effects are captured on Windows.

## Cleanup

Close only the window started above, then wait for that PID:

```powershell
$client.Refresh()
if (-not $client.HasExited) { $client.CloseMainWindow() | Out-Null; if (-not $client.WaitForExit(10000)) { Stop-Process -Id $client.Id } }
Test-Path "$run\stats.json"
```

Retain the run directory and evidence. Never kill by process name or remove
proof during cleanup. A harness must use its own isolated config directory;
never run it on an existing user instance.

## Helpers

`scripts/verify-stats` is an executable, host-only test selector. It covers
received versus successful-immediate decode accounting, separate call means,
interval sampling, premultiplied alpha/source-over math, optional JSON compatibility,
narrow/DPI layout, strip actions, and real fake-server session identity. The
integration test uses a synthetic sampling clock, not a throughput benchmark.
Run `scripts/check` separately for full gates; report failures without skipping
them. Use `/maintain-verification-skill` to keep these recipes and map current.
