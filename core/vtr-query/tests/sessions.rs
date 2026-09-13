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
    let page = session.advance(cursor).unwrap();
    assert!(page.next.is_none());
    let Reply::Summary(summary) = &page.reply else {
        panic!("expected summary");
    };
    assert_eq!(summary.bins()[0].changes, 10);
    assert!(Arc::ptr_eq(&page, &session.advance(cursor).unwrap()));
    assert!(matches!(
        session.start(query(signal), limits(), Cancellation::default()),
        Err(Error::ResourceLimit)
    ));
    session.release(cursor).unwrap();
    drop(page);
    drop(session);
    assert_eq!(budget.used(), 0);
}
