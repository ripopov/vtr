//! Viewer work: raw trace loads and client-side analysis, tagged by document generation.

use crate::trace::{TraceId, Traced};
use std::sync::Arc;
use volna_trace::data::SignalRef;
use volna_trace::session::{OpenSpec, Session, SignalLoads};

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
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    Track {
        trace: TraceId,
        generation: u64,
        request_id: u64,
        session: Arc<dyn Session>,
        track: volna_trace::data::transactions::TrackRef,
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
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    /// Classify every signal of a trace in `window` (trace times) with its
    /// activity index and count per scope (client-side work).
    Activity {
        trace: TraceId,
        generation: u64,
        window: (u64, u64),
        index: Arc<volna_trace::data::ActivityIndex>,
        counter: Arc<crate::data::ActivityCounter>,
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    /// Read the signals `counts` leaves undecided from the trace and count
    /// them in: the window's exact answer.
    ResolveActivity {
        trace: TraceId,
        generation: u64,
        session: Arc<dyn Session>,
        counter: Arc<crate::data::ActivityCounter>,
        counts: Arc<crate::data::ActivityCounts>,
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    /// Summarize a resident history for analog drawing (client-side work).
    Summary {
        generation: u64,
        signal: Traced<SignalRef>,
        history: Arc<dyn volna_trace::data::SignalHistory>,
        kind: crate::data::NumericKind,
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    /// Summarize a folded group's signals over `range` (client-side work).
    GroupSummary {
        generation: u64,
        members: Vec<Arc<dyn volna_trace::data::SignalHistory>>,
        range: (u64, u64),
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    /// Summarize a long layer of a stacked group for column means
    /// (client-side work).
    Integral {
        generation: u64,
        history: Arc<dyn volna_trace::data::SignalHistory>,
        reading: crate::wave::stack::Reading,
        budget: volna_trace::remote::memory::MemoryBudget,
    },
    /// Walk a stacked group's layers once for its whole-trace scale
    /// (client-side work).
    StackTotal {
        generation: u64,
        layers: Vec<crate::wave::stack::Layer>,
        range: (u64, u64),
        budget: volna_trace::remote::memory::MemoryBudget,
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
                result: session.load_track(track).and_then(|raw| {
                    crate::data::loaded_tracks::LoadedTrack::prepare(
                        raw,
                        session.memory_budget().as_ref(),
                    )
                }),
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
                result: Ok(Arc::new(volna_trace::data::ScopeSizes::count(
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
        track: volna_trace::data::transactions::TrackRef,
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
        result: anyhow::Result<Arc<volna_trace::data::ScopeSizes>>,
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
