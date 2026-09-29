//! The trace set seen from the app (`docs/multiple-traces.html`, stage 1):
//! adding a trace beside the open ones, closing one of several, naming
//! them, and what the panels and the sidebar do when the set changes.
//!
//! Adding and closing are undoable steps. Closing a trace removes its rows,
//! its rulers and the panels that show it together with the trace, so one
//! undo puts all of it back; the closed session stays in the journal and
//! comes back without reading the file again.

use std::sync::Arc;

use anyhow::Result;
use web_time::Instant;

use super::App;
use crate::Event;
use crate::document::TraceEdit;
use crate::session::{OpenSpec, Session};
use crate::trace::{Rescale, Retime, TraceId};

/// One trace as its chip in the frontend's toolbar, and its row in the
/// scope tree, show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceChip {
    pub trace: TraceId,
    pub name: String,
    /// Where it was opened from, for a tooltip.
    pub source: String,
    /// `VTR`, `FST`; `None` while it opens.
    pub format: Option<&'static str>,
    /// Its own time unit (`ns`); `None` while it opens.
    pub unit: Option<String>,
    pub signals: usize,
    pub loading: bool,
    /// Whether Close applies to it alone: A holds the workspace while other
    /// traces are open.
    pub closable: bool,
    /// Whether the user named it.
    pub renamed: bool,
}

impl TraceChip {
    /// `VTR · ns`, or `opening…`.
    pub fn detail(&self) -> String {
        match (self.format, &self.unit) {
            (Some(format), Some(unit)) => format!("{format} · {unit}"),
            _ => "opening…".into(),
        }
    }

    /// `VTR · ns · 16 signals`, as the scope tree's trace row says it.
    pub fn row_detail(&self) -> String {
        if self.loading {
            return self.detail();
        }
        let s = if self.signals == 1 { "" } else { "s" };
        format!("{} · {} signal{s}", self.detail(), self.signals)
    }

    /// The tooltip: the letter, the name and where it came from.
    pub fn tooltip(&self) -> String {
        format!("Trace {} · {} — {}", self.trace, self.name, self.source)
    }
}

impl App {
    /// The open and opening traces, in letter order, as chips show them.
    pub fn trace_chips(&self) -> Vec<TraceChip> {
        let traces = self.doc.traces();
        let others = traces.len() > 1;
        traces
            .iter()
            .map(|slot| TraceChip {
                trace: slot.id,
                name: traces.name(slot.id).unwrap_or_default(),
                source: slot.source.clone(),
                format: slot.format(),
                unit: slot.unit().map(|u| u.label()),
                signals: slot.session().map_or(0, |s| s.info().signal_count),
                loading: matches!(slot.state(), crate::trace::SlotState::Loading),
                closable: !(others && slot.id == TraceId::A),
                renamed: slot.rename.is_some(),
            })
            .collect()
    }

    /// Changes whenever a wave panel's rows or the panels do: what
    /// [`App::members_on_waves`] returns holds until then.
    pub fn rows_revision(&self) -> (u64, u64) {
        let rows = self
            .panels
            .iter()
            .filter_map(|p| p.kind.waves())
            .map(|w| w.revision())
            .sum();
        (self.panels.revision(), rows)
    }

    /// The variables and generators some wave panel shows as a row, so the
    /// member list can mark them. It walks every row: frontends keep it
    /// until [`App::rows_revision`] changes.
    pub fn members_on_waves(
        &self,
    ) -> std::collections::HashSet<crate::trace::Traced<crate::data::Member>> {
        use crate::data::Member;
        let rows = self
            .panels
            .iter()
            .filter_map(|p| p.kind.waves())
            .flat_map(|w| w.items().iter().map(|e| &e.row));
        let mut shown = std::collections::HashSet::new();
        for row in rows {
            if let Some(var) = row.signal().and_then(|s| s.source.var()) {
                shown.insert(var.map(Member::Var));
            } else if let Some(track) = row.lane_track()
                && let Some(h) = self.doc.hierarchy(track.trace)
                && let Some(id) = h.generators.iter().position(|g| g.track == track.item)
            {
                shown.insert(track.with(Member::Generator(id)));
            }
        }
        shown
    }

    /// The chip of one trace.
    pub fn trace_chip(&self, trace: TraceId) -> Option<TraceChip> {
        self.trace_chips().into_iter().find(|c| c.trace == trace)
    }

    /// Add a trace a host opened from a durable location (a path or URI it
    /// can open again).
    pub fn add_resource(&mut self, spec: OpenSpec, uri: String) {
        self.add_trace(spec, Some(uri));
    }

    /// Start opening another trace beside the open ones. With nothing open
    /// it is an ordinary open.
    pub(crate) fn add_trace(&mut self, spec: OpenSpec, uri: Option<String>) {
        if self.doc.traces().is_empty() {
            match uri {
                Some(uri) => self.open_resource(spec, uri),
                None => self.open(spec),
            }
            return;
        }
        match self.doc.add_trace(spec) {
            Ok(id) => self.doc.set_uri(id, uri),
            Err(error) => self.events.push(Event::Notice(format!("{error:#}"))),
        }
        self.changed();
    }

    /// Add an opened session as another trace immediately (tests, and hosts
    /// that already hold one). The first session is an ordinary open.
    pub fn add_session(&mut self, session: Arc<dyn Session>) -> Result<TraceId> {
        let session = self.account_local_session(session)?;
        if !self.doc.is_loaded() {
            self.set_session(session);
            return Ok(TraceId::A);
        }
        let (trace, refined) = self.doc.add_session(session)?;
        self.trace_joined(trace, refined, Instant::now());
        Ok(trace)
    }

    /// A trace other than the first finished opening: place the rest anew
    /// if the unit refined, show it in the sidebar, and make its arrival an
    /// undoable step.
    pub(super) fn trace_joined(&mut self, trace: TraceId, refined: Option<u64>, now: Instant) {
        if let Some(factor) = refined {
            self.refine(factor, now);
        }
        self.scopes.add_trace(self.doc.traces(), trace);
        self.variables.rebuild(self.doc.traces());
        self.record_trace_step(format!("Add trace {trace}"), TraceEdit::Remove(trace), now);
        // Panel titles and chips name traces once there are several.
        self.layout_changed();
    }

    /// Journal a trace edit as a step of its own, or as part of the command
    /// or gesture being recorded.
    fn record_trace_step(&mut self, label: String, inverse: TraceEdit, now: Instant) {
        let recording = self.history.is_recording();
        if !recording {
            self.begin_step(now);
        }
        self.doc.note_trace_edit(label, inverse);
        if !recording {
            self.end_step(now);
        }
    }

    /// The session unit became `factor` times finer to admit a trace:
    /// every time outside the document follows, and every row, table and
    /// summary of the traces already open loads again in the new unit.
    fn refine(&mut self, factor: u64, now: Instant) {
        let by = Rescale::Finer(factor);
        self.history.retime(by);
        let Self { panels, doc, .. } = self;
        for panel in panels.iter_mut() {
            panel.kind.retime(by);
            if let Some(waves) = panel.kind.waves_mut() {
                waves.reload_rows(doc);
            }
            if let Some(table) = panel.kind.table_mut() {
                table.detach(doc);
                if let Err(error) = table.attach(doc, &Default::default()) {
                    table.state = crate::table::TableState::Failed(error.to_string());
                }
            }
        }
        self.sync_analog_summaries();
        self.sync_group_summaries();
        self.workspace.scheduler.changed(now);
    }

    /// Close trace `trace` of several (`Command::RemoveTrace`): its rows,
    /// its rulers and the panels that show it go with it, as one step named
    /// “Close trace B”. Closing the only open trace closes everything.
    pub(super) fn remove_trace(&mut self, trace: TraceId) {
        let others = self.doc.traces().loaded().any(|(id, _)| id != trace);
        let Some(slot) = self.doc.traces().get(trace) else {
            return;
        };
        // A trace still opening has not joined: closing it cancels the open.
        if slot.session().is_none() && others {
            self.doc.remove_trace(trace);
            self.scopes.remove_trace(self.doc.traces(), trace);
            self.layout_changed();
            return;
        }
        if !others {
            self.close_trace();
            return;
        }
        if trace == TraceId::A {
            self.events.push(Event::Notice(
                "Trace A holds the workspace: close the other traces first, or close all.".into(),
            ));
            self.changed();
            return;
        }
        let label = format!("Close trace {trace}");
        for panel in self.panels.iter_mut() {
            if let Some(waves) = panel.kind.waves_mut() {
                waves.remove_trace_rows(trace, label.clone());
            }
            if let Some(nav) = panel.kind.nav_mut() {
                nav.forget_trace(trace);
            }
        }
        let doomed: Vec<_> = self
            .panels
            .iter()
            .filter(|p| p.kind.shows_trace(trace))
            .map(|p| p.id)
            .collect();
        for id in doomed {
            if let Err(error) =
                self.restructure(Some(label.clone()), |panels| Ok(((), panels.close(id)?)))
            {
                self.events.push(Event::Notice(error.to_string()));
            }
        }
        // Every edit above is collected before the trace leaves, so undo
        // puts the trace back first and its rows and panels after.
        self.collect_edits();
        self.sync_lane_tracks();
        let Some(slot) = self.forget_trace(trace) else {
            return;
        };
        self.doc
            .note_trace_edit(label.clone(), TraceEdit::Restore(Box::new(slot)));
        self.history.name_step(label);
        self.layout_changed();
    }

    /// Take a trace out of the document after releasing what panels still
    /// hold of it, and drop it from the sidebar.
    fn forget_trace(&mut self, trace: TraceId) -> Option<crate::trace::TraceSlot> {
        let Self { panels, doc, .. } = self;
        for (_, model) in panels.transactions_mut() {
            model.forget_trace(doc, trace);
        }
        for track in self.lane_tracks.1.iter().filter(|t| t.trace == trace) {
            self.doc.release_track(*track);
        }
        self.lane_tracks.1.retain(|t| t.trace != trace);
        let slot = self.doc.remove_trace(trace)?;
        self.scopes.remove_trace(self.doc.traces(), trace);
        self.variables.remove_trace(self.doc.traces(), trace);
        Some(slot)
    }

    /// Name a trace, as an undoable step.
    pub(super) fn rename_trace(&mut self, trace: TraceId, name: Option<&str>) {
        if self.doc.rename_trace(trace, name) {
            self.layout_changed();
        }
    }

    /// Select a trace's row in the scope tree and scroll to it.
    pub(super) fn reveal_trace(&mut self, trace: TraceId) {
        let traces = self.doc.traces();
        if traces.session(trace).is_none() {
            return;
        }
        let node = if traces.is_combined() {
            crate::sidebar::TreeNode::trace(trace)
        } else {
            match self.scopes.visible.first() {
                Some((node, _)) => *node,
                None => return,
            }
        };
        self.scopes.select(node);
        self.variables
            .set_scope(self.doc.traces(), node.traced_scope());
        if let Some(row) = self.scopes.visible.iter().position(|(n, _)| *n == node) {
            self.events.push(Event::RevealScopeRow(row));
        }
        self.changed();
    }

    /// Undo or redo a trace edit; returns its inverse.
    pub(super) fn apply_trace_edit(&mut self, edit: TraceEdit) -> Result<TraceEdit> {
        let inverse = match edit {
            TraceEdit::Remove(trace) => TraceEdit::Restore(Box::new(
                self.forget_trace(trace)
                    .ok_or_else(|| anyhow::anyhow!("trace {trace} is not open"))?,
            )),
            TraceEdit::Restore(slot) => {
                let trace = slot.id;
                if let Some(factor) = self.doc.restore_trace(*slot)? {
                    self.refine(factor, Instant::now());
                }
                self.scopes.add_trace(self.doc.traces(), trace);
                self.variables.rebuild(self.doc.traces());
                TraceEdit::Remove(trace)
            }
            TraceEdit::Rename(trace, name) => {
                TraceEdit::Rename(trace, self.doc.swap_trace_name(trace, name)?)
            }
        };
        self.layout_changed();
        Ok(inverse)
    }
}
