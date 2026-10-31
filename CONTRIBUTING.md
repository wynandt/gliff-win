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

Keep PRs small and about one thing. Explain the problem, the root cause and the
fix; the PR template shows how.

## What makes a change mergeable
Proof, not promises. Tools and assistants are welcome, for code, reports and
review. What counts is verification: show the change works against the real
thing (a command and its output, a log, a screenshot), and add a test that fails
before the change and passes after when you can. It does not matter who or what
wrote the change.

## Conduct
Be civil and assume good faith. The maintainer may close, lock or block anything
that is abusive or wastes time. There is no appeals process.

## Licence
By contributing you agree your work is released under the MIT licence in `LICENSE`.
