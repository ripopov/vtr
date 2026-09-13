use std::{
    io::Write,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{sync_channel, Receiver},
    time::{Duration, Instant},
};
use vtr_query::{
    session::{Query, Reply, SnapshotId},
    wave::Limits,
    wire::{self, proto::failure::Code},
    wire_encode,
    wire_reply::{self, Body, Response},
    wire_request::RequestBody,
    Budget, Interval, TimeBound,
};
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<Response>,
    next: u64,
    snapshot: Option<SnapshotId>,
    budget: Budget,
}
impl Client {
    fn new(path: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vtr-server"))
            .arg("--stdio")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let budget = Budget::new(64 << 20);
        let read_budget = budget.clone();
        let (tx, replies) = sync_channel(16);
        std::thread::spawn(move || {
            while let Ok(Some(frame)) =
                wire::read_frame(&mut output, wire::MAX_REPLY_BYTES, read_budget.clone())
            {
                let response = wire_reply::decode(frame.bytes(), &read_budget)
                    .expect("server emitted invalid reply");
                if tx.send(response).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            replies,
            next: 1,
            snapshot: None,
            budget,
        }
    }
    fn send(&mut self, body: RequestBody) -> u64 {
        let id = self.next;
        self.next += 1;
        let bytes = wire_encode::request(id, self.snapshot, &body, &self.budget).unwrap();
        wire::write_frame(
            self.input.as_mut().unwrap(),
            bytes.bytes(),
            wire::MAX_REQUEST_BYTES,
        )
        .unwrap();
        id
    }
    fn recv(&self) -> Response {
        self.replies
            .recv_timeout(Duration::from_secs(5))
            .expect("child must respond")
    }
    fn open(&mut self) {
        let id = self.send(RequestBody::Hello);
        let reply = self.recv();
        assert_eq!(reply.request_id, id);
        assert!(matches!(reply.body(), Body::Welcome(_)));
        let id = self.send(RequestBody::Open);
        let reply = self.recv();
        assert_eq!(reply.request_id, id);
        let Body::Opened { info, .. } = reply.body() else {
            panic!("expected opened: {:?}", reply.body())
        };
        self.snapshot = Some(info.snapshot);
    }
    fn exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "child must exit");
            std::thread::yield_now();
        }
    }
    fn close(&mut self) {
        let id = self.send(RequestBody::Close);
        loop {
            let reply = self.recv();
            if reply.request_id == id {
                assert!(matches!(reply.body(), Body::Acknowledged));
                break;
            }
        }
        assert!(self.exit().success());
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pipe.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_bits("signal", 1, 2);
    for time in 0..17 {
        writer.set_time(time).unwrap();
        writer.emit_u64(signal, time % 2).unwrap();
    }
    writer.close().unwrap();
    (dir, path, signal.0)
}
fn query(signal: u32) -> RequestBody {
    RequestBody::Query {
        query: Query::Window {
            signal,
            interval: Interval::new(0, TimeBound::AfterMax).unwrap(),
        },
        limits: Limits {
            bytes: 8192,
            records: 2,
            work: 256,
        },
    }
}
#[test]
fn real_child_handshake_pages_retry_release_and_close() {
    let (_dir, path, signal) = fixture();
    let mut c = Client::new(&path);
    c.open();
    let mut id = c.send(query(signal));
    let mut times = vec![];
    let mut first = None;
    loop {
        let response = c.recv();
        assert_eq!(response.request_id, id);
        assert_eq!(response.snapshot, c.snapshot);
        let Body::Delivery(d) = response.body() else {
            panic!("delivery: {:?}", response.body())
        };
        first.get_or_insert(d.request);
        let Reply::Window(page) = &d.reply else {
            panic!("window")
        };
        times.extend(page.changes().iter().map(|c| c.time));
        let Some(next) = d.next else { break };
        id = c.send(RequestBody::Next(next));
    }
    assert_eq!(times, (0..17).collect::<Vec<_>>());
    let id = c.send(RequestBody::Release(first.unwrap()));
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(reply.body(), Body::Acknowledged));
    let id = c.send(query(signal));
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    let Body::Delivery(d) = reply.body() else {
        panic!("delivery")
    };
    let id = c.send(RequestBody::Next(d.request));
    let retry = c.recv();
    assert_eq!(retry.request_id, id);
    let Body::Delivery(retry) = retry.body() else {
        panic!("retry")
    };
    assert_eq!(d.request, retry.request);
    assert_eq!(d.next, retry.next);
    c.close();
}
#[test]
fn errors_are_correlated_and_do_not_poison_the_open_snapshot() {
    let (_dir, path, signal) = fixture();
    let mut c = Client::new(&path);
    let id = c.send(RequestBody::Open);
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(
        reply.body(),
        Body::Failure {
            code: Code::Invalid,
            ..
        }
    ));
    c.open();
    let saved = c.snapshot;
    c.snapshot = Some(SnapshotId([0; 16]));
    let id = c.send(query(signal));
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(
        reply.body(),
        Body::Failure {
            code: Code::Invalid,
            ..
        }
    ));
    c.snapshot = saved;
    let id = c.send(query(u32::MAX));
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(reply.body(), Body::Failure { .. }));
    let id = c.send(query(signal));
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(reply.body(), Body::Delivery(_)));
    c.close();
}
#[test]
fn cancelled_requests_and_four_operation_limit_keep_control_responsive() {
    let (_dir, path, signal) = fixture();
    let mut c = Client::new(&path);
    c.open();
    let target = c.send(query(signal));
    let cancel = c.send(RequestBody::Cancel(target));
    let mut cursor = None;
    for _ in 0..2 {
        let reply = c.recv();
        if reply.request_id == cancel {
            assert!(matches!(reply.body(), Body::Acknowledged));
        } else {
            assert_eq!(reply.request_id, target);
            match reply.body() {
                Body::Delivery(d) => cursor = Some(d.request),
                Body::Failure {
                    code: Code::Cancelled,
                    ..
                } => (),
                other => panic!("{other:?}"),
            }
        }
    }
    if let Some(cursor) = cursor {
        c.send(RequestBody::Release(cursor));
        assert!(matches!(c.recv().body(), Body::Acknowledged));
    }
    // Fill retained operation capacity with unread continuations. Control
    // requests use a separate allowance and remain usable at that ceiling.
    for _ in 0..4 {
        c.send(query(signal));
        assert!(matches!(c.recv().body(), Body::Delivery(_)));
    }
    let id = c.send(query(signal));
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(
        reply.body(),
        Body::Failure {
            code: Code::ResourceLimit,
            ..
        }
    ));
    c.close();
}
#[test]
fn malformed_frames_and_incompatible_versions_terminate_without_protocol_noise() {
    let (_dir, path, _) = fixture();
    for malformed in [vec![0, 0, 0, 0], u32::MAX.to_le_bytes().to_vec()] {
        let mut c = Client::new(&path);
        c.input.as_mut().unwrap().write_all(&malformed).unwrap();
        assert!(!c.exit().success());
        assert!(c.replies.try_recv().is_err());
    }
    let mut c = Client::new(&path);
    let bytes = wire_encode::request(1, None, &RequestBody::Hello, &c.budget).unwrap();
    let mut bytes = bytes.bytes().to_vec();
    assert_eq!(&bytes[..2], &[8, 1]);
    bytes[1] = 99;
    wire::write_frame(c.input.as_mut().unwrap(), &bytes, wire::MAX_REQUEST_BYTES).unwrap();
    assert!(!c.exit().success());
    assert!(c.replies.try_recv().is_err());
}
#[test]
fn stdin_eof_and_close_during_opening_terminate_the_child() {
    let (_dir, path, _) = fixture();
    let mut c = Client::new(&path);
    c.open();
    drop(c.input.take());
    assert!(c.exit().success());
    let mut c = Client::new(&path);
    c.send(RequestBody::Hello);
    assert!(matches!(c.recv().body(), Body::Welcome(_)));
    c.send(RequestBody::Open);
    let close = c.send(RequestBody::Close);
    // Close may precede or follow opening. A snapshot-less close is valid
    // while opening; if already opened, send a close tied to that snapshot.
    loop {
        let reply = c.recv();
        match reply.body() {
            Body::Opened { info, .. } => c.snapshot = Some(info.snapshot),
            Body::Acknowledged if reply.request_id == close => {
                assert!(c.exit().success());
                break;
            }
            Body::Failure { .. } if reply.request_id == close => {
                c.close();
                break;
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn failed_open_can_retry_but_request_id_reuse_is_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("later.vtr");
    let mut c = Client::new(&path);
    c.send(RequestBody::Hello);
    assert!(matches!(c.recv().body(), Body::Welcome(_)));
    let id = c.send(RequestBody::Open);
    let reply = c.recv();
    assert_eq!(reply.request_id, id);
    assert!(matches!(
        reply.body(),
        Body::Failure {
            code: Code::Backend,
            ..
        }
    ));
    let mut writer = vtr::Writer::create(&path).unwrap();
    writer.add_bits("s", 1, 2);
    writer.close().unwrap();
    c.send(RequestBody::Open);
    let reply = c.recv();
    let Body::Opened { info, .. } = reply.body() else {
        panic!("opened")
    };
    c.snapshot = Some(info.snapshot);
    let duplicate = wire_encode::request(c.next - 1, None, &RequestBody::Open, &c.budget).unwrap();
    wire::write_frame(
        c.input.as_mut().unwrap(),
        duplicate.bytes(),
        wire::MAX_REQUEST_BYTES,
    )
    .unwrap();
    assert!(!c.exit().success());
}
