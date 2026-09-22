//! Admission for decoded client storage. Reservations follow the allocations
//! they pay for, including private builders and shared completed objects.

use std::sync::{Arc, Mutex};

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MemoryBudget(Arc<Budget>);

/// Both limits can change while the pool is live (the `memory.budgetMiB` and
/// `memory.objectMiB` settings). Lowering one never evicts: existing
/// reservations stay and only new admissions are refused.
#[derive(Debug)]
struct Budget {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    limit: u64,
    /// Largest single object an in-process load may admit.
    object_limit: u64,
    used: u64,
}

#[derive(Debug)]
pub struct Reservation {
    budget: MemoryBudget,
    bytes: u64,
}

impl MemoryBudget {
    pub fn new(limit: u64) -> Self {
        Self(Arc::new(Budget {
            state: Mutex::new(State {
                limit,
                object_limit: u64::MAX,
                used: 0,
            }),
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().expect("memory budget lock")
    }

    pub fn limit(&self) -> u64 {
        self.state().limit
    }

    pub fn set_limit(&self, limit: u64) {
        self.state().limit = limit;
    }

    pub fn object_limit(&self) -> u64 {
        self.state().object_limit
    }

    pub fn set_object_limit(&self, limit: u64) {
        self.state().object_limit = limit;
    }

    pub fn used(&self) -> u64 {
        self.state().used
    }

    /// Admit one in-process object under the current object limit.
    pub(crate) fn reserve_object(&self, what: &str, bytes: u64) -> anyhow::Result<Reservation> {
        check_object(what, bytes, self.object_limit(), "retry")?;
        self.reserve(bytes)
    }

    pub fn reserve(&self, bytes: u64) -> anyhow::Result<Reservation> {
        self.admit(bytes)?;
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }

    fn admit(&self, bytes: u64) -> anyhow::Result<()> {
        let mut state = self.state();
        let next = state.used.checked_add(bytes).filter(|&n| n <= state.limit);
        anyhow::ensure!(
            next.is_some(),
            "memory budget exceeded: {} MiB more would pass the {} MiB budget; \
             remove signals or raise memory.budgetMiB",
            bytes.div_ceil(MIB),
            state.limit / MIB
        );
        state.used = next.unwrap();
        Ok(())
    }
}

/// Refuse one decoded object above `limit`; `then` says what follows raising
/// the setting (a retry, or reopening a remote trace that negotiated it).
pub(crate) fn check_object(what: &str, bytes: u64, limit: u64, then: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        bytes <= limit,
        "{what} is {} MiB, over the {} MiB object size limit; \
         raise memory.objectMiB and {then}",
        bytes.div_ceil(MIB),
        limit / MIB
    );
    Ok(())
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.budget.state();
        state.used = state
            .used
            .checked_sub(self.bytes)
            .expect("balanced memory reservation");
    }
}

impl Reservation {
    /// The admission pool already paying for this immutable owner.
    pub(crate) fn budget(&self) -> MemoryBudget {
        self.budget.clone()
    }

    /// Release bytes no longer owned after a pessimistically admitted build.
    pub(crate) fn shrink(&mut self, bytes: u64) -> anyhow::Result<()> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| anyhow::anyhow!("reservation shrink exceeds ownership"))?;
        let mut state = self.budget.state();
        state.used = state
            .used
            .checked_sub(bytes)
            .expect("balanced memory reservation");
        Ok(())
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Move part of a reservation to an independently owned allocation. Total
    /// admission is unchanged; dropping either owner releases only its share.
    pub(crate) fn split(&mut self, bytes: u64) -> anyhow::Result<Self> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| anyhow::anyhow!("reservation split exceeds ownership"))?;
        Ok(Self {
            budget: self.budget.clone(),
            bytes,
        })
    }
    /// Extend one object's reservation as its checked decoded lengths arrive.
    pub(crate) fn grow(&mut self, bytes: u64) -> anyhow::Result<()> {
        let total = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| anyhow::anyhow!("memory size overflow"))?;
        self.budget.admit(bytes)?;
        self.bytes = total;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_are_shared_checked_and_released() {
        let budget = MemoryBudget::new(u64::MAX);
        let other = budget.clone();
        let reservation = budget.reserve(u64::MAX).unwrap();
        assert!(other.reserve(1).is_err());
        assert_eq!(budget.used(), u64::MAX);
        drop(reservation);
        assert_eq!(other.used(), 0);
        let reservation = other.reserve(7).unwrap();
        drop(other);
        assert_eq!(budget.used(), 7);
        drop(reservation);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_live_limit_change_admits_or_refuses_only_new_reservations() {
        let budget = MemoryBudget::new(10);
        let held = budget.reserve(8).unwrap();
        let error = budget.reserve(4).unwrap_err().to_string();
        assert!(error.contains("memory budget exceeded"), "{error}");
        assert!(error.contains("memory.budgetMiB"), "{error}");
        budget.set_limit(12);
        let more = budget.reserve(4).unwrap();
        budget.set_limit(1);
        assert_eq!(budget.used(), 12, "lowering never evicts");
        assert!(budget.reserve(1).is_err());
        drop((held, more));
        budget.reserve(1).unwrap();
    }

    #[test]
    fn split_ownership_releases_independently_without_readmission() {
        let budget = MemoryBudget::new(80);
        let mut parent = budget.reserve(80).unwrap();
        let child = parent.split(30).unwrap();
        assert_eq!(budget.used(), 80);
        assert!(parent.split(51).is_err());
        assert_eq!(parent.bytes(), 50);
        drop(child);
        assert_eq!(budget.used(), 50);
        drop(parent);
        assert_eq!(budget.used(), 0);
    }
}
