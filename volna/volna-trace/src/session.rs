//! Recording access shared by local and remote loaders. Blocking queries run on a host executor.

use std::sync::Arc;

use crate::data::{Hierarchy, SignalHistory, SignalRef, TraceInfo};

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
    /// Local sessions may return `None`; callers can supply a process-local
    /// pool through [`account_local_session`] for the same admission path.
    fn memory_budget(&self) -> Option<crate::remote::memory::MemoryBudget> {
        None
    }

    /// Source format (`VTR`, `FST`), or `None` when a reader does not report it.
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
/// derived allocations. Remote sessions already return a budget and are kept as
/// they are.
pub fn account_local_session(
    session: Arc<dyn Session>,
    budget: crate::remote::memory::MemoryBudget,
) -> anyhow::Result<Arc<dyn Session>> {
    if session.memory_budget().is_some() {
        return Ok(session);
    }
    let resident_bytes = session.resident_bytes();
    // Procedural/test sessions with no resident raw owner need no wrapper;
    // Their caller can use the same pool for derived allocations.
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

/// Raw loading work. `Tag` is an opaque caller identity, never interpreted or sent on the wire.
/// Generation and request IDs correlate completions; they do not store viewer state.
pub enum LoadRequest<Tag = u64> {
    BuildActivity {
        tag: Tag,
        generation: u64,
        session: Arc<dyn Session>,
        options: vtr::activity::BuildOptions,
        cache_dir: Option<std::path::PathBuf>,
        budget: crate::remote::memory::MemoryBudget,
    },
    Track {
        tag: Tag,
        generation: u64,
        request_id: u64,
        session: Arc<dyn Session>,
        track: crate::data::transactions::TrackRef,
    },
    Signals {
        tag: Tag,
        generation: u64,
        session: Arc<dyn Session>,
        signals: Vec<SignalRef>,
    },
    /// Exact raw changing-signal identities, using complete histories remotely.
    ResolveActivity {
        tag: Tag,
        generation: u64,
        session: Arc<dyn Session>,
        signals: Vec<SignalRef>,
        window: (u64, u64),
        budget: crate::remote::memory::MemoryBudget,
    },
}

impl<Tag: Copy> LoadRequest<Tag> {
    pub fn remote_id(&self) -> Option<u64> {
        match self {
            Self::Signals { session, .. }
            | Self::Track { session, .. }
            | Self::BuildActivity { session, .. }
            | Self::ResolveActivity { session, .. } => session.remote_id(),
        }
    }
    /// Complete a failed load, preserving caller correlation.
    pub fn fail(self, error: anyhow::Error) -> LoadResult<Tag> {
        match self {
            Self::BuildActivity {
                tag, generation, ..
            } => LoadResult::ActivityBuilt {
                tag,
                generation,
                result: Err(error),
            },
            Self::Track {
                tag,
                generation,
                request_id,
                track,
                ..
            } => LoadResult::Track {
                tag,
                generation,
                request_id,
                track,
                result: Err(error),
            },
            Self::Signals {
                tag,
                generation,
                signals,
                ..
            } => LoadResult::Signals {
                tag,
                generation,
                results: signals
                    .into_iter()
                    .map(|id| (id, Err(anyhow::anyhow!("{error:#}"))))
                    .collect(),
            },
            Self::ResolveActivity {
                tag,
                generation,
                window,
                ..
            } => LoadResult::ActivityResolved {
                tag,
                generation,
                window,
                result: Err(error),
            },
        }
    }
    /// Run blocking local work on the caller's executor.
    pub fn perform(self) -> LoadResult<Tag> {
        match self {
            Self::BuildActivity {
                tag,
                generation,
                session,
                options,
                cache_dir,
                budget,
            } => LoadResult::ActivityBuilt {
                tag,
                generation,
                result: session.build_activity(&options, &budget, cache_dir.as_deref()),
            },
            Self::Track {
                tag,
                generation,
                request_id,
                session,
                track,
            } => LoadResult::Track {
                tag,
                generation,
                request_id,
                track,
                result: session.load_track(track),
            },
            Self::Signals {
                tag,
                generation,
                session,
                signals,
            } => LoadResult::Signals {
                tag,
                generation,
                results: session.load_signals(&signals),
            },
            Self::ResolveActivity {
                tag,
                generation,
                session,
                signals,
                window,
                ..
            } => LoadResult::ActivityResolved {
                tag,
                generation,
                window,
                result: session.resolve_activity(&signals, window.0, window.1),
            },
        }
    }
}

/// Complete immutable recording objects or explicit loading errors.
pub enum LoadResult<Tag = u64> {
    Opened {
        tag: Tag,
        generation: u64,
        result: anyhow::Result<Arc<dyn Session>>,
    },
    ActivityBuilt {
        tag: Tag,
        generation: u64,
        result: anyhow::Result<()>,
    },
    Track {
        tag: Tag,
        generation: u64,
        request_id: u64,
        track: crate::data::transactions::TrackRef,
        result: anyhow::Result<crate::data::loaded_tracks::LoadedTrack>,
    },
    Signals {
        tag: Tag,
        generation: u64,
        results: SignalLoads,
    },
    ActivityResolved {
        tag: Tag,
        generation: u64,
        window: (u64, u64),
        result: anyhow::Result<Vec<SignalRef>>,
    },
}
