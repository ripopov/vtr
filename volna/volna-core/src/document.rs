//! The open trace and everything every view over it must agree on: the
//! session, shared navigation, markers, translators and load bookkeeping.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::data::loaded_tracks::{LoadedGenerator, LoadedTrack};
use crate::data::transactions::{TrackKind, TrackRef};
use crate::data::{Hierarchy, SignalRef, Translators};
use crate::nav::Tween;
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};
use crate::wave::timeline::TimeBase;
use crate::wave::viewport::Viewport;

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
    pub viewport: Tween<Viewport>,
    pub cursor: Option<u64>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            viewport: Tween::new(Viewport::fit((0, 1000))),
            cursor: None,
        }
    }
}

/// A completed load the document accepted (its generation was current).
pub enum Delivered {
    Track,
    Signals(crate::session::SignalLoads),
    Opened(anyhow::Result<Arc<dyn Session>>),
}

/// Navigation behaviour mirrored from the resolved user settings, so the wave
/// model reads it beside the shared viewport it animates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Navigation {
    pub animation: crate::settings::Animation,
    /// Clicks snap the cursor to an edge within this many pixels.
    pub snap_px: f64,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            animation: crate::settings::Animation::On,
            snap_px: 6.0,
        }
    }
}

pub struct Document {
    state: TraceState,
    /// A newer open, explicit session replacement, or close invalidates old results.
    generation: u64,
    pub shared: Shared,
    pub navigation: Navigation,
    pub markers: Vec<Marker>,
    next_marker: u64,
    pub translators: Translators,
    pending: HashSet<SignalRef>,
    requests: Vec<LoadRequest>,
    tracks: HashMap<TrackRef, TrackLoad>,
    next_track_request: u64,
}

/// A selected track has one document-owned load, shared by its consumers.
pub struct TrackLoad {
    consumers: usize,
    request_id: u64,
    pub state: TrackLoadState,
}

pub enum TrackLoadState {
    Loading,
    Ready(LoadedTrack),
    Failed(String),
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
            navigation: Navigation::default(),
            markers: Vec::new(),
            next_marker: 1,
            translators: Translators::builtin(),
            pending: HashSet::new(),
            requests: Vec::new(),
            tracks: HashMap::new(),
            next_track_request: 1,
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

    /// How times are written: the timescale exponent and the producer's
    /// unit name when it declared one.
    pub fn time_base(&self) -> TimeBase<'_> {
        match self.session() {
            Some(s) => TimeBase::of(s.info()),
            None => TimeBase {
                timescale: -9,
                unit: None,
            },
        }
    }

    // -- opening ----------------------------------------------------------------

    /// Start opening a trace; the previous one stays until the new one arrives.
    pub fn open(&mut self, spec: OpenSpec) {
        self.generation += 1;
        self.tracks.clear();
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
        self.tracks.clear();
        self.pending.clear();
        self.requests
            .retain(|r| matches!(r, LoadRequest::Open { .. }));
        self.shared = Shared::default();
        self.markers.clear();
    }

    // -- signal loads --------------------------------------------------------------

    /// Retain a whole raw track for a panel or analysis. Pair with release_track.
    /// A generator already loaded through a stream shares its existing object.
    pub fn retain_track(&mut self, track: TrackRef) -> anyhow::Result<()> {
        if let Some(load) = self.tracks.get_mut(&track) {
            load.consumers = load
                .consumers
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("too many track consumers"))?;
            return Ok(());
        }
        let session = self
            .session()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no open trace"))?;
        anyhow::ensure!(
            session.capabilities().transactions,
            "transactions unsupported"
        );
        let catalog = session.tracks();
        let declaration = catalog
            .iter()
            .find(|t| t.id == track)
            .ok_or_else(|| anyhow::anyhow!("unknown transaction track {}", track.0))?;
        let ids: Vec<_> = match declaration.kind {
            TrackKind::Generator { .. } => vec![track],
            TrackKind::Stream { .. } => catalog
                .iter()
                .filter_map(|t| match t.kind {
                    TrackKind::Generator { stream } if stream == track => Some(t.id),
                    _ => None,
                })
                .collect(),
        };
        let resident: Option<Vec<_>> = ids.iter().map(|id| self.resident_generator(*id)).collect();
        let request_id = self
            .next_track_request_id()
            .ok_or_else(|| anyhow::anyhow!("track request identities exhausted"))?;
        let state = if let Some(generators) = resident {
            TrackLoadState::Ready(LoadedTrack { track, generators })
        } else {
            self.requests.push(LoadRequest::Track {
                generation: self.generation,
                request_id,
                session,
                track,
            });
            TrackLoadState::Loading
        };
        self.tracks.insert(
            track,
            TrackLoad {
                consumers: 1,
                request_id,
                state,
            },
        );
        Ok(())
    }

    /// A generator object already held by another selected track.
    pub fn resident_generator(&self, id: TrackRef) -> Option<Arc<LoadedGenerator>> {
        self.tracks.values().find_map(|load| match &load.state {
            TrackLoadState::Ready(data) => data
                .generators
                .iter()
                .find(|g| g.generator() == id)
                .cloned(),
            _ => None,
        })
    }

    fn next_track_request_id(&mut self) -> Option<u64> {
        let id = self.next_track_request;
        self.next_track_request = id.checked_add(1)?;
        Some(id)
    }

    pub fn track(&self, track: TrackRef) -> Option<&TrackLoadState> {
        self.tracks.get(&track).map(|load| &load.state)
    }

    pub fn release_track(&mut self, track: TrackRef) {
        let Some(load) = self.tracks.get_mut(&track) else {
            return;
        };
        load.consumers -= 1;
        if load.consumers == 0 {
            self.tracks.remove(&track);
            self.requests
                .retain(|r| !matches!(r, LoadRequest::Track { track: id, .. } if *id == track));
        }
    }

    pub fn retry_track(&mut self, track: TrackRef) -> bool {
        let Some(session) = self.session().cloned() else {
            return false;
        };
        let Some(load) = self.tracks.get_mut(&track) else {
            return false;
        };
        if !matches!(load.state, TrackLoadState::Failed(_)) {
            return false;
        }
        let Some(request_id) = self.next_track_request_id() else {
            return false;
        };
        let load = self.tracks.get_mut(&track).expect("checked above");
        load.request_id = request_id;
        load.state = TrackLoadState::Loading;
        self.requests.push(LoadRequest::Track {
            generation: self.generation,
            request_id,
            session,
            track,
        });
        true
    }

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

    /// Prune unstarted work using the same ownership rules in every executor.
    /// Stale queues must never clear pending demand for the new recording.
    pub(crate) fn retain_queued_signals(
        &mut self,
        generation: u64,
        signals: &mut Vec<SignalRef>,
        wanted: &HashSet<SignalRef>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        signals.retain(|signal| {
            if wanted.contains(signal) {
                true
            } else {
                self.pending.remove(signal);
                false
            }
        });
        !signals.is_empty()
    }

    pub(crate) fn wants_track_request(
        &self,
        generation: u64,
        request_id: u64,
        track: TrackRef,
    ) -> bool {
        generation == self.generation
            && self.tracks.get(&track).is_some_and(|load| {
                load.request_id == request_id && matches!(load.state, TrackLoadState::Loading)
            })
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
            LoadResult::Track {
                generation,
                request_id,
                track,
                result,
            } => {
                if !self.wants_track_request(generation, request_id, track) {
                    return None;
                }
                let state = match result {
                    Ok(mut loaded) if loaded.track == track => {
                        // Reuse objects retained by other selected tracks. The
                        // recording is immutable for this document generation.
                        for generator in &mut loaded.generators {
                            if let Some(resident) = self.resident_generator(generator.generator()) {
                                *generator = resident;
                            }
                        }
                        TrackLoadState::Ready(loaded)
                    }
                    Ok(_) => TrackLoadState::Failed("track identity mismatch".into()),
                    Err(error) => TrackLoadState::Failed(format!("{error:#}")),
                };
                self.tracks.get_mut(&track).expect("selected track").state = state;
                Some(Delivered::Track)
            }
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
