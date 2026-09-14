//! The boundary through which all trace data is reached.
//!
//! A [`Session`] is an opened trace: its metadata, its hierarchy and its signal
//! histories on demand. [`LocalSession`] wraps VTR's reader; the private FST
//! adapter wraps fst-reader. Both produce the same immutable history contract.
//! Full-history loads are not sufficient for remote viewing: bounded windows
//! and summaries remain necessary before a remote implementation can scale
//! transfer with the viewport.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub waveforms: bool,
    pub transactions: bool,
    pub relations: bool,
}

pub trait Session: Send + Sync {
    /// Start bounded native queries over this same immutable reader, when supported.
    /// The host awaits readiness before exposing the opened document to row loads.
    #[cfg(not(target_family = "wasm"))]
    fn open_queries(
        &self,
        _budget: vtr_query::Budget,
    ) -> Option<vtr_query::Result<vtr_query::local_session::OpenFuture>> {
        None
    }

    /// Capabilities describe operations, not whether this recording has rows.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            waveforms: true,
            transactions: self.transactions().is_some(),
            relations: self.relations().is_some(),
        }
    }
    fn transactions(&self) -> Option<&dyn crate::data::transactions::TransactionQueries> {
        None
    }
    fn relations(&self) -> Option<&dyn crate::data::transactions::RelationQueries> {
        None
    }
    fn info(&self) -> &TraceInfo;
    fn hierarchy(&self) -> &Hierarchy;
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

/// Resident trace header for a query-backed document. Hierarchy pages belong
/// to Document; this header never opens a file or supplies history data.
pub(crate) struct QueryMetadataSession {
    pub info: TraceInfo,
    pub hierarchy: Hierarchy,
}
impl Session for QueryMetadataSession {
    fn info(&self) -> &TraceInfo {
        &self.info
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, _: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        anyhow::bail!("query-backed documents require bounded waveform requests")
    }
}

/// What to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenSpec {
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
    Open {
        generation: u64,
        spec: OpenSpec,
    },
    Signals {
        generation: u64,
        session: Arc<dyn Session>,
        signals: Vec<SignalRef>,
    },
}

impl LoadRequest {
    /// Perform the request. Blocking; run it off the UI thread where possible.
    pub fn perform(self) -> LoadResult {
        match self {
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
        }
    }
}

/// The outcome of a [`LoadRequest`], to hand to [`crate::app::App::deliver`].
pub enum LoadResult {
    Signals {
        generation: u64,
        results: SignalLoads,
    },
    Opened {
        generation: u64,
        result: anyhow::Result<Arc<dyn Session>>,
    },
}
