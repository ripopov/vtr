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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Member {
    Var(VarId),
    Generator(GeneratorId),
    Stream(ScopeId),
}

impl Member {
    /// Variable identity, if this member is a variable.
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
pub struct HierarchyBuilder {
    pub scopes: Vec<Scope>,
    pub roots: Vec<ScopeId>,
    pub vars: Vec<Variable>,
    pub generators: Vec<Generator>,
}

impl HierarchyBuilder {
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

/// Borrowed scope metadata. Names and membership stay in the immutable owner.
#[derive(Clone, Copy, Debug)]
pub struct ScopeView<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub component: &'a str,
    pub parent: Option<ScopeId>,
    pub children: Ids<'a>,
    pub vars: Ids<'a>,
    pub role: ScopeRole,
    pub generators: Ids<'a>,
}

/// Borrowed variable metadata; querying a declaration allocates nothing.
#[derive(Clone, Copy, Debug)]
pub struct VariableView<'a> {
    pub name: &'a str,
    pub scope: ScopeId,
    pub shape: SignalShape,
    pub var_type: &'a str,
    pub direction: Direction,
    pub signal: SignalRef,
    pub enum_table: Option<u32>,
}

/// A borrowed range of declaration identities. Iteration allocates nothing.
#[derive(Clone, Copy, Debug)]
pub struct Ids<'a> {
    storage: IdStorage<'a>,
    gap: Option<usize>,
}
#[derive(Clone, Copy, Debug)]
enum IdStorage<'a> {
    Slice(&'a [u32]),
    Reader(vtr::hierarchy_index::IndexedNodes<'a>),
}
impl Default for Ids<'_> {
    fn default() -> Self {
        Self::from_slice(&[])
    }
}
impl<'a> Ids<'a> {
    /// Borrow an existing column of declaration identities.
    pub fn from_slice(ids: &'a [u32]) -> Self {
        Self {
            storage: IdStorage::Slice(ids),
            gap: None,
        }
    }
    pub(crate) fn reader(ids: vtr::hierarchy_index::IndexedNodes<'a>) -> Self {
        Self {
            storage: IdStorage::Reader(ids),
            gap: None,
        }
    }
    pub(crate) fn shifted(mut self, gap: Option<usize>) -> Self {
        self.gap = gap;
        self
    }
    /// Number of members; reader-backed ranges scan the shared child column.
    pub fn len(self) -> usize {
        match self.storage {
            IdStorage::Slice(ids) => ids.len(),
            IdStorage::Reader(ids) => ids.len(),
        }
    }
    /// Whether this range has any selected member.
    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }
    /// Iterate in declaration order without allocating.
    pub fn iter(self) -> IdsIter<'a> {
        IdsIter {
            storage: match self.storage {
                IdStorage::Slice(ids) => IterStorage::Slice(ids.iter()),
                IdStorage::Reader(ids) => IterStorage::Reader(ids.iter()),
            },
            gap: self.gap,
        }
    }
    /// First selected identity, if present.
    pub fn first(self) -> Option<usize> {
        self.iter().next()
    }
}
impl<'a> IntoIterator for Ids<'a> {
    type Item = usize;
    type IntoIter = IdsIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
/// Allocation-free cursor over an immutable membership range.
pub struct IdsIter<'a> {
    storage: IterStorage<'a>,
    gap: Option<usize>,
}
enum IterStorage<'a> {
    Slice(std::slice::Iter<'a, u32>),
    Reader(vtr::hierarchy_index::IndexedIter<'a>),
}
impl Iterator for IdsIter<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        let id = match &mut self.storage {
            IterStorage::Slice(ids) => ids.next().map(|&id| id as usize),
            IterStorage::Reader(ids) => ids.next(),
        }?;
        Some(id + usize::from(self.gap.is_some_and(|gap| id >= gap)))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.storage {
            IterStorage::Slice(ids) => ids.size_hint(),
            IterStorage::Reader(ids) => ids.size_hint(),
        }
    }
}
impl DoubleEndedIterator for IdsIter<'_> {
    fn next_back(&mut self) -> Option<usize> {
        let id = match &mut self.storage {
            IterStorage::Slice(ids) => ids.next_back().map(|&id| id as usize),
            IterStorage::Reader(ids) => ids.next_back(),
        }?;
        Some(id + usize::from(self.gap.is_some_and(|gap| id >= gap)))
    }
}

/// Resident hierarchy queries shared by local and remote sessions. Implementors
/// return borrowed views; no caller owns strings or member vectors per node.
pub(crate) trait HierarchySource: Send + Sync + std::fmt::Debug {
    fn scope_count(&self) -> usize;
    fn var_count(&self) -> usize;
    fn roots(&self) -> Ids<'_>;
    fn scope(&self, id: ScopeId) -> ScopeView<'_>;
    fn var(&self, id: VarId) -> VariableView<'_>;
    fn resident_bytes(&self) -> u64;
    /// Signal identity without decoding the rest of a variable.
    fn signal(&self, id: VarId) -> SignalRef {
        self.var(id).signal
    }
    /// A variable's name without decoding the rest of it.
    fn var_name(&self, id: VarId) -> &str {
        self.var(id).name
    }
}

/// Immutable hierarchy backed by shared columns or a reader's own storage.
#[derive(Clone, Debug)]
pub struct Hierarchy {
    pub(crate) source: std::sync::Arc<dyn HierarchySource>,
    /// Shared raw generator declarations and their attributes.
    pub(crate) generators: std::sync::Arc<Vec<Generator>>,
}
impl Default for Hierarchy {
    fn default() -> Self {
        HierarchyBuilder::default().finish()
    }
}
impl Hierarchy {
    /// Borrow raw generator declarations; their storage follows the hierarchy owner.
    pub fn generators(&self) -> &[Generator] {
        &self.generators
    }
    /// Number of scope and stream declarations.
    pub fn scope_count(&self) -> usize {
        self.source.scope_count()
    }
    /// Number of variable declarations, aliases included.
    pub fn var_count(&self) -> usize {
        self.source.var_count()
    }
    /// Raw signal identity of a variable; panics for an invalid variable ID.
    pub fn signal(&self, id: VarId) -> SignalRef {
        self.source.signal(id)
    }
    /// Root scope identities in declaration order.
    pub fn roots(&self) -> Ids<'_> {
        self.source.roots()
    }
    /// Borrow a scope by its original identity; panics out of range.
    pub fn scope(&self, id: ScopeId) -> ScopeView<'_> {
        self.source.scope(id)
    }
    /// Borrow a variable by its original identity; panics out of range.
    pub fn var(&self, id: VarId) -> VariableView<'_> {
        self.source.var(id)
    }
    /// Borrow a scope, returning `None` for an invalid identity.
    pub fn get_scope(&self, id: ScopeId) -> Option<ScopeView<'_>> {
        (id < self.scope_count()).then(|| self.scope(id))
    }
    /// Borrow a variable, returning `None` for an invalid identity.
    pub fn get_var(&self, id: VarId) -> Option<VariableView<'_>> {
        (id < self.var_count()).then(|| self.var(id))
    }
    /// Borrow every scope in declaration order.
    pub fn scopes(&self) -> impl ExactSizeIterator<Item = ScopeView<'_>> {
        (0..self.scope_count()).map(|id| self.scope(id))
    }
    /// Borrow every variable in declaration order.
    pub fn vars(&self) -> impl ExactSizeIterator<Item = VariableView<'_>> {
        (0..self.var_count()).map(|id| self.var(id))
    }
    /// Resident column storage, excluding shared reader storage for a VTR view.
    pub fn resident_bytes(&self) -> u64 {
        self.source.resident_bytes()
    }
    /// Explicit owned copy for fixture construction or transformations.
    pub fn to_builder(&self) -> HierarchyBuilder {
        HierarchyBuilder {
            scopes: self
                .scopes()
                .map(|s| Scope {
                    name: s.name.into(),
                    kind: s.kind.into(),
                    component: s.component.into(),
                    parent: s.parent,
                    children: s.children.iter().collect(),
                    vars: s.vars.iter().collect(),
                    role: s.role,
                    generators: s.generators.iter().collect(),
                })
                .collect(),
            vars: self
                .vars()
                .map(|v| Variable {
                    name: v.name.into(),
                    scope: v.scope,
                    shape: v.shape,
                    var_type: v.var_type.into(),
                    direction: v.direction,
                    signal: v.signal,
                    enum_table: v.enum_table,
                })
                .collect(),
            roots: self.roots().iter().collect(),
            generators: self.generators.as_ref().clone(),
        }
    }
    pub fn find_generator(&self, path: &[impl AsRef<str>]) -> Lookup<GeneratorId> {
        let Some((name, stream)) = path.split_last() else {
            return Lookup::Missing;
        };
        match self.find_scope(stream) {
            Lookup::Found(id) => unique(
                self.scope(id)
                    .generators
                    .iter()
                    .filter(|&g| self.generators[g].name == name.as_ref()),
            ),
            Lookup::Missing => Lookup::Missing,
            Lookup::Ambiguous => Lookup::Ambiguous,
        }
    }

    pub fn member_name(&self, member: Member) -> &str {
        match member {
            Member::Var(id) => self.var(id).name,
            Member::Generator(id) => &self.generators[id].name,
            Member::Stream(id) => self.scope(id).name,
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
            Member::Stream(id) => match self.get_scope(id)?.role {
                ScopeRole::Stream { track } => Some(track),
                ScopeRole::Scope => None,
            },
            Member::Var(_) => None,
        }
    }

    pub fn is_log(&self, member: Member) -> bool {
        let scope = match member {
            Member::Generator(id) => self.scope(self.generators[id].stream),
            Member::Stream(id) => self.scope(id),
            Member::Var(_) => return false,
        };
        matches!(scope.role, ScopeRole::Stream { .. }) && scope.kind == "LOG"
    }

    /// Resolve literal path segments, including dots and escaped HDL names.
    /// Every scope along the path must be unambiguous.
    /// Whether `scope` or a scope below it declares a variable.
    pub fn has_vars(&self, scope: ScopeId) -> bool {
        if self.get_scope(scope).is_none() {
            return false;
        }
        let mut pending = vec![scope];
        while let Some(id) = pending.pop() {
            let scope = self.scope(id);
            if !scope.vars.is_empty() {
                return true;
            }
            pending.extend(scope.children.iter());
        }
        false
    }

    pub fn find_scope(&self, path: &[impl AsRef<str>]) -> Lookup<ScopeId> {
        let mut children = self.roots();
        let mut result = Lookup::Missing;
        for name in path {
            result = unique(
                children
                    .iter()
                    .filter(|&id| self.scope(id).name == name.as_ref()),
            );
            match result {
                Lookup::Found(id) => children = self.scope(id).children,
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
        let mut matches = self
            .scope(scope)
            .vars
            .iter()
            .filter(|&id| self.source.var_name(id) == name.as_ref());
        match nth {
            Some(n) => matches.nth(n).map_or(Lookup::Missing, Lookup::Found),
            None => unique(matches),
        }
    }

    /// Durable locator for a declaration. Include an occurrence only when
    /// sibling declarations have the same name; signal aliases stay distinct.
    pub fn var_path(&self, var: VarId) -> (Vec<String>, Option<usize>) {
        let v = self.var(var);
        let mut path: Vec<_> = self
            .scope_path(v.scope)
            .into_iter()
            .map(str::to_owned)
            .collect();
        path.push(v.name.to_owned());
        // One pass over the scope, comparing names only.
        let (mut before, mut total, mut seen) = (0, 0, false);
        for id in self.scope(v.scope).vars.iter() {
            seen |= id == var;
            if self.source.var_name(id) == v.name {
                total += 1;
                before += usize::from(!seen);
            }
        }
        let nth = (total > 1).then_some(before);
        (path, nth)
    }

    pub fn scope_path(&self, mut id: ScopeId) -> Vec<&str> {
        let mut parts = vec![self.scope(id).name];
        while let Some(p) = self.scope(id).parent {
            parts.push(self.scope(p).name);
            id = p;
        }
        parts.reverse();
        parts
    }

    pub fn full_name(&self, var: VarId) -> String {
        let v = self.var(var);
        let mut s = self.scope_path(v.scope).join(".");
        if !s.is_empty() {
            s.push('.');
        }
        s.push_str(v.name);
        s
    }
}
