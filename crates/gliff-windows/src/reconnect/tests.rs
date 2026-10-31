//! The policy with a fake clock: only `Instant`s built from one start.

use super::*;
use crate::connect::ServerVersion;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn the_wait_doubles_from_one_and_a_half_seconds_and_stops_at_the_cap() {
    let policy = Policy::STANDARD;
    let waits: Vec<Duration> = (1..=8).map(|n| policy.delay(n)).collect();
    assert_eq!(
        waits,
        [
            ms(1500),
            secs(3),
            secs(6),
            secs(12),
            secs(24),
            secs(30),
            secs(30),
            secs(30)
        ]
    );
}

#[test]
fn a_huge_attempt_number_does_not_overflow() {
    assert_eq!(Policy::STANDARD.delay(255), secs(30));
    assert_eq!(Policy::STANDARD.delay(0), ms(1500));
}

#[test]
fn five_retries_with_growing_waits_then_a_give_up_for_the_limit() {
    let policy = Policy::STANDARD;
    let now = Instant::now();
    let mut backoff = Backoff::default();
    let mut seen = Vec::new();
    for _ in 0..6 {
        seen.push(backoff.ended(&policy, now, true));
    }
    assert_eq!(
        seen,
        [
            Next::Retry {
                attempt: 1,
                after: ms(1500)
            },
            Next::Retry {
                attempt: 2,
                after: secs(3)
            },
            Next::Retry {
                attempt: 3,
                after: secs(6)
            },
            Next::Retry {
                attempt: 4,
                after: secs(12)
            },
            Next::Retry {
                attempt: 5,
                after: secs(24)
            },
            Next::GiveUp { exhausted: true },
        ]
    );
}

#[test]
fn a_failure_that_cannot_pass_gives_up_at_once_and_is_not_the_limit() {
    let mut backoff = Backoff::default();
    assert_eq!(
        backoff.ended(&Policy::STANDARD, Instant::now(), false),
        Next::GiveUp { exhausted: false }
    );
    assert_eq!(backoff.retries(), 0);
}

#[test]
fn a_session_that_connects_and_drops_at_once_still_runs_out_of_retries() {
    // The upstream weakness: reset on connect makes this loop for ever.
    let policy = Policy::STANDARD;
    let start = Instant::now();
    let mut backoff = Backoff::default();
    let mut now = start;
    let mut attempts = 0;
    loop {
        backoff.up(now);
        now += secs(2);
        match backoff.ended(&policy, now, true) {
            Next::Retry { after, .. } => {
                attempts += 1;
                now += after;
            }
            Next::GiveUp { exhausted } => {
                assert!(exhausted);
                break;
            }
        }
        assert!(attempts <= 5, "never more than the limit");
    }
    assert_eq!(attempts, 5);
}

#[test]
fn thirty_seconds_of_uptime_is_a_fresh_start() {
    let policy = Policy::STANDARD;
    let start = Instant::now();
    let mut backoff = Backoff::default();
    for _ in 0..3 {
        backoff.ended(&policy, start, true);
    }
    assert_eq!(backoff.retries(), 3);
    backoff.up(start);
    assert_eq!(
        backoff.ended(&policy, start + secs(30), true),
        Next::Retry {
            attempt: 1,
            after: ms(1500)
        }
    );
}

#[test]
fn twenty_nine_seconds_of_uptime_is_not() {
    let policy = Policy::STANDARD;
    let start = Instant::now();
    let mut backoff = Backoff::default();
    backoff.ended(&policy, start, true);
    backoff.up(start);
    assert_eq!(
        backoff.ended(&policy, start + secs(29), true),
        Next::Retry {
            attempt: 2,
            after: secs(3)
        }
    );
}

#[test]
fn a_second_up_report_does_not_move_the_start_of_the_uptime() {
    let policy = Policy::STANDARD;
    let start = Instant::now();
    let mut backoff = Backoff::default();
    backoff.ended(&policy, start, true);
    backoff.up(start);
    backoff.up(start + secs(25));
    assert_eq!(
        backoff.ended(&policy, start + secs(31), true),
        Next::Retry {
            attempt: 1,
            after: ms(1500)
        }
    );
}

#[test]
fn a_reset_forgets_the_count() {
    let mut backoff = Backoff::default();
    for _ in 0..5 {
        backoff.ended(&Policy::STANDARD, Instant::now(), true);
    }
    backoff.reset();
    assert_eq!(
        backoff.ended(&Policy::STANDARD, Instant::now(), true),
        Next::Retry {
            attempt: 1,
            after: ms(1500)
        }
    );
}

#[test]
fn what_a_reboot_or_a_dropped_network_causes_is_retried() {
    for end in [
        End::Closed,
        End::Error("the link failed".into()),
        End::Ssh(SshFailure::Refused),
        End::Ssh(SshFailure::Unreachable),
        End::Ssh(SshFailure::NameNotFound),
        End::Ssh(SshFailure::Other {
            last_line: "odd".into(),
        }),
    ] {
        assert!(retryable(&end), "{end:?}");
    }
}

#[test]
fn what_the_person_has_to_fix_is_not_retried() {
    let old = ServerVersion::new(0, 3, 2);
    for end in [
        End::Incompatible,
        End::Ssh(SshFailure::ClientMissing),
        End::Ssh(SshFailure::HostUnknown),
        End::Ssh(SshFailure::HostKeyChanged),
        End::Ssh(SshFailure::KeyRefused),
        End::Ssh(SshFailure::KeyFilePermissions),
        End::Ssh(SshFailure::TooManyKeys),
        End::Ssh(SshFailure::ServerMissing),
        End::Ssh(SshFailure::ServerOld { found: old }),
        End::Ssh(SshFailure::ProtocolMismatch),
    ] {
        assert!(!retryable(&end), "{end:?}");
    }
}
