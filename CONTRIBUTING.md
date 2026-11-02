# Contributing to gliff-win

gliff-win is a small Windows client for [Gliff](https://github.com/omacom/gliff),
maintained by one person. It is shared as a gift, with no promise of support.
This page keeps both sides' time cheap.

## The short version
- **Bug?** Open an issue with the form. Include versions and a reproduction.
- **Question or idea?** Use Discussions. It may not be picked up, and that is okay.
- **Fix?** Send a pull request. A PR is a proof of concept: the maintainer may
  merge it, change it on your branch, or re-implement it. Any of those is a yes.
- **Big change?** Open a Discussion first. The project has a narrow scope
  (a Windows window onto a Gliff session) and "no thank you" is a normal answer.

## Before you push
Run the same checks CI runs:

    cargo fmt --all
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked

`scripts/check --host-only` runs these checks and the repository's lints in one go;
see [docs/development.md](docs/development.md#check).

A change to how keys are held or let go (the hook, the grab, the key ledger,
the local chords) also runs the model check in
[formal/held-keys](formal/held-keys/README.md):

    formal/held-keys/run

Keep PRs small and about one thing. Explain the problem, the root cause and the
fix; the PR template shows how.

## What makes a change mergeable
Proof, not promises. Tools and assistants are welcome, for code, reports and
review. What counts is verification: show the change works against the real
thing (a command and its output, a log, a screenshot), and add a test that fails
before the change and passes after when you can. It does not matter who or what
wrote the change.

## Releasing
For the maintainer. A pushed `v*` tag runs `.github/workflows/release.yml`: it
checks the tag equals the `Cargo.toml` version and that `CHANGELOG.md` has a
section for it, runs CI, builds on Windows, and publishes a release with the
zip, `SHA256SUMS` and that section as the notes.

1. In a pull request, set `version` in the workspace `Cargo.toml` (run
   `cargo check` so `Cargo.lock` follows) and add its `## [X.Y.Z]` section to
   `CHANGELOG.md`. The release workflow's dry run on that pull request builds
   the zip and sums as a workflow artifact and publishes nothing.
2. Merge it, then tag the merged commit on `main`:

       git switch main && git pull --ff-only
       git tag -a vX.Y.Z -m "gliff-win X.Y.Z"
       git push origin vX.Y.Z

A tag on a commit that is not on `main`, or one that does not match the
version, fails before anything is built. More detail is in
[docs/development.md](docs/development.md#ci-and-release).

## Conduct
Be civil and assume good faith. The maintainer may close, lock or block anything
that is abusive or wastes time. There is no appeals process.

## Licence
By contributing you agree your work is released under the MIT licence in `LICENSE`.
