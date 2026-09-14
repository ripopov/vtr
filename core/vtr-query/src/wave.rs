//! Immutable exact waveform pages. Aggregates use a separate representation.
use crate::{Budget, Error, Interval, Reservation, Result};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bits { width: u32, states: u8 },
    Real,
    Bytes,
}

#[derive(Debug)]
struct Storage {
    bytes: Vec<u8>,
    _charge: Reservation,
}

#[derive(Clone, Debug)]
pub struct Bytes(Arc<Storage>);
impl Bytes {
    /// Payload and owning storage, excluding allocator bookkeeping.
    pub fn retained_size(payload: usize) -> Result<usize> {
        payload
            .checked_add(std::mem::size_of::<Storage>())
            .ok_or(Error::ResourceLimit)
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.0.bytes
    }
    /// The wire preflight reserves generated and converted storage together.
    /// Retaining that reservation lets decoded payloads move without copying.
    #[cfg(feature = "wire")]
    pub(crate) fn from_admitted(bytes: Vec<u8>, charge: &Reservation) -> Self {
        Self(Arc::new(Storage {
            bytes,
            _charge: charge.clone(),
        }))
    }
    pub fn from_slice(bytes: &[u8], budget: &Budget) -> Result<Self> {
        let charge = budget.reserve(Self::retained_size(bytes.len())?)?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_| Error::ResourceLimit)?;
        if owned.capacity() != bytes.len() {
            return Err(Error::ResourceLimit);
        }
        owned.extend_from_slice(bytes);
        Ok(Self(Arc::new(Storage {
            bytes: owned,
            _charge: charge,
        })))
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Bits { width: u32, states: u8, data: Bytes },
    Real(u64),
    Bytes(Bytes),
}
impl Value {
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Bits { data, .. } | Self::Bytes(data) => {
                Bytes::retained_size(data.as_slice().len()).unwrap()
            }
            Self::Real(_) => 0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Sample {
    Known(Value),
    /// VTR defaults before the first change: two-state zero, other logic X,
    /// real zero, and empty bytes. Backends with different initial semantics
    /// must return their actual sample instead of this shorthand.
    BackendDefault(Kind),
    /// Events have occurrences, not a held value.
    Event,
}
impl Sample {
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Known(value) => value.retained_bytes(),
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub struct Predecessor {
    pub sample: Sample,
    pub(crate) _charge: Reservation,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub time: u64,
    pub value: Value,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub records: usize,
    pub work: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

/// Only `Exhausted` proves no matching timestamp exists. A partial search never
/// exposes a candidate that subsequent scan work might replace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeSearchResult {
    Pending,
    Found(u64),
    Exhausted,
}

#[derive(Debug)]
pub struct ChangeSearchPage {
    pub result: ChangeSearchResult,
    pub(crate) _charge: Reservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalTime {
    pub signal: u32,
    pub time: u64,
}

#[derive(Clone, Debug)]
pub struct SampleAt {
    pub pair: SignalTime,
    pub sample: Sample,
}

/// Completed samples in the caller's pair order. A partial page does not
/// authorize interpreting any of the remaining pairs as unavailable.
#[derive(Debug)]
pub struct ValuesPage {
    pub offset: usize,
    pub complete: bool,
    pub(crate) samples: Vec<SampleAt>,
    pub(crate) _charge: Reservation,
}
impl ValuesPage {
    pub fn samples(&self) -> &[SampleAt] {
        &self.samples
    }
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 4 * 1024 * 1024,
            records: 4096,
            work: 4096,
        }
    }
}

/// The interval is the query identity, not a claim that a partial page covers
/// it. Only the final page proves exhaustion; same-time events may span pages.
#[derive(Debug)]
pub struct WindowPage {
    pub interval: Interval,
    pub predecessor: Arc<Predecessor>,
    pub complete: bool,
    pub(crate) changes: Vec<Change>,
    pub(crate) _charge: Reservation,
}
impl WindowPage {
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }
}
