# Development

How to build gliff-win, run it without a second machine, and prove a change.
For what the code is made of, read [architecture.md](architecture.md); for
the clipboard in particular, [clipboard.md](clipboard.md); for what Windows
does that the client works around, [windows-quirks.md](windows-quirks.md). The per-check
reference for the verification tool is [gwin-verify.md](gwin-verify.md).

gliff-win is checked at three levels:

1. **`scripts/check`**: terms lint, format, clippy, host tests, the workflow
   lint and the Windows cross-build. Everything must pass before a change
   lands. [CI](#ci-and-release) runs the same checks on every pull request,
   but the gate is this script, run before you push.
2. **`gwin-verify`**: a command line that proves one behaviour of the client
   per subcommand and prints one JSON line each. Most subcommands run on Linux
   against stand-ins.
3. **Real runs**: `gwin-verify.exe` and `gliff-windows.exe` on a Windows
   machine against a real `gliff-server`. Only these prove timing, display and
   ssh behaviour. Numbers from the Linux build machine are never reported as
   Windows numbers.

## Build

You need Rust 1.89 or later (`rust-version` in `Cargo.toml`); the repository
pins the exact toolchain it is built and released with (see
[The Rust toolchain](#the-rust-toolchain)). You also need Python 3 on
the path as `python3` (`scripts/lint-registry` is a Python 3 program, so
`scripts/check` needs it; without it the script stops at once with
`python3 not found: install Python 3`). It uses the standard library only.

```
cargo build --release --workspace --locked     # native: Linux build machine
```

The native build makes the Linux tools `gwin-verify` and `gwin-fake-server`.
`gliff-windows` builds there too, but on Linux it only parses its arguments and
exits: it needs Win32. To build the
Windows client, cross-build on Linux with `cargo-xwin`:

```
cargo xwin build --workspace --release --locked --target x86_64-pc-windows-msvc
```

Install `cargo-xwin`, `nasm` and LLVM 19 `clang-cl` first (OpenH264's C sources
are built with `clang-cl`). The binaries land in
`target/x86_64-pc-windows-msvc/release/`: `gliff-windows.exe` and
`gwin-verify.exe`. Copy them to the Windows machine; they need no installer.

The client has two hidden test flags, `--test-panic` and
`--hook-stall-test-ms`, which `gwin-verify keys` uses (the exit-path and
hook-recovery checks). They exist only in a build with the `test-hooks`
feature, which no released build has. For a client that `gwin-verify` drives:

```
cargo xwin build --release --locked --target x86_64-pc-windows-msvc \
    -p gliff-windows --features test-hooks
```

Without the feature the client rejects those two flags, so those checks fail
to start it.

## Check

```
scripts/check [--host-only]
```

In order:

1. `scripts/lint-terms --self-test`, then `scripts/lint-terms`.
2. `scripts/lint-registry --self-test`, then `scripts/lint-registry`: no source
   other than `lockreg.rs` and the future `autostart_reg.rs` may import the
   registry API or call a `Reg*` function. Needs Python 3. Files that git
   lists as tracked but that are deleted in the work tree are skipped. The
   [known limits](#known-limits-of-lint-registry) are below.
3. `scripts/lint-clip-log --self-test`, then `scripts/lint-clip-log`: the
   clipboard log can hold no clipboard contents or file names. Only
   `clip_log.rs` writes clipboard lines, and its types hold no free text (see
   [The log](clipboard.md#the-log)). Needs Python 3.
4. `cargo fmt --all -- --check`.
5. `cargo clippy --workspace --all-targets --locked -- -D warnings`.
6. `cargo test --workspace --locked`.
7. `scripts/lint-workflows --self-test`, then `scripts/lint-workflows`: lint
   the files in `.github/workflows` (see [CI and release](#ci-and-release)).
   Needs Python 3 and Go, and the network the first time (it installs two
   pinned tools under `target/`).
8. Unless `--host-only`: `cargo xwin clippy ... -D warnings` and
   `cargo xwin build --release` for `x86_64-pc-windows-msvc`.

It prints `check: all green` at the end. A full run takes several minutes;
`--host-only` skips the Windows steps (use it while editing, run the full gate
before you push). Win32 code is checked only by step 8 and by hand on a real
machine.

## Run on one machine

Everything except the real window runs on the build machine against
stand-ins, so a change can be exercised with no second machine:

| Stand-in | Replaces | Notes |
|---|---|---|
| `scripts/fake-ssh` | `ssh.exe` | Drops options and the target and runs the server path locally on the same stdio. A missing path prints bash's "No such file or directory" and exits 127, as a remote shell would. |
| `gwin-fake-server` | `gliff-server` | Models the upstream behaviour the checks rest on (HelloAck, Ping, StreamConfig, Resize with clamp, byte-cap flow control, the 30 to 60 fps ladder, real OpenH264 frames of the test pattern). Faults are chosen with `GWIN_FAKE_SERVER`; the names are in [gwin-verify.md](gwin-verify.md#stream). |
| `scripts/fake-bin/hyprctl` | `hyprctl` | Answers `-j monitors all` and `-j clients` from the JSON the fake server writes to `$GWIN_FAKE_HYPR_DIR`. |
| `scripts/fake-bin/ffplay` | `ffplay` | Registers a window in the fake hyprctl world instead of opening one. |
| `scripts/fake-bin/wev` | `wev` | Prints the keysyms the `gwin-keys` script's `typed:` lines read. |
| `scripts/fake-bin/wl-copy`, `wl-paste` | `wl-copy`, `wl-paste` | Keep one clipboard (and its `--type`) in a file, `$GWIN_FAKE_CLIPBOARD`, so the remote half of `gwin-verify clipboard` runs on Linux. |
| `scripts/remote/gwin-keys` | (a real script) | Runs on the remote Hyprland machine: arms a test submap that logs every chord, key, click and wheel event, read back over ssh by `keys` and `pointer`. Its Linux tests use the fake `hyprctl`. Manual: [phase4/gwin-keys.md](phase4/gwin-keys.md). |
| `scripts/remote/gwin-pattern` | (a real script) | Runs on the remote Hyprland machine and shows the moving test pattern fullscreen on the gliff output. Its Linux tests use the fakes above. |

Example: start the server over the fake ssh, as the client does:

```
scripts/gwv build
scripts/gwv run ssh-headless -- ssh user@host --ssh "$PWD/scripts/fake-ssh" \
  --server-path "$PWD/target/release/gwin-fake-server"
```

What the stand-ins do not prove: real ssh behaviour (host keys, key auth,
`BatchMode`, `IPQoS`), the real server's handling of `--headless` and
`--output` (every gate passes `--headless`; `ssh --output auto` checks the default
command line against the fake), `CREATE_NO_WINDOW` (compiled only on Windows), decode timings on
the target CPU, the real Windows clipboard, and the Vulkan High 5.2 stream of
the real server (the fake encodes with OpenH264, so never above 3840x2160).

### The helper

`scripts/gwv` wraps the Linux side:

```
scripts/gwv build                 # release build of gwin-verify
scripts/gwv doctor                # one JSON line; ok needs fresh binaries
scripts/gwv clips [frames]        # x264 clips (including 3840x2400) into target/clips
scripts/gwv run <name> -- <args>  # run gwin-verify; keep .cmd/.jsonl/.stderr/.exit
scripts/gwv cleanup               # remove this run's scratch directory
```

`GWV_RUN` names the run; results go to `verify-evidence/$GWV_RUN/` (ignored by
git). Do not report a result from a stale binary: `doctor` says whether the
binary is newer than the sources.

## Test

One bullet per tool; what it needs is in the same line.

- **`cargo test --workspace --locked`**: unit and integration tests of the
  client and of `gwin-verify`. Needs nothing but the build tools. It covers
  wire framing (`cargo test -p gliff-windows --test framing`), the session
  core and the clipboard engine against `gwin-fake-server`, format conversion
  with golden bytes, the input case list and judges, and the remote scripts
  against the fake `hyprctl`. The slow, process-spawning tests are
  `gwin-verify`'s `stream`, `pattern_follow`, `ssh`, `keys_script` and
  `multi_session` (three sessions through `Tabs` and `SessionSet`, a per-session
  re-sync, and the client-CPU measurement).
- **The settings tests** (`settings`, `config`, `history` and `startup` in
  `cargo test -p gliff-windows`): precedence of flag over file over default for
  every setting, the file rules (tolerant load, `.broken` and `.replaced` copies, atomic save), the
  recent hosts rules and the chord overlap check. They use a temporary
  folder, never a real config folder. A test also fails if one of the three
  modules imports the `windows` crate.
- **`scripts/lint-terms`**: fails on personal data (home directories, private
  IP addresses, `user@host` forms with a real login) and on paths of documents
  that are not in the repository, in any tracked text file. The three lint files
  spell those paths out, so only the personal-data patterns and your private patterns
  scan them; the self-test builds its address-like strings at run time, so
  the files hold no address. `scripts/lint-terms --text FILE` checks other
  text, such as a pull request description.
- **`gwin-verify` on Linux**: `decode`, `ssh`, `make-clip`, `render` and
  `stream` run against the stand-ins. Needs `ffmpeg` with `libx264` for the
  clips (`scripts/gwv doctor` says whether it is there).
- **`gwin-verify.exe` on Windows**: `present`, `tabs`, `keys`, `pointer` and
  `clipboard`, the hardware gates. Each needs a real remote machine; the run
  order for `keys` and `pointer` is in [gwin-verify.md](gwin-verify.md#keys-and-pointer).
- **Remote scripts** (`gwin-keys`, `gwin-pattern`): run on the remote machine
  by the gates, and by hand as described in [phase4/gwin-keys.md](phase4/gwin-keys.md).

### Tests that race

A test that passes alone and fails on a busy machine has a bug: a hidden
dependency on timing or on state the tests share. Find it; do not retry the
test, loosen what it asserts, or lengthen a fixed sleep. In this repository:

- **Wait for what you can observe, not for time.** The `pattern_follow` tests
  poll the fake window world until the pattern is on the output (20 s limit,
  50 ms steps); `gwin-pattern` does not judge a window it read while the output
  was changing (`verify` returns 2; see the comment above it in the script).
- **Run tests together unless one reads a shared number.** The `reconnect`,
  `driver` and `multi_session` tests each use their own scratch directory,
  fake ssh and fake server processes, so they run at the same time. The one
  test that reads this process's CPU time (`what_three_busy_sessions_cost_the_client_in_cpu`)
  takes a write lock and runs alone; every other `multi_session` test takes
  the read lock. A new test that reads a process-wide number takes the write
  lock.
- **Share nothing between tests that run at once.** Each `pattern_follow` test
  has its own scratch directory and its own window title
  (`GWIN_PATTERN_TITLE`), because `gwin-pattern cleanup` stops every process on
  the machine whose command line holds the title.
- **Alone when the clock is the assertion, together when it is not.** In
  `crates/gwin-verify/tests/stream.rs` the four tests that gate the decoded fps
  hold a write lock, so each runs with no other stream test beside it. The nine
  that turn the gates off hold a read lock and run together; they also settle
  for 1 s and measure for 1 s instead of 5 s and 3 s, because what they assert
  does not depend on how long the stream ran. A test that needs a quiet machine
  takes the write lock; every other test takes the read lock.
- **Leave headroom in anything paced by the wall clock.** `gwin-fake-server`
  encodes real H.264 at 60 fps and `gwin-verify stream` gates the decoded fps.
  In a debug build the encoder's Rust half cost 65% of a core, so a busy machine
  could not keep the rate. The dev profile builds `openh264-sys2` and
  `gliff-sw` with `opt-level = 3` (root `Cargo.toml`); keep any new hot
  dependency of the fakes there.

To check a test for races, run its binary many times with more busy
processes than cores and count the failing runs:

```
cargo test -p gwin-verify --locked --no-run
bin=$(ls -t target/debug/deps/pattern_follow-* | grep -v '\.d$' | head -1)
cd crates/gwin-verify
for i in $(seq 16); do (while :; do :; done) & done   # 2x the cores of an 8-core machine
for i in $(seq 50); do ../../$bin >/dev/null 2>&1 || echo "run $i failed"; done
kill $(jobs -p)
```

## Run on a Windows machine

Before a run, check the machine:

- `gwin-verify.exe --version` matches the commit under test.
- `gwin-verify` starts the client through one function, `spawn_client`, which
  always adds a fresh temporary `--config-dir`, so a check never reads or
  writes a real settings file or history. A test fails if a launcher builds a
  client command line without it.
- `where ssh.exe` is the Windows OpenSSH client.
- `ssh -o BatchMode=yes user@host true` exits 0 with no prompt.

Then, from the folder with the binaries:

```powershell
.\gwin-verify.exe decode --kill-rule --clip .\1920x1080-x264high.gwclip --clip .\2560x1440-x264high.gwclip --clip .\3840x2400-x264high.gwclip
.\gwin-verify.exe ssh user@host
.\gwin-verify.exe stream --host user@host --size 3840x2400 --seconds 30
.\gwin-verify.exe present --host user@host --client .\gliff-windows.exe --seconds 20
.\gwin-verify.exe strip-shot --out .\strip-shots
.\gwin-verify.exe tabs --host a@one --host b@two --host c@three --client .\gliff-windows.exe
.\gwin-verify.exe keys --host user@host --client .\gliff-windows.exe --remote-keys /path/to/gwin-keys
.\gwin-verify.exe pointer --host user@host --client .\gliff-windows.exe --remote-keys /path/to/gwin-keys
.\gwin-verify.exe clipboard --host user@host --client .\gliff-windows.exe --only text,image,files
```

`strip-shot` needs no host and no input; it is the way to see the tab strip at every scale
on a machine whose own scale is fixed. The other commands in this list drive the keyboard
or mouse of the machine they run on (see the safety rules below).

The clips come from `scripts/gwv clips` on the build machine. The options of
each check are in [gwin-verify.md](gwin-verify.md).

## Safety rules for real-machine runs

These runs act on real machines. Read the whole list first.

- **`keys` and `pointer` take the keyboard of the remote machine.** While
  `gwin-keys arm` is on, that machine's own keyboard is in a test submap, so
  nobody uses it. Arm it for a bounded time (`--minutes 15`) and run
  `gwin-keys disarm` after.
- **On the Windows machine, do not touch the keyboard or mouse** during
  `present`, `tabs`, `keys` and `pointer`; they drive the machine with `SendInput` and a
  stray key fails the run or lands in another window.
- **`tabs` clicks the client and presses `Ctrl+Alt` chords** on the machine it
  runs on, after a warning and a countdown. Run it only at a machine you sit
  at, never from a script or unattended ([gwin-verify.md](gwin-verify.md#tabs)).
- **`keys --only lock` is the one step in which the client changes a Windows
  setting**: the per-user `DisableLockWorkstation` value. `gwin-verify` itself
  only reads the registry. If a run is cut short, `gliff-windows.exe
  --restore-lock-policy` puts the value back. Every other check runs the client
  with `--super-l off` or `chord`, which change nothing.
- **`clipboard` overwrites the clipboards of both machines.** Do not run it
  while something you need is copied. The `files` case writes a few MiB under
  temporary directories, which it removes.
- **No check touches the settings.** The client is started with its own
  temporary config folder, and `scripts/lint-registry` (Python 3) keeps the
  registry out of every module except the two that are meant to write it. It is
  a text check with [known limits](#known-limits-of-lint-registry), so it is
  not the only guard: review a change that touches system settings.
- **The way out of a grab** is the release chord, `Ctrl+Alt+Home` by default;
  Ctrl+Alt+Del always works.
- A hand test with the real hook captures your keyboard: know the release chord
  before you start.

## Security scans

`.github/workflows/security.yml` (the `Security` workflow) checks the
dependencies. It runs on every push to `main`, on every pull request that
changes `Cargo.toml`, `Cargo.lock`, `deny.toml`, the reviewed-advisory list, the
upstream check script or the workflow itself (a
Dependabot update always does), every Monday at 06:17 UTC, and on demand
(Actions tab, Run workflow). Its three jobs:

| Job (check name) | What it checks |
|---|---|
| `cargo deny` | `deny.toml`: RustSec advisories (unmaintained and unsound crates too, anywhere in the tree, and yanked versions), licences, banned crates and sources. Only the registry and the one upstream git repository in `deny.toml` are allowed. |
| `cargo audit` | `Cargo.lock` against the RustSec database with `--deny warnings`, so unmaintained, unsound and yanked crates fail it as well as vulnerable ones. |
| `upstream advisories` | `scripts/check-upstream-advisories`: published GitHub security advisories of the upstream repository the `gliff-*` crates are pinned from. |

Both Rust tools are built with `cargo install --locked` at the version in the
workflow's `env:` (`CARGO_DENY_VERSION`, `CARGO_AUDIT_VERSION`). Dependabot
cannot bump them: change the version by hand, and check the new release's
minimum Rust version (cargo-audit needs a recent stable). The workflow uses the
runner's stable Rust, not `rust-toolchain.toml`, because it builds tools, not
the client.

**The `gliff-*` crates come from git, not from a registry.** Both tools match
advisories by crate name from the crates.io index, so neither flags a git crate
(checked by planting an advisory for `gliff-proto` in a test database: it is
ignored for the git crate and reported for a registry crate such as `syn`). What
does cover them:

- Everything they depend on is in `Cargo.lock` and is checked like any other
  dependency.
- `deny.toml` allows exactly one git source, the upstream repository, and the
  `rev` in `Cargo.toml` and the lock file fix the commit.
- `upstream advisories` fails when the upstream repository publishes a security
  advisory that is not listed in `.github/reviewed-upstream-advisories.txt`.
  When it fails, read the advisory. If the pinned `rev` has the fix or is not
  affected, add the advisory id (`GHSA-...`) to that file, with a comment on
  what you found. If not, bump the `rev` (see the note in `Cargo.toml`).

  This sees only advisories upstream has published. A fix that upstream ships
  without one reaches this repository only when somebody bumps the `rev`.

**How you find out.** A failed run on a pull request shows on the pull request.
A failed scheduled run emails the person who last edited the `cron` line in
`security.yml` (GitHub's rule), and the Actions tab lists it. Keep your account's
notification setting for Actions on (Settings, Notifications, "Actions", at least
"Send notifications for failed workflows only"), and hand the cron line to
whoever is on duty by editing it. GitHub also turns scheduled workflows off after
60 days without repository activity; the Actions tab then shows a banner and
nothing is scanned until you re-enable it.

When a scan fails, do not weaken `deny.toml` to get green. Update or replace the
dependency; if there is no fix and the issue does not apply, add an `ignore`
entry to `deny.toml` (and `--ignore` for cargo-audit in the workflow) with the
advisory id and the reason.

## Known limits of lint-registry

`scripts/lint-registry` reads Rust text, not the compiled program. It catches
the registry API by name (any case), an import of the registry module in any
layout (grouped, aliased, glob, or the crate root such as `use {windows};`),
the `reg`, `regedit` and `regini` programs, PowerShell registry commands and
registry crates in a `Cargo.toml`. It does not catch:

- **A name built at run time**, such as `concat!("Reg", "SetValueExW")`, a
  `format!`, or a string cut into pieces: only a whole literal is read.
- **Escapes in a string**: `"\x52eg"` or `"\u{52}eg add"` is read as written,
  not as the text it stands for.
- **The PowerShell `Registry` provider and its class** in a command line that
  is built from parts (`[Microsoft.Win32.Registry]::SetValue(...)`), and any
  command that does not match the cmdlet and `HKxx:` patterns.
- **Settings that write the registry for a program**: `SystemParametersInfoW`
  with `SPIF_UPDATEINIFILE`, and `ChangeDisplaySettingsExW` with
  `CDS_UPDATEREGISTRY`, are Win32 calls with no registry name in them.
- **A string that is only `reg`** is flagged, though it may be a harmless word
  (a false positive); so is a whole string equal to a registry API name.
- Code that is generated by a macro or a build script, and a file in a
  language other than Rust (only `Cargo.toml` and `*.rs` are read).

Review such a change by hand, and add a self-test case with any rule that
closes one of these.

## Docs rules

- Write for a reader who has the repository and nothing else: no host names,
  user names, home directories, private addresses or hardware model names.
  Say "remote machine" (the Linux machine running Hyprland), "Windows
  machine" and "build machine".
- `scripts/lint-terms` enforces the personal-data and dead-path
  rules over every tracked text file, and `scripts/check` runs it. Check a
  pull request description with `scripts/lint-terms --text FILE` before you
  open the pull request.
- Link to other pages with relative links; the link target must exist.
- Record a measurement with the platform (Windows machine or build machine)
  and the command that reproduces it. Describe hardware generically ("one
  high-DPI laptop screen at 200%"), with no model names, clock times or time zones.

## The Rust toolchain

`rust-toolchain.toml` pins one exact stable Rust version (`channel`), with the
`rustfmt` and `clippy` components and the `minimal` profile. `rustup` picks it
up in this directory, so a local build, `scripts/check`, CI and the release
build all use the same compiler. `rust-version` in `Cargo.toml` is a separate
thing: the oldest Rust the code is allowed to need (1.89). The pin must be at
least that.

Bump the pin by hand, in its own small pull request:

1. Read the new stable's release notes, then run `rustup toolchain install
   <version> --profile minimal --component rustfmt,clippy`.
2. Change `channel` in `rust-toolchain.toml` to that exact version (for example
   `1.99.1`, never `stable`).
3. Run `scripts/check`. Fix any new clippy lint in the same pull request.
4. Add the Windows target to the new toolchain if you cross-build:
   `rustup target add x86_64-pc-windows-msvc --toolchain <version>`.

Dependabot does not bump this file; do it when a new stable has been out for a
week or two, or sooner for a Rust security fix.

## CI and release

`.github/workflows/ci.yml` runs on every pull request and every push to
`main`. It has four jobs. The names of the first, second and fourth are the names to
require in a branch ruleset; the Windows test job is new, so add it once it has
passed on `main`. A release waits for all four.

| Job (check name) | Runs on | What it does |
|---|---|---|
| `lint, fmt, clippy, test (linux)` | ubuntu | `lint-terms`, `cargo fmt --check`, clippy with `-D warnings`, `cargo test` on the default threads |
| `clippy, build (windows)` | windows | clippy with `-D warnings` and a release build for `x86_64-pc-windows-msvc` |
| `test (windows)` | windows | `cargo test -p gliff-windows`: the tests of the Windows-only code that can run without a desktop, such as the registry code in `lockreg.rs`. The hook, window and clipboard glue still need a hand check on Windows |
| `lint workflows` | ubuntu | `scripts/lint-workflows`: zizmor and actionlint over `.github/workflows` |

`.github/workflows/release.yml` runs when a `v*` tag is pushed:

1. `tagged commit is on main` fails unless the tagged commit is an ancestor of
   `origin/main`.
2. The four CI jobs run, called from `ci.yml`.
3. `build (windows)`, with a read-only token, refuses a `Cargo.toml` version
   that is not plain semantic versioning (`1.2.3` or `1.2.3-rc.1`) or that does
   not equal the tag without its `v`, and a `CHANGELOG.md` with no section for
   it. It builds both programs with no cache, checks that
   `gliff-windows.exe --version` prints the version, makes
   `gliff-win-vX.Y.Z-x86_64-pc-windows-msvc.zip` (both programs, `LICENSE`,
   `NOTICE`, `README.md`, `CHANGELOG.md`), `SHA256SUMS` and the release notes
   (`scripts/release-notes`), and uploads them as the `release` artifact.
4. `publish release`, the only job that can write, runs on a tag push only and
   runs no code from the repository. It checks the artifact against
   `SHA256SUMS`, attests the build provenance and creates the release
   (a prerelease when the version has a `-suffix`).

The same workflow runs as a dry run on a pull request that changes the
release files, and from the Actions tab (`workflow_dispatch`, any branch).
Steps 1 and 4 are skipped, so it builds the artifact and publishes nothing.
Download it from the run page to check it. Do not require the release jobs in
the `main` ruleset: they do not run on most pull requests. The steps to cut a
release are in [CONTRIBUTING.md](../CONTRIBUTING.md#releasing).

### Rules the workflows follow

These are enforced by `scripts/lint-workflows`, which CI runs, or by review.

- **Every action is pinned to a full commit hash** with the version in a
  comment, for example `uses: actions/checkout@<40 hex digits> # v7.0.1`. A
  version tag can be moved to other code by whoever controls the action's
  repository; a hash cannot. Dependabot's `github-actions` entry in
  `.github/dependabot.yml` understands the comment and proposes hash and
  comment together, so the pins stay current.
- **To add or bump an action by hand**, resolve the tag to a commit with
  `git ls-remote https://github.com/OWNER/REPO 'refs/tags/vX.Y.Z' 'refs/tags/vX.Y.Z^{}'`.
  If the second line (the peeled `^{}` one) exists, use its hash: the tag is
  annotated and the first hash is the tag object, not the commit. Otherwise use
  the first hash. Then run `scripts/lint-workflows`; its online checks confirm
  the hash belongs to the repository's history.
- **Least privilege**: the workflow `permissions:` is `contents: read`. Only
  the `publish release` job gets `contents: write` (create the release),
  `id-token: write` and `attestations: write` (sign the attestation), and it
  checks out no code. `GH_TOKEN` is set on the one step that publishes and
  nowhere else.
- **No stored credentials**: every `actions/checkout` sets
  `persist-credentials: false`, so the job token is not left in `.git/config`
  for later steps (or a build script) to read.
- **No `${{ }}` inside a `run:` script.** An expression there is pasted into the
  script text before the shell sees it, so a crafted value runs as code. Pass it
  through `env:` and read the shell variable instead.
- **Releases are built from `main` only** and **without a build cache** (a cache
  filled by another run is an input to the release binary).
- **NASM is pinned** to an exact version and SHA-256 by
  `scripts/install-nasm-windows.ps1`. To bump it, change `$version` and
  `$sha256` there in one pull request. Take the hash from
  `https://www.nasm.us/pub/nasm/releasebuilds/<version>/win64/`: download the
  zip over HTTPS, run `sha256sum` on it, and say so in the pull request, since
  NASM publishes no checksum file to compare with.

`scripts/lint-workflows` installs its two tools under `target/lint-workflows/`:
zizmor from PyPI at the version and wheel hashes in
`scripts/lint-workflows-requirements.txt` (`pip install --require-hashes`), and
actionlint with `go install` at a fixed version (Go checks the module hashes
against its checksum database). Configuration is in `.github/zizmor.yml`. Run
`scripts/lint-workflows --self-test` to see it fail on a deliberately bad
workflow. The zizmor job in CI also passes the job token (read-only) to zizmor so
its online checks can run; locally they are skipped unless `GH_TOKEN` is set.
To bump zizmor, put the new version in `scripts/lint-workflows-requirements.txt`
with the hashes of its wheels from the
[PyPI file list](https://pypi.org/project/zizmor/#files).

Two settings live in the repository on GitHub, not in this tree: the ruleset
that protects `main` and who may push `v*` tags. Review both before the
repository goes public. The ruleset's required checks must match the three
check names in the table above exactly; a name that never reports blocks every
merge.
