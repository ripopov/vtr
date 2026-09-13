#![cfg(not(target_family = "wasm"))]
use std::{
    cell::{Cell, RefCell},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll},
};
use volna_core::{
    Scene, Theme,
    geometry::{Rect, point, size},
    wave::demand::{Demand, Progress, WaveDemands},
};
use vtr_query::{
    Budget, Grid, Result,
    local_session::{LocalSession, QueryFuture},
    session::{AsyncSession, Continuation, Delivery, Query, SessionInfo},
    wave::Limits,
};

struct GateSession {
    local: LocalSession,
    hold: Rc<Cell<bool>>,
    starts: Rc<RefCell<Vec<Grid>>>,
    exact_starts: Rc<Cell<usize>>,
}
struct GateTask {
    inner: QueryFuture,
    hold: Rc<Cell<bool>>,
}
impl Future for GateTask {
    type Output = Result<Arc<Delivery>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.hold.get() {
            Poll::Pending
        } else {
            Pin::new(&mut self.inner).poll(cx)
        }
    }
}
impl AsyncSession for GateSession {
    type Task = GateTask;
    fn info(&self) -> &SessionInfo {
        self.local.info()
    }
    fn execute(&self, query: Query, limits: Limits) -> Result<GateTask> {
        if let Query::Summary { grid, .. } = &query {
            self.starts.borrow_mut().push(*grid);
        } else if matches!(query, Query::Window { .. }) {
            self.exact_starts.set(self.exact_starts.get() + 1);
        }
        Ok(GateTask {
            inner: self.local.execute(query, limits)?,
            hold: self.hold.clone(),
        })
    }
    fn advance(&self, cursor: Continuation) -> Result<GateTask> {
        Ok(GateTask {
            inner: self.local.advance(cursor)?,
            hold: self.hold.clone(),
        })
    }
    fn cancel(&self, task: &GateTask) {
        task.inner.cancellation().cancel();
    }
    fn release(&self, cursor: Continuation) -> Result<()> {
        self.local.release(cursor)
    }
    fn close(&self) {
        self.local.close();
    }
}
fn fixture() -> (tempfile::TempDir, GateSession, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demand.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_bits("clock", 1, 2);
    for time in 0..128 {
        writer.set_time(time).unwrap();
        writer.emit_u64(signal, time % 2).unwrap();
    }
    writer.close().unwrap();
    let local =
        futures_lite::future::block_on(LocalSession::open(path, Budget::new(16 << 20), 4).unwrap())
            .unwrap();
    (
        dir,
        GateSession {
            local,
            hold: Rc::new(Cell::new(false)),
            starts: Rc::new(RefCell::new(Vec::new())),
            exact_starts: Rc::new(Cell::new(0)),
        },
        signal.0,
    )
}
fn drain<S: AsyncSession>(demand: &mut WaveDemands<S>) {
    futures_lite::future::block_on(async {
        for _ in 0..10000 {
            let progress = futures_lite::future::poll_fn(|cx| demand.poll(cx)).await;
            match progress {
                Progress::Idle => return,
                Progress::Advanced | Progress::Backpressure => {
                    futures_lite::future::yield_now().await
                }
            }
        }
        panic!("summary demand failed to become idle");
    });
}
fn limits() -> Limits {
    Limits {
        bytes: 16384,
        records: 1,
        work: 16,
    }
}
#[test]
fn native_pages_flow_through_core_demand_into_the_display_list() {
    let (_dir, session, signal) = fixture();
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &Budget::new(8192)).unwrap();
    let grid = Grid::new(0, 4, 8).unwrap();
    loader
        .set(0, Some(Demand::Summary { signal, grid }))
        .unwrap();
    drain(&mut loader);
    assert!(
        loader.slot(0).unwrap().is_complete(),
        "{:?}",
        loader.slot(0).unwrap().error()
    );
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    assert_eq!(
        loader
            .slot(0)
            .unwrap()
            .bins()
            .iter()
            .map(|b| b.changes)
            .sum::<u64>(),
        128
    );
    assert_eq!(loader.slot(0).unwrap().value_at(1, 16).unwrap(), None);
    assert_eq!(
        loader.slot(0).unwrap().value_at(15, 16).unwrap(),
        Some(volna_core::data::WaveValue::Bits("1".into()))
    );
    let mut scene = Scene::default();
    loader.slot(0).unwrap().paint(
        grid.interval(),
        Rect::new(point(0.0, 0.0), size(800.0, 24.0)),
        &Theme::one_dark(),
        &mut scene,
    );
    assert_eq!(
        scene.prims.len(),
        10,
        "eight dense bins plus clip boundaries"
    );
}
#[test]
fn obsolete_task_is_drained_and_only_the_latest_pending_viewport_runs() {
    let (_dir, session, signal) = fixture();
    let hold = session.hold.clone();
    let starts = session.starts.clone();
    hold.set(true);
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &Budget::new(8192)).unwrap();
    let first = Grid::new(0, 4, 8).unwrap();
    let middle = Grid::new(16, 3, 8).unwrap();
    let latest = Grid::new(64, 2, 8).unwrap();
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal,
                grid: first,
            }),
        )
        .unwrap();
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    assert!(loader.poll(&mut cx).is_pending());
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal,
                grid: middle,
            }),
        )
        .unwrap();
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal,
                grid: latest,
            }),
        )
        .unwrap();
    assert!(loader.slot(0).unwrap().bins().is_empty());
    hold.set(false);
    drain(&mut loader);
    assert!(
        loader.slot(0).unwrap().is_complete(),
        "{:?}",
        loader.slot(0).unwrap().error()
    );
    assert_eq!(
        loader.slot(0).unwrap().displayed(),
        Some(Demand::Summary {
            signal,
            grid: latest
        })
    );
    assert_eq!(&*starts.borrow(), &[first, latest]);
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
}
#[test]
fn clearing_releases_bins_and_repeated_navigation_reuses_operation_capacity() {
    let (_dir, session, signal) = fixture();
    let budget = Budget::new(8192);
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &budget).unwrap();
    for start in (0..20).map(|i| (i % 4) * 16) {
        loader
            .set(
                0,
                Some(Demand::Summary {
                    signal,
                    grid: Grid::new(start, 2, 8).unwrap(),
                }),
            )
            .unwrap();
        drain(&mut loader);
        assert!(
            loader.slot(0).unwrap().is_complete(),
            "{:?}",
            loader.slot(0).unwrap().error()
        );
        assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    }
    loader.set(0, None).unwrap();
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().bins().is_empty());
    assert_eq!(loader.slot(0).unwrap().displayed(), None);
    drop(loader);
    assert_eq!(budget.used(), 0);
}

#[test]
fn failed_demand_can_be_replaced_and_retry_does_not_append_old_pages() {
    let (_dir, session, signal) = fixture();
    let starts = session.starts.clone();
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &Budget::new(8192)).unwrap();
    let grid = Grid::new(0, 4, 8).unwrap();
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal: u32::MAX,
                grid,
            }),
        )
        .unwrap();
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().error().is_some());
    assert!(!loader.slot(0).unwrap().is_complete());
    loader.retry(0).unwrap();
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().error().is_some());
    assert_eq!(starts.borrow().len(), 2);
    loader
        .set(0, Some(Demand::Summary { signal, grid }))
        .unwrap();
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().is_complete());
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    // Reinstalling the same intent is a no-op, including after full completion.
    loader
        .set(0, Some(Demand::Summary { signal, grid }))
        .unwrap();
    drain(&mut loader);
    assert_eq!(starts.borrow().len(), 3);
}

#[test]
fn rows_share_one_session_with_a_fixed_operation_ceiling() {
    let (_dir, session, signal) = fixture();
    let hold = session.hold.clone();
    let starts = session.starts.clone();
    hold.set(true);
    let mut loader = WaveDemands::new(session, limits(), 16, 8, 3, &Budget::new(65536)).unwrap();
    for slot in 0..8 {
        loader
            .set(
                slot,
                Some(Demand::Summary {
                    signal,
                    grid: Grid::new(slot as u64 * 8, 2, 8).unwrap(),
                }),
            )
            .unwrap();
    }
    let mut cx = Context::from_waker(std::task::Waker::noop());
    assert!(loader.poll(&mut cx).is_pending());
    assert_eq!(starts.borrow().len(), 3);
    assert_eq!(loader.active_operations(), 3);
    hold.set(false);
    futures_lite::future::block_on(async {
        loop {
            let progress = futures_lite::future::poll_fn(|cx| loader.poll(cx)).await;
            assert!(loader.active_operations() <= 3);
            if progress == Progress::Idle {
                break;
            }
            futures_lite::future::yield_now().await;
        }
    });
    assert_eq!(starts.borrow().len(), 8);
    assert_eq!(loader.active_operations(), 0);
    for slot in 0..8 {
        assert!(
            loader.slot(slot).unwrap().is_complete(),
            "slot {slot}: {:?}",
            loader.slot(slot).unwrap().error()
        );
        assert_eq!(loader.slot(slot).unwrap().bins().len(), 8);
    }
}

#[test]
fn changing_signal_discards_old_bins_while_pan_keeps_compatible_coverage() {
    let (_dir, session, signal) = fixture();
    let mut loader = WaveDemands::new(session, limits(), 16, 2, 2, &Budget::new(65536)).unwrap();
    let grid = Grid::new(0, 4, 8).unwrap();
    loader
        .set(0, Some(Demand::Summary { signal, grid }))
        .unwrap();
    drain(&mut loader);
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal,
                grid: Grid::new(16, 3, 8).unwrap(),
            }),
        )
        .unwrap();
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    assert_eq!(
        loader.slot(0).unwrap().displayed().unwrap().grid().unwrap(),
        grid
    );
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal: signal + 1,
                grid,
            }),
        )
        .unwrap();
    assert!(loader.slot(0).unwrap().bins().is_empty());
    assert_eq!(loader.slot(0).unwrap().displayed(), None);
    assert!(
        loader
            .set(2, Some(Demand::Summary { signal, grid }))
            .is_err()
    );
    assert!(
        loader
            .set(
                1,
                Some(Demand::Summary {
                    signal,
                    grid: Grid::new(0, 0, 17).unwrap()
                })
            )
            .is_err()
    );
}

#[test]
fn identical_panel_demands_share_work_and_immutable_bins() {
    let (_dir, session, signal) = fixture();
    let hold = session.hold.clone();
    let starts = session.starts.clone();
    hold.set(true);
    let mut loader = WaveDemands::new(session, limits(), 16, 3, 3, &Budget::new(65536)).unwrap();
    let demand = Some(Demand::Summary {
        signal,
        grid: Grid::new(0, 4, 8).unwrap(),
    });
    loader.set(0, demand).unwrap();
    loader.set(1, demand).unwrap();
    let mut cx = Context::from_waker(std::task::Waker::noop());
    assert!(loader.poll(&mut cx).is_pending());
    assert_eq!(starts.borrow().len(), 1);
    hold.set(false);
    let mut shared_partial = false;
    futures_lite::future::block_on(async {
        loop {
            let progress = futures_lite::future::poll_fn(|cx| loader.poll(cx)).await;
            let follower = loader.slot(1).unwrap();
            shared_partial |= !follower.bins().is_empty() && !follower.is_complete();
            if progress == Progress::Idle {
                break;
            }
            futures_lite::future::yield_now().await;
        }
    });
    assert!(
        shared_partial,
        "aliases receive useful bins before the full grid completes"
    );
    assert_eq!(starts.borrow().len(), 1);
    for (a, b) in loader
        .slot(0)
        .unwrap()
        .bins()
        .iter()
        .zip(loader.slot(1).unwrap().bins())
    {
        assert!(Arc::ptr_eq(a, b));
    }
    // Removing the original consumer preserves the other consumer's data and
    // leaves the document session available to a later panel.
    loader.set(0, None).unwrap();
    loader.set(2, demand).unwrap();
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().bins().is_empty());
    assert!(loader.slot(1).unwrap().is_complete());
    assert!(loader.slot(2).unwrap().is_complete());
    assert_eq!(starts.borrow().len(), 1);
}

#[test]
fn empty_work_slices_preserve_old_coverage_until_a_useful_bin_arrives() {
    let (_dir, session, signal) = fixture();
    let mut small_work = limits();
    small_work.work = 1;
    let mut loader = WaveDemands::new(session, small_work, 16, 1, 1, &Budget::new(65536)).unwrap();
    let old = Grid::new(0, 4, 8).unwrap();
    loader
        .set(0, Some(Demand::Summary { signal, grid: old }))
        .unwrap();
    drain(&mut loader);
    let new = Grid::new(16, 4, 4).unwrap();
    loader
        .set(0, Some(Demand::Summary { signal, grid: new }))
        .unwrap();
    assert_eq!(
        futures_lite::future::block_on(futures_lite::future::poll_fn(|cx| loader.poll(cx))),
        Progress::Advanced
    );
    assert_eq!(
        loader.slot(0).unwrap().displayed().unwrap().grid().unwrap(),
        old
    );
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    assert!(!loader.slot(0).unwrap().is_complete());
    drain(&mut loader);
    assert_eq!(
        loader.slot(0).unwrap().displayed().unwrap().grid().unwrap(),
        new
    );
    assert_eq!(loader.slot(0).unwrap().bins().len(), 4);
    assert!(loader.slot(0).unwrap().is_complete());
}

fn interval(start: u64, end: u64) -> vtr_query::Interval {
    vtr_query::Interval::new(start, vtr_query::TimeBound::Tick(end)).unwrap()
}
#[test]
fn exact_and_summary_rows_share_the_session_and_completed_exact_data() {
    let (_dir, session, signal) = fixture();
    let hold = session.hold.clone();
    let exact_starts = session.exact_starts.clone();
    hold.set(true);
    let budget = Budget::new(65536);
    let mut loader = WaveDemands::new(session, limits(), 16, 3, 2, &budget).unwrap();
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal,
                grid: Grid::new(0, 4, 8).unwrap(),
            }),
        )
        .unwrap();
    let exact = Some(Demand::Window {
        signal,
        interval: interval(0, 8),
    });
    loader.set(1, exact).unwrap();
    loader.set(2, exact).unwrap();
    assert!(
        loader
            .poll(&mut Context::from_waker(std::task::Waker::noop()))
            .is_pending()
    );
    assert_eq!(loader.active_operations(), 2);
    assert_eq!(exact_starts.get(), 1);
    hold.set(false);
    drain(&mut loader);
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    let a = loader.slot(1).unwrap().exact().unwrap();
    let b = loader.slot(2).unwrap().exact().unwrap();
    assert!(
        std::ptr::eq(a, b),
        "completed exact pages share their owner"
    );
    assert_eq!(a.changes().count(), 8);
    assert_eq!(
        loader.slot(1).unwrap().value_at(7, 1).unwrap(),
        Some(volna_core::data::WaveValue::Bits("1".into()))
    );
    assert_eq!(
        loader.slot(1).unwrap().value_at(7, 0).unwrap_err(),
        vtr_query::Error::ResourceLimit
    );
    assert_eq!(loader.slot(1).unwrap().value_at(8, 0).unwrap(), None);
    assert!(a.sample_at(7).is_some());
    assert!(a.sample_at(8).is_none());
    assert_eq!(loader.active_operations(), 0);
    drop(loader);
    assert_eq!(budget.used(), 0);
}
#[test]
fn representation_switches_and_dense_exact_retries_do_not_exceed_the_record_cap() {
    let (_dir, session, signal) = fixture();
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &Budget::new(65536)).unwrap();
    let exact = Demand::Window {
        signal,
        interval: interval(0, 8),
    };
    let summary = Demand::Summary {
        signal,
        grid: Grid::new(0, 4, 8).unwrap(),
    };
    loader.set(0, Some(exact)).unwrap();
    drain(&mut loader);
    loader.set(0, Some(summary)).unwrap();
    assert_eq!(loader.slot(0).unwrap().displayed(), Some(exact));
    assert!(loader.slot(0).unwrap().exact().is_some());
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().exact().is_none());
    assert_eq!(loader.slot(0).unwrap().bins().len(), 8);
    loader
        .set(
            0,
            Some(Demand::Window {
                signal,
                interval: interval(0, 128),
            }),
        )
        .unwrap();
    drain(&mut loader);
    assert_eq!(
        loader.slot(0).unwrap().error(),
        Some(&vtr_query::Error::ResourceLimit)
    );
    assert_eq!(
        loader.slot(0).unwrap().exact().unwrap().changes().count(),
        16
    );
    assert_eq!(loader.active_operations(), 0);
    loader.retry(0).unwrap();
    drain(&mut loader);
    assert_eq!(
        loader.slot(0).unwrap().exact().unwrap().changes().count(),
        16
    );
    loader.set(0, Some(summary)).unwrap();
    drain(&mut loader);
    assert!(loader.slot(0).unwrap().is_complete());
    assert!(loader.slot(0).unwrap().exact().is_none());
}
#[test]
fn exact_pan_cancels_the_old_task_without_losing_the_latest_pending_intent() {
    let (_dir, session, signal) = fixture();
    let hold = session.hold.clone();
    let starts = session.exact_starts.clone();
    hold.set(true);
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &Budget::new(65536)).unwrap();
    loader
        .set(
            0,
            Some(Demand::Window {
                signal,
                interval: interval(0, 8),
            }),
        )
        .unwrap();
    assert!(
        loader
            .poll(&mut Context::from_waker(std::task::Waker::noop()))
            .is_pending()
    );
    loader
        .set(
            0,
            Some(Demand::Window {
                signal,
                interval: interval(16, 24),
            }),
        )
        .unwrap();
    let latest = Demand::Window {
        signal,
        interval: interval(64, 72),
    };
    loader.set(0, Some(latest)).unwrap();
    hold.set(false);
    drain(&mut loader);
    assert_eq!(starts.get(), 2);
    let row = loader.slot(0).unwrap();
    assert!(row.is_complete(), "{:?}", row.error());
    assert_eq!(row.displayed(), Some(latest));
    assert_eq!(
        row.exact()
            .unwrap()
            .changes()
            .map(|c| c.time)
            .collect::<Vec<_>>(),
        (64..72).collect::<Vec<_>>()
    );
}

#[test]
fn query_snapshots_render_through_the_existing_row_painter_and_reject_stale_documents() {
    use volna_core::{
        Document,
        scene::{MonoMeasure, Prim, TextCache},
        wave::{WaveModel, paint},
    };
    let (dir, session, signal) = fixture();
    let budget = Budget::new(65536);
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &budget).unwrap();
    loader
        .set(
            0,
            Some(Demand::Summary {
                signal,
                grid: Grid::new(0, 4, 8).unwrap(),
            }),
        )
        .unwrap();
    drain(&mut loader);
    let data = loader.snapshot(0).unwrap().unwrap();
    let mut doc = Document::new();
    doc.set_session(Arc::new(
        volna_core::session::LocalSession::open(&dir.path().join("demand.vtr")).unwrap(),
    ));
    let generation = doc.generation();
    assert!(doc.bind_query_session(generation, data.snapshot()));
    let mut model = WaveModel::new();
    model.add_vars(&mut doc, &[0]);
    assert!(
        doc.take_requests().is_empty(),
        "query-bound rows must not request full histories"
    );
    assert!(model.install_query(&doc, generation, 0, data.clone()));
    assert!(model.items[0].history.is_none());
    model.finish_signal(
        volna_core::data::SignalRef(signal),
        Err(anyhow::anyhow!("late legacy load")),
    );
    assert!(model.items[0].error.is_none());
    doc.shared.cursor = Some(15);
    let theme = Theme::one_dark();
    let layout = model
        .layout(
            Rect::new(point(0.0, 0.0), size(1000.0, 200.0)),
            &doc,
            &theme,
        )
        .clone();
    let mut scene = Scene::default();
    paint::paint(
        &model,
        &doc,
        &theme,
        &mut TextCache::default(),
        &mut MonoMeasure,
        &mut scene,
        true,
    );
    assert!(
        scene
            .prims
            .iter()
            .any(|p| matches!(p, Prim::Quad { fill, .. } if *fill == theme.wave_dense))
    );
    assert!(scene.prims.iter().any(|p| matches!(p, Prim::Text { text, origin, .. } if text == "1" && origin.x >= layout.values.left() && origin.x < layout.values.right())));
    assert!(!model.install_query(&doc, generation + 1, 0, data.clone()));
    assert!(!doc.bind_query_session(generation, vtr_query::session::SnapshotId([255; 16])));
    doc.close();
    assert!(!model.install_query(&doc, generation, 0, data.clone()));
    drop(loader);
    assert!(
        budget.used() > 0,
        "installed immutable snapshots retain their admission"
    );
    drop(data);
    drop(model);
    assert_eq!(budget.used(), 0);
}

#[test]
fn installed_exact_snapshots_do_not_change_when_more_pages_arrive() {
    let (_dir, session, signal) = fixture();
    let budget = Budget::new(65536);
    let mut loader = WaveDemands::new(session, limits(), 16, 1, 1, &budget).unwrap();
    loader
        .set(
            0,
            Some(Demand::Window {
                signal,
                interval: interval(0, 8),
            }),
        )
        .unwrap();
    while loader.slot(0).unwrap().exact().is_none() {
        futures_lite::future::block_on(futures_lite::future::poll_fn(|cx| loader.poll(cx)));
    }
    let first = loader.snapshot(0).unwrap().unwrap();
    assert!(first.sample_at(7).is_none());
    drain(&mut loader);
    let complete = loader.snapshot(0).unwrap().unwrap();
    assert!(complete.sample_at(7).is_some());
    assert!(
        first.sample_at(7).is_none(),
        "row snapshots must not observe later mutation"
    );
    drop(loader);
    drop(first);
    drop(complete);
    assert_eq!(budget.used(), 0);
}

#[test]
fn integer_projection_preserves_fractional_pan_and_negative_overscroll_geometry() {
    use volna_core::wave::Viewport;
    let row = Rect::new(point(100.0, 0.0), size(100.0, 20.0));
    let (coverage, projected) = Viewport {
        start: 0.5,
        end: 10.5,
    }
    .integer_projection(row)
    .unwrap();
    assert_eq!(coverage, interval(0, 11));
    assert_eq!(projected.left(), 95.0);
    assert_eq!(projected.width(), 110.0);
    let (coverage, projected) = Viewport {
        start: -5.0,
        end: 5.0,
    }
    .integer_projection(row)
    .unwrap();
    assert_eq!(coverage, interval(0, 5));
    assert_eq!(projected.left(), 150.0);
    assert_eq!(projected.width(), 50.0);
    assert!(
        Viewport {
            start: 0.0,
            end: f64::INFINITY
        }
        .integer_projection(row)
        .is_none()
    );
    assert!(
        Viewport {
            start: -10.0,
            end: -1.0
        }
        .integer_projection(row)
        .is_none()
    );
}
