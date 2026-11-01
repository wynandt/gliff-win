//! The per-user `DisableLockWorkstation` setting that lets Super+L reach the
//! remote machine, and every way back from it. Pure:
//! the registry and the process table are traits, and the Windows side
//! (`lockreg`) implements them. A lock policy left on would stop the user
//! locking the machine, so every exit path ends in [`restore_if_owned`].

use std::any::Any;
use std::fmt;

/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\System`, value
/// `DisableLockWorkstation` (DWORD): 1 turns Win+L and the lock command off.
pub const POLICY_VALUE: &str = "DisableLockWorkstation";
/// Where the marker lives, under HKCU.
pub const MARKER_KEY: &str = r"Software\gliff-windows\LockPolicy";

/// How long the setting stays on after the window loses the keyboard, so a
/// grab that flickers (a pointer jittering at the window edge in hover mode,
/// a quick focus round trip) does not flap it. Closing the gate
/// ([`LockGate`]) does not wait.
pub const CLEAR_DELAY_MS: u32 = 300;
/// How long to wait before trying a failed restore again.
const RETRY_MS: u32 = 1_000;

/// The setting's state before the client touched it: absent, or the exact
/// value it had (the user's own 0, or another number, comes back as it was).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prior {
    Absent,
    Value(u32),
}

impl Prior {
    /// The marker's `Prior` number: 0 for absent, the value plus one
    /// otherwise. (The first markers held only 0, 1 and 2 for absent, 0 and
    /// 1, which this reads the same way.)
    pub fn to_stored(self) -> u64 {
        match self {
            Self::Absent => 0,
            Self::Value(value) => u64::from(value) + 1,
        }
    }

    /// The other way; a number no [`Prior::to_stored`] writes is an error.
    pub fn from_stored(stored: u64) -> Result<Self, RegError> {
        match stored {
            0 => Ok(Self::Absent),
            n => u32::try_from(n - 1)
                .map(Self::Value)
                .map_err(|_| RegError(format!("a marker with Prior {n}"))),
        }
    }
}

/// A process, by id and creation time (a pid alone can be reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    pub start: u64,
}

/// What the client wrote before it turned the setting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    pub owner: Owner,
    pub prior: Prior,
}

/// What reading the marker found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredMarker {
    Absent,
    Complete(Marker),
    /// The marker key exists but a value in it is missing: a write that
    /// stopped midway. The marker is written in full before the setting is
    /// touched, so the setting was never changed for it.
    Incomplete,
}

/// A registry call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("registry: {0}")]
pub struct RegError(pub String);

/// The three registry operations on the setting and on the marker.
pub trait Registry {
    /// Keeps every other client, the watchdog helper and this process's own
    /// exit path out of the registry until the returned value is dropped. A
    /// restore or a turn-on is a read followed by writes: without this, the
    /// helper finishing for a dead client can clear what a new client has
    /// just set, or delete the new client's marker. Re-entrant on one
    /// thread. The default holds nothing.
    fn hold(&self) -> Box<dyn Any> {
        Box::new(())
    }
    /// Turns the setting on. Refused (an error) once this process is on its
    /// way out, so an exit path that has restored the setting is not undone.
    fn set_on(&mut self) -> Result<(), RegError>;
    fn read_value(&mut self) -> Result<Option<u32>, RegError>;
    fn write_value(&mut self, value: u32) -> Result<(), RegError>;
    fn delete_value(&mut self) -> Result<(), RegError>;
    fn read_marker(&mut self) -> Result<StoredMarker, RegError>;
    fn write_marker(&mut self, marker: Marker) -> Result<(), RegError>;
    fn delete_marker(&mut self) -> Result<(), RegError>;
}

/// The process table, for the startup check.
pub trait Processes {
    fn is_running(&self, owner: Owner) -> bool;
}

/// Blocks until the owner process has ended (the watchdog's process handle).
pub trait OwnerExit {
    fn wait(&mut self);
}

/// Whether the setting should be on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    On,
    Off,
}

/// What a call did, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Nothing,
    /// The setting is now 1, with the marker written first.
    Set,
    /// The setting was put back to its prior state and the marker removed.
    Cleared,
    /// The setting was already 1 and not ours; it is never cleared.
    NotOurs,
    /// A registry call failed; the chord is the route now.
    Failed(RegError),
}

/// What the startup check or `--restore-lock-policy` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// No marker: nothing to do.
    Nothing,
    /// A marker from a dead owner: the setting was restored per the marker.
    Recovered(Marker),
    /// The owner is running (another gliff): do not use the registry route.
    OwnerAlive(Owner),
    /// An incomplete marker (a write that stopped midway) was removed; the
    /// setting had not been changed for it.
    Discarded,
    Failed(RegError),
}

impl Recovery {
    /// The process exit code of `--restore-lock-policy`: nonzero only when
    /// the setting could not be read or put back, so a script (or the
    /// verification harness) can tell. A running owner or nothing to do is
    /// not a failure.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Failed(_) => 1,
            Self::Nothing | Self::Recovered(_) | Self::OwnerAlive(_) | Self::Discarded => 0,
        }
    }
}

impl fmt::Display for Recovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nothing => f.write_str("lock policy: nothing to restore"),
            Self::Recovered(m) => write!(
                f,
                "lock policy recovered: {} {:?} (owner pid {} had left it on)",
                POLICY_VALUE, m.prior, m.owner.pid
            ),
            Self::OwnerAlive(o) => {
                write!(f, "lock policy: owned by running pid {}, left alone", o.pid)
            }
            Self::Discarded => f.write_str("lock policy: removed an incomplete marker"),
            Self::Failed(e) => write!(f, "lock policy: restore failed: {e}"),
        }
    }
}

/// Puts the setting back per the marker and removes the marker, if there is
/// a marker and (when `only` is given) it names that owner. Idempotent. The
/// value is restored before the marker is removed, so a failure in between
/// leaves the marker for the next attempt. An incomplete marker names no
/// owner, so it is left for [`recover`] (a client that is still writing it
/// must not have it deleted from under it).
pub fn restore_if_owned(
    registry: &mut dyn Registry,
    only: Option<Owner>,
) -> Result<Option<Marker>, RegError> {
    let _hold = registry.hold();
    let StoredMarker::Complete(marker) = registry.read_marker()? else {
        return Ok(None);
    };
    if only.is_some_and(|owner| owner != marker.owner) {
        return Ok(None);
    }
    match marker.prior {
        Prior::Absent => registry.delete_value()?,
        Prior::Value(value) => registry.write_value(value)?,
    }
    registry.delete_marker()?;
    Ok(Some(marker))
}

/// A marker whose owner is gone is restored; one whose owner runs is left. An
/// incomplete marker is removed: left alone it would make every later start
/// fail to read it.
pub fn recover(registry: &mut dyn Registry, processes: &dyn Processes) -> Recovery {
    // Held across the look and the restore: a client that is writing its
    // marker holds the registry too, so a marker seen half written is a
    // dead one's.
    let _hold = registry.hold();
    let marker = match registry.read_marker() {
        Ok(StoredMarker::Complete(marker)) => marker,
        Ok(StoredMarker::Absent) => return Recovery::Nothing,
        Ok(StoredMarker::Incomplete) => {
            return match registry.delete_marker() {
                Ok(()) => Recovery::Discarded,
                Err(e) => Recovery::Failed(e),
            };
        }
        Err(e) => return Recovery::Failed(e),
    };
    if processes.is_running(marker.owner) {
        return Recovery::OwnerAlive(marker.owner);
    }
    match restore_if_owned(registry, Some(marker.owner)) {
        Ok(Some(marker)) => Recovery::Recovered(marker),
        Ok(None) => Recovery::Nothing,
        Err(e) => Recovery::Failed(e),
    }
}

/// The `--lock-watchdog` helper: waits for the owner to end, then restores
/// if the marker still names it. A newer client's marker is left alone.
pub fn run_watchdog(
    exit: &mut dyn OwnerExit,
    registry: &mut dyn Registry,
    owner: Owner,
) -> Result<Option<Marker>, RegError> {
    exit.wait();
    restore_if_owned(registry, Some(owner))
}

/// Where the setting is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// We set it to 1 (marker written).
    Held,
    /// It was 1 before us; locking is off without us.
    AlreadyOff,
    /// The registry route is not usable: a call failed, or another gliff
    /// owns the setting.
    Disabled,
}

/// The repair at start. A marker a dead client left is put right whatever
/// the Super+L mode now is: the mode may have been changed after the crash,
/// and the setting stays on until someone repairs it. In `registry` mode
/// (`route`, with a readable `owner`) this also returns the policy that
/// turns the setting on and off, already checked; in any other mode, or
/// without an owner, there is no policy and the repair is all that runs.
pub fn start_up<R: Registry>(
    mut registry: R,
    route: bool,
    owner: Option<Owner>,
    processes: &dyn Processes,
) -> (Option<LockPolicy<R>>, Recovery) {
    match owner.filter(|_| route) {
        Some(owner) => {
            let mut lock = LockPolicy::new(registry, owner);
            let recovery = lock.startup_check(processes);
            (Some(lock), recovery)
        }
        None => {
            let recovery = recover(&mut registry, processes);
            (None, recovery)
        }
    }
}

/// The state machine behind the wish (the window has the keyboard), which
/// [`LockGate`] passes on while the gate is open.
#[derive(Debug)]
pub struct LockPolicy<R: Registry> {
    registry: R,
    owner: Owner,
    state: State,
    /// When a pending Off takes effect.
    clear_at: Option<u32>,
    /// A restore failed midway, so the setting may be back to its prior
    /// while the state is still `Held`: the next wish for On writes the
    /// setting again instead of trusting it.
    restore_failed: bool,
    /// A turn-on failed and so did the removal of its marker: the half
    /// marker is ours to remove at the exit.
    stray_marker: bool,
}

impl<R: Registry> LockPolicy<R> {
    pub fn new(registry: R, owner: Owner) -> Self {
        Self {
            registry,
            owner,
            state: State::Idle,
            clear_at: None,
            restore_failed: false,
            stray_marker: false,
        }
    }

    pub fn registry(&self) -> &R {
        &self.registry
    }

    /// True while Windows will not lock on Win+L, so Super+L can be
    /// forwarded: the setting is on, or was already on without us.
    pub fn is_effective(&self) -> bool {
        matches!(self.state, State::Held | State::AlreadyOff)
    }

    /// True once the registry route cannot be used (the chord is the route).
    pub fn is_disabled(&self) -> bool {
        self.state == State::Disabled
    }

    /// Run once at start: restore a dead owner's leftovers. A live owner
    /// disables the registry route.
    pub fn startup_check(&mut self, processes: &dyn Processes) -> Recovery {
        let recovery = recover(&mut self.registry, processes);
        match recovery {
            Recovery::OwnerAlive(_) | Recovery::Failed(_) => self.state = State::Disabled,
            Recovery::Nothing | Recovery::Recovered(_) | Recovery::Discarded => {}
        }
        recovery
    }

    /// The setting should be on (`Want::On`) or off. Off takes effect after
    /// [`CLEAR_DELAY_MS`], on a later [`Self::tick`].
    pub fn want(&mut self, want: Want, now_ms: u32) -> Change {
        match (want, self.state) {
            (_, State::Disabled) => Change::Nothing,
            (Want::On, State::Held) if self.restore_failed => {
                // The marker is still ours and holds the right prior.
                let _hold = self.registry.hold();
                match self.registry.set_on() {
                    Ok(()) => {
                        self.restore_failed = false;
                        self.clear_at = None;
                        Change::Set
                    }
                    Err(e) => self.fail(e),
                }
            }
            (Want::On, State::Held | State::AlreadyOff) => {
                self.clear_at = None;
                Change::Nothing
            }
            (Want::On, State::Idle) => self.turn_on(),
            (Want::Off, State::Held) => {
                self.clear_at
                    .get_or_insert(now_ms.wrapping_add(CLEAR_DELAY_MS));
                Change::Nothing
            }
            (Want::Off, State::AlreadyOff) => {
                self.state = State::Idle;
                Change::Nothing
            }
            (Want::Off, State::Idle) => Change::Nothing,
        }
    }

    /// Call on a timer while a clear is pending ([`Self::clear_pending`]).
    pub fn tick(&mut self, now_ms: u32) -> Change {
        match self.clear_at {
            // A wrapping "now is at or past the deadline".
            Some(at) if now_ms.wrapping_sub(at) < u32::MAX / 2 => self.restore(now_ms),
            _ => Change::Nothing,
        }
    }

    pub fn clear_pending(&self) -> bool {
        self.clear_at.is_some()
    }

    /// The setting should be off now, without the delay of [`Self::want`]:
    /// the gate closed, so no flicker is to be smoothed over. A failed
    /// restore is tried again on a later [`Self::tick`].
    pub fn clear_now(&mut self, now_ms: u32) -> Change {
        match self.state {
            State::Held => self.restore(now_ms),
            State::AlreadyOff | State::Idle | State::Disabled => self.want(Want::Off, now_ms),
        }
    }

    /// Every exit path: put the setting back now. Idempotent.
    pub fn exit(&mut self) -> Change {
        self.restore(0)
    }

    fn turn_on(&mut self) -> Change {
        // From the look to the last write: nothing else restores or turns on
        // in between.
        let _hold = self.registry.hold();
        let prior = match self.registry.read_value() {
            Ok(Some(1)) => return self.found_on(),
            Ok(Some(value)) => Prior::Value(value),
            Ok(None) => Prior::Absent,
            Err(e) => return self.fail(e),
        };
        let marker = Marker {
            owner: self.owner,
            prior,
        };
        // Write-ahead: the marker exists before the value changes.
        if let Err(e) = self.registry.write_marker(marker) {
            // It may have stopped midway; take what is there back.
            return self.fail_and_take_marker_back(e);
        }
        if let Err(e) = self.registry.set_on() {
            // The value did not change; take the marker back.
            return self.fail_and_take_marker_back(e);
        }
        self.state = State::Held;
        self.clear_at = None;
        self.restore_failed = false;
        Change::Set
    }

    /// The setting is 1 already. With no marker it is the user's (or a group
    /// policy's): locking is off without us. With a marker it is another
    /// client's, which puts it back when it lets go, so this one must not
    /// count on it (the chord is the route). A marker of our own is taken
    /// back up.
    fn found_on(&mut self) -> Change {
        match self.registry.read_marker() {
            Ok(StoredMarker::Absent) => {
                self.state = State::AlreadyOff;
                Change::NotOurs
            }
            Ok(StoredMarker::Complete(marker)) if marker.owner == self.owner => {
                self.state = State::Held;
                self.clear_at = None;
                Change::Set
            }
            Ok(StoredMarker::Complete(marker)) => self.fail(RegError(format!(
                "the setting is held by another client (pid {})",
                marker.owner.pid
            ))),
            Ok(StoredMarker::Incomplete) => self.fail(RegError(
                "the setting is on and its marker is incomplete".to_owned(),
            )),
            Err(e) => self.fail(e),
        }
    }

    fn restore(&mut self, now_ms: u32) -> Change {
        self.sweep_stray_marker();
        match restore_if_owned(&mut self.registry, Some(self.owner)) {
            Ok(Some(_)) => {
                self.state = State::Idle;
                self.clear_at = None;
                self.restore_failed = false;
                Change::Cleared
            }
            Ok(None) => {
                if self.state == State::Held {
                    self.state = State::Idle;
                }
                self.clear_at = None;
                self.restore_failed = false;
                Change::Nothing
            }
            Err(e) => {
                // The marker stays, so the watchdog and the next start can
                // still finish it; try again soon.
                self.clear_at = Some(now_ms.wrapping_add(RETRY_MS));
                self.restore_failed = true;
                Change::Failed(e)
            }
        }
    }

    /// A turn-on failed after the marker was started: remove the marker. If
    /// that fails too, the failure says both (the log line is the only place
    /// it is seen), and the exit path and the next start try again.
    fn fail_and_take_marker_back(&mut self, error: RegError) -> Change {
        match self.registry.delete_marker() {
            Ok(()) => self.fail(error),
            Err(delete) => {
                self.stray_marker = true;
                self.fail(RegError(format!(
                    "{error}; removing the marker failed too ({delete}), \
                     so it is left for the exit and the next start"
                )))
            }
        }
    }

    /// Removes a half-written marker this client left behind, once the
    /// registry lets it. Only after our own failed removal: another client
    /// may be writing its own marker.
    fn sweep_stray_marker(&mut self) {
        if !self.stray_marker {
            return;
        }
        let _hold = self.registry.hold();
        match self.registry.read_marker() {
            Ok(StoredMarker::Incomplete) => {
                if self.registry.delete_marker().is_ok() {
                    self.stray_marker = false;
                }
            }
            Ok(StoredMarker::Absent) => self.stray_marker = false,
            // Ours, or someone else's by now: the normal restore deals with it.
            Ok(StoredMarker::Complete(_)) => self.stray_marker = false,
            Err(_) => {}
        }
    }

    fn fail(&mut self, error: RegError) -> Change {
        self.state = State::Disabled;
        Change::Failed(error)
    }
}

/// The lock setting as the window drives it: on only while the shown tab's
/// machine has *Send Win+L to this machine* on (the gate is open) and the
/// window has the keyboard (the wish). A wish that goes off is cleared after
/// [`CLEAR_DELAY_MS`]; a gate that closes (the switch goes off, the shown tab
/// changes to a machine without it, no tab is shown, the computer goes to
/// sleep) clears it at once.
#[derive(Debug)]
pub struct LockGate<R: Registry> {
    policy: LockPolicy<R>,
    open: bool,
    wish: Want,
}

impl<R: Registry> LockGate<R> {
    /// Closed, with no wish: nothing is written until both turn on.
    pub fn new(policy: LockPolicy<R>) -> Self {
        Self {
            policy,
            open: false,
            wish: Want::Off,
        }
    }

    pub fn policy(&self) -> &LockPolicy<R> {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut LockPolicy<R> {
        &mut self.policy
    }

    /// Whether moving to `open` and `wish` would write the setting on: the
    /// caller starts the watchdog helper first.
    pub fn would_turn_on(&self, open: bool, wish: Want) -> bool {
        open && wish == Want::On && !self.policy.is_effective() && !self.policy.is_disabled()
    }

    /// Moves to `open` and `wish`. `ready` is false when a turn-on must not
    /// happen (the watchdog helper did not start): the setting is then left
    /// as it is, and the next call tries again.
    pub fn set(&mut self, open: bool, wish: Want, ready: bool, now_ms: u32) -> Change {
        let closing = self.open && !open;
        self.open = open;
        self.wish = wish;
        if closing {
            return self.policy.clear_now(now_ms);
        }
        let want = if open { wish } else { Want::Off };
        if want == Want::On && !ready && !self.policy.is_effective() {
            return Change::Nothing;
        }
        self.policy.want(want, now_ms)
    }

    pub fn wish(&self) -> Want {
        self.wish
    }
}

#[cfg(test)]
mod tests;
