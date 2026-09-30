//! Scope sizes: how many distinct signals, variables and scopes each scope
//! holds, subscopes included.
//!
//! A signal aliased by several variables below a scope counts once there
//! (ports name the nets of their parents, so summing variables overcounts).
//! [`Census`] is a format-neutral streaming counter fed in the order a
//! hierarchy is declared (`$scope` … `$upscope`);
//! [`Hierarchy::scope_sizes`] is its front end for a VTR hierarchy.
//!
//! The method is colour-set-size counting (Hui, CPM 1992): every scope that
//! holds a signal adds one, the lowest common ancestor of each pair of
//! consecutive (in preorder) scopes holding it subtracts one, and a subtree
//! sum gives the distinct count. Tarjan's offline LCA answers each pair as
//! the second scope opens, so the count is one pass, linear in variables
//! plus scopes (with the near-constant union-find factor). Memory is 24
//! bytes per scope and 4 per signal; variables are never stored.
//!
//! ```
//! let mut c = vtr::Census::new();
//! let top = c.enter();
//! c.var(0); // clk
//! let a = c.enter();
//! c.var(0); // a port aliasing clk
//! c.var(1);
//! c.leave();
//! c.leave();
//! let s = c.finish();
//! assert_eq!((s.signals(top), s.variables(top), s.scopes(top)), (2, 3, 2));
//! assert_eq!(s.signals(a), 2);
//! ```

use crate::hierarchy::{Hierarchy, NodeId, NodeKind, SignalId};

const NONE: u32 = u32::MAX;

/// Streaming scope-size counter. Scopes are numbered in the order they are
/// entered (preorder); [`var`](Self::var) adds a variable of a signal to the
/// innermost open scope.
#[derive(Debug, Default)]
pub struct Census {
    parent: Vec<u32>,
    /// Union-find over scopes (closed subtrees join their parent's set) and
    /// the open ancestor that represents each set (`NONE` once its root closed).
    uf: Vec<u32>,
    anc: Vec<u32>,
    /// Weights: +1 per scope holding a signal, −1 at each LCA. Partial sums
    /// can dip below zero; the subtree sums cannot, so wrapping `u32`
    /// arithmetic is exact.
    signals: Vec<u32>,
    variables: Vec<u32>,
    /// Last scope that held each signal.
    last: Vec<u32>,
    open: Vec<u32>,
}

impl Census {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a scope inside the innermost open one (or as a root) and
    /// returns its index.
    pub fn enter(&mut self) -> u32 {
        let i = self.parent.len() as u32;
        self.parent.push(self.open.last().copied().unwrap_or(NONE));
        self.uf.push(i);
        self.anc.push(i);
        self.signals.push(0);
        self.variables.push(0);
        self.open.push(i);
        i
    }

    /// Adds a variable naming `signal` to the innermost open scope.
    ///
    /// # Panics
    /// When no scope is open.
    pub fn var(&mut self, signal: u32) {
        let i = *self.open.last().expect("Census::var outside a scope");
        self.variables[i as usize] += 1;
        let g = signal as usize;
        if g >= self.last.len() {
            self.last.resize(g + 1, NONE);
        }
        let prev = self.last[g];
        if prev == i {
            return;
        }
        self.signals[i as usize] = self.signals[i as usize].wrapping_add(1);
        if prev != NONE {
            let root = self.find(prev);
            let l = self.anc[root as usize];
            // A signal shared across roots has no common ancestor.
            if l != NONE {
                self.signals[l as usize] = self.signals[l as usize].wrapping_sub(1);
            }
        }
        self.last[g] = i;
    }

    /// Closes the innermost open scope.
    ///
    /// # Panics
    /// When no scope is open.
    pub fn leave(&mut self) {
        let i = self.open.pop().expect("Census::leave without a scope");
        let p = self.parent[i as usize];
        let b = self.find(i);
        if p == NONE {
            self.anc[b as usize] = NONE;
        } else {
            let a = self.find(p);
            self.uf[b as usize] = a;
            self.anc[a as usize] = p;
        }
    }

    /// Number of scopes open (entered and not yet left).
    pub fn depth(&self) -> usize {
        self.open.len()
    }

    /// Closes any scopes still open and sums the subtrees.
    pub fn finish(mut self) -> ScopeSizes {
        while !self.open.is_empty() {
            self.leave();
        }
        let n = self.parent.len();
        let mut scopes = vec![1u32; n];
        for i in (0..n).rev() {
            let p = self.parent[i];
            if p != NONE {
                let p = p as usize;
                self.signals[p] = self.signals[p].wrapping_add(self.signals[i]);
                self.variables[p] += self.variables[i];
                scopes[p] += scopes[i];
            }
        }
        ScopeSizes { parent: self.parent, signals: self.signals, variables: self.variables, scopes }
    }

    fn find(&mut self, mut x: u32) -> u32 {
        let mut r = x;
        while self.uf[r as usize] != r {
            r = self.uf[r as usize];
        }
        while self.uf[x as usize] != r {
            let n = self.uf[x as usize];
            self.uf[x as usize] = r;
            x = n;
        }
        r
    }
}

/// Per-scope totals, subscopes included, indexed by the order scopes were
/// entered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeSizes {
    parent: Vec<u32>,
    signals: Vec<u32>,
    variables: Vec<u32>,
    scopes: Vec<u32>,
}

impl ScopeSizes {
    /// Number of scopes counted.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// The enclosing scope, `None` for a root.
    pub fn parent(&self, scope: u32) -> Option<u32> {
        Some(self.parent[scope as usize]).filter(|&p| p != NONE)
    }

    /// Distinct signals named at or below `scope`.
    pub fn signals(&self, scope: u32) -> u32 {
        self.signals[scope as usize]
    }

    /// Variables at or below `scope`; aliases count every time.
    pub fn variables(&self, scope: u32) -> u32 {
        self.variables[scope as usize]
    }

    /// Scopes at or below `scope`, itself included.
    pub fn scopes(&self, scope: u32) -> u32 {
        self.scopes[scope as usize]
    }
}

impl Hierarchy {
    /// Scope sizes, and the scope node of each [`ScopeSizes`] index (scopes
    /// in preorder). Streams hold no variables and are not counted.
    ///
    /// Requires [`build_index`](Self::build_index) (a [`crate::Reader`]'s
    /// hierarchy is indexed).
    pub fn scope_sizes(&self) -> (Vec<NodeId>, ScopeSizes) {
        self.scope_sizes_of(|_| true)
    }

    /// [`scope_sizes`](Self::scope_sizes) counting only the variables of the
    /// signals `keep` accepts, such as the signals that change in a window.
    /// Scopes and their order are the same.
    pub fn scope_sizes_of(&self, mut keep: impl FnMut(SignalId) -> bool) -> (Vec<NodeId>, ScopeSizes) {
        let mut c = Census::new();
        let mut nodes = Vec::new();
        // Preorder with explicit leave markers: `None` closes a scope.
        let mut stack: Vec<Option<NodeId>> = self.roots().filter(|&n| self.kind(n) == NodeKind::Scope).map(Some).collect();
        stack.reverse();
        let mut kids = Vec::new();
        while let Some(entry) = stack.pop() {
            let Some(n) = entry else {
                c.leave();
                continue;
            };
            c.enter();
            nodes.push(n);
            stack.push(None);
            for k in self.children(n) {
                match self.kind(k) {
                    NodeKind::Var => {
                        let s = self.signal_of(k).expect("var has a signal");
                        if keep(s) {
                            c.var(s.0);
                        }
                    }
                    NodeKind::Scope => kids.push(k),
                    _ => {}
                }
            }
            stack.extend(kids.drain(..).rev().map(Some));
        }
        (nodes, c.finish())
    }
}
