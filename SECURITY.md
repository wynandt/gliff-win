# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue:
go to the **Security** tab of this repository and choose **Report a vulnerability**.

gliff-win is maintained by one person in spare time. Expect an acknowledgement
within about a week; there is no bug bounty.

## What counts

gliff-win is a Windows client. It starts `ssh.exe` and speaks the Gliff protocol
over that pipe; it opens no listening port. A vulnerability is something that
lets a malicious or compromised Gliff server (or a network attacker) make the
client do something it should not: run code, read or write files outside its
remit, leak the clipboard to the wrong place, or crash it in an exploitable way.

Not vulnerabilities: needing physical access, a user running a binary they built
from a modified source, or issues in the upstream Gliff server (report those to
[omacom/gliff](https://github.com/omacom/gliff)).

## What the automated scans cover

A workflow checks the dependencies on every push to `main`, on every change to
`Cargo.toml` or `Cargo.lock`, and every week:

- known vulnerabilities, unmaintained and unsound crates, and yanked versions
  (RustSec advisory database, through cargo-deny and cargo-audit);
- licences, banned crates and unexpected sources (only crates.io and the
  upstream Gliff repository are allowed);
- security advisories that the upstream Gliff repository publishes.

They do not cover this repository's own code (that is clippy, tests and review),
flaws nobody has reported yet, or a fix that upstream Gliff ships without an
advisory. The details are in
[docs/development.md](docs/development.md#security-scans).

## Supported versions

Only the latest release is supported.

## How releases are protected

The build and release workflows are part of the attack surface, so they are
kept small and checked:

- Every GitHub Action is pinned to a full commit hash, not a movable tag.
- A release is built only from a commit that is already on `main`, with a pinned
  Rust toolchain, a pinned NASM and no build cache, and the release asset comes
  with a SHA-256 file and a build-provenance attestation.
- The workflow token is read-only except in the release job, and checkout does
  not leave it in the work tree.
- `scripts/lint-workflows` (zizmor and actionlint) runs in CI and fails on an
  unpinned action or a script-injection pattern.

The details and the bump procedures are in
[docs/development.md](docs/development.md#ci-and-release).
