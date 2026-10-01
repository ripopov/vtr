//! Content lifecycle and journal dispatch. Layout callers describe the panel
//! delta; each kind owns loading, retirement, saved state and property swaps.

use anyhow::{Context, Result};

use super::{Panel, PanelKind};
use crate::document::Document;
use crate::wave::model::Resident;

/// Panel properties carry the owning kind's edit, rather than its fields.
pub(crate) enum Prop {
    Title(Option<String>),
    Clocks {
        rulers: Option<Vec<crate::clock::ClockKey>>,
        origin: Option<u64>,
    },
    Table(crate::table::model::Prop),
    Transaction(crate::transaction::model::Prop),
}

impl Prop {
    pub(crate) fn kind(&self) -> (u8, u8) {
        match self {
            Self::Title(_) => (0, 0),
            Self::Clocks { .. } => (1, 0),
            Self::Table(prop) => (2, prop.kind()),
            Self::Transaction(prop) => (3, prop.kind()),
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::Title(t) => t.as_ref().map_or(0, String::len),
            Self::Clocks { rulers, .. } => rulers.iter().flatten().map(|k| k.item.len() + 24).sum(),
            Self::Table(prop) => prop.bytes(),
            Self::Transaction(prop) => prop.bytes(),
        }
    }
}

impl crate::trace::Retime for Prop {
    fn retime(&mut self, by: crate::trace::Rescale) {
        if let Self::Clocks { origin, .. } = self {
            origin.retime(by);
        }
    }
}

impl PanelKind {
    /// A kind enters the live layout. Repeated adoption does not retain twice.
    pub(crate) fn adopt(&mut self, doc: &mut Document, resident: &Resident) -> Result<()> {
        match self {
            Self::Waves(w) => {
                if w.needs_attach() {
                    w.attach_rows(doc, resident);
                }
                Ok(())
            }
            Self::Pipeline(p) => p.attach(doc),
            Self::Table(t) => {
                let result = t.attach(doc, resident);
                if let Err(error) = &result {
                    t.state = crate::table::TableState::Failed(error.to_string());
                }
                result
            }
            Self::Transaction(t) => t.attach(doc),
            Self::Start | Self::Settings | Self::Unsupported(_) => Ok(()),
        }
    }

    /// A kind leaves the live layout. The journal keeps its description,
    /// without loaded trace objects, builders, gestures or panel reservations.
    pub(crate) fn retire(&mut self, doc: &mut Document) {
        match self {
            Self::Waves(w) => w.detach_rows(),
            Self::Pipeline(p) => p.detach(doc),
            Self::Table(t) => t.park(doc),
            Self::Transaction(t) => t.park(doc),
            Self::Start | Self::Settings | Self::Unsupported(_) => {}
        }
    }

    /// With a label, collect a trace's cockpit removals before closing its
    /// dedicated panels. Without one, forget transient records after those
    /// edits, or when a workspace replaces the whole view without journaling.
    pub(crate) fn forget_trace(
        &mut self,
        doc: &mut Document,
        trace: crate::trace::TraceId,
        label: Option<&str>,
    ) {
        if label.is_some()
            && let Some(nav) = self.nav_mut()
        {
            nav.forget_trace(trace);
        }
        match self {
            Self::Waves(w) => {
                if let Some(label) = label {
                    w.remove_trace_rows(trace, label.to_owned());
                }
            }
            Self::Transaction(t) if label.is_none() || !t.pinned() => {
                t.forget_trace(doc, trace);
            }
            _ => {}
        }
    }
}

impl Panel {
    pub(crate) fn matches_prop(&self, prop: &Prop) -> bool {
        match prop {
            Prop::Title(title) => self.title.get() == title,
            Prop::Clocks { rulers, origin } => self.kind.nav().is_some_and(|nav| {
                nav.clocks().rulers == *rulers && nav.clocks().origin == *origin
            }),
            Prop::Table(prop) => self.kind.table().is_some_and(|t| t.matches_prop(prop)),
            Prop::Transaction(prop) => self
                .kind
                .transaction()
                .is_some_and(|t| t.matches_prop(prop)),
        }
    }

    pub(crate) fn swap_prop(&mut self, doc: &mut Document, prop: Prop) -> Result<Prop> {
        Ok(match prop {
            Prop::Title(title) => Prop::Title(self.title.swap(title)),
            Prop::Clocks { rulers, origin } => {
                let nav = self.kind.nav_mut().context("not a timed panel")?;
                let (rulers, origin) = nav.swap_clocks((rulers, origin));
                Prop::Clocks { rulers, origin }
            }
            Prop::Table(prop) => Prop::Table(
                self.kind
                    .table_mut()
                    .context("not a table")?
                    .swap_prop(prop),
            ),
            Prop::Transaction(prop) => Prop::Transaction(
                self.kind
                    .transaction_mut()
                    .context("not a transaction panel")?
                    .swap_prop(doc, prop),
            ),
        })
    }
}
