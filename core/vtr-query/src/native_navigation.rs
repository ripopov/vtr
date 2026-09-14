//! Bounded cold-path navigation over raw signal occurrences.
use crate::wave::{ChangeSearchPage, ChangeSearchResult, Direction, Limits};
use crate::{Budget, Cancellation, Error, Result};
use std::sync::Arc;
use vtr::{Reader, ScanAction, SignalId};

/// Find the closest raw change strictly before or after a timestamp. Multiple
/// occurrences at that timestamp are skipped together. This is timestamp
/// navigation, not an iterator over individual same-time occurrences.
///
/// The cold path scans forward and keeps only a candidate timestamp. A previous
/// search must prove exhaustion before exposing its candidate. Warm indexed
/// execution can produce the same result without scanning the prefix.
pub struct FindChange<'a> {
    scan: Option<vtr::ChangeScan<'a>>,
    direction: Direction,
    candidate: Option<u64>,
    result: ChangeSearchResult,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
}

impl<'a> FindChange<'a> {
    pub fn new(
        reader: &'a Reader,
        signal: u32,
        from: u64,
        direction: Direction,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        if limits.work == 0 || limits.records == 0 {
            return Err(Error::Invalid("zero record or work limit"));
        }
        if limits.bytes < std::mem::size_of::<ChangeSearchPage>() {
            return Err(Error::ResourceLimit);
        }
        let signal = SignalId(signal);
        reader
            .signal_kind(signal)
            .map_err(|error| Error::Backend(error.to_string()))?;
        let interval = match direction {
            Direction::Next => from.checked_add(1).map(|start| (start, u64::MAX)),
            Direction::Previous => from.checked_sub(1).map(|end| (0, end)),
        };
        let scan = interval
            .map(|(start, end)| reader.change_scan(signal, start, end))
            .transpose()
            .map_err(|error| Error::Backend(error.to_string()))?;
        Ok(Self {
            scan,
            direction,
            candidate: None,
            result: ChangeSearchResult::Pending,
            limits,
            budget,
            cancellation,
        })
    }

    pub fn next_page(&mut self) -> Result<Arc<ChangeSearchPage>> {
        self.cancellation.check()?;
        // Admit the result before advancing so allocation pressure cannot lose
        // progress or turn a discovered hit into an unrepeatable delivery.
        let charge = self
            .budget
            .reserve(std::mem::size_of::<ChangeSearchPage>())?;
        let mut work = self.limits.work;
        while self.result == ChangeSearchResult::Pending && work > 0 {
            self.cancellation.check()?;
            let Some(scan) = self.scan.as_mut() else {
                self.result = ChangeSearchResult::Exhausted;
                break;
            };
            let batch = work.min(256);
            work -= batch;
            let complete = scan
                .scan(batch, |time, _| {
                    self.candidate = Some(time);
                    match self.direction {
                        Direction::Next => ScanAction::StopAfter,
                        Direction::Previous => ScanAction::Continue,
                    }
                })
                .map_err(|error| Error::Backend(error.to_string()))?;
            if complete || (self.direction == Direction::Next && self.candidate.is_some()) {
                self.result = self
                    .candidate
                    .map_or(ChangeSearchResult::Exhausted, ChangeSearchResult::Found);
                self.scan = None;
            }
        }
        self.cancellation.check()?;
        Ok(Arc::new(ChangeSearchPage {
            result: self.result,
            _charge: charge,
        }))
    }
}
