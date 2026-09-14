//! Bounded batches of exact point samples using the reader's indexed lookup.
use crate::{
    native::{payload_bytes, value},
    wave::{Limits, Sample, SampleAt, SignalTime, ValuesPage},
    Budget, Cancellation, Error, Reservation, Result,
};
use std::sync::Arc;
use vtr::{Reader, SignalId, VarType};

/// Samples preserve pair order, including repeated signals and times. A work
/// unit is one reader point lookup; cancellation is checked between lookups.
/// Reader decode scratch and the temporary owned point value are not admitted
/// by this layer, just as with the exact-window predecessor lookup.
pub struct ValuesAt<'a> {
    reader: &'a Reader,
    pairs: Vec<SignalTime>,
    offset: usize,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
    _charge: Reservation,
}
impl<'a> ValuesAt<'a> {
    pub fn new(
        reader: &'a Reader,
        pairs: &[SignalTime],
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        if limits.records == 0 || limits.work == 0 {
            return Err(Error::Invalid("zero record or work limit"));
        }
        if pairs.len() > 4096 {
            return Err(Error::ResourceLimit);
        }
        // Validate the entire request before any partial delivery.
        for pair in pairs {
            cancellation.check()?;
            reader
                .signal_kind(SignalId(pair.signal))
                .map_err(|error| Error::Backend(error.to_string()))?;
        }
        let charge = budget.reserve(std::mem::size_of_val(pairs))?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(pairs.len())
            .map_err(|_| Error::ResourceLimit)?;
        if owned.capacity() != pairs.len() {
            return Err(Error::ResourceLimit);
        }
        owned.extend_from_slice(pairs);
        Ok(Self {
            reader,
            pairs: owned,
            offset: 0,
            limits,
            budget,
            cancellation,
            _charge: charge,
        })
    }

    fn lookup(&self, pair: SignalTime) -> Result<Option<vtr::OwnedSignalValue>> {
        self.cancellation.check()?;
        let signal = SignalId(pair.signal);
        if self
            .reader
            .signal_var_type(signal)
            .map_err(|error| Error::Backend(error.to_string()))?
            == VarType::Event
        {
            Ok(None)
        } else {
            self.reader
                .value_at(signal, pair.time)
                .map(Some)
                .map_err(|error| Error::Backend(error.to_string()))
        }
    }

    pub fn next_page(&mut self) -> Result<Arc<ValuesPage>> {
        self.cancellation.check()?;
        let overhead = std::mem::size_of::<ValuesPage>();
        let available = self
            .limits
            .bytes
            .checked_sub(overhead)
            .ok_or(Error::ResourceLimit)?;
        let minimum_rows =
            usize::from(self.offset < self.pairs.len()) * std::mem::size_of::<SampleAt>();
        if available < minimum_rows {
            return Err(Error::ResourceLimit);
        }
        // Admit the minimum result before reader work. The reader's temporary
        // owned value is still its responsibility; the first lookup also tells
        // us how much reply space must remain after allocating fixed row slots.
        let preparation = self.budget.reserve(overhead + minimum_rows)?;
        let mut first = self
            .pairs
            .get(self.offset)
            .copied()
            .map(|pair| self.lookup(pair))
            .transpose()?;
        let first_bytes = first
            .as_ref()
            .and_then(Option::as_ref)
            .map_or(0, |raw| payload_bytes(raw.borrow()));
        let fixed_bytes = available
            .checked_sub(first_bytes)
            .ok_or(Error::ResourceLimit)?;
        let capacity = self
            .limits
            .records
            .min(self.limits.work)
            .min(self.pairs.len() - self.offset)
            .min(fixed_bytes / std::mem::size_of::<SampleAt>());
        if capacity == 0 && self.offset < self.pairs.len() {
            return Err(Error::ResourceLimit);
        }
        let rows = capacity * std::mem::size_of::<SampleAt>();
        // No page or row allocation exists yet; replace the preparation lease
        // with admission for the exact allocation before reserving the vector.
        drop(preparation);
        let charge = self.budget.reserve(overhead + rows)?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        if samples.capacity() != capacity {
            return Err(Error::ResourceLimit);
        }
        let mut remaining = available - rows;
        for (index, pair) in self.pairs[self.offset..self.offset + capacity]
            .iter()
            .enumerate()
        {
            self.cancellation.check()?;
            let raw = if index == 0 {
                first.take().unwrap()
            } else {
                self.lookup(*pair)?
            };
            let sample = if let Some(raw) = raw {
                let size = payload_bytes(raw.borrow());
                if size > remaining {
                    if samples.is_empty() {
                        return Err(Error::ResourceLimit);
                    }
                    break;
                }
                let value = match value(raw.borrow(), &self.budget) {
                    Ok(value) => value,
                    Err(Error::ResourceLimit) if !samples.is_empty() => break,
                    Err(error) => return Err(error),
                };
                remaining -= size;
                Sample::Known(value)
            } else {
                Sample::Event
            };
            samples.push(SampleAt {
                pair: *pair,
                sample,
            });
        }
        self.cancellation.check()?;
        let offset = self.offset;
        self.offset += samples.len();
        Ok(Arc::new(ValuesPage {
            offset,
            complete: self.offset == self.pairs.len(),
            samples,
            _charge: charge,
        }))
    }
}
