//! Borrowed browser metadata over resident or query-backed storage. The view
//! does not copy the hierarchy or own reader/transport state.
use super::{
    Direction, Hierarchy, ScopeId, SignalRef, SignalShape, VarId, query_hierarchy::QueryHierarchy,
};
use crate::sidebar::{scopes::ScopeHierarchy, variables::VariableHierarchy};
use vtr_query::metadata::DeclarationData;

#[derive(Clone, Copy)]
pub enum BrowserHierarchy<'a> {
    Resident(&'a Hierarchy),
    Paged(&'a QueryHierarchy),
}
pub struct BrowserScope<'a> {
    pub name: Option<&'a str>,
    pub kind: &'a str,
    pub parent: Option<ScopeId>,
}
pub struct BrowserVariable<'a> {
    pub name: Option<&'a str>,
    pub scope: Option<ScopeId>,
    pub shape: SignalShape,
    pub direction: Direction,
    pub signal: SignalRef,
}
impl<'a> BrowserHierarchy<'a> {
    fn resident(&self) -> Option<&'a Hierarchy> {
        if let Self::Resident(h) = self {
            Some(h)
        } else {
            None
        }
    }
    fn paged(&self) -> Option<&'a QueryHierarchy> {
        if let Self::Paged(h) = self {
            Some(h)
        } else {
            None
        }
    }
    pub fn scope(&self, id: ScopeId) -> Option<BrowserScope<'a>> {
        match self {
            Self::Resident(h) => h.scopes.get(id).map(|scope| BrowserScope {
                name: Some(&scope.name),
                kind: &scope.kind,
                parent: scope.parent,
            }),
            Self::Paged(h) => {
                let node = h.declaration(u32::try_from(id).ok()?)?;
                let DeclarationData::Scope { type_code, .. } = node.data else {
                    return None;
                };
                Some(BrowserScope {
                    name: h.text(&node.name),
                    kind: vtr::ScopeType::from_code(type_code).name(),
                    parent: node.parent.map(|id| id as ScopeId),
                })
            }
        }
    }
    pub fn variable(&self, id: VarId) -> Option<BrowserVariable<'a>> {
        match self {
            Self::Resident(h) => h.vars.get(id).map(|var| BrowserVariable {
                name: Some(&var.name),
                scope: Some(var.scope),
                shape: var.shape,
                direction: var.direction,
                signal: var.signal,
            }),
            Self::Paged(h) => {
                let node = h.declaration(u32::try_from(id).ok()?)?;
                let DeclarationData::Variable {
                    type_code,
                    signal,
                    kind,
                    ..
                } = node.data
                else {
                    return None;
                };
                let shape = if vtr::VarType::from_code(type_code) == vtr::VarType::Event {
                    SignalShape::Event
                } else {
                    match kind {
                        vtr_query::wave::Kind::Bits { width: 1, .. } => SignalShape::Bit,
                        vtr_query::wave::Kind::Bits { width, .. } => SignalShape::Vector {
                            width: width.max(2),
                        },
                        vtr_query::wave::Kind::Real => SignalShape::Real,
                        vtr_query::wave::Kind::Bytes => SignalShape::Text,
                    }
                };
                Some(BrowserVariable {
                    name: h.text(&node.name),
                    scope: node.parent.map(|id| id as ScopeId),
                    shape,
                    direction: h.variable_direction(id),
                    signal: SignalRef(signal),
                })
            }
        }
    }
    /// None means some required ancestor/name is not loaded yet. Literal path
    /// segments stay separate; dots inside an HDL identifier are not separators.
    pub fn scope_path(&self, mut scope: Option<ScopeId>) -> Option<Vec<&'a str>> {
        if let Self::Resident(h) = self {
            return Some(scope.map(|id| h.scope_path(id)).unwrap_or_default());
        }
        let mut path = Vec::new();
        while let Some(id) = scope {
            let node = self.scope(id)?;
            path.push(node.name?);
            scope = node.parent;
        }
        path.reverse();
        Some(path)
    }
    pub fn var_path(&self, id: VarId) -> Option<(Vec<String>, Option<usize>)> {
        if let Self::Resident(h) = self {
            return Some(h.var_path(id));
        }
        let Self::Paged(h) = self else { unreachable!() };
        let var = self.variable(id)?;
        let parent = var.scope.map(|id| id as u32);
        if !h.state(parent).is_some_and(|state| state.complete) {
            return None;
        }
        let name = var.name?;
        let mut count = 0;
        let mut occurrence = None;
        for node in h.children(parent) {
            if matches!(node.data, DeclarationData::Variable { .. })
                && matches_name(h, &node.name, name)?
            {
                if node.id as usize == id {
                    occurrence = Some(count);
                }
                count += 1;
            }
        }
        let mut path: Vec<_> = self
            .scope_path(var.scope)?
            .into_iter()
            .map(str::to_owned)
            .collect();
        path.push(name.to_owned());
        Some((path, if count > 1 { occurrence } else { None }))
    }
    pub fn find_scope(&self, path: &[impl AsRef<str>]) -> super::source::Lookup<ScopeId> {
        use super::source::Lookup;
        if let Self::Resident(h) = self {
            return h.find_scope(path);
        }
        let Self::Paged(h) = self else { unreachable!() };
        let mut parent = None;
        let mut result = Lookup::Missing;
        for segment in path {
            if !h.state(parent).is_some_and(|state| state.complete) {
                return Lookup::Pending;
            }
            result = Lookup::Missing;
            for node in h
                .children(parent)
                .filter(|node| matches!(node.data, DeclarationData::Scope { .. }))
            {
                let Some(matches) = matches_name(h, &node.name, segment.as_ref()) else {
                    return Lookup::Pending;
                };
                if matches {
                    if matches!(result, Lookup::Found(_)) {
                        return Lookup::Ambiguous;
                    }
                    result = Lookup::Found(node.id as ScopeId);
                }
            }
            match result {
                Lookup::Found(id) => parent = Some(id as u32),
                _ => return result,
            }
        }
        result
    }
    /// First unloaded sibling group needed to resolve a literal saved path.
    /// A variable path also needs the final scope's children. Unknown names
    /// return no group until their text arrives; missing or ambiguous prefixes
    /// never cause speculative descent into an unrelated subtree.
    pub(crate) fn pending_path_children(
        &self,
        path: &[String],
        variable: bool,
    ) -> Option<Option<u32>> {
        let Self::Paged(h) = self else { return None };
        let scopes = if variable { path.split_last()?.1 } else { path };
        let mut parent = None;
        for segment in scopes {
            if !h.state(parent).is_some_and(|state| state.complete) {
                return Some(parent);
            }
            let mut found = None;
            for node in h
                .children(parent)
                .filter(|node| matches!(node.data, DeclarationData::Scope { .. }))
            {
                if matches_name(h, &node.name, segment)? {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(node.id);
                }
            }
            parent = Some(found?);
        }
        (variable && !h.state(parent).is_some_and(|state| state.complete)).then_some(parent)
    }
    pub fn find_var(
        &self,
        path: &[impl AsRef<str>],
        occurrence: Option<usize>,
    ) -> super::source::Lookup<VarId> {
        use super::source::Lookup;
        if let Self::Resident(h) = self {
            return h.find_var(path, occurrence);
        }
        let Self::Paged(h) = self else { unreachable!() };
        let Some((name, scopes)) = path.split_last() else {
            return Lookup::Missing;
        };
        let parent = if scopes.is_empty() {
            None
        } else {
            match self.find_scope(scopes) {
                Lookup::Found(id) => Some(id as u32),
                Lookup::Missing => return Lookup::Missing,
                Lookup::Ambiguous => return Lookup::Ambiguous,
                Lookup::Pending => return Lookup::Pending,
            }
        };
        if !h.state(parent).is_some_and(|state| state.complete) {
            return Lookup::Pending;
        }
        let mut found = None;
        let mut count = 0;
        for node in h
            .children(parent)
            .filter(|node| matches!(node.data, DeclarationData::Variable { .. }))
        {
            let Some(matches) = matches_name(h, &node.name, name.as_ref()) else {
                return Lookup::Pending;
            };
            if matches {
                if occurrence.is_none() && found.is_some() {
                    return Lookup::Ambiguous;
                }
                if occurrence.is_none_or(|nth| count == nth) {
                    found = Some(node.id as VarId);
                }
                count += 1;
            }
        }
        found.map_or(Lookup::Missing, Lookup::Found)
    }
    pub fn full_name(&self, var: VarId) -> Option<String> {
        let var = self.variable(var)?;
        let mut path = self.scope_path(var.scope)?;
        path.push(var.name?);
        Some(path.join("."))
    }
}
impl ScopeHierarchy for BrowserHierarchy<'_> {
    fn root_scopes(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.resident()
            .into_iter()
            .flat_map(ScopeHierarchy::root_scopes)
            .chain(
                self.paged()
                    .into_iter()
                    .flat_map(ScopeHierarchy::root_scopes),
            )
    }
    fn child_scopes(&self, id: ScopeId) -> impl Iterator<Item = ScopeId> + '_ {
        self.resident()
            .into_iter()
            .flat_map(move |h| h.child_scopes(id))
            .chain(
                self.paged()
                    .into_iter()
                    .flat_map(move |h| h.child_scopes(id)),
            )
    }
    fn scope_ids(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.resident()
            .into_iter()
            .flat_map(ScopeHierarchy::scope_ids)
            .chain(self.paged().into_iter().flat_map(ScopeHierarchy::scope_ids))
    }
    fn parent_scope(&self, id: ScopeId) -> Option<ScopeId> {
        self.scope(id).and_then(|scope| scope.parent)
    }
    fn has_child_scopes(&self, id: ScopeId) -> bool {
        match self {
            Self::Resident(h) => h.has_child_scopes(id),
            Self::Paged(h) => h.has_child_scopes(id),
        }
    }
}
impl VariableHierarchy for BrowserHierarchy<'_> {
    fn variables(&self, scope: Option<ScopeId>) -> impl Iterator<Item = VarId> + '_ {
        self.resident()
            .into_iter()
            .flat_map(move |h| h.variables(scope))
            .chain(
                self.paged()
                    .into_iter()
                    .flat_map(move |h| h.variables(scope)),
            )
    }
    fn root_variables(&self) -> impl Iterator<Item = VarId> + '_ {
        self.resident()
            .into_iter()
            .flat_map(VariableHierarchy::root_variables)
            .chain(
                self.paged()
                    .into_iter()
                    .flat_map(VariableHierarchy::root_variables),
            )
    }
    fn variable_name(&self, id: VarId) -> Option<&str> {
        self.variable(id).and_then(|var| var.name)
    }
    fn variable_direction(&self, id: VarId) -> Direction {
        self.variable(id)
            .map_or(Direction::None, |var| var.direction)
    }
    fn variables_complete(&self, scope: Option<ScopeId>, global: bool) -> bool {
        match self {
            Self::Resident(h) => h.variables_complete(scope, global),
            Self::Paged(h) => h.variables_complete(scope, global),
        }
    }
}

fn matches_name(
    h: &QueryHierarchy,
    text: &vtr_query::metadata::Text,
    expected: &str,
) -> Option<bool> {
    if let Some(name) = h.text(text) {
        return Some(name == expected);
    }
    if let vtr_query::metadata::Text::Reference { bytes, .. } = text
        && *bytes != expected.len() as u64
    {
        return Some(false);
    }
    None
}
