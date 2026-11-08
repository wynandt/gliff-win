# Stream statistics overlay

Users need live stream statistics without opening diagnostic files. The graph
button beside fullscreen toggles an opaque readout over the active picture.
The choice belongs to the window, starts off, and is not saved.

## Ownership

The protocol core counts received frames and video payload bytes. The driver
already measures decode calls and knows the decoder actually in use, including
CPU fallback. Its existing snapshot carries those values to the UI thread.
Each session view samples cumulative counters on the existing one-second
heartbeat. The immutable sample supplies both the readout and the additive
per-session `stream` field in the diagnostic JSON. Existing top-level fields
are unchanged.

The presenter caches a GDI text bitmap as a small texture and draws it after
video through the existing blit shader. It creates no child window. This
matters because the grab's pointer poll treats child windows as outside the
picture. A child readout would need new input policy. Rasterization and upload
happen only when content, bounds, or DPI change. Device recreation rebuilds
the cache. A stats-only redraw is not a new decoded-frame latency sample.

## Metric definitions

Received FPS counts VideoFrame messages. Mbit/s counts main and auxiliary video
payload bytes, excluding framing, SSH overhead, clipboard, and cursor traffic.
The decode mean includes every measured decode call, including calls that
buffer a picture or fail, but excludes separate idle drains. All rates use the
actual interval, not an assumed one second. An interval with no traffic reports
zero. A new connection attempt starts a new sampler.

Dimensions are the accepted StreamConfig's stream and remote view dimensions,
not the Windows window size or the dimensions of a later decoded picture.
The pipeline label distinguishes the server encoder tier from the actual
client decoder. An unknown server tier is `?`.

These definitions follow issue #41's received-FPS requirement. Linux at
32bb062 counts successful immediate decoded pictures and their decode times,
so edge-path semantics are deliberately different.

## Verification boundary

Host tests can prove counter arithmetic, sampling, JSON compatibility, layout,
and toggle actions. Cross-compilation checks the Windows code. Neither proves
that the real Windows window draws correctly or preserves grab behavior.
Native checks must cover clicks and scrolls through the readout, focus and
hover grab, tab changes, idle refresh, fullscreen reveal, DPI, and device
recovery. No native proof is claimed until those checks run on Windows.
