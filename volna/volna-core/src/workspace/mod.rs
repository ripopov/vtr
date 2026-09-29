//! Frontend-neutral workspace files. Parsing and resolution are side-effect free;
//! a validated plan installs the complete session view in one operation.

use crate::clock::ClockView;
use crate::nav::Tween;
use crate::panels::{Layout, Panel, PanelId, PanelKind, Panels};
use crate::pipeline::{PipelineModel, RowView, TrackSource};
use crate::sidebar::ScopeTreeModel;
use crate::table::columns::{ColumnSet, TransactionColumn};
use crate::table::{SignalSource, TableModel, TableSource};
use crate::transaction::{ShownRecord, TransactionModel, ViewPrefs, view::SectionKey};
use crate::wave::{
    Tint,
    analog::{Analog, AnalogDraw, AnalogRange},
    lane::TxLane,
    model::{DisplayedSignal, GroupRow, Link, RowHeight, RowSource, WaveModel, WaveRow},
    tree::{self, Entry},
    viewport::Viewport,
};
use crate::{
    App,
    data::Hierarchy,
    data::source::Lookup,
    data::transactions::TrackKind,
    marker::{Marker, Reference},
    sidebar::TreeNode,
    trace::{TraceId, Traced},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::{BTreeSet, HashSet};

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

#[derive(Debug, Serialize, Deserialize)]
struct Columns {
    names: f32,
    values: f32,
}

/// A wave row: a signal with its format, a generator's transaction lane, a
/// clock, or a group holding rows.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Row {
    Signal {
        #[serde(default, skip_serializing_if = "TraceId::is_a")]
        trace: TraceId,
        signal: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nth: Option<usize>,
        format: String,
        #[serde(default, skip_serializing_if = "RowHeight::is_default")]
        height: RowHeight,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        analog: Option<SavedAnalog>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wave::tint::serde_name"
        )]
        tint: Option<Tint>,
    },
    Lane {
        #[serde(default, skip_serializing_if = "TraceId::is_a")]
        trace: TraceId,
        generator: Vec<String>,
        #[serde(default, skip_serializing_if = "RowHeight::is_default")]
        height: RowHeight,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wave::tint::serde_name"
        )]
        tint: Option<Tint>,
    },
    /// A declared clock by its path, drawn from its stretches.
    Clock {
        #[serde(default, skip_serializing_if = "TraceId::is_a")]
        trace: TraceId,
        clock: String,
        #[serde(default, skip_serializing_if = "RowHeight::is_default")]
        height: RowHeight,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wave::tint::serde_name"
        )]
        tint: Option<Tint>,
    },
    /// A named group; its rows follow it on screen, indented.
    Group {
        name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        collapsed: bool,
        #[serde(default, skip_serializing_if = "RowHeight::is_default")]
        height: RowHeight,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "crate::wave::tint::serde_name"
        )]
        tint: Option<Tint>,
        rows: Vec<Row>,
    },
}

/// Rows as the tree a workspace stores: each group holds its rows.
fn nest(items: &[Entry], row: &dyn Fn(&WaveRow) -> Row) -> Vec<Row> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let end = tree::subtree_end(items, i);
        out.push(match &items[i].row {
            WaveRow::Group(g) => Row::Group {
                name: g.name.clone(),
                collapsed: g.collapsed,
                height: g.height,
                tint: g.tint,
                rows: nest(&items[i + 1..end], row),
            },
            other => row(other),
        });
        i = end;
    }
    out
}

/// The stored tree as pre-order rows with depths, refusing groups nested
/// deeper than [`tree::MAX_DEPTH`].
fn flatten(rows: Vec<Row>, depth: u8, out: &mut Vec<(u8, Row)>) -> Result<()> {
    for row in rows {
        ensure!(
            depth < tree::MAX_DEPTH,
            "groups nested deeper than {}",
            tree::MAX_DEPTH
        );
        match row {
            Row::Group {
                name,
                collapsed,
                height,
                tint,
                rows,
            } => {
                ensure!(!name.trim().is_empty(), "empty group name");
                out.push((
                    depth,
                    Row::Group {
                        name,
                        collapsed,
                        height,
                        tint,
                        rows: Vec::new(),
                    },
                ));
                flatten(rows, depth + 1, out)?;
            }
            row => out.push((depth, row)),
        }
    }
    Ok(())
}

/// A signal row drawn as a plot.
#[derive(Debug, Serialize, Deserialize)]
struct SavedAnalog {
    draw: AnalogDraw,
    range: AnalogRange,
}

/// The wave panel format: version 2 added transaction lanes as typed rows,
/// version 3 analog rows, version 4 groups (rows as a tree; `selected`
/// counts rows in pre-order, groups included), version 5 row colours
/// (`tint`, a name; an unknown one reads as Default).
const WAVES_VERSION: u32 = 5;

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
    /// Clock rulers, snapping clock and cycle origin.
    #[serde(default, skip_serializing_if = "ClockView::is_default")]
    clocks: ClockView,
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
    #[serde(default, skip_serializing_if = "TraceId::is_a")]
    trace: TraceId,
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
    /// The two-axis zoom's row cap, when a rows-only zoom raised it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row_cap: Option<f32>,
    label_width: f32,
    #[serde(default, skip_serializing_if = "ClockView::is_default")]
    clocks: ClockView,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SavedTableSource {
    Generator {
        #[serde(default, skip_serializing_if = "TraceId::is_a")]
        trace: TraceId,
        path: Vec<String>,
    },
    Signals {
        signals: Vec<SavedTableSignal>,
    },
}

#[derive(Serialize, Deserialize)]
struct SavedTableSignal {
    #[serde(default, skip_serializing_if = "TraceId::is_a")]
    trace: TraceId,
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

/// A pinned transaction panel remembers exactly which record it froze on.
#[derive(Serialize, Deserialize)]
struct SavedRecord {
    #[serde(default, skip_serializing_if = "TraceId::is_a")]
    trace: TraceId,
    track: Vec<String>,
    id: u64,
}

/// A transaction panel: the reader's choices, and the record when pinned.
/// An unpinned panel restores empty and waits for the next selection.
#[derive(Serialize, Deserialize)]
struct TransactionPanel {
    id: PanelId,
    kind: String,
    version: u32,
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pinned: Option<SavedRecord>,
    #[serde(default)]
    collapsed: Vec<String>,
    #[serde(default)]
    radix: std::collections::BTreeMap<String, String>,
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

/// The open traces a workspace's letters resolved to.
#[derive(Default)]
struct Traces<'a> {
    open: std::collections::HashMap<TraceId, &'a std::sync::Arc<dyn crate::session::Session>>,
}

impl<'a> Traces<'a> {
    fn insert(
        &mut self,
        letter: TraceId,
        session: &'a std::sync::Arc<dyn crate::session::Session>,
    ) {
        self.open.insert(letter, session);
    }

    fn hierarchy(&self, letter: TraceId) -> Option<&'a Hierarchy> {
        self.open.get(&letter).map(|s| s.hierarchy())
    }

    fn tracks(&self, letter: TraceId) -> &'a [crate::data::transactions::Track] {
        self.open.get(&letter).map_or(&[], |s| s.tracks())
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
                                title: panel.title.get().clone(),
                                trace: p.track.trace(),
                                track: p.track.path().to_vec(),
                                link: p.nav.link,
                                viewport: (!p.nav.link.viewport)
                                    .then(|| p.nav.local_viewport.target()),
                                cursor: (!p.nav.link.cursor)
                                    .then(|| serde_json::value::to_raw_value(&p.nav.local_cursor))
                                    .transpose()?,
                                rows: p.rows.target(),
                                row_cap: (p.row_cap != crate::pipeline::zoom::ROW_PX_CAP)
                                    .then_some(p.row_cap),
                                label_width: p.label_width,
                                clocks: p.nav.clocks().clone(),
                            })?)
                        }
                        PanelKind::Table(table) => {
                            let source = match &table.source {
                                TableSource::Generator(track) => SavedTableSource::Generator {
                                    trace: track.trace(),
                                    path: track.path().to_vec(),
                                },
                                TableSource::Signals(signals) => SavedTableSource::Signals {
                                    signals: signals
                                        .iter()
                                        .map(|signal| SavedTableSignal {
                                            trace: signal.trace,
                                            path: signal.path.clone(),
                                            nth: signal.nth,
                                        })
                                        .collect(),
                                },
                            };
                            let columns = match table.columns.get() {
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
                                title: panel.title.get().clone(),
                                source,
                                columns,
                                link: table.nav.link,
                            })?)
                        }
                        PanelKind::Transaction(model) => {
                            let shown = model.shown();
                            Ok(serde_json::value::to_raw_value(&TransactionPanel {
                                id: panel.id,
                                kind: "transaction".into(),
                                version: 1,
                                title: panel.title.get().clone(),
                                pinned: model
                                    .pinned()
                                    .then(|| {
                                        shown.map(|record| SavedRecord {
                                            trace: record.track.trace(),
                                            track: record.track.path().to_vec(),
                                            id: record.id.0,
                                        })
                                    })
                                    .flatten(),
                                collapsed: model
                                    .prefs()
                                    .collapsed
                                    .iter()
                                    .map(|section| section.key().to_owned())
                                    .collect(),
                                radix: model
                                    .prefs()
                                    .radix
                                    .iter()
                                    .map(|(key, radix)| (key.clone(), radix.name().to_owned()))
                                    .collect(),
                            })?)
                        }
                        PanelKind::Start => Ok(serde_json::value::to_raw_value(&PanelHeader {
                            id: panel.id,
                            kind: "start".into(),
                            version: 1,
                            title: panel.title.get().clone(),
                        })?),
                        _ => unreachable!("settings panels are not saved"),
                    };
                };
                let rows = nest(w.items(), &|row| match row {
                    WaveRow::Signal(item) => {
                        let (signal, nth) = item.source.locator(&app.doc);
                        Row::Signal {
                            trace: item.source.trace(),
                            signal,
                            nth,
                            format: item.format_id(),
                            height: item.height,
                            analog: item.analog.as_ref().map(|a| SavedAnalog {
                                draw: a.draw,
                                range: a.range,
                            }),
                            tint: item.tint,
                        }
                    }
                    WaveRow::Lane(lane) => Row::Lane {
                        trace: lane.source.trace(),
                        generator: lane.source.path().to_vec(),
                        height: lane.height,
                        tint: lane.tint,
                    },
                    WaveRow::Clock(clock) => Row::Clock {
                        trace: clock.key.trace,
                        clock: clock.key.item.clone(),
                        height: clock.height,
                        tint: clock.tint,
                    },
                    WaveRow::Group(_) => unreachable!("nest writes groups"),
                });
                Ok(serde_json::value::to_raw_value(&WavePanel {
                    id: panel.id,
                    kind: "waves".into(),
                    version: WAVES_VERSION,
                    title: panel.title.get().clone(),
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
                    clocks: w.nav.clocks().clone(),
                })?)
            })
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
                    saved.row_cap.is_none_or(|cap| {
                        (crate::pipeline::zoom::ROW_PX_CAP..=crate::pipeline::rows::ROW_PX_MAX)
                            .contains(&cap)
                    }),
                    "invalid pipeline row cap"
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
                let trace = saved.trace;
                let track = match open.tracks(trace).iter().find(|t| t.path == saved.track) {
                    Some(t) => TrackSource::Resolved {
                        track: Traced::new(trace, t.id),
                        path: saved.track,
                    },
                    None => {
                        report.push(format!("Missing pipeline track: {:?}", saved.track));
                        TrackSource::Unresolved {
                            trace,
                            path: saved.track,
                        }
                    }
                };
                let mut p = PipelineModel::new(track, saved.link);
                p.follow = saved.follow;
                p.nav.restore_clocks(saved.clocks);
                if let Some(v) = saved.viewport {
                    valid_viewport(v)?;
                    p.nav.local_viewport.set(v);
                }
                if let Some(c) = saved.cursor {
                    p.nav.local_cursor =
                        serde_json::from_str(c.get()).context("invalid local cursor")?;
                }
                p.rows.set(saved.rows);
                p.row_cap = saved.row_cap.unwrap_or(crate::pipeline::zoom::ROW_PX_CAP);
                p.label_width = saved.label_width;
                panels.push(Panel {
                    id: saved.id,
                    title: crate::history::Journaled::new(saved.title),
                    kind: PanelKind::Pipeline(Box::new(p)),
                });
                continue;
            }
            if header.kind == "table" && header.version == 2 {
                let saved: TablePanel =
                    serde_json::from_str(raw.get()).context("invalid table panel")?;
                let source = match saved.source {
                    SavedTableSource::Generator { trace, path } => {
                        ensure!(!path.is_empty(), "empty table generator path");
                        let found = open
                            .hierarchy(trace)
                            .map_or(Lookup::Missing, |h| h.find_generator(&path));
                        match found {
                            Lookup::Found(generator) => {
                                let h = open.hierarchy(trace).expect("found in it");
                                TableSource::Generator(TrackSource::Resolved {
                                    track: Traced::new(trace, h.generators[generator].track),
                                    path,
                                })
                            }
                            other => {
                                report.push(format!("{other:?} table generator: {path:?}"));
                                TableSource::Generator(TrackSource::Unresolved { trace, path })
                            }
                        }
                    }
                    SavedTableSource::Signals { signals } => {
                        ensure!(!signals.is_empty(), "empty table signal set");
                        let mut restored = Vec::with_capacity(signals.len());
                        for signal in signals {
                            ensure!(!signal.path.is_empty(), "empty table signal path");
                            let trace = signal.trace;
                            let h = open.hierarchy(trace);
                            let found =
                                h.map_or(Lookup::Missing, |h| h.find_var(&signal.path, signal.nth));
                            let (var, reference, name) = match (found, h) {
                                (Lookup::Found(var), Some(h)) => (
                                    Some(var),
                                    Some(Traced::new(trace, h.var(var).signal)),
                                    h.full_name(var),
                                ),
                                (other, _) => {
                                    report
                                        .push(format!("{other:?} table signal: {:?}", signal.path));
                                    (None, None, signal.path.join("."))
                                }
                            };
                            restored.push(SignalSource {
                                trace,
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
                let mut table = TableModel::new(source, saved.link, app.table_memory_budget());
                let mut columns = table.columns.get().clone();
                match &mut columns {
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
                table.columns.restore(columns);
                panels.push(Panel {
                    id: saved.id,
                    title: crate::history::Journaled::new(saved.title),
                    kind: PanelKind::Table(Box::new(table)),
                });
                continue;
            }
            if header.kind == "transaction" && header.version == 1 {
                let saved: TransactionPanel =
                    serde_json::from_str(raw.get()).context("invalid transaction panel")?;
                let mut model = TransactionModel::new(
                    app.table_memory_budget(),
                    app.settings.resolved().transaction.detail_items,
                );
                let mut prefs = ViewPrefs::default();
                for key in &saved.collapsed {
                    let section = SectionKey::parse(key)
                        .ok_or_else(|| anyhow::anyhow!("unknown transaction section {key:?}"))?;
                    prefs.collapsed.insert(section);
                }
                for (key, name) in &saved.radix {
                    let radix = crate::data::text::Radix::parse(name)
                        .ok_or_else(|| anyhow::anyhow!("unknown radix {name:?}"))?;
                    prefs.radix.insert(key.clone(), radix);
                }
                let record = match saved.pinned {
                    Some(pinned) => {
                        ensure!(!pinned.track.is_empty(), "empty transaction track path");
                        let trace = pinned.trace;
                        let found = open.tracks(trace).iter().find(|t| t.path == pinned.track);
                        let track = match found {
                            Some(t) => TrackSource::Resolved {
                                track: Traced::new(trace, t.id),
                                path: pinned.track,
                            },
                            None => {
                                report
                                    .push(format!("Missing transaction track: {:?}", pinned.track));
                                TrackSource::Unresolved {
                                    trace,
                                    path: pinned.track,
                                }
                            }
                        };
                        Some(ShownRecord {
                            track,
                            id: crate::data::transactions::TransactionRef(pinned.id),
                        })
                    }
                    None => None,
                };
                match record {
                    Some(record) => model.restore(record, true, prefs),
                    None => model.restore_prefs(prefs),
                }
                panels.push(Panel {
                    id: saved.id,
                    title: crate::history::Journaled::new(saved.title),
                    kind: PanelKind::Transaction(Box::new(model)),
                });
                continue;
            }
            if header.kind == "start" && header.version == 1 {
                panels.push(Panel {
                    id: header.id,
                    title: crate::history::Journaled::new(header.title),
                    kind: PanelKind::Start,
                });
                continue;
            }
            if header.kind != "waves" || header.version != WAVES_VERSION {
                report.push(format!(
                    "Unsupported panel {} ({} version {})",
                    header.id.0, header.kind, header.version
                ));
                panels.push(Panel {
                    id: header.id,
                    title: crate::history::Journaled::new(header.title),
                    kind: PanelKind::Unsupported(raw),
                });
                continue;
            }
            let mut saved: WavePanel =
                serde_json::from_str(raw.get()).context("invalid waveform panel")?;
            let mut flat = Vec::new();
            flatten(std::mem::take(&mut saved.rows), 0, &mut flat)?;
            row_count += flat.len();
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
                saved.selected.iter().all(|i| *i < flat.len()),
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
            w.nav.restore_clocks(saved.clocks);
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
            let mut items = Vec::with_capacity(flat.len());
            for (depth, row) in flat {
                let (trace, signal, nth, format, height, analog, tint) = match row {
                    Row::Signal {
                        trace,
                        signal,
                        nth,
                        format,
                        height,
                        analog,
                        tint,
                    } => (trace, signal, nth, format, height, analog, tint),
                    Row::Lane {
                        trace,
                        generator,
                        height,
                        tint,
                    } => {
                        ensure!(!generator.is_empty(), "empty lane generator path");
                        let track = open.tracks(trace).iter().find(|t| {
                            t.path == generator && matches!(t.kind, TrackKind::Generator { .. })
                        });
                        let lane = match track {
                            Some(track) => TxLane {
                                height,
                                auto_height: false,
                                ..TxLane::new(&app.doc, Traced::new(trace, track.id))
                                    .context("lane generator vanished")?
                            },
                            None => {
                                report.push(format!("Missing lane generator: {generator:?}"));
                                TxLane::unresolved(trace, generator, height)
                            }
                        };
                        let lane = TxLane { tint, ..lane };
                        items.push(Entry::new(depth, WaveRow::Lane(lane)));
                        continue;
                    }
                    Row::Clock {
                        trace,
                        clock,
                        height,
                        tint,
                    } => {
                        ensure!(!clock.is_empty(), "empty clock path");
                        let key = Traced::new(trace, clock);
                        if app.doc.clocks.find(&key).is_none() {
                            report.push(format!("Missing clock: {}", key.item));
                        }
                        items.push(Entry::new(
                            depth,
                            WaveRow::Clock(crate::wave::model::ClockRow {
                                height,
                                tint,
                                ..crate::wave::model::ClockRow::new(key)
                            }),
                        ));
                        continue;
                    }
                    Row::Group {
                        name,
                        collapsed,
                        height,
                        tint,
                        ..
                    } => {
                        items.push(Entry::new(
                            depth,
                            WaveRow::Group(GroupRow {
                                name,
                                collapsed,
                                height,
                                tint,
                            }),
                        ));
                        continue;
                    }
                };
                let row_signal = signal;
                ensure!(!row_signal.is_empty(), "empty signal path");
                let h = open.hierarchy(trace);
                let found = h.map_or(Lookup::Missing, |h| h.find_var(&row_signal, nth));
                let (source, name, scope, shape) = match (found, h) {
                    (Lookup::Found(var), Some(h)) => {
                        let v = h.var(var);
                        (
                            RowSource::Resolved {
                                trace,
                                var,
                                signal: v.signal,
                            },
                            v.name.to_owned(),
                            h.scope_path(v.scope).join("."),
                            v.shape,
                        )
                    }
                    (found, _) => {
                        let ambiguous = found == Lookup::Ambiguous;
                        report.push(format!(
                            "{} signal: {:?}",
                            if ambiguous { "Ambiguous" } else { "Missing" },
                            row_signal
                        ));
                        let name = row_signal.last().unwrap().clone();
                        let scope = row_signal[..row_signal.len() - 1].join(".");
                        (
                            RowSource::Unresolved {
                                trace,
                                path: row_signal,
                                nth,
                                ambiguous,
                            },
                            name,
                            scope,
                            crate::data::SignalShape::Bit,
                        )
                    }
                };
                let requested = app.doc.translators.get(&format);
                let translator = requested
                    .clone()
                    .unwrap_or_else(|| app.doc.translators.default_for(shape));
                if requested.is_none() {
                    report.push(format!("Unknown translator: {format}"));
                }
                items.push(Entry::new(
                    depth,
                    WaveRow::Signal(DisplayedSignal {
                        source,
                        requested_format: requested.is_none().then_some(format),
                        name,
                        scope,
                        shape,
                        translator,
                        history: None,
                        error: None,
                        height,
                        analog: analog.map(|a| Analog::new(a.draw, a.range)),
                        tint,
                    }),
                ));
            }
            w.restore_rows(items);
            // Selected rows inside folded groups select the group instead.
            w.selected = saved.selected;
            w.anchor = w.selected.first().copied();
            w.fix_hidden_selection();
            panels.push(Panel {
                id: saved.id,
                title: crate::history::Journaled::new(saved.title),
                kind: PanelKind::Waves(Box::new(w)),
            });
        }
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
        for panel in app.panels.iter_mut() {
            if let Some(p) = panel.kind.pipeline_mut()
                && let Err(error) = p.attach(&mut app.doc)
            {
                report.push(format!("Pipeline {}: {error:#}", p.track.path().join(".")));
            }
        }
        let resident = crate::wave::model::Resident::new();
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
            .set_scope(app.doc.traces(), app.scopes.selected_scope());
        for panel in app.panels.iter() {
            if let Some(w) = panel.kind.waves() {
                for row in w.items() {
                    if let Some(signal) = row.signal_ref() {
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

pub mod recent;
pub mod state;
