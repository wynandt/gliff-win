<!--
gliff-win is maintained by one person. A PR is a proof of concept: it shows the
change works and why it is wanted. It may be merged, reworked on your branch, or
re-implemented. Thanks for sending it either way.
-->

## Problem
<!-- What breaks or is missing, on what hardware/Windows version? Evidence welcome. -->

## Root cause
<!-- The first place the system went wrong, not the symptom. -->

## Fix
<!-- What you changed and why this approach. -->

## Proof
<!-- Show it works against the real thing. A test that fails before the
change and passes after is the best proof when you can add one. -->

- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` pass
- [ ] If this change touches how keys are held or let go (the hook, the grab,
      the key ledger, the local chords, or the release paths in the session
      or input modules): the output of `formal/held-keys/run` is attached
      below (or in a comment)
- [ ] Otherwise: the command you ran and its output, a log excerpt, or a
      screenshot is attached below (or in a comment)
