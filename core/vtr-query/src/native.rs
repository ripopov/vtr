//! Native execution over the VTR reader. Response admission is bounded here;
//! decoder scratch/cache admission remains the reader's responsibility.
use crate::wave::{Bytes, Change, Kind, Limits, Predecessor, Sample, Value, WindowPage};
use crate::{Budget, Cancellation, Error, Interval, Result, TimeBound};
use std::sync::Arc;
use vtr::{Reader, ScanAction, SignalId, SignalKind, SignalValue};

fn backend(error: vtr::Error) -> Error {
    Error::Backend(error.to_string())
}
pub(crate) fn kind(kind: SignalKind) -> Kind {
    match kind {
        SignalKind::Bits { width, states } => Kind::Bits { width, states },
        SignalKind::Real => Kind::Real,
        SignalKind::VarLen => Kind::Bytes,
    }
}
pub(crate) fn payload_bytes(value: SignalValue<'_>) -> usize {
    match value {
        SignalValue::Bits { data, .. } | SignalValue::VarLen(data) => {
            Bytes::retained_size(data.len()).unwrap_or(usize::MAX)
        }
        SignalValue::Real(_) => 0,
    }
}
pub(crate) fn value(value: SignalValue<'_>, budget: &Budget) -> Result<Value> {
    Ok(match value {
        SignalValue::Bits {
            width,
            states,
            data,
        } => Value::Bits {
            width,
            states,
            data: Bytes::from_slice(data, budget)?,
        },
        SignalValue::Real(real) => Value::Real(real.to_bits()),
        SignalValue::VarLen(data) => Value::Bytes(Bytes::from_slice(data, budget)?),
    })
}

/// A native exact-window operation. Each call returns one bounded immutable
/// page. The caller controls continuation and never needs to collect all pages.
/// Dropping the operation releases its cursor and reader pins.
pub struct Window<'a> {
    interval: Interval,
    scan: Option<vtr::ChangeScan<'a>>,
    predecessor: Arc<Predecessor>,
    budget: Budget,
    limits: Limits,
    cancellation: Cancellation,
}
impl<'a> Window<'a> {
    pub fn new(
        reader: &'a Reader,
        signal: u32,
        interval: Interval,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        if limits.records == 0 || limits.work == 0 {
            return Err(Error::Invalid("zero record or work limit"));
        }
        let signal = SignalId(signal);
        let declared = reader.signal_kind(signal).map_err(backend)?;
        let predecessor_charge = budget.reserve(std::mem::size_of::<Predecessor>())?;
        let predecessor = if reader.signal_var_type(signal).map_err(backend)? == vtr::VarType::Event
        {
            Sample::Event
        } else if interval.start() == 0 {
            Sample::BackendDefault(kind(declared))
        } else {
            let previous = reader
                .value_at(signal, interval.start() - 1)
                .map_err(backend)?;
            if payload_bytes(previous.borrow()) > limits.bytes {
                return Err(Error::ResourceLimit);
            }
            Sample::Known(value(previous.borrow(), &budget)?)
        };
        let scan = if interval.is_empty() {
            None
        } else {
            let end = match interval.end() {
                TimeBound::Tick(t) => t - 1,
                TimeBound::AfterMax => u64::MAX,
            };
            Some(
                reader
                    .change_scan(signal, interval.start(), end)
                    .map_err(backend)?,
            )
        };
        Ok(Self {
            interval,
            scan,
            predecessor: Arc::new(Predecessor {
                sample: predecessor,
                _charge: predecessor_charge,
            }),
            budget,
            limits,
            cancellation,
        })
    }

    pub fn next_page(&mut self) -> Result<Arc<WindowPage>> {
        self.cancellation.check()?;
        let overhead = std::mem::size_of::<WindowPage>()
            + std::mem::size_of::<Predecessor>()
            + self.predecessor.sample.retained_bytes();
        let available = self
            .limits
            .bytes
            .checked_sub(overhead)
            .ok_or(Error::ResourceLimit)?;
        // Empty work slices need only a page header. Allocate row slots after
        // the first event reveals how much space its payload must retain.
        let mut charge = Some(self.budget.reserve(std::mem::size_of::<WindowPage>())?);
        let mut changes = Vec::new();
        let mut capacity = 0;
        let mut remaining = available;
        let mut failure = None;
        let mut page_full = false;
        let mut complete = self.scan.is_none();
        let mut work = self.limits.work;
        while !complete && !page_full && work > 0 {
            self.cancellation.check()?;
            let batch = work.min(256);
            work -= batch;
            complete =
                self.scan
                    .as_mut()
                    .unwrap()
                    .scan(batch, |time, raw| {
                        let bytes = payload_bytes(raw);
                        if capacity == 0 {
                            capacity = self.limits.records.min(self.limits.work).min(
                                available.saturating_sub(bytes) / std::mem::size_of::<Change>(),
                            );
                            if capacity == 0 || bytes > available {
                                failure = Some(Error::ResourceLimit);
                                page_full = true;
                                return ScanAction::StopBefore;
                            }
                            let row_bytes = capacity * std::mem::size_of::<Change>();
                            // No row allocation exists yet. Replace the preparation
                            // lease before allocating the exact admitted capacity.
                            drop(charge.take());
                            match self
                                .budget
                                .reserve(row_bytes + std::mem::size_of::<WindowPage>())
                            {
                                Ok(reservation) => charge = Some(reservation),
                                Err(error) => {
                                    failure = Some(error);
                                    page_full = true;
                                    return ScanAction::StopBefore;
                                }
                            }
                            if changes.try_reserve_exact(capacity).is_err()
                                || changes.capacity() != capacity
                            {
                                failure = Some(Error::ResourceLimit);
                                page_full = true;
                                return ScanAction::StopBefore;
                            }
                            remaining = available - row_bytes;
                        }
                        if bytes > remaining {
                            if changes.is_empty() {
                                failure = Some(Error::ResourceLimit);
                            }
                            page_full = true;
                            return ScanAction::StopBefore;
                        }
                        match value(raw, &self.budget) {
                            Ok(value) => {
                                remaining -= bytes;
                                changes.push(Change { time, value });
                            }
                            Err(error) => {
                                if changes.is_empty() {
                                    failure = Some(error);
                                }
                                page_full = true;
                                return ScanAction::StopBefore;
                            }
                        }
                        if changes.len() == capacity {
                            page_full = true;
                            ScanAction::StopAfter
                        } else {
                            ScanAction::Continue
                        }
                    })
                    .map_err(backend)?;
        }
        if let Some(error) = failure {
            return Err(error);
        }
        self.cancellation.check()?;
        Ok(Arc::new(WindowPage {
            interval: self.interval,
            predecessor: self.predecessor.clone(),
            complete,
            changes,
            _charge: charge.unwrap(),
        }))
    }
}

pub use crate::summary::SummaryPage;

/// Resumable cold-path summary scan. It retains one accumulator, never the
/// exact history. Warm indexed summaries can implement the same result type.
pub struct Summary<'a> {
    grid: crate::Grid,
    scan: vtr::ChangeScan<'a>,
    scan_done: bool,
    offset: u32,
    builder: Option<crate::summary::BinBuilder>,
    ready: Option<Arc<crate::summary::WaveBin>>,
    entry: Sample,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
}
impl<'a> Summary<'a> {
    pub fn new(
        reader: &'a Reader,
        signal: u32,
        grid: crate::Grid,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        let window = Window::new(
            reader,
            signal,
            grid.interval(),
            limits,
            budget.clone(),
            cancellation.clone(),
        )?;
        Ok(Self {
            grid,
            scan: window.scan.unwrap(),
            scan_done: false,
            offset: 0,
            builder: None,
            ready: None,
            entry: window.predecessor.sample.clone(),
            limits,
            budget,
            cancellation,
        })
    }
    pub fn next_page(&mut self) -> Result<Arc<SummaryPage>> {
        self.cancellation.check()?;
        let page_size = std::mem::size_of::<SummaryPage>();
        let available = self
            .limits
            .bytes
            .checked_sub(page_size)
            .ok_or(Error::ResourceLimit)?;
        let capacity = self
            .limits
            .records
            .min((self.grid.count() - self.offset) as usize)
            .min(
                available
                    / (std::mem::size_of::<crate::summary::WaveBin>()
                        + std::mem::size_of::<Arc<crate::summary::WaveBin>>()),
            );
        if capacity == 0 && self.offset < self.grid.count() {
            return Err(Error::ResourceLimit);
        }
        let row_bytes = capacity * std::mem::size_of::<Arc<crate::summary::WaveBin>>();
        let charge = self.budget.reserve(page_size + row_bytes)?;
        let mut bins = Vec::new();
        bins.try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        if bins.capacity() != capacity {
            return Err(Error::ResourceLimit);
        }
        let mut remaining = available - row_bytes;
        let first = self.offset;
        let mut work = self.limits.work;
        while bins.len() < capacity && (work > 0 || self.ready.is_some()) {
            self.cancellation.check()?;
            if let Some(ready) = &self.ready {
                let size = ready.delivery_bytes();
                if size > remaining {
                    if bins.is_empty() {
                        return Err(Error::ResourceLimit);
                    }
                    break;
                }
                remaining -= size;
                let ready = self.ready.take().unwrap();
                self.entry = ready.exit.clone();
                bins.push(ready);
                self.offset += 1;
                work = work.saturating_sub(1);
                continue;
            }
            if self.builder.is_none() {
                match crate::summary::BinBuilder::new(
                    self.grid.bin(self.offset)?,
                    self.entry.clone(),
                    &self.budget,
                ) {
                    Ok(builder) => self.builder = Some(builder),
                    Err(Error::ResourceLimit) if !bins.is_empty() => break,
                    Err(error) => return Err(error),
                }
            }
            let mut boundary = self.scan_done;
            let mut failure = None;
            if !self.scan_done {
                let batch = work.min(256);
                work -= batch;
                let end = self.grid.bin(self.offset)?.end();
                let builder = self.builder.as_mut().unwrap();
                self.scan_done = self
                    .scan
                    .scan(batch, |time, raw| {
                        if u128::from(time) >= end.wide() {
                            boundary = true;
                            return ScanAction::StopBefore;
                        }
                        if payload_bytes(raw) > self.limits.bytes {
                            failure = Some(Error::ResourceLimit);
                            return ScanAction::StopBefore;
                        }
                        match value(raw, &self.budget).and_then(|value| builder.push(time, value)) {
                            Ok(()) => ScanAction::Continue,
                            Err(error) => {
                                failure = Some(error);
                                ScanAction::StopBefore
                            }
                        }
                    })
                    .map_err(backend)?;
            } else {
                work -= 1;
            }
            if let Some(error) = failure {
                if error == Error::ResourceLimit && !bins.is_empty() {
                    break;
                }
                return Err(error);
            }
            if boundary || self.scan_done {
                self.ready = Some(self.builder.take().unwrap().finish());
            }
        }
        Ok(Arc::new(SummaryPage {
            grid: self.grid,
            offset: first,
            complete: self.offset == self.grid.count(),
            bins,
            _charge: charge,
        }))
    }
}
