# Clipboard

Copy on one machine and paste on the other, both ways, for plain text, rich
text, images and files. The Linux machine runs the stock `gliff-server`, so the
client speaks upstream's clipboard protocol unchanged and translates between
Windows clipboard formats and MIME types.

This page is the behaviour spec. It also says what is checked how
([Test matrix](#test-matrix)) and what is not done yet
([Known gaps](#known-gaps)).

**Status.** Text, images and files are built in both directions and covered by
host tests. The hands-free gate (`gwin-verify clipboard`) ran on real machines.
The first run passed text and image and failed files at the
`refused-names-remote-to-windows` step, because the client refused the whole
offer when a name held a colon. After the fix, one build ran `files`, `text`
and `image` again and all three passed ([Hardware gate](#hardware-gate)).
HTML and rich text are written and tested on the build machine only, and the
gate has no case for them yet (`--only html` and `--only rtf` print "not built
yet"). Virtual files are not built.

## Tested where

Hand test on a real Windows machine and a real Linux machine with an
earlier build (the exact build was not recorded):

| What | Result |
|---|---|
| Text, both directions | Passed, including non-ASCII text (accented Latin and CJK). |
| Images, both directions | Passed: a Windows snip pasted on Linux, and a Linux screenshot pasted on Windows. Win+Shift+S stayed local, so the snip was taken on Windows. |
| Files and folders, both directions | Reported as passed, but not confirmed. The record of the tree (36 entries with tricky names) is not kept, and the first gate run found that a Linux name with a colon, such as `a:b?.txt`, made the client refuse the whole offer on Windows. The gate's `files` case passed after the fix, see [Hardware gate](#hardware-gate). |

Not passed, or not recorded:

| What | State |
|---|---|
| Gate cases not built | `gwin-verify clipboard --only html` and `--only rtf` print `pending: not built yet` and exit 1. Not passed, see [Hardware gate](#hardware-gate). |
| HTML and rich text | Host tests on the build machine. |
| Timings | None: no clipboard transfer has a measured throughput. |

[Test matrix](#test-matrix) has the layers; [windows-quirks.md](windows-quirks.md#clipboard)
has what Windows does to the clipboard.

## What the user sees

- Copy text, a formatted page, an image or files on one side and paste on the
  other. Nothing is pushed: the other side only learns what kinds of data are
  available, and the data moves when something pastes.
- A large paste (a 100 MiB file, a folder) takes as long as the link needs. A
  paste that is over the limits is refused cleanly, never cut short.
- Screenshots and snips work as copy and paste. Press Win+Shift+S while the
  keyboard is captured, take the snip, and paste it on the remote machine; copy
  a picture on the remote machine and paste it into Paint or a chat on
  Windows. See [Pictures and snips](#pictures-and-snips).
- Copying a password with a password manager that marks it private keeps it
  out of the other side's clipboard.
- Remote copies stay out of Win+V history and cloud sync unless you ask for
  them.

## Protocol (upstream v0.4.1)

The client mirrors upstream's client:

| Message | Meaning |
|---|---|
| `Offer { serial, mime_types, files }` | The sender's selection changed. Empty lists mean cleared or nothing forwardable. |
| `Request { id, serial, item }` | Send one item of the offer with that serial. A stale serial is refused with `Abort`. |
| `Data { id, offset, data_len, done }` + payload | One chunk of at most 256 KiB, at most 4 in flight. |
| `Ack { id, received }` | The receiver consumed this many bytes. |
| `Abort { id }` | Either side gives up on a transfer. |

Rules the client follows, as upstream's client does since 0.4.1:

- Every local change **revokes** the current offer first (stops serving it,
  refuses requests against it), then may send a new offer.
- Serials are **never reused**: the last serial is kept across revocations, and
  an empty offer takes a fresh serial.
- Request ids are odd on the client. A request with an even-parity id from the
  server is answered by the engine; one with our parity is refused.
- A transfer with no progress for 30 s is abandoned.

## Limits

The same constants as upstream, from `gliff-proto`:

| Limit | Value |
|---|---|
| One non-file item | 32 MiB (over: refused with `Abort`, the next copy works) |
| Files in one offer | 4 GiB total, 10,000 entries, 512 KiB of paths |
| Chunk | 256 KiB, 4 in flight |
| Idle transfer | 30 s |

## Formats

### Windows to Linux

The offer is built from the format names alone. No data is read at offer time,
except the file paths of a file copy, so other programs' delayed formats are not
forced.

| Windows format | MIME offered | On request |
|---|---|---|
| `CF_UNICODETEXT` | `text/plain;charset=utf-8` (and the other text flavours upstream collapses to it) | UTF-16 to UTF-8, CRLF to LF, trailing NUL removed; a lone surrogate becomes U+FFFD |
| `HTML Format` | `text/html` | the fragment, with a UTF-8 `meta` tag in front |
| `Rich Text Format` | `text/rtf` and `application/rtf` | bytes as they are, trailing NULs removed |
| `PNG` | `image/png` | bytes as they are |
| `CF_DIBV5` or `CF_DIB`, no `PNG` | `image/png` | converted to PNG at request time: 1, 4, 8 (palette), 16, 24 and 32 bit, BI_RGB and BI_BITFIELDS, top-down and bottom-up; RLE and embedded JPEG or PNG are refused. Alpha is kept only when the header has an alpha mask that is not zero everywhere (32-bit BI_RGB has an unused fourth byte, so it is opaque). A result over 32 MiB is refused |
| `JFIF`, `GIF` | `image/jpeg`, `image/gif` | bytes as they are |
| any other registered format whose name looks like a MIME type (`type/subtype`) | that name | bytes as they are |
| `CF_HDROP` | `text/uri-list` and `x-special/gnome-copied-files` come from the file list | the paths, walked (see [Files](#files)) |

Formats with no mapping (`CF_BITMAP`, `CF_METAFILEPICT`, OLE-only formats) are
not offered. Virtual files (copying out of a zip in Explorer, a mail
attachment, a browser download) are not offered either; see
[Known gaps](#known-gaps).

### Linux to Windows

A remote offer puts delayed-render formats on the Windows clipboard
(`SetClipboardData(format, NULL)`). The data is requested when a program asks
for the format, which blocks that program's `GetClipboardData` until the data
arrives or fails.

| MIME in the offer | Windows formats set | Conversion at render time |
|---|---|---|
| any text flavour | `CF_UNICODETEXT` | UTF-8 to UTF-16, LF kept (no CRLF added) |
| `text/html` | `HTML Format` | wrapped in a CF_HTML header with byte offsets |
| `text/rtf`, `application/rtf` | `Rich Text Format` | bytes as they are, one NUL added |
| `image/png` | `PNG` and `CF_DIBV5` (Windows adds `CF_DIB` and `CF_BITMAP`) | `PNG` as it is; `CF_DIBV5` decoded to 32-bit BI_BITFIELDS with alpha |
| `image/jpeg`, `image/gif`, `image/webp` and every other image type | a registered format named after the MIME (`JFIF` for jpeg, `GIF` for gif) | bytes as they are; no decoder, so Paint does not see webp, and jpeg does not become a bitmap |
| any other MIME | a registered format named after the MIME | bytes as they are |
| files in the offer | `CF_HDROP` and `Preferred DropEffect` (copy) | the whole file set is spooled to disk first, then the list is answered |

The marker format `application/x-gliff-windows-source` is put on the clipboard
whenever the client sets a remote offer, so the client can tell its own write
from another program's.

## Pictures and snips

A screen snip (Win+Shift+S, the Snipping Tool, Print Screen) leaves a bitmap
on the Windows clipboard: `CF_DIB` alone, and Windows makes the other bitmap
formats from it. The client turns that into `image/png` for the remote
machine when something there pastes, not when the snip is taken. In the other
direction a remote `image/png` is offered as `PNG` and `CF_DIBV5`, so Paint,
Office and chat programs paste it, with its alpha.

- Windows to Linux: a registered `PNG` is passed on as it is, and is preferred
  to a bitmap. Otherwise the bitmap is converted as the [format
  table](#windows-to-linux) says. An opaque bitmap becomes an RGB PNG, so a
  large snip stays small. The 32 MiB limit applies to the **PNG that is sent**,
  so a 4K snip (over 32 MiB as a bitmap) is copied when its PNG fits. A
  bitmap the client cannot read (RLE, an embedded JPEG) is refused for that
  paste only. A `PNG` the client cannot read is replaced by the bitmap if
  there is one.
- Linux to Windows: `PNG` is served as it is. `CF_DIBV5` is a 32-bit
  `BI_BITFIELDS` bitmap, bottom-up, with the alpha channel kept as it is in
  the PNG (not premultiplied). A picture of more than 64 megapixels (256
  MiB as a bitmap) is refused for that paste only.
- The echo guard, serials and history rules are the same as for text: a
  picture the client wrote is not sent back, a newer copy on either side
  revokes the older one, and a copy from the remote machine stays out of
  Win+V history unless `--clipboard-history` is given.

### Win+Shift+S while the keyboard is captured

While the keyboard is captured every key goes to the remote machine, so
Win+Shift+S would never reach Windows and the snipping overlay would not
open. The client therefore keeps a short list of *local chords* for Windows
(`--local-chords`, default `win+shift+s`). When the last key of a local chord
goes down with exactly the chord's modifiers held:

1. the client tells the remote machine to release every key it holds, so no
   modifier stays down there;
2. it replays the modifiers and the key to Windows, which never saw the
   modifiers go down;
3. the real key presses and repeats of the chord's last key are swallowed, and
   the real releases of the modifiers pass through, so Windows sees a normal
   Win+Shift+S and nothing stays held on either machine.

The snip is then taken on Windows and lands on the Windows clipboard, and the
clipboard sync offers it to the remote machine like any other copy.

Trade-off: for a moment the remote machine sees Win and Shift go down and
come up (step 1 releases them, so they never stay held). That is harmless on
a desktop that has no binding on a bare Win or Shift release; if it has one,
set `--local-chords none` and take snips on the Windows side without the
capture, or on the remote machine with its own tool. The list takes other
chords, for example `win+shift+s,win+v`; only the default is on, because a
chord kept for Windows is a chord the remote machine never sees.

Not covered: a snip tool that is started from outside the keyboard (the
Snipping Tool button, Print Screen on its own): Print Screen is forwarded to
the remote machine while the capture is on, like any other key.

## Text rules

- Windows to Linux: CRLF becomes LF. A lone CR stays.
- Linux to Windows: LF stays LF. Most Windows programs paste LF correctly;
  adding CR would change what the user copied.
- Unicode (CJK, emoji, right-to-left, combining marks) passes through exactly,
  as UTF-16 surrogate pairs where needed.

## Files

The problem: copy files or folders in Explorer and paste them into a Linux file
manager or a program that takes a file list, and the reverse, without a
shared drive. The two machines share no file system, so the contents travel
only when something pastes, and a paste must never see half a copy.

Windows to Linux: the paths of `CF_HDROP` are walked into the upstream list of
`relative path, size, dir`. The client has its own walker because upstream's
joins with `\` on Windows and rejects a name with an unpaired surrogate:

- The offer carries only that list. It carries no `text/uri-list` and no
  `x-special/gnome-copied-files`: the server makes both for its own programs
  from the list, with its spool paths, as it does for a Linux client.
- Nothing is read at copy time except metadata. A paste on the Linux machine
  makes the server request each file, and the client reads it from where it is.
  A file that changed size since the copy, or that is gone or locked, fails
  that paste, and the log gives the reason (see [The log](#the-log); it never
  names the file).
- Paths in the offer always use `/`.
- Symbolic links, junctions and other reparse points are skipped, so a loop
  cannot run the walk forever and a link cannot lead outside the copied tree.
- A path that cannot be listed (no access, gone) makes the whole offer empty, with
  a log line (the kind of failure, not the path), rather than offer part of a
  copy.
- A copied path's top-level name is its own file name; a drive root has none and
  is refused. Two copied paths with the same top-level name (from different
  folders) are both offered, the later ones as `name (2)`, `name (3)`, because
  the server refuses an offer with a repeated path.
- Zero-byte files and empty folders are offered as they are.
- A name with an unpaired surrogate (possible on NTFS) is offered with U+FFFD in
  its place. If that makes two names equal, the offer is refused and the log
  says so (`files_valid=false`, no name).
- The caps above apply while walking. A copy that is over them is refused
  whole, never cut.
- Cut in Explorer is treated as copy: the source stays.

Linux to Windows: the pasting side spools every file into
`%LOCALAPPDATA%\gliff-windows\clipboard`, one directory per offer
(`<process id>-<number>`), private to the user. The client offers
`CF_HDROP` and `Preferred DropEffect` (copy) at once and spools only when a
program asks for the list, so the program waits for the whole spool. The list is
handed over only when every file has arrived whole and its size matches the
offer; a failed or cancelled paste removes what it wrote and offers nothing, so
no partial file is ever listed. The spool's life:

- When the client starts it removes spool directories of processes that are not
  running any more, and none of a running client's.
- When the client exits it removes its own. Files are not rendered at exit.
- A replaced offer's spool lives 5 more minutes so a program that was just
  handed the list can still open the files.

A peer's offer is checked as text, not with the host's path parser, so the
check says the same on every OS. A path is refused if it is empty, starts with
`/`, has an empty, `.` or `..` part, or has a NUL; two equal paths, a folder
with a size, and an offer over upstream's caps (entries, path bytes, total
size) are refused too. A backslash or a colon is part of a name, as on Linux,
and the rename below removes both before anything is written. A refused offer
makes the whole list empty (`files_valid=false` in the log).

Names Windows refuses are renamed on disk only, never refused; the offer's
indexes stay stable:

- `<>:"\|?*` and control characters become `_`.
- A trailing dot or space is removed.
- Reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, `LPT1`
  to `LPT9`, with or without an extension) get a `_` in front.
- Two names that differ only in case, or collide after the changes above, get
  ` (2)`, ` (3)` and so on before the extension. This holds for top-level
  names too, so two copied folders with one name both arrive.
- Path parts longer than 255 UTF-16 units are shortened, and the full path is
  given to Windows with the `\\?\` prefix (from 240 characters) for the
  client's own file work. The top-level name is also shortened so the path
  handed to the pasting program stays under 260 characters; deeper items can be
  longer, and a program that does not support long paths cannot open those.
- The server's own walker skips symbolic links inside a copied folder, so a
  link on the Linux machine never leads outside the copied tree.

A transfer ends at the first failure (a short read, a stale serial, 30 s
without data, the link going down) and the log says which. Progress is in the
log: a `paste started` and a `paste ended` line for every paste (the whole
paste of files is one transfer, with its file count and size, never a name),
and `transfer final` for what the user would see. A progress window with
a cancel button is not built (see [Known gaps](#known-gaps)); the engine
already stops a transfer on command, and ending the client stops all of them.
Remote files stay out of Win+V history and cloud sync unless
`--clipboard-history`, like every remote copy.

## Echo loops and ownership

- The client sets the clipboard through its own message-only window and
  records the clipboard sequence number after each of its writes. An update
  with that number, or while the clipboard owner is our window, is ours and
  is ignored.
- Updates are debounced for 50 ms, because some programs write a clipboard in
  bursts.
- A remote offer revokes our own offer (and stops serving it) only when it
  takes over the Windows clipboard, which is when something in it would be
  written (a format Windows can hold, or files). An offer with nothing usable
  does not revoke our offer, as upstream's client does; if this window still
  owns the clipboard with a promise of an earlier remote offer, Windows clears
  that promise, and otherwise the user's clipboard is left alone. The
  case that matters is the server's empty offer at connect: both sides offer
  within a moment and both get serial 1, and the server's history watcher then
  reads our offer. Revoking for the empty one would make us refuse those reads
  with `Abort`, which the server shows as "Paste of text failed: the peer
  aborted the transfer" although nothing was copied. A local change revokes the
  remote offer by emptying the clipboard, but only when we still own it.
- A usable remote offer that arrives while the server is reading our offer
  still aborts that read, as upstream does: the clipboard really changed.
- Result: one copy produces one offer on each side, never a ping-pong.

## The log

The client's daily log (`gliff-windows-YYYY-MM-DD.log` in `--log-dir`) has one
line per clipboard event, in the form `YYYY-MM-DDTHH:MM:SSZ clipboard:
[session=N ]<event> key=value ...` (UTC). `session=N` is the tab's number, on the lines of its engine;
the Windows clipboard thread serves every tab and has none. Times have
second resolution, so the order of the lines is the order of events.

**The log never holds clipboard contents.** Not text, not a preview, not a
hash or a cut-off piece of it; and no file name, path or folder name (the
spool's own path included). A line has only an event kind, numeric ids, serials,
counts, sizes, durations, a MIME type name from a fixed table (`text/plain`,
`text/html`, `image/png`, ... and `other` for any other name, which can be
chosen by a program), and a reason from a fixed list. The rule is built in, not
just followed:

- `clip_log.rs` is the only writer of these lines. Its `Event` type has no
  free-form string field, so a call site cannot pass anything it read from a
  clipboard. `MimeName::of` and `MimeList::of` are the only entries that take a
  `&str` and they answer from a closed table.
- Errors cross threads as the closed `Cause` enum, never as a message: an I/O
  error is kept as its kind (`io_NotFound`) only, because its text can hold a
  path.
- `scripts/lint-clip-log` fails the check when a clipboard module writes a
  `clipboard:` line to the log by another way, or when `clip_log.rs` gains a
  string-typed field or entry that is not marked as the table lookup.
- A test feeds a sentinel string through offers, requests, transfers of text,
  pictures and files, failures and revocations, and fails if it appears in
  any line. Every line that any engine test logs is also checked against the
  forms a line may have (a fixed word, a number, an `io_<kind>`, or names from
  the mime table), so a leak that is not the sentinel, such as a hash or a cut-off
  piece, fails too.
- The few `clipboard:` lines that are written outside the clipboard modules
  (the client's start) carry an I/O error kind or a fixed phrase and a Win32
  error number, never the text of an error.

| Line | When | Fields |
| --- | --- | --- |
| `local change` | The Windows clipboard changed (after the 50 ms debounce) | `sequence`, `own`: `no` (a copy by the user or a program), or `sequence`, `owner`, `marker` (how this client knew a change was its own write; nothing is offered). A busy clipboard that is retried logs the same sequence again |
| `local change not offered` | A copy that is not forwarded at all | `sequence`, `cause` (`private`, `nothing_forwardable`) |
| `local files not offered` | The files of a copy are not forwarded; its other flavours still are | `sequence`, `cause` (`too_many_files`, `files_too_large`, `no_file_name`, `files_invalid`, `io_*`) |
| `clipboard busy` | Another program held the clipboard open | `result` (`copy_not_forwarded`, `old_remote_copy_stays`, `remote_copy_dropped`) |
| `offer sent` | This client offered its clipboard (no types and no files withdraws) | `serial`, `generation`, `mimes`, `files` (count), `file_bytes` |
| `offer received` | The server offered its clipboard | `serial`, `mimes`, `files`, `file_bytes`, `files_valid` |
| `offer applied` | What Windows got of that offer | `serial`, `result` (`promised`, `cleared`) |
| `offer revoked` | The local offer was withdrawn while it was offered or served. `remote_offer` is logged when a server offer that would take over the clipboard arrives, also for a tab in the background, which does not apply it; an offer with nothing usable (such as the empty one at connect) logs no revoke | `why` (`local_change`, `remote_offer`, `tab_left`, `sharing_off`), `serial`, `serving` (transfers cut off) |
| `request received` | The server asked for an item of the local offer | `id`, `serial`, `mime` or `file_index` |
| `request refused` | This client turned that request down; an `abort sent` follows. A request with an id of this client's own parity is answered by the transfer layer and has an `abort sent` only | `id`, `cause`, `initiator=local` |
| `request sent` | This client asked for an item of the server's offer | `id`, `serial`, `mime` or `file_index` |
| `abort received` | The server aborted a transfer | `id`, `transfer` (`serving`, `fetching`), `initiator=peer` |
| `abort sent` | This client aborted a transfer (the server's fetch fails with "the peer aborted the transfer") | `id` |
| `serve started` | A body is being sent | `id`, `bytes` (text and pictures), `read_ms` (the clipboard read) |
| `serve ended` | A served transfer is over | `id`, `result` (`done`, `cut`, `failed`), `bytes`, `ms`. `done` means this client sent it all; a server that aborted while the clipboard was still being read is not noticed, so `abort received` can come before `serve started` and `serve ended result=done` |
| `paste started` | A program on Windows pastes a remote copy | `paste`, `serial`, `mime` or `files` and `file_bytes` |
| `paste ended` | That paste is over. A second paste of the same remote files reuses the first one's copies: no request, `bytes=0` | `paste`, `result`, `cause` and `initiator` (`local`, `peer`, `link`, `timeout`) on a failure, `bytes`, `ms` |
| `paste cancelled` | The user cancelled a running paste; its `paste ended` has `cause=cancelled` | `paste`, `initiator=local` |
| `transfer final` | A transfer reached its end, as the user's progress would show. Its number is the progress window's, not the paste's or the request's | `transfer`, `state` |
| `spool entry not removed` | A leftover spool entry could not be deleted | the kind of I/O error |

A `cause` is one of: `no_offer`, `stale_serial`, `not_offered`, `unknown_file`,
`revoked_while_reading`, `read_timeout`, `read_unanswered`, `over_limit`,
`clipboard_changed` (a newer copy replaced the offered one),
`clipboard_sequence_moved` (the clipboard changed since the offer was built),
`clipboard_busy`, `thread_busy`, `not_shown` (a background tab may not read),
`not_ready`, `format_missing`, `image_conversion`, `offer_replaced`, `no_files`,
`files_invalid`, `wrong_file_size`, `peer_aborted`, `peer_gone`,
`idle_timeout`, `chunk_protocol`, `cancelled` (the user), `dropped` (the
session ended), `private`, `nothing_forwardable`, `too_many_files`,
`files_too_large`, `no_file_name`, or `io_<kind>` (the
name of a Rust `std::io::ErrorKind`, such as `io_NotFound`).

Reading a "peer aborted" toast on the other machine: find the `abort sent id=N`
line. The line before it, `request refused id=N cause=...`, is the reason (the
clipboard moved, was busy, was withdrawn, ...). An `offer revoked` with
`serving` above 0 at that moment, and a `serve ended ... result=cut`, say it was a
withdrawal. A transfer that the server itself abandons leaves `abort received`
and no `abort sent`. Three numbers name a paste: `paste` (this client's own
count, on the `paste` lines), the request `id` (odd, on the request, serve and
abort lines) and the progress window's `transfer`.

## Privacy

- A local item that has `ExcludeClipboardContentFromMonitorProcessing`, or
  `CanIncludeInClipboardHistory` set to 0, or `CanUploadToCloudClipboard` set
  to 0, or `Clipboard Viewer Ignore`, is sent as an empty offer.
- A remote offer that lists `x-kde-passwordManagerHint` gets the same three
  privacy formats even with `--clipboard-history` (a password is never kept in
  history or sync). The server itself trusts the connected host and withholds
  nothing.
- **Remote copies stay out of Win+V history and cloud sync by default.** The
  client sets `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` to 0
  and `ExcludeClipboardContentFromMonitorProcessing` on what it puts there,
  because history and sync read every format at copy time and that would force
  every remote copy to be transferred at once, up to 4 GiB for files. The option
  `--clipboard-history` leaves them out, so remote copies can appear in history
  and sync.
- The spool is per user and never world readable.
- The log has no clipboard contents and no file or folder names, by
  construction (see [The log](#the-log)).
- `--no-clipboard` turns the whole feature off: no offer is sent, none is
  accepted, and the clipboard is never touched.

## Client options

| Option | Default | Meaning |
|---|---|---|
| `--no-clipboard` | off | No clipboard at all |
| `--clipboard-history` | off | Let remote copies appear in Win+V history and cloud sync |
| `--local-chords LIST` | `win+shift+s` | Key combinations that stay on Windows while the keyboard is captured, comma separated, for example `win+shift+s,win+v`; `none` keeps nothing local. Each is modifiers (ctrl, alt, shift, win) and one key |

## Trying it (text)

Plain text is the part to try first; it is the part that has a gate case.

1. Install `wl-clipboard` on the remote machine (`wl-copy` and `wl-paste`),
   and use a Gliff server that is v0.4.1 or newer.
2. Start the client as usual, for example `gliff-windows.exe user@host`. The
   clipboard is on unless you pass `--no-clipboard`.
3. Copy text on Windows, then paste in a program on the remote screen. Copy
   text on the remote screen, then paste on Windows. Line ends follow the
   [text rules](#text-rules): CRLF becomes LF on the way to the remote
   machine, and LF stays LF on the way back.
4. Remote copies stay out of Win+V history and cloud sync; pass
   `--clipboard-history` to allow them.
5. The log in `--log-dir` records each offer, request and transfer with its
   serial and the reason it ended (see [The log](#the-log)); read it first
   when a paste does not arrive.

To run the same check without hands (it **overwrites both clipboards**), on
the Windows machine, with no other copy in progress:

```
gwin-verify.exe clipboard --host user@host --only text
```

It starts the client with `--super-l off`, copies on one side, reads the
other over ssh with `wl-copy` and `wl-paste`, and prints one JSON line per
step (ASCII, Unicode, CRLF, LF, then a quiet period in which nothing may
change). `--only html,rtf` name cases that are not built yet and fail as
pending.

## Trying it (pictures and snips)

1. Do the steps for text first.
2. Press Win+Shift+S while the keyboard is captured, take a snip, and paste it
   on the remote machine into a program that takes images. On the remote
   machine, `wl-paste --type image/png > snip.png` then holds the snip.
3. Copy a picture on the remote machine (for example `wl-copy --type
   image/png < picture.png`) and paste on Windows into Paint.
4. The hand test that has no gate case: step 2 with the real hook. The gate
   runs without the keyboard captured, so only the clipboard half of a snip is
   automatic.

The same check without hands (it **overwrites both clipboards**):

```
gwin-verify.exe clipboard --host user@host --only image
```

The steps are a 1920x1080 picture put on Windows as a snip bitmap, a small
translucent picture with the `PNG` format, the same two the other way (with
the alpha checked), a check that a remote copy carries the history-exclusion
mark, a 3840x2160 snip, and a quiet period. Pictures are compared pixel by
pixel after decoding, by the gate's own PNG and bitmap code, not the client's.

## Trying it (files)

1. Do the steps for text first.
2. Copy a file and a folder in Explorer, and paste in the Linux machine's file
   manager. The folder arrives with its contents. `wl-paste --type
   text/uri-list` on the Linux machine lists the pasted paths.
3. Copy a file and a folder in the Linux file manager and paste in Explorer.
   A big copy makes Explorer wait until the whole copy has arrived.

The same check without hands (it **overwrites both clipboards**, and writes
a few MiB under the user's temp directory on Windows and under
`~/.cache/gwin-verify-clip` on the remote machine, both removed at the end):

```
gwin-verify.exe clipboard --host user@host --only files
```

The steps are a folder with a Unicode name and nested folders, a zero-byte file
and a 6 MiB file copied on Windows and compared on the remote machine, the same
shape the other way (with the drop effect and the history mark checked), names
Windows refuses and a case collision from the remote machine, a quiet period,
and, after the client has ended, that its spool is gone. Trees are compared by
path, size and SHA-256 by the gate's own code and tar reader and writer, not
the client's.

## Design

- **One thread owns the Windows clipboard** (`clip_win`): a message-only window,
  `AddClipboardFormatListener`, `OpenClipboard` with retry (another program
  can hold it for a short time), delayed rendering, and the render of every
  promised format when the window is destroyed (`WM_RENDERALLFORMATS`). A
  blocking render stalls only this thread, never the picture.
- **One thread runs the transfer engine** (`clip_engine`): upstream's
  `Transfers` on a current-thread runtime with a `LocalSet`, plus the bridge
  that keeps the serials and the spool. It takes `Command`s from the Windows
  thread, answers with `HostEvent`s, and talks to the session through the
  existing writer. A paste returns whole bodies (at most 32 MiB) or spool
  paths, so the Windows thread never handles chunks.
- **The engine thread is started by the session** (`clip_link`) when the
  clipboard is on. The server's five clipboard messages reach it through the
  protocol state machine (also before the first `StreamConfig`, because a
  server may offer its selection at once); what it sends goes into the same
  writer as every other message, with the `Data` payload. At the end of a
  session the engine stops first, so stdin closes and the server exits.
  `--no-clipboard` starts neither thread.
- **The rules are pure** (`clip_core`): what a local change becomes, what a
  remote offer becomes, privacy, the echo guard and the debounce. Serials,
  revocation order and limits are in `clip_engine` and are tested against
  upstream's engine playing the remote side.
- **Format conversion is pure** (`clip_formats`, `clip_files`), so it is tested on
  the build machine with golden bytes.
- Design C of the plan: the classic Win32 clipboard for text, HTML, RTF, images,
  other MIME types and files by spool. Streaming through OLE (`IDataObject`),
  with no spool and with virtual files, is a later phase that replaces the
  `FileSource` seam without touching the rest.

## Test matrix

| Layer | What proves it | Where |
|---|---|---|
| Format conversion | golden bytes, round trips, Unicode, DIB variants, name sanitizer | host tests |
| State machine and engine | offers, requests, stale serials, the empty server offer at connect not aborting reads of our offer, 33 MiB refusal, abort, idle timeout, no ping-pong, against `gwin-fake-server` | host tests |
| Windows clipboard thread | `cargo xwin clippy` and build | build machine |
| Keyboard chords | the decision for a local chord, and that a chord is not forwarded and leaves no stuck modifier on the server, against `gwin-fake-server` through the hook's routing | host tests |
| Keyboard hook injection | `cargo xwin clippy` and build; the replay itself runs only on a real Windows machine | build machine, then hand test |
| Files: spool, walker, engine | a fake file system on a temporary directory: nested and Unicode trees, zero bytes, 16 MiB, names Windows refuses, collisions at any depth and at the top, short and vanished files, abort and cancel, rapid copies, stale serials, spool sweep and exit cleanup | host tests |
| `gwin-verify clipboard` cases | driver and judge against a fake pair of clipboards; the remote half against `scripts/fake-ssh` and a fake `wl-copy`/`wl-paste`; the cases that need the real clipboard and the remote machine run only at the gate | host tests, then hardware |

### Hardware gate

`gwin-verify clipboard` takes no keyboard or mouse capture, but it
**overwrites the clipboards of both machines**.

Two runs, each on a real Windows machine and a real Linux
machine, one `--only` value at a time.

**First run, before the fix.** `text` and `image` passed. `files` failed at
`refused-names-remote-to-windows`: nothing arrived. The client log said
`files_valid=false`. Upstream's path check reads each name with the host's path
parser, and on Windows `a:b?.txt` is a drive prefix and a name, so the whole
offer was refused. The two tree steps before it passed. The steps after it
(`drop-effect-and-history`, `quiet`) did not run; only the spool check after
exit did. Not a pass.

The Linux host tests missed it because the same check accepts every name on
Linux. The offer is now checked as text in the client, the same on every OS,
and a host test holds that.

**Second run, after the fix.** One build of the fixed code ran each
case; the whole run took about 1 minute 10 seconds. `files`, `text` and `image`
each exited 0 and ended with `clipboard-summary ok:true cases:1 passed:1
failed:[]`. Each line is one step that passed:

| `--only` | Exit | Steps, in order |
|---|---|---|
| `files` | 0 | `tree-windows-to-remote`, `tree-remote-to-windows`, `refused-names-remote-to-windows`, `drop-effect-and-history`, `quiet`, `spool-removed-at-exit` |
| `text` | 0 | `ascii-windows-to-remote`, `ascii-remote-to-windows`, `unicode-windows-to-remote`, `unicode-remote-to-windows`, `crlf-windows-to-remote`, `lf-remote-to-windows`, `quiet` |
| `image` | 0 | `snip-windows-to-remote`, `png-alpha-windows-to-remote`, `picture-remote-to-windows`, `alpha-remote-to-windows`, `remote-copy-stays-out-of-history`, `4k-windows-to-remote`, `quiet` |
| `html` | 1 | Not built: prints `pending: not built yet`, summary `passed:0 failed:["html"]`. Not a pass. |
| `rtf` | 1 | Not built: prints `pending: not built yet`, summary `passed:0 failed:["rtf"]`. Not a pass. |

The first `files` attempt of the second run failed at `tree-windows-to-remote`
with `the copy failed: the Windows clipboard stayed busy`, because the Windows
screen was locked and a locked desktop does not let a program open the
clipboard. The same command, run again unchanged on the unlocked machine,
passed. The gate needs an unlocked Windows desktop.

What a pass covers is the steps above and nothing more. Text of 1, 16 and 33
MiB, a lone CR, a privacy-flagged item, rapid back-to-back copies, the
no-ping-pong check, and killing the source mid-transfer have no step in the
gate code; they are host tests or hand tests. `html` and `rtf` are not built.

The cases below are the plan for the gate. Those that name an `--only` value
have a step in `gwin-verify clipboard`; the others do not yet:

| Case | Both directions | Pass |
|---|---|---|
| ASCII, tabs, Unicode | yes | exact under the text rules |
| CRLF policy | Windows to Linux CRLF to LF; Linux to Windows LF kept; lone CR kept | exact |
| 1 MiB and 16 MiB text | yes | exact |
| 33 MiB | yes | refused within 1 s, no hang, the next copy works |
| HTML table with non-ASCII | yes | fragment equal after decoding |
| Picture 1920x1080 (`--only image`) | yes; Windows side from a snip bitmap alone | pixels exact |
| Translucent picture (`--only image`) | yes; Windows side from `PNG` and bitmap | pixels exact, alpha included |
| 3840x2160 snip (`--only image`) | Windows to Linux | PNG within 32 MiB arrives, pixels exact |
| Remote copy and history (`--only image`) | Linux to Windows | the history-exclusion mark is set |
| Quiet end (`--only image`) | both | neither clipboard changes |
| Files: nested folder with a Unicode name, a zero-byte file, a 6 MiB file (`--only files`) | yes | path, size and sha256 equal |
| Names Windows refuses, case collisions (`--only files`) | Linux to Windows | renamed as above, contents equal |
| History mark and drop effect on a remote file copy (`--only files`) | Linux to Windows | mark set, copy effect |
| Quiet end and spool gone after exit (`--only files`) | both | nothing changes; no spool directory of the client's process is left |
| A 100 MiB file | yes | sha256 equal (hand test, not in the gate: it would take minutes) |
| Privacy-flagged item | Windows to Linux | the Linux side sees an empty offer |
| Rapid back-to-back copies | yes | stale serial refused, last copy wins |
| Source killed mid-transfer | Linux to Windows | abort, no hang, no partial file listed (host tests; the kill itself by hand) |
| No ping-pong | after each copy | at most one offer per copy over 5 s |

## Known gaps

- Virtual files (zip contents from Explorer, mail attachments, browser
  downloads) and streaming a big file without a spool: a later phase.
- jpeg, gif, webp and other image types pass through under their own name;
  there is no decoder, so they do not become a Windows bitmap. A Linux program
  that offers only `image/jpeg` therefore does not paste as a picture into
  Paint. Programs that offer `image/png` (screenshot tools, browsers, most
  image editors) do.
- A snip is converted when something pastes it, so a very large one takes a
  moment on the first paste.
- Win+Shift+S stays on Windows only as a local chord (see above). It worked in
  the hand test; the gate does not exercise the hook.
- Dot-files copied on Linux arrive on Windows only if the file manager's
  selection includes them (show hidden files first). The pasted file does not
  get the Windows hidden attribute, so it looks like any other file.
- Explorer trims a leading space from a folder name when it pastes.
- Primary selection and file drag-and-drop do not exist upstream either.
- A remote offer's formats are set at once, so a program that reads every
  format when the clipboard changes will force the transfer (history and sync
  are kept away by default; third-party clipboard managers are not).
- A spool directory of a process that crashed stays until the next client
  starts. A reused process id keeps an old directory until that process ends.
- There is no progress window and no cancel button: progress and failure are in
  the log, and a paste that the program gives up on ends at the 30 s idle limit
  at the latest.
- Cut in Explorer is a copy; the source is not removed.
- Files inside an archive Explorer shows as a folder, and files on a cloud
  placeholder that is not on disk, are virtual or not local; see the first gap.
- A path below the top level that is longer than 259 characters is spooled but
  only opened by programs that support long paths.
- Paste into a Linux terminal depends on that program: it receives what the
  server offers (`text/uri-list` and the file manager form), not a path as text.
