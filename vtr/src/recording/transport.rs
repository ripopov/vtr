use std::{
    cell::Cell,
    collections::VecDeque,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    marker::PhantomData,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use super::{
    Backend, Ending, Operation, ProducerId, Recorder, WriterOptions, WriterStats,
    capture::{Capture, Frontend},
    spool,
};
use crate::{
    Error, Result,
    attributes::Value,
    framing::{Fingerprint, RecordingId},
};

#[derive(Default)]
struct Admission {
    bytes: usize,
    failed: bool,
}
struct Budget {
    capacity: usize,
    state: Mutex<Admission>,
    changed: Condvar,
    waits: AtomicU64,
    wait_nanoseconds: AtomicU64,
}
impl Budget {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::default(),
            changed: Condvar::new(),
            waits: AtomicU64::new(0),
            wait_nanoseconds: AtomicU64::new(0),
        }
    }
    fn acquire(&self, bytes: usize) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        while !state.failed && state.bytes != 0 && bytes > self.capacity.saturating_sub(state.bytes)
        {
            self.waits.fetch_add(1, Ordering::Relaxed);
            let began = std::time::Instant::now();
            state = self.changed.wait(state).unwrap();
            self.wait_nanoseconds.fetch_add(
                u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
        if state.failed {
            return Err(Error::State("recording worker failed"));
        }
        state.bytes = state
            .bytes
            .checked_add(bytes)
            .ok_or(Error::invalid("capture byte count overflow"))?;
        Ok(())
    }
    fn release(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        state.bytes -= bytes;
        self.changed.notify_all();
    }
    fn fail(&self) {
        self.state.lock().unwrap().failed = true;
        self.changed.notify_all();
    }
}

type Control = Box<dyn FnOnce(&mut Backend, Option<String>) + Send>;
enum Command {
    Operation {
        producer: ProducerId,
        stamp: u64,
        operation: Operation,
        bytes: usize,
        budget: Arc<Budget>,
    },
    Control(Control),
    AddProducer(mpsc::Sender<Result<(ProducerId, u64)>>),
    ProducerDone(ProducerId, mpsc::Sender<Result<()>>),
    Flush(mpsc::Sender<Result<()>>),
    Finish {
        incomplete: bool,
        ending: Option<(Ending, u64)>,
        rescue: bool,
        reply: mpsc::Sender<Result<Fingerprint>>,
    },
    Stats(mpsc::Sender<WriterStats>),
    Shutdown,
}

struct Shared {
    sender: mpsc::Sender<Command>,
    failure: Mutex<Option<String>>,
    budgets: Mutex<Vec<Arc<Budget>>>,
    capacity: usize,
    final_stats: Mutex<WriterStats>,
    closed: Mutex<bool>,
    ids: Arc<AtomicU64>,
    submitted: AtomicU64,
    completed: AtomicU64,
    queued_bytes: AtomicUsize,
    queue_peak_bytes: AtomicUsize,
}
impl Shared {
    fn error(&self) -> Result<()> {
        if *self.closed.lock().unwrap() {
            return Err(Error::State("recording is finalized"));
        }
        self.failure()
    }
    fn failure(&self) -> Result<()> {
        if let Some(failure) = &*self.failure.lock().unwrap() {
            Err(Error::Recording(failure.clone()))
        } else {
            Ok(())
        }
    }
    fn fail(&self, error: impl ToString) {
        self.failure
            .lock()
            .unwrap()
            .get_or_insert_with(|| error.to_string());
        for budget in self.budgets.lock().unwrap().iter() {
            budget.fail();
        }
    }
    fn control<R: Send + 'static>(
        self: &Arc<Self>,
        action: impl FnOnce(&mut Backend) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.error()?;
        let (sender, receiver) = mpsc::channel();
        let shared = self.clone();
        let closed = self.closed.lock().unwrap();
        if *closed {
            return Err(Error::State("recording is finalized"));
        }
        self.sender
            .send(Command::Control(Box::new(move |backend, failure| {
                let result = match failure {
                    Some(error) => Err(Error::Recording(error)),
                    None => action(backend),
                };
                if let Err(error) = &result {
                    shared.fail(error);
                }
                drop(sender.send(result));
            })))
            .map_err(|_| Error::State("recording worker stopped"))?;
        drop(closed);
        receiver
            .recv()
            .map_err(|_| Error::State("recording control call failed"))?
    }
    fn publish(
        &self,
        producer: ProducerId,
        stamp: u64,
        operation: Operation,
        budget: &Arc<Budget>,
    ) -> Result<()> {
        self.error()?;
        let bytes = charge(&operation);
        budget.acquire(bytes)?;
        let closed = self.closed.lock().unwrap();
        if *closed {
            budget.release(bytes);
            return Err(Error::State("recording is finalized"));
        }
        let queued = self.queued_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.queue_peak_bytes.fetch_max(queued, Ordering::Relaxed);
        self.submitted.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .send(Command::Operation {
                producer,
                stamp,
                operation,
                bytes,
                budget: budget.clone(),
            })
            .is_err()
        {
            self.submitted.fetch_sub(1, Ordering::Relaxed);
            self.queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
            budget.release(bytes);
            return Err(Error::State("recording worker stopped"));
        }
        drop(closed);
        self.failure()
    }
}

#[derive(Clone)]
pub(super) struct Rescue {
    shared: Arc<Shared>,
}
impl Rescue {
    pub fn seal(&self, ending: Ending) -> Result<Fingerprint> {
        let (reply, done) = mpsc::channel();
        {
            let mut closed = self.shared.closed.lock().unwrap();
            if *closed {
                return Err(Error::State("recording is finalized"));
            }
            let id = self
                .shared
                .ids
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                    (id != 0).then(|| id.wrapping_add(1))
                })
                .map_err(|_| Error::State("record identifiers exhausted"))?;
            *closed = true;
            for budget in self.shared.budgets.lock().unwrap().iter() {
                budget.fail();
            }
            // The same gate orders accepted captures before this finalization command.
            self.shared
                .sender
                .send(Command::Finish {
                    incomplete: true,
                    ending: (ending != Ending::Closed).then_some((ending, id)),
                    rescue: true,
                    reply,
                })
                .map_err(|_| Error::State("recording worker stopped"))?;
        }
        done.recv()
            .map_err(|_| Error::State("recording rescue failed"))?
    }
}

pub(super) struct Transport {
    shared: Arc<Shared>,
    implicit: Arc<Budget>,
    worker: Option<JoinHandle<()>>,
    cpu: super::cpu::WorkerCpu,
}

impl Transport {
    pub fn new(
        backend: Backend,
        options: &WriterOptions,
        _: RecordingId,
        ids: Arc<AtomicU64>,
    ) -> Result<Self> {
        if cfg!(target_family = "wasm") {
            return Err(Error::invalid(
                "threaded recording is unavailable on this target",
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let implicit = Arc::new(Budget::new(options.buffer.queue_bytes));
        let shared = Arc::new(Shared {
            sender,
            failure: Mutex::default(),
            budgets: Mutex::new(vec![implicit.clone()]),
            capacity: options.buffer.queue_bytes,
            final_stats: Mutex::default(),
            closed: Mutex::new(false),
            ids,
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            queued_bytes: AtomicUsize::new(0),
            queue_peak_bytes: AtomicUsize::new(0),
        });
        let worker_shared = shared.clone();
        let pending_bytes = options.buffer.pending_bytes;
        let cpu = super::cpu::WorkerCpu::default();
        let worker_cpu = cpu.clone();
        let worker = std::thread::Builder::new()
            .name("vtr-recording".into())
            .spawn(move || {
                let _measurement = worker_cpu.measure();
                let mut worker = Worker::new(backend, pending_bytes, worker_shared.clone());
                loop {
                    if worker_shared.failure().is_ok() {
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            worker.backend.poll_output()
                        })) {
                            Ok(Ok(())) => (),
                            Ok(Err(error)) => worker_shared.fail(error),
                            Err(_) => {
                                worker.panicked = true;
                                worker_shared.fail("recording output panicked");
                            }
                        }
                    }
                    let command = match receiver.recv_timeout(Duration::from_millis(10)) {
                        Ok(command) => command,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if matches!(command, Command::Shutdown) {
                        worker_shared.fail("recording owner was dropped");
                        break;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        worker.command(command)
                    }));
                    match result {
                        Ok(Ok(finished)) => {
                            if finished {
                                break;
                            }
                        }
                        Ok(Err(error)) => worker_shared.fail(error),
                        Err(_) => {
                            worker.panicked = true;
                            worker_shared.fail("recording worker panicked");
                        }
                    }
                }
            })?;
        Ok(Self {
            shared,
            implicit,
            worker: Some(worker),
            cpu,
        })
    }
    pub fn worker_cpu(&self) -> super::cpu::WorkerCpu {
        self.cpu.clone()
    }
    pub fn usable(&self) -> Result<()> {
        self.shared.error()
    }
    pub fn rescue(&self) -> Rescue {
        Rescue {
            shared: self.shared.clone(),
        }
    }
    pub fn publish(&self, producer: ProducerId, stamp: u64, operation: Operation) -> Result<()> {
        self.shared
            .publish(producer, stamp, operation, &self.implicit)
    }
    pub fn control<R: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Backend) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.shared.control(action)
    }
    pub fn add_producer(&self, origin: RecordingId, ids: Arc<AtomicU64>) -> Result<Producer> {
        self.shared.error()?;
        let (sender, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(Command::AddProducer(sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        let (producer, time) = receiver
            .recv()
            .map_err(|_| Error::State("producer registration failed"))??;
        let budget = self.shared.budgets.lock().unwrap()[producer.0 as usize].clone();
        Ok(Producer {
            shared: self.shared.clone(),
            budget,
            front: Frontend {
                origin,
                ids,
                time,
                producer,
                finished: false,
            },
            marker: PhantomData,
        })
    }
    pub fn flush(&self) -> Result<()> {
        self.shared.error()?;
        let (sender, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(Command::Flush(sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        receiver
            .recv()
            .map_err(|_| Error::State("recording flush failed"))?
    }
    pub fn finish_producer(&self, id: ProducerId) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(Command::ProducerDone(id, sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        receiver
            .recv()
            .map_err(|_| Error::State("producer finalization failed"))?
    }
    pub fn stats(&self) -> Result<WriterStats> {
        if self.worker.is_none() || *self.shared.closed.lock().unwrap() {
            return Ok(*self.shared.final_stats.lock().unwrap());
        }
        let (sender, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(Command::Stats(sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        receiver
            .recv()
            .map_err(|_| Error::State("recording statistics unavailable"))
    }
    pub fn finish(
        &mut self,
        incomplete: bool,
        ending: Option<(Ending, u64)>,
    ) -> Result<Fingerprint> {
        if *self.shared.closed.lock().unwrap() {
            return Err(Error::State("recording is finalized"));
        }
        let (sender, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(Command::Finish {
                incomplete,
                ending,
                rescue: false,
                reply: sender,
            })
            .map_err(|_| Error::State("recording worker stopped"))?;
        let result = receiver
            .recv()
            .map_err(|_| Error::State("recording finalization failed"))?;
        if result.is_ok()
            && let Some(worker) = self.worker.take()
        {
            worker
                .join()
                .map_err(|_| Error::State("recording worker panicked"))?;
        }
        result
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            drop(self.shared.sender.send(Command::Shutdown));
            drop(worker.join());
        }
    }
}

/// Exclusive logical producer: it may move between threads, but cannot be shared concurrently.
pub struct Producer {
    shared: Arc<Shared>,
    budget: Arc<Budget>,
    front: Frontend,
    marker: PhantomData<Cell<()>>,
}
impl Producer {
    #[must_use]
    pub fn id(&self) -> ProducerId {
        self.front.producer
    }
    pub fn finish(&mut self) -> Result<()> {
        if self.front.finished {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        self.shared
            .sender
            .send(Command::ProducerDone(self.front.producer, sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        let result = receiver
            .recv()
            .map_err(|_| Error::State("producer finalization failed"))?;
        self.front.finished = true;
        result
    }
}
impl Capture for Producer {
    fn state(&self) -> &Frontend {
        &self.front
    }
    fn state_mut(&mut self) -> &mut Frontend {
        &mut self.front
    }
    fn dispatch(&mut self, operation: Operation) -> Result<()> {
        if self.front.finished {
            return Err(Error::State("producer is finished"));
        }
        self.shared.publish(
            self.front.producer,
            self.front.time,
            operation,
            &self.budget,
        )
    }
}
impl Recorder for Producer {
    fn intern(&mut self, name: &str) -> Result<u32> {
        if self.front.finished {
            return Err(Error::State("producer is finished"));
        }
        let name = name.to_owned();
        self.shared.control(move |backend| backend.intern(&name))
    }
}
impl Drop for Producer {
    fn drop(&mut self) {
        drop(self.finish());
    }
}

struct Pending {
    stamp: u64,
    sequence: u64,
    bytes: usize,
    operation: Operation,
}
#[derive(Clone, Copy)]
struct SpillHead {
    offset: u64,
    next: u64,
    stamp: u64,
    sequence: u64,
    length: u64,
    bytes: u64,
}
#[derive(Default)]
struct Queue {
    memory: VecDeque<Pending>,
    spill: Option<SpillHead>,
    tail: u64,
}
struct ProducerState {
    time: u64,
    finished: bool,
    queue: Queue,
}
struct SpillFile {
    file: File,
    path: PathBuf,
    length: u64,
}
impl SpillFile {
    fn new() -> Result<Self> {
        let id = RecordingId::generate()?;
        let name = id.to_string();
        let path = std::env::temp_dir().join(format!("vtr-pending-{name}"));
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            file,
            path,
            length: 0,
        })
    }
    fn append(&mut self, queue: &mut Queue, pending: Pending) -> Result<()> {
        let payload = spool::encode(&pending.operation)?;
        let offset = self.length;
        self.file.seek(SeekFrom::Start(offset))?;
        for value in [
            0,
            pending.stamp,
            pending.sequence,
            payload.len() as u64,
            pending.bytes as u64,
        ] {
            self.file.write_all(&value.to_le_bytes())?;
        }
        self.file.write_all(&payload)?;
        self.length += 40 + payload.len() as u64;
        let head = SpillHead {
            offset,
            next: 0,
            stamp: pending.stamp,
            sequence: pending.sequence,
            length: payload.len() as u64,
            bytes: pending.bytes as u64,
        };
        if let Some(first) = &mut queue.spill {
            self.file.seek(SeekFrom::Start(queue.tail))?;
            self.file.write_all(&offset.to_le_bytes())?;
            if first.offset == queue.tail {
                first.next = offset;
            }
        } else {
            queue.spill = Some(head);
        }
        queue.tail = offset;
        Ok(())
    }
    fn pop(&mut self, queue: &mut Queue) -> Result<Pending> {
        let head = queue.spill.take().unwrap();
        self.file.seek(SeekFrom::Start(head.offset + 40))?;
        let length = usize::try_from(head.length)
            .map_err(|_| Error::Corrupt("spool length exceeds usize"))?;
        if head.length > self.length - head.offset - 40 {
            return Err(Error::Corrupt("spool record exceeds file"));
        }
        let mut bytes = vec![0; length];
        self.file.read_exact(&mut bytes)?;
        if head.next != 0 {
            self.file.seek(SeekFrom::Start(head.next))?;
            let mut header = [0u8; 40];
            self.file.read_exact(&mut header)?;
            let mut cursor = crate::wire::Cursor::new(&header);
            queue.spill = Some(SpillHead {
                offset: head.next,
                next: cursor.u64()?,
                stamp: cursor.u64()?,
                sequence: cursor.u64()?,
                length: cursor.u64()?,
                bytes: cursor.u64()?,
            });
        }
        Ok(Pending {
            stamp: head.stamp,
            sequence: head.sequence,
            bytes: head.bytes as usize,
            operation: spool::decode(&bytes)?,
        })
    }
}
impl Drop for SpillFile {
    fn drop(&mut self) {
        drop(std::fs::remove_file(&self.path));
    }
}

struct Worker {
    backend: Backend,
    shared: Arc<Shared>,
    producers: Vec<ProducerState>,
    memory: usize,
    limit: usize,
    sequence: u64,
    spill: Option<SpillFile>,
    memory_peak: usize,
    spilled_bytes: u64,
    panicked: bool,
}
impl Worker {
    fn new(backend: Backend, limit: usize, shared: Arc<Shared>) -> Self {
        Self {
            backend,
            shared,
            producers: vec![ProducerState {
                time: 0,
                finished: false,
                queue: Queue::default(),
            }],
            memory: 0,
            limit,
            sequence: 0,
            spill: None,
            memory_peak: 0,
            spilled_bytes: 0,
            panicked: false,
        }
    }
    fn failure(&self) -> Option<String> {
        self.shared.failure.lock().unwrap().clone()
    }
    fn push(
        &mut self,
        producer: ProducerId,
        stamp: u64,
        operation: Operation,
        bytes: usize,
    ) -> Result<()> {
        let state = self
            .producers
            .get_mut(producer.0 as usize)
            .ok_or(Error::invalid("unknown producer"))?;
        if state.finished || stamp < state.time {
            return Err(Error::invalid("invalid producer timestamp or lifetime"));
        }
        if let Operation::Progress(time) = operation {
            if time != stamp {
                return Err(Error::invalid("invalid producer progress"));
            }
            state.time = time;
        }
        let pending = Pending {
            stamp,
            sequence: self.sequence,
            bytes,
            operation,
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::invalid("capture sequence exhausted"))?;
        if state.queue.spill.is_none() && bytes <= self.limit.saturating_sub(self.memory) {
            self.memory += bytes;
            self.memory_peak = self.memory_peak.max(self.memory);
            state.queue.memory.push_back(pending);
        } else {
            if self.spill.is_none() {
                self.spill = Some(SpillFile::new()?);
            }
            let spool = self.spill.as_mut().unwrap();
            let before = spool.length;
            spool.append(&mut state.queue, pending)?;
            self.spilled_bytes += spool.length - before;
        }
        self.drain(false)
    }
    fn drain(&mut self, all: bool) -> Result<()> {
        let mut active = self.producers.iter().filter(|producer| !producer.finished);
        let first = active.next().map(|producer| producer.time);
        let other: Vec<_> = active.map(|producer| producer.time).collect();
        let frontier = first.map(|first| other.iter().copied().fold(first, u64::min));
        let single = other.is_empty();
        loop {
            let selected = self
                .producers
                .iter()
                .enumerate()
                .filter_map(|(id, producer)| {
                    producer
                        .queue
                        .memory
                        .front()
                        .map(|head| (head.stamp, head.sequence, id))
                        .or_else(|| {
                            producer
                                .queue
                                .spill
                                .map(|head| (head.stamp, head.sequence, id))
                        })
                })
                .min();
            let Some((stamp, _, id)) = selected else {
                return Ok(());
            };
            if !all
                && frontier
                    .is_some_and(|frontier| stamp > frontier || (!single && stamp == frontier))
            {
                return Ok(());
            }
            let queue = &mut self.producers[id].queue;
            let pending = if let Some(pending) = queue.memory.pop_front() {
                self.memory -= pending.bytes;
                pending
            } else {
                self.spill.as_mut().unwrap().pop(queue)?
            };
            if self
                .producers
                .iter()
                .all(|producer| producer.queue.spill.is_none())
            {
                self.spill.take();
            }
            self.backend
                .apply(ProducerId(id as u32), pending.stamp, pending.operation)?;
        }
    }
    fn command(&mut self, command: Command) -> Result<bool> {
        match command {
            Command::Operation {
                producer,
                stamp,
                operation,
                bytes,
                budget,
            } => {
                self.shared.completed.fetch_add(1, Ordering::Relaxed);
                self.shared.queued_bytes.fetch_sub(bytes, Ordering::Relaxed);
                budget.release(bytes);
                if self.failure().is_none() {
                    self.push(producer, stamp, operation, bytes)?;
                }
            }
            Command::Control(action) => {
                let failure = self.failure();
                action(&mut self.backend, failure);
            }
            Command::AddProducer(reply) => {
                let result = if let Some(error) = self.failure() {
                    Err(Error::Recording(error))
                } else {
                    let id = u32::try_from(self.producers.len())
                        .map_err(|_| Error::invalid("producer identifiers exhausted"))?;
                    let time = self.backend.captured_time;
                    self.producers.push(ProducerState {
                        time,
                        finished: false,
                        queue: Queue::default(),
                    });
                    self.shared
                        .budgets
                        .lock()
                        .unwrap()
                        .push(Arc::new(Budget::new(self.shared.capacity)));
                    Ok((ProducerId(id), time))
                };
                drop(reply.send(result));
            }
            Command::ProducerDone(id, reply) => {
                self.producers
                    .get_mut(id.0 as usize)
                    .ok_or(Error::invalid("unknown producer"))?
                    .finished = true;
                let result = if let Some(error) = self.failure() {
                    Err(Error::Recording(error))
                } else {
                    self.drain(false)
                };
                if let Err(error) = &result {
                    self.shared.fail(error);
                }
                drop(reply.send(result));
            }
            Command::Flush(reply) => {
                let result = if let Some(error) = self.failure() {
                    Err(Error::Recording(error))
                } else {
                    self.drain(false).and_then(|()| self.backend.flush())
                };
                if let Err(error) = &result {
                    self.shared.fail(error);
                }
                drop(reply.send(result));
            }
            Command::Finish {
                incomplete,
                ending,
                rescue,
                reply,
            } => {
                if rescue {
                    for producer in &mut self.producers {
                        producer.finished = true;
                    }
                }
                let result = if self.panicked {
                    Err(Error::State(
                        "a panicked backend retains only its verified file prefix",
                    ))
                } else if self
                    .producers
                    .iter()
                    .skip(1)
                    .any(|producer| !producer.finished)
                {
                    Err(Error::State(
                        "finish every explicit producer before closing or sealing",
                    ))
                } else if !incomplete && self.failure().is_some() {
                    Err(Error::Recording(self.failure().unwrap()))
                } else {
                    self.producers[0].finished = true;
                    if self.failure().is_none() {
                        self.drain(true)
                            .and_then(|()| self.backend.finish_with(incomplete, ending))
                    } else {
                        self.backend.finish_with(true, ending)
                    }
                };
                let finished = result.is_ok();
                if let Err(error) = &result {
                    self.shared.fail(error);
                }
                if finished || rescue {
                    *self.shared.closed.lock().unwrap() = true;
                    for budget in self.shared.budgets.lock().unwrap().iter() {
                        budget.fail();
                    }
                }
                *self.shared.final_stats.lock().unwrap() = self.stats();
                drop(reply.send(result));
                return Ok(finished || rescue);
            }
            Command::Stats(reply) => {
                let _ = reply.send(self.stats());
            }
            Command::Shutdown => return Ok(true),
        }
        Ok(false)
    }
    fn stats(&self) -> WriterStats {
        let mut stats = self.backend.stats();
        stats.pending_peak_bytes = self.memory_peak;
        stats.spilled_bytes = self.spilled_bytes;
        stats.transport.submitted = self.shared.submitted.load(Ordering::Relaxed);
        stats.transport.completed = self.shared.completed.load(Ordering::Relaxed);
        stats.transport.peak_bytes = self.shared.queue_peak_bytes.load(Ordering::Relaxed);
        for budget in self.shared.budgets.lock().unwrap().iter() {
            stats.transport.waits += budget.waits.load(Ordering::Relaxed);
            stats.transport.wait_nanoseconds += budget.wait_nanoseconds.load(Ordering::Relaxed);
        }
        stats
    }
}

fn value_charge(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.capacity(),
        Value::Bytes(value)
        | Value::Bits { data: value, .. }
        | Value::Logic { data: value, .. }
        | Value::Logic9 { data: value, .. } => value.capacity(),
        Value::List(values) => {
            values.capacity() * std::mem::size_of::<Value>()
                + values.iter().map(value_charge).sum::<usize>()
        }
        Value::Map(values) => attributes_charge(values),
        _ => 0,
    }
}
pub(super) fn attributes_charge(attributes: &crate::attributes::Attributes) -> usize {
    // Spare slots move with the capture and remain part of its admission charge.
    attributes.capacity() * std::mem::size_of::<crate::attributes::Attribute>()
        + attributes
            .iter()
            .map(|attr| value_charge(&attr.value))
            .sum::<usize>()
}
fn charge(operation: &Operation) -> usize {
    std::mem::size_of::<Operation>()
        + match operation {
            Operation::EmitBatch(updates) => {
                updates.capacity() * std::mem::size_of::<super::SignalUpdate>()
            }
            Operation::Begin { payload, .. }
            | Operation::BeginStage { payload, .. }
            | Operation::End { payload, .. }
            | Operation::EndStage { payload, .. } => payload.as_ref().map_or(0, Vec::capacity),
            Operation::Point { payload, .. } | Operation::Payload { payload, .. } => {
                payload.capacity()
            }
            Operation::Emit {
                value: super::SignalInput::Packed { bytes, .. } | super::SignalInput::Bytes(bytes),
                ..
            } => bytes.capacity(),
            Operation::Attribute { attribute, .. } => value_charge(&attribute.value),
            Operation::EmbeddedEvent { event, .. } => attributes_charge(&event.attributes),
            Operation::EmbeddedStage { stage, .. } => attributes_charge(&stage.attributes),
            Operation::Relate(relation) => attributes_charge(&relation.attributes),
            _ => 0,
        }
}
