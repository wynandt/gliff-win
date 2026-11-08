# Tabs and independent readouts

Every connection has its own statistics; switching changes the shown readout.

## Sub-features

- TABS-1: strip clicks and switch chords select the expected tab.
- TABS-2: hidden sessions keep independent samples; reconnect starts fresh.

## How to get to it (user POV)

Add targets using the plus/address bar, then click tab bodies or use
Ctrl+Alt+PageDown, Ctrl+Alt+PageUp and Ctrl+Alt+1 through 9. Reconnect through
the tab menu. Enable the graph once; it stays selected across tabs.

## Driving it with the native client

Preconditions: isolated native launch with at least two authorized distinct
sources, recognizably different sizes or caps.

- Click and chord: enable stats, switch by both entry points; record shown
  session ID, sequence, readout and source. Only the selected source is drawn.
- Reconnect: use the tab menu; verify no old rates appear before a fresh sample.
- Regression: run the documented `gwin-verify.exe tabs` harness separately with
  three authorized targets. It owns its fresh config and sends real input.
- Host identity proof: `cargo test -p gwin-verify --test multi_session --locked stream_samples_follow_session_ids`.

## Gotchas

The host test uses real drivers and a fake-server boundary, not Windows tab
clicks. The native tabs harness has not been run for this change. Legacy JSON
top-level counters still describe the primary session; use `sessions[].shown`
and that entry's `stream`, not top-level counters, for readout comparisons.
