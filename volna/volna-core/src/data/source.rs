//! Trace metadata and hierarchy model shared by every session.

use super::value::SignalShape;

pub type ScopeId = usize;
pub type VarId = usize;

/// Opaque handle to a signal inside a source (VTR `SignalId`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SignalRef(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    None,
    Input,
    Output,
    InOut,
}

#[derive(Clone, Debug)]
pub struct Scope {
    pub name: String,
    /// Scope kind name (`module`, `task`, ...), used for the icon.
    pub kind: String,
    pub parent: Option<ScopeId>,
    pub children: Vec<ScopeId>,
    pub vars: Vec<VarId>,
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub scope: ScopeId,
    pub shape: SignalShape,
    /// Variable type name (`wire`, `reg`, ...).
    pub var_type: String,
    pub direction: Direction,
    pub signal: SignalRef,
}

#[derive(Clone, Debug, Default)]
pub struct Hierarchy {
    pub scopes: Vec<Scope>,
    pub roots: Vec<ScopeId>,
    pub vars: Vec<Variable>,
}

impl Hierarchy {
    pub fn scope_path(&self, mut id: ScopeId) -> Vec<&str> {
        let mut parts = vec![self.scopes[id].name.as_str()];
        while let Some(p) = self.scopes[id].parent {
            parts.push(self.scopes[p].name.as_str());
            id = p;
        }
        parts.reverse();
        parts
    }

    pub fn full_name(&self, var: VarId) -> String {
        let v = &self.vars[var];
        let mut s = self.scope_path(v.scope).join(".");
        if !s.is_empty() {
            s.push('.');
        }
        s.push_str(&v.name);
        s
    }
}

#[derive(Clone, Debug)]
pub struct TraceInfo {
    /// Display name (file name).
    pub name: String,
    /// `10^timescale` seconds per time unit.
    pub timescale: i8,
    pub time_range: (u64, u64),
    pub signal_count: usize,
    /// Total value changes when cheaply known.
    pub change_count: Option<u64>,
}
