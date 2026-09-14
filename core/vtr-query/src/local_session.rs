//! Asynchronous in-process execution. Opening, reader work and destruction run
//! on one session worker. Futures only poll an admitted result slot; native
//! requests and replies never pass through a wire codec.
use crate::native_session::Session;
use crate::session::{Continuation, Delivery, Query, SessionInfo};
use crate::wave::Limits;
use crate::{Budget, Cancellation, Error, Reservation, Result};
use futures_channel::oneshot;
use std::{
    collections::VecDeque,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    task::{Context, Poll},
};

struct CancelState {
    requested: Cancellation,
    running: Mutex<Option<Cancellation>>,
    _charge: Reservation,
}
/// A cancellation handle remains usable while a worker is executing a request.
/// It owns no delivery credit and does not wait for reader work to stop.
#[derive(Clone)]
pub struct CancelHandle(Arc<CancelState>);
impl CancelHandle {
    pub fn cancel(&self) {
        self.0.requested.cancel();
        if let Some(token) = &*self.0.running.lock().unwrap() {
            token.cancel();
        }
    }
    fn link(&self, token: Cancellation) {
        let mut running = self.0.running.lock().unwrap();
        if self.0.requested.check().is_err() {
            token.cancel();
        }
        *running = Some(token);
    }
}
struct Ticket {
    cancel: CancelHandle,
    shared: Weak<Shared>,
    _charge: Reservation,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            shared.outstanding.fetch_sub(1, Ordering::AcqRel);
            shared.notify_progress();
        }
    }
}
enum Command {
    Start(Query, Limits),
    Next(Continuation),
}
struct Job {
    command: Command,
    result: oneshot::Sender<Result<Arc<Delivery>>>,
    ticket: Arc<Ticket>,
}
struct Queue {
    jobs: VecDeque<Job>,
    releases: Vec<Continuation>,
    closed: bool,
    running: Option<CancelHandle>,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    budget: Budget,
    capacity: usize,
    outstanding: AtomicUsize,
    stopped: AtomicBool,
    progress: Mutex<Option<std::task::Waker>>,
    _charge: Reservation,
}
impl Shared {
    fn notify_progress(&self) {
        let wake = self.progress.lock().unwrap().take();
        if let Some(wake) = wake {
            wake.wake();
        }
    }

    fn close(&self) {
        let mut queue = self.queue.lock().unwrap();
        queue.closed = true;
        if let Some(running) = &queue.running {
            running.cancel();
        }
        for job in &queue.jobs {
            job.ticket.cancel.cancel();
        }
        self.ready.notify_one();
    }
    fn release(&self, cursor: Continuation) -> Result<()> {
        let mut queue = self.queue.lock().unwrap();
        if queue.closed {
            return Err(Error::Closed);
        }
        if queue
            .releases
            .iter()
            .any(|c| c.snapshot == cursor.snapshot && c.operation == cursor.operation)
        {
            return Ok(());
        }
        if queue.releases.len() == self.capacity {
            return Err(Error::ResourceLimit);
        }
        queue.releases.push(cursor);
        self.ready.notify_one();
        Ok(())
    }
    fn discard(&self, cursor: Continuation) {
        // Releasing abandoned results must never depend on data-queue credit.
        // If the reserved control queue is misused or exhausted, closing the
        // host releases every operation instead of leaking an unreachable slot.
        if self.release(cursor).is_err() {
            self.close();
        }
    }
}

pub struct LocalSession {
    info: SessionInfo,
    shared: Arc<Shared>,
}
impl LocalSession {
    /// Starts opening a VTR file on a dedicated worker. No file I/O occurs here
    /// or in OpenFuture::poll. The caller's path storage is admitted at transfer.
    pub fn open(path: PathBuf, budget: Budget, capacity: usize) -> Result<OpenFuture> {
        Self::open_reader(budget, capacity, path.capacity(), move || {
            vtr::Reader::open(path).map_err(|error| Error::Backend(error.to_string()))
        })
    }
    /// Share an already-open immutable reader with a query worker. Reader storage
    /// remains owned by the caller and worker; query allocations use `budget`.
    pub fn from_reader(
        reader: Arc<vtr::Reader>,
        budget: Budget,
        capacity: usize,
    ) -> Result<OpenFuture> {
        Self::open_reader(budget, capacity, 0, move || Ok(reader))
    }
    fn open_reader<R: std::borrow::Borrow<vtr::Reader> + Send + 'static>(
        budget: Budget,
        capacity: usize,
        input_bytes: usize,
        open: impl FnOnce() -> Result<R> + Send + 'static,
    ) -> Result<OpenFuture> {
        if capacity == 0 {
            return Err(Error::Invalid("session needs request capacity"));
        }
        let bytes = capacity
            .checked_mul(std::mem::size_of::<Job>() + std::mem::size_of::<Continuation>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Shared>() + 512))
            .ok_or(Error::ResourceLimit)?;
        let charge = budget.reserve(bytes)?;
        let path_charge =
            budget.reserve(input_bytes.checked_add(512).ok_or(Error::ResourceLimit)?)?;
        let mut jobs = VecDeque::new();
        jobs.try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        let mut releases = Vec::new();
        releases
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        if jobs.capacity() != capacity || releases.capacity() != capacity {
            return Err(Error::ResourceLimit);
        }
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                jobs,
                releases,
                closed: false,
                running: None,
            }),
            ready: Condvar::new(),
            budget,
            capacity,
            outstanding: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            progress: Mutex::new(None),
            _charge: charge,
        });
        let (tx, rx) = oneshot::channel();
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("vtr-query".into())
            .spawn(move || {
                let _stopped = Stopped(worker_shared.clone());
                if worker_shared.queue.lock().unwrap().closed {
                    return;
                }
                let reader = match open() {
                    Ok(reader) => reader,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return;
                    }
                };
                drop(path_charge);
                let session =
                    match Session::new(reader.borrow(), worker_shared.budget.clone(), capacity) {
                        Ok(session) => session,
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            return;
                        }
                    };
                if tx.send(Ok(session.info().clone())).is_ok() {
                    run(session, &worker_shared);
                }
            })
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(OpenFuture {
            receiver: rx,
            shared: Some(shared),
        })
    }
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }
    pub fn execute(&self, query: Query, limits: Limits) -> Result<QueryFuture> {
        self.submit(Command::Start(query, limits))
    }
    pub fn advance(&self, cursor: Continuation) -> Result<QueryFuture> {
        self.validate(cursor)?;
        self.submit(Command::Next(cursor))
    }
    pub fn release(&self, cursor: Continuation) -> Result<()> {
        self.validate(cursor)?;
        self.shared.release(cursor)
    }
    pub fn close(&self) {
        self.shared.close();
    }
    pub fn is_stopped(&self) -> bool {
        self.shared.stopped.load(Ordering::Acquire)
    }
    fn validate(&self, cursor: Continuation) -> Result<()> {
        if cursor.snapshot != self.info.snapshot {
            return Err(Error::Invalid("continuation belongs to another snapshot"));
        }
        Ok(())
    }
    fn submit(&self, command: Command) -> Result<QueryFuture> {
        let input = match &command {
            Command::Start(query, _) => input_bytes(query)?,
            Command::Next(_) => 0,
        };
        // Includes the fixed oneshot/control objects, whose implementation
        // overhead is independent of trace size and separately measured in RSS.
        let charge = self
            .shared
            .budget
            .reserve(input.checked_add(512).ok_or(Error::ResourceLimit)?)?;
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.closed {
            return Err(Error::Closed);
        }
        self.shared
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.shared.capacity).then_some(n + 1)
            })
            .map_err(|_| Error::ResourceLimit)?;
        let completion_charge = charge.clone();
        let ticket = Arc::new(Ticket {
            cancel: CancelHandle(Arc::new(CancelState {
                requested: Cancellation::default(),
                running: Mutex::new(None),
                _charge: charge.clone(),
            })),
            shared: Arc::downgrade(&self.shared),
            _charge: charge,
        });
        let abandoned_cursor = match command {
            Command::Next(c) => Some(c),
            _ => None,
        };
        let (tx, rx) = oneshot::channel();
        queue.jobs.push_back(Job {
            command,
            result: tx,
            ticket: ticket.clone(),
        });
        self.shared.ready.notify_one();
        Ok(QueryFuture {
            receiver: rx,
            ticket: Some(ticket),
            shared: self.shared.clone(),
            abandoned_cursor,
            _completion_charge: completion_charge,
        })
    }
}
impl Drop for LocalSession {
    fn drop(&mut self) {
        self.shared.close();
    }
}

pub struct OpenFuture {
    receiver: oneshot::Receiver<Result<SessionInfo>>,
    shared: Option<Arc<Shared>>,
}
impl Future for OpenFuture {
    type Output = Result<LocalSession>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(r) => r.unwrap_or(Err(Error::Closed)),
        };
        Poll::Ready(result.map(|info| LocalSession {
            info,
            shared: self.shared.take().unwrap(),
        }))
    }
}
impl Drop for OpenFuture {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.close();
        }
    }
}

pub struct QueryFuture {
    receiver: oneshot::Receiver<Result<Arc<Delivery>>>,
    ticket: Option<Arc<Ticket>>,
    shared: Arc<Shared>,
    abandoned_cursor: Option<Continuation>,
    _completion_charge: Reservation,
}
impl QueryFuture {
    pub fn cancellation(&self) -> CancelHandle {
        self.ticket
            .as_ref()
            .expect("completed query future")
            .cancel
            .clone()
    }
}
impl Future for QueryFuture {
    type Output = Result<Arc<Delivery>>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = match Pin::new(&mut self.receiver).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(r) => r.unwrap_or(Err(Error::Closed)),
        };
        self.ticket.take();
        self.abandoned_cursor = None;
        Poll::Ready(result)
    }
}
impl Drop for QueryFuture {
    fn drop(&mut self) {
        if let Some(ticket) = &self.ticket {
            ticket.cancel.cancel();
            self.receiver.close();
            if let Ok(Some(Ok(delivery))) = self.receiver.try_recv() {
                self.shared.discard(delivery.request);
            }
            if let Some(cursor) = self.abandoned_cursor {
                self.shared.discard(cursor);
            }
        }
    }
}
struct Stopped(Arc<Shared>);
impl Drop for Stopped {
    fn drop(&mut self) {
        self.0.close();
        let jobs = {
            let mut queue = self.0.queue.lock().unwrap();
            queue.running = None;
            std::mem::take(&mut queue.jobs)
        };
        drop(jobs); // Dropped senders wake pending futures with Closed.
        self.0.stopped.store(true, Ordering::Release);
    }
}
fn input_bytes(query: &Query) -> Result<usize> {
    let mut bytes = 0usize;
    let mut add = |n| {
        bytes = bytes.checked_add(n).ok_or(Error::ResourceLimit)?;
        Ok(())
    };
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
                for segment in &path.segments {
                    add(segment.capacity())?;
                }
            }
        }
        _ => (),
    }
    Ok(bytes)
}
enum Work {
    Release(Continuation),
    Job(Job),
}
fn run(mut session: Session<'_>, shared: &Shared) {
    loop {
        let work = {
            let mut queue = shared.queue.lock().unwrap();
            loop {
                if queue.closed {
                    return;
                }
                if let Some(cursor) = queue.releases.pop() {
                    break Work::Release(cursor);
                }
                if let Some(job) = queue.jobs.pop_front() {
                    queue.running = Some(job.ticket.cancel.clone());
                    break Work::Job(job);
                }
                queue = shared.ready.wait(queue).unwrap();
            }
        };
        let job = match work {
            Work::Release(cursor) => {
                let _ = session.release(cursor);
                shared.notify_progress();
                continue;
            }
            Work::Job(job) => job,
        };
        let cancellation = job.ticket.cancel.clone();
        let mut cursor = match &job.command {
            Command::Next(c) => Some(*c),
            _ => None,
        };
        let result = (|| {
            cancellation.0.requested.check()?;
            let start = matches!(&job.command, Command::Start(..));
            let c = match job.command {
                Command::Start(query, limits) => {
                    let token = Cancellation::default();
                    cancellation.link(token.clone());
                    session.start(query, limits, token)?
                }
                Command::Next(c) => {
                    cancellation.link(session.cancellation(c)?);
                    c
                }
            };
            cursor = Some(c);
            let result = session.advance(c);
            if result.is_err() && (start || cancellation.0.requested.check().is_err()) {
                let _ = session.release(c);
            }
            result
        })();
        if cancellation.0.requested.check().is_err() {
            if let Some(cursor) = cursor {
                let _ = session.release(cursor);
            }
        }
        *cancellation.0.running.lock().unwrap() = None;
        shared.queue.lock().unwrap().running = None;
        // Leave credit with the receiver before waking it, so an immediately
        // awaited continuation can use the just-consumed request slot.
        drop(job.ticket);
        if job.result.send(result).is_err() {
            if let Some(cursor) = cursor {
                let _ = session.release(cursor);
            }
        }
    }
}

impl crate::session::AsyncSession for LocalSession {
    fn register_progress_waker(&self, waker: &std::task::Waker) {
        let mut registered = self.shared.progress.lock().unwrap();
        if registered.as_ref().is_none_or(|old| !old.will_wake(waker)) {
            *registered = Some(waker.clone());
        }
    }

    type Task = QueryFuture;
    fn info(&self) -> &SessionInfo {
        LocalSession::info(self)
    }
    fn execute(&self, query: Query, limits: Limits) -> Result<Self::Task> {
        LocalSession::execute(self, query, limits)
    }
    fn advance(&self, cursor: Continuation) -> Result<Self::Task> {
        LocalSession::advance(self, cursor)
    }
    fn cancel(&self, task: &Self::Task) {
        task.cancellation().cancel();
    }
    fn release(&self, cursor: Continuation) -> Result<()> {
        LocalSession::release(self, cursor)
    }
    fn close(&self) {
        LocalSession::close(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_poll_and_drop_do_not_execute_or_join_blocked_reader_work() {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let budget = Budget::new(1 << 20);
        let mut opening = LocalSession::open_reader(budget.clone(), 1, 0, move || {
            entered_tx.send(std::thread::current().id()).unwrap();
            release_rx.recv().unwrap();
            Err::<vtr::Reader, _>(Error::Invalid("test open failure"))
        })
        .unwrap();
        let worker = entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_ne!(worker, std::thread::current().id());
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(Pin::new(&mut opening).poll(&mut cx).is_pending());
        drop(opening);
        // A blocking Drop would deadlock before this release can be sent.
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while budget.used() != 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
