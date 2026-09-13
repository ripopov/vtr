#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};
use vtr_query::{
    local_session::LocalSession,
    session::{Query, Reply},
    wave::Limits,
    Budget, Error, Interval, TimeBound,
};
fn wait<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(futures_lite::future::block_on(future));
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("worker/future must make progress")
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_bits("s", 1, 2);
    for i in 0..100 {
        writer.set_time(i).unwrap();
        writer.emit_u64(signal, i % 2).unwrap();
    }
    writer.close().unwrap();
    (dir, path, signal.0)
}
fn query(signal: u32) -> Query {
    Query::Window {
        signal,
        interval: Interval::new(0, TimeBound::AfterMax).unwrap(),
    }
}
fn limits() -> Limits {
    Limits {
        bytes: 8192,
        records: 1,
        work: 256,
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "worker must retire abandoned state"
        );
        std::thread::yield_now();
    }
}
#[test]
fn asynchronous_pages_share_native_deliveries_and_use_reclaimed_credit_immediately() {
    let (_dir, path, signal) = fixture();
    let budget = Budget::new(1 << 20);
    let session = wait(LocalSession::open(path, budget.clone(), 1).unwrap()).unwrap();
    let mut page = wait(session.execute(query(signal), limits()).unwrap()).unwrap();
    let first = page.request;
    let retry = wait(session.advance(first).unwrap()).unwrap();
    assert!(
        Arc::ptr_eq(&page, &retry),
        "local delivery must not encode or copy pages"
    );
    drop(retry);
    let mut times = vec![];
    loop {
        let Reply::Window(window) = &page.reply else {
            panic!("window")
        };
        times.extend(window.changes().iter().map(|c| c.time));
        let Some(next) = page.next else { break };
        page = wait(session.advance(next).unwrap()).unwrap();
    }
    assert_eq!(times, (0..100).collect::<Vec<_>>());
    session.release(first).unwrap();
    session.close();
    until(|| session.is_stopped());
    drop(session);
    assert!(
        budget.used() > 0,
        "pinned delivery remains admitted after shutdown"
    );
    drop(page);
    until(|| budget.used() == 0);
}
#[test]
fn delivery_credit_is_bounded_until_consumed_and_dropped_futures_release_slots() {
    let (_dir, path, signal) = fixture();
    let budget = Budget::new(1 << 20);
    let session = wait(LocalSession::open(path, budget.clone(), 1).unwrap()).unwrap();
    let pending = session.execute(query(signal), limits()).unwrap();
    assert!(matches!(
        session.execute(query(signal), limits()),
        Err(Error::ResourceLimit)
    ));
    drop(pending);
    let mut next = None;
    until(|| match session.execute(query(signal), limits()) {
        Ok(f) => {
            next = Some(f);
            true
        }
        Err(Error::ResourceLimit) => false,
        Err(e) => panic!("{e}"),
    });
    let page = wait(next.unwrap()).unwrap();
    assert_eq!(page.request.step, 0);
    // Cancel a queued/active continuation; cancellation must also free its
    // operation when the receiver consumes the error instead of being dropped.
    let future = session.advance(page.next.unwrap()).unwrap();
    let cancel = future.cancellation();
    cancel.cancel();
    match wait(future) {
        Err(Error::Cancelled) => (),
        Ok(reply) => {
            session.release(reply.request).unwrap();
        }
        Err(e) => panic!("{e}"),
    }
    drop(cancel);
    drop(page);
    let replacement = wait(session.execute(query(signal), limits()).unwrap()).unwrap();
    session.release(replacement.request).unwrap();
    drop(replacement);
    drop(session);
    until(|| budget.used() == 0);
}
#[test]
fn cancelling_completed_request_does_not_cancel_later_pages() {
    let (_dir, path, signal) = fixture();
    let budget = Budget::new(1 << 20);
    let session = wait(LocalSession::open(path, budget.clone(), 1).unwrap()).unwrap();
    let future = session.execute(query(signal), limits()).unwrap();
    let old_cancel = future.cancellation();
    let first = wait(future).unwrap();
    old_cancel.cancel();
    let next = wait(session.advance(first.next.unwrap()).unwrap()).unwrap();
    session.release(next.request).unwrap();
    drop(first);
    drop(next);
    session.close();
    until(|| session.is_stopped());
    drop(session);
    assert!(
        budget.used() > 0,
        "retained cancellation state remains admitted"
    );
    drop(old_cancel);
    until(|| budget.used() == 0);
}
#[test]
fn open_failure_close_and_foreign_snapshots_are_reported() {
    let budget = Budget::new(1 << 20);
    let missing =
        LocalSession::open("/definitely/missing/trace.vtr".into(), budget.clone(), 1).unwrap();
    assert!(matches!(wait(missing), Err(Error::Backend(_))));
    until(|| budget.used() == 0);
    let (_dir, path, signal) = fixture();
    let session = wait(LocalSession::open(path, budget.clone(), 2).unwrap()).unwrap();
    let pending = session.execute(query(signal), limits()).unwrap();
    session.close();
    let _ = wait(pending);
    until(|| session.is_stopped());
    assert!(matches!(
        session.execute(query(signal), limits()),
        Err(Error::Closed)
    ));
    let foreign = vtr_query::session::Continuation {
        snapshot: vtr_query::session::SnapshotId([0; 16]),
        operation: 1,
        step: 0,
    };
    assert!(matches!(session.advance(foreign), Err(Error::Invalid(_))));
    drop(session);
    until(|| budget.used() == 0);
}
#[test]
fn decoded_delivery_waiting_in_an_unpolled_future_is_released_on_drop() {
    let (_dir, path, signal) = fixture();
    let budget = Budget::new(1 << 20);
    let session = wait(LocalSession::open(path, budget.clone(), 2).unwrap()).unwrap();
    let first = session.execute(query(signal), limits()).unwrap();
    // FIFO execution makes the second reply a barrier proving that the first
    // reply has been produced, although the first receiver has never polled.
    let second = wait(session.execute(query(signal), limits()).unwrap()).unwrap();
    drop(first);
    session.release(second.request).unwrap();
    drop(second);
    let replacement = wait(session.execute(query(signal), limits()).unwrap()).unwrap();
    session.release(replacement.request).unwrap();
    drop(replacement);
    drop(session);
    until(|| budget.used() == 0);
}
