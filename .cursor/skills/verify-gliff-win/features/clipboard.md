# Clipboard

Copy on one machine and paste on the other: text, pictures and files. The
other side only learns what kinds of data are available; the data moves
when something pastes.

## Sub-features

- CLIP-1: text, images and files travel both ways. HTML and rich text are
  built and tested on the build machine but have no hardware-gate case, so
  they are not claimed.
- CLIP-2: `--no-clipboard` or unticking *Share clipboard* turns sharing off;
  remote copies stay out of Win+V history and cloud sync unless
  `--clipboard-history` is set.
- CLIP-3: a remote copy stays with the tab that supplied it when you switch
  tabs; an empty offer from another tab cannot withdraw that copy.

## How to get to it (user POV)

Copy on one side and paste on the other while a session is live. Limits,
formats and file rules are in `docs/clipboard.md`. Use `--no-clipboard` on a
host you do not trust.

## Driving it with the native client

Preconditions: authorized disposable Windows session, `wl-clipboard` on the
remote machine, and nothing already on either clipboard that you need.
The check overwrites both clipboards.

- Regression: `gwin-verify.exe clipboard --only text,image,files`. It needs
  no focus and does not capture the keyboard. A run without `--only` also
  asks for `html` and `rtf`, which are still `pending` and fail. See
  `docs/gwin-verify.md` and `docs/clipboard.md` (test matrix and hardware
  gate).
- Hand cases with no gate yet (snips with `Win+Shift+S` while the keyboard
  is captured, HTML and rich text) stay in `docs/clipboard.md`; do not
  claim them.

Host companions (not native proof):

- `cargo test -p gwin-verify --locked` for the clipboard driver and judge
  against fake clipboards and the fake remote `wl-copy` / `wl-paste`.
- Client clipboard engine and format tests in `crates/gliff-windows`.

## Gotchas

The gate compares pictures and file trees with its own code, not the
client's. The `files` case writes a few MiB under temporary directories and
removes them at the end. The README records that `gwin-verify clipboard`
passed for text, images and files on a real pair. This map expansion did
not re-run it.
