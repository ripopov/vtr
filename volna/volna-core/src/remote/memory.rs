//! Admission for decoded client storage. Reservations follow the allocations
//! they pay for, including private builders and shared completed objects.

use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct MemoryBudget(Arc<Budget>);

#[derive(Debug)]
struct Budget {
    limit: u64,
    used: Mutex<u64>,
}

#[derive(Debug)]
pub struct Reservation {
    budget: MemoryBudget,
    bytes: u64,
}

impl MemoryBudget {
    pub fn new(limit: u64) -> Self {
        Self(Arc::new(Budget {
            limit,
            used: Mutex::new(0),
        }))
    }

    pub fn used(&self) -> u64 {
        *self.0.used.lock().expect("memory budget lock")
    }

    pub fn reserve(&self, bytes: u64) -> anyhow::Result<Reservation> {
        let mut used = self.0.used.lock().expect("memory budget lock");
        let next = used.checked_add(bytes).filter(|&next| next <= self.0.limit);
        anyhow::ensure!(
            next.is_some(),
            "client memory budget exceeded; remove tracks or raise the limit"
        );
        *used = next.unwrap();
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = self.budget.0.used.lock().expect("memory budget lock");
        *used = used
            .checked_sub(self.bytes)
            .expect("balanced memory reservation");
    }
}

impl Reservation {
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
    pub fn grow(&mut self, bytes: u64) -> anyhow::Result<()> {
        let total = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| anyhow::anyhow!("memory size overflow"))?;
        let mut used = self.budget.0.used.lock().expect("memory budget lock");
        let next = used
            .checked_add(bytes)
            .filter(|&next| next <= self.budget.0.limit);
        anyhow::ensure!(
            next.is_some(),
            "client memory budget exceeded; remove tracks or raise the limit"
        );
        *used = next.unwrap();
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
