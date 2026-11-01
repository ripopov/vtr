//! Capacity admission for activity scan and encoding buffers.

use crate::{Error, Result};
use std::sync::Arc;

pub(super) trait Budget: Send + Sync {
    fn acquire(&self, minimum: u64, preferred: u64) -> Result<Option<u64>>;
    fn release(&self, bytes: u64);
}

pub(super) struct Reservation {
    budget: Arc<dyn Budget>,
    bytes: u64,
}

impl Reservation {
    pub(super) fn new(budget: Arc<dyn Budget>) -> Self {
        Self { budget, bytes: 0 }
    }

    pub(super) fn clear(&mut self) {
        self.budget.release(std::mem::take(&mut self.bytes));
    }

    fn reserve<T>(&mut self, values: &mut Vec<T>, length: usize, exact: bool) -> Result<bool> {
        if length <= values.capacity() {
            return Ok(true);
        }
        let size = std::mem::size_of::<T>() as u64;
        let capacity = if exact {
            length
        } else {
            length.max(values.capacity().saturating_mul(2).max(4))
        };
        let minimum = ((length - values.capacity()) as u64).saturating_mul(size);
        let preferred = ((capacity - values.capacity()) as u64).saturating_mul(size);
        let Some(granted) = self.budget.acquire(minimum, preferred)? else {
            return Ok(false);
        };
        let capacity = values.capacity() + (granted / size) as usize;
        let used = ((capacity - values.capacity()) as u64) * size;
        let result = values.try_reserve_exact(capacity - values.len());
        if let Err(error) = result {
            self.budget.release(granted);
            return Err(Error::invalid(format!(
                "cannot allocate activity workspace: {error}"
            )));
        }
        self.bytes += used;
        self.budget.release(granted - used);
        Ok(true)
    }

    fn shrink<T>(&mut self, values: &mut Vec<T>) {
        let before = bytes(values);
        values.shrink_to_fit();
        let released = before - bytes(values);
        self.bytes -= released;
        self.budget.release(released);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.clear();
    }
}

pub(super) fn try_reserve_buffer<T>(
    values: &mut Vec<T>,
    length: usize,
    reservation: Option<&mut Reservation>,
) -> Result<bool> {
    match reservation {
        Some(reservation) => reservation.reserve(values, length, false),
        None => reserve(values, length, u64::MAX).map(|()| true),
    }
}

pub(super) fn reserve_exact_buffer<T>(
    values: &mut Vec<T>,
    length: usize,
    reservation: Option<&mut Reservation>,
) -> Result<()> {
    match reservation {
        Some(reservation) => {
            if !reservation.reserve(values, length, true)? {
                return Err(exhausted());
            }
            Ok(())
        }
        None => reserve(values, length, u64::MAX),
    }
}

pub(super) fn shrink_buffer<T>(values: &mut Vec<T>, reservation: Option<&mut Reservation>) {
    if let Some(reservation) = reservation {
        reservation.shrink(values);
    } else {
        values.shrink_to_fit();
    }
}

pub(super) fn bytes<T>(values: &Vec<T>) -> u64 {
    (values.capacity() as u64).saturating_mul(std::mem::size_of::<T>() as u64)
}

pub(super) fn check(used: u64, limit: u64) -> Result<()> {
    if used > limit {
        return Err(exhausted());
    }
    Ok(())
}

fn exhausted() -> Error {
    Error::invalid("activity workspace exceeds the memory limit")
}

pub(super) fn reserve<T>(values: &mut Vec<T>, length: usize, available: u64) -> Result<()> {
    if length <= values.capacity() {
        return Ok(());
    }
    let maximum = (available / std::mem::size_of::<T>() as u64).min(usize::MAX as u64) as usize;
    check(length as u64, maximum as u64)?;
    let capacity = length
        .max(values.capacity().saturating_mul(2).max(4))
        .min(maximum);
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|error| Error::invalid(format!("cannot allocate activity workspace: {error}")))?;
    Ok(())
}
