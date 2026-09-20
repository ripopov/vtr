//! Frontend-neutral workspace files. Parsing and resolution are side-effect free;
//! a validated plan installs the complete session view in one operation.

use crate::nav::Tween;
use crate::panels::{Layout, Panel, PanelId, PanelKind, Panels};
use crate::pipeline::{PipelineModel, RowView, TrackSource};
use crate::sidebar::ScopeTreeModel;
use crate::table::columns::{ColumnSet, TransactionColumn};
use crate::table::{SignalSource, TableModel, TableSource};
use crate::wave::{
    model::{DisplayedSignal, Link, RowSource, WaveModel},
    viewport::Viewport,
};
use crate::{App, data::source::Lookup, document::Marker};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::{BTreeSet, HashSet};

pub const FORMAT: &str = "volna-workspace";
pub const VERSION: u32 = 2;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ROWS: usize = 100_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub format: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub trace: Trace,
    pub layout: Layout,
    pub focused: PanelId,
    pub panels: Vec<Box<RawValue>>,
    pub shared: Shared,
    pub sidebar: Sidebar,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub path: String,
    pub name: String,
    pub timescale: i8,
    pub time_range: (u64, u64),
    pub design_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Shared {
    pub viewport: Viewport,
    pub cursor: Option<u64>,
    pub markers: Vec<Marker>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sidebar {
    pub visible: bool,
    pub width: f32,
    pub scopes_fraction: f32,
    pub selected_scope: Option<Vec<String>>,
    pub expanded: Vec<Vec<String>>,
    pub filter: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Columns {
    names: f32,
    values: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Row {
    signal: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nth: Option<usize>,
    format: String,
}

// RawValue distinguishes an omitted local cursor from an explicitly saved null.
#[derive(Serialize, Deserialize)]
struct WavePanel {
    id: PanelId,
    kind: String,
    version: u32,
    title: Option<String>,
    link: Link,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    viewport: Option<Viewport>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_raw"
    )]
    cursor: Option<Box<RawValue>>,
    scroll_y: f32,
    columns: Columns,
    rows: Vec<Row>,
    selected: BTreeSet<usize>,
}

fn present_raw<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

/// A pipeline panel: the track path, its navigation, row axis and label column.
#[derive(Serialize, Deserialize)]
struct PipelinePanel {
    #[serde(default)]
    follow: crate::pipeline::FollowActivity,
    id: PanelId,
    kind: String,
    version: u32,
    title: Option<String>,
    track: Vec<String>,
    link: Link,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    viewport: Option<Viewport>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_raw"
    )]
    cursor: Option<Box<RawValue>>,
    rows: RowView,
    label_width: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SavedTableSource {
    Generator { path: Vec<String> },
    Signals { signals: Vec<SavedTableSignal> },
}

#[derive(Serialize, Deserialize)]
struct SavedTableSignal {
    path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nth: Option<usize>,
}

#[derive(Serialize, Deserialize)]
struct TablePanel {
    id: PanelId,
    kind: String,
    version: u32,
    title: Option<String>,
    source: SavedTableSource,
    columns: Vec<String>,
    link: Link,
}

/// The fields every saved panel shares; alone, they describe a start panel.
#[derive(Serialize, Deserialize)]
struct PanelHeader {
    id: PanelId,
    kind: String,
    version: u32,
    title: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub notices: Vec<String>,
}
impl RestoreReport {
    fn push(&mut self, text: impl Into<String>) {
        self.notices.push(text.into());
    }
}

pub struct RestorePlan {
    generation: u64,
    panels: Panels,
    shared: Shared,
    sidebar: Sidebar,
    scopes: ScopeTreeModel,
    report: RestoreReport,
}

fn valid_viewport(v: Viewport) -> Result<()> {
    ensure!(
        v.start.is_finite()
            && v.end.is_finite()
            && v.start < v.end
            && (v.end - v.start).is_finite(),
        "invalid viewport"
    );
    Ok(())
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

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Capture destinations use a durable URI, or a reference relative to the workspace.
    /// Histories and transient input/animation state are never serialized.
    pub fn capture(app: &App, trace_path: String, supersedes: Option<String>) -> Result<Self> {
        let session = app.doc.session().context("no trace open")?;
        let h = session.hierarchy();
        let info = session.info();
        let (layout, focused, saved_panels) = app.panels.saved_view();
        let panels = saved_panels
            .into_iter()
            .map(|panel| -> Result<_> {
                let PanelKind::Waves(w) = &panel.kind else {
                    return match &panel.kind {
                        PanelKind::Unsupported(raw) => Ok(raw.clone()),
                        PanelKind::Pipeline(p) => {
                            Ok(serde_json::value::to_raw_value(&PipelinePanel {
                                follow: p.follow,
                                id: panel.id,
                                kind: "pipeline".into(),
                                version: 1,
                                title: panel.title.clone(),
                                track: p.track.path().to_vec(),
                                link: p.nav.link,
                                viewport: (!p.nav.link.viewport)
                                    .then(|| p.nav.local_viewport.target()),
                                cursor: (!p.nav.link.cursor)
                                    .then(|| serde_json::value::to_raw_value(&p.nav.local_cursor))
                                    .transpose()?,
                                rows: p.rows.target(),
                                label_width: p.label_width,
                            })?)
                        }
                        PanelKind::Table(table) => {
                            let source = match &table.source {
                                TableSource::Generator(track) => SavedTableSource::Generator {
                                    path: track.path().to_vec(),
                                },
                                TableSource::Signals(signals) => SavedTableSource::Signals {
                                    signals: signals
                                        .iter()
                                        .map(|signal| SavedTableSignal {
                                            path: signal.path.clone(),
                                            nth: signal.nth,
                                        })
                                        .collect(),
                                },
                            };
                            let columns = match &table.columns {
                                ColumnSet::Transactions(visible) => visible
                                    .iter()
                                    .map(|column| column.key().to_owned())
                                    .collect(),
                                ColumnSet::Signals { time, visible } => {
                                    let mut keys = Vec::new();
                                    if *time {
                                        keys.push("time".into());
                                    }
                                    keys.extend(
                                        visible
                                            .iter()
                                            .enumerate()
                                            .filter(|(_, visible)| **visible)
                                            .map(|(index, _)| format!("signal:{index}")),
                                    );
                                    keys
                                }
                            };
                            Ok(serde_json::value::to_raw_value(&TablePanel {
                                id: panel.id,
                                kind: "table".into(),
                                version: 2,
                                title: panel.title.clone(),
                                source,
                                columns,
                                link: table.nav.link,
                            })?)
                        }
                        PanelKind::Start => Ok(serde_json::value::to_raw_value(&PanelHeader {
                            id: panel.id,
                            kind: "start".into(),
                            version: 1,
                            title: panel.title.clone(),
                        })?),
                        _ => unreachable!("settings panels are not saved"),
                    };
                };
                let rows = w
                    .items
                    .iter()
                    .map(|item| {
                        let (signal, nth) = item.source.locator(h);
                        Row {
                            signal,
                            nth,
                            format: item.format_id(),
                        }
                    })
                    .collect();
                Ok(serde_json::value::to_raw_value(&WavePanel {
                    id: panel.id,
                    kind: "waves".into(),
                    version: 1,
                    title: panel.title.clone(),
                    link: w.nav.link,
                    viewport: (!w.nav.link.viewport).then(|| w.nav.local_viewport.target()),
                    cursor: (!w.nav.link.cursor)
                        .then(|| serde_json::value::to_raw_value(&w.nav.local_cursor))
                        .transpose()?,
                    scroll_y: w.scroll_y,
                    columns: Columns {
                        names: w.names_width,
                        values: w.values_width,
                    },
                    rows,
                    selected: w.selected.clone(),
                })?)
            })
            .collect::<Result<_>>()?;
        let path = |id| {
            h.scope_path(id)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let mut expanded = app
            .scopes
            .expanded()
            .map(path)
            .chain(app.scopes.unresolved_expanded.iter().cloned())
            .collect::<Vec<_>>();
        expanded.sort();
        expanded.dedup();
        Ok(Self {
            format: FORMAT.into(),
            version: VERSION,
            supersedes,
            trace: Trace {
                path: trace_path,
                name: info.name.clone(),
                timescale: info.timescale,
                time_range: info.time_range,
                design_id: info.design_id.clone(),
            },
            layout,
            focused,
            panels,
            shared: Shared {
                viewport: app.doc.shared.viewport.target(),
                cursor: app.doc.shared.cursor,
                markers: app.doc.markers.clone(),
            },
            sidebar: Sidebar {
                visible: app.sidebar_visible,
                width: app.sidebar_width,
                scopes_fraction: app.scopes_fraction,
                selected_scope: app
                    .scopes
                    .unresolved_selected
                    .clone()
                    .or_else(|| app.scopes.selected.map(path)),
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
        ensure!(
            resolve_trace(&self.trace.path, workspace_location)? == expected_trace,
            "workspace references a different trace; open that trace first"
        );
        let session = app.doc.session().context("no trace open")?;
        let h = session.hierarchy();
        let mut report = RestoreReport::default();
        if self.trace.name != session.info().name {
            report.push("Trace name differs from the saved workspace");
        }
        if self.trace.timescale != session.info().timescale {
            report.push("Trace timescale differs; saved times were kept unchanged");
        }
        if let (Some(saved), Some(current)) = (&self.trace.design_id, &session.info().design_id)
            && saved != current
        {
            report.push("Trace design identity differs from the saved workspace");
        }
        ensure!(
            self.trace.time_range.0 <= self.trace.time_range.1,
            "invalid trace time range"
        );
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
            ensure!(
                m.id > 0 && m.id < u64::MAX && marker_ids.insert(m.id),
                "invalid or duplicate marker ID"
            );
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
        let mut panels = Vec::new();
        let mut row_count = 0;
        for raw in self.panels {
            let header: PanelHeader =
                serde_json::from_str(raw.get()).context("invalid panel header")?;
            if header.kind == "pipeline" && header.version == 1 {
                let saved: PipelinePanel =
                    serde_json::from_str(raw.get()).context("invalid pipeline panel")?;
                ensure!(!saved.track.is_empty(), "empty pipeline track path");
                ensure!(
                    saved.rows.top.is_finite()
                        && saved.rows.row_px.is_finite()
                        && (crate::pipeline::rows::ROW_PX_MIN..=crate::pipeline::rows::ROW_PX_MAX)
                            .contains(&saved.rows.row_px),
                    "invalid pipeline rows"
                );
                ensure!(
                    saved.label_width.is_finite()
                        && (crate::pipeline::layout::LABEL_W_MIN
                            ..=crate::pipeline::layout::LABEL_W_MAX)
                            .contains(&saved.label_width),
                    "invalid pipeline label width"
                );
                ensure!(
                    saved.link.viewport == saved.viewport.is_none(),
                    "local viewport must exist exactly when unlinked"
                );
                ensure!(
                    saved.link.cursor == saved.cursor.is_none(),
                    "local cursor must exist exactly when unlinked"
                );
                let track = match session.tracks().iter().find(|t| t.path == saved.track) {
                    Some(t) => TrackSource::Resolved {
                        track: t.id,
                        path: saved.track,
                    },
                    None => {
                        report.push(format!("Missing pipeline track: {:?}", saved.track));
                        TrackSource::Unresolved { path: saved.track }
                    }
                };
                let mut p = PipelineModel::new(track, saved.link);
                p.follow = saved.follow;
                if let Some(v) = saved.viewport {
                    valid_viewport(v)?;
                    p.nav.local_viewport.set(v);
                }
                if let Some(c) = saved.cursor {
                    p.nav.local_cursor =
                        serde_json::from_str(c.get()).context("invalid local cursor")?;
                }
                p.rows.set(saved.rows);
                p.label_width = saved.label_width;
                panels.push(Panel {
                    id: saved.id,
                    title: saved.title,
                    kind: PanelKind::Pipeline(Box::new(p)),
                });
                continue;
            }
            if header.kind == "table" && header.version == 2 {
                let saved: TablePanel =
                    serde_json::from_str(raw.get()).context("invalid table panel")?;
                let source = match saved.source {
                    SavedTableSource::Generator { path } => {
                        ensure!(!path.is_empty(), "empty table generator path");
                        match h.find_generator(&path) {
                            Lookup::Found(generator) => {
                                TableSource::Generator(TrackSource::Resolved {
                                    track: h.generators[generator].track,
                                    path,
                                })
                            }
                            other => {
                                report.push(format!("{other:?} table generator: {path:?}"));
                                TableSource::Generator(TrackSource::Unresolved { path })
                            }
                        }
                    }
                    SavedTableSource::Signals { signals } => {
                        ensure!(!signals.is_empty(), "empty table signal set");
                        let mut restored = Vec::with_capacity(signals.len());
                        for signal in signals {
                            ensure!(!signal.path.is_empty(), "empty table signal path");
                            let found = h.find_var(&signal.path, signal.nth);
                            let (var, reference, name) = match found {
                                Lookup::Found(var) => {
                                    (Some(var), Some(h.vars[var].signal), h.full_name(var))
                                }
                                other => {
                                    report
                                        .push(format!("{other:?} table signal: {:?}", signal.path));
                                    (None, None, signal.path.join("."))
                                }
                            };
                            restored.push(SignalSource {
                                path: signal.path,
                                nth: signal.nth,
                                var,
                                signal: reference,
                                name,
                            });
                        }
                        TableSource::Signals(restored)
                    }
                };
                let mut table = TableModel::new(
                    source,
                    saved.link,
                    app.table_memory_budget(),
                    app.settings.resolved().table.detail_items,
                );
                match &mut table.columns {
                    ColumnSet::Transactions(visible) => {
                        *visible = TransactionColumn::ALL
                            .into_iter()
                            .filter(|column| saved.columns.iter().any(|key| key == column.key()))
                            .collect();
                        if visible.is_empty() {
                            *visible = TransactionColumn::DEFAULT.to_vec();
                        }
                    }
                    ColumnSet::Signals { time, visible } => {
                        *time = saved.columns.iter().any(|key| key == "time");
                        for (index, value) in visible.iter_mut().enumerate() {
                            *value = saved
                                .columns
                                .iter()
                                .any(|key| key == &format!("signal:{index}"));
                        }
                        if !*time && !visible.iter().any(|value| *value) {
                            *time = true;
                        }
                    }
                }
                panels.push(Panel {
                    id: saved.id,
                    title: saved.title,
                    kind: PanelKind::Table(Box::new(table)),
                });
                continue;
            }
            if header.kind == "start" && header.version == 1 {
                panels.push(Panel {
                    id: header.id,
                    title: header.title,
                    kind: PanelKind::Start,
                });
                continue;
            }
            if header.kind != "waves" || header.version != 1 {
                report.push(format!(
                    "Unsupported panel {} ({} version {})",
                    header.id.0, header.kind, header.version
                ));
                panels.push(Panel {
                    id: header.id,
                    title: header.title,
                    kind: PanelKind::Unsupported(raw),
                });
                continue;
            }
            let saved: WavePanel =
                serde_json::from_str(raw.get()).context("invalid waveform panel")?;
            row_count += saved.rows.len();
            ensure!(row_count <= MAX_ROWS, "too many workspace rows");
            ensure!(
                saved.scroll_y.is_finite() && saved.scroll_y >= 0.0,
                "invalid row scroll"
            );
            ensure!(
                [saved.columns.names, saved.columns.values]
                    .iter()
                    .all(|v| v.is_finite() && *v >= crate::wave::layout::MIN_COLUMN),
                "invalid column width"
            );
            ensure!(
                saved.selected.iter().all(|i| *i < saved.rows.len()),
                "invalid selected row"
            );
            ensure!(
                saved.link.viewport == saved.viewport.is_none(),
                "local viewport must exist exactly when unlinked"
            );
            ensure!(
                saved.link.cursor == saved.cursor.is_none(),
                "local cursor must exist exactly when unlinked"
            );
            let mut w = WaveModel::new();
            w.nav.link = saved.link;
            if let Some(v) = saved.viewport {
                valid_viewport(v)?;
                w.nav.local_viewport.set(v);
            }
            if let Some(c) = saved.cursor {
                w.nav.local_cursor =
                    serde_json::from_str(c.get()).context("invalid local cursor")?;
            }
            w.scroll_y = saved.scroll_y;
            w.names_width = saved.columns.names;
            w.values_width = saved.columns.values;
            w.selected = saved.selected;
            w.anchor = w.selected.first().copied();
            for row in saved.rows {
                ensure!(!row.signal.is_empty(), "empty signal path");
                let found = h.find_var(&row.signal, row.nth);
                let (source, name, scope, shape) = match found {
                    Lookup::Found(var) => {
                        let v = &h.vars[var];
                        (
                            RowSource::Resolved {
                                var,
                                signal: v.signal,
                            },
                            v.name.clone(),
                            h.scope_path(v.scope).join("."),
                            v.shape,
                        )
                    }
                    _ => {
                        let ambiguous = found == Lookup::Ambiguous;
                        report.push(format!(
                            "{} signal: {:?}",
                            if ambiguous { "Ambiguous" } else { "Missing" },
                            row.signal
                        ));
                        let name = row.signal.last().unwrap().clone();
                        let scope = row.signal[..row.signal.len() - 1].join(".");
                        (
                            RowSource::Unresolved {
                                path: row.signal,
                                nth: row.nth,
                                ambiguous,
                            },
                            name,
                            scope,
                            crate::data::SignalShape::Bit,
                        )
                    }
                };
                let requested = app.doc.translators.get(&row.format);
                let translator = requested
                    .clone()
                    .unwrap_or_else(|| app.doc.translators.default_for(shape));
                if requested.is_none() {
                    report.push(format!("Unknown translator: {}", row.format));
                }
                w.items.push(DisplayedSignal {
                    source,
                    requested_format: requested.is_none().then_some(row.format),
                    name,
                    scope,
                    shape,
                    translator,
                    history: None,
                    error: None,
                });
            }
            panels.push(Panel {
                id: saved.id,
                title: saved.title,
                kind: PanelKind::Waves(Box::new(w)),
            });
        }
        let panels = Panels::restore(self.layout, panels, self.focused, &app.panels)?;
        let mut scopes = ScopeTreeModel::default();
        let selected = match &self.sidebar.selected_scope {
            Some(path) => match h.find_scope(path) {
                Lookup::Found(id) => Some(id),
                other => {
                    report.push(format!("{other:?} selected scope: {path:?}"));
                    scopes.unresolved_selected = Some(path.clone());
                    None
                }
            },
            None => None,
        };
        let mut expanded = HashSet::new();
        for path in &self.sidebar.expanded {
            match h.find_scope(path) {
                Lookup::Found(id) => {
                    expanded.insert(id);
                }
                other => {
                    report.push(format!("{other:?} expanded scope: {path:?}"));
                    scopes.unresolved_expanded.push(path.clone());
                }
            }
        }
        scopes.restore(h, selected, expanded);
        Ok(RestorePlan {
            generation: app.doc.generation(),
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
        let session = app.doc.session().context("no trace open")?.clone();
        // Invalidate earlier history results before installing rows.
        app.doc.set_session(session);
        app.doc.shared.viewport = Tween::new(self.shared.viewport);
        app.doc.shared.cursor = self.shared.cursor;
        app.doc.restore_markers(self.shared.markers);
        app.panels = self.panels;
        let mut report = self.report;
        for panel in app.panels.iter_mut() {
            if let Some(p) = panel.kind.pipeline_mut()
                && let Err(error) = p.attach(&mut app.doc)
            {
                report.push(format!("Pipeline {}: {error:#}", p.track.path().join(".")));
            }
        }
        let resident = std::collections::HashMap::new();
        for panel in app.panels.iter_mut() {
            if let Some(table) = panel.kind.table_mut()
                && let Err(error) = table.attach(&mut app.doc, &resident)
            {
                report.push(format!("Table: {error:#}"));
            }
        }
        app.scopes = self.scopes;
        app.sidebar_width = self.sidebar.width;
        app.sidebar_visible = self.sidebar.visible;
        app.scopes_fraction = self.sidebar.scopes_fraction;
        app.variables.filter = self.sidebar.filter;
        app.variables
            .set_scope(app.doc.hierarchy(), app.scopes.selected);
        for panel in app.panels.iter() {
            if let Some(w) = panel.kind.waves() {
                for row in &w.items {
                    if let Some(signal) = row.source.signal() {
                        app.doc.request_signal(signal);
                    }
                }
            }
        }
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

pub mod state;
