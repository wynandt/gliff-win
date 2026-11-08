# Stream statistics overlay

Users need live stream statistics without opening diagnostic files. The graph
button beside fullscreen toggles a translucent readout over the active picture.
The choice belongs to the window, starts off, and is not saved.

## Ownership

The protocol core counts received frames, immediate successful decode outputs,
and video payload bytes. The driver already measures decode calls and knows the decoder actually in use, including
CPU fallback. Its existing snapshot carries those values to the UI thread.
Each session view samples cumulative counters on the existing one-second
heartbeat. The immutable sample supplies both the readout and the additive
per-session `stream` field in the diagnostic JSON. Existing top-level fields
are unchanged.

The presenter caches a premultiplied BGRA text bitmap as a small texture and
blends it after video with a dedicated alpha-preserving shader. It creates no
child window. This matters because the grab's pointer poll treats child windows as outside the
picture. A child readout would need new input policy. Rasterization and upload
happen only when content, bounds, or DPI change. Device recreation rebuilds
the cache. A stats-only redraw is not a new decoded-frame latency sample.

## Metric definitions

The visible `fps` counts only decode calls returning `Ok(Some(picture))`, as
Linux does at 32bb062. Its decode mean uses the durations of those same calls.
Neither counts buffered/error calls, separate idle drains, slot deliveries,
redraws, or Windows presentations. Windows and Linux decoder work differs;
matching paths and denominators does not promise identical measured timings.

Mbit/s counts main and auxiliary video payload bytes, excluding framing, SSH,
clipboard, and cursor traffic. All rates use the actual interval. Idle intervals
report zero; a new connection attempt starts a fresh sampler. Diagnostic
`received_fps` still counts VideoFrame messages. `decode_call_ms` preserves the
all-measured-call mean, including buffer/error paths but excluding idle drains.
The experimental per-session `decode_ms` now means the successful-immediate-call
mean and `fps` is that output rate. Existing top-level fields are unchanged.
Samples belong to live sessions. Retirement removes the sampler, not a retained
history. The final whole-window shutdown snapshot can contain live samples
captured before shutdown, but already retired attempts have no stream entry.

Dimensions are the accepted StreamConfig's stream and remote view dimensions,
not the Windows window size or the dimensions of a later decoded picture.
The pipeline label distinguishes the server encoder tier from the actual
client decoder. An unknown server tier is `?`.

## Transparency correction

Linux CSS is `rgba(0,0,0,0.6)` with white monospace text. An opaque dark fill is
not equivalent. GDI draws a white grayscale mask on black. Coverage `c` becomes
premultiplied white RGB `c` and alpha `0.6 + 0.4*c`. Thus background pixels have
alpha 0.6, while solid glyphs remain white and opaque. BGRA bytes match the
existing texture format. Grayscale antialiasing avoids ClearType colour fringes.

A single cached texture and source-over blend were chosen over separate
background and glyph passes. Two passes need extra resources and blend state;
one premultiplied image hides that policy behind the existing overlay setter.
The presenter uses ONE/INV_SRC_ALPHA and restores opaque blending immediately
after drawing stats. Both video passes explicitly establish opaque state.
No new HWND, input policy, protocol, or pointer geometry is introduced.

## Verification boundary

Host tests can prove counter arithmetic, sampling, alpha math, JSON
compatibility, layout, and toggle actions. Cross-compilation checks the Windows code. Neither proves
that the real Windows window draws correctly or preserves grab behavior.
Native checks must cover clicks and scrolls through the readout, focus and
hover grab, tab changes, idle refresh, fullscreen reveal, DPI, and device
recovery. No native proof is claimed until those checks run on Windows.


## Repeatable checks

Run `scripts/verify-stats` for focused host logic and real-driver/fake-server
session identity tests. The integration clock is synthetic: it verifies identity
and export, not throughput. Run `scripts/check` for repository gates. The
[project-local verification skill](../.cursor/skills/verify-gliff-win/SKILL.md)
and its feature map describe the native journeys; they remain draft until a
Windows run captures their action and outcome.
