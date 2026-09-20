//! Durable declaration references captured when a table is opened.

use crate::data::source::Lookup;
use crate::data::transactions::{TrackKind, TrackRef};
use crate::data::{Hierarchy, SignalRef, VarId};
use crate::pipeline::TrackSource;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalSource {
    pub path: Vec<String>,
    pub nth: Option<usize>,
    pub var: Option<VarId>,
    pub signal: Option<SignalRef>,
    pub name: String,
}

impl SignalSource {
    pub fn resolved(hierarchy: &Hierarchy, var: VarId) -> anyhow::Result<Self> {
        let declaration = hierarchy
            .vars
            .get(var)
            .ok_or_else(|| anyhow::anyhow!("selected signal is missing"))?;
        let (path, nth) = hierarchy.var_path(var);
        Ok(Self {
            name: hierarchy.full_name(var),
            path,
            nth,
            var: Some(var),
            signal: Some(declaration.signal),
        })
    }

    pub fn resolve(&mut self, hierarchy: &Hierarchy) -> bool {
        match hierarchy.find_var(&self.path, self.nth) {
            Lookup::Found(var) => {
                self.var = Some(var);
                self.signal = Some(hierarchy.vars[var].signal);
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
    pub fn generator(
        hierarchy: &Hierarchy,
        tracks: &[crate::data::transactions::Track],
        track: TrackRef,
    ) -> anyhow::Result<Self> {
        let declaration = tracks
            .iter()
            .find(|candidate| candidate.id == track)
            .ok_or_else(|| anyhow::anyhow!("selected transaction source is missing"))?;
        anyhow::ensure!(
            matches!(declaration.kind, TrackKind::Generator { .. }),
            "Choose one generator, or only signals."
        );
        let member = hierarchy
            .generators
            .iter()
            .find(|g| g.track == track)
            .ok_or_else(|| anyhow::anyhow!("selected generator declaration is missing"))?;
        let mut path = hierarchy
            .scope_path(member.stream)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        path.push(member.name.clone());
        Ok(Self::Generator(TrackSource::Resolved { track, path }))
    }

    pub fn signals(hierarchy: &Hierarchy, vars: &[VarId]) -> anyhow::Result<Self> {
        anyhow::ensure!(!vars.is_empty(), "Choose one generator, or only signals.");
        let mut sources = Vec::with_capacity(vars.len());
        for &var in vars {
            if !sources.iter().any(|s: &SignalSource| s.var == Some(var)) {
                sources.push(SignalSource::resolved(hierarchy, var)?);
            }
        }
        Ok(Self::Signals(sources))
    }
}
