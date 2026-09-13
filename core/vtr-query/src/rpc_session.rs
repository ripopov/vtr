//! Single-threaded RPC session state, shared by a WASM host and its query
//! futures. The host moves opaque packets, pumps receive outside paint/layout,
//! and tears down the child when the driver closes. No JavaScript integers or
//! transport endpoint details enter query state.
use crate::session::{Continuation, Delivery, Query, Reply, SessionInfo};
use crate::wave::{Limits, Predecessor, Sample};
use crate::wire::{
    self,
    proto::{self as p, failure::Code},
};
use crate::wire_request::RequestBody;
use crate::{wire_encode, wire_reply};
use crate::{Budget, Error, Grid, Interval, Reservation, Result};
use futures_channel::oneshot;
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::{Rc, Weak},
    sync::Arc,
    task::{Context, Poll, Waker},
};
const DATA: usize = wire::MAX_OUTSTANDING;
const CONTROLS: usize = 8;
const CONTROL_FRAME: usize = 16 * 1024;
const CONTROL_DECODED: usize = 64 * 1024;
const CONTROL_POOL: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Incarnation(pub [u8; 16]);
pub struct Outbound {
    pub incarnation: Incarnation,
    pub request_id: u64,
    bytes: wire_encode::Encoded,
}
impl Outbound {
    pub fn bytes(&self) -> &[u8] {
        self.bytes.bytes()
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Hello,
    Open,
    Ready,
    Closing,
    Closed,
}
#[derive(Clone, Copy)]
enum Purpose {
    Hello,
    Open,
    Cancel(u64),
    Release(Continuation),
    Close,
}
struct Control {
    purpose: Purpose,
    sent: Option<u64>,
    order: u64,
}
#[derive(Clone, Copy)]
enum Shape {
    Window(Interval),
    Summary(Grid),
    Children(Option<u32>),
    Search,
    Resolve(usize),
    Text(u32, u64, usize),
}
struct Operation {
    key: u64,
    shape: Shape,
    limits: Limits,
    last: Option<Arc<Delivery>>,
    next: Option<Continuation>,
    predecessor: Option<Arc<Predecessor>>,
    last_time: Option<u64>,
    offset: u64,
    last_node: Option<u32>,
    exit: Option<Sample>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveryState {
    Queued,
    Sent,
    Delivered,
}
struct Pending {
    ticket: u64,
    operation: u64,
    command: Option<RequestBody>,
    sent: Option<u64>,
    cursor: Option<Continuation>,
    phase: DeliveryState,
    cancelled: bool,
    abandoned: bool,
    limits: Limits,
    quota: Option<Reservation>,
    sender: Option<oneshot::Sender<Result<Arc<Delivery>>>>,
    _charge: Reservation,
}
enum Notice {
    Open(
        oneshot::Sender<Result<Arc<SessionInfo>>>,
        Result<Arc<SessionInfo>>,
    ),
    Query(
        oneshot::Sender<Result<Arc<Delivery>>>,
        Result<Arc<Delivery>>,
    ),
}
// Wake consumers only after releasing the state borrow. A waker may schedule
// immediate work which submits or polls another request on the same thread.
fn flush(state: &Rc<RefCell<State>>) {
    loop {
        let notice = {
            let mut state = state.borrow_mut();
            state.notices.iter_mut().find_map(Option::take)
        };
        match notice {
            Some(Notice::Open(sender, result)) => {
                let _ = sender.send(result);
            }
            Some(Notice::Query(sender, result)) => {
                let _ = sender.send(result);
            }
            None => {
                let wake = {
                    let mut state = state.borrow_mut();
                    if std::mem::take(&mut state.notify_outbound) {
                        state.outbound_waker.take()
                    } else {
                        None
                    }
                };
                if let Some(wake) = wake {
                    wake.wake();
                }
                break;
            }
        }
    }
}
struct State {
    incarnation: Incarnation,
    phase: Phase,
    info: Option<Arc<SessionInfo>>,
    capabilities: u8,
    limits: p::Welcome,
    next_ticket: u64,
    next_wire: u64,
    outbound_waker: Option<Waker>,
    notify_outbound: bool,
    pending: [Option<Pending>; DATA],
    operations: [Option<Operation>; DATA],
    controls: [Option<Control>; CONTROLS],
    opened: Option<oneshot::Sender<Result<Arc<SessionInfo>>>>,
    notices: [Option<Notice>; DATA + 1],
    budget: Budget,
    control_budget: Budget,
    _charge: Reservation,
}
pub struct RpcDriver {
    state: Rc<RefCell<State>>,
}
pub struct RpcSession {
    state: Rc<RefCell<State>>,
    info: Arc<SessionInfo>,
}
pub struct RpcOpenFuture {
    receiver: oneshot::Receiver<Result<Arc<SessionInfo>>>,
    state: Weak<RefCell<State>>,
    _charge: Reservation,
    finished: bool,
}
pub struct RpcQueryFuture {
    receiver: oneshot::Receiver<Result<Arc<Delivery>>>,
    state: Weak<RefCell<State>>,
    ticket: u64,
    finished: bool,
    _charge: Reservation,
}
#[derive(Clone)]
pub struct CancelHandle {
    state: Weak<RefCell<State>>,
    ticket: u64,
}
impl CancelHandle {
    pub fn cancel(&self) {
        if let Some(state) = self.state.upgrade() {
            let mut state = state.borrow_mut();
            if state.cancel(self.ticket, false).is_err() {
                state.fail();
            }
            drop(state);
            flush(&self.state.upgrade().unwrap());
        }
    }
}
impl RpcDriver {
    pub fn new(incarnation: Incarnation, budget: Budget) -> Result<(Self, RpcOpenFuture)> {
        let charge = budget.reserve(std::mem::size_of::<State>() + 512)?;
        let control_budget = budget.child(CONTROL_POOL)?;
        let (tx, rx) = oneshot::channel();
        let mut state = State {
            incarnation,
            phase: Phase::Hello,
            info: None,
            capabilities: 0,
            limits: p::Welcome {
                max_request_bytes: wire::MAX_REQUEST_BYTES as u32,
                max_reply_bytes: wire::MAX_REPLY_BYTES as u32,
                max_decoded_bytes: wire::MAX_DECODED_BYTES as u32,
                max_records: wire::MAX_RECORDS as u32,
                max_outstanding: DATA as u32,
            },
            next_ticket: 0,
            next_wire: 0,
            outbound_waker: None,
            notify_outbound: false,
            pending: std::array::from_fn(|_| None),
            operations: std::array::from_fn(|_| None),
            controls: std::array::from_fn(|_| None),
            opened: Some(tx),
            notices: std::array::from_fn(|_| None),
            budget,
            control_budget,
            _charge: charge.clone(),
        };
        state.control(Purpose::Hello)?;
        let state = Rc::new(RefCell::new(state));
        let open = RpcOpenFuture {
            receiver: rx,
            state: Rc::downgrade(&state),
            _charge: charge,
            finished: false,
        };
        Ok((Self { state }, open))
    }
    /// IDs are assigned here, after priority selection. A control message can
    /// overtake an unsent data request without violating server ID ordering.
    pub fn take_outbound(&mut self) -> Result<Option<Outbound>> {
        let result = self.state.borrow_mut().outbound();
        if result.is_err() {
            self.state.borrow_mut().fail();
        }
        flush(&self.state);
        result
    }
    /// Await a queued packet without a render-loop poll. A host can call this
    /// through poll_fn; no state borrow is retained while the future is pending.
    pub fn poll_outbound(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<Outbound>>> {
        let mut state = self.state.borrow_mut();
        let result = state.outbound();
        let result = match result {
            Ok(None) if state.phase != Phase::Closed => {
                if state
                    .outbound_waker
                    .as_ref()
                    .is_none_or(|old| !old.will_wake(cx.waker()))
                {
                    state.outbound_waker = Some(cx.waker().clone());
                }
                state.notify_outbound = false;
                Poll::Pending
            }
            Err(error) => {
                state.fail();
                Poll::Ready(Err(error))
            }
            result => Poll::Ready(result),
        };
        drop(state);
        flush(&self.state);
        result
    }
    /// True means a packet from this incarnation was consumed. Old-incarnation
    /// packets are discarded without decoding or touching current requests.
    pub fn receive(&mut self, incarnation: Incarnation, bytes: &[u8]) -> Result<bool> {
        let mut state = self.state.borrow_mut();
        if incarnation != state.incarnation || state.phase == Phase::Closed {
            return Ok(false);
        }
        let result = state.receive(bytes);
        if result.is_err() {
            state.fail();
        }
        drop(state);
        flush(&self.state);
        result.map(|()| true)
    }
    pub fn transport_failed(&mut self) {
        self.state.borrow_mut().fail();
        flush(&self.state);
    }
    pub fn is_closed(&self) -> bool {
        self.state.borrow().phase == Phase::Closed
    }
}
impl Drop for RpcDriver {
    fn drop(&mut self) {
        self.state.borrow_mut().fail();
        flush(&self.state);
    }
}
impl RpcSession {
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }
    pub fn execute(&self, query: Query, limits: Limits) -> Result<RpcQueryFuture> {
        let mut state = self.state.borrow_mut();
        state.ready()?;
        let limits = query_limits(limits, &state.limits)?;
        if let Query::Resolve { paths } = &query {
            if paths
                .iter()
                .any(|p| p.kind.is_some_and(|k| !(1..=5).contains(&k)))
            {
                return Err(Error::Invalid("invalid node kind"));
            }
        }
        let (shape, capability) = shape(&query);
        if state.capabilities & (1 << capability) == 0 {
            return Err(Error::Invalid("remote operation is unsupported"));
        }
        let slot = state
            .operations
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ResourceLimit)?;
        let future = state.submit(
            RequestBody::Query { query, limits },
            None,
            None,
            limits,
            &self.state,
        )?;
        state.operations[slot] = Some(Operation {
            key: future.ticket,
            shape,
            limits,
            last: None,
            next: None,
            predecessor: None,
            last_time: None,
            offset: 0,
            last_node: None,
            exit: None,
        });
        drop(state);
        flush(&self.state);
        Ok(future)
    }
    pub fn advance(&self, cursor: Continuation) -> Result<RpcQueryFuture> {
        let mut state = self.state.borrow_mut();
        state.ready()?;
        if cursor.snapshot != self.info.snapshot {
            return Err(Error::Invalid("foreign snapshot"));
        }
        let op = state
            .operations
            .iter()
            .flatten()
            .find(|o| {
                o.last
                    .as_ref()
                    .is_some_and(|d| d.request.operation == cursor.operation)
            })
            .ok_or(Error::Invalid("unknown operation"))?;
        let key = op.key;
        if state.pending.iter().flatten().any(|p| p.operation == key) {
            return Err(Error::ResourceLimit);
        }
        let cached = op.last.as_ref().filter(|d| d.request == cursor).cloned();
        if cached.is_none() && op.next != Some(cursor) {
            return Err(Error::Invalid("stale or future continuation"));
        }
        let limits = op.limits;
        // Per-operation limits are restored by submit from the stored page
        // contract, including for a local retry of the last received delivery.
        let future = state.submit(
            RequestBody::Next(cursor),
            Some(key),
            cached,
            limits,
            &self.state,
        )?;
        drop(state);
        flush(&self.state);
        Ok(future)
    }
    pub fn release(&self, cursor: Continuation) -> Result<()> {
        let mut state = self.state.borrow_mut();
        state.ready()?;
        if cursor.snapshot != self.info.snapshot {
            return Err(Error::Invalid("foreign snapshot"));
        }
        if let Some(key) = state
            .operations
            .iter()
            .flatten()
            .find(|o| {
                o.last
                    .as_ref()
                    .is_some_and(|d| d.request.operation == cursor.operation)
            })
            .map(|o| o.key)
        {
            let result = state.release(key);
            drop(state);
            flush(&self.state);
            return result;
        }
        Ok(())
    }
    pub fn close(&self) {
        let mut state = self.state.borrow_mut();
        if state.begin_close().is_err() {
            state.fail();
        }
        drop(state);
        flush(&self.state);
    }
}
impl Drop for RpcSession {
    fn drop(&mut self) {
        self.close();
    }
}
impl Future for RpcOpenFuture {
    type Output = Result<RpcSession>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(r) => r.unwrap_or(Err(Error::Closed)),
        };
        self.finished = true;
        Poll::Ready(value.and_then(|info| {
            let state = self.state.upgrade().ok_or(Error::Closed)?;
            if state.borrow().phase != Phase::Ready {
                return Err(Error::Closed);
            }
            Ok(RpcSession { state, info })
        }))
    }
}
impl Drop for RpcOpenFuture {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(state) = self.state.upgrade() {
                state.borrow_mut().fail();
                flush(&state);
            }
        }
    }
}
impl RpcQueryFuture {
    pub fn cancellation(&self) -> CancelHandle {
        CancelHandle {
            state: self.state.clone(),
            ticket: self.ticket,
        }
    }
}
impl Future for RpcQueryFuture {
    type Output = Result<Arc<Delivery>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(r) => r.unwrap_or(Err(Error::Closed)),
        };
        self.finished = true;
        let value = if let Some(state) = self.state.upgrade() {
            let mut state = state.borrow_mut();
            let cancelled = state
                .pending
                .iter()
                .flatten()
                .any(|p| p.ticket == self.ticket && p.cancelled);
            let closed = matches!(state.phase, Phase::Closed | Phase::Closing);
            state.retire(self.ticket);
            if closed {
                Err(Error::Closed)
            } else if cancelled {
                Err(Error::Cancelled)
            } else {
                value
            }
        } else {
            Err(Error::Closed)
        };
        Poll::Ready(value)
    }
}
impl Drop for RpcQueryFuture {
    fn drop(&mut self) {
        if !self.finished {
            self.receiver.close();
            if let Some(state) = self.state.upgrade() {
                let mut state = state.borrow_mut();
                if state.cancel(self.ticket, true).is_err() {
                    state.fail();
                }
                drop(state);
                flush(&self.state.upgrade().unwrap());
            }
        }
    }
}
fn query_limits(limits: Limits, peer: &p::Welcome) -> Result<Limits> {
    if limits.bytes == 0 || limits.records == 0 || limits.work == 0 {
        return Err(Error::Invalid("zero query limit"));
    }
    Ok(Limits {
        bytes: limits.bytes.min(peer.max_decoded_bytes as usize),
        records: limits.records.min(peer.max_records as usize),
        work: limits.work.min(65536),
    })
}
fn shape(query: &Query) -> (Shape, u8) {
    match query {
        Query::Window { interval, .. } => (Shape::Window(*interval), 1),
        Query::Summary { grid, .. } => (Shape::Summary(*grid), 2),
        Query::Children { parent } => (Shape::Children(*parent), 3),
        Query::Search { .. } => (Shape::Search, 4),
        Query::Resolve { paths } => (Shape::Resolve(paths.len()), 5),
        Query::Text { id, offset, length } => (Shape::Text(*id, *offset, *length), 6),
    }
}
fn input_bytes(body: &RequestBody) -> Result<usize> {
    let mut total = 512usize;
    let mut add = |n: usize| {
        total = total.checked_add(n).ok_or(Error::ResourceLimit)?;
        Ok(())
    };
    if let RequestBody::Query { query, .. } = body {
        match query {
            Query::Search { needle, .. } => add(needle.capacity())?,
            Query::Resolve { paths } => {
                add(paths
                    .capacity()
                    .checked_mul(std::mem::size_of::<crate::metadata::Path>())
                    .ok_or(Error::ResourceLimit)?)?;
                for path in paths {
                    add(path
                        .segments
                        .capacity()
                        .checked_mul(std::mem::size_of::<String>())
                        .ok_or(Error::ResourceLimit)?)?;
                    for s in &path.segments {
                        add(s.capacity())?;
                    }
                }
            }
            _ => (),
        }
    }
    Ok(total)
}
impl State {
    fn notice(&mut self, notice: Notice) {
        *self
            .notices
            .iter_mut()
            .find(|n| n.is_none())
            .expect("one notice per fixed receiver slot") = Some(notice);
    }
    fn ready(&self) -> Result<()> {
        if self.phase == Phase::Ready {
            Ok(())
        } else {
            Err(Error::Closed)
        }
    }
    fn ticket(&mut self) -> Result<u64> {
        self.next_ticket = self
            .next_ticket
            .checked_add(1)
            .ok_or(Error::ResourceLimit)?;
        Ok(self.next_ticket)
    }
    fn control(&mut self, purpose: Purpose) -> Result<()> {
        if self
            .controls
            .iter()
            .flatten()
            .any(|c| match (c.purpose, purpose) {
                (Purpose::Cancel(a), Purpose::Cancel(b)) => a == b,
                (Purpose::Release(a), Purpose::Release(b)) => {
                    a.snapshot == b.snapshot && a.operation == b.operation
                }
                _ => false,
            })
        {
            return Ok(());
        }
        let i = self
            .controls
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ResourceLimit)?;
        let order = self.ticket()?;
        self.controls[i] = Some(Control {
            purpose,
            sent: None,
            order,
        });
        self.notify_outbound = true;
        Ok(())
    }
    fn submit(
        &mut self,
        command: RequestBody,
        key: Option<u64>,
        cached: Option<Arc<Delivery>>,
        limits: Limits,
        shared: &Rc<RefCell<Self>>,
    ) -> Result<RpcQueryFuture> {
        if self.pending.iter().flatten().count() >= self.limits.max_outstanding as usize {
            return Err(Error::ResourceLimit);
        }
        let index = self
            .pending
            .iter()
            .position(Option::is_none)
            .ok_or(Error::ResourceLimit)?;
        let charge = self.budget.reserve(input_bytes(&command)?)?;
        let quota = if cached.is_none() {
            Some(self.budget.reserve(
                self.limits.max_reply_bytes as usize + self.limits.max_decoded_bytes as usize,
            )?)
        } else {
            None
        };
        let ticket = self.ticket()?;
        let operation = key.unwrap_or(ticket);
        let cursor = match &command {
            RequestBody::Next(c) => Some(*c),
            _ => None,
        };
        let (tx, rx) = oneshot::channel();
        let mut p = Pending {
            ticket,
            operation,
            command: Some(command),
            sent: None,
            cursor,
            phase: DeliveryState::Queued,
            cancelled: false,
            abandoned: false,
            limits,
            quota,
            sender: Some(tx),
            _charge: charge.clone(),
        };
        if let Some(reply) = cached {
            p.command = None;
            p.phase = DeliveryState::Delivered;
            let _ = p.sender.take().unwrap().send(Ok(reply));
        }
        self.notify_outbound |= p.phase == DeliveryState::Queued;
        self.pending[index] = Some(p);
        Ok(RpcQueryFuture {
            receiver: rx,
            state: Rc::downgrade(shared),
            ticket,
            finished: false,
            _charge: charge,
        })
    }
    fn retire(&mut self, ticket: u64) {
        if let Some(slot) = self.pending.iter_mut().find(|s| {
            s.as_ref()
                .is_some_and(|p| p.ticket == ticket && p.phase == DeliveryState::Delivered)
        }) {
            *slot = None;
        }
    }
    fn cancel(&mut self, ticket: u64, abandoned: bool) -> Result<()> {
        let Some(i) = self
            .pending
            .iter()
            .position(|s| s.as_ref().is_some_and(|p| p.ticket == ticket))
        else {
            return Ok(());
        };
        let p = self.pending[i].as_mut().unwrap();
        p.abandoned |= abandoned;
        if p.cancelled && !abandoned {
            return Ok(());
        }
        p.cancelled = true;
        let key = p.operation;
        let phase = p.phase;
        let sent = p.sent;
        if phase == DeliveryState::Sent {
            if let Some(id) = sent {
                self.control(Purpose::Cancel(id))?;
            }
        } else {
            self.complete(i, Err(Error::Cancelled));
            self.remove_operation(key)?;
        }
        Ok(())
    }
    fn release(&mut self, key: u64) -> Result<()> {
        if let Some(ticket) = self
            .pending
            .iter()
            .flatten()
            .find(|p| p.operation == key)
            .map(|p| p.ticket)
        {
            self.cancel(ticket, false)?;
        }
        self.remove_operation(key)
    }
    fn remove_operation(&mut self, key: u64) -> Result<()> {
        if let Some(i) = self
            .operations
            .iter()
            .position(|o| o.as_ref().is_some_and(|o| o.key == key))
        {
            let cursor = self.operations[i]
                .as_ref()
                .unwrap()
                .last
                .as_ref()
                .map(|last| last.request);
            if let Some(cursor) = cursor {
                self.control(Purpose::Release(cursor))?;
            }
            self.operations[i] = None;
        }
        Ok(())
    }
    fn complete(&mut self, i: usize, result: Result<Arc<Delivery>>) {
        let p = self.pending[i].as_mut().unwrap();
        p.phase = DeliveryState::Delivered;
        p.command = None;
        p.quota = None;
        let sender = p.sender.take();
        if p.abandoned {
            self.pending[i] = None;
        }
        if let Some(sender) = sender {
            self.notice(Notice::Query(sender, result));
        }
    }
    fn fail(&mut self) {
        self.phase = Phase::Closed;
        self.notify_outbound = true;
        if let Some(sender) = self.opened.take() {
            self.notice(Notice::Open(sender, Err(Error::Closed)));
        }
        for i in 0..self.pending.len() {
            if let Some(mut p) = self.pending[i].take() {
                if let Some(sender) = p.sender.take() {
                    self.notice(Notice::Query(sender, Err(Error::Closed)));
                }
            }
        }
        self.operations = std::array::from_fn(|_| None);
        self.controls = std::array::from_fn(|_| None);
    }
    fn begin_close(&mut self) -> Result<()> {
        if matches!(self.phase, Phase::Closed | Phase::Closing) {
            return Ok(());
        }
        if self.phase != Phase::Ready {
            self.fail();
            return Ok(());
        }
        self.fail();
        self.phase = Phase::Closing;
        self.control(Purpose::Close)
    }
    fn outbound(&mut self) -> Result<Option<Outbound>> {
        if self.phase == Phase::Closed {
            return Ok(None);
        }
        loop {
            let control = self
                .controls
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    c.as_ref()
                        .filter(|c| c.sent.is_none())
                        .map(|c| (i, c.order))
                })
                .min_by_key(|(_, order)| *order)
                .map(|(i, _)| i);
            let data = if control.is_none() {
                self.pending
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| {
                        p.as_ref()
                            .filter(|p| p.phase == DeliveryState::Queued)
                            .map(|p| (i, p.ticket))
                    })
                    .min_by_key(|(_, ticket)| *ticket)
                    .map(|(i, _)| i)
            } else {
                None
            };
            if control.is_none() && data.is_none() {
                return Ok(None);
            }
            self.next_wire = self.next_wire.checked_add(1).ok_or(Error::ResourceLimit)?;
            let id = self.next_wire;
            let snapshot = self.info.as_ref().map(|i| i.snapshot);
            if let Some(i) = control {
                let c = self.controls[i].as_mut().unwrap();
                let body = match c.purpose {
                    Purpose::Hello => RequestBody::Hello,
                    Purpose::Open => RequestBody::Open,
                    Purpose::Cancel(id) => RequestBody::Cancel(id),
                    Purpose::Release(c) => RequestBody::Release(c),
                    Purpose::Close => RequestBody::Close,
                };
                let bytes = wire_encode::request(id, snapshot, &body, &self.control_budget)?;
                if bytes.bytes().len() > self.limits.max_request_bytes as usize {
                    return Err(Error::ResourceLimit);
                }
                c.sent = Some(id);
                return Ok(Some(Outbound {
                    incarnation: self.incarnation,
                    request_id: id,
                    bytes,
                }));
            }
            let i = data.unwrap();
            let p = self.pending[i].as_mut().unwrap();
            let body = p.command.take().unwrap();
            let encoded =
                wire_encode::request(id, snapshot, &body, &self.control_budget).and_then(|b| {
                    if b.bytes().len() > self.limits.max_request_bytes as usize {
                        Err(Error::ResourceLimit)
                    } else {
                        Ok(b)
                    }
                });
            match encoded {
                Ok(bytes) => {
                    p.phase = DeliveryState::Sent;
                    p.sent = Some(id);
                    return Ok(Some(Outbound {
                        incarnation: self.incarnation,
                        request_id: id,
                        bytes,
                    }));
                }
                Err(error) => {
                    let key = p.operation;
                    self.complete(i, Err(error));
                    self.remove_operation(key)?;
                }
            }
        }
    }
    fn receive(&mut self, bytes: &[u8]) -> Result<()> {
        let id = wire_reply::request_id(bytes)?;
        if let Some(i) = self
            .controls
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| c.sent == Some(id)))
        {
            if bytes.len() > CONTROL_FRAME {
                return Err(Error::ResourceLimit);
            }
            if crate::wire_admission::decoded_bytes(
                crate::wire_admission::Schema::ReplyEnvelope,
                bytes,
            )? > CONTROL_DECODED
            {
                return Err(Error::ResourceLimit);
            }
            let response = wire_reply::decode(bytes, &self.control_budget)?;
            // Control parsing never installs payload-bearing replies. Its
            // decoded ceiling is checked before allocation by a separate budget.
            let purpose = self.controls[i].take().unwrap().purpose;
            return self.control_reply(purpose, response);
        }
        let Some(i) = self.pending.iter().position(|p| {
            p.as_ref()
                .is_some_and(|p| p.sent == Some(id) && p.phase == DeliveryState::Sent)
        }) else {
            return if self.phase == Phase::Closing {
                Ok(())
            } else {
                Err(Error::Frame("unsolicited or duplicate reply"))
            };
        };
        if bytes.len() > self.limits.max_reply_bytes as usize {
            return Err(Error::ResourceLimit);
        }
        let quota = self.pending[i].as_mut().unwrap().quota.take().unwrap();
        let response =
            wire_reply::decode_reserved(bytes, quota, self.limits.max_decoded_bytes as usize)?;
        if response.snapshot != self.info.as_ref().map(|i| i.snapshot) {
            return Err(Error::Frame("reply belongs to another snapshot"));
        }
        let p = self.pending[i].as_ref().unwrap();
        let key = p.operation;
        let cancelled = p.cancelled;
        let expected = p.cursor;
        let limits = p.limits;
        match response.body() {
            wire_reply::Body::Delivery(d) => {
                if expected.is_some_and(|c| d.request != c)
                    || expected.is_none() && d.request.step != 0
                {
                    return Err(Error::Frame("reply has wrong continuation"));
                }
                if self.operations.iter().flatten().any(|o| {
                    o.key != key
                        && o.last
                            .as_ref()
                            .is_some_and(|last| last.request.operation == d.request.operation)
                }) {
                    return Err(Error::Frame("operation identity reused"));
                }
                if cancelled {
                    self.control(Purpose::Release(d.request))?;
                    self.remove_operation(key)?;
                    self.complete(i, Err(Error::Cancelled));
                } else {
                    let op = self
                        .operations
                        .iter_mut()
                        .flatten()
                        .find(|o| o.key == key)
                        .ok_or(Error::Frame("reply has no pending operation"))?;
                    validate(op, d, limits, self.info.as_ref().unwrap())?;
                    op.last = Some(d.clone());
                    op.next = d.next;
                    self.complete(i, Ok(d.clone()));
                }
            }
            wire_reply::Body::Failure { code, .. } => {
                if cancelled || *code != Code::ResourceLimit || expected.is_none() {
                    self.remove_operation(key)?;
                }
                self.complete(
                    i,
                    Err(if cancelled {
                        Error::Cancelled
                    } else {
                        remote_error(*code)
                    }),
                );
                if *code == Code::Stale {
                    self.fail();
                }
            }
            _ => return Err(Error::Frame("wrong reply family for data request")),
        }
        Ok(())
    }
    fn control_reply(&mut self, purpose: Purpose, response: wire_reply::Response) -> Result<()> {
        match (purpose, response.body()) {
            (Purpose::Hello, wire_reply::Body::Welcome(limits)) if self.phase == Phase::Hello => {
                self.limits = *limits;
                self.phase = Phase::Open;
                self.control(Purpose::Open)?;
            }
            (Purpose::Open, wire_reply::Body::Opened { info, operations })
                if self.phase == Phase::Open =>
            {
                let info = Arc::new(info.clone());
                self.info = Some(info.clone());
                self.phase = Phase::Ready;
                self.capabilities = operations
                    .iter()
                    .fold(0, |bits, op| bits | (1 << (*op as u8)));
                if let Some(sender) = self.opened.take() {
                    self.notice(Notice::Open(sender, Ok(info)));
                }
            }
            (
                Purpose::Cancel(_) | Purpose::Release(_) | Purpose::Close,
                wire_reply::Body::Acknowledged,
            ) => {
                if response.snapshot != self.info.as_ref().map(|i| i.snapshot) {
                    return Err(Error::Frame("control reply has wrong snapshot"));
                }
                if matches!(purpose, Purpose::Close) {
                    self.fail();
                }
            }
            (_, wire_reply::Body::Failure { code, .. }) => {
                if let Some(sender) = self.opened.take() {
                    self.notice(Notice::Open(sender, Err(remote_error(*code))));
                }
                return Err(Error::Frame("remote control request failed"));
            }
            _ => return Err(Error::Frame("wrong reply family for control request")),
        }
        Ok(())
    }
}
fn remote_error(code: Code) -> Error {
    match code {
        Code::ResourceLimit => Error::ResourceLimit,
        Code::Cancelled => Error::Cancelled,
        Code::Stale => Error::Closed,
        Code::Unsupported => Error::Invalid("remote query unsupported"),
        _ => Error::Invalid("remote query failed"),
    }
}
fn validate(op: &mut Operation, d: &Delivery, limits: Limits, info: &SessionInfo) -> Result<()> {
    let invalid = || Error::Frame("reply does not match requested coverage");
    match (op.shape, &d.reply) {
        (Shape::Window(range), Reply::Window(p)) => {
            if p.interval != range || p.changes().len() > limits.records {
                return Err(invalid());
            }
            if let Some(first) = &op.predecessor {
                if !wire_reply::same_sample(&first.sample, &p.predecessor.sample) {
                    return Err(invalid());
                }
            } else {
                op.predecessor = Some(p.predecessor.clone());
            }
            if p.changes()
                .first()
                .is_some_and(|c| op.last_time.is_some_and(|last| c.time < last))
            {
                return Err(invalid());
            }
            if let Some(last) = p.changes().last() {
                op.last_time = Some(last.time);
            }
        }
        (Shape::Summary(grid), Reply::Summary(p)) => {
            if p.grid != grid || u64::from(p.offset) != op.offset || p.bins().len() > limits.records
            {
                return Err(invalid());
            }
            for bin in p.bins() {
                if let Some(exit) = &op.exit {
                    if !wire_reply::same_sample(exit, &bin.entry) {
                        return Err(invalid());
                    }
                }
                op.exit = Some(bin.exit.clone());
            }
            op.offset = op
                .offset
                .checked_add(p.bins().len() as u64)
                .ok_or_else(invalid)?;
        }
        (Shape::Children(parent), Reply::Children(p)) => {
            if p.parent != parent || p.offset != op.offset {
                return Err(invalid());
            }
            nodes(op, p.declarations(), limits, info)?;
            op.offset = op
                .offset
                .checked_add(p.declarations().len() as u64)
                .ok_or_else(invalid)?;
        }
        (Shape::Search, Reply::Search(p)) => nodes(op, p.declarations(), limits, info)?,
        (Shape::Resolve(count), Reply::Resolve(p)) => {
            if p.offset as u64 != op.offset || p.results().len() > limits.records {
                return Err(invalid());
            }
            op.offset = op
                .offset
                .checked_add(p.results().len() as u64)
                .ok_or_else(invalid)?;
            if op.offset > count as u64 || p.complete != (op.offset == count as u64) {
                return Err(invalid());
            }
            if p.results().iter().any(|r|matches!(r,crate::metadata::Resolution::Found(id) if u64::from(*id)>=info.declarations)){return Err(invalid());}
        }
        (Shape::Text(id, offset, length), Reply::Text(p)) => {
            if p.id != id
                || p.offset != offset
                || p.bytes.as_slice().len() as u64
                    != (length as u64).min(p.total_bytes.saturating_sub(offset))
            {
                return Err(invalid());
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}
fn nodes(
    op: &mut Operation,
    nodes: &[crate::metadata::Declaration],
    limits: Limits,
    info: &SessionInfo,
) -> Result<()> {
    if nodes.len() > limits.records {
        return Err(Error::Frame("too many declarations"));
    }
    for node in nodes {
        if u64::from(node.id) >= info.declarations
            || op.last_node.is_some_and(|last| node.id <= last)
        {
            return Err(Error::Frame("declaration order or identity changed"));
        }
        if node
            .parent
            .is_some_and(|id| u64::from(id) >= info.declarations)
        {
            return Err(Error::Frame("invalid parent declaration"));
        }
        if matches!(&node.data,crate::metadata::DeclarationData::Variable {signal,..} if *signal>=info.signals)
        {
            return Err(Error::Frame("invalid signal identity"));
        }
        op.last_node = Some(node.id);
    }
    Ok(())
}

impl crate::session::AsyncSession for RpcSession {
    type Task = RpcQueryFuture;
    fn info(&self) -> &SessionInfo {
        RpcSession::info(self)
    }
    fn execute(&self, query: Query, limits: Limits) -> Result<Self::Task> {
        RpcSession::execute(self, query, limits)
    }
    fn advance(&self, cursor: Continuation) -> Result<Self::Task> {
        RpcSession::advance(self, cursor)
    }
    fn cancel(&self, task: &Self::Task) {
        task.cancellation().cancel();
    }
    fn release(&self, cursor: Continuation) -> Result<()> {
        RpcSession::release(self, cursor)
    }
    fn close(&self) {
        RpcSession::close(self);
    }
}
