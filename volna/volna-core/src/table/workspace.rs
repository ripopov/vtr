//! The table panel's workspace schema, capture, validation and resolution.
//! Preparing content never retains data or changes the live document.

use super::{
    SignalSource, TableModel, TableSource,
    columns::{ColumnSet, TransactionColumn},
};
use crate::panels::{Panel, PanelId, PanelKind, workspace::RestoreContext};
use crate::pipeline::TrackSource;
use crate::trace::{TraceId, Traced};
use crate::wave::model::Link;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use volna_trace::data::source::Lookup;

pub(crate) const VERSION: u32 = 2;

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

pub(crate) fn save(
    table: &TableModel,
    id: PanelId,
    title: Option<String>,
) -> Result<Box<RawValue>> {
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
        id,
        kind: "table".into(),
        version: VERSION,
        title,
        source,
        columns,
        link: table.nav.link,
    })?)
}

pub(crate) fn restore(raw: &RawValue, ctx: &mut RestoreContext<'_>) -> Result<Panel> {
    let saved: TablePanel = serde_json::from_str(raw.get()).context("invalid table panel")?;
    let source = match saved.source {
        SavedTableSource::Generator { trace, path } => {
            ensure!(!path.is_empty(), "empty table generator path");
            let found = ctx
                .traces
                .hierarchy(trace)
                .map_or(Lookup::Missing, |h| h.find_generator(&path));
            match found {
                Lookup::Found(generator) => {
                    let h = ctx.traces.hierarchy(trace).expect("found in it");
                    TableSource::Generator(TrackSource::Resolved {
                        track: Traced::new(trace, h.generators()[generator].track),
                        path,
                    })
                }
                other => {
                    ctx.report
                        .push(format!("{other:?} table generator: {path:?}"));
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
                let h = ctx.traces.hierarchy(trace);
                let found = h.map_or(Lookup::Missing, |h| h.find_var(&signal.path, signal.nth));
                let (var, reference, name) = match (found, h) {
                    (Lookup::Found(var), Some(h)) => (
                        Some(var),
                        Some(Traced::new(trace, h.var(var).signal)),
                        h.full_name(var),
                    ),
                    (other, _) => {
                        ctx.report
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
    let mut table = TableModel::new(source, saved.link, ctx.budget.clone());
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
    Ok(Panel {
        id: saved.id,
        title: crate::history::Journaled::new(saved.title),
        kind: PanelKind::Table(Box::new(table)),
    })
}
