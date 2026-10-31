# Held-keys model check

A TLA+ model of how the client holds and lets go of keys, checked with TLC.
Run it when a change touches hook.rs, grab.rs, grab_state.rs, key_ledger.rs,
chord_watch.rs, local_chords.rs, route.rs, or the release paths in
session/machine.rs or input.rs, and before merging such a change.

## The promise

Every key the client sends down to the remote machine is sent up again, or
the remote is told to let go of every key, by the time things settle. And
Windows never counts a key as down that the user has let go of because of
something the client did (a swallowed Up, an injected Down).

The harm when this breaks: a stuck key. On the remote, a held letter
repeats or a held Ctrl, Alt, Shift or Win turns every later key into a
shortcut. On Windows, the same happens to the local desktop after the grab.
Either one is a key the user did not press, acting on their machine.

The model holds no real key codes and the check reads no keys. The client
must never log keystrokes; nothing here records them.

## Run

```sh
formal/held-keys/run
```

About half a minute. The first run downloads a pinned TLC (tla2tools) and,
on x86-64 Linux, a pinned Java runtime into `target/tla-tools`, checking
their SHA-256 sums (set `HELD_KEYS_TOOLS` to keep them elsewhere). On
another system it uses `java` (11 or later) from `PATH`. TLC's output goes
to `target/held-keys`.

It checks `model/HeldKeys.tla` with `model/Gate.cfg`, which must pass.
Exit 1 when it does not.

A known finding, when there is one, gets its own `model/NAME.cfg` (the gate
widened to the case) and an entry in the `findings` list in `run`. It must
still break the rule it is known to break: exit 2 when it no longer does, or
breaks a different rule. The code changed, so find out why, and update this
README. `run gate` checks the gate only.

To read a result, take the path TLC prints for the config that broke and
replay it against the code, one step per action.

## The rules (INVARIANTs)

"Settled" means: no command waiting for the session, no key message
waiting for the window, no release chord fired and not yet taken in by the
window, no sweep pending, the screen unlocked, and the route agreeing with
the session (both there or both gone).

- `RemoteLetGo`: settled and not grabbing, the remote holds no key.
- `RemoteFollowsUser`: settled, the remote holds only keys the user holds.
- `WindowsFollowsUser`: settled, Windows counts down only keys the user holds.
- `TakenIsHeld`: settled with the hook installed, the local chords keep a
  taken trigger only while the user holds it, so the next press of a key is
  never swallowed.

## What the model covers

Paths are under `crates/gliff-windows/src`. Each action in the model names
the code it stands for.

| Code | Functions | Model |
|---|---|---|
| hook.rs | `Ctx::handle`, `Ctx::grab`, `Ctx::local_chord`, `Ctx::windows_gets`, `Ctx::release_remote`, `set_mode`, `note_legacy`, `HookThread::start`, `release_at_grab_end`, `release_at_reinstall`, `physical_saw`, `saw_later` | `HookSeesKey`, `Apply`, `UiLegacyKey`, `AfterGrabEndSweep`, `AfterReinstallSweep` |
| key_ledger.rs | `KeyLedger` (all), `Physical::stuck_at_grab_end`, `stuck_at_reinstall`, `moved`, `saw_later`, `KeyId` | `ledgerWin`, `ledgerRemote`, `physView`, `msgs` (superseded) |
| chord_watch.rs | `ChordWatch::push`, `seed`, `after_reinstall`, `Taker::fate` (release only), `HookGate` | `watch`, `gate`, `fired` |
| local_chords.rs | `LocalChords::push`, `reset` | `localHeld`, `localTaking` |
| grab.rs | `Grab::apply`, `release_chord`, `tick` (overdue, sweep), `legacy_key`, `session` (unlock) | `Apply`, `UiEvent`, `TakeInReleaseChord`, `ReleaseOverdue`, `SweepAfterUnlock`, `UiLegacyKey` |
| grab_state.rs | `GrabState::compute`, `handle` (focus, click, release chord, timeout, lock) | `UiMode`, `FocusLost` ... `Unlock` |
| route.rs | `InputRoute::send` | `CanSend`, `SendOn` |
| session/machine.rs, input.rs, keymap.rs | `on_ui` for `Key`, `ReleaseInput`, `ReleaseGrab`, `Bye`; `KeyPipeline` release chord; `KeyState::release_all` | `SessionTakesCmd`, `SessionEnds` |
| pass_log.rs | `silent_hook_judge` | `UiLegacyKey` |

Synced with commit 1839f39 plus the commits that changed this line, which
make the fixes noted under the findings below. Move this forward only after
reading the changes to the files above since that commit and updating the
model to match.

Keys: `Mod` stands for any one modifier, `Rel` for the release chord's
trigger (chord Mod+Rel), `Loc` for a local chord's trigger (chord Mod+Loc).

Bounds in the gate: 5 physical key events, 4 UI events (focus, click,
timeout, lock, unlock), one silent hook, one session end and reconnect, at
most two key messages queued for the window. No auto-repeat.

## Assumptions

- `Grab::apply` is one step. It sets the gate first, and a hook in Off mode
  passes every key, so a hook key in the middle sees what it would see after.
- The server lets go of every key when its session ends.
- Keys pressed or let go on the lock screen still reach this desktop's key
  state (`LockHidesKeys = FALSE`). With `TRUE`, Windows keeps a key down even
  with no grab at all, so the model cannot tell the client's part.
- The window handles key messages already queued for it before the user can
  move focus, click, or lock, and before the grab times out.
- `Grab::legacy_key` is one step. On the code it runs on the UI thread
  while the hook thread runs on: a hook event of the same key that lands
  between its `saw_later` check and `note_legacy` (microseconds) is not
  covered.
- Keys the client injects (the local chord replay, the sweeps) pass the hook
  untouched. That includes the nested hook call from `SendInput` inside
  `local_chord` while GATE is held: `hook_proc` cannot borrow the context
  and passes the key before it locks anything.

## Known findings

None open. A finding is a stuck key on current code, replayed on the pure
parts (`KeyLedger`, `LocalChords`, `silent_hook_judge`) in the order
`Ctx::handle` calls them, with the Windows glue read by hand.

Fixed: with no session, the hook passed keys to Windows without telling
`LocalChords`, so a modifier let go then still counted as held, and the next
local chord in the grab replayed it to Windows. `Ctx::windows_gets` now
pushes the key to the local chords too. The gate covers keys with no route
(`"gone"`).

Fixed: the Up of a taken local-chord trigger that reached Windows as a
local key (the session ended in between) left `LocalChords::taking` set, so
the next press of that key was swallowed. `LocalChords::push` now ends the
take on that Up as well. `TakenIsHeld` covers it.

Fixed: with the session ended but the route still pointing at it, the
trigger of a local chord was marked taken before `local_chord` found it
could not send. The Down reached Windows unrecorded and its Up was
swallowed, so Windows held the key. `Ctx::grab` now cancels the take and
sends the key on as any other, so Windows gets it and its Up. The gate
covers keys in that gap (`GapKeys = {"stale", "gone"}`).

Fixed: with the hook silent, two key Downs queued up as window messages.
The first was sent and the hook installed again. The user let go of the
second key, and the new hook sent its Up to Windows. Then the window handled
the out-of-date Down and sent it to the remote, which held the key.
`Grab::legacy_key` now drops a message the hook has overtaken: it handled a
later event of the same physical key (`Physical::saw_later`, by the
event times; left and right Shift, Ctrl and Alt are different keys). The gate
covers two queued messages (`MaxBacklog = 2`).

When a fix lands, its finding config stops breaking (exit 2): widen the
gate to cover that case, drop the finding config and its `run` entry, and
update this list.

## What it does not cover

Hover follow and SuperL-only mode, left and right modifiers (one `Mod`),
typing and IME, AltGr, the switch chord and tab switches (the route
ordering on a switch), more than one modifier per chord, auto-repeat, `KeyPolicy` filtering, pointer buttons, the
watchdog after a hard kill, `shutdown`, and timing (a property that needs a
deadline). A pass says nothing about keys beyond the bounds above.

## A real bug or the model?

Replay the path against the code, one step per action, as a test on the
pure parts or a live repro. If the code does not do what the step says,
the model is wrong: fix the model, not the rule. Never weaken a rule or
narrow the gate to make a new break go away; a new break is a finding until
the replay shows otherwise.

## Keeping it true

After a change to the covered code, update the matching action, run the
check, and move the sync commit forward.
