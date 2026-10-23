use std::{
    cell::Cell,
    collections::VecDeque,
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    marker::PhantomData,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use super::{
    Backend, Ending, Operation, Origin, ProducerId, Recorder, WriterOptions, WriterStats,
    capture::{Capture, Frontend},
    channel, spool,
};
use crate::{
    Error, Result,
    attributes::Value,
    framing::{Fingerprint, RecordingId},
};

type Control = Box<dyn FnOnce(&mut Backend, Option<String>) + Send>;
enum Command {
    Control(Control),
    AddProducer(mpsc::Sender<Result<(ProducerId, u64, channel::Sender)>>),
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

pub(super) struct Shared {
    sender: mpsc::Sender<Command>,
    failure: Mutex<Option<String>>,
    channels: Mutex<Vec<Arc<channel::Channel>>>,
    failed: AtomicBool,
    worker: OnceLock<std::thread::Thread>,
    sleeping: AtomicBool,
    capacity: usize,
    final_stats: Mutex<WriterStats>,
    closed: AtomicBool,
    ids: Arc<AtomicU64>,
}
impl Shared {
    pub(super) fn error(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::State("recording is finalized"));
        }
        self.failure()
    }
    fn failure(&self) -> Result<()> {
        if !self.failed.load(Ordering::Acquire) {
            return Ok(());
        }
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
        self.failed.store(true, Ordering::Release);
        self.wake();
    }
    fn wake(&self) {
        if let Some(worker) = self.worker.get() {
            worker.unpark();
        }
    }
    pub(super) fn captured(&self) {
        if self.sleeping.load(Ordering::Acquire) {
            self.wake();
        }
    }
    fn send(&self, command: Command) -> Result<()> {
        self.sender
            .send(command)
            .map_err(|_| Error::State("recording worker stopped"))?;
        self.wake();
        Ok(())
    }
    fn stop(&self) {
        self.closed.store(true, Ordering::Release);
        for channel in self.channels.lock().unwrap().iter() {
            channel.close();
        }
        self.wake();
    }

    pub(super) fn control<R: Send + 'static>(
        self: &Arc<Self>,
        action: impl FnOnce(&mut Backend) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.error()?;
        let (sender, receiver) = mpsc::channel();
        let shared = self.clone();
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::State("recording is finalized"));
        }
        self.send(Command::Control(Box::new(move |backend, failure| {
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
        receiver
            .recv()
            .map_err(|_| Error::State("recording control call failed"))?
    }
}

#[derive(Clone)]
pub(super) struct Rescue {
    shared: Arc<Shared>,
}
impl Rescue {
    pub fn seal(&self, ending: Ending) -> Result<Fingerprint> {
        let (reply, done) = mpsc::channel();
        if self.shared.closed.swap(true, Ordering::AcqRel) {
            return Err(Error::State("recording is finalized"));
        }
        let id = self
            .shared
            .ids
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                (id != 0).then(|| id.wrapping_add(1))
            })
            .map_err(|_| Error::State("record identifiers exhausted"))?;
        self.shared.stop();
        self.shared.send(Command::Finish {
            incomplete: true,
            ending: (ending != Ending::Closed).then_some((ending, id)),
            rescue: true,
            reply,
        })?;
        done.recv()
            .map_err(|_| Error::State("recording rescue failed"))?
    }
}

pub(super) struct Transport {
    shared: Arc<Shared>,
    implicit: channel::Sender,
    worker: Option<JoinHandle<()>>,
    cpu: super::cpu::WorkerCpu,
}

impl Transport {
    pub fn new(backend: Backend, options: &WriterOptions, ids: Arc<AtomicU64>) -> Result<Self> {
        if cfg!(target_family = "wasm") {
            return Err(Error::invalid(
                "threaded recording is unavailable on this target",
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let (implicit, input) = channel::exclusive_channel(options.buffer.queue_bytes, 0);
        let shared = Arc::new(Shared {
            sender,
            failure: Mutex::default(),
            channels: Mutex::new(vec![implicit.channel()]),
            failed: AtomicBool::new(false),
            worker: OnceLock::new(),
            sleeping: AtomicBool::new(false),
            capacity: options.buffer.queue_bytes,
            final_stats: Mutex::default(),
            closed: AtomicBool::new(false),
            ids,
        });
        let worker_shared = shared.clone();
        let pending_bytes = options.buffer.pending_bytes;
        let cpu = super::cpu::WorkerCpu::default();
        let worker_cpu = cpu.clone();
        let worker = std::thread::Builder::new()
            .name("vtr-recording".into())
            .spawn(move || {
                let _measurement = worker_cpu.measure();
                worker_shared.worker.set(std::thread::current()).ok();
                let mut worker = Worker::new(backend, pending_bytes, worker_shared.clone(), input);
                let mut idle_rounds = 0usize;
                loop {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || -> Result<(bool, bool)> {
                            let mut active = worker.ingest(4096)?;
                            if !worker_shared.failed.load(Ordering::Acquire) {
                                worker.backend.poll_output()?;
                            }
                            match receiver.try_recv() {
                                Ok(command) => {
                                    worker.ingest(usize::MAX)?;
                                    active = true;
                                    if matches!(command, Command::Shutdown) {
                                        worker_shared.fail("recording owner was dropped");
                                        return Ok((true, active));
                                    }
                                    Ok((worker.command(command)?, active))
                                }
                                Err(mpsc::TryRecvError::Empty) => Ok((false, active)),
                                Err(mpsc::TryRecvError::Disconnected) => Ok((true, active)),
                            }
                        },
                    ));
                    match result {
                        Ok(Ok((true, _))) => break,
                        Ok(Ok((false, false))) => {
                            idle_rounds = idle_rounds.saturating_add(1);
                            if idle_rounds < 64 {
                                std::thread::yield_now();
                                continue;
                            }
                            worker_shared.sleeping.store(true, Ordering::Release);
                            if !worker.inputs.iter_mut().any(|input| input.available() != 0) {
                                std::thread::park_timeout(Duration::from_micros(10));
                            }
                            worker_shared.sleeping.store(false, Ordering::Release);
                        }
                        Ok(Ok((false, true))) => idle_rounds = 0,
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
    pub(super) fn capture_parts(&mut self) -> (&mut channel::Sender, &Arc<Shared>) {
        (&mut self.implicit, &self.shared)
    }
    pub fn worker_cpu(&self) -> super::cpu::WorkerCpu {
        self.cpu.clone()
    }
    pub fn usable(&self) -> Result<()> {
        self.shared.error()
    }
    pub fn rescue(&self) -> Rescue {
        self.implicit.enable_independent_stop();
        Rescue {
            shared: self.shared.clone(),
        }
    }
    pub fn control<R: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Backend) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        self.shared.control(action)
    }
    pub fn add_producer(&self, origin: Origin) -> Result<Producer> {
        self.shared.error()?;
        let (sender, receiver) = mpsc::channel();
        self.shared
            .send(Command::AddProducer(sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        let (producer, time, sender) = receiver
            .recv()
            .map_err(|_| Error::State("producer registration failed"))??;
        Ok(Producer {
            shared: self.shared.clone(),
            sender,
            front: Frontend {
                origin,
                ids: super::capture::RecordIds::Shared(self.shared.ids.clone()),
                time,
                producer,
                finished: false,
                payload: Vec::new(),
            },
            marker: PhantomData,
        })
    }
    pub fn flush(&self) -> Result<()> {
        self.shared.error()?;
        let (sender, receiver) = mpsc::channel();
        self.shared
            .send(Command::Flush(sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        receiver
            .recv()
            .map_err(|_| Error::State("recording flush failed"))?
    }
    pub fn finish_producer(&self, id: ProducerId) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        self.shared
            .send(Command::ProducerDone(id, sender))
            .map_err(|_| Error::State("recording worker stopped"))?;
        receiver
            .recv()
            .map_err(|_| Error::State("producer finalization failed"))?
    }
    pub fn stats(&self) -> Result<WriterStats> {
        if self.worker.is_none() || self.shared.closed.load(Ordering::Acquire) {
            return Ok(*self.shared.final_stats.lock().unwrap());
        }
        let (sender, receiver) = mpsc::channel();
        self.shared
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
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(Error::State("recording is finalized"));
        }
        let (sender, receiver) = mpsc::channel();
        self.shared
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
            drop(self.shared.send(Command::Shutdown));
            drop(worker.join());
        }
    }
}

/// Exclusive logical producer: it may move between threads, but cannot be shared concurrently.
pub struct Producer {
    shared: Arc<Shared>,
    sender: channel::Sender,
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
    #[inline]
    fn state(&self) -> &Frontend {
        &self.front
    }
    #[inline]
    fn session(&mut self) -> super::session::Session<'_> {
        super::session::Session::producer(&mut self.front, &mut self.sender, &self.shared)
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
    stamp: u64,
    sequence: u64,
    length: u64,
    bytes: u64,
}
#[derive(Default)]
struct Queue {
    memory: VecDeque<Pending>,
    spill: Option<SpillFile>,
}
struct ProducerState {
    time: u64,
    finished: bool,
    queue: Queue,
}
struct SpillFile {
    reader: Option<BufReader<File>>,
    writer: Option<BufWriter<File>>,
    path: PathBuf,
    length: u64,
    visible: u64,
    read: u64,
    head: Option<SpillHead>,
}
impl SpillFile {
    fn new() -> Result<Self> {
        let id = RecordingId::generate()?;
        let path = std::env::temp_dir().join(format!("vtr-pending-{id}"));
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut spool = Self {
            reader: None,
            writer: Some(BufWriter::with_capacity(64 << 10, file)),
            path,
            length: 0,
            visible: 0,
            read: 32,
            head: None,
        };
        // A separate open gives the buffered reader its own file position.
        let mut reader = File::open(&spool.path)?;
        reader.seek(SeekFrom::Start(32))?;
        spool.reader = Some(BufReader::with_capacity(64 << 10, reader));
        Ok(spool)
    }
    fn append(&mut self, pending: Pending) -> Result<()> {
        let payload = spool::encode(&pending.operation)?;
        let head = SpillHead {
            stamp: pending.stamp,
            sequence: pending.sequence,
            length: payload.len() as u64,
            bytes: pending.bytes as u64,
        };
        let mut header = [0u8; 32];
        for (field, value) in header.as_chunks_mut::<8>().0.iter_mut().zip([
            head.stamp,
            head.sequence,
            head.length,
            head.bytes,
        ]) {
            field.copy_from_slice(&value.to_le_bytes());
        }
        let writer = self.writer.as_mut().unwrap();
        writer.write_all(&header)?;
        writer.write_all(&payload)?;
        self.length += 32 + payload.len() as u64;
        if self.head.is_none() {
            self.head = Some(head);
        }
        Ok(())
    }
    fn flush_through(&mut self, end: u64) -> Result<()> {
        if end > self.visible {
            self.writer.as_mut().unwrap().flush()?;
            self.visible = self.length;
        }
        Ok(())
    }
    fn pop(&mut self) -> Result<Pending> {
        let head = self.head.take().unwrap();
        let length = usize::try_from(head.length)
            .map_err(|_| Error::Corrupt("spool length exceeds usize"))?;
        let end = self
            .read
            .checked_add(head.length)
            .filter(|end| *end <= self.length)
            .ok_or_else(|| Error::corrupt("spool record exceeds file"))?;
        self.flush_through(end)?;
        let mut bytes = vec![0; length];
        self.reader.as_mut().unwrap().read_exact(&mut bytes)?;
        self.read = end;
        if self.read < self.length {
            let end = self
                .read
                .checked_add(32)
                .filter(|end| *end <= self.length)
                .ok_or_else(|| Error::corrupt("spool header exceeds file"))?;
            self.flush_through(end)?;
            let mut header = [0; 32];
            self.reader.as_mut().unwrap().read_exact(&mut header)?;
            let mut cursor = crate::wire::Cursor::new(&header);
            self.head = Some(SpillHead {
                stamp: cursor.u64()?,
                sequence: cursor.u64()?,
                length: cursor.u64()?,
                bytes: cursor.u64()?,
            });
            self.read = end;
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
        drop(self.reader.take());
        drop(self.writer.take());
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
    memory_peak: usize,
    spilled_bytes: u64,
    panicked: bool,
    inputs: Vec<channel::Receiver>,
}
impl Worker {
    fn new(backend: Backend, limit: usize, shared: Arc<Shared>, input: channel::Receiver) -> Self {
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
            memory_peak: 0,
            spilled_bytes: 0,
            panicked: false,
            inputs: vec![input],
        }
    }
    fn ingest(&mut self, limit: usize) -> Result<bool> {
        let mut active = false;
        for id in 0..self.inputs.len() {
            let quota = self.inputs[id].available().min(limit);
            let mut consumed = 0;
            while consumed < quota {
                let Some(capture) = self.inputs[id].next() else {
                    break;
                };
                consumed += capture.cells();
                let stamp = capture.stamp();
                active = true;
                if self.shared.failed.load(Ordering::Acquire) {
                    continue;
                }
                if self.producers.len() == 1 {
                    self.producers[0].time = stamp;
                    if let Some(time) = capture.progress() {
                        self.backend.advance(time)?;
                    } else if let Some((action, record, payload)) = capture.payload() {
                        self.backend.apply_payload(
                            ProducerId(0),
                            stamp,
                            action,
                            record,
                            payload,
                        )?;
                    } else if let Some((signal, value)) = capture.scalar() {
                        self.backend
                            .emit_signal(ProducerId(0), stamp, signal, value)?;
                    } else if let Some((signal, value)) = capture.signal_ref() {
                        self.backend
                            .emit_signal_ref(ProducerId(0), stamp, signal, value)?;
                    } else {
                        self.backend
                            .apply(ProducerId(0), stamp, capture.into_operation())?;
                    }
                } else {
                    let operation = capture.into_operation();
                    let bytes = charge(&operation);
                    self.push(ProducerId(id as u32), stamp, operation, bytes)?;
                }
            }
            self.inputs[id].release();
        }
        if active && self.producers.len() > 1 && !self.shared.failed.load(Ordering::Acquire) {
            self.drain(false)?;
        }
        Ok(active)
    }
    fn failure(&self) -> Option<String> {
        if self.shared.failed.load(Ordering::Acquire) {
            self.shared.failure.lock().unwrap().clone()
        } else {
            None
        }
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
            .ok_or_else(|| Error::invalid("unknown producer"))?;
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
            .ok_or_else(|| Error::invalid("capture sequence exhausted"))?;
        if state.queue.spill.is_none() && bytes <= self.limit.saturating_sub(self.memory) {
            self.memory += bytes;
            self.memory_peak = self.memory_peak.max(self.memory);
            state.queue.memory.push_back(pending);
        } else {
            if state.queue.spill.is_none() {
                state.queue.spill = Some(SpillFile::new()?);
            }
            let spool = state.queue.spill.as_mut().unwrap();
            let before = spool.length;
            spool.append(pending)?;
            self.spilled_bytes += spool.length - before;
        }
        Ok(())
    }
    fn drain(&mut self, all: bool) -> Result<()> {
        let (frontier, active) = self
            .producers
            .iter()
            .filter(|producer| !producer.finished)
            .fold((None, 0usize), |(frontier, count), producer| {
                (
                    Some(frontier.map_or(producer.time, |time: u64| time.min(producer.time))),
                    count + 1,
                )
            });
        let single = active <= 1;
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
                                .as_ref()
                                .and_then(|spool| spool.head)
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
                let spool = queue.spill.as_mut().unwrap();
                let pending = spool.pop()?;
                if spool.head.is_none() {
                    queue.spill.take();
                }
                pending
            };
            self.backend
                .apply(ProducerId(id as u32), pending.stamp, pending.operation)?;
        }
    }
    fn command(&mut self, command: Command) -> Result<bool> {
        match command {
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
                    let (sender, receiver) = channel::channel(self.shared.capacity, time);
                    self.shared.channels.lock().unwrap().push(sender.channel());
                    self.inputs.push(receiver);
                    Ok((ProducerId(id), time, sender))
                };
                drop(reply.send(result));
            }
            Command::ProducerDone(id, reply) => {
                self.producers
                    .get_mut(id.0 as usize)
                    .ok_or_else(|| Error::invalid("unknown producer"))?
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
                    self.shared.stop();
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
        for channel in self.shared.channels.lock().unwrap().iter() {
            let current = channel.stats();
            stats.transport.submitted += current.submitted;
            stats.transport.completed += current.completed;
            stats.transport.peak_bytes += current.peak_bytes;
            stats.transport.waits += current.waits;
            stats.transport.wait_nanoseconds += current.wait_nanoseconds;
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
pub(super) fn charge(operation: &Operation) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(index: u64) -> Pending {
        let length = [0, 31, 65_503, 65_504, 65_535, 65_536, 65_537][index as usize % 7];
        let operation = Operation::Emit {
            signal: crate::SignalId(17),
            value: super::super::SignalInput::Bytes(vec![index as u8; length]),
        };
        Pending {
            stamp: index / 3,
            sequence: index,
            bytes: charge(&operation),
            operation,
        }
    }

    fn check(actual: Pending, index: u64) {
        let expected = pending(index);
        assert_eq!(
            (actual.stamp, actual.sequence, actual.bytes),
            (expected.stamp, expected.sequence, expected.bytes)
        );
        let Operation::Emit {
            signal,
            value: super::super::SignalInput::Bytes(bytes),
        } = actual.operation
        else {
            panic!("spool changed the operation kind");
        };
        assert_eq!(signal, crate::SignalId(17));
        let Operation::Emit {
            value: super::super::SignalInput::Bytes(expected),
            ..
        } = expected.operation
        else {
            unreachable!()
        };
        assert_eq!(bytes, expected);
    }

    #[test]
    fn spill_replay_crosses_buffer_boundaries_while_producers_append_independently() {
        let mut a = SpillFile::new().unwrap();
        let mut b = SpillFile::new().unwrap();
        let paths = [a.path.clone(), b.path.clone()];
        for index in 0..16 {
            a.append(pending(index)).unwrap();
            b.append(pending(index + 1000)).unwrap();
        }
        for index in 0..16 {
            check(a.pop().unwrap(), index);
            b.append(pending(index + 1016)).unwrap();
            check(b.pop().unwrap(), index + 1000);
            a.append(pending(index + 16)).unwrap();
        }
        for index in 16..32 {
            check(b.pop().unwrap(), index + 1000);
            check(a.pop().unwrap(), index);
        }
        assert!(a.head.is_none());
        assert!(b.head.is_none());
        drop((a, b));
        for path in paths {
            assert!(!path.exists());
        }
    }
}
