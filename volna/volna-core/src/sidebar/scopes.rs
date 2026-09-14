//! The scope tree: which scopes are expanded, which is selected, and the
//! flattened list of visible rows a frontend paints.

use std::collections::HashSet;

use super::Key;
use crate::data::{Hierarchy, ScopeId};
use crate::icons::IconName;

/// Read-only topology used by the tree model. An unloaded remote scope may
/// have children even when no child rows have arrived yet.
pub trait ScopeHierarchy {
    fn root_scopes(&self) -> impl Iterator<Item = ScopeId> + '_;
    fn child_scopes(&self, id: ScopeId) -> impl Iterator<Item = ScopeId> + '_;
    fn scope_ids(&self) -> impl Iterator<Item = ScopeId> + '_;
    fn parent_scope(&self, id: ScopeId) -> Option<ScopeId>;
    fn has_child_scopes(&self, id: ScopeId) -> bool;
}
impl ScopeHierarchy for Hierarchy {
    fn root_scopes(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.roots.iter().copied()
    }
    fn child_scopes(&self, id: ScopeId) -> impl Iterator<Item = ScopeId> + '_ {
        self.scopes
            .get(id)
            .into_iter()
            .flat_map(|scope| scope.children.iter().copied())
    }
    fn scope_ids(&self) -> impl Iterator<Item = ScopeId> + '_ {
        0..self.scopes.len()
    }
    fn parent_scope(&self, id: ScopeId) -> Option<ScopeId> {
        self.scopes.get(id).and_then(|scope| scope.parent)
    }
    fn has_child_scopes(&self, id: ScopeId) -> bool {
        self.child_scopes(id).next().is_some()
    }
}
impl ScopeHierarchy for crate::data::query_hierarchy::QueryHierarchy {
    fn root_scopes(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.children(None).filter_map(scope_id)
    }
    fn child_scopes(&self, id: ScopeId) -> impl Iterator<Item = ScopeId> + '_ {
        u32::try_from(id)
            .ok()
            .into_iter()
            .flat_map(|parent| self.children(Some(parent)))
            .filter_map(scope_id)
    }
    fn scope_ids(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.declarations().filter_map(scope_id)
    }
    fn parent_scope(&self, id: ScopeId) -> Option<ScopeId> {
        self.declaration(u32::try_from(id).ok()?)
            .and_then(|node| node.parent)
            .map(|id| id as ScopeId)
    }
    fn has_child_scopes(&self, id: ScopeId) -> bool {
        let Ok(raw) = u32::try_from(id) else {
            return false;
        };
        self.child_scopes(id).next().is_some()
            || (self.declaration(raw).is_some_and(|node| node.children > 0)
                && !self.state(Some(raw)).is_some_and(|state| state.complete))
    }
}
fn scope_id(node: &vtr_query::metadata::Declaration) -> Option<ScopeId> {
    matches!(
        node.data,
        vtr_query::metadata::DeclarationData::Scope { .. }
    )
    .then_some(node.id as ScopeId)
}

#[derive(Default)]
pub struct ScopeTreeModel {
    expanded: HashSet<ScopeId>,
    /// Unresolved saved paths remain owned by the scope tree until explicitly replaced.
    pub(crate) unresolved_selected: Option<Vec<String>>,
    pub(crate) unresolved_expanded: Vec<Vec<String>>,
    pub selected: Option<ScopeId>,
    /// Flattened visible rows: (scope, depth).
    pub visible: Vec<(ScopeId, usize)>,
}

/// What a key press did, so the frontend can scroll and refocus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeKeyOutcome {
    pub changed: bool,
    /// Row to scroll into view.
    pub reveal: Option<usize>,
}

impl ScopeTreeModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start over for a new hierarchy: expand the first two levels so the
    /// tree is not a bare list of roots, and select the first root.
    pub fn reset(&mut self, h: Option<&impl ScopeHierarchy>) {
        self.expanded.clear();
        self.unresolved_selected = None;
        self.unresolved_expanded.clear();
        self.selected = None;
        if let Some(h) = h {
            for r in h.root_scopes() {
                self.expanded.insert(r);
                for c in h.child_scopes(r) {
                    self.expanded.insert(c);
                }
            }
            self.selected = h.root_scopes().next();
        }
        self.refresh(h);
    }

    pub fn expanded(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.expanded.iter().copied()
    }

    pub(crate) fn restore(
        &mut self,
        h: &impl ScopeHierarchy,
        selected: Option<ScopeId>,
        expanded: HashSet<ScopeId>,
    ) {
        self.selected = selected;
        self.expanded = expanded;
        self.refresh(Some(h));
    }

    pub fn is_expanded(&self, id: ScopeId) -> bool {
        self.expanded.contains(&id)
    }

    pub fn refresh(&mut self, h: Option<&impl ScopeHierarchy>) {
        self.visible.clear();
        let Some(h) = h else { return };
        let mut pending: Vec<_> = h.root_scopes().map(|id| (id, 0)).collect();
        pending.reverse();
        while let Some((id, depth)) = pending.pop() {
            self.visible.push((id, depth));
            if self.expanded.contains(&id) {
                let first = pending.len();
                pending.extend(h.child_scopes(id).map(|child| (child, depth + 1)));
                pending[first..].reverse();
            }
        }
    }

    pub fn toggle(&mut self, h: &impl ScopeHierarchy, id: ScopeId) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.refresh(Some(h));
    }

    /// Returns true when the selection changed.
    pub fn select(&mut self, id: ScopeId) -> bool {
        self.unresolved_selected = None;
        if self.selected != Some(id) {
            self.selected = Some(id);
            true
        } else {
            false
        }
    }

    pub fn set_all(&mut self, h: &impl ScopeHierarchy, expand: bool) {
        self.unresolved_expanded.clear();
        self.expanded.clear();
        if expand {
            self.expanded.extend(h.scope_ids());
        }
        self.refresh(Some(h));
    }

    /// Keyboard navigation: up/down move, right expands, left collapses or
    /// goes to the parent, enter/space toggle. Returns whether the selection
    /// changed (the variable list follows it) and the row to reveal.
    pub fn key(&mut self, h: &impl ScopeHierarchy, key: &Key) -> ScopeKeyOutcome {
        let Some(sel) = self.selected else {
            return ScopeKeyOutcome::default();
        };
        let Some(pos) = self.visible.iter().position(|(id, _)| *id == sel) else {
            return ScopeKeyOutcome::default();
        };
        let has_children = h.has_child_scopes(sel);
        let mut out = ScopeKeyOutcome::default();
        match key {
            Key::Down if pos + 1 < self.visible.len() => {
                let id = self.visible[pos + 1].0;
                out.changed = self.select(id);
                out.reveal = Some(pos + 1);
            }
            Key::Up if pos > 0 => {
                let id = self.visible[pos - 1].0;
                out.changed = self.select(id);
                out.reveal = Some(pos - 1);
            }
            Key::Right if has_children && !self.expanded.contains(&sel) => {
                self.toggle(h, sel);
            }
            Key::Left => {
                if self.expanded.contains(&sel) {
                    self.toggle(h, sel);
                } else if let Some(p) = h.parent_scope(sel) {
                    out.changed = self.select(p);
                }
            }
            Key::Enter | Key::Space if has_children => {
                self.toggle(h, sel);
            }
            _ => {}
        }
        out
    }
}

/// Icon for a scope kind name (`module`, `struct`, ...).
pub fn scope_icon(kind: &str) -> IconName {
    match kind {
        "module" | "sc_module" | "core" => IconName::Box,
        "struct" | "union" | "class" | "interface" | "vhdl_record" => IconName::Braces,
        "package" | "vhdl_package" => IconName::Folder,
        _ => IconName::Folder,
    }
}
