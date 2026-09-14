//! Warm queries over the reader's existing immutable history storage.
//! This borrowed view does not construct or admit that history. Its owner must
//! account for history construction, retained capacity and cache eviction.
use crate::{
    native::{payload_bytes, value},
    summary::WaveBin,
    wave::{Change, Direction, Sample},
    Budget, Error, Interval, Result,
};
use std::{ops::Range, sync::Arc};
use vtr::{SignalData, SignalKind, VarType};

pub struct SignalIndex<'a> {
    history: &'a SignalData,
    event: bool,
}
impl<'a> SignalIndex<'a> {
    /// The raw variable type must belong to this history's canonical signal.
    pub fn new(history: &'a SignalData, variable_type: VarType) -> Self {
        Self {
            history,
            event: variable_type == VarType::Event,
        }
    }

    fn range(&self, interval: Interval) -> Range<usize> {
        let times = self.history.times();
        times.partition_point(|&time| time < interval.start())
            ..times.partition_point(|&time| u128::from(time) < interval.end().wide())
    }

    pub fn find_change(&self, from: u64, direction: Direction) -> Option<u64> {
        let times = self.history.times();
        let index = match direction {
            Direction::Next => times.partition_point(|&time| time <= from),
            Direction::Previous => times.partition_point(|&time| time < from).checked_sub(1)?,
        };
        times.get(index).copied()
    }

    pub fn sample(&self, time: u64, bytes: usize, budget: &Budget) -> Result<Sample> {
        let payload = if self.event {
            0
        } else {
            payload_bytes(self.history.value_at(time))
        };
        if std::mem::size_of::<Sample>().saturating_add(payload) > bytes {
            return Err(Error::ResourceLimit);
        }
        if self.event {
            Ok(Sample::Event)
        } else {
            Ok(Sample::Known(value(self.history.value_at(time), budget)?))
        }
    }

    /// Discrete bins require only two timestamp searches and boundary values;
    /// they do not traverse the changes inside the bin. Real extrema require a
    /// separate range index and are deliberately not approximated here.
    pub fn discrete_summary(
        &self,
        interval: Interval,
        bytes: usize,
        budget: &Budget,
    ) -> Result<Arc<WaveBin>> {
        if self.history.kind() == SignalKind::Real {
            return Err(Error::Invalid(
                "real summaries require a range extrema index",
            ));
        }
        let range = self.range(interval);
        let first_index = (!range.is_empty()).then_some(range.start);
        let last_index = (!range.is_empty()).then(|| range.end - 1);
        let entry_raw = if self.event {
            None
        } else {
            Some(if range.start == 0 {
                self.history.initial()
            } else {
                self.history.get(range.start - 1)
            })
        };
        let entry_bytes = entry_raw.map_or(0, payload_bytes);
        let first_bytes = first_index.map_or(0, |index| payload_bytes(self.history.get(index)));
        let last_bytes = last_index.map_or(0, |index| payload_bytes(self.history.get(index)));
        let exit_bytes = if self.event {
            0
        } else if last_index.is_some() {
            last_bytes
        } else {
            entry_bytes
        };
        let size = [entry_bytes, first_bytes, last_bytes, exit_bytes]
            .into_iter()
            .try_fold(std::mem::size_of::<WaveBin>(), |total, n| {
                total.checked_add(n)
            })
            .ok_or(Error::ResourceLimit)?;
        if size > bytes {
            return Err(Error::ResourceLimit);
        }
        let charge = budget.reserve(std::mem::size_of::<WaveBin>())?;
        let entry = if self.event {
            Sample::Event
        } else if let Some(raw) = entry_raw {
            Sample::Known(value(raw, budget)?)
        } else {
            unreachable!("held signals always have an initial sample")
        };
        let change = |index| -> Result<Change> {
            Ok(Change {
                time: self.history.times()[index],
                value: value(self.history.get(index), budget)?,
            })
        };
        let first = first_index.map(change).transpose()?;
        let last = if first_index == last_index {
            first.clone()
        } else {
            last_index.map(change).transpose()?
        };
        let exit = if self.event {
            Sample::Event
        } else {
            last.as_ref()
                .map_or_else(|| entry.clone(), |last| Sample::Known(last.value.clone()))
        };
        Ok(Arc::new(WaveBin {
            interval,
            entry,
            exit,
            changes: range.len() as u64,
            first,
            last,
            real: None,
            _charge: charge,
        }))
    }
}
