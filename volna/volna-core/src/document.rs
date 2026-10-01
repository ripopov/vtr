//! The open traces and everything every view over them must agree on: the
//! trace set, shared navigation, markers, translators and load bookkeeping.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::data::loaded_tracks::{LoadedGenerator, LoadedTrack};
use crate::data::transactions::{TrackKind, TrackRef, TransactionRef};
use crate::data::{Hierarchy, NumericKind, SignalRef, Translators};
use crate::marker::{self, Marker, MarkerId, Reference};
use crate::nav::Tween;
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};
use crate::trace::{Rescale, Retime, SlotState, TraceId, TraceSet, TraceSlot, Traced};
use crate::wave::analog::AnalogSummary;
use crate::wave::timeline::TimeBase;
use crate::wave::viewport::Viewport;

/// What the document shows as a whole: nothing, the first trace opening, a
/// failed open, or opened traces (the first of them named).
#[derive(Clone)]
pub enum TraceState {
    Empty,
    Loading { name: String },
    Loaded(Arc<dyn Session>),
    Error(String),
}

/// The record every panel agrees is selected: which generator of which
/// trace holds it, which record it is, and the panel that chose it. Panels
/// that cannot show that generator simply show no highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxSelection {
    pub track: Traced<TrackRef>,
    pub id: TransactionRef,
    pub origin: crate::panels::PanelId,
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
    Summary,
    /// A trace's scope sizes were counted.
    Sizes,
    Signals(TraceId, crate::session::SignalLoads),
    /// A trace finished opening, or failed to. `refined` is the factor by
    /// which the session unit became finer to admit it: every time held
    /// outside the document must be multiplied by it, and every other
    /// trace's histories load again in the new unit.
    Opened {
        trace: TraceId,
        result: Result<Arc<dyn Session>, String>,
        refined: Option<u64>,
    },
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

/// A change to the trace set as the undo journal swaps it
/// ([`crate::history::Edit::Traces`]).
#[derive(Clone)]
pub(crate) enum TraceEdit {
    /// Take the trace out of the set; the slot is kept so the inverse puts
    /// it back already open.
    Remove(TraceId),
    /// Put a removed trace back.
    Restore(Box<TraceSlot>),
    /// Name a trace; `None` goes back to the derived name.
    Rename(TraceId, Option<String>),
}

pub struct Document {
    traces: TraceSet,
    /// The document's identity: a new open or close ignores delayed input
    /// and results from before.
    epoch: u64,
    /// Hands out generations: every slot takes a fresh one when it is added
    /// and when its placement changes.
    generations: u64,
    pub shared: Shared,
    pub navigation: Navigation,
    /// Journaled cockpit state (`volna/volna/ARCHITECTURE.md`, "Undo and redo"); read through
    /// [`Document::markers`].
    markers: crate::history::Journaled<Vec<Marker>>,
    /// Where measurements start; navigation state, outside the journal.
    reference: Option<Reference>,
    /// The marker the reference was attached to when a marker edit since
    /// the last [`Document::take_edits`] removed it.
    detached: Option<MarkerId>,
    /// Trace set edits since the last [`Document::take_edits`], as inverses.
    trace_edits: Vec<(String, TraceEdit)>,
    pub translators: Translators,
    pending: HashSet<Traced<SignalRef>>,
    requests: Vec<LoadRequest>,
    tracks: HashMap<Traced<TrackRef>, TrackLoad>,
    next_track_request: u64,
    selection: Option<TxSelection>,
    /// Wave rows copied for pasting into any wave panel. They hold no
    /// histories or records; a paste shares resident data or loads it again.
    pub copied_rows: Vec<crate::wave::Entry>,
    /// Analog summaries of resident histories, per signal and reading.
    summaries: HashMap<(Traced<SignalRef>, NumericKind), SummaryLoad>,
    /// Activity summaries of folded groups, by their signals' identity.
    group_summaries: HashMap<Vec<usize>, crate::wave::group::SummaryLoad>,
    /// Whole-trace walks of stacked groups, by their layers' identity.
    stack_totals: HashMap<crate::wave::stack::Key, crate::wave::stack::TotalLoad>,
    /// Integral summaries of long stacked layers, by history identity and
    /// reading; the weak handle keeps the identity from being reused.
    integrals: HashMap<(usize, crate::wave::stack::Reading), Integral>,
    /// Every open trace's declared clocks; their stretches load when the
    /// trace opens.
    pub clocks: crate::clock::Clocks,
}

/// A long stacked layer's integral summary and the history it summarizes.
struct Integral {
    history: std::sync::Weak<dyn crate::data::SignalHistory>,
    load: crate::wave::stack::IntegralLoad,
}

/// The state of one analog summary; `history` is the identity of the
/// history it summarizes.
pub enum SummaryLoad {
    Building {
        history: usize,
    },
    Ready(Arc<AnalogSummary>),
    /// Refused (the memory budget); plots scan the history instead.
    Failed {
        history: usize,
    },
}

impl SummaryLoad {
    fn history(&self) -> usize {
        match self {
            Self::Building { history } | Self::Failed { history } => *history,
            Self::Ready(s) => s.identity(),
        }
    }
}

/// A selected track has one document-owned load, shared by its consumers.
pub struct TrackLoad {
    consumers: usize,
    request_id: u64,
    pub state: TrackLoadState,
}

/// Load state of a retained track, read without querying the session.
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
            traces: TraceSet::default(),
            epoch: 0,
            generations: 0,
            shared: Shared::default(),
            navigation: Navigation::default(),
            markers: Default::default(),
            reference: None,
            detached: None,
            trace_edits: Vec::new(),
            translators: Translators::builtin(),
            pending: HashSet::new(),
            requests: Vec::new(),
            tracks: HashMap::new(),
            next_track_request: 1,
            selection: None,
            copied_rows: Vec::new(),
            summaries: HashMap::new(),
            group_summaries: HashMap::new(),
            stack_totals: HashMap::new(),
            integrals: HashMap::new(),
            clocks: crate::clock::Clocks::default(),
        }
    }

    // -- selection -----------------------------------------------------------------

    /// The selected record, or none. Writers are the table and pipeline
    /// panels and the transaction panel's own jumps.
    pub fn selection(&self) -> Option<TxSelection> {
        self.selection
    }

    /// Select a record, or clear the selection. Returns whether it changed.
    pub fn select(&mut self, selection: Option<TxSelection>) -> bool {
        let changed = self.selection != selection;
        self.selection = selection;
        changed
    }

    // -- the trace set -----------------------------------------------------------------

    pub fn traces(&self) -> &TraceSet {
        &self.traces
    }

    /// What the document shows as a whole (see [`TraceState`]).
    pub fn state(&self) -> TraceState {
        if let Some((_, session)) = self.traces.first_loaded() {
            return TraceState::Loaded(Arc::clone(session));
        }
        let Some(first) = self.traces.iter().next() else {
            return TraceState::Empty;
        };
        match first.state() {
            SlotState::Error(e) => TraceState::Error(e.clone()),
            _ => TraceState::Loading {
                name: first.file_name().to_owned(),
            },
        }
    }

    /// Changes on every open and close: input and results from before are
    /// stale.
    pub fn generation(&self) -> u64 {
        self.epoch
    }

    fn next_generation(&mut self) -> u64 {
        self.generations += 1;
        self.generations
    }

    pub fn session(&self, trace: TraceId) -> Option<&Arc<dyn Session>> {
        self.traces.session(trace)
    }

    pub fn hierarchy(&self, trace: TraceId) -> Option<&Hierarchy> {
        self.session(trace).map(|s| s.hierarchy())
    }

    /// Whether some trace is open.
    pub fn is_loaded(&self) -> bool {
        self.traces.first_loaded().is_some()
    }

    /// The name of trace `trace` (see [`TraceSet::name`]).
    pub fn trace_name(&self, trace: TraceId) -> Option<String> {
        self.traces.name(trace)
    }

    /// The traces' names, as a window title shows them.
    pub fn name(&self) -> Option<String> {
        let names: Vec<String> = self
            .traces
            .ids()
            .filter_map(|id| self.traces.name(id))
            .collect();
        (!names.is_empty()).then(|| names.join(" + "))
    }

    /// The span of every open trace on the session timeline.
    pub fn limits(&self) -> (u64, u64) {
        self.traces.limits().unwrap_or((0, 1000))
    }

    pub fn timescale(&self) -> i8 {
        self.traces.unit().map_or(-9, |u| u.timescale)
    }

    /// How session times are written: the session unit's exponent and the
    /// producers' unit name when they declared one.
    pub fn time_base(&self) -> TimeBase<'_> {
        match self.traces.unit() {
            Some(unit) => TimeBase {
                timescale: unit.timescale,
                unit: unit.name.as_deref(),
            },
            None => TimeBase {
                timescale: -9,
                unit: None,
            },
        }
    }

    // -- opening ----------------------------------------------------------------

    /// Start opening a trace in place of every open one. The document shows
    /// it loading; its panels are replaced when it arrives.
    pub fn open(&mut self, spec: OpenSpec) {
        self.epoch = self.next_generation();
        let generation = self.next_generation();
        self.tracks.clear();
        self.traces = TraceSet::default();
        self.traces
            .insert(TraceSlot::new(TraceId::A, spec.source(), None, generation));
        self.requests.push(LoadRequest::Open {
            trace: TraceId::A,
            generation,
            spec,
        });
    }

    /// Start opening another trace beside the open ones, under the lowest
    /// free letter.
    pub fn add_trace(&mut self, spec: OpenSpec) -> anyhow::Result<TraceId> {
        let id = self
            .traces
            .free_id()
            .ok_or_else(|| anyhow::anyhow!("at most {} traces can be open", TraceId::MAX))?;
        self.add_trace_as(spec, id)
    }

    /// Start opening another trace under letter `id` (a workspace names
    /// its traces' letters).
    pub(crate) fn add_trace_as(&mut self, spec: OpenSpec, id: TraceId) -> anyhow::Result<TraceId> {
        anyhow::ensure!(self.traces.get(id).is_none(), "trace {id} is already open");
        let generation = self.next_generation();
        self.traces
            .insert(TraceSlot::new(id, spec.source(), None, generation));
        self.requests.push(LoadRequest::Open {
            trace: id,
            generation,
            spec,
        });
        Ok(id)
    }

    /// Record the durable identity of a trace (for workspaces and recents).
    pub(crate) fn set_uri(&mut self, trace: TraceId, uri: Option<String>) {
        if let Some(slot) = self.traces.get_mut(trace) {
            slot.uri = uri;
        }
    }

    /// Replace every trace with `session` immediately (tests, demos, hosts
    /// that already hold one).
    pub fn set_session(&mut self, session: Arc<dyn Session>) {
        self.epoch = self.next_generation();
        let generation = self.next_generation();
        self.traces = TraceSet::default();
        let source = session.info().name.clone();
        self.traces
            .insert(TraceSlot::new(TraceId::A, source, None, generation));
        self.install(TraceId::A, session)
            .expect("the first trace sets the unit");
    }

    /// Add an opened session as another trace immediately. Returns its
    /// letter and, when the session unit became finer to admit it, the
    /// factor (see [`Delivered::Opened`]).
    pub fn add_session(
        &mut self,
        session: Arc<dyn Session>,
    ) -> anyhow::Result<(TraceId, Option<u64>)> {
        let id = self
            .traces
            .free_id()
            .ok_or_else(|| anyhow::anyhow!("at most {} traces can be open", TraceId::MAX))?;
        let generation = self.next_generation();
        let source = session.info().name.clone();
        self.traces
            .insert(TraceSlot::new(id, source, None, generation));
        match self.install(id, session) {
            Ok(refined) => Ok((id, refined)),
            Err(error) => {
                self.traces.remove(id);
                Err(error)
            }
        }
    }

    /// Start every view afresh over the same traces (a restored
    /// workspace): loads in flight become stale, retained tracks and
    /// summaries go, and clocks and any unfinished scope counts load again.
    pub(crate) fn restart(&mut self) {
        self.epoch = self.next_generation();
        // A trace still opening keeps its generation: its open completes.
        let loaded: Vec<TraceId> = self.traces.loaded().map(|(id, _)| id).collect();
        for id in loaded {
            let generation = self.next_generation();
            if let Some(slot) = self.traces.get_mut(id) {
                slot.generation = generation;
            }
        }
        self.reset_state();
        self.shared.viewport.set(Viewport::fit(self.limits()));
        let loaded: Vec<(TraceId, Arc<dyn Session>)> = self
            .traces
            .loaded()
            .map(|(id, s)| (id, Arc::clone(s)))
            .collect();
        for (id, session) in loaded {
            self.load_scope_sizes(id, &session);
            self.clocks.add_trace(id, session.tracks());
            self.retain_clocks(id);
        }
    }

    /// Install a saved trace name; not an edit.
    pub(crate) fn restore_trace_name(&mut self, trace: TraceId, name: Option<String>) {
        if let Some(slot) = self.traces.get_mut(trace) {
            slot.rename = name;
        }
    }

    pub fn close(&mut self) {
        self.epoch = self.next_generation();
        self.traces = TraceSet::default();
        self.reset_state();
    }

    /// Forget everything that belonged to the previous traces.
    fn reset_state(&mut self) {
        self.tracks.clear();
        self.selection = None;
        self.pending.clear();
        self.requests
            .retain(|r| matches!(r, LoadRequest::Open { .. }));
        self.shared = Shared::default();
        self.markers.restore(Vec::new());
        self.reference = None;
        self.detached = None;
        self.trace_edits.clear();
        self.copied_rows.clear();
        self.summaries.clear();
        self.group_summaries.clear();
        self.stack_totals.clear();
        self.integrals.clear();
        self.clocks = crate::clock::Clocks::default();
    }

    /// Make an opened session trace `id`'s: place it on the session
    /// timeline and start loading its clocks. The first trace of a document
    /// starts everything shared afresh. Refused when its unit cannot share
    /// the timeline (see [`TraceSet::admit`]).
    fn install(&mut self, id: TraceId, session: Arc<dyn Session>) -> anyhow::Result<Option<u64>> {
        let first = !self.is_loaded();
        if first {
            self.reset_state();
            self.traces.forget_unit();
        }
        let (unit, placement, factor) = self.traces.admit(session.info())?;
        let refined = (factor > 1).then(|| self.refine(factor));
        self.traces.set_unit(unit);
        let slot = self
            .traces
            .get_mut(id)
            .expect("installed traces have slots");
        slot.placement = placement;
        slot.state = SlotState::Loaded(Arc::clone(&session));
        self.load_scope_sizes(id, &session);
        if first {
            self.shared.viewport.set(Viewport::fit(self.limits()));
        }
        // Every clock is loaded with its trace: rulers, readouts and
        // pipelines need them.
        self.clocks.add_trace(id, session.tracks());
        self.retain_clocks(id);
        Ok(refined)
    }

    /// Completed counts survive a workspace restore; unfinished counts need
    /// a request under the trace's current generation.
    fn load_scope_sizes(&mut self, trace: TraceId, session: &Arc<dyn Session>) {
        let slot = self
            .traces
            .get_mut(trace)
            .expect("loaded traces have slots");
        if slot.sizes.is_none() {
            slot.sizes = session.scope_sizes();
        }
        if slot.sizes.is_none() {
            self.requests.push(LoadRequest::Sizes {
                trace,
                generation: slot.generation,
                session: Arc::clone(session),
            });
        }
    }

    fn retain_clocks(&mut self, trace: TraceId) {
        let streams: Vec<_> = self
            .clocks
            .iter()
            .filter(|c| c.trace == trace)
            .map(|c| c.stream())
            .collect();
        for stream in streams {
            if let Err(error) = self.retain_track(stream) {
                self.clocks.deliver(stream, Err(&format!("{error:#}")));
            }
        }
    }

    /// The session unit becomes `factor` times finer: every trace open so
    /// far is placed again (its loads start over under a new generation)
    /// and every shared time is rewritten. Returns `factor`.
    fn refine(&mut self, factor: u64) -> u64 {
        let loaded: Vec<TraceId> = self.traces.loaded().map(|(id, _)| id).collect();
        for id in loaded {
            let generation = self.next_generation();
            let slot = self.traces.get_mut(id).expect("loaded");
            slot.generation = generation;
            slot.placement = slot.placement.refined(factor);
            self.clocks.reload(id);
            self.reload_tracks(id);
        }
        // Histories of the old placement leave with their rows, which the
        // app attaches again.
        let stale: HashSet<TraceId> = self.traces.ids().collect();
        self.pending.retain(|s| !stale.contains(&s.trace));
        self.requests
            .retain(|r| !matches!(r, LoadRequest::Signals { .. }));
        self.summaries.clear();
        self.group_summaries.clear();
        self.stack_totals.clear();
        self.integrals.clear();
        self.retime(Rescale::Finer(factor));
        factor
    }

    /// Rewrite the shared times: the viewport, the cursor, the markers (as
    /// state, not an edit; the journal is rewritten with them) and a
    /// reference at a time.
    pub(crate) fn retime(&mut self, by: Rescale) {
        self.shared.viewport.retime(by);
        self.shared.cursor.retime(by);
        let mut markers = self.markers.to_vec();
        for m in &mut markers {
            m.time.retime(by);
        }
        self.markers.restore(markers);
        if let Some(Reference::Time(t)) = &mut self.reference {
            t.retime(by);
        }
    }

    /// Load every retained track of `trace` again under its new generation.
    fn reload_tracks(&mut self, trace: TraceId) {
        let Some(session) = self.session(trace).cloned() else {
            return;
        };
        let generation = self.traces.get(trace).map_or(0, |s| s.generation);
        let tracks: Vec<Traced<TrackRef>> = self
            .tracks
            .keys()
            .filter(|t| t.trace == trace)
            .copied()
            .collect();
        for track in tracks {
            let Some(request_id) = self.next_track_request_id() else {
                continue;
            };
            let load = self.tracks.get_mut(&track).expect("listed above");
            load.request_id = request_id;
            load.state = TrackLoadState::Loading;
            self.requests.push(LoadRequest::Track {
                trace,
                generation,
                request_id,
                session: Arc::clone(&session),
                track: track.item,
            });
        }
    }

    /// Take a trace out of the set with everything the document holds of
    /// it. Its slot, still open, is returned for [`Document::restore_trace`].
    /// Panels and rows that name it are the app's to remove first.
    pub(crate) fn remove_trace(&mut self, trace: TraceId) -> Option<TraceSlot> {
        let slot = self.traces.remove(trace)?;
        self.forget_trace(trace);
        Some(slot)
    }

    fn forget_trace(&mut self, trace: TraceId) {
        self.tracks.retain(|t, _| t.trace != trace);
        self.pending.retain(|s| s.trace != trace);
        self.requests.retain(|r| match r {
            LoadRequest::Open { trace: t, .. }
            | LoadRequest::BuildActivity { trace: t, .. }
            | LoadRequest::Sizes { trace: t, .. }
            | LoadRequest::ActivityCounter { trace: t, .. }
            | LoadRequest::Activity { trace: t, .. }
            | LoadRequest::ResolveActivity { trace: t, .. }
            | LoadRequest::Signals { trace: t, .. }
            | LoadRequest::Track { trace: t, .. } => *t != trace,
            LoadRequest::Summary { signal, .. } => signal.trace != trace,
            LoadRequest::GroupSummary { .. }
            | LoadRequest::Integral { .. }
            | LoadRequest::StackTotal { .. } => true,
        });
        self.summaries.retain(|(s, _), _| s.trace != trace);
        if self.selection.is_some_and(|s| s.track.trace == trace) {
            self.selection = None;
        }
        self.copied_rows
            .retain(|e| e.row.trace().is_none_or(|t| t != trace));
        self.clocks.remove_trace(trace);
    }

    /// Put a removed trace back under its letter, already open. It takes a
    /// new generation and reloads its clocks; rows that name it load again
    /// as the app attaches them. Returns the factor by which the session
    /// unit refined to admit it, as [`Delivered::Opened`] does.
    pub(crate) fn restore_trace(&mut self, mut slot: TraceSlot) -> anyhow::Result<Option<u64>> {
        anyhow::ensure!(
            self.traces.get(slot.id).is_none(),
            "trace {} is already open",
            slot.id
        );
        slot.generation = self.next_generation();
        let id = slot.id;
        let session = slot.session().cloned();
        slot.state = SlotState::Loading;
        self.traces.insert(slot);
        let session = session.ok_or_else(|| anyhow::anyhow!("trace {id} was not open"))?;
        self.install(id, session).inspect_err(|_| {
            self.traces.remove(id);
        })
    }

    /// Name a trace, or go back to its derived name. Returns whether its
    /// name changed; the change is an undoable edit.
    pub fn rename_trace(&mut self, trace: TraceId, name: Option<&str>) -> bool {
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned);
        let Some(slot) = self.traces.get_mut(trace) else {
            return false;
        };
        if slot.rename == name {
            return false;
        }
        let before = std::mem::replace(&mut slot.rename, name);
        self.trace_edits.push((
            format!("Rename trace {trace}"),
            TraceEdit::Rename(trace, before),
        ));
        true
    }

    /// Note that trace `trace` joined or left the set as an edit, with the
    /// inverse the journal keeps.
    pub(crate) fn note_trace_edit(&mut self, label: String, inverse: TraceEdit) {
        self.trace_edits.push((label, inverse));
    }

    /// Install a trace's name while undoing or redoing; returns the name
    /// it replaces.
    pub(crate) fn swap_trace_name(
        &mut self,
        trace: TraceId,
        name: Option<String>,
    ) -> anyhow::Result<Option<String>> {
        let slot = self
            .traces
            .get_mut(trace)
            .ok_or_else(|| anyhow::anyhow!("trace {trace} is not open"))?;
        Ok(std::mem::replace(&mut slot.rename, name))
    }

    // -- analog summaries ----------------------------------------------------------

    /// The summary of `signal` read as `kind`, or whether one is building.
    pub fn analog_summary(
        &self,
        signal: Traced<SignalRef>,
        kind: NumericKind,
    ) -> Option<&SummaryLoad> {
        self.summaries.get(&(signal, kind))
    }

    /// Hold summaries for exactly the `wanted` histories: queue builds for
    /// new ones, and release those no plot shows (or whose history was
    /// replaced) together with their memory.
    pub(crate) fn sync_summaries(
        &mut self,
        wanted: HashMap<(Traced<SignalRef>, NumericKind), Arc<dyn crate::data::SignalHistory>>,
        budget: &crate::remote::memory::MemoryBudget,
    ) {
        self.summaries.retain(|key, load| {
            wanted
                .get(key)
                .is_some_and(|h| crate::wave::analog::history_identity(h) == load.history())
        });
        for ((signal, kind), history) in wanted {
            if self.summaries.contains_key(&(signal, kind)) {
                continue;
            }
            let Some(generation) = self.traces.get(signal.trace).map(|s| s.generation) else {
                continue;
            };
            self.summaries.insert(
                (signal, kind),
                SummaryLoad::Building {
                    history: crate::wave::analog::history_identity(&history),
                },
            );
            self.requests.push(LoadRequest::Summary {
                generation,
                signal,
                history,
                kind,
                budget: budget.clone(),
            });
        }
    }

    /// The activity summary of the folded group whose signals are `key`.
    pub fn group_summary(&self, key: &[usize]) -> Option<&crate::wave::group::SummaryLoad> {
        self.group_summaries.get(key)
    }

    /// Hold summaries for exactly the `wanted` folded groups (by the key of
    /// their signals): queue builds for new ones and release the others.
    pub(crate) fn sync_group_summaries(
        &mut self,
        wanted: HashMap<Vec<usize>, Vec<Arc<dyn crate::data::SignalHistory>>>,
        budget: &crate::remote::memory::MemoryBudget,
    ) {
        self.group_summaries
            .retain(|key, _| wanted.contains_key(key));
        for (key, members) in wanted {
            if self.group_summaries.contains_key(&key) {
                continue;
            }
            self.group_summaries
                .insert(key, crate::wave::group::SummaryLoad::Building);
            self.requests.push(LoadRequest::GroupSummary {
                generation: self.epoch,
                members,
                range: self.limits(),
                budget: budget.clone(),
            });
        }
    }

    /// The whole-trace walk of the stacked layers whose identity is `key`.
    pub fn stack_total(
        &self,
        key: &crate::wave::stack::Key,
    ) -> Option<&crate::wave::stack::TotalLoad> {
        self.stack_totals.get(key)
    }

    /// Hold walks for exactly the `wanted` stacks (by the key of their
    /// layers): short ones are walked at once, long ones queued for the load
    /// worker, and the others are released.
    pub(crate) fn sync_stack_totals(
        &mut self,
        wanted: HashMap<crate::wave::stack::Key, Vec<crate::wave::stack::Layer>>,
        budget: &crate::remote::memory::MemoryBudget,
    ) {
        use crate::wave::stack::{self, TotalLoad, TotalSummary};
        self.stack_totals.retain(|key, _| wanted.contains_key(key));
        for (key, layers) in wanted {
            if self.stack_totals.contains_key(&key) {
                continue;
            }
            if stack::changes(&layers) <= stack::WALK_MAX {
                let load = match TotalSummary::build(&layers, self.limits()).account(budget) {
                    Ok(summary) => TotalLoad::Ready(Arc::new(summary)),
                    Err(_) => TotalLoad::Failed,
                };
                self.stack_totals.insert(key, load);
                continue;
            }
            self.stack_totals.insert(key, TotalLoad::Building);
            self.requests.push(LoadRequest::StackTotal {
                generation: self.epoch,
                layers,
                range: self.limits(),
                budget: budget.clone(),
            });
        }
    }

    /// The integral summary of `history` read as `reading`, or whether one
    /// is building.
    pub fn integral(
        &self,
        history: &Arc<dyn crate::data::SignalHistory>,
        reading: crate::wave::stack::Reading,
    ) -> Option<&crate::wave::stack::IntegralLoad> {
        let key = (crate::wave::analog::history_identity(history), reading);
        self.integrals.get(&key).map(|i| &i.load)
    }

    /// Give `layers` the integral summaries the document holds for them.
    pub fn summarize_layers(&self, layers: &mut [crate::wave::stack::Layer]) {
        use crate::wave::stack::IntegralLoad;
        for layer in layers {
            match self.integral(&layer.history, layer.reading) {
                Some(IntegralLoad::Ready(s)) => {
                    layer.summary =
                        Some(s.clone()).filter(|s| s.matches(&layer.history, layer.reading));
                }
                Some(IntegralLoad::Building { .. }) => layer.building = true,
                _ => {}
            }
        }
    }

    /// Hold integral summaries for exactly the `wanted` histories: queue
    /// builds for new ones and release the others, and those whose history
    /// is gone, with their memory.
    pub(crate) fn sync_integrals(
        &mut self,
        wanted: HashMap<(usize, crate::wave::stack::Reading), Arc<dyn crate::data::SignalHistory>>,
        budget: &crate::remote::memory::MemoryBudget,
    ) {
        use crate::wave::stack::IntegralLoad;
        self.integrals
            .retain(|key, i| wanted.contains_key(key) && i.history.strong_count() > 0);
        for ((identity, reading), history) in wanted {
            if self.integrals.contains_key(&(identity, reading)) {
                continue;
            }
            self.integrals.insert(
                (identity, reading),
                Integral {
                    history: Arc::downgrade(&history),
                    load: IntegralLoad::Building { history: identity },
                },
            );
            self.requests.push(LoadRequest::Integral {
                generation: self.epoch,
                history,
                reading,
                budget: budget.clone(),
            });
        }
    }

    // -- signal loads --------------------------------------------------------------

    /// Retain a whole raw track for a panel or analysis. Pair with release_track.
    /// A generator already loaded through a stream shares its existing object.
    pub fn retain_track(&mut self, track: Traced<TrackRef>) -> anyhow::Result<()> {
        if let Some(load) = self.tracks.get_mut(&track) {
            load.consumers = load
                .consumers
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("too many track consumers"))?;
            return Ok(());
        }
        let slot = self
            .traces
            .get(track.trace)
            .ok_or_else(|| anyhow::anyhow!("trace {} is not open", track.trace))?;
        let generation = slot.generation;
        let session = slot
            .session()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("trace {} is not open", track.trace))?;
        anyhow::ensure!(
            session.capabilities().transactions,
            "transactions unsupported"
        );
        let catalog = session.tracks();
        let declaration = catalog
            .iter()
            .find(|t| t.id == track.item)
            .ok_or_else(|| anyhow::anyhow!("unknown transaction track {}", track.item.0))?;
        let ids: Vec<_> = match declaration.kind {
            TrackKind::Generator { .. } => vec![track.item],
            TrackKind::Stream { .. } => catalog
                .iter()
                .filter_map(|t| match t.kind {
                    TrackKind::Generator { stream } if stream == track.item => Some(t.id),
                    _ => None,
                })
                .collect(),
        };
        let resident: Option<Vec<_>> = ids
            .iter()
            .map(|id| self.resident_generator(track.with(*id)))
            .collect();
        let request_id = self
            .next_track_request_id()
            .ok_or_else(|| anyhow::anyhow!("track request identities exhausted"))?;
        let state = if let Some(generators) = resident {
            TrackLoadState::Ready(LoadedTrack {
                track: track.item,
                generators,
            })
        } else {
            self.requests.push(LoadRequest::Track {
                trace: track.trace,
                generation,
                request_id,
                session,
                track: track.item,
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

    /// Every generator object of `trace` currently held by a selected
    /// track, once per object: the same generator reached through a stream
    /// and on its own is one shared object.
    pub fn resident_generators(
        &self,
        trace: TraceId,
    ) -> impl Iterator<Item = &Arc<LoadedGenerator>> {
        let mut seen = std::collections::HashSet::new();
        self.tracks
            .iter()
            .filter(move |(track, _)| track.trace == trace)
            .filter_map(|(_, load)| match &load.state {
                TrackLoadState::Ready(data) => Some(data.generators.iter()),
                _ => None,
            })
            .flatten()
            .filter(move |g| seen.insert(g.generator()))
    }

    /// A generator object already held by another selected track.
    pub fn resident_generator(&self, id: Traced<TrackRef>) -> Option<Arc<LoadedGenerator>> {
        self.tracks
            .iter()
            .filter(|(track, _)| track.trace == id.trace)
            .find_map(|(_, load)| match &load.state {
                TrackLoadState::Ready(data) => data
                    .generators
                    .iter()
                    .find(|g| g.generator() == id.item)
                    .cloned(),
                _ => None,
            })
    }

    fn next_track_request_id(&mut self) -> Option<u64> {
        let id = self.next_track_request;
        self.next_track_request = id.checked_add(1)?;
        Some(id)
    }

    /// Load state of a retained track; `None` when no consumer retains it.
    pub fn track(&self, track: Traced<TrackRef>) -> Option<&TrackLoadState> {
        self.tracks.get(&track).map(|load| &load.state)
    }

    /// Drop one consumer; the last one forgets the track and its queued load.
    pub fn release_track(&mut self, track: Traced<TrackRef>) {
        let Some(load) = self.tracks.get_mut(&track) else {
            return;
        };
        load.consumers -= 1;
        if load.consumers == 0 {
            self.tracks.remove(&track);
            self.requests.retain(|r| {
                !matches!(r, LoadRequest::Track { trace, track: id, .. }
                    if *trace == track.trace && *id == track.item)
            });
        }
    }

    /// Re-request a failed track under a new request identity, so a late
    /// completion of the superseded load is ignored. Returns whether a
    /// request was queued.
    pub fn retry_track(&mut self, track: Traced<TrackRef>) -> bool {
        let Some(slot) = self.traces.get(track.trace) else {
            return false;
        };
        let generation = slot.generation;
        let Some(session) = slot.session().cloned() else {
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
            trace: track.trace,
            generation,
            request_id,
            session,
            track: track.item,
        });
        true
    }

    /// Queue a history load unless one is already pending. Returns whether a
    /// request was queued.
    pub fn request_signal(&mut self, signal: Traced<SignalRef>) -> bool {
        let Some(slot) = self.traces.get(signal.trace) else {
            return false;
        };
        let generation = slot.generation;
        let Some(session) = slot.session().cloned() else {
            return false;
        };
        if !self.pending.insert(signal) {
            return false;
        }
        if let Some(LoadRequest::Signals {
            trace,
            signals,
            generation: g,
            ..
        }) = self.requests.last_mut()
            && (*trace, *g) == (signal.trace, generation)
        {
            signals.push(signal.item);
        } else {
            self.requests.push(LoadRequest::Signals {
                trace: signal.trace,
                generation,
                session,
                signals: vec![signal.item],
            });
        }
        true
    }

    pub fn is_pending(&self, signal: Traced<SignalRef>) -> bool {
        self.pending.contains(&signal)
    }

    /// Whether a load requested under `generation` for `trace` is current.
    fn current(&self, trace: TraceId, generation: u64) -> bool {
        self.traces
            .get(trace)
            .is_some_and(|slot| slot.generation == generation)
    }

    /// Prune unstarted work using the same ownership rules in every executor.
    /// Stale queues must never clear pending demand for the new recording.
    pub(crate) fn retain_queued_signals(
        &mut self,
        trace: TraceId,
        generation: u64,
        signals: &mut Vec<SignalRef>,
        wanted: &HashSet<Traced<SignalRef>>,
    ) -> bool {
        if !self.current(trace, generation) {
            return false;
        }
        signals.retain(|&signal| {
            let signal = Traced::new(trace, signal);
            if wanted.contains(&signal) {
                true
            } else {
                self.pending.remove(&signal);
                false
            }
        });
        !signals.is_empty()
    }

    pub(crate) fn wants_track_request(
        &self,
        trace: TraceId,
        generation: u64,
        request_id: u64,
        track: TrackRef,
    ) -> bool {
        self.current(trace, generation)
            && self
                .tracks
                .get(&Traced::new(trace, track))
                .is_some_and(|load| {
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

    /// Accept a completed load if its generation is still current, placing
    /// what it brought on the session timeline.
    pub fn deliver(&mut self, result: LoadResult) -> Option<Delivered> {
        match result {
            // The sidebar's activity model owns these (`App::deliver`).
            LoadResult::ActivityBuilt { .. }
            | LoadResult::ActivityCounter { .. }
            | LoadResult::Activity { .. }
            | LoadResult::ActivityResolved { .. } => None,
            LoadResult::Track {
                trace,
                generation,
                request_id,
                track,
                result,
            } => {
                if !self.wants_track_request(trace, generation, request_id, track) {
                    return None;
                }
                let placement = self.traces.get(trace)?.placement();
                let state = match result.and_then(|mut loaded| {
                    placement.place_track(&mut loaded)?;
                    Ok(loaded)
                }) {
                    Ok(mut loaded) if loaded.track == track => {
                        // Reuse objects retained by other selected tracks. The
                        // recording is immutable for this generation.
                        for generator in &mut loaded.generators {
                            if let Some(resident) =
                                self.resident_generator(Traced::new(trace, generator.generator()))
                            {
                                *generator = resident;
                            }
                        }
                        TrackLoadState::Ready(loaded)
                    }
                    Ok(_) => TrackLoadState::Failed("track identity mismatch".into()),
                    Err(error) => TrackLoadState::Failed(format!("{error:#}")),
                };
                let track = Traced::new(trace, track);
                if self.clocks.is_clock_track(track) {
                    self.clocks.deliver(
                        track,
                        match &state {
                            TrackLoadState::Ready(t) => Ok(&t.generators),
                            TrackLoadState::Failed(e) => Err(e),
                            TrackLoadState::Loading => Err("not loaded"),
                        },
                    );
                }
                self.tracks.get_mut(&track).expect("selected track").state = state;
                Some(Delivered::Track)
            }
            LoadResult::Signals {
                trace,
                generation,
                results,
            } => {
                if !self.current(trace, generation) {
                    return None;
                }
                let placement = self.traces.get(trace)?.placement();
                for (signal, _) in &results {
                    self.pending.remove(&Traced::new(trace, *signal));
                }
                let results = if placement.is_identity() {
                    results
                } else {
                    results
                        .into_iter()
                        .map(|(signal, result)| {
                            (signal, result.map(|h| placement.place_history(h)))
                        })
                        .collect()
                };
                Some(Delivered::Signals(trace, results))
            }
            LoadResult::Sizes {
                trace,
                generation,
                result,
            } => {
                if !self.current(trace, generation) {
                    return None;
                }
                // The count cannot fail locally; a lost one leaves the column empty.
                self.traces.get_mut(trace)?.sizes = Some(result.ok()?);
                Some(Delivered::Sizes)
            }
            LoadResult::Summary {
                generation,
                signal,
                kind,
                history,
                result,
            } => {
                if !self.current(signal.trace, generation) {
                    return None;
                }
                let load = self.summaries.get_mut(&(signal, kind))?;
                if !matches!(load, SummaryLoad::Building { history: h } if *h == history) {
                    return None;
                }
                *load = match result {
                    Ok(summary) => SummaryLoad::Ready(summary),
                    Err(_) => SummaryLoad::Failed { history },
                };
                Some(Delivered::Summary)
            }
            LoadResult::Integral {
                generation,
                history,
                reading,
                result,
            } => {
                use crate::wave::stack::IntegralLoad;
                let entry = self.integrals.get_mut(&(history, reading))?;
                if generation != self.epoch
                    || !matches!(entry.load, IntegralLoad::Building { history: h } if h == history)
                {
                    return None;
                }
                entry.load = match result {
                    Ok(summary) => IntegralLoad::Ready(summary),
                    Err(_) => IntegralLoad::Failed { history },
                };
                Some(Delivered::Summary)
            }
            LoadResult::StackTotal {
                generation,
                key,
                result,
            } => {
                use crate::wave::stack::TotalLoad;
                let load = self.stack_totals.get_mut(&key)?;
                if generation != self.epoch || !matches!(load, TotalLoad::Building) {
                    return None;
                }
                *load = match result {
                    Ok(summary) => TotalLoad::Ready(summary),
                    Err(_) => TotalLoad::Failed,
                };
                Some(Delivered::Summary)
            }
            LoadResult::GroupSummary {
                generation,
                key,
                result,
            } => {
                use crate::wave::group::SummaryLoad;
                let load = self.group_summaries.get_mut(&key)?;
                if generation != self.epoch || !matches!(load, SummaryLoad::Building) {
                    return None;
                }
                *load = match result {
                    Ok(summary) => SummaryLoad::Ready(summary),
                    Err(_) => SummaryLoad::Failed,
                };
                Some(Delivered::Summary)
            }
            LoadResult::Opened {
                trace,
                generation,
                result,
            } => {
                if !self.current(trace, generation)
                    || !matches!(self.traces.get(trace)?.state(), SlotState::Loading)
                {
                    return None;
                }
                let installed = result
                    .and_then(|session| Ok((self.install(trace, Arc::clone(&session))?, session)));
                match installed {
                    Ok((refined, session)) => Some(Delivered::Opened {
                        trace,
                        result: Ok(session),
                        refined,
                    }),
                    Err(error) => {
                        let error = format!("{error:#}");
                        if self.is_loaded() {
                            // A trace that cannot join leaves the set; the app
                            // says why.
                            self.traces.remove(trace);
                        } else {
                            self.traces.get_mut(trace)?.state = SlotState::Error(error.clone());
                        }
                        Some(Delivered::Opened {
                            trace,
                            result: Err(error),
                            refined: None,
                        })
                    }
                }
            }
        }
    }

    // -- cursor and markers ----------------------------------------------------------

    /// Install already validated markers, in time order whatever order the
    /// workspace listed them in.
    pub(crate) fn restore_markers(&mut self, mut markers: Vec<Marker>) {
        markers.sort_by_key(|m| m.time);
        self.markers.restore(markers);
    }

    /// Where measurements start, if anywhere.
    pub fn reference(&self) -> Option<Reference> {
        self.reference
    }

    /// The reference's time: its own, or its marker's.
    pub fn reference_time(&self) -> Option<u64> {
        match self.reference? {
            Reference::Time(t) => Some(t),
            Reference::Marker(id) => self.markers.iter().find(|m| m.id == id).map(|m| m.time),
        }
    }

    /// Set or clear the reference. Not an undoable edit: the reference is
    /// navigation state like the cursor. A marker reference must name an
    /// existing marker. Returns whether it changed.
    pub fn set_reference(&mut self, reference: Option<Reference>) -> bool {
        if let Some(Reference::Marker(id)) = reference
            && !self.markers.iter().any(|m| m.id == id)
        {
            return false;
        }
        let changed = self.reference != reference;
        self.reference = reference;
        changed
    }

    /// Keep the reference where its marker was when an edit removed that
    /// marker; `before` is the list before the edit. Returns the marker it
    /// was attached to, if it detached.
    fn detach_reference(&mut self, before: &[Marker]) -> Option<MarkerId> {
        let Some(Reference::Marker(id)) = self.reference else {
            return None;
        };
        if self.markers.iter().any(|m| m.id == id) {
            return None;
        }
        self.reference = before
            .iter()
            .find(|m| m.id == id)
            .map(|m| Reference::Time(m.time));
        Some(id)
    }

    /// Reattach the reference to marker `id` if it still sits at that
    /// marker's time, as its detachment left it; a reference moved since
    /// stays where the user put it.
    fn reattach_reference(&mut self, id: MarkerId) {
        if let (Some(Reference::Time(t)), Some(m)) =
            (self.reference, self.markers.iter().find(|m| m.id == id))
            && m.time == t
        {
            self.reference = Some(Reference::Marker(id));
        }
    }

    /// The markers, in time order: the Markers lane finds the visible ones
    /// by binary search.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// Mark `time` with the lowest free number; `None` when a marker is
    /// already there.
    pub fn add_marker(&mut self, time: u64) -> Option<MarkerId> {
        if marker::at(&self.markers, time).is_some() {
            return None;
        }
        let id = marker::free_id(&self.markers)?;
        let ix = self.markers.partition_point(|m| m.time < time);
        self.markers.update(|markers| {
            markers.insert(
                ix,
                Marker {
                    id,
                    time,
                    label: None,
                },
            )
        });
        Some(id)
    }

    /// Move marker `id` to `time`, keeping the list in time order; a
    /// reference on the marker follows it. Refused when another marker is
    /// already there. Returns whether it moved.
    pub fn move_marker(&mut self, id: MarkerId, time: u64) -> bool {
        let Some(ix) = self.markers.iter().position(|m| m.id == id) else {
            return false;
        };
        if self.markers[ix].time == time || marker::at(&self.markers, time).is_some() {
            return false;
        }
        self.markers.update(|markers| {
            let mut m = markers.remove(ix);
            m.time = time;
            let at = markers.partition_point(|n| n.time < time);
            markers.insert(at, m);
        })
    }

    /// Remove marker `id`; a reference on it stays at its time.
    pub fn remove_marker(&mut self, id: MarkerId) -> bool {
        let Some(ix) = self.markers.iter().position(|m| m.id == id) else {
            return false;
        };
        let removed = [self.markers[ix].clone()];
        let changed = self.markers.update(|markers| _ = markers.remove(ix));
        self.detached = self.detach_reference(&removed).or(self.detached);
        changed
    }

    pub fn remove_all_markers(&mut self) -> bool {
        let before = self.markers.to_vec();
        let changed = self.markers.set(Vec::new());
        self.detached = self.detach_reference(&before).or(self.detached);
        changed
    }

    /// Name marker `id` (see [`marker::clean_name`]); an empty name clears
    /// it. Returns whether the name changed.
    pub fn rename_marker(&mut self, id: MarkerId, name: &str) -> bool {
        let label = marker::clean_name(name);
        match self.markers.iter().position(|m| m.id == id) {
            Some(ix) => self.markers.update(|markers| markers[ix].label = label),
            None => false,
        }
    }

    /// Hand the markers before the edits since the last call to the undo
    /// journal, unless they cancelled out.
    pub(crate) fn take_edits(&mut self, history: &mut crate::history::History) {
        self.take_marker_edits(history);
        for (label, edit) in std::mem::take(&mut self.trace_edits) {
            history.record(crate::history::Edit::Traces(edit), Some(label));
        }
    }

    fn take_marker_edits(&mut self, history: &mut crate::history::History) {
        let attached = self.detached.take();
        let Some(before) = self.markers.take_before() else {
            return;
        };
        // The one marker whose name alone differs between `a` and `b`.
        let renamed = |a: &[Marker], b: &[Marker]| {
            let mut diff = a.iter().zip(b).filter(|(x, y)| x != y);
            match (diff.next(), diff.next()) {
                (Some((x, y)), None) if (x.id, x.time) == (y.id, y.time) => Some(x.id),
                _ => None,
            }
        };
        // The one marker in `a` that `b` lacks.
        let only = |a: &[Marker], b: &[Marker]| {
            a.iter()
                .find(|m| !b.iter().any(|n| n.id == m.id))
                .map(|m| m.id)
        };
        // The one marker whose time alone differs between `a` and `b`.
        let moved = |a: &[Marker], b: &[Marker]| {
            let mut diff = a.iter().filter(|x| !b.contains(x));
            match (diff.next(), diff.next()) {
                (Some(x), None) => b
                    .iter()
                    .find(|y| y.id == x.id && y.label == x.label && y.time != x.time)
                    .map(|_| x.id),
                _ => None,
            }
        };
        let label = match (before.len(), self.markers.len()) {
            (b, a) if a == b + 1 => only(&self.markers, &before)
                .map_or_else(|| "Add marker".into(), |id| format!("Add marker {id}")),
            (b, a) if a + 1 == b => only(&before, &self.markers).map_or_else(
                || "Remove marker".into(),
                |id| format!("Remove marker {id}"),
            ),
            (_, 0) => "Remove all markers".into(),
            (b, a) if a == b => match (
                renamed(&before, &self.markers),
                moved(&before, &self.markers),
            ) {
                (Some(id), _) => format!("Rename marker {id}"),
                (None, Some(id)) => format!("Move marker {id}"),
                (None, None) => "Change markers".into(),
            },
            _ => "Change markers".into(),
        };
        history.record(
            crate::history::Edit::Markers {
                markers: before,
                attached,
            },
            Some(label),
        );
    }

    /// Install markers while undoing or redoing, reattaching the reference
    /// to marker `attach` when it came back where the reference still is.
    /// Returns the replaced markers and the marker this swap detached the
    /// reference from, which the inverse reattaches.
    pub(crate) fn swap_markers(
        &mut self,
        markers: Vec<Marker>,
        attach: Option<MarkerId>,
    ) -> (Vec<Marker>, Option<MarkerId>) {
        let before = self.markers.swap(markers);
        if let Some(id) = attach {
            self.reattach_reference(id);
        }
        let detached = self.detach_reference(&before);
        (before, detached)
    }
}
