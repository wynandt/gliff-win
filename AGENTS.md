# Agent and contributor guide

This file is for people and coding agents working in this repository.
Build, test, and product behaviour are documented in [README.md](README.md),
[CONTRIBUTING.md](CONTRIBUTING.md), and [docs/development.md](docs/development.md).
If this file and those pages disagree, follow those pages.

## What this project is

gliff-win is a Windows client for [Gliff](https://github.com/omacom/gliff).
It shows a remote Hyprland session in a normal Windows window, one tab per
machine, with ssh as the only transport. It speaks the stock `gliff-server`
protocol unchanged.

## Working with pstack

This repository follows [pstack](https://github.com/cursor/plugins/tree/main/pstack).
If you have pstack installed, start a task with [`/poteto-mode`]. It picks
the matching playbook. The process sections below map to these playbooks
and skills. Public copies live under the
[pstack tree](https://github.com/cursor/plugins/blob/main/pstack/):

- Bug fix uses the [bug-fix playbook]. Use [`/tdd`] when a cheap local test exists.
- Feature uses the [feature playbook]. Use [`/architect`] before you add a
  module, and [`/technical-writing`] for README-first docs.
- Refactor uses the [refactoring playbook].
- Investigation uses [`/how`] and [`/why`].
- Performance uses the [perf-issue playbook] and [`/benchmark-checklist`].
- Verification uses the repo [verify-gliff-win](.cursor/skills/verify-gliff-win/SKILL.md)
  skill. Use [`/maintain-verification-skill`] to keep it current.

If you do not have pstack, follow the plain process sections below.

## Choose a process

Pick one process and stay with it. Name it in the pull request.

### Bug fix

Reproduce the bug on the real Windows client first, with the same steps a
user would take. Host tests and cross-builds are not that reproduction.
Binary-search the cause until one mechanism is left, and keep runtime
evidence: a log, a failing test, or a trace. Plan the smallest change that
evidence justifies. Land a failing regression test in git history before the
fix when a cheap local test can capture the bug. Verify the original
reproduction on the same Windows client after the fix. A host-only result is
not a pass. Say so if you could not run the client.

### Feature

Trace the affected code. Update [README.md](README.md) and the linked docs
first when the change is user-visible, so the docs are the contract the code
must meet. Design types and module boundaries before you add a module. For an
unknown interaction, timing, or approach, build a throwaway prototype in a
scratch directory outside this tree, and keep it out of the product.
Implement in small units that you can build, test, and commit one at a time.
Verify on the real Windows client when the feature is user-visible.

### Refactor

Pin current behaviour with a characterization test or an equivalence check
before you move structure. Subtract dead code first. Change structure in
small steps that keep the pin green. Behaviour must not change. Prove that
on the real program or a recorded baseline, not by compilation alone. If you
find a bug or a missing feature, split it into its own change.

### Investigation

Investigations are read-only. They produce a cited explanation or a recommendation,
not a code change. If the answer calls for a code change, start a bug fix or a feature.

### Performance

Capture a baseline with the command and platform that produced it, as
[Docs rules](docs/development.md#docs-rules) require. Explain every number.
Say what you measured, on which machine (Windows machine or build machine),
and with which command. Change only what the measurement supports, cheapest
step first. Measure again on the same setup after the change, and put both
numbers in the pull request. An unexplained number is not evidence.

## Verify

Run the same checks CI runs before you push. The local gate is
`scripts/check`. Use `scripts/check --host-only` while you edit. It skips the
MSVC cross-build. Run the full gate before you push, or the host checks plus
the Windows jobs in CI:

    cargo fmt --all
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked

`scripts/check --host-only` also runs `scripts/lint-terms`,
`scripts/lint-ssh`, `scripts/lint-registry`, `scripts/lint-clip-log`,
`formal/held-keys/run`, and `scripts/lint-workflows`. CI runs those lints,
the held-keys model, a Windows clippy and release build, and
`cargo test -p gliff-windows` on Windows. See [CONTRIBUTING.md](CONTRIBUTING.md)
and [docs/development.md](docs/development.md#check).

If the change touches how keys are held or released, attach the output of
`formal/held-keys/run`. See [formal/held-keys](formal/held-keys/README.md).

For product behaviour, read the [verification skill](.cursor/skills/verify-gliff-win/SKILL.md)
and the [feature map](.cursor/skills/verify-gliff-win/features/README.md)
before you claim a journey works. Host helpers, picture judges, and
cross-builds are not native UI evidence. Prove the change on the real
artifact and include the command output, a log excerpt, or a screenshot in
the pull request, as the [pull request template](.github/pull_request_template.md)
asks. If a native journey did not run, say that. Do not claim it passed.
Never change ssh policy or provision host trust for a test.

## Repository rules

- ssh host-key checking stays at OpenSSH defaults. The client must not pass
  `StrictHostKeyChecking`, `UserKnownHostsFile`, `PasswordAuthentication`,
  `ForwardAgent`, or related options, and must not call `ssh-keygen`,
  `ssh-keyscan`, or `ssh-add`. Do not set `StrictHostKeyChecking` to
  `accept-new` or `no`. `scripts/lint-ssh` and the runner tests enforce this.
- The `gliff-proto`, `gliff-transport`, and `gliff-sw` crates are pinned
  to one git revision in the workspace `Cargo.toml`, currently the
  [Gliff v0.4.1](https://github.com/omacom/gliff/releases/tag/v0.4.1)
  commit. Bump that revision only deliberately, one upstream release at a
  time, in its own change. Speak the stock protocol unchanged. Do not add a
  protocol feature in this client until upstream has shipped it. Advertise
  only features this client implements (`session::FEATURES`).
- Docs are written for a reader who has this repository and nothing else.
  Use "Windows machine", "remote machine", and "build machine". See
  [Docs rules](docs/development.md#docs-rules). Scan a pull request
  description with `scripts/lint-terms --text FILE` before you open it.

## Pull requests

Keep a pull request to one focused change. Use the template: problem, root
cause, fix, and proof. For a user-visible change, update the README and the
linked docs in the same pull request, and write those docs before or with
the first commit of the behaviour they describe.

Do not merge a pull request, push a `v*` tag, or publish a release unless
the maintainer has approved that exact step. Release steps are in
[CONTRIBUTING.md](CONTRIBUTING.md#releasing).

[`/poteto-mode`]: https://github.com/cursor/plugins/blob/main/pstack/skills/poteto-mode/SKILL.md
[bug-fix playbook]: https://github.com/cursor/plugins/blob/main/pstack/skills/poteto-mode/playbooks/bug-fix.md
[`/tdd`]: https://github.com/cursor/plugins/blob/main/pstack/skills/tdd/SKILL.md
[feature playbook]: https://github.com/cursor/plugins/blob/main/pstack/skills/poteto-mode/playbooks/feature.md
[`/architect`]: https://github.com/cursor/plugins/blob/main/pstack/skills/architect/SKILL.md
[`/technical-writing`]: https://github.com/cursor/plugins/blob/main/pstack/skills/technical-writing/SKILL.md
[refactoring playbook]: https://github.com/cursor/plugins/blob/main/pstack/skills/poteto-mode/playbooks/refactoring.md
[`/how`]: https://github.com/cursor/plugins/blob/main/pstack/skills/how/SKILL.md
[`/why`]: https://github.com/cursor/plugins/blob/main/pstack/skills/why/SKILL.md
[perf-issue playbook]: https://github.com/cursor/plugins/blob/main/pstack/skills/poteto-mode/playbooks/perf-issue.md
[`/benchmark-checklist`]: https://github.com/cursor/plugins/blob/main/pstack/skills/benchmark-checklist/SKILL.md
[`/maintain-verification-skill`]: https://github.com/cursor/plugins/blob/main/pstack/skills/maintain-verification-skill/SKILL.md
