#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use std::sync::Arc;
use vtr_query::{
    native_session::Session,
    session::{Query, Reply},
    wave::Limits,
    Budget, Cancellation, Error, Grid, Interval, TimeBound,
};

fn fixture() -> (tempfile::TempDir, vtr::Reader, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_bits("s", 1, 2);
    for i in 0..10 {
        writer.set_time(i).unwrap();
        writer.emit_u64(signal, i % 2).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    (dir, reader, signal.0)
}
fn query(signal: u32) -> Query {
    Query::Window {
        signal,
        interval: Interval::new(0, TimeBound::AfterMax).unwrap(),
    }
}
fn limits() -> Limits {
    Limits {
        bytes: 4096,
        records: 1,
        work: 256,
    }
}

#[test]
fn repeated_summaries_reuse_completed_history_without_build_progress() {
    let (_dir, reader, signal) = fixture();
    let budget = Budget::new(32768);
    let mut session = Session::new(&reader, budget.clone(), 2).unwrap();
    let grid = Grid::new(0, 2, 4).unwrap();
    let mut run = || {
        let cursor = session
            .start(
                Query::Summary { signal, grid },
                Limits {
                    records: 4,
                    work: 4,
                    ..limits()
                },
                Cancellation::default(),
            )
            .unwrap();
        let mut next = cursor;
        let mut progress = 0;
        let mut counts = Vec::new();
        loop {
            let delivery = session.advance(next).unwrap();
            let Reply::Summary(page) = &delivery.reply else {
                panic!("wrong reply")
            };
            assert_eq!(page.offset as usize, counts.len());
            if page.bins().is_empty() {
                progress += 1;
            }
            counts.extend(page.bins().iter().map(|bin| bin.changes));
            if let Some(cursor) = delivery.next {
                next = cursor;
            } else {
                break;
            }
        }
        session.release(cursor).unwrap();
        (progress, counts)
    };
    let first = run();
    let warm = run();
    assert!(first.0 > 0);
    assert_eq!(warm.0, 0);
    assert_eq!(first.1, vec![4, 4, 2, 0]);
    assert_eq!(warm.1, first.1);
    drop(session);
    assert_eq!(budget.used(), 0);
}

#[test]
fn warm_edges_are_exact_and_complete_in_one_work_unit() {
    use vtr_query::wave::{ChangeSearchResult, Direction};
    let (_dir, reader, signal) = fixture();
    let mut session = Session::new(&reader, Budget::new(32768), 2).unwrap();
    let one = Limits {
        work: 1,
        ..limits()
    };
    let cold = session
        .start(
            Query::FindChange {
                signal,
                from: 8,
                direction: Direction::Previous,
            },
            one,
            Cancellation::default(),
        )
        .unwrap();
    let first = session.advance(cold).unwrap();
    let Reply::FindChange(page) = &first.reply else {
        panic!("wrong reply")
    };
    assert_eq!(page.result, ChangeSearchResult::Pending);
    session.release(cold).unwrap();
    drop(first);
    let warm = session
        .start(
            Query::Summary {
                signal,
                grid: Grid::new(0, 4, 1).unwrap(),
            },
            one,
            Cancellation::default(),
        )
        .unwrap();
    let mut cursor = warm;
    loop {
        let page = session.advance(cursor).unwrap();
        if let Some(next) = page.next {
            cursor = next;
        } else {
            break;
        }
    }
    session.release(warm).unwrap();
    for (from, direction, expected) in [
        (0, Direction::Previous, ChangeSearchResult::Exhausted),
        (9, Direction::Next, ChangeSearchResult::Exhausted),
        (4, Direction::Previous, ChangeSearchResult::Found(3)),
        (4, Direction::Next, ChangeSearchResult::Found(5)),
        (u64::MAX, Direction::Previous, ChangeSearchResult::Found(9)),
    ] {
        let cursor = session
            .start(
                Query::FindChange {
                    signal,
                    from,
                    direction,
                },
                one,
                Cancellation::default(),
            )
            .unwrap();
        let delivery = session.advance(cursor).unwrap();
        assert!(delivery.next.is_none());
        let Reply::FindChange(page) = &delivery.reply else {
            panic!("wrong reply")
        };
        assert_eq!(page.result, expected);
        session.release(cursor).unwrap();
    }
}

#[test]
fn cursor_batches_mix_cached_logic_events_and_cold_real_values() {
    use vtr_query::wave::{Sample, SignalTime, Value};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cached-points.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            background: false,
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, bits) = writer.add_bits("bits", 8, 4);
    let (_, event) = writer.add_var(
        "event",
        vtr::VarType::Event,
        vtr::Direction::Implicit,
        vtr::SignalKind::Bits {
            width: 1,
            states: 2,
        },
    );
    let (_, real) = writer.add_var(
        "real",
        vtr::VarType::Real,
        vtr::Direction::Implicit,
        vtr::SignalKind::Real,
    );
    let nan = 0x7ff8_0000_0000_002au64;
    writer.set_time(3).unwrap();
    writer.emit_u64(bits, 1).unwrap();
    writer.emit_u64(bits, 2).unwrap();
    writer.emit_u64(event, 1).unwrap();
    writer.emit_real(real, f64::from_bits(nan)).unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let mut session = Session::new(&reader, Budget::new(65536), 2).unwrap();
    for signal in [bits, event] {
        let first = session
            .start(
                Query::Summary {
                    signal: signal.0,
                    grid: Grid::new(0, 2, 1).unwrap(),
                },
                limits(),
                Cancellation::default(),
            )
            .unwrap();
        let mut cursor = first;
        loop {
            let page = session.advance(cursor).unwrap();
            if let Some(next) = page.next {
                cursor = next;
            } else {
                break;
            }
        }
        session.release(first).unwrap();
    }
    let pairs = vec![
        SignalTime {
            signal: bits.0,
            time: 0,
        },
        SignalTime {
            signal: real.0,
            time: 3,
        },
        SignalTime {
            signal: bits.0,
            time: 3,
        },
        SignalTime {
            signal: event.0,
            time: u64::MAX,
        },
        SignalTime {
            signal: bits.0,
            time: u64::MAX,
        },
    ];
    let first = session
        .start(
            Query::ValuesAt {
                pairs: pairs.clone(),
            },
            Limits {
                work: 1,
                ..limits()
            },
            Cancellation::default(),
        )
        .unwrap();
    let mut cursor = first;
    let mut offset = 0;
    loop {
        let delivery = session.advance(cursor).unwrap();
        let Reply::Values(page) = &delivery.reply else {
            panic!("wrong reply")
        };
        assert_eq!(page.offset, offset);
        assert_eq!(page.samples().len(), 1);
        let sample = &page.samples()[0];
        assert_eq!(sample.pair, pairs[offset]);
        match (&sample.sample, offset) {
            (
                Sample::Known(Value::Bits {
                    width: 8,
                    states: 4,
                    data,
                }),
                index,
            ) => {
                let raw = vtr::SignalValue::Bits {
                    width: 8,
                    states: 4,
                    data: data.as_slice(),
                };
                if index == 0 {
                    assert_eq!(raw.to_ascii(), "xxxxxxxx");
                } else {
                    assert_eq!(raw.as_u64(), Some(2));
                }
            }
            (Sample::Known(Value::Real(bits)), 1) => assert_eq!(*bits, nan),
            (Sample::Event, 3) => {}
            _ => panic!("unexpected sample {:?}", sample.sample),
        }
        offset += 1;
        if let Some(next) = delivery.next {
            cursor = next;
        } else {
            break;
        }
    }
    assert_eq!(offset, pairs.len());
    session.release(first).unwrap();
}

#[test]
fn cached_windows_preserve_same_time_values_bounds_and_pressure_retries() {
    use vtr_query::wave::{Sample, Value};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cached-windows.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            block_records: 2,
            background: false,
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_var(
        "bytes",
        vtr::VarType::String,
        vtr::Direction::Implicit,
        vtr::SignalKind::VarLen,
    );
    for (time, byte) in [(0, 1), (0, 2), (3, 3), (u64::MAX, 4)] {
        writer.set_time(time).unwrap();
        writer.emit_varlen(signal, &vec![byte; 1024]).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(1 << 20);
    let mut session = Session::new(&reader, budget.clone(), 2).unwrap();
    let warm = session
        .start(
            Query::Summary {
                signal: signal.0,
                grid: Grid::new(0, 64, 1).unwrap(),
            },
            Limits {
                bytes: 16384,
                records: 1,
                work: 256,
            },
            Cancellation::default(),
        )
        .unwrap();
    let mut cursor = warm;
    loop {
        let delivery = session.advance(cursor).unwrap();
        if let Some(next) = delivery.next {
            cursor = next;
        } else {
            break;
        }
    }
    session.release(warm).unwrap();
    for (start, end, predecessor, expected) in [
        (
            0,
            TimeBound::AfterMax,
            None,
            vec![(0, 1), (0, 2), (3, 3), (u64::MAX, 4)],
        ),
        (0, TimeBound::Tick(3), None, vec![(0, 1), (0, 2)]),
        (3, TimeBound::AfterMax, Some(2), vec![(3, 3), (u64::MAX, 4)]),
        (u64::MAX, TimeBound::AfterMax, Some(3), vec![(u64::MAX, 4)]),
        (3, TimeBound::Tick(3), Some(2), vec![]),
    ] {
        let first = session
            .start(
                Query::Window {
                    signal: signal.0,
                    interval: Interval::new(start, end).unwrap(),
                },
                Limits {
                    bytes: 2600,
                    records: 100,
                    work: 100,
                },
                Cancellation::default(),
            )
            .unwrap();
        let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
        assert!(matches!(session.advance(first), Err(Error::ResourceLimit)));
        drop(pressure);
        let mut cursor = first;
        let mut actual = Vec::new();
        loop {
            let delivery = session.advance(cursor).unwrap();
            let Reply::Window(page) = &delivery.reply else {
                panic!("wrong reply")
            };
            let Sample::Known(Value::Bytes(bytes)) = &page.predecessor.sample else {
                panic!("wrong predecessor")
            };
            assert_eq!(bytes.as_slice().first().copied(), predecessor);
            assert!(
                page.changes().len() <= 1,
                "wide values must obey the byte cap despite a large record limit"
            );
            for change in page.changes() {
                let Value::Bytes(bytes) = &change.value else {
                    panic!("wrong value")
                };
                assert_eq!(bytes.as_slice().len(), 1024);
                actual.push((change.time, bytes.as_slice()[0]));
            }
            if let Some(next) = delivery.next {
                cursor = next;
            } else {
                break;
            }
        }
        assert_eq!(actual, expected);
        session.release(first).unwrap();
    }
    drop(session);
    assert_eq!(budget.used(), 0);
}

#[test]
fn retry_returns_identical_delivery_and_old_cursors_cannot_advance() {
    let (_dir, reader, signal) = fixture();
    let budget = Budget::new(32768);
    let mut session = Session::new(&reader, budget.clone(), 2).unwrap();
    let cursor = session
        .start(query(signal), limits(), Cancellation::default())
        .unwrap();
    let first = session.advance(cursor).unwrap();
    assert!(Arc::ptr_eq(&first, &session.advance(cursor).unwrap()));
    assert!(session
        .advance(vtr_query::session::Continuation { step: 99, ..cursor })
        .is_err());
    let second = session.advance(first.next.unwrap()).unwrap();
    assert!(session.advance(cursor).is_err());
    let Reply::Window(page) = &second.reply else {
        panic!("expected window");
    };
    assert_eq!(page.changes()[0].time, 1);
    session.release(cursor).unwrap();
    session.release(cursor).unwrap();
    assert_eq!(session.active_operations(), 0);
    assert!(session.advance(second.request).is_err());
    drop(session);
    assert!(
        budget.used() > 0,
        "returned deliveries retain their charges"
    );
    drop(first);
    drop(second);
    assert_eq!(budget.used(), 0);
}

#[test]
fn snapshots_reject_cross_session_and_reopened_continuations() {
    let (_dir, reader, signal) = fixture();
    let mut first = Session::new(&reader, Budget::new(32768), 2).unwrap();
    let mut second = Session::new(&reader, Budget::new(32768), 2).unwrap();
    let cursor = first
        .start(query(signal), limits(), Cancellation::default())
        .unwrap();
    assert_ne!(first.info().snapshot, second.info().snapshot);
    assert!(second.advance(cursor).is_err());
    assert!(second.release(cursor).is_err());
    drop(first);
    let mut reopened = Session::new(&reader, Budget::new(32768), 2).unwrap();
    assert!(reopened.advance(cursor).is_err());
}

#[test]
fn capacity_cancellation_and_allocation_failure_preserve_control() {
    let (_dir, reader, signal) = fixture();
    let budget = Budget::new(32768);
    let mut session = Session::new(&reader, budget.clone(), 1).unwrap();
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        session.start(query(signal), limits(), cancelled),
        Err(Error::Cancelled)
    ));
    let token = Cancellation::default();
    let cursor = session
        .start(query(signal), limits(), token.clone())
        .unwrap();
    assert!(matches!(
        session.start(query(signal), limits(), Cancellation::default()),
        Err(Error::ResourceLimit)
    ));
    let pin = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(matches!(session.advance(cursor), Err(Error::ResourceLimit)));
    drop(pin);
    let first = session.advance(cursor).unwrap();
    let Reply::Window(page) = &first.reply else {
        panic!("expected window");
    };
    assert_eq!(page.changes()[0].time, 0);
    token.cancel();
    assert!(matches!(
        session.advance(first.next.unwrap()),
        Err(Error::Cancelled)
    ));
    session.release(cursor).unwrap();
    assert!(session
        .start(
            Query::Children { parent: None },
            limits(),
            Cancellation::default()
        )
        .is_ok());
}

#[test]
fn completed_query_retries_until_release_and_summary_uses_same_dispatcher() {
    let (_dir, reader, signal) = fixture();
    let budget = Budget::new(32768);
    let mut session = Session::new(&reader, budget.clone(), 1).unwrap();
    let cursor = session
        .start(
            Query::Summary {
                signal,
                grid: Grid::new(0, 4, 1).unwrap(),
            },
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let mut next = cursor;
    let page = loop {
        let page = session.advance(next).unwrap();
        if let Some(cursor) = page.next {
            next = cursor;
        } else {
            break page;
        }
    };
    let Reply::Summary(summary) = &page.reply else {
        panic!("expected summary");
    };
    assert_eq!(summary.bins()[0].changes, 10);
    assert!(Arc::ptr_eq(&page, &session.advance(page.request).unwrap()));
    assert!(matches!(
        session.start(query(signal), limits(), Cancellation::default()),
        Err(Error::ResourceLimit)
    ));
    session.release(cursor).unwrap();
    drop(page);
    drop(session);
    assert_eq!(budget.used(), 0);
}

#[cfg(feature = "wire")]
#[test]
fn native_pages_cross_the_wire_without_changing_results_or_continuations() {
    use prost::Message;
    use vtr_query::{
        wire::{proto, MAX_DECODED_BYTES},
        wire_encode, wire_reply,
    };
    let (_dir, reader, signal) = fixture();
    let budget = Budget::new(MAX_DECODED_BYTES);
    let mut session = Session::new(&reader, budget.clone(), 1).unwrap();
    let queries = [
        query(signal),
        Query::Summary {
            signal,
            grid: Grid::new(0, 2, 3).unwrap(),
        },
        Query::Children { parent: None },
        Query::Search {
            scope: None,
            needle: "s".into(),
        },
        Query::Resolve {
            paths: vec![vtr_query::metadata::Path {
                segments: vec!["s".into()],
                occurrence: None,
                kind: None,
            }],
        },
    ];
    for query in queries {
        let first = session
            .start(
                query,
                Limits {
                    work: 1,
                    ..limits()
                },
                Cancellation::default(),
            )
            .unwrap();
        let mut cursor = first;
        let mut complete = false;
        for _ in 0..100 {
            let native = session.advance(cursor).unwrap();
            let bytes = wire_encode::delivery(1, &native, &budget).unwrap();
            let response = wire_reply::decode(bytes.bytes(), &budget).unwrap();
            let wire_reply::Body::Delivery(remote) = response.body() else {
                panic!("delivery")
            };
            assert_eq!(remote.request, native.request);
            assert_eq!(remote.next, native.next);
            assert_eq!(remote.reply.complete(), native.reply.complete());
            let again = wire_encode::delivery(1, remote, &budget).unwrap();
            assert_eq!(
                proto::Envelope::decode(bytes.bytes()).unwrap(),
                proto::Envelope::decode(again.bytes()).unwrap()
            );
            match remote.next {
                Some(next) => cursor = next,
                None => {
                    complete = true;
                    break;
                }
            }
        }
        assert!(complete, "query must exhaust under single-unit work slices");
        session.release(first).unwrap();
    }
    drop(session);
    assert_eq!(budget.used(), 0);
}

#[test]
fn panning_thirty_two_rows_keeps_admitted_histories_warm() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pan.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let signals: Vec<_> = (0..32)
        .map(|i| writer.add_bits(&format!("s{i}"), 1, 2).1)
        .collect();
    for time in 0..64 {
        writer.set_time(time).unwrap();
        for &signal in &signals {
            writer.emit_u64(signal, time % 2).unwrap();
        }
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(256 << 20);
    let mut session = Session::new(&reader, budget.clone(), 4).unwrap();
    for pan in 0..3 {
        for signal in &signals {
            let first = session
                .start(
                    Query::Summary {
                        signal: signal.0,
                        grid: Grid::new(pan * 8, 2, 8).unwrap(),
                    },
                    Limits {
                        bytes: 65536,
                        records: 8,
                        work: 8,
                    },
                    Cancellation::default(),
                )
                .unwrap();
            let mut cursor = first;
            loop {
                let page = session.advance(cursor).unwrap();
                let Reply::Summary(summary) = &page.reply else {
                    panic!("summary expected")
                };
                if pan > 0 {
                    assert!(
                        !summary.bins().is_empty(),
                        "pan {pan}, signal {} rebuilt an already admitted history",
                        signal.0
                    );
                }
                if let Some(next) = page.next {
                    cursor = next;
                } else {
                    break;
                }
            }
            session.release(first).unwrap();
        }
    }
    drop(session);
    assert_eq!(budget.used(), 0);
}
