//! Admission before allocation, with charges retained by shared owners.

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use crate::{Error, Result};

#[derive(Debug)]
struct Account {
    limit: usize,
    used: AtomicUsize,
    _backing: Option<Reservation>,
}

/// Clones refer to the same ceiling, including cached, pinned and queued data.
#[derive(Clone, Debug)]
pub struct Budget(Arc<Account>);

impl Budget {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Account {
            limit,
            used: AtomicUsize::new(0),
            _backing: None,
        }))
    }

    /// Fixed reserved capacity for protocol controls. Its backing admission
    /// follows child reservations even after the child Budget handle is dropped.
    #[cfg(feature = "wire")]
    pub(crate) fn child(&self, limit: usize) -> Result<Self> {
        let backing = self.reserve(limit)?;
        Ok(Self(Arc::new(Account {
            limit,
            used: AtomicUsize::new(0),
            _backing: Some(backing),
        })))
    }

    pub fn limit(&self) -> usize {
        self.0.limit
    }

    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }

    /// Reserve the allocation's capacity, not just the number of populated
    /// entries. Keep the returned lease beside the allocation for its lifetime.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation> {
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&sum| sum <= self.0.limit)
            })
            .map_err(|_| Error::ResourceLimit)?;
        Ok(Reservation(Arc::new(Lease {
            account: self.0.clone(),
            bytes: AtomicUsize::new(bytes),
        })))
    }
}

#[derive(Debug)]
struct Lease {
    account: Arc<Account>,
    bytes: AtomicUsize,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.account
            .used
            .fetch_sub(self.bytes.load(Ordering::Acquire), Ordering::AcqRel);
    }
}

/// Share this only when sharing the backing allocation. Cloning does not
/// release or double-charge it; its last owner releases the reservation.
#[derive(Clone, Debug)]
pub struct Reservation(Arc<Lease>);

impl Reservation {
    pub fn bytes(&self) -> usize {
        self.0.bytes.load(Ordering::Acquire)
    }
    /// Return unused prepaid capacity after a decoder has established the
    /// retained allocation bound. All shared descendants observe the new charge.
    #[cfg(feature = "wire")]
    pub(crate) fn shrink(&self, bytes: usize) -> Result<()> {
        let previous = self
            .0
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                (bytes <= old).then_some(bytes)
            })
            .map_err(|_| Error::ResourceLimit)?;
        self.0
            .account
            .used
            .fetch_sub(previous - bytes, Ordering::AcqRel);
        Ok(())
    }
}

/// Cooperative cancellation. Workers must check this between bounded units of
/// work; dropping a future by itself does not stop a decoder.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
