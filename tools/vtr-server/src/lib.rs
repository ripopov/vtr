//! A single document's stdio endpoint. The input pump, query worker and output
//! pump are independent so stdout backpressure cannot stop cancellation input.
use async_channel::{Receiver, Sender};
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
};
use vtr_query::local_session::{CancelHandle, LocalSession};
use vtr_query::session::{Continuation, Delivery, SnapshotId};
use vtr_query::wire::{
    self, proto::failure::Code, MAX_OUTSTANDING, MAX_REPLY_BYTES, MAX_REQUEST_BYTES,
};
use vtr_query::wire_encode::{self, Encoded};
use vtr_query::wire_request::{self, Request, RequestBody};
use vtr_query::{Budget, Cancellation, Error, Result};

const CONTROLS: usize = 8;
const QUEUED: usize = MAX_OUTSTANDING + CONTROLS;
// Application allocations; reader cache/scratch and OS pipe memory are not
// covered by this ceiling and still require reader admission and RSS tests.
const DATA_BUDGET: usize = 64 * 1024 * 1024;
const CONTROL_BUDGET: usize = 256 * 1024;
struct Pending {
    id: u64,
    cursor: Option<Continuation>,
    cancel: CancelHandle,
    delivery_cancel: Cancellation,
}
struct Output {
    id: u64,
    bytes: Encoded,
    slot: Option<usize>,
    close: bool,
    snapshot: Option<SnapshotId>,
    cancel: Option<Cancellation>,
    release: Option<(Arc<LocalSession>, Continuation)>,
}
enum Finished {
    Open(u64, Result<LocalSession>),
    Query(usize, u64, Result<Arc<Delivery>>),
}
type Work = Pin<Box<dyn Future<Output = Finished> + Send>>;
enum Event {
    Input(std::result::Result<Request, async_channel::RecvError>),
    Finished(Finished),
    Written(std::result::Result<(Option<usize>, bool), async_channel::RecvError>),
}
struct Endpoint {
    session: Option<Arc<LocalSession>>,
    welcomed: bool,
    opening: bool,
    last_id: u64,
    pending: [Option<Pending>; MAX_OUTSTANDING],
    work: FuturesUnordered<Work>,
    control: Sender<Output>,
    data: Sender<Output>,
    budget: Budget,
    control_budget: Budget,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            session.close();
        }
    }
}
impl Endpoint {
    fn send(&self, id: u64, bytes: Encoded, close: bool) -> Result<()> {
        self.control
            .try_send(Output {
                id,
                bytes,
                slot: None,
                close,
                snapshot: None,
                cancel: None,
                release: None,
            })
            .map_err(|_| Error::ResourceLimit)
    }
    fn failure(&self, id: u64, snapshot: Option<SnapshotId>, error: &Error) -> Result<()> {
        let (code, message) = match error {
            Error::Closed => (Code::Stale, "query session closed"),
            Error::ResourceLimit => (Code::ResourceLimit, "query resource limit exceeded"),
            Error::Cancelled => (Code::Cancelled, "query cancelled"),
            Error::Invalid(s) => (Code::Invalid, *s),
            Error::Frame(s) => (Code::Invalid, *s),
            // Detailed backend diagnostics stay on stderr; the wire error is
            // bounded and does not copy arbitrary reader error strings.
            Error::Backend(s) => {
                eprintln!("vtr-server: {s}");
                (Code::Backend, "trace backend failed")
            }
        };
        self.send(
            id,
            wire_encode::failure(id, snapshot, code, message, &self.control_budget)?,
            false,
        )
    }
    fn current(&self, identity: Option<SnapshotId>) -> Result<Arc<LocalSession>> {
        let session = self
            .session
            .as_ref()
            .ok_or(Error::Invalid("trace is not open"))?;
        if identity != Some(session.info().snapshot) {
            return Err(Error::Invalid("request belongs to another snapshot"));
        }
        Ok(session.clone())
    }
    fn request(&mut self, r: Request, path: &std::path::Path) -> Result<bool> {
        if r.request_id <= self.last_id {
            return Err(Error::Frame(
                "request IDs must increase within a child session",
            ));
        }
        self.last_id = r.request_id;
        let id = r.request_id;
        let identity = r.snapshot;
        let result = (|| {
            match r.body {
                RequestBody::Hello if !self.welcomed && identity.is_none() => {
                    self.welcomed = true;
                    self.send(id, wire_encode::welcome(id, &self.control_budget)?, false)?;
                }
                RequestBody::Open
                    if self.welcomed
                        && !self.opening
                        && self.session.is_none()
                        && identity.is_none() =>
                {
                    let open = LocalSession::open(
                        path.to_path_buf(),
                        self.budget.clone(),
                        MAX_OUTSTANDING,
                    )?;
                    self.opening = true;
                    self.work
                        .push(Box::pin(async move { Finished::Open(id, open.await) }));
                }
                RequestBody::Query { query, mut limits } => {
                    let session = self.current(identity)?;
                    let slot = self
                        .pending
                        .iter()
                        .position(Option::is_none)
                        .ok_or(Error::ResourceLimit)?;
                    // Limits are upper bounds; shorter pages keep codec work
                    // bounded. Value/summary bytes still have independent caps.
                    limits.records = limits.records.min(128);
                    limits.bytes = limits.bytes.min(256 * 1024);
                    let future = session.execute(query, limits)?;
                    self.pending[slot] = Some(Pending {
                        id,
                        cursor: None,
                        cancel: future.cancellation(),
                        delivery_cancel: Cancellation::default(),
                    });
                    self.work.push(Box::pin(
                        async move { Finished::Query(slot, id, future.await) },
                    ));
                }
                RequestBody::Next(cursor) => {
                    let session = self.current(identity)?;
                    let slot = self
                        .pending
                        .iter()
                        .position(Option::is_none)
                        .ok_or(Error::ResourceLimit)?;
                    let future = session.advance(cursor)?;
                    self.pending[slot] = Some(Pending {
                        id,
                        cursor: Some(cursor),
                        cancel: future.cancellation(),
                        delivery_cancel: Cancellation::default(),
                    });
                    self.work.push(Box::pin(
                        async move { Finished::Query(slot, id, future.await) },
                    ));
                }
                RequestBody::Cancel(target) => {
                    self.current(identity)?;
                    if let Some(p) = self.pending.iter().flatten().find(|p| p.id == target) {
                        p.cancel.cancel();
                        p.delivery_cancel.cancel();
                    }
                    self.send(
                        id,
                        wire_encode::acknowledged(id, identity, &self.control_budget)?,
                        false,
                    )?;
                }
                RequestBody::Release(cursor) => {
                    self.current(identity)?.release(cursor)?;
                    self.send(
                        id,
                        wire_encode::acknowledged(id, identity, &self.control_budget)?,
                        false,
                    )?;
                }
                RequestBody::Close => {
                    if let Some(session) = &self.session {
                        if identity != Some(session.info().snapshot) {
                            return Err(Error::Invalid("close belongs to another snapshot"));
                        }
                        session.close();
                    }
                    for p in self.pending.iter().flatten() {
                        p.cancel.cancel();
                        p.delivery_cancel.cancel();
                    }
                    self.work.clear();
                    self.send(
                        id,
                        wire_encode::acknowledged(id, identity, &self.control_budget)?,
                        true,
                    )?;
                    return Ok(true);
                }
                _ => return Err(Error::Invalid("invalid handshake state")),
            }
            Ok(false)
        })();
        match result {
            Ok(close) => Ok(close),
            Err(error) => {
                self.failure(id, identity, &error)?;
                Ok(false)
            }
        }
    }
    fn finished(&mut self, finished: Finished) -> Result<()> {
        match finished {
            Finished::Open(id, result) => {
                self.opening = false;
                match result {
                    Ok(session) => {
                        let bytes = wire_encode::opened(id, session.info(), &self.control_budget)?;
                        self.session = Some(Arc::new(session));
                        self.send(id, bytes, false)?;
                    }
                    Err(error) => self.failure(id, None, &error)?,
                }
            }
            Finished::Query(slot, id, result) => {
                let pending = self.pending[slot].as_ref().ok_or(Error::Closed)?;
                let session = self.session.as_ref().ok_or(Error::Closed)?;
                let identity = Some(session.info().snapshot);
                let (bytes, release) = match result {
                    Ok(delivery) => match wire_encode::delivery(id, &delivery, &self.budget) {
                        Ok(bytes) => (bytes, Some((session.clone(), delivery.request))),
                        Err(_error) => {
                            session.release(delivery.request)?;
                            (
                                wire_encode::failure(
                                    id,
                                    identity,
                                    Code::ResourceLimit,
                                    "reply exceeds transport limits",
                                    &self.control_budget,
                                )?,
                                None,
                            )
                        }
                    },
                    Err(error) => {
                        let code = match error {
                            Error::Cancelled => Code::Cancelled,
                            Error::ResourceLimit => Code::ResourceLimit,
                            Error::Backend(_) => Code::Backend,
                            Error::Closed => Code::Stale,
                            _ => Code::Invalid,
                        };
                        (
                            wire_encode::failure(
                                id,
                                identity,
                                code,
                                "query failed",
                                &self.control_budget,
                            )?,
                            pending.cursor.map(|cursor| (session.clone(), cursor)),
                        )
                    }
                };
                // The slot remains occupied until the output pump has written
                // or discarded this page. A slow consumer cannot grow this queue.
                self.data
                    .try_send(Output {
                        id,
                        bytes,
                        slot: Some(slot),
                        close: false,
                        snapshot: identity,
                        cancel: Some(pending.delivery_cancel.clone()),
                        release,
                    })
                    .map_err(|_| Error::ResourceLimit)?;
            }
        }
        Ok(())
    }
}

pub fn run_stdio(path: PathBuf) -> Result<()> {
    let budget = Budget::new(DATA_BUDGET);
    let control_budget = Budget::new(CONTROL_BUDGET);
    let _queues = control_budget
        .reserve(QUEUED * (std::mem::size_of::<Request>() + std::mem::size_of::<Output>() + 512))?;
    let (input_tx, input) = async_channel::bounded(QUEUED);
    let input_failure = Arc::new(Mutex::new(None));
    let failure = input_failure.clone();
    let input_budget = budget.clone();
    std::thread::Builder::new()
        .name("vtr-stdin".into())
        .spawn(move || {
            let result = (|| {
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                while let Some(frame) =
                    wire::read_frame(&mut input, MAX_REQUEST_BYTES, input_budget.clone())
                        .map_err(|e| Error::Backend(e.to_string()))?
                {
                    let request = wire_request::decode(frame.bytes(), &input_budget)?;
                    input_tx
                        .try_send(request)
                        .map_err(|_| Error::ResourceLimit)?;
                }
                Ok(())
            })();
            *failure.lock().unwrap() = result.err();
            input_tx.close();
        })
        .map_err(|e| Error::Backend(e.to_string()))?;
    let (control_tx, control) = async_channel::bounded(CONTROLS);
    let (data_tx, data) = async_channel::bounded(MAX_OUTSTANDING);
    let (written_tx, written) = async_channel::bounded(QUEUED);
    let output_budget = control_budget.clone();
    std::thread::Builder::new()
        .name("vtr-stdout".into())
        .spawn(move || output(control, data, written_tx, output_budget))
        .map_err(|e| Error::Backend(e.to_string()))?;
    let mut endpoint = Endpoint {
        session: None,
        welcomed: false,
        opening: false,
        last_id: 0,
        pending: std::array::from_fn(|_| None),
        work: FuturesUnordered::new(),
        control: control_tx,
        data: data_tx,
        budget,
        control_budget,
    };
    futures_lite::future::block_on(async {
        let mut closing = false;
        loop {
            let incoming = async {
                if closing {
                    std::future::pending::<Event>().await
                } else {
                    Event::Input(input.recv().await)
                }
            };
            let completed = async {
                if endpoint.work.is_empty() {
                    std::future::pending::<Event>().await
                } else {
                    Event::Finished(endpoint.work.next().await.unwrap())
                }
            };
            let outgoing = async { Event::Written(written.recv().await) };
            let event = futures_lite::future::race(
                incoming,
                futures_lite::future::race(completed, outgoing),
            )
            .await;
            match event {
                Event::Input(Ok(request)) => closing = endpoint.request(request, &path)?,
                Event::Input(Err(_)) => {
                    return input_failure.lock().unwrap().take().map_or(Ok(()), Err)
                }
                Event::Finished(finished) => endpoint.finished(finished)?,
                Event::Written(Ok((slot, close))) => {
                    if let Some(slot) = slot {
                        endpoint.pending[slot] = None;
                    }
                    if close {
                        return Ok(());
                    }
                }
                Event::Written(Err(_)) => return Err(Error::Closed),
            }
        }
    })
}
fn output(
    control: Receiver<Output>,
    data: Receiver<Output>,
    written: Sender<(Option<usize>, bool)>,
    budget: Budget,
) {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    loop {
        let item = match control.try_recv() {
            Ok(item) => Ok(item),
            Err(_) => futures_lite::future::block_on(futures_lite::future::race(
                control.recv(),
                data.recv(),
            )),
        };
        let Ok(item) = item else { return };
        let cancelled = item.cancel.as_ref().is_some_and(|c| c.check().is_err());
        let bytes = if cancelled {
            if let Some((session, cursor)) = &item.release {
                let _ = session.release(*cursor);
            }
            let identity = item.snapshot;
            match wire_encode::failure(
                item.id,
                identity,
                Code::Cancelled,
                "query cancelled",
                &budget,
            ) {
                Ok(bytes) => bytes,
                Err(_) => return,
            }
        } else {
            item.bytes
        };
        if wire::write_frame(&mut stdout, bytes.bytes(), MAX_REPLY_BYTES).is_err() {
            return;
        }
        if written.try_send((item.slot, item.close)).is_err() || item.close {
            return;
        }
    }
}
