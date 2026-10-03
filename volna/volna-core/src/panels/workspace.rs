//! Shared panel envelope and the read-only context for content codecs.

use super::{Panel, PanelId, PanelKind};
use crate::trace::TraceId;
use crate::wave::viewport::Viewport;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use volna_trace::data::Hierarchy;

/// The fields every saved panel shares; alone, they describe a start panel.
#[derive(Serialize, Deserialize)]
pub(crate) struct PanelHeader {
    pub(crate) id: PanelId,
    pub(crate) kind: String,
    pub(crate) version: u32,
    pub(crate) title: Option<String>,
}

pub(crate) fn present_raw<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

/// The open traces a workspace's letters resolved to.
#[derive(Default)]
pub(crate) struct Traces<'a> {
    open: std::collections::HashMap<TraceId, &'a std::sync::Arc<dyn volna_trace::session::Session>>,
}

impl<'a> Traces<'a> {
    pub(crate) fn insert(
        &mut self,
        letter: TraceId,
        session: &'a std::sync::Arc<dyn volna_trace::session::Session>,
    ) {
        self.open.insert(letter, session);
    }

    pub(crate) fn hierarchy(&self, letter: TraceId) -> Option<&'a Hierarchy> {
        self.open.get(&letter).map(|s| s.hierarchy())
    }

    pub(crate) fn tracks(&self, letter: TraceId) -> &'a [volna_trace::data::transactions::Track] {
        self.open.get(&letter).map_or(&[], |s| s.tracks())
    }
}

pub(crate) fn valid_viewport(v: Viewport) -> Result<()> {
    ensure!(
        v.start.is_finite()
            && v.end.is_finite()
            && v.start < v.end
            && (v.end - v.start).is_finite(),
        "invalid viewport"
    );
    Ok(())
}

/// Inputs for path resolution; all loads wait until the prepared panels adopt.
pub(crate) struct RestoreContext<'a> {
    pub(crate) doc: &'a crate::Document,
    pub(crate) traces: &'a Traces<'a>,
    pub(crate) budget: volna_trace::remote::memory::MemoryBudget,
    pub(crate) detail_items: usize,
    pub(crate) report: &'a mut crate::workspace::RestoreReport,
    pub(crate) row_count: usize,
}

impl Panel {
    pub(crate) fn save(&self, doc: &crate::Document) -> Result<Box<RawValue>> {
        let id = self.id;
        let title = self.title.get().clone();
        match &self.kind {
            PanelKind::Waves(w) => crate::wave::workspace::save(w, id, title, doc),
            PanelKind::Pipeline(p) => crate::pipeline::workspace::save(p, id, title),
            PanelKind::Table(t) => crate::table::workspace::save(t, id, title),
            PanelKind::Transaction(t) => crate::transaction::workspace::save(t, id, title),
            PanelKind::Unsupported(raw) => Ok(raw.clone()),
            PanelKind::Start => Ok(serde_json::value::to_raw_value(&PanelHeader {
                id,
                kind: "start".into(),
                version: 1,
                title,
            })?),
            PanelKind::Settings => anyhow::bail!("settings panels are not saved"),
        }
    }

    pub(crate) fn restore(raw: Box<RawValue>, ctx: &mut RestoreContext<'_>) -> Result<Self> {
        let header: PanelHeader =
            serde_json::from_str(raw.get()).context("invalid panel header")?;
        match (header.kind.as_str(), header.version) {
            ("waves", crate::wave::workspace::VERSION) => {
                crate::wave::workspace::restore(&raw, ctx)
            }
            ("pipeline", crate::pipeline::workspace::VERSION) => {
                crate::pipeline::workspace::restore(&raw, ctx)
            }
            ("table", crate::table::workspace::VERSION) => {
                crate::table::workspace::restore(&raw, ctx)
            }
            ("transaction", crate::transaction::workspace::VERSION) => {
                crate::transaction::workspace::restore(&raw, ctx)
            }
            ("start", 1) => Ok(Self {
                id: header.id,
                title: crate::history::Journaled::new(header.title),
                kind: PanelKind::Start,
            }),
            _ => {
                ctx.report.push(format!(
                    "Unsupported panel {} ({} version {})",
                    header.id.0, header.kind, header.version
                ));
                Ok(Self {
                    id: header.id,
                    title: crate::history::Journaled::new(header.title),
                    kind: PanelKind::Unsupported(raw),
                })
            }
        }
    }
}
