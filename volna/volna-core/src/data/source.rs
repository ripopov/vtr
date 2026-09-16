//! Trace metadata and hierarchy model shared by every session.

use super::value::SignalShape;

pub type ScopeId = usize;
pub type VarId = usize;

/// A durable name must resolve uniquely; ambiguous names never select a
/// declaration by accident. Runtime IDs remain local to one hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<T> {
    Found(T),
    Missing,
    Ambiguous,
}

fn unique<T>(mut matches: impl Iterator<Item = T>) -> Lookup<T> {
    match (matches.next(), matches.next()) {
        (None, _) => Lookup::Missing,
        (Some(id), None) => Lookup::Found(id),
        _ => Lookup::Ambiguous,
    }
}

/// Opaque handle to a signal inside a source (VTR `SignalId`).
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct SignalRef(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    None,
    Input,
    Output,
    InOut,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Scope {
    pub name: String,
    /// Scope kind name (`module`, `task`, ...), used for the icon.
    pub kind: String,
    pub parent: Option<ScopeId>,
    pub children: Vec<ScopeId>,
    pub vars: Vec<VarId>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Variable {
    pub name: String,
    pub scope: ScopeId,
    pub shape: SignalShape,
    /// Variable type name (`wire`, `reg`, ...).
    pub var_type: String,
    pub direction: Direction,
    pub signal: SignalRef,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Hierarchy {
    pub scopes: Vec<Scope>,
    pub roots: Vec<ScopeId>,
    pub vars: Vec<Variable>,
}

impl Hierarchy {
    /// Resolve literal path segments, including dots and escaped HDL names.
    /// Every scope along the path must be unambiguous.
    pub fn find_scope(&self, path: &[impl AsRef<str>]) -> Lookup<ScopeId> {
        let mut children = &self.roots;
        let mut result = Lookup::Missing;
        for name in path {
            result = unique(
                children
                    .iter()
                    .copied()
                    .filter(|&id| self.scopes[id].name == name.as_ref()),
            );
            match result {
                Lookup::Found(id) => children = &self.scopes[id].children,
                _ => return result,
            }
        }
        result
    }

    /// Resolve a variable, optionally choosing the zero-based occurrence of
    /// its name in declaration order. `nth` never disambiguates a scope.
    pub fn find_var(&self, path: &[impl AsRef<str>], nth: Option<usize>) -> Lookup<VarId> {
        let Some((name, scope)) = path.split_last() else {
            return Lookup::Missing;
        };
        let scope = match self.find_scope(scope) {
            Lookup::Found(id) => id,
            Lookup::Missing => return Lookup::Missing,
            Lookup::Ambiguous => return Lookup::Ambiguous,
        };
        let mut matches = self.scopes[scope]
            .vars
            .iter()
            .copied()
            .filter(|&id| self.vars[id].name == name.as_ref());
        match nth {
            Some(n) => matches.nth(n).map_or(Lookup::Missing, Lookup::Found),
            None => unique(matches),
        }
    }

    /// Durable locator for a declaration. Include an occurrence only when
    /// sibling declarations have the same name; signal aliases stay distinct.
    pub fn var_path(&self, var: VarId) -> (Vec<String>, Option<usize>) {
        let v = &self.vars[var];
        let mut path: Vec<_> = self
            .scope_path(v.scope)
            .into_iter()
            .map(str::to_owned)
            .collect();
        path.push(v.name.clone());
        let matches: Vec<_> = self.scopes[v.scope]
            .vars
            .iter()
            .copied()
            .filter(|&id| self.vars[id].name == v.name)
            .collect();
        let nth = (matches.len() > 1).then(|| matches.iter().position(|&id| id == var).unwrap());
        (path, nth)
    }

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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TraceInfo {
    /// Display name (file name).
    pub name: String,
    /// Optional build identity attached to the trace, without VDB presentation data.
    pub design_id: Option<String>,
    /// `10^timescale` seconds per time unit.
    pub timescale: i8,
    pub time_range: (u64, u64),
    pub signal_count: usize,
    /// Total value changes when cheaply known.
    pub change_count: Option<u64>,
}
