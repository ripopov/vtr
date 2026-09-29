//! The scope tree: which scopes are expanded, which row is selected, and the
//! flattened list of visible rows a frontend paints. With several traces
//! open, each trace heads its own root row and its scopes sit below it.

use std::collections::HashSet;

use super::Key;
use crate::data::{Hierarchy, Member, ScopeId, ScopeRole, ScopeSize};
use crate::trace::{TraceId, TraceSet, Traced};

/// A row of the tree: a trace's own row (`scope` is `None`, shown only
/// while several traces are open) or one of its scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreeNode {
    pub trace: TraceId,
    pub scope: Option<ScopeId>,
}

impl TreeNode {
    pub fn trace(trace: TraceId) -> Self {
        Self { trace, scope: None }
    }

    pub fn scope(scope: Traced<ScopeId>) -> Self {
        Self {
            trace: scope.trace,
            scope: Some(scope.item),
        }
    }

    /// The scope, when this row is one.
    pub fn traced_scope(self) -> Option<Traced<ScopeId>> {
        Some(Traced::new(self.trace, self.scope?))
    }
}

#[derive(Default)]
pub struct ScopeTreeModel {
    /// Only scopes with children can be expanded. Leaves never need state or
    /// saved paths, even when the entire hierarchy is open.
    expanded: HashSet<Traced<ScopeId>>,
    /// Trace rows whose scopes are hidden.
    folded_traces: HashSet<TraceId>,
    /// Unresolved saved paths remain owned by the scope tree until explicitly replaced.
    pub(crate) unresolved_selected: Option<Traced<Vec<String>>>,
    pub(crate) unresolved_expanded: Vec<Traced<Vec<String>>>,
    pub selected: Option<TreeNode>,
    /// Flattened visible rows: (row, depth).
    pub visible: Vec<(TreeNode, usize)>,
}

/// What a key press did, so the frontend can scroll and refocus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeKeyOutcome {
    pub changed: bool,
    /// Row to scroll into view.
    pub reveal: Option<usize>,
    pub activate: Option<Traced<Member>>,
}

impl ScopeTreeModel {
    /// Start over for a new set of traces: expand the first two levels of
    /// each so the tree is not a bare list of roots, and select the first
    /// root.
    pub fn reset(&mut self, traces: &TraceSet) {
        self.expanded.clear();
        self.folded_traces.clear();
        self.unresolved_selected = None;
        self.unresolved_expanded.clear();
        self.selected = None;
        for (trace, session) in traces.loaded() {
            self.expand_top(trace, session.hierarchy());
        }
        self.selected = self.first_root(traces);
        self.rebuild(traces);
    }

    /// A trace joined: open its first two levels, keeping everything else.
    pub fn add_trace(&mut self, traces: &TraceSet, trace: TraceId) {
        if let Some(session) = traces.session(trace) {
            self.expand_top(trace, session.hierarchy());
        }
        if self.selected.is_none() {
            self.selected = self.first_root(traces);
        }
        self.rebuild(traces);
    }

    /// A trace left: forget its rows.
    pub fn remove_trace(&mut self, traces: &TraceSet, trace: TraceId) {
        self.expanded.retain(|s| s.trace != trace);
        self.folded_traces.remove(&trace);
        self.unresolved_expanded.retain(|p| p.trace != trace);
        if self
            .unresolved_selected
            .as_ref()
            .is_some_and(|p| p.trace == trace)
        {
            self.unresolved_selected = None;
        }
        if self.selected.is_some_and(|s| s.trace == trace) {
            self.selected = self.first_root(traces);
        }
        self.rebuild(traces);
    }

    fn expand_top(&mut self, trace: TraceId, h: &Hierarchy) {
        for &r in &h.roots {
            if !h.scopes[r].children.is_empty() {
                self.expanded.insert(Traced::new(trace, r));
            }
            for &c in &h.scopes[r].children {
                if !h.scopes[c].children.is_empty() {
                    self.expanded.insert(Traced::new(trace, c));
                }
            }
        }
    }

    fn first_root(&self, traces: &TraceSet) -> Option<TreeNode> {
        let (trace, session) = traces.first_loaded()?;
        let root = session.hierarchy().roots.first().copied();
        Some(match root {
            Some(root) if !traces.is_combined() => TreeNode::scope(Traced::new(trace, root)),
            _ => TreeNode::trace(trace),
        })
    }

    /// Expanded branches, including those currently hidden by a folded ancestor.
    /// Leaves never have expansion state.
    pub fn expanded(&self) -> impl Iterator<Item = Traced<ScopeId>> + '_ {
        self.expanded.iter().copied()
    }

    pub(crate) fn restore(
        &mut self,
        traces: &TraceSet,
        selected: Option<TreeNode>,
        mut expanded: HashSet<Traced<ScopeId>>,
    ) {
        expanded.retain(|&scope| Self::has_children(traces, TreeNode::scope(scope)));
        self.selected = selected;
        self.expanded = expanded;
        self.rebuild(traces);
    }

    pub fn is_expanded(&self, node: TreeNode) -> bool {
        match node.traced_scope() {
            Some(scope) => self.expanded.contains(&scope),
            None => !self.folded_traces.contains(&node.trace),
        }
    }

    /// Whether a row has rows below it to show.
    pub fn has_children(traces: &TraceSet, node: TreeNode) -> bool {
        let Some(h) = traces.session(node.trace).map(|s| s.hierarchy()) else {
            return false;
        };
        match node.scope {
            Some(scope) => !h.scopes[scope].children.is_empty(),
            None => !h.roots.is_empty(),
        }
    }

    /// A scope row's size, once its trace's count has finished; trace rows
    /// have none.
    pub fn size(traces: &TraceSet, node: TreeNode) -> Option<ScopeSize> {
        traces.get(node.trace)?.sizes()?.get(node.scope?)
    }

    pub(crate) fn rebuild(&mut self, traces: &TraceSet) {
        self.visible.clear();
        let combined = traces.is_combined();
        for (trace, session) in traces.loaded() {
            let h = session.hierarchy();
            let base = usize::from(combined);
            if combined {
                self.visible.push((TreeNode::trace(trace), 0));
                if self.folded_traces.contains(&trace) {
                    continue;
                }
            }
            let mut pending: Vec<_> = h.roots.iter().rev().map(|&id| (id, base)).collect();
            while let Some((id, depth)) = pending.pop() {
                self.visible
                    .push((TreeNode::scope(Traced::new(trace, id)), depth));
                if !h.scopes[id].children.is_empty()
                    && self.expanded.contains(&Traced::new(trace, id))
                {
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
    }

    pub fn toggle(&mut self, traces: &TraceSet, node: TreeNode) {
        if !Self::has_children(traces, node) {
            return;
        }
        match node.traced_scope() {
            Some(scope) => {
                if !self.expanded.remove(&scope) {
                    self.expanded.insert(scope);
                }
            }
            None => {
                if !self.folded_traces.remove(&node.trace) {
                    self.folded_traces.insert(node.trace);
                }
            }
        }
        self.rebuild(traces);
    }

    /// Returns true when the selection changed.
    pub fn select(&mut self, node: TreeNode) -> bool {
        self.unresolved_selected = None;
        if self.selected != Some(node) {
            self.selected = Some(node);
            true
        } else {
            false
        }
    }

    /// The selected scope, when a scope row is selected.
    pub fn selected_scope(&self) -> Option<Traced<ScopeId>> {
        self.selected?.traced_scope()
    }

    /// Open or close every branch. Leaves remain stateless, so expanding a
    /// gate-level hierarchy does not save a path for every cell.
    pub fn set_all(&mut self, traces: &TraceSet, expand: bool) {
        self.unresolved_expanded.clear();
        self.expanded.clear();
        self.folded_traces.clear();
        if expand {
            for (trace, session) in traces.loaded() {
                self.expanded.extend(
                    session
                        .hierarchy()
                        .scopes
                        .iter()
                        .enumerate()
                        .filter(|(_, scope)| !scope.children.is_empty())
                        .map(|(id, _)| Traced::new(trace, id)),
                );
            }
        }
        self.rebuild(traces);
    }

    /// The row above `node`: its scope's parent, or its trace's row.
    fn parent(traces: &TraceSet, node: TreeNode) -> Option<TreeNode> {
        let scope = node.scope?;
        let h = traces.session(node.trace)?.hierarchy();
        match h.scopes[scope].parent {
            Some(p) => Some(TreeNode::scope(Traced::new(node.trace, p))),
            None => traces.is_combined().then(|| TreeNode::trace(node.trace)),
        }
    }

    /// Keyboard navigation: up/down move, right expands, left collapses or
    /// goes to the parent, enter/space toggle. Returns whether the selection
    /// changed (the variable list follows it) and the row to reveal.
    pub fn key(&mut self, traces: &TraceSet, key: &Key) -> ScopeKeyOutcome {
        let Some(sel) = self.selected else {
            return ScopeKeyOutcome::default();
        };
        let Some(pos) = self.visible.iter().position(|(node, _)| *node == sel) else {
            return ScopeKeyOutcome::default();
        };
        let has_children = Self::has_children(traces, sel);
        let expanded = self.is_expanded(sel);
        let stream = sel.traced_scope().and_then(|scope| {
            let h = traces.session(scope.trace)?.hierarchy();
            matches!(h.scopes[scope.item].role, ScopeRole::Stream { .. })
                .then(|| scope.with(Member::Stream(scope.item)))
        });
        let mut out = ScopeKeyOutcome::default();
        match key {
            Key::Down if pos + 1 < self.visible.len() => {
                let node = self.visible[pos + 1].0;
                out.changed = self.select(node);
                out.reveal = Some(pos + 1);
            }
            Key::Up if pos > 0 => {
                let node = self.visible[pos - 1].0;
                out.changed = self.select(node);
                out.reveal = Some(pos - 1);
            }
            Key::Right if has_children && !expanded => {
                self.toggle(traces, sel);
            }
            Key::Left => {
                if has_children && expanded {
                    self.toggle(traces, sel);
                } else if let Some(p) = Self::parent(traces, sel) {
                    out.changed = self.select(p);
                    out.reveal = self.visible.iter().position(|(node, _)| *node == p);
                }
            }
            Key::Enter | Key::Space if has_children => {
                self.toggle(traces, sel);
            }
            Key::Enter | Key::Space if stream.is_some() => {
                out.activate = stream;
            }
            _ => {}
        }
        out
    }
}
