//! Durable declaration references captured when a table is opened.

use crate::pipeline::TrackSource;
use crate::trace::{TraceId, Traced};
use volna_trace::data::source::Lookup;
use volna_trace::data::transactions::{TrackKind, TrackRef};
use volna_trace::data::{Hierarchy, SignalRef, VarId};

/// A column of a signal table: a variable of one trace, by its path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSource {
    pub trace: TraceId,
    pub path: Vec<String>,
    pub nth: Option<usize>,
    pub var: Option<VarId>,
    pub signal: Option<Traced<SignalRef>>,
    pub name: String,
}

impl SignalSource {
    pub fn resolved(hierarchy: &Hierarchy, var: Traced<VarId>) -> anyhow::Result<Self> {
        let declaration = hierarchy
            .get_var(var.item)
            .ok_or_else(|| anyhow::anyhow!("selected signal is missing"))?;
        let (path, nth) = hierarchy.var_path(var.item);
        Ok(Self {
            trace: var.trace,
            name: hierarchy.full_name(var.item),
            path,
            nth,
            var: Some(var.item),
            signal: Some(var.with(declaration.signal)),
        })
    }

    /// Find the variable again in its trace's `hierarchy`.
    pub fn resolve(&mut self, hierarchy: &Hierarchy) -> bool {
        match hierarchy.find_var(&self.path, self.nth) {
            Lookup::Found(var) => {
                self.var = Some(var);
                self.signal = Some(Traced::new(self.trace, hierarchy.var(var).signal));
                self.name = hierarchy.full_name(var);
                true
            }
            _ => {
                self.var = None;
                self.signal = None;
                false
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableSource {
    Generator(TrackSource),
    Signals(Vec<SignalSource>),
}

impl TableSource {
    /// The traces the table's data comes from.
    pub fn traces(&self) -> impl Iterator<Item = TraceId> + '_ {
        let (generator, signals) = match self {
            Self::Generator(source) => (Some(source.trace()), &[][..]),
            Self::Signals(sources) => (None, sources.as_slice()),
        };
        generator.into_iter().chain(signals.iter().map(|s| s.trace))
    }

    /// The generator a record table shows, in its trace.
    pub fn generator_track(&self) -> Option<Traced<TrackRef>> {
        match self {
            Self::Generator(source) => source.track(),
            Self::Signals(_) => None,
        }
    }

    pub fn generator(
        hierarchy: &Hierarchy,
        tracks: &[volna_trace::data::transactions::Track],
        track: Traced<TrackRef>,
    ) -> anyhow::Result<Self> {
        let declaration = tracks
            .iter()
            .find(|candidate| candidate.id == track.item)
            .ok_or_else(|| anyhow::anyhow!("selected transaction source is missing"))?;
        anyhow::ensure!(
            matches!(declaration.kind, TrackKind::Generator { .. }),
            "Choose one generator, or only signals."
        );
        let member = hierarchy
            .generators()
            .iter()
            .find(|g| g.track == track.item)
            .ok_or_else(|| anyhow::anyhow!("selected generator declaration is missing"))?;
        let mut path = hierarchy
            .scope_path(member.stream)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        path.push(member.name.clone());
        Ok(Self::Generator(TrackSource::Resolved { track, path }))
    }

    /// A table of variables, from any open traces.
    pub fn signals(doc: &crate::Document, vars: &[Traced<VarId>]) -> anyhow::Result<Self> {
        anyhow::ensure!(!vars.is_empty(), "Choose one generator, or only signals.");
        let mut sources = Vec::with_capacity(vars.len());
        for &var in vars {
            if !sources
                .iter()
                .any(|s: &SignalSource| (s.trace, s.var) == (var.trace, Some(var.item)))
            {
                let hierarchy = doc
                    .hierarchy(var.trace)
                    .ok_or_else(|| anyhow::anyhow!("trace {} is not open", var.trace))?;
                sources.push(SignalSource::resolved(hierarchy, var)?);
            }
        }
        Ok(Self::Signals(sources))
    }
}
