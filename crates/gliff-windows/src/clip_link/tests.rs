use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use bytes::Bytes;
use gliff_proto::ClipboardMsg;
use gliff_transport::clipboard::files::LocalFiles;
use tokio::sync::oneshot;

use super::EngineIn;
use super::EngineThread;
use super::Host;
use super::Port;
use super::spawn;
use crate::clip_engine::Command;
use crate::clip_engine::HostEvent;

const TEXT: &str = "text/plain;charset=utf-8";
const WAIT: Duration = Duration::from_secs(10);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gwin-clip-link-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A host that hands every event to the test.
struct Recorder(Mutex<mpsc::Sender<HostEvent>>);

impl Host for Recorder {
    fn event(&self, event: HostEvent) {
        let _ = self.0.lock().expect("the recorder lock").send(event);
    }
}

struct Rig {
    engine: EngineThread,
    input: tokio::sync::mpsc::UnboundedSender<EngineIn>,
    host: mpsc::Receiver<HostEvent>,
    wire: mpsc::Receiver<(ClipboardMsg, Bytes)>,
    dir: TempDir,
}

fn rig(label: &str, prepare: impl FnOnce(&Path)) -> Rig {
    let dir = TempDir::new(label);
    let spool = dir.path().join("spool");
    prepare(&spool);
    let (input, output) = Port::channel();
    let (host_tx, host) = mpsc::channel();
    let (wire_tx, wire) = mpsc::channel();
    let wire_tx = Mutex::new(wire_tx);
    let port = Port::new(
        input.clone(),
        output,
        Arc::new(Recorder(Mutex::new(host_tx))),
        spool,
    );
    let engine = spawn(
        port,
        Arc::new(move |msg, payload| {
            let _ = wire_tx.lock().expect("the wire lock").send((msg, payload));
        }),
    )
    .expect("start the engine thread");
    Rig {
        engine,
        input,
        host,
        wire,
        dir,
    }
}

#[test]
fn a_server_offer_reaches_the_windows_thread() {
    let rig = rig("offer", |_| {});
    rig.engine.peer(
        ClipboardMsg::Offer {
            serial: 4,
            mime_types: vec![TEXT.to_owned()],
            files: Vec::new(),
        },
        Bytes::new(),
    );
    match rig.host.recv_timeout(WAIT).expect("a host event") {
        HostEvent::RemoteOffer {
            serial, mime_types, ..
        } => {
            assert_eq!(serial, 4, "the server's serial");
            assert_eq!(mime_types, vec![TEXT.to_owned()], "the flavours");
        }
        _ => panic!("expected a remote offer"),
    }
    rig.engine.stop();
}

#[test]
fn a_local_offer_goes_to_the_server() {
    let rig = rig("local", |_| {});
    rig.input
        .send(EngineIn::Command(Command::LocalOffer {
            mime_types: vec![TEXT.to_owned()],
            files: LocalFiles::default(),
            generation: 1,
        }))
        .expect("the engine is alive");
    let (msg, payload) = rig.wire.recv_timeout(WAIT).expect("a message");
    assert!(
        matches!(&msg, ClipboardMsg::Offer { mime_types, .. } if mime_types == &[TEXT.to_owned()]),
        "an offer of the one flavour, got {msg:?}"
    );
    assert!(payload.is_empty(), "an offer has no payload");
    rig.engine.stop();
}

#[test]
fn a_paste_fetches_the_body_from_the_server() {
    let rig = rig("paste", |_| {});
    rig.engine.peer(
        ClipboardMsg::Offer {
            serial: 9,
            mime_types: vec![TEXT.to_owned()],
            files: Vec::new(),
        },
        Bytes::new(),
    );
    let _ = rig.host.recv_timeout(WAIT).expect("the offer event");
    let (result, answer) = oneshot::channel();
    rig.input
        .send(EngineIn::Command(Command::FetchMime {
            mime_type: TEXT.to_owned(),
            serial: 9,
            result,
        }))
        .expect("the engine is alive");
    let (msg, _) = rig.wire.recv_timeout(WAIT).expect("a request");
    let ClipboardMsg::Request { id, serial, .. } = msg else {
        panic!("expected a request, got {msg:?}");
    };
    assert_eq!(serial, 9, "the request names the offer");
    rig.engine.peer(
        ClipboardMsg::Data {
            id,
            offset: 0,
            data_len: 5,
            done: true,
        },
        Bytes::from_static(b"hello"),
    );
    let body = answer
        .blocking_recv()
        .expect("an answer")
        .expect("the paste succeeds");
    assert_eq!(body, b"hello".to_vec(), "the pasted body");
    rig.engine.stop();
}

#[test]
fn start_and_stop_clean_the_spool_but_never_a_running_clients() {
    let live = std::process::id();
    let rig = rig("sweep", |spool| {
        // Process id 4294967000 does not exist.
        std::fs::create_dir_all(spool.join("4294967000-1")).expect("an old spool dir");
        std::fs::write(spool.join("4294967000-1").join("a.txt"), b"x").expect("an old file");
        std::fs::create_dir_all(spool.join(format!("{live}-77"))).expect("this process's dir");
    });
    // Anything the thread has handled proves the start-up sweep has run.
    rig.engine.peer(
        ClipboardMsg::Offer {
            serial: 1,
            mime_types: vec![TEXT.to_owned()],
            files: Vec::new(),
        },
        Bytes::new(),
    );
    let _ = rig.host.recv_timeout(WAIT).expect("the offer event");
    let spool = rig.dir.path().join("spool");
    assert!(
        !spool.join("4294967000-1").exists(),
        "the ended run's files are gone at the start"
    );
    assert!(spool.join(format!("{live}-77")).exists());
    rig.engine.stop();
    assert!(
        !spool.join(format!("{live}-77")).exists(),
        "this process's spool is gone after stop"
    );
}

#[test]
fn stopping_releases_a_paste_still_waiting() {
    let rig = rig("release", |_| {});
    rig.engine.peer(
        ClipboardMsg::Offer {
            serial: 2,
            mime_types: vec![TEXT.to_owned()],
            files: Vec::new(),
        },
        Bytes::new(),
    );
    let _ = rig.host.recv_timeout(WAIT).expect("the offer event");
    let (result, answer) = oneshot::channel();
    rig.input
        .send(EngineIn::Command(Command::FetchMime {
            mime_type: TEXT.to_owned(),
            serial: 2,
            result,
        }))
        .expect("the engine is alive");
    let _ = rig.wire.recv_timeout(WAIT).expect("a request");
    rig.engine.stop();
    let outcome = answer.blocking_recv();
    assert!(
        !matches!(outcome, Ok(Ok(_))),
        "a paste cut off by the stop must not succeed"
    );
}
