//! Frontend-neutral workspace files. Parsing and resolution are side-effect free;
//! a validated plan installs the complete session view in one operation.

use crate::nav::Tween;
use crate::panels::{
    Layout, Panel, PanelId, Panels,
    workspace::{RestoreContext, Traces, valid_viewport},
};
use crate::sidebar::ScopeTreeModel;
use crate::wave::viewport::Viewport;
use crate::{
    App,
    data::source::Lookup,
    marker::{Marker, Reference},
    sidebar::TreeNode,
    trace::{TraceId, Traced},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::HashSet;

pub const FORMAT: &str = "volna-workspace";
pub const VERSION: u32 = 5;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ROWS: usize = 100_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub format: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// The session's traces; A owns the workspace (its sidecar and recent
    /// entry name it).
    pub traces: Vec<Trace>,
    /// The unit every saved time is counted in, `10^timescale` seconds: the
    /// finest of the traces when it was saved. A restore converts the times
    /// to the unit of the traces it opens.
    pub timescale: i8,
    pub layout: Layout,
    pub focused: PanelId,
    pub panels: Vec<Box<RawValue>>,
    pub shared: Shared,
    pub sidebar: Sidebar,
}

/// One trace of a saved session: where it is, relative to the workspace
/// when it can be, and what it was when saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub letter: TraceId,
    pub path: String,
    pub name: String,
    /// The name the user gave it, when they did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    pub timescale: i8,
    pub time_range: (u64, u64),
    pub design_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Shared {
    pub viewport: Viewport,
    pub cursor: Option<u64>,
    pub markers: Vec<Marker>,
    /// Where measurements start: `{"marker": n}` or `{"time": t}`.
    pub reference: Option<Reference>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sidebar {
    pub visible: bool,
    pub width: f32,
    pub scopes_fraction: f32,
    /// The selected scope's path in its trace; an empty path is the
    /// trace's own row.
    pub selected_scope: Option<Traced<Vec<String>>>,
    pub expanded: Vec<Traced<Vec<String>>>,
    pub filter: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub notices: Vec<String>,
}
impl RestoreReport {
    pub(crate) fn push(&mut self, text: impl Into<String>) {
        self.notices.push(text.into());
    }
}

pub struct RestorePlan {
    generation: u64,
    /// How saved times convert to the session unit, when it differs.
    retime: Option<crate::trace::Rescale>,
    /// The names the user gave the traces, by letter.
    renames: Vec<(TraceId, Option<String>)>,
    panels: Panels,
    shared: Shared,
    sidebar: Sidebar,
    scopes: ScopeTreeModel,
    report: RestoreReport,
}

impl Workspace {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_BYTES,
            "workspace exceeds {MAX_BYTES} bytes"
        );
        #[derive(Deserialize)]
        struct Header {
            format: String,
            version: u32,
        }
        let header: Header = serde_json::from_slice(bytes).context("invalid workspace JSON")?;
        ensure!(header.format == FORMAT, "unrecognized workspace format");
        ensure!(
            header.version == VERSION,
            "unsupported workspace version {}",
            header.version
        );
        serde_json::from_slice(bytes).context("invalid workspace")
    }

    /// A workspace of this format saved by an older version. During the
    /// research phase such files are discarded rather than migrated.
    pub fn is_outdated(bytes: &[u8]) -> bool {
        #[derive(Deserialize)]
        struct Header {
            format: String,
            version: u32,
        }
        bytes.len() <= MAX_BYTES
            && serde_json::from_slice::<Header>(bytes)
                .is_ok_and(|h| h.format == FORMAT && h.version < VERSION)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Capture destinations use a durable URI, or a reference relative to the workspace:
    /// `path_of` says how each trace is referenced, and `None` leaves a
    /// trace out (its rows then restore unresolved). Histories and
    /// transient input/animation state are never serialized.
    pub fn capture(
        app: &App,
        path_of: impl Fn(&crate::trace::TraceSlot) -> Result<Option<String>>,
        supersedes: Option<String>,
    ) -> Result<Self> {
        ensure!(app.doc.is_loaded(), "no trace open");
        let mut traces = Vec::new();
        for slot in app.doc.traces().iter() {
            let Some(session) = slot.session() else {
                continue;
            };
            let Some(path) = path_of(slot)? else {
                continue;
            };
            let info = session.info();
            traces.push(Trace {
                letter: slot.id,
                path,
                name: info.name.clone(),
                rename: slot.rename.clone(),
                timescale: info.timescale,
                time_range: info.time_range,
                design_id: info.design_id.clone(),
            });
        }
        ensure!(
            traces.iter().any(|t| t.letter == TraceId::A),
            "trace A has no durable location"
        );
        let (layout, focused, saved_panels) = app.panels.saved_view();
        let panels = saved_panels
            .into_iter()
            .map(|p| p.save(&app.doc))
            .collect::<Result<_>>()?;
        let path = |scope: Traced<crate::data::ScopeId>| {
            let h = app.doc.hierarchy(scope.trace)?;
            let path = h.scope_path(scope.item).into_iter().map(str::to_owned);
            Some(scope.with(path.collect::<Vec<_>>()))
        };
        let mut expanded = app
            .scopes
            .expanded()
            .filter_map(path)
            .chain(app.scopes.unresolved_expanded.iter().cloned())
            .collect::<Vec<_>>();
        expanded.sort();
        expanded.dedup();
        let selected_scope = app.scopes.unresolved_selected.clone().or_else(|| {
            let node = app.scopes.selected?;
            match node.traced_scope() {
                Some(scope) => path(scope),
                None => Some(Traced::new(node.trace, Vec::new())),
            }
        });
        Ok(Self {
            format: FORMAT.into(),
            version: VERSION,
            supersedes,
            traces,
            timescale: app.doc.timescale(),
            layout,
            focused,
            panels,
            shared: Shared {
                viewport: app.doc.shared.viewport.target(),
                cursor: app.doc.shared.cursor,
                markers: app.doc.markers().to_vec(),
                reference: app.doc.reference(),
            },
            sidebar: Sidebar {
                visible: app.sidebar_visible,
                width: app.sidebar_width,
                scopes_fraction: app.scopes_fraction,
                selected_scope,
                expanded,
                filter: app.variables.filter.clone(),
            },
        })
    }

    /// Resolve against an immutable live session. Hosts resolve the trace resource
    /// before this call; `expected_trace` is its durable identity.
    pub fn prepare(
        self,
        app: &App,
        expected_trace: &str,
        workspace_location: &str,
    ) -> Result<RestorePlan> {
        ensure!(
            self.format == FORMAT && self.version == VERSION,
            "unsupported workspace format/version"
        );
        let owner = self
            .traces
            .iter()
            .find(|t| t.letter == TraceId::A)
            .context("workspace names no trace A")?;
        ensure!(
            resolve_trace(&owner.path, workspace_location)? == expected_trace,
            "workspace references a different trace; open that trace first"
        );
        let mut report = RestoreReport::default();
        let mut letters = HashSet::new();
        let mut open = Traces::default();
        for saved in &self.traces {
            ensure!(
                letters.insert(saved.letter),
                "trace {} listed twice",
                saved.letter
            );
            ensure!(
                saved.time_range.0 <= saved.time_range.1,
                "invalid trace time range"
            );
            // A was checked against the caller's trace above. Rows of the
            // others resolve against the trace that holds their letter, the
            // saved one or one the user opened in its place.
            let uri = resolve_trace(&saved.path, workspace_location)?;
            let slot = app.doc.traces().get(saved.letter);
            let Some(session) = slot.and_then(|slot| slot.session()) else {
                report.push(format!(
                    "Trace {} is not open: {}",
                    saved.letter, saved.path
                ));
                continue;
            };
            if saved.letter != TraceId::A
                && slot.is_some_and(|slot| slot.uri.as_deref() != Some(uri.as_str()))
            {
                report.push(format!(
                    "Trace {} is another file than the saved {}; its rows resolve by path",
                    saved.letter, saved.path
                ));
            }
            let info = session.info();
            let which = |what: &str| match saved.letter {
                TraceId::A => format!("Trace {what}"),
                letter => format!("Trace {letter} {what}"),
            };
            if saved.name != info.name {
                report.push(format!(
                    "{} differs from the saved workspace",
                    which("name")
                ));
            }
            if saved.timescale != info.timescale {
                report.push(format!(
                    "{} differs from the saved workspace",
                    which("timescale")
                ));
            }
            if let (Some(saved), Some(current)) = (&saved.design_id, &info.design_id)
                && saved != current
            {
                report.push(format!(
                    "{} differs from the saved workspace",
                    which("design identity")
                ));
            }
            open.insert(saved.letter, session);
        }
        let renames = self
            .traces
            .iter()
            .filter(|t| open.hierarchy(t.letter).is_some())
            .map(|t| (t.letter, t.rename.clone()))
            .collect();
        let retime = crate::trace::Rescale::between(self.timescale, app.doc.timescale());
        if let Some(crate::trace::Rescale::Coarser(_)) = retime {
            report.push(format!(
                "Times were saved in {}; they are rounded to {}",
                crate::wave::timeline::unit_label(self.timescale),
                crate::wave::timeline::unit_label(app.doc.timescale())
            ));
        }
        if let Some(hash) = &self.supersedes {
            ensure!(
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
                "invalid fallback base hash"
            );
        }
        valid_viewport(self.shared.viewport)?;
        let mut marker_ids = HashSet::new();
        for m in &self.shared.markers {
            // Numbers are positive by type; a zero fails to parse.
            ensure!(marker_ids.insert(m.id), "duplicate marker ID");
        }
        if let Some(Reference::Marker(id)) = self.shared.reference {
            ensure!(marker_ids.contains(&id), "reference to missing marker {id}");
        }
        ensure!(
            self.sidebar.width.is_finite() && self.sidebar.width > 0.0,
            "invalid sidebar width"
        );
        ensure!(
            self.sidebar.scopes_fraction.is_finite()
                && (0.0..1.0).contains(&self.sidebar.scopes_fraction),
            "invalid sidebar fraction"
        );
        ensure!(
            self.panels.len() <= crate::panels::MAX_PANELS,
            "too many panels"
        );
        let mut context = RestoreContext {
            doc: &app.doc,
            traces: &open,
            budget: app.table_memory_budget(),
            detail_items: app.settings.resolved().transaction.detail_items,
            report: &mut report,
            row_count: 0,
        };
        let panels = self
            .panels
            .into_iter()
            .map(|raw| Panel::restore(raw, &mut context))
            .collect::<Result<_>>()?;
        let panels = Panels::restore(self.layout, panels, self.focused, &app.panels)?;
        let mut scopes = ScopeTreeModel::default();
        let find = |path: &Traced<Vec<String>>| {
            open.hierarchy(path.trace)
                .map_or(Lookup::Missing, |h| h.find_scope(&path.item))
        };
        let selected = match &self.sidebar.selected_scope {
            Some(path) if path.item.is_empty() && open.hierarchy(path.trace).is_some() => {
                Some(TreeNode::trace(path.trace))
            }
            Some(path) => match find(path) {
                Lookup::Found(id) => Some(TreeNode::scope(path.with(id))),
                other => {
                    report.push(format!("{other:?} selected scope: {:?}", path.item));
                    scopes.unresolved_selected = Some(path.clone());
                    None
                }
            },
            None => None,
        };
        let mut expanded = HashSet::new();
        for path in &self.sidebar.expanded {
            match find(path) {
                Lookup::Found(id) => {
                    expanded.insert(path.with(id));
                }
                other => {
                    report.push(format!("{other:?} expanded scope: {:?}", path.item));
                    scopes.unresolved_expanded.push(path.clone());
                }
            }
        }
        scopes.restore(app.doc.traces(), selected, expanded);
        Ok(RestorePlan {
            generation: app.doc.generation(),
            retime,
            renames,
            panels,
            shared: self.shared,
            sidebar: self.sidebar,
            scopes,
            report,
        })
    }
}

impl RestorePlan {
    pub fn report(&self) -> &RestoreReport {
        &self.report
    }

    pub fn commit(self, app: &mut App) -> Result<RestoreReport> {
        ensure!(
            app.doc.generation() == self.generation,
            "trace changed while preparing workspace"
        );
        ensure!(app.doc.is_loaded(), "no trace open");
        app.retire_panels();
        // Invalidate earlier history results before installing rows.
        app.doc.restart();
        for (trace, name) in self.renames {
            app.doc.restore_trace_name(trace, name);
        }
        app.doc.shared.viewport = Tween::new(self.shared.viewport);
        app.doc.shared.cursor = self.shared.cursor;
        app.doc.restore_markers(self.shared.markers);
        app.doc.set_reference(self.shared.reference);
        app.panels = self.panels;
        // Saved times follow the unit of the traces open now.
        if let Some(by) = self.retime {
            use crate::trace::Retime;
            app.doc.retime(by);
            for panel in app.panels.iter_mut() {
                panel.kind.retime(by);
            }
        }
        let mut report = self.report;
        let ids: Vec<_> = app.panels.iter().map(|p| p.id).collect();
        report.notices.extend(app.adopt_panels(&ids));
        app.scopes = self.scopes;
        app.sidebar_width = self.sidebar.width;
        app.sidebar_visible = self.sidebar.visible;
        app.scopes_fraction = self.sidebar.scopes_fraction;
        app.variables.filter = self.sidebar.filter;
        app.variables
            .set_scope(app.doc.traces(), app.scopes.selected_scope());
        app.workspace_restored();
        Ok(report)
    }
}

/// URI resolution preserves remote schemes and authorities. A host-storage
/// fallback must contain an absolute trace URI, since it has no directory.
pub fn resolve_trace(reference: &str, workspace_location: &str) -> Result<String> {
    if let Ok(uri) = url::Url::parse(reference) {
        return Ok(uri.into());
    }
    let base = url::Url::parse(workspace_location).context("workspace location is not a URI")?;
    ensure!(
        !base.cannot_be_a_base(),
        "relative trace reference needs a workspace directory"
    );
    Ok(base.join(reference)?.into())
}

pub mod persistence;

mod changes;
mod controller;
pub(crate) use changes::Stamp;
pub use controller::State;

pub mod recent;
pub mod state;
