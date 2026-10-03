//! Scope activity (docs/hierarchy-activity.html): how many of each scope's
//! signals change in a window, from the trace's activity index at once, and
//! exactly once the signals the index leaves undecided are read.
//!
//! [`ActivityCounter`] holds every signal's census weights over the trace's
//! scopes, built once per trace off the UI thread. [`ActivityCounts`] is one
//! window's answer per scope; a frame costs one classification of every
//! signal and one pass over the changing signals' weights and the scopes.

use volna_trace::data::SignalRef;
use volna_trace::data::source::{Hierarchy, ScopeId};
use volna_trace::remote::memory::{MemoryBudget, Reservation};

/// Every signal's census weights over one trace's scopes, so that the
/// distinct signals of any set below every scope cost the set's variables
/// plus one pass over the scopes.
#[derive(Debug)]
pub struct ActivityCounter {
    contributions: vtr::Contributions,
    /// Census index → scope.
    order: Vec<ScopeId>,
    reservation: Option<Reservation>,
}

impl ActivityCounter {
    /// One recording census over the hierarchy in preorder.
    pub fn build(h: &Hierarchy) -> Self {
        let mut census = vtr::Census::recording();
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
        let (_, contributions) = census.finish_recorded();
        Self {
            contributions,
            order,
            reservation: None,
        }
    }

    pub fn account(mut self, budget: &MemoryBudget) -> anyhow::Result<Self> {
        self.reservation =
            Some(budget.reserve_object("the activity counter", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        self.contributions.memory_bytes()
            + (self.order.len() * std::mem::size_of::<ScopeId>()) as u64
    }

    /// Distinct signals of `signals` at or below every scope, by scope.
    fn count(&self, signals: impl IntoIterator<Item = u32>) -> Vec<u32> {
        let mut by_census = Vec::new();
        self.contributions.count(signals, &mut by_census);
        let mut by_scope = vec![0; self.order.len()];
        for (i, &scope) in self.order.iter().enumerate() {
            by_scope[scope] = by_census[i];
        }
        by_scope
    }
}

/// One window's activity per scope: the signals that change, and at most
/// how many may while some are undecided.
#[derive(Debug)]
pub struct ActivityCounts {
    /// The window in trace times, both ends included.
    pub window: (u64, u64),
    /// Signals below each scope known to change, by scope.
    changing: Vec<u32>,
    /// Changing or undecided, by scope; empty once exact.
    upper: Vec<u32>,
    /// Signals the index cannot decide in this window, to read from the
    /// trace; empty once exact.
    pub undecided: Vec<SignalRef>,
    reservation: Option<Reservation>,
}

impl ActivityCounts {
    /// What the index says about `window` (trace times).
    pub fn classify(
        index: &vtr::activity::Index,
        counter: &ActivityCounter,
        window: (u64, u64),
    ) -> Self {
        let c = index.classify(window.0, window.1);
        let changing = counter.count(c.active.iter().map(|s| s.0));
        let upper = if c.undecided.is_empty() {
            Vec::new()
        } else {
            let may = counter.count(c.undecided.iter().map(|s| s.0));
            changing.iter().zip(may).map(|(a, b)| a + b).collect()
        };
        Self {
            window,
            changing,
            upper,
            undecided: c.undecided.into_iter().map(|s| SignalRef(s.0)).collect(),
            reservation: None,
        }
    }

    /// The exact counts once the undecided signals were read: `changed` are
    /// those of them that change in the window.
    pub fn resolved(&self, counter: &ActivityCounter, changed: &[SignalRef]) -> Self {
        let more = counter.count(changed.iter().map(|s| s.0));
        Self {
            window: self.window,
            changing: self.changing.iter().zip(more).map(|(a, b)| a + b).collect(),
            upper: Vec::new(),
            undecided: Vec::new(),
            reservation: None,
        }
    }

    pub fn account(mut self, budget: &MemoryBudget) -> anyhow::Result<Self> {
        self.reservation =
            Some(budget.reserve_object("the scope activity", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        ((self.changing.len() + self.upper.len()) * 4
            + self.undecided.len() * std::mem::size_of::<SignalRef>()) as u64
    }

    /// Whether every signal is decided.
    pub fn exact(&self) -> bool {
        self.undecided.is_empty()
    }

    /// `(changing, upper)` below `scope`.
    pub fn get(&self, scope: ScopeId) -> Option<(u32, u32)> {
        let changing = *self.changing.get(scope)?;
        Some((changing, self.upper.get(scope).copied().unwrap_or(changing)))
    }
}

/// What a scope row shows of the activity in the viewport: of its `total`
/// signals, `changing` change for sure and at most `upper` may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeActivity {
    pub changing: u32,
    pub upper: u32,
    pub total: u32,
}

impl ScopeActivity {
    /// Nothing is left to read.
    pub fn exact(&self) -> bool {
        self.changing == self.upper
    }

    /// No signal below the scope can change: the row is drawn faint.
    pub fn quiet(&self) -> bool {
        self.total > 0 && self.upper == 0
    }

    /// The count column: `2,483 / 6,779`, or a range while signals are
    /// read: `1,462–2,210 / 6,779`.
    pub fn label(&self) -> String {
        let total = super::sizes::grouped(self.total);
        match self.exact() {
            true => format!("{} / {total}", super::sizes::grouped(self.changing)),
            false => format!(
                "{}–{} / {total}",
                super::sizes::grouped(self.changing),
                super::sizes::grouped(self.upper)
            ),
        }
    }

    /// Shares of the scope's signals that change and that may: the
    /// meter's solid and hatched lengths, in `0..=1`.
    pub fn shares(&self) -> (f32, f32) {
        match self.total {
            0 => (0.0, 0.0),
            t => (
                self.changing as f32 / t as f32,
                self.upper as f32 / t as f32,
            ),
        }
    }

    /// The tooltip line: `2,483 of 6,779 signals change in the view`.
    pub fn detail(&self) -> String {
        let total = super::sizes::grouped(self.total);
        match self.exact() {
            true => format!(
                "{} of {total} signals change in the view",
                super::sizes::grouped(self.changing)
            ),
            false => format!(
                "{} to {} of {total} signals change in the view; reading the rest from the trace",
                super::sizes::grouped(self.changing),
                super::sizes::grouped(self.upper)
            ),
        }
    }
}

/// Estimated build duration from the compressed recording size.
pub fn estimated_seconds(info: volna_trace::data::ActivityBuildInfo) -> u64 {
    let rate = match info.format {
        vtr::activity::SourceFormat::Vtr => 1 << 30,
        vtr::activity::SourceFormat::Fst => 400 << 20,
    };
    info.bytes.div_ceil(rate).max(1)
}
