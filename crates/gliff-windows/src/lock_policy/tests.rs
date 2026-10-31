use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use super::CLEAR_DELAY_MS;
use super::Change;
use super::LockPolicy;
use super::Marker;
use super::Owner;
use super::OwnerExit;
use super::Prior;
use super::Processes;
use super::Recovery;
use super::RegError;
use super::Registry;
use super::StoredMarker;
use super::Want;
use super::recover;
use super::restore_if_owned;
use super::run_watchdog;
use super::start_up;

const ME: Owner = Owner {
    pid: 10,
    start: 111,
};
const OTHER: Owner = Owner {
    pid: 20,
    start: 222,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    ReadValue,
    WriteValue(u32),
    DeleteValue,
    ReadMarker,
    WriteMarker(Marker),
    DeleteMarker,
}

/// An in-memory registry that records every call. Nothing here touches a
/// real registry.
#[derive(Debug, Default)]
struct Fake {
    value: Option<u32>,
    marker: Option<Marker>,
    ops: Vec<Op>,
    fail_write_value: bool,
    fail_write_marker: bool,
    /// The marker write stops after its first value, as a crash or a
    /// failing registry call can leave it.
    fail_write_marker_midway: bool,
    /// What a marker that stopped midway leaves behind.
    partial_marker: bool,
    fail_delete_marker: bool,
    fail_reads: bool,
    /// How many holds ([`Registry::hold`]) are open now.
    holds: Rc<Cell<u32>>,
    /// Writes made while nothing was held.
    unheld: Vec<Op>,
    /// What an exiting process answers to a turn-on.
    refuse_set_on: bool,
}

/// An open hold on the fake registry.
struct FakeHold(Rc<Cell<u32>>);

impl Drop for FakeHold {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl Fake {
    fn record(&mut self, op: Op) {
        let is_write = !matches!(op, Op::ReadValue | Op::ReadMarker);
        if is_write && self.holds.get() == 0 {
            self.unheld.push(op);
        }
        self.ops.push(op);
    }

    fn with_value(value: Option<u32>) -> Self {
        Self {
            value,
            ..Self::default()
        }
    }

    fn writes(&self) -> Vec<Op> {
        self.ops
            .iter()
            .copied()
            .filter(|op| !matches!(op, Op::ReadValue | Op::ReadMarker))
            .collect()
    }
}

fn err(what: &str) -> RegError {
    RegError(what.to_owned())
}

impl Registry for Fake {
    fn hold(&self) -> Box<dyn std::any::Any> {
        self.holds.set(self.holds.get() + 1);
        Box::new(FakeHold(Rc::clone(&self.holds)))
    }

    fn set_on(&mut self) -> Result<(), RegError> {
        if self.refuse_set_on {
            return Err(err("the client is exiting"));
        }
        self.write_value(1)
    }

    fn read_value(&mut self) -> Result<Option<u32>, RegError> {
        self.record(Op::ReadValue);
        if self.fail_reads {
            return Err(err("read denied"));
        }
        Ok(self.value)
    }

    fn write_value(&mut self, value: u32) -> Result<(), RegError> {
        self.record(Op::WriteValue(value));
        if self.fail_write_value {
            return Err(err("write denied"));
        }
        self.value = Some(value);
        Ok(())
    }

    fn delete_value(&mut self) -> Result<(), RegError> {
        self.record(Op::DeleteValue);
        self.value = None;
        Ok(())
    }

    fn read_marker(&mut self) -> Result<StoredMarker, RegError> {
        self.record(Op::ReadMarker);
        if self.fail_reads {
            return Err(err("read denied"));
        }
        Ok(match (self.marker, self.partial_marker) {
            (Some(marker), _) => StoredMarker::Complete(marker),
            (None, true) => StoredMarker::Incomplete,
            (None, false) => StoredMarker::Absent,
        })
    }

    fn write_marker(&mut self, marker: Marker) -> Result<(), RegError> {
        self.record(Op::WriteMarker(marker));
        if self.fail_write_marker {
            return Err(err("marker denied"));
        }
        if self.fail_write_marker_midway {
            self.partial_marker = true;
            return Err(err("marker denied after its first value"));
        }
        self.marker = Some(marker);
        Ok(())
    }

    fn delete_marker(&mut self) -> Result<(), RegError> {
        self.record(Op::DeleteMarker);
        if self.fail_delete_marker {
            return Err(err("marker delete denied"));
        }
        self.marker = None;
        self.partial_marker = false;
        Ok(())
    }
}

struct Running(BTreeSet<u32>);

impl Processes for Running {
    fn is_running(&self, owner: Owner) -> bool {
        self.0.contains(&owner.pid)
    }
}

fn policy(value: Option<u32>) -> LockPolicy<Fake> {
    LockPolicy::new(Fake::with_value(value), ME)
}

fn marker(prior: Prior) -> Marker {
    Marker { owner: ME, prior }
}

#[test]
fn the_marker_is_written_before_the_value() {
    let mut p = policy(None);
    assert_eq!(p.want(Want::On, 0), Change::Set);
    assert_eq!(
        p.registry().writes(),
        vec![Op::WriteMarker(marker(Prior::Absent)), Op::WriteValue(1)],
        "write-ahead order"
    );
    assert!(p.is_effective(), "Win+L no longer locks");
}

#[test]
fn an_absent_value_is_deleted_again() {
    let mut p = policy(None);
    p.want(Want::On, 0);
    assert_eq!(p.exit(), Change::Cleared);
    let r = p.registry();
    assert_eq!((r.value, r.marker), (None, None), "as it was");
    assert_eq!(
        r.writes()[2..],
        [Op::DeleteValue, Op::DeleteMarker],
        "value first, then the marker"
    );
}

#[test]
fn a_zero_value_is_written_back_as_zero() {
    let mut p = policy(Some(0));
    p.want(Want::On, 0);
    assert_eq!(p.registry().marker, Some(marker(Prior::Value(0))));
    assert_eq!(p.registry().value, Some(1));
    p.exit();
    assert_eq!((p.registry().value, p.registry().marker), (Some(0), None));
}

#[test]
fn a_value_of_one_without_a_marker_is_never_touched() {
    let mut p = policy(Some(1));
    assert_eq!(p.want(Want::On, 0), Change::NotOurs);
    assert!(
        p.is_effective(),
        "locking is off already, so Super+L can go through"
    );
    assert_eq!(p.want(Want::Off, 10), Change::Nothing);
    p.tick(10_000);
    assert_eq!(p.exit(), Change::Nothing);
    assert_eq!(p.registry().writes(), vec![], "no write of any kind");
    assert_eq!(p.registry().value, Some(1), "still the user's 1");
}

#[test]
fn the_setting_stays_on_for_300_ms_after_want_goes_off() {
    let mut p = policy(None);
    p.want(Want::On, 1_000);
    assert_eq!(p.want(Want::Off, 2_000), Change::Nothing);
    assert!(p.clear_pending());
    assert_eq!(
        p.tick(2_000 + CLEAR_DELAY_MS - 1),
        Change::Nothing,
        "299 ms"
    );
    assert!(p.is_effective());
    assert_eq!(p.tick(2_000 + CLEAR_DELAY_MS), Change::Cleared, "300 ms");
    assert!(!p.is_effective());
    assert!(!p.clear_pending());
}

#[test]
fn want_coming_back_cancels_the_clear() {
    let mut p = policy(None);
    p.want(Want::On, 0);
    p.want(Want::Off, 100);
    assert_eq!(p.want(Want::On, 200), Change::Nothing, "no second write");
    assert_eq!(p.tick(10_000), Change::Nothing);
    assert!(p.is_effective());
}

#[test]
fn a_jittering_pointer_writes_the_registry_once() {
    let mut p = policy(None);
    for i in 0..20_u32 {
        p.want(Want::On, i * 100);
        p.want(Want::Off, i * 100 + 50);
        p.tick(i * 100 + 99);
    }
    let sets = p
        .registry()
        .writes()
        .iter()
        .filter(|op| **op == Op::WriteValue(1))
        .count();
    assert_eq!(sets, 1, "the setting flapped");
}

#[test]
fn a_second_off_does_not_push_the_deadline_out() {
    let mut p = policy(None);
    p.want(Want::On, 0);
    p.want(Want::Off, 1_000);
    p.want(Want::Off, 1_200);
    assert_eq!(p.tick(1_300), Change::Cleared, "300 ms from the first Off");
}

#[test]
fn the_deadline_works_across_the_u32_wrap() {
    let mut p = policy(None);
    p.want(Want::On, u32::MAX - 100);
    p.want(Want::Off, u32::MAX - 100);
    assert_eq!(p.tick(u32::MAX - 50), Change::Nothing);
    assert_eq!(
        p.tick(CLEAR_DELAY_MS),
        Change::Cleared,
        "now 300 is past the wrapped deadline"
    );
}

#[test]
fn exit_is_immediate_and_idempotent() {
    let mut p = policy(Some(0));
    p.want(Want::On, 0);
    assert_eq!(p.exit(), Change::Cleared);
    let n = p.registry().ops.len();
    assert_eq!(p.exit(), Change::Nothing, "the second exit does nothing");
    assert_eq!(
        p.registry().writes().len(),
        4,
        "marker, value, value, marker"
    );
    assert!(p.registry().ops.len() > n, "only a marker read");

    // The marker went while the setting was held (the watchdog finished):
    // there is nothing to restore, and the policy stops counting on it.
    let mut p = policy(Some(0));
    p.want(Want::On, 0);
    p.registry.marker = None;
    assert_eq!(p.exit(), Change::Nothing);
    assert!(!p.is_effective());
}

#[test]
fn a_failed_value_write_takes_the_marker_back_and_disables_the_route() {
    let fake = Fake {
        fail_write_value: true,
        ..Fake::default()
    };
    let mut p = LockPolicy::new(fake, ME);
    assert_eq!(p.want(Want::On, 0), Change::Failed(err("write denied")));
    assert_eq!(
        p.registry().marker,
        None,
        "no marker for a value that never changed"
    );
    assert!(p.is_disabled(), "the chord is the route now");
    assert!(!p.is_effective(), "Win+L would still lock");
    assert_eq!(p.want(Want::On, 100), Change::Nothing, "no retry storm");
}

#[test]
fn a_failed_marker_write_never_changes_the_value() {
    let fake = Fake {
        fail_write_marker: true,
        ..Fake::default()
    };
    let mut p = LockPolicy::new(fake, ME);
    assert!(matches!(p.want(Want::On, 0), Change::Failed(_)));
    assert!(
        !p.registry().ops.contains(&Op::WriteValue(1)),
        "no value without its marker"
    );
}

#[test]
fn a_failed_read_disables_the_route() {
    let fake = Fake {
        fail_reads: true,
        ..Fake::default()
    };
    let mut p = LockPolicy::new(fake, ME);
    assert!(matches!(p.want(Want::On, 0), Change::Failed(_)));
    assert!(p.is_disabled());
}

#[test]
fn a_failed_restore_keeps_the_marker_and_retries() {
    let mut p = policy(None);
    p.want(Want::On, 0);
    // The marker removal fails after the value is back.
    p.registry.fail_delete_marker = true;
    let c = p.exit();
    assert!(matches!(c, Change::Failed(_)), "{c:?}");
    assert_eq!(p.registry().value, None, "the value was restored first");
    assert!(
        p.registry().marker.is_some(),
        "the marker stays for the next attempt"
    );
    assert!(p.clear_pending(), "and a retry is scheduled");
    p.registry.fail_delete_marker = false;
    assert_eq!(p.tick(1_000), Change::Cleared, "the retry finishes it");
    assert_eq!(p.registry().marker, None);
}

#[test]
fn a_stale_marker_is_restored_at_startup() {
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(Marker {
        owner: OTHER,
        prior: Prior::Absent,
    });
    let mut p = LockPolicy::new(fake, ME);
    let rec = p.startup_check(&Running(BTreeSet::new()));
    assert_eq!(
        rec,
        Recovery::Recovered(Marker {
            owner: OTHER,
            prior: Prior::Absent
        })
    );
    assert_eq!((p.registry().value, p.registry().marker), (None, None));
    assert!(!p.is_disabled());
    assert!(
        rec.to_string().starts_with("lock policy recovered"),
        "the log line the gate looks for: {rec}"
    );
}

#[test]
fn a_dead_clients_marker_is_repaired_at_start_in_every_mode() {
    let stale = || {
        let mut fake = Fake::with_value(Some(1));
        fake.marker = Some(Marker {
            owner: OTHER,
            prior: Prior::Absent,
        });
        fake
    };
    let none = Running(BTreeSet::new());
    // The mode was switched to chord after the crash: no policy, yet repaired.
    let (lock, rec) = start_up(stale(), false, Some(ME), &none);
    assert!(
        lock.is_none() && matches!(rec, Recovery::Recovered(_)),
        "{rec:?}"
    );
    // The same with no readable owner while the mode is registry.
    let (lock, rec) = start_up(stale(), true, None, &none);
    assert!(
        lock.is_none() && matches!(rec, Recovery::Recovered(_)),
        "{rec:?}"
    );
    // Registry mode keeps the policy, and it sees the repair done.
    let (lock, rec) = start_up(stale(), true, Some(ME), &none);
    let lock = lock.expect("the policy");
    assert!(matches!(rec, Recovery::Recovered(_)), "{rec:?}");
    assert_eq!(
        (lock.registry().value, lock.registry().marker),
        (None, None)
    );
    // A live owner's marker is left alone in a mode that does not use it.
    let running = Running(BTreeSet::from([OTHER.pid]));
    let (_, rec) = start_up(stale(), false, Some(ME), &running);
    assert_eq!(rec, Recovery::OwnerAlive(OTHER));
}

#[test]
fn a_marker_with_a_live_owner_disables_the_registry_route() {
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(Marker {
        owner: OTHER,
        prior: Prior::Absent,
    });
    let mut p = LockPolicy::new(fake, ME);
    let rec = p.startup_check(&Running(BTreeSet::from([OTHER.pid])));
    assert_eq!(rec, Recovery::OwnerAlive(OTHER));
    assert!(p.is_disabled());
    assert_eq!(p.want(Want::On, 0), Change::Nothing);
    assert_eq!(
        p.registry().writes(),
        vec![],
        "the other gliff's setting is untouched"
    );
    p.exit();
    assert_eq!(p.registry().value, Some(1), "and exit does not clear it");
    assert!(p.registry().marker.is_some());
}

#[test]
fn startup_with_no_marker_does_nothing_even_with_a_value_of_one() {
    let mut p = policy(Some(1));
    assert_eq!(
        p.startup_check(&Running(BTreeSet::new())),
        Recovery::Nothing
    );
    assert_eq!(p.registry().writes(), vec![]);
}

#[test]
fn recover_is_what_restore_lock_policy_runs() {
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(marker(Prior::Value(0)));
    let rec = recover(&mut fake, &Running(BTreeSet::new()));
    assert!(matches!(rec, Recovery::Recovered(_)), "{rec:?}");
    assert_eq!((fake.value, fake.marker), (Some(0), None));
    assert_eq!(
        recover(&mut fake, &Running(BTreeSet::new())),
        Recovery::Nothing,
        "idempotent"
    );
}

#[test]
fn restore_lock_policy_exits_nonzero_only_when_it_failed() {
    let none = Running(BTreeSet::new());
    let mut failing = Fake {
        fail_reads: true,
        ..Fake::default()
    };
    assert_eq!(recover(&mut failing, &none).exit_code(), 1, "read failed");
    let mut nothing = Fake::with_value(Some(0));
    assert_eq!(recover(&mut nothing, &none).exit_code(), 0, "nothing to do");
    let mut stale = Fake::with_value(Some(1));
    stale.marker = Some(marker(Prior::Value(0)));
    assert_eq!(recover(&mut stale, &none).exit_code(), 0, "restored");
    let mut alive = Fake::with_value(Some(1));
    alive.marker = Some(marker(Prior::Value(0)));
    let running = Running(BTreeSet::from([alive
        .marker
        .as_ref()
        .expect("m")
        .owner
        .pid]));
    assert_eq!(recover(&mut alive, &running).exit_code(), 0, "left alone");
}

#[test]
fn recover_reports_a_read_failure() {
    let mut fake = Fake {
        fail_reads: true,
        ..Fake::default()
    };
    assert!(matches!(
        recover(&mut fake, &Running(BTreeSet::new())),
        Recovery::Failed(_)
    ));
}

struct FakeExit {
    waited: bool,
}

impl OwnerExit for FakeExit {
    fn wait(&mut self) {
        self.waited = true;
    }
}

#[test]
fn the_watchdog_restores_after_its_owner_ends() {
    // The client was killed with the setting on: no exit() ran.
    let mut p = policy(None);
    p.want(Want::On, 0);
    let mut reg = std::mem::take(&mut p.registry);
    assert_eq!(reg.value, Some(1), "left on by the killed client");
    let mut exit = FakeExit { waited: false };
    let restored = run_watchdog(&mut exit, &mut reg, ME).expect("restore");
    assert!(exit.waited, "it waits for the owner first");
    assert_eq!(restored, Some(marker(Prior::Absent)));
    assert_eq!((reg.value, reg.marker), (None, None));
}

#[test]
fn the_watchdog_leaves_a_newer_clients_marker_alone() {
    let mut reg = Fake::with_value(Some(1));
    reg.marker = Some(Marker {
        owner: OTHER,
        prior: Prior::Absent,
    });
    let mut exit = FakeExit { waited: false };
    assert_eq!(run_watchdog(&mut exit, &mut reg, ME).expect("ok"), None);
    assert_eq!(reg.value, Some(1), "the other gliff's setting stays on");
    assert!(reg.marker.is_some());
}

#[test]
fn the_watchdog_after_a_normal_exit_does_nothing() {
    let mut reg = Fake::default();
    let mut exit = FakeExit { waited: false };
    assert_eq!(run_watchdog(&mut exit, &mut reg, ME).expect("ok"), None);
    assert_eq!(reg.writes(), vec![]);
}

#[test]
fn a_pid_reused_with_another_start_time_is_not_the_owner() {
    let mut reg = Fake::with_value(Some(1));
    reg.marker = Some(Marker {
        owner: Owner {
            pid: ME.pid,
            start: ME.start + 1,
        },
        prior: Prior::Absent,
    });
    assert_eq!(restore_if_owned(&mut reg, Some(ME)).expect("ok"), None);
    assert_eq!(reg.value, Some(1));
}

#[test]
fn a_marker_write_that_stops_midway_is_taken_back() {
    let fake = Fake {
        fail_write_marker_midway: true,
        ..Fake::default()
    };
    let mut p = LockPolicy::new(fake, ME);
    assert!(matches!(p.want(Want::On, 0), Change::Failed(_)));
    assert!(!p.registry().partial_marker, "no half marker is left");
    assert_eq!(p.registry().value, None, "and the value never changed");
}

#[test]
fn a_half_marker_that_cannot_be_removed_is_said_so_and_removed_at_the_exit() {
    let fake = Fake {
        fail_write_marker_midway: true,
        fail_delete_marker: true,
        ..Fake::default()
    };
    let mut p = LockPolicy::new(fake, ME);
    let Change::Failed(RegError(said)) = p.want(Want::On, 0) else {
        panic!("a failed turn-on is a Failed");
    };
    assert!(
        said.contains("marker denied after its first value"),
        "{said}"
    );
    assert!(
        said.contains("marker delete denied"),
        "the failed removal is in the same line: {said}"
    );
    assert!(
        p.registry().partial_marker,
        "the half marker is still there"
    );
    assert_eq!(p.registry().value, None, "and the value never changed");

    // The registry lets the delete through now: the exit path removes it.
    p.registry.fail_delete_marker = false;
    p.exit();
    assert!(
        !p.registry().partial_marker,
        "the exit removed the leftover"
    );
}

#[test]
fn a_half_marker_left_behind_is_removed_by_the_next_start_too() {
    let mut fake = Fake {
        fail_write_marker_midway: true,
        fail_delete_marker: true,
        ..Fake::default()
    };
    assert!(fake.write_marker(marker(Prior::Absent)).is_err());
    assert!(fake.delete_marker().is_err());
    fake.fail_delete_marker = false;
    assert_eq!(
        recover(&mut fake, &Running(BTreeSet::new())),
        Recovery::Discarded
    );
    assert!(!fake.partial_marker);
}

#[test]
fn an_exit_does_not_touch_a_half_marker_it_did_not_leave() {
    let mut fake = Fake::with_value(Some(1));
    fake.partial_marker = true;
    let mut p = LockPolicy::new(fake, ME);
    p.exit();
    assert!(
        p.registry().partial_marker,
        "another client may still be writing it"
    );
}

#[test]
fn an_incomplete_marker_is_removed_at_startup_instead_of_failing_forever() {
    let mut fake = Fake::with_value(None);
    fake.partial_marker = true;
    let mut p = LockPolicy::new(fake, ME);
    let rec = p.startup_check(&Running(BTreeSet::new()));
    assert_eq!(rec, Recovery::Discarded);
    assert!(!p.registry().partial_marker);
    assert!(!p.is_disabled(), "the registry route is usable again");
    assert_eq!(
        p.startup_check(&Running(BTreeSet::new())),
        Recovery::Nothing,
        "the next start has nothing to read"
    );
    assert_eq!(p.want(Want::On, 0), Change::Set);
}

#[test]
fn an_incomplete_marker_does_not_leave_a_half_marker_when_its_removal_fails() {
    let mut fake = Fake {
        partial_marker: true,
        fail_delete_marker: true,
        ..Fake::default()
    };
    assert!(matches!(
        recover(&mut fake, &Running(BTreeSet::new())),
        Recovery::Failed(_)
    ));
    assert!(fake.partial_marker, "still there, to try again next start");
}

#[test]
fn restore_after_exit_leaves_an_incomplete_marker_to_the_startup_check() {
    let mut fake = Fake::with_value(Some(1));
    fake.partial_marker = true;
    assert_eq!(restore_if_owned(&mut fake, Some(ME)).expect("ok"), None);
    assert!(
        fake.partial_marker,
        "another client may still be writing it"
    );
    assert_eq!(fake.value, Some(1));
}

#[test]
fn the_recovery_of_an_incomplete_marker_has_a_log_line() {
    assert_eq!(
        Recovery::Discarded.to_string(),
        "lock policy: removed an incomplete marker"
    );
}

#[test]
fn any_value_other_than_one_is_written_back_as_it_was() {
    let mut p = policy(Some(7));
    p.want(Want::On, 0);
    assert_eq!(p.registry().value, Some(1));
    p.exit();
    assert_eq!(
        (p.registry().value, p.registry().marker),
        (Some(7), None),
        "not collapsed to 0"
    );
}

#[test]
fn a_marker_with_a_prior_of_any_value_is_restored_to_it() {
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(marker(Prior::Value(u32::MAX)));
    restore_if_owned(&mut fake, Some(ME)).expect("restore");
    assert_eq!((fake.value, fake.marker), (Some(u32::MAX), None));
}

#[test]
fn the_stored_prior_round_trips_and_reads_the_first_markers() {
    for prior in [
        Prior::Absent,
        Prior::Value(0),
        Prior::Value(1),
        Prior::Value(2),
        Prior::Value(u32::MAX),
    ] {
        assert_eq!(Prior::from_stored(prior.to_stored()), Ok(prior));
    }
    // What the first markers held: 0 absent, 1 for a 0, 2 for a 1.
    assert_eq!(Prior::from_stored(1), Ok(Prior::Value(0)));
    assert_eq!(Prior::from_stored(2), Ok(Prior::Value(1)));
    assert!(Prior::from_stored(u64::from(u32::MAX) + 2).is_err());
}

#[test]
fn every_write_happens_while_the_registry_is_held() {
    let mut p = policy(Some(0));
    p.want(Want::On, 0);
    p.want(Want::Off, 10);
    p.tick(10_000);
    p.want(Want::On, 20_000);
    p.exit();
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(marker(Prior::Absent));
    recover(&mut fake, &Running(BTreeSet::new()));
    fake.partial_marker = true;
    recover(&mut fake, &Running(BTreeSet::new()));
    let mut exit = FakeExit { waited: false };
    fake.marker = Some(marker(Prior::Absent));
    run_watchdog(&mut exit, &mut fake, ME).expect("watchdog");
    assert_eq!(p.registry().unheld, vec![], "the policy wrote unheld");
    assert_eq!(fake.unheld, vec![], "a restore wrote unheld");
    assert_eq!(p.registry().holds.get(), 0, "every hold was let go");
}

#[test]
fn a_turn_on_refused_at_exit_leaves_neither_a_marker_nor_a_value() {
    let fake = Fake {
        refuse_set_on: true,
        ..Fake::with_value(Some(0))
    };
    let mut p = LockPolicy::new(fake, ME);
    assert!(matches!(p.want(Want::On, 0), Change::Failed(_)));
    assert_eq!((p.registry().value, p.registry().marker), (Some(0), None));
    assert!(!p.is_effective());
}

#[test]
fn a_value_of_one_with_another_clients_marker_is_not_the_users() {
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(Marker {
        owner: OTHER,
        prior: Prior::Absent,
    });
    let mut p = LockPolicy::new(fake, ME);
    assert!(matches!(p.want(Want::On, 0), Change::Failed(_)));
    assert!(p.is_disabled(), "the chord is the route, not the setting");
    assert!(!p.is_effective(), "the other client may put it back");
    assert_eq!(
        p.registry().writes(),
        vec![],
        "and nothing of theirs is touched"
    );
}

#[test]
fn a_value_of_one_with_our_own_marker_is_taken_back_up() {
    let mut fake = Fake::with_value(Some(1));
    fake.marker = Some(marker(Prior::Absent));
    let mut p = LockPolicy::new(fake, ME);
    assert_eq!(p.want(Want::On, 0), Change::Set);
    assert!(p.is_effective());
    p.exit();
    assert_eq!((p.registry().value, p.registry().marker), (None, None));
}

#[test]
fn a_wish_for_on_after_a_failed_clear_turns_the_setting_on_again() {
    let mut p = policy(None);
    p.want(Want::On, 0);
    p.want(Want::Off, 10);
    // The value goes back, then the marker cannot be removed.
    p.registry.fail_delete_marker = true;
    assert!(matches!(p.tick(1_000), Change::Failed(_)));
    assert_eq!(p.registry().value, None, "locking works again");
    p.registry.fail_delete_marker = false;
    // The pointer comes back before the retry.
    assert_eq!(p.want(Want::On, 1_100), Change::Set);
    assert_eq!(
        p.registry().value,
        Some(1),
        "Win+L goes to the remote again"
    );
    assert!(!p.clear_pending());
}
