//! The scope tree: which scopes are expanded, which is selected, and the
//! flattened list of visible rows a frontend paints.

use std::collections::HashSet;

use super::Key;
use crate::data::{Hierarchy, Member, ScopeId, ScopeRole};

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
    pub activate: Option<Member>,
}

impl ScopeTreeModel {
    /// Start over for a new hierarchy: expand the first two levels so the
    /// tree is not a bare list of roots, and select the first root.
    pub fn reset(&mut self, h: Option<&Hierarchy>) {
        self.expanded.clear();
        self.unresolved_selected = None;
        self.unresolved_expanded.clear();
        self.selected = None;
        if let Some(h) = h {
            for &r in &h.roots {
                self.expanded.insert(r);
                for &c in &h.scopes[r].children {
                    self.expanded.insert(c);
                }
            }
            self.selected = h.roots.first().copied();
        }
        self.rebuild(h);
    }

    pub fn expanded(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.expanded.iter().copied()
    }

    pub(crate) fn restore(
        &mut self,
        h: &Hierarchy,
        selected: Option<ScopeId>,
        expanded: HashSet<ScopeId>,
    ) {
        self.selected = selected;
        self.expanded = expanded;
        self.rebuild(Some(h));
    }

    pub fn is_expanded(&self, id: ScopeId) -> bool {
        self.expanded.contains(&id)
    }

    fn rebuild(&mut self, h: Option<&Hierarchy>) {
        self.visible.clear();
        let Some(h) = h else { return };
        let mut pending: Vec<_> = h.roots.iter().rev().map(|&id| (id, 0)).collect();
        while let Some((id, depth)) = pending.pop() {
            self.visible.push((id, depth));
            if self.expanded.contains(&id) {
                pending.extend(
                    h.scopes[id]
                        .children
                        .iter()
                        .rev()
                        .map(|&id| (id, depth + 1)),
                );
            }
        }
    }

    pub fn toggle(&mut self, h: &Hierarchy, id: ScopeId) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.rebuild(Some(h));
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

    pub fn set_all(&mut self, h: &Hierarchy, expand: bool) {
        self.unresolved_expanded.clear();
        self.expanded.clear();
        if expand {
            self.expanded.extend(0..h.scopes.len());
        }
        self.rebuild(Some(h));
    }

    /// Keyboard navigation: up/down move, right expands, left collapses or
    /// goes to the parent, enter/space toggle. Returns whether the selection
    /// changed (the variable list follows it) and the row to reveal.
    pub fn key(&mut self, h: &Hierarchy, key: &Key) -> ScopeKeyOutcome {
        let Some(sel) = self.selected else {
            return ScopeKeyOutcome::default();
        };
        let Some(pos) = self.visible.iter().position(|(id, _)| *id == sel) else {
            return ScopeKeyOutcome::default();
        };
        let has_children = !h.scopes[sel].children.is_empty();
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
                if has_children && self.expanded.contains(&sel) {
                    self.toggle(h, sel);
                } else if let Some(p) = h.scopes[sel].parent {
                    out.changed = self.select(p);
                    out.reveal = self.visible.iter().position(|(id, _)| *id == p);
                }
            }
            Key::Enter | Key::Space if has_children => {
                self.toggle(h, sel);
            }
            Key::Enter | Key::Space if matches!(h.scopes[sel].role, ScopeRole::Stream { .. }) => {
                out.activate = Some(Member::Stream(sel));
            }
            _ => {}
        }
        out
    }
}
