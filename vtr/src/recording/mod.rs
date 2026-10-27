//! Typed recording with explicit producer clocks, bounded capture, and sealing.

mod backend;
mod capture;
mod channel;
mod cpu;
mod crash;
pub mod ending;
pub use crash::{CrashState, Guarded, Sealer};
pub use ending::Ending;
mod output;
mod payload_pool;
mod session;
mod spool;
mod tracker;
mod transport;
pub use tracker::{Keyspace, Tracker, TrackerDiagnostic};
mod waveform;

pub use crate::records::{SpanKind, Status};
use capture::{Capture, Frontend, Operation, SignalInput};
pub use capture::{Recorder, SignalUpdate};
pub use transport::Producer;

use crate::{
    Compression, Error, Result,
    attributes::{Attribute, Attributes, Value},
    declarations::Metadata,
    framing::{Fingerprint, RecordingId},
    hierarchy::{Direction, NodeId, ScopeType, SignalId, SignalKind, VarType},
    schema::Payload,
};
use backend::Backend;
use std::{
    cell::Cell,
    io::{BufWriter, Write},
    marker::PhantomData,
    num::NonZeroU64,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use transport::Transport;

// Process-local IDs keep handles compact and reject stale handles when allocation addresses are reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Origin(NonZeroU64);

impl Origin {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| Error::invalid("recording identities exhausted"))?;
        Ok(Self(
            NonZeroU64::new(id).expect("recording identities start at one"),
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Inline,
    Threaded,
}

#[derive(Clone, Debug)]
pub struct BufferOptions {
    pub queue_bytes: usize,
    pub pending_bytes: usize,
}
impl Default for BufferOptions {
    fn default() -> Self {
        Self {
            queue_bytes: 256 << 10,
            pending_bytes: 16 << 20,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EncodingOptions {
    pub compression: Compression,
    pub page_bytes: usize,
    pub block_bytes: usize,
    pub block_records: usize,
    pub waveform_bytes: usize,
    pub waveform_records: usize,
    pub waveform_deduplicate: bool,
    pub separate_logs: bool,
    pub separate_clocks: bool,
    pub workers: usize,
    pub payload_pool_bytes: usize,
    pub queue_jobs: usize,
    pub queue_bytes: usize,
}
impl Default for EncodingOptions {
    fn default() -> Self {
        Self {
            compression: Compression::LZ4,
            page_bytes: 64 << 10,
            block_bytes: 512 << 10,
            block_records: 65536,
            waveform_bytes: 16 << 20,
            waveform_records: 1 << 24,
            waveform_deduplicate: true,
            separate_logs: true,
            separate_clocks: true,
            workers: 0,
            payload_pool_bytes: 16 << 20,
            queue_jobs: 8,
            queue_bytes: 256 << 20,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WriterOptions {
    pub mode: ExecutionMode,
    pub buffer: BufferOptions,
    pub encoding: EncodingOptions,
    pub metadata: Metadata,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct QueueStats {
    /// Accepted owned captures or encoding jobs, excluding control messages.
    pub submitted: u64,
    /// Captures removed from the input queue, or encoding jobs published to output.
    pub completed: u64,
    /// Backpressure waits, excluding explicit final drains.
    pub waits: u64,
    pub wait_nanoseconds: u64,
    /// Peak admission byte charge; capture sums conservative per-producer bounds.
    pub peak_bytes: usize,
}

/// Buffer peaks use the admission charges that trigger flushing. Block counts
/// include only sections successfully written; queued encoding is counted separately.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriterStats {
    pub live_owners: usize,
    pub live_stages: usize,
    pub max_created_id: u64,
    pub output_bytes: u64,
    pub transport: QueueStats,
    pub encoding: QueueStats,
    /// Owned active payload capacity and attribute/child data, excluding map overhead.
    pub active_payload_bytes: usize,
    pub payload_pool_bytes: usize,
    pub peak_transaction_bytes: usize,
    pub peak_record_bytes: usize,
    pub peak_waveform_bytes: usize,
    pub transaction_blocks: u64,
    pub record_blocks: u64,
    pub waveform_blocks: u64,
    pub backend_cpu_nanoseconds: Option<u64>,
    pub pending_peak_bytes: usize,
    pub spilled_bytes: u64,
}
impl Default for WriterOptions {
    fn default() -> Self {
        Self {
            mode: if cfg!(target_family = "wasm") {
                ExecutionMode::Inline
            } else {
                ExecutionMode::Threaded
            },
            buffer: BufferOptions::default(),
            encoding: EncodingOptions::default(),
            metadata: Metadata::default(),
        }
    }
}
impl WriterOptions {
    fn validate(&self) -> Result<()> {
        if self.mode == ExecutionMode::Inline && self.encoding.workers != 0 {
            return Err(Error::invalid(
                "inline execution cannot use encoding workers",
            ));
        }
        if self.mode == ExecutionMode::Threaded
            && (self.buffer.queue_bytes == 0 || self.buffer.pending_bytes == 0)
        {
            return Err(Error::invalid("threaded capture budgets must be positive"));
        }
        if self.mode == ExecutionMode::Threaded && self.buffer.queue_bytes > isize::MAX as usize / 4
        {
            return Err(Error::invalid("capture ring exceeds addressable storage"));
        }
        if self.encoding.page_bytes == 0
            || self.encoding.page_bytes >= crate::wire::MAX_BLOCK_BYTES
            || self.encoding.block_bytes == 0
            || self.encoding.block_records == 0
            || self.encoding.waveform_bytes == 0
            || self.encoding.waveform_records == 0
            || (self.encoding.workers != 0
                && (self.encoding.queue_jobs == 0 || self.encoding.queue_bytes == 0))
        {
            return Err(Error::invalid("invalid encoding budget"));
        }
        self.metadata.encode(u64::MAX)?;
        Ok(())
    }
}

#[derive(Clone, Default, Debug)]
pub struct SourceOptions {
    pub name: String,
    pub parent: Option<NodeId>,
    pub attributes: Attributes,
}
impl SourceOptions {
    pub fn new(name: impl Into<String>, parent: Option<NodeId>) -> Self {
        Self {
            name: name.into(),
            parent,
            attributes: vec![],
        }
    }
}

/// Static categories distinguish otherwise identical payload bindings.
pub mod source {
    pub struct Transaction;
    pub struct Stage;
    pub struct Event;
    pub struct Log;
    pub(super) struct Clock;
}

/// A source binds both its payload and recording category.
///
/// ```compile_fail
/// use vtr::{Recorder, SourceOptions, Writer};
/// fn wrong_source(writer: &mut Writer) -> vtr::Result<()> {
///     let logs = writer.logger(SourceOptions::new("diagnostics", None))?;
///     writer.begin(logs, &42, None)?;
///     Ok(())
/// }
/// ```
pub struct Source<T, K = source::Transaction> {
    id: u32,
    origin: Origin,
    marker: PhantomData<fn() -> (T, K)>,
}
impl<T, K> Copy for Source<T, K> {}
impl<T, K> Clone for Source<T, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, K> Source<T, K> {
    #[must_use]
    pub fn id(self) -> u32 {
        self.id
    }
}

#[derive(Clone, Debug, Default)]
pub struct DatatypeOptions {
    pub name: String,
    pub attributes: crate::Attributes,
}
impl DatatypeOptions {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            attributes: vec![],
        }
    }
}

pub type Logger = Source<(), source::Log>;
/// A writer-bound handle to a one-field datatype wrapping `T`.
pub struct PayloadType<T> {
    id: u32,
    origin: Origin,
    marker: PhantomData<fn() -> T>,
}
impl<T> Copy for PayloadType<T> {}
impl<T> Clone for PayloadType<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PayloadType<T> {
    #[must_use]
    pub fn id(self) -> u32 {
        self.id
    }
}

pub struct ActiveRecord<T, K = source::Transaction> {
    id: u64,
    origin: Origin,
    marker: PhantomData<fn() -> (T, K)>,
}
impl<T, K> Copy for ActiveRecord<T, K> {}
impl<T, K> Clone for ActiveRecord<T, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, K> ActiveRecord<T, K> {
    #[must_use]
    pub fn id(self) -> u64 {
        self.id
    }
}
impl<T> ActiveRecord<T, source::Transaction> {
    #[must_use]
    pub fn owner(self) -> OwnerRef {
        OwnerRef {
            id: self.id,
            origin: self.origin,
        }
    }
}
pub type Transaction<T = ()> = ActiveRecord<T>;
pub type Stage<T = ()> = ActiveRecord<T, source::Stage>;

#[derive(Clone, Copy, Debug)]
pub struct OwnerRef {
    id: u64,
    origin: Origin,
}
impl OwnerRef {
    #[must_use]
    pub fn id(self) -> u64 {
        self.id
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ClockSource {
    id: u32,
    origin: Origin,
}
impl ClockSource {
    #[must_use]
    pub fn id(self) -> u32 {
        self.id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProducerId(u32);
impl ProducerId {
    #[must_use]
    pub fn value(self) -> u32 {
        self.0
    }
}

enum Dispatch {
    Inline(Box<Backend>),
    Threaded(Transport),
}

pub struct Writer {
    identity: RecordingId,
    dispatch: Dispatch,
    front: Frontend,
    failure: Option<String>,
    finished: bool,
    crash: Arc<CrashState>,
    worker_cpu: Vec<cpu::WorkerCpu>,
}

impl Writer {
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        Self::create_with(path, WriterOptions::default())
    }
    pub fn create_with(path: impl AsRef<Path>, options: WriterOptions) -> Result<Self> {
        options.validate()?;
        Self::from_writer(
            Box::new(BufWriter::new(std::fs::File::create(path)?)),
            options,
        )
    }
    pub fn from_writer(output: Box<dyn Write + Send>, options: WriterOptions) -> Result<Self> {
        options.validate()?;
        let backend = Backend::new(output, options.clone())?;
        let mut worker_cpu = backend.worker_cpu();
        let identity = backend.identity();
        let origin = Origin::new()?;
        let (dispatch, ids) = match options.mode {
            ExecutionMode::Inline => (
                Dispatch::Inline(Box::new(backend)),
                capture::RecordIds::Local(Cell::new(1)),
            ),
            ExecutionMode::Threaded => {
                let ids = Arc::new(AtomicU64::new(1));
                let transport = Transport::new(backend, &options, ids.clone())?;
                worker_cpu.push(transport.worker_cpu());
                (
                    Dispatch::Threaded(transport),
                    capture::RecordIds::Shared(ids),
                )
            }
        };
        let front = Frontend {
            origin,
            ids,
            time: 0,
            producer: ProducerId(0),
            finished: false,
            payload: Vec::new(),
        };
        Ok(Self {
            identity,
            dispatch,
            front,
            failure: None,
            finished: false,
            crash: Arc::default(),
            worker_cpu,
        })
    }
    #[must_use]
    pub fn identity(&self) -> RecordingId {
        let _guard = self.crash.guard();
        self.identity
    }
    fn control<R: Send + 'static>(
        &mut self,
        action: impl FnOnce(&mut Backend) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let _guard = self.crash.guard();
        self.usable()?;
        let result = match &mut self.dispatch {
            Dispatch::Inline(backend) => action(backend),
            Dispatch::Threaded(transport) => transport.control(action),
        };
        if let Err(error) = &result {
            self.failure = Some(error.to_string());
        }
        result
    }
    #[inline]
    fn usable(&self) -> Result<()> {
        if self.finished {
            return Err(Error::State("recording is finalized"));
        }
        if self.crash.poisoned() {
            return Err(Error::State("recording frontend panicked"));
        }
        if let Dispatch::Threaded(transport) = &self.dispatch {
            transport.usable()?;
        }
        if let Some(failure) = &self.failure {
            return Err(Error::Recording(failure.clone()));
        }
        Ok(())
    }
    #[inline]
    fn capture_signal(&mut self, signal: SignalId, value: SignalInput) -> Result<()> {
        let _guard = self.crash.scoped();
        self.usable()?;
        if self.front.finished {
            return Err(Error::State("producer is finished"));
        }
        let result = match &mut self.dispatch {
            Dispatch::Inline(backend) => {
                backend.emit_signal(self.front.producer, self.front.time, signal, value)
            }
            Dispatch::Threaded(transport) => {
                let (sender, shared) = transport.capture_parts();
                sender
                    .publish_signal(signal, value, || shared.error())
                    .map(|()| shared.captured())
            }
        };
        if let Err(error) = &result {
            Self::capture_failure(&mut self.failure, error);
        }
        result
    }
    #[cold]
    #[inline(never)]
    fn capture_failure(failure: &mut Option<String>, error: &Error) {
        *failure = Some(error.to_string());
    }
    pub fn metadata(&mut self) -> Result<Metadata> {
        let _guard = self.crash.guard();
        self.control(|backend| Ok(backend.metadata.clone()))
    }
    pub fn set_metadata(&mut self, metadata: Metadata) -> Result<()> {
        let _guard = self.crash.guard();
        self.control(move |backend| backend.metadata(metadata))
    }
    pub fn intern(&mut self, name: &str) -> Result<u32> {
        let _guard = self.crash.guard();
        let name = name.to_owned();
        self.control(move |backend| backend.intern(&name))
    }
    pub fn scope(&mut self, name: impl Into<String>, parent: Option<NodeId>) -> Result<NodeId> {
        let _guard = self.crash.guard();
        self.scope_with(SourceOptions::new(name, parent), ScopeType::Module, "")
    }
    pub fn scope_with(
        &mut self,
        options: SourceOptions,
        kind: ScopeType,
        component: &str,
    ) -> Result<NodeId> {
        let _guard = self.crash.guard();
        if options.parent.is_none() && options.name == ending::SCOPE {
            return Err(Error::invalid("the run-ending scope is reserved"));
        }
        let component = component.to_owned();
        self.control(move |backend| backend.scope(options, kind, &component))
    }
    fn source<T: Payload + 'static, K>(
        &mut self,
        options: SourceOptions,
        kind: &str,
    ) -> Result<Source<T, K>> {
        let kind = kind.to_owned();
        let id = self.control(move |backend| backend.source::<T>(options, &kind))?;
        Ok(Source {
            id,
            origin: self.front.origin,
            marker: PhantomData,
        })
    }
    pub fn transactions<T: Payload + 'static>(
        &mut self,
        options: SourceOptions,
        kind: &str,
    ) -> Result<Source<T>> {
        let _guard = self.crash.guard();
        if matches!(kind, "CLOCK" | "STAGE" | "EVENT" | "LOG") {
            return Err(Error::invalid("reserved source kind"));
        }
        self.source::<T, source::Transaction>(options, kind)
    }
    pub fn stages<T: Payload + 'static>(
        &mut self,
        options: SourceOptions,
    ) -> Result<Source<T, source::Stage>> {
        let _guard = self.crash.guard();
        self.source::<T, source::Stage>(options, "STAGE")
    }
    pub fn events<T: Payload + 'static>(
        &mut self,
        options: SourceOptions,
    ) -> Result<Source<T, source::Event>> {
        let _guard = self.crash.guard();
        self.source::<T, source::Event>(options, "EVENT")
    }
    pub fn logger(&mut self, options: SourceOptions) -> Result<Logger> {
        let _guard = self.crash.guard();
        self.source::<(), source::Log>(options, "LOG")
    }
    pub fn declare_datatype(
        &mut self,
        options: DatatypeOptions,
        kind: crate::schema::TypeKind,
    ) -> Result<crate::TypeId> {
        let _guard = self.crash.guard();
        self.control(move |backend| backend.declare_datatype(options, kind))
    }
    pub fn datatype<T: Payload + 'static>(
        &mut self,
        options: DatatypeOptions,
    ) -> Result<PayloadType<T>> {
        let _guard = self.crash.guard();
        let id = self.control(move |backend| backend.datatype::<T>(options))?;
        Ok(PayloadType {
            id,
            origin: self.front.origin,
            marker: PhantomData,
        })
    }
    pub fn clock(&mut self, options: SourceOptions) -> Result<ClockSource> {
        let _guard = self.crash.guard();
        let source = self.source::<(), source::Clock>(options, "CLOCK")?;
        Ok(ClockSource {
            id: source.id,
            origin: source.origin,
        })
    }
    pub fn associate_clock<T, K>(
        &mut self,
        source: Source<T, K>,
        clock: ClockSource,
    ) -> Result<()> {
        let _guard = self.crash.guard();
        self.front.check(source.origin)?;
        self.front.check(clock.origin)?;
        let key = self.intern("vtr.clock")?;
        self.control(move |backend| {
            backend.node_attribute(
                NodeId(source.id),
                Attribute {
                    key,
                    value: Value::U32(clock.id),
                },
            )
        })
    }
    pub fn node_attribute(&mut self, node: NodeId, key: u32, value: Value) -> Result<()> {
        let _guard = self.crash.guard();
        self.control(move |backend| backend.node_attribute(node, Attribute { key, value }))
    }
    pub fn signal(
        &mut self,
        options: SourceOptions,
        kind: SignalKind,
        var_type: VarType,
        direction: Direction,
    ) -> Result<SignalId> {
        let _guard = self.crash.guard();
        self.control(move |backend| backend.signal(options, kind, var_type, direction))
    }
    pub fn alias(
        &mut self,
        signal: SignalId,
        options: SourceOptions,
        var_type: VarType,
        direction: Direction,
    ) -> Result<NodeId> {
        let _guard = self.crash.guard();
        self.control(move |backend| backend.alias(options, signal, var_type, direction))
    }
    pub fn enum_table(
        &mut self,
        options: SourceOptions,
        entries: Vec<(u32, u32)>,
    ) -> Result<NodeId> {
        let _guard = self.crash.guard();
        self.control(move |backend| backend.enum_table(options, entries))
    }
    /// Bind a payload type once in this recording's datatype table.
    pub fn bind<T: Payload + 'static>(&mut self) -> Result<crate::TypeId> {
        let _guard = self.crash.guard();
        self.control(Backend::bind::<T>)
    }
    pub fn add_producer(&mut self) -> Result<Producer> {
        let _guard = self.crash.guard();
        self.usable()?;
        match &mut self.dispatch {
            Dispatch::Inline(_) => {
                Err(Error::invalid("inline recording has one implicit producer"))
            }
            Dispatch::Threaded(transport) => transport.add_producer(self.front.origin),
        }
    }
    pub fn finish_producer(&mut self) -> Result<()> {
        let _guard = self.crash.guard();
        if self.front.finished {
            return Ok(());
        }
        match &self.dispatch {
            Dispatch::Inline(_) => {
                return Err(Error::invalid(
                    "inline producer is finalized by closing the writer",
                ));
            }
            Dispatch::Threaded(transport) => transport.finish_producer(self.front.producer)?,
        }
        self.front.finished = true;
        Ok(())
    }
    pub fn flush(&mut self) -> Result<()> {
        let _guard = self.crash.guard();
        self.usable()?;
        let result = match &mut self.dispatch {
            Dispatch::Inline(backend) => backend.flush(),
            Dispatch::Threaded(transport) => transport.flush(),
        };
        if let Err(error) = &result {
            self.failure = Some(error.to_string());
        }
        result
    }
    pub fn stats(&self) -> Result<WriterStats> {
        let _guard = self.crash.guard();
        let mut stats = match &self.dispatch {
            Dispatch::Inline(backend) => Ok(backend.stats()),
            Dispatch::Threaded(transport) => transport.stats(),
        }?;
        stats.backend_cpu_nanoseconds = self.backend_cpu_nanoseconds();
        Ok(stats)
    }
    /// Sample owned worker CPU without waiting for ingestion or output. Linux
    /// retains final totals after exit; unsupported worker clocks return None.
    #[must_use]
    pub fn backend_cpu_nanoseconds(&self) -> Option<u64> {
        self.worker_cpu
            .iter()
            .try_fold(0u64, |total, cpu| total.checked_add(cpu.nanoseconds()?))
    }
    pub fn close(&mut self) -> Result<Fingerprint> {
        let _guard = self.crash.guard();
        self.finish(false, None)
    }
    pub fn seal(&mut self) -> Result<Fingerprint> {
        let _guard = self.crash.guard();
        self.finish(true, None)
    }
    /// Finish with a typed run-ending log. Abnormal endings mark the trace incomplete.
    pub fn close_with(&mut self, ending: Ending) -> Result<()> {
        let _guard = self.crash.guard();
        let abnormal = ending != Ending::Closed;
        self.finish(abnormal, Some(ending)).map(|_| ())
    }
    pub fn guarded<R>(&mut self, action: impl FnOnce(&mut Self) -> R) -> R {
        let _guard = self.crash.guard();
        action(self)
    }
    #[must_use]
    pub fn state(&self) -> &CrashState {
        &self.crash
    }
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.crash.poisoned()
    }
    #[must_use]
    pub fn sealer(&self) -> Sealer {
        let _guard = self.crash.guard();
        Sealer {
            state: self.crash.clone(),
            rescue: match &self.dispatch {
                Dispatch::Inline(_) => None,
                Dispatch::Threaded(transport) => Some(transport.rescue()),
            },
        }
    }
    fn finish(&mut self, incomplete: bool, ending: Option<Ending>) -> Result<Fingerprint> {
        let _guard = self.crash.guard();
        if self.finished {
            return Err(Error::State("recording is finalized"));
        }
        if self.crash.poisoned() {
            let ending = match ending {
                None | Some(Ending::Closed) => Ending::Poisoned,
                Some(ending) => ending,
            };
            let result = self.sealer().finish(ending);
            if result.is_ok() {
                self.finished = true;
                self.front.finished = true;
            }
            return result;
        }
        if !incomplete {
            self.usable()?;
        }
        let ending = ending
            .filter(|end| *end != Ending::Closed)
            .map(|end| self.front.reserve().map(|id| (end, id)))
            .transpose()?;
        let result = match &mut self.dispatch {
            Dispatch::Inline(backend) => backend.finish_with(incomplete, ending),
            Dispatch::Threaded(transport) => transport.finish(incomplete, ending),
        };
        if result.is_ok() {
            self.finished = true;
            self.front.finished = true;
        } else if let Err(error) = &result {
            self.failure = Some(error.to_string());
        }
        result
    }
}

impl Capture for Writer {
    #[inline]
    fn state(&self) -> &Frontend {
        &self.front
    }
    #[inline]
    fn time(&self) -> u64 {
        let _guard = self.crash.scoped();
        self.front.time
    }
    #[inline]
    fn session(&mut self) -> session::Session<'_> {
        session::Session::writer(
            &mut self.front,
            &mut self.dispatch,
            &mut self.failure,
            self.finished,
            &self.crash,
        )
    }
}
impl Recorder for Writer {
    #[inline]
    fn advance_time(&mut self, time: u64) -> Result<()> {
        let _guard = self.crash.scoped();
        if time < self.front.time {
            return Err(Error::invalid("producer time moved backwards"));
        }
        self.usable()?;
        if self.front.finished {
            return Err(Error::State("producer is finished"));
        }
        if time == self.front.time {
            return Ok(());
        }
        self.front.time = time;
        let result = match &mut self.dispatch {
            Dispatch::Inline(backend) => backend.advance(time),
            Dispatch::Threaded(transport) => {
                let (sender, shared) = transport.capture_parts();
                sender
                    .publish_progress(time, || shared.error())
                    .map(|()| shared.captured())
            }
        };
        if let Err(error) = &result {
            Self::capture_failure(&mut self.failure, error);
        }
        result
    }

    #[inline]
    fn emit_u64(&mut self, signal: SignalId, value: u64) -> Result<()> {
        self.capture_signal(signal, SignalInput::Unsigned(value))
    }
    #[inline]
    fn emit_real(&mut self, signal: SignalId, value: f64) -> Result<()> {
        self.capture_signal(signal, SignalInput::Real(value.to_bits()))
    }
    fn intern(&mut self, name: &str) -> Result<u32> {
        Writer::intern(self, name)
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if !self.finished {
            drop(self.seal());
        }
    }
}
