#![cfg(not(target_family = "wasm"))]
use volna_core::{
    Scene, Theme,
    geometry::{Rect, point, size},
    scene::Prim,
    wave::{bounded::paint_exact, exact::ExactWindow},
};
use vtr_query::{
    Budget, Cancellation, Error, Interval, TimeBound,
    native_session::Session,
    session::Query,
    wave::{Limits, Sample, Value},
};
fn fixture(event: bool) -> (tempfile::TempDir, vtr::Reader, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exact.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_var(
        "s",
        if event {
            vtr::VarType::Event
        } else {
            vtr::VarType::Wire
        },
        vtr::Direction::Implicit,
        vtr::SignalKind::Bits {
            width: 1,
            states: 2,
        },
    );
    for (i, time) in [0, 5, 5, 5, 10, u64::MAX].into_iter().enumerate() {
        writer.set_time(time).unwrap();
        writer.emit_u64(signal, (i % 2) as u64).unwrap();
    }
    writer.close().unwrap();
    (dir, vtr::Reader::open(path).unwrap(), signal.0)
}
fn limits() -> Limits {
    Limits {
        bytes: 8192,
        records: 1,
        work: 256,
    }
}
fn range() -> Interval {
    Interval::new(0, TimeBound::Tick(20)).unwrap()
}
fn row() -> Rect {
    Rect::new(point(0.0, 0.0), size(100.0, 24.0))
}
#[test]
fn repeated_timestamp_pages_remain_unproven_until_their_boundary_is_complete() {
    let (_dir, reader, signal) = fixture(false);
    let budget = Budget::new(65536);
    let mut session = Session::new(&reader, budget.clone(), 2).unwrap();
    let mut cursor = session
        .start(
            Query::Window {
                signal,
                interval: range(),
            },
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let first = cursor;
    let mut window = ExactWindow::new(16, &budget).unwrap();
    loop {
        let delivery = session.advance(cursor).unwrap();
        assert!(window.append(delivery.clone()).unwrap());
        assert!(
            !window.append(delivery.clone()).unwrap(),
            "retry must not duplicate changes"
        );
        if !window.is_complete() {
            let boundary = window.changes().last().unwrap().time;
            assert_eq!(window.coverage().unwrap().end(), TimeBound::Tick(boundary));
            assert!(window.sample_at(boundary).is_none());
        }
        let Some(next) = delivery.next else {
            break;
        };
        cursor = next;
    }
    assert_eq!(
        window.changes().map(|c| c.time).collect::<Vec<_>>(),
        [0, 5, 5, 5, 10]
    );
    let Some(Sample::Known(Value::Bits { data, .. })) = window.sample_at(5) else {
        panic!("known bit");
    };
    assert_eq!(
        data.as_slice()[0] & 1,
        1,
        "last same-time change wins only after proof"
    );
    assert!(window.sample_at(20).is_none());
    session.release(first).unwrap();
    drop(session);
    drop(window);
    drop(data);
    assert_eq!(budget.used(), 0);
}
#[test]
fn refused_pages_preserve_the_prefix_and_foreign_chains_are_rejected() {
    let (_dir, reader, signal) = fixture(false);
    let budget = Budget::new(65536);
    let mut session = Session::new(&reader, budget.clone(), 2).unwrap();
    let cursor = session
        .start(
            Query::Window {
                signal,
                interval: range(),
            },
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let first = session.advance(cursor).unwrap();
    let mut window = ExactWindow::new(1, &budget).unwrap();
    window.append(first.clone()).unwrap();
    let next = first.next.unwrap();
    let second = session.advance(next).unwrap();
    assert_eq!(window.append(second).unwrap_err(), Error::ResourceLimit);
    assert_eq!(window.next(), Some(next));
    assert_eq!(window.changes().count(), 1);
    let foreign = session
        .start(
            Query::Window {
                signal,
                interval: range(),
            },
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    assert!(matches!(
        window.append(session.advance(foreign).unwrap()),
        Err(Error::Invalid(_))
    ));
    assert_eq!(window.next(), Some(next));
}
#[test]
fn exact_paint_stops_before_the_incomplete_timestamp() {
    let (_dir, reader, signal) = fixture(false);
    let budget = Budget::new(65536);
    let mut session = Session::new(&reader, budget.clone(), 1).unwrap();
    let cursor = session
        .start(
            Query::Window {
                signal,
                interval: range(),
            },
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let mut window = ExactWindow::new(16, &budget).unwrap();
    let page = session.advance(cursor).unwrap();
    let next = page.next.unwrap();
    window.append(page).unwrap();
    window.append(session.advance(next).unwrap()).unwrap();
    assert_eq!(window.coverage().unwrap().end(), TimeBound::Tick(5));
    let mut scene = Scene::default();
    paint_exact(&window, range(), row(), &Theme::one_dark(), &mut scene);
    assert!(scene.prims.len() > 2);
    for prim in &scene.prims {
        match prim {
            Prim::Lines { segments, .. } => assert!(segments.iter().flatten().all(|p| p.x <= 25.0)),
            Prim::Quad { rect, .. } => assert!(rect.right() <= 25.0),
            _ => {}
        }
    }
}
#[test]
fn events_do_not_gain_held_values_and_maximum_time_is_renderable() {
    let (_dir, reader, signal) = fixture(true);
    let budget = Budget::new(65536);
    let mut session = Session::new(&reader, budget.clone(), 1).unwrap();
    let interval = Interval::new(0, TimeBound::AfterMax).unwrap();
    let mut cursor = session
        .start(
            Query::Window { signal, interval },
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let mut window = ExactWindow::new(16, &budget).unwrap();
    loop {
        let page = session.advance(cursor).unwrap();
        let next = page.next;
        window.append(page).unwrap();
        let Some(next) = next else {
            break;
        };
        cursor = next;
    }
    assert!(window.sample_at(5).is_none());
    assert_eq!(window.changes().count(), 6);
    let mut scene = Scene::default();
    let theme = Theme::one_dark();
    paint_exact(&window, range(), row(), &theme, &mut scene);
    assert!(scene.prims.iter().any(|p| matches!(p, Prim::Lines { color, segments, .. } if *color == theme.wave_event_coalesced && segments[0][0].x == 25.0)));
    scene.clear();
    paint_exact(
        &window,
        Interval::new(u64::MAX - 1, TimeBound::AfterMax).unwrap(),
        row(),
        &theme,
        &mut scene,
    );
    assert!(matches!(&scene.prims[1], Prim::Lines { segments, .. } if segments[0][0].x == 50.0));
}
