//! The hub with fake engines and a fake Windows thread.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gliff_transport::clipboard::progress::Progress;
use gliff_transport::clipboard::progress::State;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use super::ClipHub;
use crate::clip_engine::Command;
use crate::clip_engine::HostEvent;
use crate::clip_link::EngineIn;
use crate::clip_link::Host;
use crate::clip_log::Cause;
use crate::clip_log::RevokeWhy;
use crate::tabs::SessionId;

/// What reached the Windows thread, as short lines.
#[derive(Default)]
struct FakeWindows(Mutex<Vec<String>>);

impl Host for FakeWindows {
    fn event(&self, event: HostEvent) {
        let line = match event {
            HostEvent::RemoteOffer { mime_types, .. } => {
                format!("remote-offer {}", mime_types.join(","))
            }
            HostEvent::ReadLocal {
                mime_type, reply, ..
            } => {
                let _ = reply.send(Ok(b"local".to_vec()));
                format!("read-local {mime_type}")
            }
            HostEvent::Transfer { id, .. } => format!("transfer {id}"),
        };
        self.0.lock().expect("lock").push(line);
    }
}

impl FakeWindows {
    fn lines(&self) -> Vec<String> {
        self.0.lock().expect("lock").clone()
    }
}

struct Fixture {
    hub: ClipHub,
    windows: Arc<FakeWindows>,
    input: mpsc::UnboundedSender<EngineIn>,
    hosts: Vec<Arc<dyn Host>>,
    engines: Vec<mpsc::UnboundedReceiver<EngineIn>>,
}

/// Three engines, ids 1 to 3, session `shown` shown.
fn fixture(shown: u32) -> Fixture {
    let (hub, input) = ClipHub::new().expect("the router thread");
    let windows = Arc::new(FakeWindows::default());
    hub.attach_windows(windows.clone());
    let mut hosts = Vec::new();
    let mut engines = Vec::new();
    for number in 1..=3 {
        let (tx, rx) = mpsc::unbounded_channel();
        hosts.push(hub.register(SessionId::from_raw(number), tx));
        engines.push(rx);
    }
    hub.show(Some(SessionId::from_raw(shown)));
    Fixture {
        hub,
        windows,
        input,
        hosts,
        engines,
    }
}

fn kinds(rx: &mut mpsc::UnboundedReceiver<EngineIn>) -> Vec<&'static str> {
    let mut out = Vec::new();
    while let Ok(message) = rx.try_recv() {
        out.push(match message {
            EngineIn::Command(Command::RevokeLocal(_)) => "revoke",
            EngineIn::Command(Command::LocalOffer { .. }) => "offer",
            EngineIn::Command(Command::FetchMime { .. }) => "fetch",
            EngineIn::Command(Command::FetchFiles { .. }) => "fetch-files",
            EngineIn::Command(Command::CancelTransfer(_)) => "cancel",
            EngineIn::Peer(..) => "peer",
            EngineIn::Stop => "stop",
        });
    }
    out
}

/// Everything each engine received so far. A cancel for a transfer of each
/// engine goes in last; the router handles messages in order, so once an
/// engine has that cancel, everything sent before it has been handled. The
/// marker is dropped from the result.
fn drained(fixture: &mut Fixture) -> Vec<Vec<&'static str>> {
    let progress = || Progress {
        label: "marker".to_owned(),
        done: 0,
        total: None,
        rate: 0.0,
        state: State::Running,
    };
    for (index, host) in fixture.hosts.iter().enumerate() {
        let local = 1_000_000 + index as u32;
        host.event(HostEvent::Transfer {
            id: local,
            progress: progress(),
        });
    }
    // The fake Windows thread saw the ids the hub made up; read them back.
    let lines = fixture.windows.lines();
    for line in &lines[lines.len() - fixture.hosts.len()..] {
        let global: u32 = line
            .strip_prefix("transfer ")
            .and_then(|id| id.parse().ok())
            .expect("a transfer line");
        fixture
            .input
            .send(EngineIn::Command(Command::CancelTransfer(global)))
            .expect("send");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut all: Vec<Vec<&'static str>> = vec![Vec::new(); fixture.engines.len()];
    for (index, engine) in fixture.engines.iter_mut().enumerate() {
        while all[index].last() != Some(&"cancel") {
            all[index].extend(kinds(engine));
            assert!(
                std::time::Instant::now() < deadline,
                "engine {index} never got the marker"
            );
            std::thread::yield_now();
        }
        all[index].pop();
    }
    all
}

fn local_change(fixture: &Fixture) {
    fixture
        .input
        .send(EngineIn::Command(Command::RevokeLocal(
            RevokeWhy::LocalChange,
        )))
        .expect("send");
    fixture
        .input
        .send(EngineIn::Command(Command::LocalOffer {
            mime_types: vec!["text/plain".to_owned()],
            files: Default::default(),
            generation: 1,
        }))
        .expect("send");
}

#[test]
fn a_local_change_revokes_every_engine_and_offers_only_the_shown_one() {
    let mut fx = fixture(2);
    local_change(&fx);
    assert_eq!(
        drained(&mut fx),
        [vec!["revoke"], vec!["revoke", "offer"], vec!["revoke"]]
    );
}

#[test]
fn after_a_switch_the_new_shown_engine_gets_the_offer() {
    let mut fx = fixture(1);
    fx.hub.show(Some(SessionId::from_raw(3)));
    local_change(&fx);
    assert_eq!(
        drained(&mut fx),
        [vec!["revoke"], vec!["revoke"], vec!["revoke", "offer"]]
    );
}

fn offer(host: &Arc<dyn Host>, text: &str) {
    host.event(HostEvent::RemoteOffer {
        serial: 1,
        mime_types: vec![text.to_owned()],
        files: Vec::new(),
    });
}

#[test]
fn only_the_shown_sessions_remote_offer_reaches_windows() {
    let fx = fixture(1);
    offer(&fx.hosts[1], "text/background");
    offer(&fx.hosts[0], "text/shown");
    assert_eq!(fx.windows.lines(), ["remote-offer text/shown"]);
}

#[test]
fn a_paste_goes_to_the_owner_after_a_switch() {
    let mut fx = fixture(1);
    offer(&fx.hosts[0], "text/plain");
    fx.hub.show(Some(SessionId::from_raw(2)));
    let (result, _answer) = oneshot::channel();
    fx.input
        .send(EngineIn::Command(Command::FetchMime {
            mime_type: "text/plain".to_owned(),
            serial: 1,
            result,
        }))
        .expect("send");
    assert_eq!(drained(&mut fx), [vec!["fetch"], vec![], vec![]]);
}

#[test]
fn a_paste_with_no_owner_fails_at_once() {
    let fx = fixture(1);
    let (result, answer) = oneshot::channel::<Result<Vec<u8>, Cause>>();
    fx.input
        .send(EngineIn::Command(Command::FetchMime {
            mime_type: "text/plain".to_owned(),
            serial: 1,
            result,
        }))
        .expect("send");
    assert!(
        answer.blocking_recv().is_err(),
        "the reply channel is dropped"
    );
}

#[test]
fn when_the_owner_leaves_its_data_is_withdrawn_and_its_engine_stopped() {
    let mut fx = fixture(1);
    offer(&fx.hosts[0], "text/plain");
    fx.hub.session_gone(SessionId::from_raw(1));
    assert_eq!(
        fx.windows.lines(),
        ["remote-offer text/plain", "remote-offer "],
        "an empty offer withdraws the formats"
    );
    assert_eq!(kinds(&mut fx.engines[0]), ["stop"]);
    let (result, answer) = oneshot::channel::<Result<Vec<u8>, Cause>>();
    fx.input
        .send(EngineIn::Command(Command::FetchMime {
            mime_type: "text/plain".to_owned(),
            serial: 1,
            result,
        }))
        .expect("send");
    assert!(answer.blocking_recv().is_err(), "no owner: fails at once");
}

#[test]
fn sharing_off_stops_both_ways_and_takes_back_only_its_own_offer() {
    let mut fx = fixture(1);
    offer(&fx.hosts[0], "text/plain");
    fx.hub.set_sharing(SessionId::from_raw(2), false);
    assert_eq!(
        fx.windows.lines(),
        ["remote-offer text/plain"],
        "a session that owns nothing takes nothing back"
    );
    fx.hub.set_sharing(SessionId::from_raw(1), false);
    assert_eq!(
        fx.windows.lines(),
        ["remote-offer text/plain", "remote-offer "],
        "the owner's empty offer: Windows clears it only if it is still ours"
    );
    offer(&fx.hosts[0], "text/after");
    assert_eq!(fx.windows.lines().len(), 2, "its new offer is dropped");
    let (reply, answer) = oneshot::channel();
    fx.hosts[0].event(HostEvent::ReadLocal {
        mime_type: "text/plain".to_owned(),
        generation: 1,
        reply,
    });
    assert!(answer.blocking_recv().expect("an answer").is_err());
    local_change(&fx);
    assert_eq!(
        drained(&mut fx),
        [
            vec!["revoke", "revoke"],
            vec!["revoke", "revoke"],
            vec!["revoke"]
        ],
        "each turned-off engine is revoked at once, and offered nothing after"
    );
    fx.hub.set_sharing(SessionId::from_raw(1), true);
    local_change(&fx);
    assert_eq!(
        drained(&mut fx),
        [vec!["revoke", "offer"], vec!["revoke"], vec!["revoke"]]
    );
}

#[test]
fn a_background_session_may_not_read_the_local_clipboard() {
    let fx = fixture(1);
    let (reply, answer) = oneshot::channel();
    fx.hosts[1].event(HostEvent::ReadLocal {
        mime_type: "text/plain".to_owned(),
        generation: 1,
        reply,
    });
    assert!(answer.blocking_recv().expect("an answer").is_err());
    let (reply, answer) = oneshot::channel();
    fx.hosts[0].event(HostEvent::ReadLocal {
        mime_type: "text/plain".to_owned(),
        generation: 1,
        reply,
    });
    assert_eq!(
        answer.blocking_recv().expect("an answer"),
        Ok(b"local".to_vec())
    );
}

#[test]
fn transfers_of_two_engines_get_different_ids_and_a_cancel_finds_its_engine() {
    let mut fx = fixture(1);
    let progress = || Progress {
        label: "x".to_owned(),
        done: 0,
        total: None,
        rate: 0.0,
        state: State::Running,
    };
    for host in &fx.hosts[..2] {
        host.event(HostEvent::Transfer {
            id: 1,
            progress: progress(),
        });
    }
    assert_eq!(fx.windows.lines(), ["transfer 1", "transfer 2"]);
    fx.input
        .send(EngineIn::Command(Command::CancelTransfer(2)))
        .expect("send");
    assert_eq!(drained(&mut fx), [vec![], vec!["cancel"], vec![]]);
}

#[test]
fn events_before_the_windows_thread_is_attached_are_refused_not_lost() {
    let (hub, _input) = ClipHub::new().expect("thread");
    let (tx, _rx) = mpsc::unbounded_channel();
    let host = hub.register(SessionId::from_raw(1), tx);
    let (reply, answer) = oneshot::channel();
    host.event(HostEvent::ReadLocal {
        mime_type: "text/plain".to_owned(),
        generation: 1,
        reply,
    });
    assert!(answer.blocking_recv().expect("an answer").is_err());
}

#[test]
fn revoke_reaches_only_that_engine() {
    let mut fx = fixture(2);
    fx.hub.revoke(SessionId::from_raw(1));
    assert_eq!(kinds(&mut fx.engines[0]), ["revoke"]);
    assert!(kinds(&mut fx.engines[1]).is_empty());
    assert!(kinds(&mut fx.engines[2]).is_empty());
}

#[test]
fn cancel_transfers_cancels_each_of_that_sessions_transfers_by_its_own_id() {
    let mut fx = fixture(1);
    let progress = || Progress {
        label: "x".to_owned(),
        done: 0,
        total: None,
        rate: 0.0,
        state: State::Running,
    };
    for (host, id) in [(0, 1), (0, 2), (1, 1)] {
        fx.hosts[host].event(HostEvent::Transfer {
            id,
            progress: progress(),
        });
    }
    fx.hub.cancel_transfers(SessionId::from_raw(1));
    assert_eq!(kinds(&mut fx.engines[0]), ["cancel", "cancel"]);
    assert!(
        kinds(&mut fx.engines[1]).is_empty(),
        "another session's transfer stays"
    );
    fx.hub.cancel_transfers(SessionId::from_raw(3));
    assert!(kinds(&mut fx.engines[2]).is_empty(), "none to cancel");
}
