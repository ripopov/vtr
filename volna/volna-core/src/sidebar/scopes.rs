//! The scope tree: which scopes are expanded, which is selected, and the
//! flattened list of visible rows a frontend paints.

use std::collections::HashSet;

use super::Key;
use crate::data::{Hierarchy, ScopeId};
use crate::icons::IconName;

#[derive(Default)]
pub struct ScopeTreeModel {
    expanded: HashSet<ScopeId>,
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
    pub fn reset(&mut self, h: Option<&Hierarchy>) {
        self.expanded.clear();
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

    pub fn is_expanded(&self, id: ScopeId) -> bool {
        self.expanded.contains(&id)
    }

    fn rebuild(&mut self, h: Option<&Hierarchy>) {
        self.visible.clear();
        let Some(h) = h else { return };
        fn walk(
            h: &Hierarchy,
            id: ScopeId,
            depth: usize,
            expanded: &HashSet<ScopeId>,
            out: &mut Vec<(ScopeId, usize)>,
        ) {
            out.push((id, depth));
            if expanded.contains(&id) {
                for &c in &h.scopes[id].children {
                    walk(h, c, depth + 1, expanded, out);
                }
            }
        }
        for &r in &h.roots {
            walk(h, r, 0, &self.expanded, &mut self.visible);
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
        if self.selected != Some(id) {
            self.selected = Some(id);
            true
        } else {
            false
        }
    }

    pub fn set_all(&mut self, h: &Hierarchy, expand: bool) {
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
            Key::Down => {
                if pos + 1 < self.visible.len() {
                    let id = self.visible[pos + 1].0;
                    out.changed = self.select(id);
                    out.reveal = Some(pos + 1);
                }
            }
            Key::Up => {
                if pos > 0 {
                    let id = self.visible[pos - 1].0;
                    out.changed = self.select(id);
                    out.reveal = Some(pos - 1);
                }
            }
            Key::Right => {
                if has_children && !self.expanded.contains(&sel) {
                    self.toggle(h, sel);
                }
            }
            Key::Left => {
                if self.expanded.contains(&sel) {
                    self.toggle(h, sel);
                } else if let Some(p) = h.scopes[sel].parent {
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
