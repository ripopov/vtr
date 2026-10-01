//! The boundary through which all trace data is reached.
//!
//! A [`Session`] is an opened trace: its metadata, its hierarchy and its signal
//! histories on demand. [`LocalSession`] wraps VTR's reader; the private FST
//! adapter wraps fst-reader. Both produce the same immutable history contract.
//! Remote loading uses these same complete objects; the client performs
//! navigation and queries after loading. Selected histories and tracks must
//! fit in client memory. See `volna/volna/ARCHITECTURE.md`, "The session seam".
//!
//! Loading is pull based so it fits any executor: the document queues
//! [`LoadRequest`]s, the frontend performs them wherever it likes (a thread, a
//! task, the browser's main loop) and hands the [`LoadResult`] back to
//! [`crate::app::App::deliver`]. Results carry their trace and the generation
//! they were requested under; a newer open, a close, or a change of the
//! trace's placement makes them no-ops.

use std::sync::Arc;

use crate::data::{Hierarchy, SignalHistory, SignalRef, TraceInfo};
use crate::trace::{TraceId, Traced};

pub use crate::data::vtr_source::LocalSession;

/// Results are in request order, including duplicates. Invalid or unsupported
/// identities have per-signal errors; a shared I/O/decode failure can fail the
/// whole batch. Duplicate successful identities share an immutable history.
pub type SignalLoads = Vec<(SignalRef, anyhow::Result<Arc<dyn SignalHistory>>)>;

/// Kinds of data a backend can serve: VTR supports all three, FST waveforms only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    pub waveforms: bool,
    pub transactions: bool,
    pub relations: bool,
}

/// An opened trace.
///
/// [`info`](Self::info), [`hierarchy`](Self::hierarchy) and
/// [`tracks`](Self::tracks) are resident metadata. The `load_*` methods are
/// blocking complete-object queries for a loader executor; their results are
/// immutable, shared and remain valid after the session is dropped.
/// `SignalRef`, `TrackRef` and `TransactionRef` belong to this session and must
/// not be reused after it is replaced. Remote sessions reject the blocking
/// loads; their requests go through [`crate::remote::client::RemoteClient`].
pub trait Session: Send + Sync {
    /// Bytes retained by the opened session before on-demand signal/track
    /// owners are loaded (mapped/input image, hierarchy and backend indexes).
    fn resident_bytes(&self) -> u64 {
        0
    }

    /// Shared admission pool for resident raw data and client-side indexes.
    /// Local sessions may return `None`; the application then supplies its
    /// process-local pool so native and remote table construction use the
    /// same ownership path.
    fn memory_budget(&self) -> Option<crate::remote::memory::MemoryBudget> {
        None
    }

    /// The file format it was read from (`VTR`, `FST`), as a trace chip
    /// names it; `None` when the reader does not say (a remote recording).
    fn format(&self) -> Option<&'static str> {
        None
    }

    /// Server identity for asynchronous executor routing. Local sessions return
    /// None and use the blocking load methods on a background executor.
    fn remote_id(&self) -> Option<u64> {
        None
    }
    /// Resident raw track catalog; reading it performs no I/O.
    fn tracks(&self) -> &[crate::data::transactions::Track] {
        &[]
    }
    /// Capabilities describe operations, not whether this recording has rows.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            waveforms: true,
            transactions: false,
            relations: false,
        }
    }
    fn info(&self) -> &TraceInfo;
    fn hierarchy(&self) -> &Hierarchy;
    /// Precomputed scope sizes, when supplied with resident remote metadata.
    fn scope_sizes(&self) -> Option<Arc<crate::data::ScopeSizes>> {
        None
    }
    /// The trace's activity index (docs/hierarchy-activity.html), when a
    /// valid sidecar was found at open or built later by the session.
    /// Resident and shared; counted in [`resident_bytes`](Self::resident_bytes).
    fn activity(&self) -> Option<Arc<crate::data::ActivityIndex>> {
        None
    }
    /// Whether this session's reader can build an activity sidecar. Byte
    /// images and recovered files have no stable build source.
    fn activity_build_info(&self) -> Option<crate::data::ActivityBuildInfo> {
        None
    }
    /// Stable raw recording identity for sidecar validation.
    fn activity_identity(&self) -> Option<vtr::activity::Identity> {
        None
    }
    /// Read a validated raw sidecar image for transport beside the reader.
    fn activity_image(&self, _cache_dir: Option<&std::path::Path>) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("this session has no activity sidecar")
    }
    /// Host of a remote reader, for the build offer.
    fn activity_host(&self) -> Option<&str> {
        None
    }
    /// A remote reader already has an index that the client should fetch.
    fn activity_available(&self) -> bool {
        self.activity().is_some()
    }
    /// Install a fully validated index received by the remote executor.
    /// Blocking local readers reject this operation; their builder owns installation.
    fn install_activity(&self, _index: Arc<crate::data::ActivityIndex>) -> anyhow::Result<()> {
        anyhow::bail!("this session does not accept remote activity indexes")
    }
    /// Builds and installs an immutable activity index through the library.
    /// Blocking work for the loader; supply a control in `options` for
    /// progress and cancellation. The late-built index is charged to `budget`.
    fn build_activity(
        &self,
        _options: &vtr::activity::BuildOptions,
        _budget: &crate::remote::memory::MemoryBudget,
        _cache_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("this session cannot build an activity index")
    }
    /// The signals among `signals` that change in `[t0, t1]` (trace times,
    /// both included), read from the trace: the exact answer for those the
    /// activity index leaves undecided. Blocking work for a loader executor.
    fn resolve_activity(
        &self,
        _signals: &[SignalRef],
        _t0: u64,
        _t1: u64,
    ) -> anyhow::Result<Vec<SignalRef>> {
        anyhow::bail!("this trace cannot read activity")
    }
    /// Load all records and incident relations of a stream or generator,
    /// including empty member generators. Invalid identities and unsupported
    /// backends return errors. This is blocking work for a loader executor,
    /// never a painting call.
    fn load_track(
        &self,
        _track: crate::data::transactions::TrackRef,
    ) -> anyhow::Result<crate::data::loaded_tracks::LoadedTrack> {
        anyhow::bail!("transaction track loading is unsupported")
    }
    /// Load the full change history of a signal. May be slow; call off the UI thread.
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>>;

    /// Load complete histories together. Backends can override this to share a
    /// filtered read. This is an expensive query, not resident metadata access.
    /// See [`SignalLoads`] for result order and errors.
    fn load_signals(&self, signals: &[SignalRef]) -> SignalLoads {
        let mut loaded = std::collections::BTreeMap::new();
        signals
            .iter()
            .map(|&signal| {
                let result = loaded
                    .entry(signal)
                    .or_insert_with(|| self.load_signal(signal));
                (
                    signal,
                    match result {
                        Ok(history) => Ok(Arc::clone(history)),
                        Err(error) => Err(anyhow::anyhow!("{error:#}")),
                    },
                )
            })
            .collect()
    }
}

/// Give an in-process backend the same admission owner used by remote data and
/// table allocations. Remote sessions already return a budget and are kept as
/// they are.
pub(crate) fn account_local_session(
    session: Arc<dyn Session>,
    budget: crate::remote::memory::MemoryBudget,
) -> anyhow::Result<Arc<dyn Session>> {
    if session.memory_budget().is_some() {
        return Ok(session);
    }
    let resident_bytes = session.resident_bytes();
    // Procedural/test sessions with no resident raw owner need no wrapper;
    // App's fallback still gives their tables this same process-local budget.
    if resident_bytes == 0 {
        return Ok(session);
    }
    let reservation = budget.reserve(resident_bytes)?;
    Ok(Arc::new(AccountedSession {
        inner: session,
        budget,
        _reservation: reservation,
    }))
}

/// A local trace charged to the shared budget. Loads check the budget's
/// current object limit, so a raised `memory.objectMiB` applies on retry.
struct AccountedSession {
    inner: Arc<dyn Session>,
    budget: crate::remote::memory::MemoryBudget,
    _reservation: crate::remote::memory::Reservation,
}

impl Session for AccountedSession {
    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }
    fn memory_budget(&self) -> Option<crate::remote::memory::MemoryBudget> {
        Some(self.budget.clone())
    }
    fn format(&self) -> Option<&'static str> {
        self.inner.format()
    }
    fn tracks(&self) -> &[crate::data::transactions::Track] {
        self.inner.tracks()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn info(&self) -> &TraceInfo {
        self.inner.info()
    }
    fn hierarchy(&self) -> &Hierarchy {
        self.inner.hierarchy()
    }
    fn scope_sizes(&self) -> Option<Arc<crate::data::ScopeSizes>> {
        self.inner.scope_sizes()
    }
    fn activity(&self) -> Option<Arc<crate::data::ActivityIndex>> {
        self.inner.activity()
    }
    fn activity_build_info(&self) -> Option<crate::data::ActivityBuildInfo> {
        self.inner.activity_build_info()
    }
    fn activity_identity(&self) -> Option<vtr::activity::Identity> {
        self.inner.activity_identity()
    }
    fn activity_image(&self, cache_dir: Option<&std::path::Path>) -> anyhow::Result<Vec<u8>> {
        self.inner.activity_image(cache_dir)
    }
    fn build_activity(
        &self,
        options: &vtr::activity::BuildOptions,
        budget: &crate::remote::memory::MemoryBudget,
        cache_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<()> {
        self.inner.build_activity(options, budget, cache_dir)
    }
    fn resolve_activity(
        &self,
        signals: &[SignalRef],
        t0: u64,
        t1: u64,
    ) -> anyhow::Result<Vec<SignalRef>> {
        self.inner.resolve_activity(signals, t0, t1)
    }
    fn load_track(
        &self,
        track: crate::data::transactions::TrackRef,
    ) -> anyhow::Result<crate::data::loaded_tracks::LoadedTrack> {
        let mut loaded = self.inner.load_track(track)?;
        for generator in &mut loaded.generators {
            let Some(generator) = Arc::get_mut(generator) else {
                anyhow::bail!("new local track unexpectedly shared its generator owner");
            };
            if generator.reservation.is_none() {
                generator.reservation = Some(
                    self.budget
                        .reserve_object("a generator", generator.resident_bytes())?,
                );
            }
        }
        Ok(loaded)
    }
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        let history = self.inner.load_signal(signal)?;
        account_history(history, &self.budget)
    }
    fn load_signals(&self, signals: &[SignalRef]) -> SignalLoads {
        let mut accounted = std::collections::BTreeMap::new();
        self.inner
            .load_signals(signals)
            .into_iter()
            .map(|(signal, result)| {
                let result = match accounted.get(&signal) {
                    Some(history) => Ok(Arc::clone(history)),
                    None => result
                        .and_then(|history| account_history(history, &self.budget))
                        .inspect(|history| {
                            accounted.insert(signal, Arc::clone(history));
                        }),
                };
                (signal, result)
            })
            .collect()
    }
}

fn account_history(
    inner: Arc<dyn SignalHistory>,
    budget: &crate::remote::memory::MemoryBudget,
) -> anyhow::Result<Arc<dyn SignalHistory>> {
    let reservation = budget.reserve_object("the signal history", inner.resident_bytes())?;
    Ok(Arc::new(AccountedHistory {
        inner,
        _reservation: reservation,
    }))
}

struct AccountedHistory {
    inner: Arc<dyn SignalHistory>,
    _reservation: crate::remote::memory::Reservation,
}

impl SignalHistory for AccountedHistory {
    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }
    fn shape(&self) -> crate::data::SignalShape {
        self.inner.shape()
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn time(&self, index: usize) -> u64 {
        self.inner.time(index)
    }
    fn value(&self, index: Option<usize>) -> crate::data::WaveValue {
        self.inner.value(index)
    }
    fn value_view(&self, index: Option<usize>) -> crate::data::value_view::ValueView<'_> {
        self.inner.value_view(index)
    }
    fn always_normal(&self) -> bool {
        self.inner.always_normal()
    }
    fn bit(&self, index: Option<usize>) -> crate::data::Bit {
        self.inner.bit(index)
    }
    fn index_at(&self, time: u64) -> Option<usize> {
        self.inner.index_at(time)
    }
    fn index_at_hint(&self, time: u64, hint: usize) -> Option<usize> {
        self.inner.index_at_hint(time, hint)
    }
}

/// What to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenSpec {
    /// Recording opened by the workspace host's remote executor.
    Remote {
        name: String,
        limits: crate::remote::limits::Limits,
    },
    /// A trace on disk: memory-mapped VTR or buffered FST.
    #[cfg(not(target_family = "wasm"))]
    Path(std::path::PathBuf),
    /// A VTR or FST image already in memory (web hosts, drag and drop of bytes).
    Bytes { name: String, bytes: Vec<u8> },
}

impl OpenSpec {
    /// Display name while loading.
    pub fn name(&self) -> String {
        match self {
            OpenSpec::Remote { name, .. } => name.clone(),
            #[cfg(not(target_family = "wasm"))]
            OpenSpec::Path(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            OpenSpec::Bytes { name, .. } => name.clone(),
        }
    }

    /// Where it comes from, for naming the trace: the path, or the name.
    pub fn source(&self) -> String {
        match self {
            OpenSpec::Remote { name, .. } | OpenSpec::Bytes { name, .. } => name.clone(),
            #[cfg(not(target_family = "wasm"))]
            OpenSpec::Path(p) => p.to_string_lossy().into_owned(),
        }
    }

    /// Open the trace. Blocking; run it where blocking is acceptable.
    pub fn open(self) -> anyhow::Result<Arc<dyn Session>> {
        match self {
            OpenSpec::Remote { .. } => {
                anyhow::bail!("remote opening requires the asynchronous executor")
            }
            #[cfg(not(target_family = "wasm"))]
            OpenSpec::Path(p) => {
                use std::io::{Read, Seek};
                let mut input = std::io::BufReader::new(std::fs::File::open(&p)?);
                let mut prefix = [0; 9];
                input.read_exact(&mut prefix)?;
                input.rewind()?;
                if is_fst(&prefix)? {
                    let name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let mut session =
                        crate::data::fst_source::FstSession::open(name, Box::new(input))?;
                    session.find_activity(&p);
                    Ok(Arc::new(session))
                } else {
                    Ok(Arc::new(LocalSession::open(&p)?))
                }
            }
            OpenSpec::Bytes { name, bytes } => {
                if is_fst(&bytes)? {
                    Ok(Arc::new(crate::data::fst_source::FstSession::open(
                        name,
                        Box::new(std::io::Cursor::new(bytes)),
                    )?))
                } else {
                    Ok(Arc::new(LocalSession::from_bytes(name, bytes)?))
                }
            }
        }
    }
}

fn is_fst(bytes: &[u8]) -> anyhow::Result<bool> {
    if bytes.starts_with(&vtr::container::FILE_MAGIC) {
        return Ok(false);
    }
    if bytes.len() >= 9
        && (bytes[0] == 254
            || (bytes[0] == 0 && u64::from_be_bytes(bytes[1..9].try_into().unwrap()) == 329))
    {
        return Ok(true);
    }
    anyhow::bail!("unrecognized trace format: expected VTR or FST header")
}

/// Work the document wants done. Obtain with [`crate::app::App::take_requests`].
///
/// Trace loads name their trace and the generation of its slot; the
/// document places what they return on the session timeline as it arrives
/// ([`crate::trace::Placement`]), so executors hand back trace times.
pub enum LoadRequest {
    /// Build a missing local sidecar off the UI thread. Progress and
    /// cancellation travel through `options.control`.
    BuildActivity {
        trace: TraceId,
        generation: u64,
        session: Arc<dyn Session>,
        options: vtr::activity::BuildOptions,
        cache_dir: Option<std::path::PathBuf>,
        budget: crate::remote::memory::MemoryBudget,
    },
    Track {
        trace: TraceId,
        generation: u64,
        request_id: u64,
        session: Arc<dyn Session>,
        track: crate::data::transactions::TrackRef,
    },
    Open {
        trace: TraceId,
        generation: u64,
        spec: OpenSpec,
    },
    Signals {
        trace: TraceId,
        generation: u64,
        session: Arc<dyn Session>,
        signals: Vec<SignalRef>,
    },
    /// Count the distinct signals below every scope of an opened trace
    /// (client-side work over its hierarchy).
    Sizes {
        trace: TraceId,
        generation: u64,
        session: Arc<dyn Session>,
    },
    /// Build the census weights that count a trace's changing signals per
    /// scope (client-side work over its hierarchy), once per trace with an
    /// activity index.
    ActivityCounter {
        trace: TraceId,
        generation: u64,
        session: Arc<dyn Session>,
        budget: crate::remote::memory::MemoryBudget,
    },
    /// Classify every signal of a trace in `window` (trace times) with its
    /// activity index and count per scope (client-side work).
    Activity {
        trace: TraceId,
        generation: u64,
        window: (u64, u64),
        index: Arc<crate::data::ActivityIndex>,
        counter: Arc<crate::data::ActivityCounter>,
        budget: crate::remote::memory::MemoryBudget,
    },
    /// Read the signals `counts` leaves undecided from the trace and count
    /// them in: the window's exact answer.
    ResolveActivity {
        trace: TraceId,
        generation: u64,
        session: Arc<dyn Session>,
        counter: Arc<crate::data::ActivityCounter>,
        counts: Arc<crate::data::ActivityCounts>,
        budget: crate::remote::memory::MemoryBudget,
    },
    /// Summarize a resident history for analog drawing (client-side work).
    Summary {
        generation: u64,
        signal: Traced<SignalRef>,
        history: Arc<dyn crate::data::SignalHistory>,
        kind: crate::data::NumericKind,
        budget: crate::remote::memory::MemoryBudget,
    },
    /// Summarize a folded group's signals over `range` (client-side work).
    GroupSummary {
        generation: u64,
        members: Vec<Arc<dyn crate::data::SignalHistory>>,
        range: (u64, u64),
        budget: crate::remote::memory::MemoryBudget,
    },
    /// Summarize a long layer of a stacked group for column means
    /// (client-side work).
    Integral {
        generation: u64,
        history: Arc<dyn crate::data::SignalHistory>,
        reading: crate::wave::stack::Reading,
        budget: crate::remote::memory::MemoryBudget,
    },
    /// Walk a stacked group's layers once for its whole-trace scale
    /// (client-side work).
    StackTotal {
        generation: u64,
        layers: Vec<crate::wave::stack::Layer>,
        range: (u64, u64),
        budget: crate::remote::memory::MemoryBudget,
    },
}

impl LoadRequest {
    /// Transport routing for an already opened recording. Opening is routed
    /// from its OpenSpec before a server identity exists.
    pub fn remote_id(&self) -> Option<u64> {
        match self {
            Self::Signals { session, .. }
            | Self::Track { session, .. }
            | Self::ResolveActivity { session, .. }
            | Self::BuildActivity { session, .. } => session.remote_id(),
            Self::Open { .. }
            | Self::Sizes { .. }
            | Self::ActivityCounter { .. }
            | Self::Activity { .. }
            | Self::Summary { .. }
            | Self::GroupSummary { .. }
            | Self::Integral { .. }
            | Self::StackTotal { .. } => None,
        }
    }

    /// Complete failed work with its original document and object identities.
    pub fn fail(self, error: anyhow::Error) -> LoadResult {
        match self {
            Self::BuildActivity {
                trace, generation, ..
            } => LoadResult::ActivityBuilt {
                trace,
                generation,
                result: Err(error),
            },
            Self::Open {
                trace, generation, ..
            } => LoadResult::Opened {
                trace,
                generation,
                result: Err(error),
            },
            Self::Signals {
                trace,
                generation,
                signals,
                ..
            } => LoadResult::Signals {
                trace,
                generation,
                results: signals
                    .into_iter()
                    .map(|id| (id, Err(anyhow::anyhow!("{error:#}"))))
                    .collect(),
            },
            Self::Track {
                trace,
                generation,
                request_id,
                track,
                ..
            } => LoadResult::Track {
                trace,
                generation,
                request_id,
                track,
                result: Err(error),
            },
            Self::Sizes {
                trace, generation, ..
            } => LoadResult::Sizes {
                trace,
                generation,
                result: Err(error),
            },
            Self::ActivityCounter {
                trace, generation, ..
            } => LoadResult::ActivityCounter {
                trace,
                generation,
                result: Err(error),
            },
            Self::Activity {
                trace,
                generation,
                window,
                ..
            } => LoadResult::Activity {
                trace,
                generation,
                window,
                result: Err(error),
            },
            Self::ResolveActivity {
                trace,
                generation,
                counts,
                ..
            } => LoadResult::ActivityResolved {
                trace,
                generation,
                window: counts.window,
                result: Err(error),
            },
            Self::Summary {
                generation,
                signal,
                history,
                kind,
                ..
            } => LoadResult::Summary {
                generation,
                signal,
                kind,
                history: crate::wave::analog::history_identity(&history),
                result: Err(error),
            },
            Self::GroupSummary {
                generation,
                members,
                ..
            } => LoadResult::GroupSummary {
                generation,
                key: crate::wave::group::key(&members),
                result: Err(error),
            },
            Self::Integral {
                generation,
                history,
                reading,
                ..
            } => LoadResult::Integral {
                generation,
                history: crate::wave::analog::history_identity(&history),
                reading,
                result: Err(error),
            },
            Self::StackTotal {
                generation, layers, ..
            } => LoadResult::StackTotal {
                generation,
                key: crate::wave::stack::key(&layers),
                result: Err(error),
            },
        }
    }

    /// Perform the request. Blocking; run it off the UI thread where possible.
    pub fn perform(self) -> LoadResult {
        match self {
            Self::BuildActivity {
                trace,
                generation,
                session,
                options,
                cache_dir,
                budget,
            } => LoadResult::ActivityBuilt {
                trace,
                generation,
                result: session.build_activity(&options, &budget, cache_dir.as_deref()),
            },
            LoadRequest::Track {
                trace,
                generation,
                request_id,
                session,
                track,
            } => LoadResult::Track {
                trace,
                generation,
                request_id,
                track,
                result: session.load_track(track),
            },
            LoadRequest::Open {
                trace,
                generation,
                spec,
            } => LoadResult::Opened {
                trace,
                generation,
                result: spec.open(),
            },
            LoadRequest::Signals {
                trace,
                generation,
                session,
                signals,
            } => LoadResult::Signals {
                trace,
                generation,
                results: session.load_signals(&signals),
            },
            LoadRequest::Sizes {
                trace,
                generation,
                session,
            } => LoadResult::Sizes {
                trace,
                generation,
                result: Ok(Arc::new(crate::data::ScopeSizes::count(
                    session.hierarchy(),
                ))),
            },
            LoadRequest::ActivityCounter {
                trace,
                generation,
                session,
                budget,
            } => LoadResult::ActivityCounter {
                trace,
                generation,
                result: crate::data::ActivityCounter::build(session.hierarchy())
                    .account(&budget)
                    .map(Arc::new),
            },
            LoadRequest::Activity {
                trace,
                generation,
                window,
                index,
                counter,
                budget,
            } => LoadResult::Activity {
                trace,
                generation,
                window,
                result: crate::data::ActivityCounts::classify(&index, &counter, window)
                    .account(&budget)
                    .map(Arc::new),
            },
            LoadRequest::ResolveActivity {
                trace,
                generation,
                session,
                counter,
                counts,
                budget,
            } => LoadResult::ActivityResolved {
                trace,
                generation,
                window: counts.window,
                result: session
                    .resolve_activity(&counts.undecided, counts.window.0, counts.window.1)
                    .and_then(|changed| counts.resolved(&counter, &changed).account(&budget))
                    .map(Arc::new),
            },
            LoadRequest::Summary {
                generation,
                signal,
                history,
                kind,
                budget,
            } => LoadResult::Summary {
                generation,
                signal,
                kind,
                history: crate::wave::analog::history_identity(&history),
                result: crate::wave::analog::AnalogSummary::build(&history, kind)
                    .account(&budget)
                    .map(Arc::new),
            },
            LoadRequest::GroupSummary {
                generation,
                members,
                range,
                budget,
            } => {
                let summary = crate::wave::group::GroupSummary::build(&members, range);
                LoadResult::GroupSummary {
                    generation,
                    key: summary.key().to_vec(),
                    result: summary.account(&budget).map(Arc::new),
                }
            }
            LoadRequest::Integral {
                generation,
                history,
                reading,
                budget,
            } => LoadResult::Integral {
                generation,
                history: crate::wave::analog::history_identity(&history),
                reading,
                result: crate::wave::stack::IntegralSummary::build(&history, reading)
                    .account(&budget)
                    .map(Arc::new),
            },
            LoadRequest::StackTotal {
                generation,
                layers,
                range,
                budget,
            } => {
                let summary = crate::wave::stack::TotalSummary::build(&layers, range);
                LoadResult::StackTotal {
                    generation,
                    key: summary.key().clone(),
                    result: summary.account(&budget).map(Arc::new),
                }
            }
        }
    }
}

/// The outcome of a [`LoadRequest`], to hand to [`crate::app::App::deliver`].
pub enum LoadResult {
    ActivityBuilt {
        trace: TraceId,
        generation: u64,
        result: anyhow::Result<()>,
    },
    Track {
        trace: TraceId,
        generation: u64,
        request_id: u64,
        track: crate::data::transactions::TrackRef,
        result: anyhow::Result<crate::data::loaded_tracks::LoadedTrack>,
    },
    Signals {
        trace: TraceId,
        generation: u64,
        results: SignalLoads,
    },
    Opened {
        trace: TraceId,
        generation: u64,
        result: anyhow::Result<Arc<dyn Session>>,
    },
    Sizes {
        trace: TraceId,
        generation: u64,
        result: anyhow::Result<Arc<crate::data::ScopeSizes>>,
    },
    ActivityCounter {
        trace: TraceId,
        generation: u64,
        result: anyhow::Result<Arc<crate::data::ActivityCounter>>,
    },
    /// What the activity index says about `window` (trace times).
    Activity {
        trace: TraceId,
        generation: u64,
        window: (u64, u64),
        result: anyhow::Result<Arc<crate::data::ActivityCounts>>,
    },
    /// `window`'s exact counts, its undecided signals read from the trace.
    ActivityResolved {
        trace: TraceId,
        generation: u64,
        window: (u64, u64),
        result: anyhow::Result<Arc<crate::data::ActivityCounts>>,
    },
    Summary {
        generation: u64,
        signal: Traced<SignalRef>,
        kind: crate::data::NumericKind,
        /// Identity of the summarized history.
        history: usize,
        result: anyhow::Result<Arc<crate::wave::analog::AnalogSummary>>,
    },
    GroupSummary {
        generation: u64,
        /// Identity of the summarized signals ([`crate::wave::group::key`]).
        key: Vec<usize>,
        result: anyhow::Result<Arc<crate::wave::group::GroupSummary>>,
    },
    Integral {
        generation: u64,
        /// Identity of the summarized history.
        history: usize,
        reading: crate::wave::stack::Reading,
        result: anyhow::Result<Arc<crate::wave::stack::IntegralSummary>>,
    },
    StackTotal {
        generation: u64,
        /// Identity of the stacked layers ([`crate::wave::stack::key`]).
        key: crate::wave::stack::Key,
        result: anyhow::Result<Arc<crate::wave::stack::TotalSummary>>,
    },
}
