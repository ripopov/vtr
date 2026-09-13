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
        let mut exact = matches!(&query, Query::Window { .. })
            .then(|| volna_core::wave::exact::ExactWindow::new(64, &budget).unwrap());
        let future = session.execute(query, limits).unwrap();
        let mut reply = host.drive(future).unwrap();
        let first = reply.request;
        let mut complete = false;
        for _ in 0..256 {
            if let Some(exact) = &mut exact {
                exact.append(reply.clone()).unwrap();
            }
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
        if let Some(exact) = exact {
            assert!(exact.is_complete());
            assert_eq!(
                exact
                    .changes()
                    .map(|change| change.time)
                    .collect::<Vec<_>>(),
                (0..17).collect::<Vec<_>>()
            );
            assert!(exact.sample_at(16).is_some());
        }
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
    // The production viewer controller consumes the same session contract as
    // its native worker tests, with real child framing and RPC validation here.
    use volna_core::wave::demand::{Demand, Progress, WaveDemands};
    let mut demands = WaveDemands::new(session, limits, 16, 3, 2, &budget).unwrap();
    let grid = Grid::new(0, 2, 8).unwrap();
    demands
        .set(
            0,
            Some(Demand::Summary {
                signal: signal.0,
                grid,
            }),
        )
        .unwrap();
    demands
        .set(
            1,
            Some(Demand::Summary {
                signal: signal.0,
                grid,
            }),
        )
        .unwrap();
    demands
        .set(
            2,
            Some(Demand::Summary {
                signal: signal.0,
                grid: Grid::new(8, 1, 8).unwrap(),
            }),
        )
        .unwrap();
    let mut complete = false;
    for _ in 0..512 {
        let progress = host.drive(futures_lite::future::poll_fn(|cx| demands.poll(cx)));
        assert!(demands.active_operations() <= 2);
        match progress {
            Progress::Idle => {
                complete = true;
                break;
            }
            Progress::Backpressure => host.exchange(),
            Progress::Advanced => {}
        }
    }
    assert!(complete, "RPC demand scheduler must finish every row");
    for slot in 0..3 {
        let row = demands.slot(slot).unwrap();
        assert!(row.is_complete(), "row {slot}: {:?}", row.error());
        assert_eq!(row.bins().len(), 8);
    }
    assert_eq!(
        demands
            .slot(0)
            .unwrap()
            .bins()
            .iter()
            .map(|b| b.changes)
            .sum::<u64>(),
        17
    );
    for (a, b) in demands
        .slot(0)
        .unwrap()
        .bins()
        .iter()
        .zip(demands.slot(1).unwrap().bins())
    {
        assert!(std::sync::Arc::ptr_eq(a, b));
    }
    assert_eq!(demands.slot(0).unwrap().value_at(1, 16).unwrap(), None);
    assert_eq!(
        demands.slot(0).unwrap().value_at(3, 16).unwrap(),
        Some(volna_core::data::WaveValue::Bits("1".into()))
    );
    let mut scene = volna_core::Scene::default();
    demands.slot(0).unwrap().paint(
        grid.interval(),
        volna_core::geometry::Rect::new(
            volna_core::geometry::point(0.0, 0.0),
            volna_core::geometry::size(800.0, 24.0),
        ),
        &volna_core::Theme::one_dark(),
        &mut scene,
    );
    assert!(
        scene.prims.len() > 2,
        "wire results must reach the display list"
    );
    let exact_interval = Interval::new(8, TimeBound::Tick(16)).unwrap();
    for slot in 0..2 {
        demands
            .set(
                slot,
                Some(Demand::Window {
                    signal: signal.0,
                    interval: exact_interval,
                }),
            )
            .unwrap();
    }
    let mut exact_complete = false;
    for _ in 0..512 {
        match host.drive(futures_lite::future::poll_fn(|cx| demands.poll(cx))) {
            Progress::Idle => {
                exact_complete = true;
                break;
            }
            Progress::Backpressure => host.exchange(),
            Progress::Advanced => {}
        }
    }
    assert!(exact_complete);
    for slot in 0..2 {
        let row = demands.slot(slot).unwrap();
        assert!(row.is_complete(), "exact row {slot}: {:?}", row.error());
        assert!(row.bins().is_empty());
        let exact = row.exact().unwrap();
        assert_eq!(
            row.value_at(9, 16).unwrap(),
            Some(volna_core::data::WaveValue::Bits("1".into()))
        );
        assert_eq!(row.value_at(16, 16).unwrap(), None);
        assert_eq!(
            exact.changes().map(|c| c.time).collect::<Vec<_>>(),
            (8..16).collect::<Vec<_>>()
        );
    }
    assert!(std::ptr::eq(
        demands.slot(0).unwrap().exact().unwrap(),
        demands.slot(1).unwrap().exact().unwrap()
    ));
    assert_eq!(demands.slot(2).unwrap().bins().len(), 8);
    scene.clear();
    demands.slot(0).unwrap().paint(
        exact_interval,
        volna_core::geometry::Rect::new(
            volna_core::geometry::point(0.0, 0.0),
            volna_core::geometry::size(800.0, 24.0),
        ),
        &volna_core::Theme::one_dark(),
        &mut scene,
    );
    assert!(scene.prims.len() > 2);
    // Exercise the existing row model and full panel painter with remote data.
    let data = demands.snapshot(0).unwrap().unwrap();
    let mut doc = volna_core::Document::new();
    doc.set_session(std::sync::Arc::new(
        volna_core::session::LocalSession::open(&path).unwrap(),
    ));
    let generation = doc.generation();
    assert!(doc.bind_query_session(generation, data.snapshot()));
    let mut model = volna_core::wave::WaveModel::new();
    model.add_vars(&mut doc, &[0]);
    assert!(doc.take_requests().is_empty());
    assert!(model.install_query(&doc, generation, 0, data.clone()));
    doc.shared.cursor = Some(9);
    let theme = volna_core::Theme::one_dark();
    let layout = model
        .layout(
            volna_core::geometry::Rect::new(
                volna_core::geometry::point(0.0, 0.0),
                volna_core::geometry::size(1000.0, 200.0),
            ),
            &doc,
            &theme,
        )
        .clone();
    scene.clear();
    volna_core::wave::paint::paint(
        &model,
        &doc,
        &theme,
        &mut volna_core::scene::TextCache::default(),
        &mut volna_core::scene::MonoMeasure,
        &mut scene,
        true,
    );
    assert!(scene.prims.iter().any(|p| matches!(p, volna_core::scene::Prim::Text { text, origin, .. } if text == "1" && origin.x >= layout.values.left() && origin.x < layout.values.right())));
    drop(model);
    drop(doc);
    drop(data);
    drop(demands); // closes the owned RPC session
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
    drop(host);
    assert_eq!(budget.used(), 0);
}
