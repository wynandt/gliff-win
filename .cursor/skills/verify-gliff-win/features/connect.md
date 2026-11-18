# Connecting to a machine

The address bar or a command-line target starts an ssh session. The client
never asks for a password, and ssh is the only transport.

## Sub-features

- CONNECT-1: no arguments shows the address bar; a target opens that machine
  (further targets connect in the background); starting again with a target
  adds it to the window that is already open; with no target, the machines
  the last run left open are reopened.
- CONNECT-2: a target is `user@host` or a name from the ssh config, passed to
  ssh after a check; a `host:port` and a `ssh://` URL are refused.
- CONNECT-3: a drop that can pass is retried up to five times with a growing
  wait; a refused or changed host key is not retried.

## How to get to it (user POV)

Start the client with no arguments and type a target into the address bar, or
pass targets on the command line. The first target is shown. See the README
("Use") and `docs/ssh-keys.md`.

## Driving it with the native client

Preconditions: parent skill launch and doctor; for a live session, an already
configured authorized target. Never provision trust or change ssh policy.

- Empty start: launch with no target; the address bar is shown.
- Connect: type a valid target and press Enter; doctor until the remote
  picture streams. A command-line target should do the same without the bar.
- Refusal: a `host:port` or `ssh://` target is rejected before ssh starts.
- Second process: start again with a target; it becomes a new tab on the
  window that is already open, not a second window.

Loopback (no remote machine):

- Connect: `gwin-native.ps1 launch -Bundle .` then `doctor`. The target
  `verify@loopback` reaches a streaming session through `--ssh
  gwin-fake-ssh.exe`; `input.jsonl` starts with a `hello` line.
- Reconnect (CONNECT-3): `gwin-native.ps1 drop -Run $run` kills the fake
  server this run started. Pass: a second `hello` in `input.jsonl` and a
  streaming session in `stats.json` within 30 s; `evidence\drop.json`
  keeps `sessions` with `retries`.
- Windows run: passed under Windows PowerShell 5.1 on one high-DPI laptop
  screen at 200% scale, built from this branch, in a run folder with a fresh
  config. Connect and reconnect both passed:
  the second `hello` arrived and the session was streaming with
  `retries` 1.
- Refused key and changed host key cannot be produced on the loopback;
  `GWIN_FAKE_SSH_STDERR` with a file from
  `crates/gliff-windows/tests/ssh_corpus` makes the fake ssh fail with
  that real text, but `launch` does not set it yet.

Host companions (not native proof):

- `gwin-verify ssh <target>`: HelloAck, Ping/Pong, StreamConfig, Bye. Stand-ins
  on Linux, or a real server. See `docs/gwin-verify.md`.
- `gwin-verify stream --host <target> ...`: the session core, resize, fps and
  the test pattern, headless.
- `cargo test -p gwin-verify --test ssh --test stream --test reconnect --locked`.
- `gwin-verify strip-shot`: address-bar scenes (`bar-empty`, `bar-typed`,
  `bar-error`, and the others listed in `docs/gwin-verify.md`), no window.

## Gotchas

The address bar itself is a native EDIT control; strip-shot shows the field
the strip draws, not typing into the control. Checks that start the real
client pass `--headless`; a default-mirror run of those gates is a separate
step and has not been recorded (`docs/gwin-verify.md`, "Which screen the
gates use"). Opening a session from the address bar, adding a target from a
second process, and the retry/give-up paths have no recorded hardware gate
(README, "Verification"). This map expansion did not run them.
