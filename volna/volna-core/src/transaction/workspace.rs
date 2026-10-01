//! The transaction panel's workspace schema, capture, validation and resolution.
//! Preparing content never retains data or changes the live document.

use super::{ShownRecord, TransactionModel, ViewPrefs, view::SectionKey};
use crate::panels::{Panel, PanelId, PanelKind, workspace::RestoreContext};
use crate::pipeline::TrackSource;
use crate::trace::{TraceId, Traced};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub(crate) const VERSION: u32 = 1;

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

pub(crate) fn save(
    model: &TransactionModel,
    id: PanelId,
    title: Option<String>,
) -> Result<Box<RawValue>> {
    let shown = model.shown();
    Ok(serde_json::value::to_raw_value(&TransactionPanel {
        id,
        kind: "transaction".into(),
        version: VERSION,
        title,
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

pub(crate) fn restore(raw: &RawValue, ctx: &mut RestoreContext<'_>) -> Result<Panel> {
    let saved: TransactionPanel =
        serde_json::from_str(raw.get()).context("invalid transaction panel")?;
    let mut model = TransactionModel::new(ctx.budget.clone(), ctx.detail_items);
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
            let found = ctx
                .traces
                .tracks(trace)
                .iter()
                .find(|t| t.path == pinned.track);
            let track = match found {
                Some(t) => TrackSource::Resolved {
                    track: Traced::new(trace, t.id),
                    path: pinned.track,
                },
                None => {
                    ctx.report
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
    Ok(Panel {
        id: saved.id,
        title: crate::history::Journaled::new(saved.title),
        kind: PanelKind::Transaction(Box::new(model)),
    })
}
