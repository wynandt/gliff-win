# End log

The client log has one line for each session that ends and one line when the
client exits, so a bug report can say why without guessing.

## Sub-features

- ENDLOG-1: `session <n>: ended: <reason>; ssh exit <code>` for each session
  that ends. The reason is Bye, a server error, an incompatible server, a lost
  link or an ssh failure.
- ENDLOG-2: after an ssh error, `; ssh stderr tail: ...` with at most 6 lines
  of 200 characters. File paths, host key fingerprints, key text, long hex
  digests, private key blocks and secret values are hidden. Control and
  invisible characters are removed, so a carriage return cannot fake a line.
- ENDLOG-3: a symbolic link, junction, folder or hard link in place of
  `ssh-stderr-<n>.log` is not read. The tail says
  `(not read: the file is not a plain file)`.
- ENDLOG-4: `client exit: <why>; exit code <n>` with a session tally after a
  normal close.

## How to get to it (user POV)

Open the client log (`gliff-windows-YYYY-MM-DD.log` in the log folder) after
a session ends or the client exits. See the README ("Why a session or the
client ended").

## Driving it with the native client

*Loopback.* Preconditions: the parent skill's isolated launch with its own
`--log-dir`.

- ENDLOG-1, ENDLOG-2 and ENDLOG-4: set `GWIN_FAKE_SSH_STDERR` to a file of
  ssh-like text that holds a fingerprint, a path, `password=...`, a carriage
  return, an escape code and U+202E, and `GWIN_FAKE_SSH_EXIT=255`. Close the
  window after `session 0: ended:`. Record both lines and search the whole log
  for each planted secret.
- ENDLOG-3: after `session 0: stream config`, replace `ssh-stderr-0.log` with
  a hard link or a junction to a file of known text, then end the session.
  Record the tail and check that none of that text is in the log. A symbolic
  link needs admin rights or Developer Mode. Do not change either setting.
- `GWIN_FAKE_SERVER=error` with `GWIN_FAKE_ERROR_FILE` drives a server error.

Host companions (not native proof):

- `cargo test -p gwin-verify --test end_log --locked`: every reason, a
  hostile server error and hostile ssh text.
- The `end_log` unit tests in `crates/gliff-windows`, including the symbolic
  link (Unix) and hard link cases.

## Runs

- 2026-11-21, Windows 11, loopback, build of PR #84 at `d27ac2f`:
  ENDLOG-1, ENDLOG-2, ENDLOG-3 (hard link and junction) and ENDLOG-4 passed.
  No planted secret was in any log. Symbolic link not run: no admin rights.

## Gotchas

A hard link already in place before the session starts is followed when the
client creates the stderr file, so ssh's text is written into the link's
target. The tail is still refused. This write is older than the end log and
is not fixed yet.
