//! Admitted construction and ownership of a warm history, without a second copy.
use crate::{native_index::SignalIndex, Budget, Cancellation, Error, Reservation, Result};
use std::sync::Arc;

pub struct History {
    data: vtr::SignalData,
    variable_type: vtr::VarType,
    _charge: Reservation,
}

impl History {
    /// A borrowed query view cannot outlive its history's byte reservation.
    pub fn index(&self) -> SignalIndex<'_> {
        SignalIndex::new(&self.data, self.variable_type)
    }

    pub(crate) fn value_at(&self, time: u64) -> Option<vtr::SignalValue<'_>> {
        (self.variable_type != vtr::VarType::Event).then(|| self.data.value_at(time))
    }

    pub(crate) fn initial(&self) -> Option<vtr::SignalValue<'_>> {
        (self.variable_type != vtr::VarType::Event).then(|| self.data.initial())
    }
}

/// A pinned exact range. Indices refer into the reader-owned immutable buffers;
/// no second timestamp or value array is built for a viewport.
pub(crate) struct HistoryWindow {
    history: Arc<History>,
    next: usize,
    end: usize,
}
impl HistoryWindow {
    pub(crate) fn new(history: Arc<History>, interval: crate::Interval) -> Self {
        let times = history.data.times();
        let next = times.partition_point(|&time| time < interval.start());
        let end = times.partition_point(|&time| u128::from(time) < interval.end().wide());
        Self { history, next, end }
    }

    pub(crate) fn scan(
        &mut self,
        work: usize,
        mut visit: impl FnMut(u64, vtr::SignalValue<'_>) -> vtr::ScanAction,
    ) -> bool {
        for _ in 0..work {
            if self.next == self.end {
                break;
            }
            let action = visit(
                self.history.data.times()[self.next],
                self.history.data.get(self.next),
            );
            if action == vtr::ScanAction::StopBefore {
                return false;
            }
            self.next += 1;
            if action == vtr::ScanAction::StopAfter {
                break;
            }
        }
        self.next == self.end
    }
}

/// Prepay output construction before allocating; return unused capacity at
/// completion. Reader caches and scratch require separate admission.
pub struct Build<'a> {
    load: Option<vtr::HistoryLoad<'a>>,
    charge: Option<Reservation>,
    variable_type: vtr::VarType,
    cancellation: Cancellation,
    complete: Option<Arc<History>>,
}

impl<'a> Build<'a> {
    pub fn new(
        reader: &'a vtr::Reader,
        signal: u32,
        bytes: usize,
        budget: &Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        let variable_type = reader
            .signal_var_type(vtr::SignalId(signal))
            .map_err(backend)?;
        let charge = budget.reserve(
            bytes
                .checked_add(std::mem::size_of::<History>())
                .ok_or(Error::ResourceLimit)?,
        )?;
        let load = reader
            .history_load(vtr::SignalId(signal), bytes)
            .map_err(backend)?;
        Ok(Self {
            load: Some(load),
            charge: Some(charge),
            variable_type,
            cancellation,
            complete: None,
        })
    }

    /// None means pending. ResourceLimit means use the bounded cold path;
    /// cancellation and errors discard partial construction and its admission.
    /// `blocks` is a decoder work count, not a wall-time limit.
    pub fn advance(&mut self, blocks: usize) -> Result<Option<Arc<History>>> {
        let result = self.advance_inner(blocks);
        if result.is_err() {
            self.load = None;
            self.charge = None;
            self.complete = None;
        }
        result
    }

    fn advance_inner(&mut self, blocks: usize) -> Result<Option<Arc<History>>> {
        self.cancellation.check()?;
        if blocks == 0 {
            return Err(Error::Invalid("zero history work limit"));
        }
        if let Some(history) = &self.complete {
            return Ok(Some(history.clone()));
        }
        let load = self.load.as_mut().ok_or(Error::ResourceLimit)?;
        let progress = load.advance(blocks).map_err(backend)?;
        self.cancellation.check()?;
        match progress {
            vtr::HistoryProgress::Pending => Ok(None),
            vtr::HistoryProgress::BudgetExceeded => Err(Error::ResourceLimit),
            vtr::HistoryProgress::Complete(data) => {
                let charge = self.charge.take().unwrap();
                charge.shrink(
                    data.retained_bytes()
                        .checked_add(std::mem::size_of::<History>())
                        .ok_or(Error::ResourceLimit)?,
                )?;
                let history = Arc::new(History {
                    data,
                    variable_type: self.variable_type,
                    _charge: charge,
                });
                self.load = None;
                self.complete = Some(history.clone());
                Ok(Some(history))
            }
        }
    }
}

fn backend(error: vtr::Error) -> Error {
    Error::Backend(error.to_string())
}
