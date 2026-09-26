//! Trace metadata and hierarchy model shared by every session.

use super::transactions::{Attributes, TrackRef};
use super::value::SignalShape;

pub type ScopeId = usize;
pub type VarId = usize;
pub type GeneratorId = usize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScopeRole {
    #[default]
    Scope,
    Stream {
        track: TrackRef,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Member {
    Var(VarId),
    Generator(GeneratorId),
    Stream(ScopeId),
}

impl Member {
    pub fn var(self) -> Option<VarId> {
        if let Self::Var(id) = self {
            Some(id)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Generator {
    pub name: String,
    pub stream: ScopeId,
    pub track: TrackRef,
    /// Raw declaration attributes, including log-site provenance.
    pub attributes: Attributes,
}

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

impl From<vtr::Direction> for Direction {
    fn from(direction: vtr::Direction) -> Self {
        match direction {
            vtr::Direction::Input => Self::Input,
            vtr::Direction::Output => Self::Output,
            vtr::Direction::InOut => Self::InOut,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Scope {
    pub name: String,
    /// Raw scope type name or producer-defined stream kind.
    pub kind: String,
    pub parent: Option<ScopeId>,
    pub children: Vec<ScopeId>,
    pub vars: Vec<VarId>,
    pub role: ScopeRole,
    pub component: String,
    pub generators: Vec<GeneratorId>,
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
    /// Source-local enum table identity; tables are type metadata, not members.
    pub enum_table: Option<u32>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Hierarchy {
    pub scopes: Vec<Scope>,
    pub roots: Vec<ScopeId>,
    pub vars: Vec<Variable>,
    pub generators: Vec<Generator>,
}

impl Hierarchy {
    /// Append a scope and link it under `parent` or among the roots.
    pub fn push_scope(&mut self, name: String, kind: String, parent: Option<ScopeId>) -> ScopeId {
        let id = self.scopes.len();
        self.scopes.push(Scope {
            name,
            kind,
            parent,
            children: Vec::new(),
            vars: Vec::new(),
            role: ScopeRole::Scope,
            component: String::new(),
            generators: Vec::new(),
        });
        match parent {
            Some(p) => self.scopes[p].children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    pub fn find_generator(&self, path: &[impl AsRef<str>]) -> Lookup<GeneratorId> {
        let Some((name, stream)) = path.split_last() else {
            return Lookup::Missing;
        };
        match self.find_scope(stream) {
            Lookup::Found(id) => unique(
                self.scopes[id]
                    .generators
                    .iter()
                    .copied()
                    .filter(|&g| self.generators[g].name == name.as_ref()),
            ),
            Lookup::Missing => Lookup::Missing,
            Lookup::Ambiguous => Lookup::Ambiguous,
        }
    }

    pub fn member_name(&self, member: Member) -> &str {
        match member {
            Member::Var(id) => &self.vars[id].name,
            Member::Generator(id) => &self.generators[id].name,
            Member::Stream(id) => &self.scopes[id].name,
        }
    }

    pub fn member_path(&self, member: Member) -> String {
        match member {
            Member::Var(id) => self.full_name(id),
            Member::Generator(id) => {
                let g = &self.generators[id];
                format!("{}.{}", self.scope_path(g.stream).join("."), g.name)
            }
            Member::Stream(id) => self.scope_path(id).join("."),
        }
    }

    pub fn member_track(&self, member: Member) -> Option<TrackRef> {
        match member {
            Member::Generator(id) => Some(self.generators.get(id)?.track),
            Member::Stream(id) => match self.scopes.get(id)?.role {
                ScopeRole::Stream { track } => Some(track),
                ScopeRole::Scope => None,
            },
            Member::Var(_) => None,
        }
    }

    pub fn is_log(&self, member: Member) -> bool {
        let scope = match member {
            Member::Generator(id) => &self.scopes[self.generators[id].stream],
            Member::Stream(id) => &self.scopes[id],
            Member::Var(_) => return false,
        };
        matches!(scope.role, ScopeRole::Stream { .. }) && scope.kind == "LOG"
    }

    /// Resolve literal path segments, including dots and escaped HDL names.
    /// Every scope along the path must be unambiguous.
    /// Whether `scope` or a scope below it declares a variable.
    pub fn has_vars(&self, scope: ScopeId) -> bool {
        self.scopes
            .get(scope)
            .is_some_and(|s| !s.vars.is_empty() || s.children.iter().any(|&c| self.has_vars(c)))
    }

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
    /// A producer-named time unit (`time.unit`, e.g. `cycle`) shown instead
    /// of SI scaling of `timescale`.
    pub time_unit: Option<String>,
}
