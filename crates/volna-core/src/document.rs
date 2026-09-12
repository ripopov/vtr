//! The open trace and everything every view over it must agree on: the
//! session, the cursor, markers, translators and the load bookkeeping.

use std::collections::HashSet;
use std::sync::Arc;

use crate::data::{Hierarchy, SignalHistory, SignalRef, Translators};
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};

#[derive(Clone)]
pub enum TraceState {
    Empty,
    Loading { name: String },
    Loaded(Arc<dyn Session>),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    pub time: u64,
}

/// A completed load the document accepted (its generation was current).
pub enum Delivered {
    Opened(anyhow::Result<Arc<dyn Session>>),
    Signal {
        signal: SignalRef,
        result: anyhow::Result<Arc<dyn SignalHistory>>,
    },
}

pub struct Document {
    state: TraceState,
    /// A newer open, explicit session replacement, or close invalidates old results.
    generation: u64,
    pub cursor: Option<u64>,
    pub markers: Vec<Marker>,
    pub translators: Translators,
    pending: HashSet<SignalRef>,
    requests: Vec<LoadRequest>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Document {
            state: TraceState::Empty,
            generation: 0,
            cursor: None,
            markers: Vec::new(),
            translators: Translators::builtin(),
            pending: HashSet::new(),
            requests: Vec::new(),
        }
    }

    pub fn state(&self) -> &TraceState {
        &self.state
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn session(&self) -> Option<&Arc<dyn Session>> {
        match &self.state {
            TraceState::Loaded(s) => Some(s),
            _ => None,
        }
    }

    pub fn hierarchy(&self) -> Option<&Hierarchy> {
        self.session().map(|s| s.hierarchy())
    }

    pub fn is_loaded(&self) -> bool {
        self.session().is_some()
    }

    /// Name of the trace being shown or loaded.
    pub fn name(&self) -> Option<String> {
        match &self.state {
            TraceState::Loaded(s) => Some(s.info().name.clone()),
            TraceState::Loading { name } => Some(name.clone()),
            _ => None,
        }
    }

    pub fn limits(&self) -> (u64, u64) {
        self.session()
            .map(|s| s.info().time_range)
            .unwrap_or((0, 1000))
    }

    pub fn timescale(&self) -> i8 {
        self.session().map(|s| s.info().timescale).unwrap_or(-9)
    }

    // -- opening ----------------------------------------------------------------

    /// Start opening a trace; the previous one stays until the new one arrives.
    pub fn open(&mut self, spec: OpenSpec) {
        self.generation += 1;
        self.state = TraceState::Loading { name: spec.name() };
        self.requests.push(LoadRequest::Open {
            generation: self.generation,
            spec,
        });
    }

    /// Replace the session immediately (tests, demos, hosts that already hold one).
    pub fn set_session(&mut self, session: Arc<dyn Session>) {
        self.generation += 1;
        self.reset_state();
        self.state = TraceState::Loaded(session);
    }

    pub fn close(&mut self) {
        self.generation += 1;
        self.reset_state();
        self.state = TraceState::Empty;
    }

    fn reset_state(&mut self) {
        self.pending.clear();
        self.requests
            .retain(|r| matches!(r, LoadRequest::Open { .. }));
        self.cursor = None;
        self.markers.clear();
    }

    // -- signal loads --------------------------------------------------------------

    /// Queue a history load unless one is already pending. Returns whether a
    /// request was queued.
    pub fn request_signal(&mut self, signal: SignalRef) -> bool {
        let Some(session) = self.session().cloned() else {
            return false;
        };
        if !self.pending.insert(signal) {
            return false;
        }
        self.requests.push(LoadRequest::Signal {
            generation: self.generation,
            session,
            signal,
        });
        true
    }

    pub fn is_pending(&self, signal: SignalRef) -> bool {
        self.pending.contains(&signal)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Requests queued since the last call. The frontend performs them.
    pub fn take_requests(&mut self) -> Vec<LoadRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Accept a completed load if its generation is still current.
    pub fn deliver(&mut self, result: LoadResult) -> Option<Delivered> {
        match result {
            LoadResult::Opened { generation, result } => {
                if generation != self.generation {
                    return None;
                }
                match result {
                    Ok(session) => {
                        self.set_session(session.clone());
                        Some(Delivered::Opened(Ok(session)))
                    }
                    Err(e) => {
                        self.state = TraceState::Error(format!("{e:#}"));
                        Some(Delivered::Opened(Err(e)))
                    }
                }
            }
            LoadResult::Signal {
                generation,
                signal,
                result,
            } => {
                if generation != self.generation {
                    return None;
                }
                self.pending.remove(&signal);
                Some(Delivered::Signal { signal, result })
            }
        }
    }

    // -- cursor and markers ----------------------------------------------------------

    pub fn set_cursor(&mut self, t: Option<u64>) -> bool {
        if self.cursor != t {
            self.cursor = t;
            true
        } else {
            false
        }
    }

    pub fn add_marker_at_cursor(&mut self) -> bool {
        let Some(c) = self.cursor else { return false };
        if self.markers.iter().any(|m| m.time == c) {
            return false;
        }
        self.markers.push(Marker { time: c });
        self.markers.sort_by_key(|m| m.time);
        true
    }

    pub fn clear_markers(&mut self) {
        self.markers.clear();
    }

    pub fn remove_marker(&mut self, ix: usize) -> bool {
        if ix < self.markers.len() {
            self.markers.remove(ix);
            true
        } else {
            false
        }
    }
}
