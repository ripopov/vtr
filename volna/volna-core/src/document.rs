//! The open trace and everything every view over it must agree on: the
//! session, shared navigation, markers, translators and load bookkeeping.

use std::collections::HashSet;
use std::sync::Arc;

use crate::data::{Hierarchy, SignalRef, Translators};
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};
use crate::wave::viewport::{Viewport, ViewportState};

#[derive(Clone)]
pub enum TraceState {
    Empty,
    Loading { name: String },
    Loaded(Arc<dyn Session>),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub id: u64,
    pub time: u64,
    pub label: Option<String>,
}

pub struct Shared {
    pub viewport: ViewportState,
    pub cursor: Option<u64>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            viewport: ViewportState::new(Viewport::fit((0, 1000))),
            cursor: None,
        }
    }
}

/// A completed load the document accepted (its generation was current).
pub enum Delivered {
    Signals(crate::session::SignalLoads),
    Opened(anyhow::Result<Arc<dyn Session>>),
}

pub struct Document {
    state: TraceState,
    /// A newer open, explicit session replacement, or close invalidates old results.
    generation: u64,
    pub shared: Shared,
    pub markers: Vec<Marker>,
    next_marker: u64,
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
            shared: Shared::default(),
            markers: Vec::new(),
            next_marker: 1,
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
        self.shared.viewport.set(Viewport::fit(self.limits()));
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
        self.shared = Shared::default();
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
        if let Some(LoadRequest::Signals {
            signals,
            generation,
            ..
        }) = self.requests.last_mut()
            && *generation == self.generation
        {
            signals.push(signal);
        } else {
            self.requests.push(LoadRequest::Signals {
                generation: self.generation,
                session,
                signals: vec![signal],
            });
        }
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
            LoadResult::Signals {
                generation,
                results,
            } => {
                if generation != self.generation {
                    return None;
                }
                for (signal, _) in &results {
                    self.pending.remove(signal);
                }
                Some(Delivered::Signals(results))
            }
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
        }
    }

    // -- cursor and markers ----------------------------------------------------------

    /// Install already validated marker identities, preserving monotonic allocation.
    pub(crate) fn restore_markers(&mut self, markers: Vec<Marker>) {
        self.next_marker = self
            .next_marker
            .max(markers.iter().map(|m| m.id).max().unwrap_or(0) + 1);
        self.markers = markers;
    }

    pub fn add_marker(&mut self, c: u64) -> bool {
        if self.markers.iter().any(|m| m.time == c) {
            return false;
        }
        let Some(next) = self.next_marker.checked_add(1) else {
            return false;
        };
        self.markers.push(Marker {
            id: self.next_marker,
            time: c,
            label: None,
        });
        self.next_marker = next;
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
