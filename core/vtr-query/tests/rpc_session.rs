#![cfg(feature = "wire")]
use prost::Message;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use vtr_query::{
    rpc_session::{Incarnation, RpcDriver, RpcSession},
    session::{Query, SessionInfo, SnapshotId},
    wave::Limits,
    wire::{self, proto as p},
    wire_encode,
    wire_request::{self, RequestBody},
    Budget, Error, Interval, TimeBound,
};
const HOST: Incarnation = Incarnation([1; 16]);
const SNAP: SnapshotId = SnapshotId([7; 16]);
fn poll<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
}
fn ready<F: Future + Unpin>(mut future: F) -> F::Output {
    match poll(&mut future) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("future not ready"),
    }
}
fn connected() -> (RpcDriver, RpcSession, Budget) {
    let budget = Budget::new(64 << 20);
    let (mut driver, open) = RpcDriver::new(HOST, budget.clone()).unwrap();
    let wire_budget = Budget::new(1 << 20);
    let hello = driver.take_outbound().unwrap().unwrap();
    assert!(matches!(
        wire_request::decode(hello.bytes(), &wire_budget)
            .unwrap()
            .body,
        RequestBody::Hello
    ));
    let welcome = wire_encode::welcome(hello.request_id, &wire_budget).unwrap();
    driver.receive(HOST, welcome.bytes()).unwrap();
    let request = driver.take_outbound().unwrap().unwrap();
    assert!(matches!(
        wire_request::decode(request.bytes(), &wire_budget)
            .unwrap()
            .body,
        RequestBody::Open
    ));
    let info = SessionInfo {
        snapshot: SNAP,
        timescale: -9,
        time_range: Some(range()),
        signals: 10,
        declarations: 100,
    };
    let opened = wire_encode::opened(request.request_id, &info, &wire_budget).unwrap();
    driver.receive(HOST, opened.bytes()).unwrap();
    let session = ready(open).unwrap();
    assert_eq!(session.info().snapshot, SNAP);
    (driver, session, budget)
}
fn range() -> Interval {
    Interval::new(0, TimeBound::Tick(100)).unwrap()
}
fn query() -> Query {
    Query::Window {
        signal: 1,
        interval: range(),
    }
}
fn limits() -> Limits {
    Limits {
        bytes: 8192,
        records: 2,
        work: 32,
    }
}
fn cursor(operation: u64, step: u64) -> p::Continuation {
    p::Continuation {
        snapshot: SNAP.0.to_vec(),
        operation,
        step,
    }
}
fn window(
    id: u64,
    operation: u64,
    step: u64,
    times: &[u64],
    complete: bool,
    predecessor: u64,
) -> Vec<u8> {
    p::Envelope {
        version: 1,
        request_id: id,
        snapshot: SNAP.0.to_vec(),
        body: Some(p::envelope::Body::Reply(p::Delivery {
            request: Some(cursor(operation, step)),
            next: (!complete).then(|| cursor(operation, step + 1)),
            page: Some(p::delivery::Page::Window(p::WindowPage {
                interval: Some(p::Interval {
                    start: 0,
                    end: Some(p::Bound {
                        value: Some(p::bound::Value::Tick(100)),
                    }),
                }),
                predecessor: Some(p::Sample {
                    value: Some(p::sample::Value::Known(p::Value {
                        value: Some(p::value::Value::Real(predecessor)),
                    })),
                }),
                changes: times
                    .iter()
                    .map(|time| p::Change {
                        time: *time,
                        value: Some(p::Value {
                            value: Some(p::value::Value::Real(0)),
                        }),
                    })
                    .collect(),
                complete,
            })),
        })),
    }
    .encode_to_vec()
}
fn ack(driver: &mut RpcDriver, packet: &vtr_query::rpc_session::Outbound) {
    let b = Budget::new(1 << 20);
    let bytes = wire_encode::acknowledged(packet.request_id, Some(SNAP), &b).unwrap();
    driver.receive(HOST, bytes.bytes()).unwrap();
}
#[test]
fn admission_precedes_sending_and_unconsumed_futures_keep_delivery_credit() {
    let (mut driver, session, budget) = connected();
    let baseline = budget.used();
    let futures: Vec<_> = (0..4)
        .map(|_| session.execute(query(), limits()).unwrap())
        .collect();
    assert!(budget.used() >= baseline + 4 * (wire::MAX_DECODED_BYTES + wire::MAX_REPLY_BYTES));
    assert!(matches!(
        session.execute(query(), limits()),
        Err(Error::ResourceLimit)
    ));
    drop(futures);
    assert!(driver.take_outbound().unwrap().is_none());
    assert_eq!(budget.used(), baseline);
    let future = session.execute(query(), limits()).unwrap();
    let packet = driver.take_outbound().unwrap().unwrap();
    driver
        .receive(HOST, &window(packet.request_id, 1, 0, &[1], true, 0))
        .unwrap();
    drop(packet);
    assert!(
        budget.used() < baseline + wire::MAX_DECODED_BYTES,
        "unused prepaid capacity is returned after bounded decode"
    );
    let page = ready(future).unwrap();
    session.release(page.request).unwrap();
    let release = driver.take_outbound().unwrap().unwrap();
    ack(&mut driver, &release);
    drop(release);
    drop(session);
    drop(driver);
    assert!(budget.used() > 0, "pinned remote page keeps its charge");
    drop(page);
    assert_eq!(budget.used(), 0);
}
#[test]
fn priority_cancel_overtakes_unsent_queries_without_reordering_wire_ids() {
    let (mut driver, session, budget) = connected();
    let first = session.execute(query(), limits()).unwrap();
    let packet = driver.take_outbound().unwrap().unwrap();
    let original_id = packet.request_id;
    drop(packet);
    let second = session.execute(query(), limits()).unwrap();
    drop(first);
    let cancel = driver.take_outbound().unwrap().unwrap();
    let decoded = wire_request::decode(cancel.bytes(), &budget).unwrap();
    assert!(matches!(decoded.body,RequestBody::Cancel(id) if id==original_id));
    drop(decoded);
    let later = driver.take_outbound().unwrap().unwrap();
    assert!(later.request_id > cancel.request_id);
    assert!(cancel.request_id > original_id);
    ack(&mut driver, &cancel);
    driver
        .receive(HOST, &window(original_id, 1, 0, &[], true, 0))
        .unwrap();
    let release = driver.take_outbound().unwrap().unwrap();
    assert!(matches!(
        wire_request::decode(release.bytes(), &budget).unwrap().body,
        RequestBody::Release(_)
    ));
    ack(&mut driver, &release);
    driver
        .receive(HOST, &window(later.request_id, 2, 0, &[3], true, 0))
        .unwrap();
    assert!(ready(second).is_ok());
}
#[test]
fn pages_validate_order_preserve_same_time_changes_and_retry_shared_delivery() {
    let (mut driver, session, _) = connected();
    let first = session.execute(query(), limits()).unwrap();
    let packet = driver.take_outbound().unwrap().unwrap();
    driver
        .receive(HOST, &window(packet.request_id, 1, 0, &[3, 3], false, 0))
        .unwrap();
    let first = ready(first).unwrap();
    let second = session.advance(first.next.unwrap()).unwrap();
    let packet = driver.take_outbound().unwrap().unwrap();
    driver
        .receive(HOST, &window(packet.request_id, 1, 1, &[3, 4], true, 0))
        .unwrap();
    let second = ready(second).unwrap();
    let retry = ready(session.advance(second.request).unwrap()).unwrap();
    assert!(Arc::ptr_eq(&retry, &second));
    assert!(driver.take_outbound().unwrap().is_none());
    assert!(session.advance(first.request).is_err());
}
#[test]
fn wrong_coverage_or_predecessor_or_record_limits_close_the_driver() {
    for mode in 0..4 {
        let (mut driver, session, _) = connected();
        let first = session.execute(query(), limits()).unwrap();
        let packet = driver.take_outbound().unwrap().unwrap();
        driver
            .receive(HOST, &window(packet.request_id, 1, 0, &[3], false, 0))
            .unwrap();
        let first = ready(first).unwrap();
        let second = session.advance(first.next.unwrap()).unwrap();
        let packet = driver.take_outbound().unwrap().unwrap();
        let bytes = match mode {
            0 => window(packet.request_id, 1, 1, &[2], true, 0),
            1 => window(packet.request_id, 1, 1, &[4], true, 1),
            2 => window(packet.request_id, 1, 1, &[4, 5, 6], true, 0),
            _ => window(packet.request_id, 1, 2, &[4], true, 0),
        };
        assert!(driver.receive(HOST, &bytes).is_err());
        assert!(driver.is_closed());
        assert!(matches!(ready(second), Err(Error::Closed)));
    }
}
#[test]
fn replaced_incarnation_is_ignored_but_wrong_snapshot_and_duplicate_replies_fail() {
    let (mut driver, session, budget) = connected();
    let pending = session.execute(query(), limits()).unwrap();
    let packet = driver.take_outbound().unwrap().unwrap();
    let charged = budget.used();
    assert!(!driver.receive(Incarnation([2; 16]), &[]).unwrap());
    assert_eq!(budget.used(), charged);
    let good = window(packet.request_id, 1, 0, &[], true, 0);
    driver.receive(HOST, &good).unwrap();
    assert!(driver.receive(HOST, &good).is_err());
    assert!(matches!(ready(pending), Err(Error::Closed)));
    let (mut driver, session, _) = connected();
    let pending = session.execute(query(), limits()).unwrap();
    let packet = driver.take_outbound().unwrap().unwrap();
    let mut wrong =
        p::Envelope::decode(window(packet.request_id, 1, 0, &[], true, 0).as_slice()).unwrap();
    wrong.snapshot[0] = 9;
    assert!(driver.receive(HOST, &wrong.encode_to_vec()).is_err());
    assert!(matches!(ready(pending), Err(Error::Closed)));
}
#[test]
fn cancellation_of_delivered_unconsumed_result_discards_it_and_releases_operation() {
    let (mut driver, session, budget) = connected();
    let future = session.execute(query(), limits()).unwrap();
    let cancel = future.cancellation();
    let packet = driver.take_outbound().unwrap().unwrap();
    driver
        .receive(HOST, &window(packet.request_id, 1, 0, &[1], false, 0))
        .unwrap();
    cancel.cancel();
    assert!(matches!(ready(future), Err(Error::Cancelled)));
    let release = driver.take_outbound().unwrap().unwrap();
    assert!(matches!(
        wire_request::decode(release.bytes(), &budget).unwrap().body,
        RequestBody::Release(_)
    ));
    ack(&mut driver, &release);
    assert!(session.execute(query(), limits()).is_ok());
}
#[test]
fn close_fails_pending_futures_and_waits_for_its_acknowledgement() {
    let (mut driver, session, budget) = connected();
    let future = session.execute(query(), limits()).unwrap();
    let _packet = driver.take_outbound().unwrap().unwrap();
    session.close();
    assert!(matches!(ready(future), Err(Error::Closed)));
    let close = driver.take_outbound().unwrap().unwrap();
    assert!(matches!(
        wire_request::decode(close.bytes(), &budget).unwrap().body,
        RequestBody::Close
    ));
    assert!(!driver.is_closed());
    ack(&mut driver, &close);
    assert!(driver.is_closed());
}

thread_local! {
    static WAKE_SESSION:std::cell::RefCell<Option<std::rc::Rc<RpcSession>>>=const {std::cell::RefCell::new(None)};
    static WAKE_COUNT:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};
}
struct Reenter;
impl std::task::Wake for Reenter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        WAKE_SESSION.with(|slot| {
            if let Some(session) = slot.borrow().as_ref() {
                drop(session.execute(query(), limits()).unwrap());
            }
        });
        WAKE_COUNT.with(|count| count.set(count.get() + 1));
    }
}
#[test]
fn waking_a_consumer_can_reenter_the_client_without_borrowing_panics() {
    let (mut driver, session, _) = connected();
    let session = std::rc::Rc::new(session);
    WAKE_SESSION.with(|slot| *slot.borrow_mut() = Some(session.clone()));
    WAKE_COUNT.with(|count| count.set(0));
    let mut future = session.execute(query(), limits()).unwrap();
    let waker = Waker::from(Arc::new(Reenter));
    assert!(Pin::new(&mut future)
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    let packet = driver.take_outbound().unwrap().unwrap();
    driver
        .receive(HOST, &window(packet.request_id, 1, 0, &[], true, 0))
        .unwrap();
    assert!(ready(future).is_ok());
    assert!(WAKE_COUNT.with(|count| count.get()) > 0);
    WAKE_SESSION.with(|slot| slot.borrow_mut().take());
}

#[test]
fn full_control_queue_preserves_a_release_for_retry() {
    let (mut driver, session, budget) = connected();
    let mut controls = vec![];
    for operation in 1..=9 {
        let future = session.execute(query(), limits()).unwrap();
        let packet = loop {
            let packet = driver.take_outbound().unwrap().unwrap();
            if matches!(
                wire_request::decode(packet.bytes(), &budget).unwrap().body,
                RequestBody::Release(_)
            ) {
                controls.push(packet);
            } else {
                break packet;
            }
        };
        driver
            .receive(HOST, &window(packet.request_id, operation, 0, &[], true, 0))
            .unwrap();
        let page = ready(future).unwrap();
        if operation < 9 {
            session.release(page.request).unwrap();
        } else {
            assert_eq!(controls.len(), 8);
            assert!(matches!(
                session.release(page.request),
                Err(Error::ResourceLimit)
            ));
            ack(&mut driver, &controls[0]);
            session.release(page.request).unwrap();
            let retry = driver.take_outbound().unwrap().unwrap();
            assert!(
                matches!(wire_request::decode(retry.bytes(),&budget).unwrap().body,RequestBody::Release(c) if c.operation==9)
            );
        }
    }
}
#[test]
fn locally_oversized_query_fails_its_future_without_poisoning_the_session() {
    let (mut driver, session, _) = connected();
    let paths = (0..5000)
        .map(|_| vtr_query::metadata::Path {
            segments: vec![],
            kind: None,
            occurrence: None,
        })
        .collect();
    let future = session.execute(Query::Resolve { paths }, limits()).unwrap();
    assert!(driver.take_outbound().unwrap().is_none());
    assert!(matches!(ready(future), Err(Error::ResourceLimit)));
    assert!(!driver.is_closed());
    assert!(session.execute(query(), limits()).is_ok());
}

struct CountWake(std::sync::atomic::AtomicUsize);
impl std::task::Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
#[test]
fn queued_work_wakes_an_idle_transport_without_waiting_for_a_viewer_frame() {
    let (mut driver, session, _) = connected();
    let wake = Arc::new(CountWake(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(driver.poll_outbound(&mut cx).is_pending());
    let future = session.execute(query(), limits()).unwrap();
    assert_eq!(wake.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(matches!(
        driver.poll_outbound(&mut cx),
        Poll::Ready(Ok(Some(_)))
    ));
    assert!(driver.poll_outbound(&mut cx).is_pending());
    drop(future);
    assert_eq!(wake.0.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(matches!(
        driver.poll_outbound(&mut cx),
        Poll::Ready(Ok(Some(_)))
    ));
    assert!(driver.poll_outbound(&mut cx).is_pending());
    session.close();
    assert_eq!(wake.0.load(std::sync::atomic::Ordering::Relaxed), 3);
}
