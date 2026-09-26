//! The boundary through which all trace data is reached.
//!
//! A [`Session`] is an opened trace: its metadata, its hierarchy and its signal
//! histories on demand. [`LocalSession`] wraps VTR's reader; the private FST
//! adapter wraps fst-reader. Both produce the same immutable history contract.
//! Remote loading uses these same complete objects; the client performs
//! navigation and queries after loading. Selected histories and tracks must
//! fit in client memory. See `docs/client-server-simple.html`.
//!
//! Loading is pull based so it fits any executor: the document queues
//! [`LoadRequest`]s, the frontend performs them wherever it likes (a thread, a
//! task, the browser's main loop) and hands the [`LoadResult`] back to
//! [`crate::app::App::deliver`]. Results carry the generation they were requested
//! under; a newer open or close makes them no-ops.

use std::sync::Arc;

use crate::data::synth::SynthSource;
use crate::data::{Hierarchy, SignalHistory, SignalRef, TraceInfo};

pub use crate::data::vtr_source::LocalSession;

/// Results are in request order, including duplicates. Invalid or unsupported
/// identities have per-signal errors; a shared I/O/decode failure can fail the
/// whole batch. Duplicate successful identities share an immutable history.
pub type SignalLoads = Vec<(SignalRef, anyhow::Result<Arc<dyn SignalHistory>>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    pub waveforms: bool,
    pub transactions: bool,
    pub relations: bool,
}

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
    /// Load all records and incident relations of a stream or generator.
    /// This is blocking work for a loader executor, never a painting call.
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
    /// The procedural stress trace with this many transitions on its busiest signal.
    Synthetic(usize),
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
            OpenSpec::Synthetic(n) => format!("synthetic {n}"),
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
                    Ok(Arc::new(crate::data::fst_source::FstSession::open(
                        name,
                        Box::new(input),
                    )?))
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
            OpenSpec::Synthetic(n) => Ok(Arc::new(SynthSource::new(n))),
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
pub enum LoadRequest {
    Track {
        generation: u64,
        request_id: u64,
        session: Arc<dyn Session>,
        track: crate::data::transactions::TrackRef,
    },
    Open {
        generation: u64,
        spec: OpenSpec,
    },
    Signals {
        generation: u64,
        session: Arc<dyn Session>,
        signals: Vec<SignalRef>,
    },
    /// Summarize a resident history for analog drawing (client-side work).
    Summary {
        generation: u64,
        signal: SignalRef,
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
}

impl LoadRequest {
    /// Transport routing for an already opened recording. Opening is routed
    /// from its OpenSpec before a server identity exists.
    pub fn remote_id(&self) -> Option<u64> {
        match self {
            Self::Signals { session, .. } | Self::Track { session, .. } => session.remote_id(),
            Self::Open { .. } | Self::Summary { .. } | Self::GroupSummary { .. } => None,
        }
    }

    /// Complete failed work with its original document and object identities.
    pub fn fail(self, error: anyhow::Error) -> LoadResult {
        match self {
            Self::Open { generation, .. } => LoadResult::Opened {
                generation,
                result: Err(error),
            },
            Self::Signals {
                generation,
                signals,
                ..
            } => LoadResult::Signals {
                generation,
                results: signals
                    .into_iter()
                    .map(|id| (id, Err(anyhow::anyhow!("{error:#}"))))
                    .collect(),
            },
            Self::Track {
                generation,
                request_id,
                track,
                ..
            } => LoadResult::Track {
                generation,
                request_id,
                track,
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
        }
    }

    /// Perform the request. Blocking; run it off the UI thread where possible.
    pub fn perform(self) -> LoadResult {
        match self {
            LoadRequest::Track {
                generation,
                request_id,
                session,
                track,
            } => LoadResult::Track {
                generation,
                request_id,
                track,
                result: session.load_track(track),
            },
            LoadRequest::Open { generation, spec } => LoadResult::Opened {
                generation,
                result: spec.open(),
            },
            LoadRequest::Signals {
                generation,
                session,
                signals,
            } => LoadResult::Signals {
                generation,
                results: session.load_signals(&signals),
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
        }
    }
}

/// The outcome of a [`LoadRequest`], to hand to [`crate::app::App::deliver`].
pub enum LoadResult {
    Track {
        generation: u64,
        request_id: u64,
        track: crate::data::transactions::TrackRef,
        result: anyhow::Result<crate::data::loaded_tracks::LoadedTrack>,
    },
    Signals {
        generation: u64,
        results: SignalLoads,
    },
    Opened {
        generation: u64,
        result: anyhow::Result<Arc<dyn Session>>,
    },
    Summary {
        generation: u64,
        signal: SignalRef,
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
}
