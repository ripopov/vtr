//! The boundary through which all trace data is reached.
//!
//! A [`Session`] is an opened trace: its metadata, its hierarchy and its signal
//! histories on demand. Today there is one implementation, [`LocalSession`],
//! over the `vtr` reader in this process (memory-mapped natively, an in-memory
//! image on wasm). A remote implementation would answer the same calls over a
//! wire; nothing above this module knows the difference.
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

pub trait Session: Send + Sync {
    fn info(&self) -> &TraceInfo;
    fn hierarchy(&self) -> &Hierarchy;
    /// Load the full change history of a signal. May be slow; call off the UI thread.
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>>;
}

/// What to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenSpec {
    /// A VTR file on disk, memory-mapped by the local session.
    #[cfg(not(target_family = "wasm"))]
    Path(std::path::PathBuf),
    /// A VTR image already in memory (web hosts, drag and drop of bytes).
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
            OpenSpec::Path(p) => Ok(Arc::new(LocalSession::open(&p)?)),
            OpenSpec::Bytes { name, bytes } => Ok(Arc::new(LocalSession::from_bytes(name, bytes)?)),
            OpenSpec::Synthetic(n) => Ok(Arc::new(SynthSource::new(n))),
        }
    }
}

/// Work the document wants done. Obtain with [`crate::app::App::take_requests`].
pub enum LoadRequest {
    Open {
        generation: u64,
        spec: OpenSpec,
    },
    Signal {
        generation: u64,
        session: Arc<dyn Session>,
        signal: SignalRef,
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
            LoadRequest::Signal {
                generation,
                session,
                signal,
            } => LoadResult::Signal {
                generation,
                signal,
                result: session.load_signal(signal),
            },
        }
    }
}

/// The outcome of a [`LoadRequest`], to hand to [`crate::app::App::deliver`].
pub enum LoadResult {
    Opened {
        generation: u64,
        result: anyhow::Result<Arc<dyn Session>>,
    },
    Signal {
        generation: u64,
        signal: SignalRef,
        result: anyhow::Result<Arc<dyn SignalHistory>>,
    },
}
