//! Scope sizes: the distinct signals, variables and scopes at or below each
//! scope (docs/hierarchy-scope-sizes.html). Counted once per trace off the
//! UI thread with [`vtr::Census`], whatever the trace's format.

use super::source::{Hierarchy, ScopeId};

/// Totals of one scope, subscopes included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScopeSize {
    /// Distinct signals: an aliased signal counts once.
    pub signals: u32,
    /// Variables: every alias counts.
    pub variables: u32,
    /// Scopes, itself included.
    pub scopes: u32,
}

/// Every scope's [`ScopeSize`], indexed by [`ScopeId`].
#[derive(Debug, Default)]
pub struct ScopeSizes {
    pub(crate) sizes: Vec<ScopeSize>,
    pub(crate) _ownership: Option<std::sync::Arc<Vec<crate::remote::memory::Reservation>>>,
}

impl ScopeSizes {
    /// One preorder pass over the hierarchy (linear in scopes and variables).
    pub fn count(h: &Hierarchy) -> Self {
        let mut census = vtr::Census::new();
        // Census index → scope, and explicit leave markers (`None`).
        let mut order = Vec::with_capacity(h.scope_count());
        let mut stack: Vec<Option<ScopeId>> = h.roots().iter().rev().map(Some).collect();
        while let Some(entry) = stack.pop() {
            let Some(id) = entry else {
                census.leave();
                continue;
            };
            census.enter();
            order.push(id);
            let scope = h.scope(id);
            for v in scope.vars {
                census.var(h.signal(v).0);
            }
            stack.push(None);
            stack.extend(scope.children.iter().rev().map(Some));
        }
        let counted = census.finish();
        let mut sizes = vec![ScopeSize::default(); h.scope_count()];
        for (i, &id) in order.iter().enumerate() {
            let i = i as u32;
            sizes[id] = ScopeSize {
                signals: counted.signals(i),
                variables: counted.variables(i),
                scopes: counted.scopes(i),
            };
        }
        Self {
            sizes,
            _ownership: None,
        }
    }

    pub fn get(&self, scope: ScopeId) -> Option<ScopeSize> {
        self.sizes.get(scope).copied()
    }
}
