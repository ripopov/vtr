//! The wave panel's workspace schema, capture, validation and resolution.
//! Preparing content never retains data or changes the live document.

use super::{
    Tint,
    analog::{Analog, AnalogDraw, AnalogRange},
    lane::TxLane,
    model::{
        DisplayedSignal, GroupRow, GroupStyle, Link, RowHeight, RowSource, WaveModel, WaveRow,
    },
    tree::{self, Entry},
    viewport::Viewport,
};
use crate::clock::ClockView;
use crate::data::{source::Lookup, transactions::TrackKind};
use crate::panels::workspace::valid_viewport;
use crate::panels::{Panel, PanelId, PanelKind, workspace::RestoreContext};
use crate::trace::{TraceId, Traced};
use crate::workspace::MAX_ROWS;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeSet;

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
        #[serde(default, skip_serializing_if = "SavedStyle::is_activity")]
        style: SavedStyle,
        /// A stacked group without its peak band says `false`.
        #[serde(default = "shows_peak", skip_serializing_if = "is_true")]
        peak: bool,
        rows: Vec<Row>,
    },
}

fn shows_peak() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// How a group draws: activity, the default and not written, or `"stack"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedStyle {
    #[default]
    Activity,
    Stack,
}

impl SavedStyle {
    fn is_activity(&self) -> bool {
        *self == Self::Activity
    }
}

impl SavedStyle {
    /// The stored style and peak switch of `style`.
    fn of(style: GroupStyle) -> (Self, bool) {
        match style {
            GroupStyle::Activity => (Self::Activity, true),
            GroupStyle::Stack { peak } => (Self::Stack, peak),
        }
    }

    fn style(self, peak: bool) -> GroupStyle {
        match self {
            Self::Activity => GroupStyle::Activity,
            Self::Stack => GroupStyle::Stack { peak },
        }
    }
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
                style: SavedStyle::of(g.style).0,
                peak: SavedStyle::of(g.style).1,
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
                style,
                peak,
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
                        style,
                        peak,
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
/// (`tint`, a name; an unknown one reads as Default), version 6 stacked
/// groups (`"style": "stack"`, and `"peak": false` without the peak band;
/// activity is not written).
pub(crate) const VERSION: u32 = 6;

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
        deserialize_with = "crate::panels::workspace::present_raw"
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

pub(crate) fn save(
    w: &WaveModel,
    id: PanelId,
    title: Option<String>,
    doc: &crate::Document,
) -> Result<Box<RawValue>> {
    let rows = nest(w.items(), &|row| match row {
        WaveRow::Signal(item) => {
            let (signal, nth) = item.source.locator(doc);
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
        id,
        kind: "waves".into(),
        version: VERSION,
        title,
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
}

pub(crate) fn restore(raw: &RawValue, ctx: &mut RestoreContext<'_>) -> Result<Panel> {
    let mut saved: WavePanel = serde_json::from_str(raw.get()).context("invalid waveform panel")?;
    let mut flat = Vec::new();
    flatten(std::mem::take(&mut saved.rows), 0, &mut flat)?;
    ctx.row_count += flat.len();
    ensure!(ctx.row_count <= MAX_ROWS, "too many workspace rows");
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
        w.nav.local_cursor = serde_json::from_str(c.get()).context("invalid local cursor")?;
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
                let track =
                    ctx.traces.tracks(trace).iter().find(|t| {
                        t.path == generator && matches!(t.kind, TrackKind::Generator { .. })
                    });
                let lane = match track {
                    Some(track) => TxLane {
                        height,
                        auto_height: false,
                        ..TxLane::new(ctx.doc, Traced::new(trace, track.id))
                            .context("lane generator vanished")?
                    },
                    None => {
                        ctx.report
                            .push(format!("Missing lane generator: {generator:?}"));
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
                if ctx.doc.clocks.find(&key).is_none() {
                    ctx.report.push(format!("Missing clock: {}", key.item));
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
                style,
                peak,
                ..
            } => {
                items.push(Entry::new(
                    depth,
                    WaveRow::Group(GroupRow {
                        name,
                        collapsed,
                        height,
                        tint,
                        style: style.style(peak),
                        restore_height: None,
                    }),
                ));
                continue;
            }
        };
        let row_signal = signal;
        ensure!(!row_signal.is_empty(), "empty signal path");
        let h = ctx.traces.hierarchy(trace);
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
                ctx.report.push(format!(
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
        let requested = ctx.doc.translators.get(&format);
        let translator = requested
            .clone()
            .unwrap_or_else(|| ctx.doc.translators.default_for(shape));
        if requested.is_none() {
            ctx.report.push(format!("Unknown translator: {format}"));
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
    Ok(Panel {
        id: saved.id,
        title: crate::history::Journaled::new(saved.title),
        kind: PanelKind::Waves(Box::new(w)),
    })
}
