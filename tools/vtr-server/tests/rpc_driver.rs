use std::{
    future::Future,
    pin::Pin,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{sync_channel, Receiver},
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};
use vtr_query::{
    metadata::{Path, Text},
    rpc_session::{Incarnation, RpcDriver},
    session::{Query, Reply},
    wave::Limits,
    wire, Budget, Grid, Interval, TimeBound,
};
struct Host {
    driver: RpcDriver,
    child: Child,
    input: ChildStdin,
    frames: Receiver<wire::Frame>,
    incarnation: Incarnation,
}
impl Host {
    fn drive<F: Future + Unpin>(&mut self, mut future: F) -> F::Output {
        loop {
            if let Poll::Ready(value) =
                Pin::new(&mut future).poll(&mut Context::from_waker(Waker::noop()))
            {
                return value;
            }
            self.exchange();
        }
    }
    fn exchange(&mut self) {
        while let Some(packet) = self.driver.take_outbound().unwrap() {
            wire::write_frame(&mut self.input, packet.bytes(), wire::MAX_REQUEST_BYTES).unwrap();
        }
        let frame = self
            .frames
            .recv_timeout(Duration::from_secs(5))
            .expect("child response");
        self.driver
            .receive(self.incarnation, frame.bytes())
            .unwrap();
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn rpc_client_and_native_child_agree_on_every_current_query_family() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rpc.vtr");
    let name = "λ".repeat(600);
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_bits(&name, 1, 2);
    for i in 0..17 {
        writer.set_time(i).unwrap();
        writer.emit_u64(signal, i % 2).unwrap();
    }
    writer.close().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vtr-server"))
        .arg("--stdio")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let (tx, frames) = sync_channel(4);
    std::thread::spawn(move || {
        let budget = Budget::new(8 << 20);
        while let Ok(Some(frame)) =
            wire::read_frame(&mut output, wire::MAX_REPLY_BYTES, budget.clone())
        {
            if tx.send(frame).is_err() {
                break;
            }
        }
    });
    let budget = Budget::new(64 << 20);
    let incarnation = Incarnation([3; 16]);
    let (driver, opening) = RpcDriver::new(incarnation, budget.clone()).unwrap();
    let mut host = Host {
        driver,
        child,
        input,
        frames,
        incarnation,
    };
    let session = host.drive(opening).unwrap();
    let limits = Limits {
        bytes: 8192,
        records: 2,
        work: 1,
    };
    let mut text = None;
    let queries = [
        Query::Window {
            signal: signal.0,
            interval: Interval::new(0, TimeBound::AfterMax).unwrap(),
        },
        Query::Summary {
            signal: signal.0,
            grid: Grid::new(0, 2, 8).unwrap(),
        },
        Query::Children { parent: None },
        Query::Search {
            scope: None,
            needle: "λ".into(),
        },
        Query::Resolve {
            paths: vec![Path {
                segments: vec![name.clone()],
                kind: None,
                occurrence: None,
            }],
        },
    ];
    for query in queries {
        let future = session.execute(query, limits).unwrap();
        let mut reply = host.drive(future).unwrap();
        let first = reply.request;
        let mut complete = false;
        for _ in 0..256 {
            if let Reply::Children(page) = &reply.reply {
                for declaration in page.declarations() {
                    if let Text::Reference { id, bytes } = &declaration.name {
                        text = Some((*id, *bytes));
                    }
                }
            }
            let Some(next) = reply.next else {
                complete = true;
                break;
            };
            reply = host.drive(session.advance(next).unwrap()).unwrap();
        }
        assert!(complete, "bounded work slices must eventually exhaust");
        session.release(first).unwrap();
    }
    let (id, bytes) = text.expect("large name reference");
    assert_eq!(bytes, name.len() as u64);
    let part = host
        .drive(
            session
                .execute(
                    Query::Text {
                        id,
                        offset: 1,
                        length: 3,
                    },
                    limits,
                )
                .unwrap(),
        )
        .unwrap();
    let Reply::Text(text) = &part.reply else {
        panic!("text part")
    };
    assert_eq!(text.bytes.as_slice(), &name.as_bytes()[1..4]);
    session.release(part.request).unwrap();
    drop(part);
    session.close();
    while !host.driver.is_closed() {
        host.exchange();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = host.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(session);
    drop(host);
    assert_eq!(budget.used(), 0);
}
