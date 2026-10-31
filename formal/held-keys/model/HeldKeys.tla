---------------------------- MODULE HeldKeys ----------------------------
(***************************************************************************)
(* Held keys across a grab: every key the client lets the remote machine   *)
(* hold down is let go of, and Windows never counts a key down that the    *)
(* user no longer holds, whatever order the grab, the release chord, a     *)
(* local chord, focus, the lock, a silent hook and its reinstall, and the  *)
(* session ending come in.                                                 *)
(*                                                                         *)
(* Each action names the code it stands for (crates/gliff-windows/src).    *)
(* One action is one step under one lock in the code, except where the     *)
(* comment says why a UI-thread sequence may be one step here.             *)
(*                                                                         *)
(* Keys: "Mod" stands for any one modifier (Win, Ctrl, Alt or Shift);      *)
(* "Rel" is the trigger of the release chord Mod+Rel; "Loc" is the         *)
(* trigger of a local chord Mod+Loc (local_chords.rs). The model holds no  *)
(* real key codes and the client logs none.                                *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS MaxKeyEvents, MaxUiEvents, MaxSilences, MaxSessionEnds, MaxInbox,
          LockHidesKeys, \* TRUE: keys on the lock screen never reach this desktop's key state
          MaxBacklog,     \* key messages that may queue up for the window while the hook is silent
          GapKeys        \* when a key may reach the live hook with no open session:
                         \* "stale" (route still on an ended session), "gone" (no route)

Keys == {"Mod", "Rel", "Loc"}
Mods == {"Mod"}
None == "none"

VARIABLES
    \* The world
    phys,          \* keys the user physically holds
    winDown,       \* keys Windows counts as down (GetAsyncKeyState)
    \* hook.rs statics, all under GATE
    physView,      \* PHYSICAL: Physical::keys, what the hook saw last
    gate,          \* GATE: HookGate::mode ("Off" | "Grab")
    fired,         \* GATE: HookGate::fired (release chord fired, not yet taken in)
    ledgerWin,     \* LEDGER: KeyLedger slots with `windows`
    ledgerRemote,  \* LEDGER: KeyLedger slots with `remote`
    watch,         \* WATCH: ChordWatch::keys
    \* hook.rs Ctx, owned by the hook thread, new on every install
    localHeld,     \* Ctx::chords (LocalChords::held)
    localTaking,   \* Ctx::chords (LocalChords::taking)
    ctxLast,       \* Ctx::last_mode
    \* grab.rs Grab::hook and Windows' view of it
    hookInstalled, \* Grab::hook is Some
    hookLive,      \* Windows still calls the hook (false: it went silent)
    \* route.rs InputRoute and the session (session/machine.rs, input.rs)
    routeTarget,   \* InputRoute::target is Some
    chanOpen,      \* the session's command channel is open
    chan,          \* UiCmd queue to the session, in order
    remote,        \* KeyState::held of the session = what the remote holds
    \* the UI thread (grab.rs, grab_state.rs)
    uiInbox,       \* UiEvent::GrabReleased not yet handled by the window
    msgs,          \* key messages that reached the window while the hook was silent:
                   \* <<key, press, superseded (the hook saw a later event of the key)>>
    focused, armed, locked, \* GrabState flags (follow = focus)
    sweepPending,  \* Grab::sweep armed after an unlock
    \* bounds
    keyEvents, uiEvents, silences, sessionEnds

vars == <<phys, winDown, physView, gate, fired, ledgerWin, ledgerRemote, watch,
          localHeld, localTaking, ctxLast, hookInstalled, hookLive,
          routeTarget, chanOpen, chan, remote, uiInbox, msgs,
          focused, armed, locked, sweepPending, keyEvents, uiEvents, silences, sessionEnds>>

\* GrabState::compute with follow = focus (no typing, secure merged into locked).
UiMode(f, a, l) == IF f /\ a /\ ~l THEN "Grab" ELSE "Off"
uiMode == UiMode(focused, armed, locked)

Set(S, k, press) == IF press = "Down" THEN S \cup {k} ELSE S \ {k}

\* InputRoute::send: true only with a target whose channel is open.
CanSend == routeTarget /\ chanOpen
SendOn(ch, c) == IF CanSend THEN Append(ch, c) ELSE ch
KeyCmd(k, press) == <<"Key", k, press>>
ReleaseInput == <<"ReleaseInput", None, None>>
ReleaseGrab == <<"ReleaseGrab", None, None>>

\* Physical::stuck_at_grab_end via hook::release_at_grab_end: an injected Up
\* for each modifier Windows counts down that the hook did not see held.
AfterGrabEndSweep(view) == winDown \ {m \in Mods : view[m] # "Down"}
\* Physical::stuck_at_reinstall via hook::release_at_reinstall.
AfterReinstallSweep(wd, view) == wd \ {m \in Mods : view[m] = "Up"}

TypeOK ==
    /\ phys \subseteq Keys /\ winDown \subseteq Keys
    /\ physView \in [Keys -> {"Unknown", "Up", "Down"}]
    /\ gate \in {"Off", "Grab"} /\ fired \in BOOLEAN
    /\ ledgerWin \subseteq Keys /\ ledgerRemote \subseteq Keys /\ watch \subseteq Keys
    /\ localHeld \subseteq Mods /\ localTaking \in Keys \cup {None}
    /\ ctxLast \in {"Off", "Grab"}
    /\ remote \subseteq Keys

Init ==
    /\ phys = {} /\ winDown = {}
    /\ physView = [k \in Keys |-> "Unknown"]
    /\ gate = "Off" /\ fired = FALSE
    /\ ledgerWin = {} /\ ledgerRemote = {} /\ watch = {}
    /\ localHeld = {} /\ localTaking = None /\ ctxLast = "Off"
    /\ hookInstalled = FALSE /\ hookLive = FALSE
    /\ routeTarget = TRUE /\ chanOpen = TRUE /\ chan = <<>> /\ remote = {}
    /\ uiInbox = 0 /\ msgs = <<>>
    /\ focused = FALSE /\ armed = TRUE /\ locked = FALSE /\ sweepPending = FALSE
    /\ keyEvents = 0 /\ uiEvents = 0 /\ silences = 0 /\ sessionEnds = 0

\* LocalChords::push for a local key: the Up of the taken trigger ends the take.
EndTake(lt, k, press) == IF k = lt /\ press = "Up" THEN None ELSE lt

\* A physical key event is possible (no auto-repeat in the model).
CanPress(k, press) ==
    /\ keyEvents < MaxKeyEvents
    /\ IF press = "Down" THEN k \notin phys ELSE k \in phys

-----------------------------------------------------------------------------
(* The hook thread: hook_proc -> Ctx::handle, one step under GATE.          *)

\* Ctx::handle with the hook installed and live.
HookSeesKey(k, press) ==
  /\ hookInstalled /\ hookLive /\ ~locked
  /\ CanPress(k, press)
  /\ \/ routeTarget /\ chanOpen
     \/ "stale" \in GapKeys /\ routeTarget /\ ~chanOpen
     \/ "gone" \in GapKeys /\ ~routeTarget
  /\ phys' = Set(phys, k, press)
  /\ keyEvents' = keyEvents + 1
  /\ physView' = [physView EXCEPT ![k] = press]           \* physical_saw
  /\ msgs' = [i \in DOMAIN msgs |->                        \* ... with its time
                IF msgs[i][1] = k THEN <<k, msgs[i][2], TRUE>> ELSE msgs[i]]
  /\ LET now == gate
         changed == now # ctxLast
         lh0 == IF changed THEN {} ELSE localHeld            \* chords.reset()
         lt0 == IF changed THEN None ELSE localTaking
         takeRem == changed /\ ctxLast = "Grab"               \* release_remote()
         lr0 == IF takeRem THEN {} ELSE ledgerRemote
         ch0 == IF takeRem /\ ledgerRemote # {} THEN SendOn(chan, ReleaseInput) ELSE chan
         \* watch_decides -> ChordWatch::push (decide, then apply)
         releaseChord == now # "Off" /\ k = "Rel" /\ press = "Down"
                         /\ "Mod" \in watch /\ "Rel" \notin watch
         w0 == IF now # "Off" THEN Set(watch, k, press) ELSE watch
     IN
     /\ ctxLast' = now
     /\ CASE
        \* Fate::Release: gate.chord_fired(), WATCH cleared, ReleaseGrab sent;
        \* if sent, take_remote_held() and swallow, else Windows gets the key.
           releaseChord /\ now = "Grab" /\ routeTarget ->
             /\ gate' = "Off" /\ fired' = TRUE /\ watch' = {}
             /\ chan' = SendOn(ch0, ReleaseGrab)
             /\ ledgerRemote' = IF CanSend THEN {} ELSE lr0
             /\ winDown' = IF CanSend THEN winDown ELSE Set(winDown, k, press)
             /\ UNCHANGED ledgerWin
             /\ localHeld' = lh0 /\ localTaking' = lt0
        \* Fate::Pass (no target): windows_gets() notes it in Grab, in the
        \* ledger and in the local chords (as a local key); key passes.
        [] ~routeTarget ->
             /\ ledgerWin' = IF now = "Grab" THEN Set(ledgerWin, k, press) ELSE ledgerWin
             /\ winDown' = Set(winDown, k, press)
             /\ watch' = w0 /\ chan' = ch0 /\ ledgerRemote' = lr0
             /\ localHeld' = IF now = "Grab" /\ k \in Mods THEN Set(lh0, k, press) ELSE lh0
             /\ localTaking' = IF now = "Grab" THEN EndTake(lt0, k, press) ELSE lt0
             /\ UNCHANGED <<gate, fired>>
        \* HookMode::Off: the key passes.
        [] now = "Off" ->
             /\ winDown' = Set(winDown, k, press)
             /\ watch' = w0 /\ chan' = ch0 /\ ledgerRemote' = lr0
             /\ localHeld' = lh0 /\ localTaking' = lt0
             /\ UNCHANGED <<gate, fired, ledgerWin>>
        \* Ctx::grab
        [] OTHER ->
          /\ watch' = w0 /\ UNCHANGED <<gate, fired>>
          /\ IF k \in ledgerWin
             THEN \* KeyLedger::route -> Route::Windows { remote }
               /\ ledgerWin' = IF press = "Up" THEN ledgerWin \ {k} ELSE ledgerWin
               /\ ledgerRemote' = IF press = "Up" THEN lr0 \ {k} ELSE lr0
               /\ localHeld' = IF k \in Mods THEN Set(lh0, k, press) ELSE lh0
               /\ localTaking' = EndTake(lt0, k, press)
               /\ chan' = IF k \in lr0 THEN SendOn(ch0, KeyCmd(k, press)) ELSE ch0
               /\ winDown' = Set(winDown, k, press)
             ELSE IF k \in Mods \/ (k # lt0 /\ press = "Up")
                     \/ ~(k = "Loc" /\ press = "Down" /\ lh0 = Mods /\ k # lt0)
             THEN IF k = lt0
               THEN \* ChordDecision::Swallow (the taken trigger)
                 /\ localTaking' = IF press = "Up" THEN None ELSE lt0
                 /\ localHeld' = lh0
                 /\ chan' = ch0 /\ ledgerRemote' = lr0
                 /\ UNCHANGED <<ledgerWin, winDown>>
               ELSE \* ChordDecision::Forward: send; if not sent Windows gets it
                 /\ localHeld' = IF k \in Mods THEN Set(lh0, k, press) ELSE lh0
                 /\ localTaking' = lt0
                 /\ chan' = SendOn(ch0, KeyCmd(k, press))
                 /\ IF CanSend
                    THEN /\ ledgerRemote' = Set(lr0, k, press)
                         /\ UNCHANGED <<ledgerWin, winDown>>
                    ELSE /\ ledgerRemote' = lr0
                         /\ ledgerWin' = Set(ledgerWin, k, press)
                         /\ winDown' = Set(winDown, k, press)
             ELSE \* ChordDecision::Take -> Ctx::local_chord (taking = trigger)
               /\ localTaking' = IF CanSend THEN k ELSE lt0 \* else cancel_take()
               /\ localHeld' = lh0
               /\ IF CanSend
                  THEN \* ReleaseInput, take_remote_held, replay mods + trigger
                    /\ chan' = Append(ch0, ReleaseInput)
                    /\ ledgerRemote' = {}
                    /\ ledgerWin' = ledgerWin \cup lh0
                    /\ winDown' = (winDown \cup lh0) \ {k}
                  ELSE \* not done: sent as a key, which fails, so Windows gets it
                    /\ chan' = ch0 /\ ledgerRemote' = lr0
                    /\ ledgerWin' = Set(ledgerWin, k, press)
                    /\ winDown' = Set(winDown, k, press)
  /\ UNCHANGED <<hookInstalled, hookLive, routeTarget, chanOpen, remote, uiInbox,
                 focused, armed, locked, sweepPending, silences, sessionEnds, uiEvents>>

\* Windows took the hook away without a word (it answered too slowly):
\* keys reach Windows and the window as messages.
HookGoesSilent ==
  /\ hookInstalled /\ hookLive /\ silences < MaxSilences
  /\ hookLive' = FALSE /\ silences' = silences + 1
  /\ UNCHANGED <<phys, winDown, physView, gate, fired, ledgerWin, ledgerRemote, watch,
                 localHeld, localTaking, ctxLast, hookInstalled, routeTarget, chanOpen,
                 chan, remote, uiInbox, msgs, focused, armed, locked, sweepPending,
                 keyEvents, sessionEnds, uiEvents>>

\* A key while the hook is installed but silent: Windows sees it and the
\* window gets a key message (handled by UiLegacyKey).
SilentHookKey(k, press) ==
  /\ hookInstalled /\ ~hookLive /\ ~locked
  /\ CanPress(k, press)
  /\ Len(msgs) < MaxBacklog
  /\ phys' = Set(phys, k, press) /\ keyEvents' = keyEvents + 1
  /\ winDown' = Set(winDown, k, press)
  /\ msgs' = Append(msgs, <<k, press, FALSE>>)
  /\ UNCHANGED <<physView, gate, fired, ledgerWin, ledgerRemote, watch, localHeld,
                 localTaking, ctxLast, hookInstalled, hookLive, routeTarget, chanOpen,
                 chan, remote, uiInbox, focused, armed, locked, sweepPending,
                 silences, sessionEnds, uiEvents>>

\* A key with no hook installed: Windows has it.
NoHookKey(k, press) ==
  /\ ~hookInstalled /\ ~locked
  /\ CanPress(k, press)
  /\ phys' = Set(phys, k, press) /\ keyEvents' = keyEvents + 1
  /\ winDown' = Set(winDown, k, press)
  /\ UNCHANGED <<physView, gate, fired, ledgerWin, ledgerRemote, watch, localHeld,
                 localTaking, ctxLast, hookInstalled, hookLive, routeTarget, chanOpen,
                 chan, remote, uiInbox, msgs, focused, armed, locked, sweepPending,
                 silences, sessionEnds, uiEvents>>

\* A key on the lock screen (secure desktop): the hook does not see it.
\* Whether this desktop's key state does is an assumption (LockHidesKeys).
LockedKey(k, press) ==
  /\ locked
  /\ CanPress(k, press)
  /\ phys' = Set(phys, k, press) /\ keyEvents' = keyEvents + 1
  /\ winDown' = IF LockHidesKeys THEN winDown ELSE Set(winDown, k, press)
  /\ UNCHANGED <<physView, gate, fired, ledgerWin, ledgerRemote, watch,
                 localHeld, localTaking, ctxLast, hookInstalled, hookLive, routeTarget,
                 chanOpen, chan, remote, uiInbox, msgs, focused, armed, locked,
                 sweepPending, silences, sessionEnds, uiEvents>>

-----------------------------------------------------------------------------
(* The UI thread: Grab::dispatch -> GrabState::handle -> Grab::apply.       *)
(* apply is one step here: it calls hook::set_mode (under GATE), drops or   *)
(* installs the hook, sends ReleaseInput and runs release_at_grab_end. A    *)
(* hook key between those calls runs with the gate already set, so it sees  *)
(* the same mode it would after the whole of apply.                         *)

\* Grab::apply(out) for GrabState going from `before` to `after`, with the
\* flags in their new values. `ack` is release_handled() right after
\* (Grab::release_chord). Fixes every hook-side and UI-side variable.
Apply(before, after, ack) ==
  LET installNow == after = "Grab" /\ ~hookInstalled
      newGate == IF after = "Grab" /\ ~fired THEN "Grab" ELSE "Off"   \* HookGate::request
      released == before = "Grab" /\ after = "Off"                      \* Release::All
  IN
  /\ gate' = newGate
  /\ fired' = IF ack THEN FALSE ELSE fired
  \* hook::set_mode: forget_windows when not grabbing; seed on a new grab.
  /\ ledgerWin' = IF newGate = "Off" THEN {}
                  ELSE IF gate # "Grab" THEN winDown ELSE ledgerWin
  /\ ledgerRemote' = IF newGate = "Grab" /\ gate # "Grab" THEN {} ELSE ledgerRemote
  /\ watch' = IF newGate = "Off" THEN {}
              ELSE IF gate = "Off" THEN winDown \cap Mods ELSE watch
  \* Off: hook = None. Grab: install() if there is none (a new Ctx).
  /\ hookInstalled' = (after = "Grab")
  /\ hookLive' = IF after = "Grab" THEN (installNow \/ hookLive) ELSE FALSE
  /\ ctxLast' = IF installNow THEN "Off" ELSE ctxLast
  /\ localHeld' = IF installNow THEN {} ELSE localHeld
  /\ localTaking' = IF installNow THEN None ELSE localTaking
  \* Release::All: ReleaseInput, then release_at_grab_end.
  /\ chan' = IF released THEN SendOn(chan, ReleaseInput) ELSE chan
  /\ winDown' = IF released THEN AfterGrabEndSweep(physView) ELSE winDown

\* Assumption: the window handles the key messages already queued for it
\* before the user can move focus, click, lock the screen or the grab times
\* out (see README.md).
UiEvent(f2, a2, l2) ==
  /\ msgs = <<>>
  /\ uiEvents < MaxUiEvents /\ uiEvents' = uiEvents + 1
  /\ focused' = f2 /\ armed' = a2 /\ locked' = l2
  /\ Apply(uiMode, UiMode(f2, a2, l2), FALSE)

FocusLost == focused /\ UiEvent(FALSE, armed, locked)
              /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                             sweepPending, keyEvents, silences, sessionEnds>>
FocusGained == ~focused /\ UiEvent(TRUE, armed, locked)
              /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                             sweepPending, keyEvents, silences, sessionEnds>>
Click == ~armed /\ UiEvent(focused, TRUE, locked)
              /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                             sweepPending, keyEvents, silences, sessionEnds>>
GrabTimesOut == uiMode = "Grab" /\ UiEvent(focused, FALSE, locked)
              /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                             sweepPending, keyEvents, silences, sessionEnds>>
Lock == ~locked /\ UiEvent(focused, armed, TRUE)
              /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                             sweepPending, keyEvents, silences, sessionEnds>>
\* Grab::session(Unlocked) arms the sweep.
Unlock == locked /\ UiEvent(focused, armed, FALSE) /\ sweepPending' = TRUE
              /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                             keyEvents, silences, sessionEnds>>

\* Grab::release_chord: dispatch(ReleaseChord) then hook::release_handled.
\* From UiEvent::GrabReleased (window.rs) ...
TakeInReleaseChord ==
  /\ uiInbox > 0 /\ uiInbox' = uiInbox - 1
  /\ focused' = focused /\ armed' = FALSE /\ locked' = locked
  /\ Apply(uiMode, UiMode(focused, FALSE, locked), TRUE)
  /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, msgs, sweepPending,
                 keyEvents, silences, sessionEnds, uiEvents>>
\* ... or from Grab::tick when hook::release_overdue (no session answered).
ReleaseOverdue ==
  /\ fired
  /\ focused' = focused /\ armed' = FALSE /\ locked' = locked
  /\ Apply(uiMode, UiMode(focused, FALSE, locked), TRUE)
  /\ UNCHANGED <<phys, physView, routeTarget, chanOpen, remote, uiInbox, msgs,
                 sweepPending, keyEvents, silences, sessionEnds, uiEvents>>

\* Grab::tick: the sweep after the lock screen (release_at_grab_end).
SweepAfterUnlock ==
  /\ sweepPending /\ sweepPending' = FALSE
  /\ winDown' = AfterGrabEndSweep(physView)
  /\ UNCHANGED <<phys, physView, gate, fired, ledgerWin, ledgerRemote, watch, localHeld,
                 localTaking, ctxLast, hookInstalled, hookLive, routeTarget, chanOpen,
                 chan, remote, uiInbox, msgs, focused, armed, locked, keyEvents,
                 silences, sessionEnds, uiEvents>>

\* Grab::legacy_key: a key message while the hook looked silent. If
\* silent_hook_judge says so, send it, note_legacy, and install the hook
\* again (HookThread::start in Grab: ledger.reinstall, watch after_reinstall,
\* a new Ctx, ClearModifiers, release_at_reinstall).
UiLegacyKey ==
  /\ msgs # <<>>
  /\ msgs' = Tail(msgs)
  /\ LET k == Head(msgs)[1]
         press == Head(msgs)[2]
         superseded == Head(msgs)[3]   \* Physical::saw_later: dropped, not sent
         judge == uiMode = "Grab" /\ gate = "Grab"
         sent == CanSend
         view == [physView EXCEPT ![k] = press]
         lw == Set(ledgerWin, k, press)
     IN IF superseded \/ ~judge
        THEN UNCHANGED <<physView, ledgerWin, ledgerRemote, watch, chan, winDown,
                         localHeld, localTaking, ctxLast, hookInstalled, hookLive>>
        ELSE
          /\ chan' = SendOn(chan, KeyCmd(k, press))
          /\ physView' = view
          /\ ledgerWin' = lw \cup winDown
          /\ ledgerRemote' = IF sent \/ press = "Up" THEN Set(ledgerRemote, k, press)
                             ELSE ledgerRemote
          /\ watch' = winDown \cap Mods
          /\ ctxLast' = "Off" /\ localHeld' = {} /\ localTaking' = None
          /\ hookInstalled' = TRUE /\ hookLive' = TRUE
          /\ winDown' = AfterReinstallSweep(winDown, view)
  /\ UNCHANGED <<phys, gate, fired, routeTarget, chanOpen, remote, uiInbox, focused,
                 armed, locked, sweepPending, keyEvents, silences, sessionEnds, uiEvents>>

-----------------------------------------------------------------------------
(* The session thread: SessionMachine::on_ui with KeyPipeline.              *)

SessionTakesCmd ==
  /\ chan # <<>>
  /\ chan' = Tail(chan)
  /\ LET c == Head(chan) IN
     CASE c[1] = "Key" /\ c[3] = "Down" ->
            \* KeyPipeline::run: the release chord also completes here.
            IF c[2] = "Rel" /\ "Mod" \in remote /\ "Rel" \notin remote
            THEN /\ remote' = {} /\ uiInbox' = uiInbox + 1
            ELSE /\ remote' = remote \cup {c[2]} /\ UNCHANGED uiInbox
       [] c[1] = "Key" -> remote' = remote \ {c[2]} /\ UNCHANGED uiInbox
       [] c[1] = "ReleaseInput" -> remote' = {} /\ UNCHANGED uiInbox
       [] c[1] = "ReleaseGrab" -> remote' = {} /\ uiInbox' = uiInbox + 1
  /\ uiInbox' <= MaxInbox
  /\ UNCHANGED <<phys, winDown, physView, gate, fired, ledgerWin, ledgerRemote, watch,
                 localHeld, localTaking, ctxLast, hookInstalled, hookLive, routeTarget,
                 chanOpen, msgs, focused, armed, locked, sweepPending, keyEvents,
                 silences, sessionEnds, uiEvents>>

\* The session ends (network drop or the far end closes): its channel closes
\* and the server lets go of its keys (assumption). The route still points at
\* it until the UI thread hears.
SessionEnds ==
  /\ chanOpen /\ sessionEnds < MaxSessionEnds
  /\ chanOpen' = FALSE /\ chan' = <<>> /\ remote' = {}
  /\ sessionEnds' = sessionEnds + 1
  /\ UNCHANGED <<phys, winDown, physView, gate, fired, ledgerWin, ledgerRemote, watch,
                 localHeld, localTaking, ctxLast, hookInstalled, hookLive, routeTarget,
                 uiInbox, msgs, focused, armed, locked, sweepPending, keyEvents, silences, uiEvents>>

\* SessionSet: the UI thread moves the route off the ended session.
RouteCleared ==
  /\ routeTarget /\ ~chanOpen /\ routeTarget' = FALSE
  /\ UNCHANGED <<phys, winDown, physView, gate, fired, ledgerWin, ledgerRemote, watch,
                 localHeld, localTaking, ctxLast, hookInstalled, hookLive, chanOpen,
                 chan, remote, uiInbox, msgs, focused, armed, locked, sweepPending,
                 keyEvents, silences, sessionEnds, uiEvents>>

\* Reconnect: a new session, the route points at it.
Reconnect ==
  /\ ~routeTarget /\ ~chanOpen
  /\ routeTarget' = TRUE /\ chanOpen' = TRUE /\ chan' = <<>> /\ remote' = {}
  /\ UNCHANGED <<phys, winDown, physView, gate, fired, ledgerWin, ledgerRemote, watch,
                 localHeld, localTaking, ctxLast, hookInstalled, hookLive, uiInbox,
                 msgs, focused, armed, locked, sweepPending, keyEvents, silences,
                 sessionEnds, uiEvents>>

-----------------------------------------------------------------------------

UserPresses == \E k \in Keys :
    HookSeesKey(k, "Down") \/ SilentHookKey(k, "Down") \/ NoHookKey(k, "Down")
    \/ LockedKey(k, "Down")
UserReleases == \E k \in Keys :
    HookSeesKey(k, "Up") \/ SilentHookKey(k, "Up") \/ NoHookKey(k, "Up")
    \/ LockedKey(k, "Up")

Next ==
    \/ UserPresses \/ UserReleases
    \/ HookGoesSilent
    \/ FocusLost \/ FocusGained \/ Click \/ GrabTimesOut \/ Lock \/ Unlock
    \/ TakeInReleaseChord \/ ReleaseOverdue \/ SweepAfterUnlock \/ UiLegacyKey
    \/ SessionTakesCmd \/ SessionEnds \/ RouteCleared \/ Reconnect

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* The rules. "Settled": nothing is on its way (no command for the session, *)
(* no key message for the window, no release chord waiting to be taken in,  *)
(* no sweep pending), the screen is not locked, and the route agrees with    *)
(* the session.                                                             *)

Settled ==
    /\ chan = <<>> /\ msgs = <<>> /\ uiInbox = 0 /\ ~fired /\ ~sweepPending
    /\ ~locked /\ routeTarget = chanOpen

\* Once the grab is over and things settle, the remote holds no key.
RemoteLetGo == Settled /\ uiMode = "Off" => remote = {}
\* Once things settle, the remote holds only keys the user holds.
RemoteFollowsUser == Settled => remote \subseteq phys
\* Once things settle, Windows counts down only keys the user holds.
WindowsFollowsUser == Settled => winDown \subseteq phys
\* Once things settle, the hook swallows no key the user presses next: it
\* only keeps a taken trigger the user still holds.
TakenIsHeld == Settled /\ hookInstalled /\ ctxLast = "Grab" => localTaking \in phys \cup {None}
=============================================================================
