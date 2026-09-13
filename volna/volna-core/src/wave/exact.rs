//! Bounded exact coverage assembled from one validated query continuation chain.
//! Incomplete pages prove coverage strictly before their last received timestamp:
//! more changes at that same timestamp may still arrive on the following page.
use std::sync::Arc;
use vtr_query::{
    Budget, Error, Interval, Reservation, Result, TimeBound,
    session::{Continuation, Delivery, Reply},
    wave::{Change, Predecessor, Sample, WindowPage},
};

pub struct ExactWindow {
    pages: Vec<Arc<WindowPage>>,
    last: Option<Arc<Delivery>>,
    predecessor: Option<Arc<Predecessor>>,
    interval: Option<Interval>,
    last_time: Option<u64>,
    changes: usize,
    max_changes: usize,
    _charge: Reservation,
}
impl ExactWindow {
    pub fn new(max_changes: usize, budget: &Budget) -> Result<Self> {
        if max_changes == 0 || max_changes > 4096 {
            return Err(Error::Invalid("exact coverage needs 1..4096 changes"));
        }
        let charge = budget.reserve(max_changes * std::mem::size_of::<Arc<WindowPage>>())?;
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(max_changes)
            .map_err(|_| Error::ResourceLimit)?;
        Ok(Self {
            pages,
            last: None,
            predecessor: None,
            interval: None,
            last_time: None,
            changes: 0,
            max_changes,
            _charge: charge,
        })
    }
    /// Freeze a view of the accepted prefix. Raw pages and payloads remain
    /// shared; the independent page-reference array is admitted before copying.
    pub fn snapshot(&self, budget: &Budget) -> Result<Self> {
        let mut copy = Self::new(self.max_changes, budget)?;
        copy.pages.extend(self.pages.iter().cloned());
        copy.last = self.last.clone();
        copy.predecessor = self.predecessor.clone();
        copy.interval = self.interval;
        copy.last_time = self.last_time;
        copy.changes = self.changes;
        Ok(copy)
    }
    /// Accept the next page atomically. A retry of the same shared delivery is
    /// a no-op; rejected pages leave the accepted prefix and cursor unchanged.
    /// Empty work pages advance the cursor without accumulating page storage.
    pub fn append(&mut self, delivery: Arc<Delivery>) -> Result<bool> {
        if self
            .last
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &delivery))
        {
            return Ok(false);
        }
        let Reply::Window(page) = &delivery.reply else {
            return Err(Error::Invalid("expected exact window page"));
        };
        if let Some(last) = &self.last {
            if last.next != Some(delivery.request) || self.interval != Some(page.interval) {
                return Err(Error::Invalid(
                    "window page is outside the continuation chain",
                ));
            }
        } else if delivery.request.step != 0 {
            return Err(Error::Invalid(
                "exact window must begin with its first page",
            ));
        }
        if page.complete != delivery.next.is_none()
            || delivery.next.is_some_and(|next| {
                next.snapshot != delivery.request.snapshot
                    || next.operation != delivery.request.operation
                    || delivery.request.step.checked_add(1) != Some(next.step)
            })
        {
            return Err(Error::Invalid("invalid exact window continuation"));
        }
        if self.changes + page.changes().len() > self.max_changes {
            return Err(Error::ResourceLimit);
        }
        let mut last_time = self.last_time;
        for change in page.changes() {
            if !page.interval.contains(change.time)
                || last_time.is_some_and(|last| change.time < last)
            {
                return Err(Error::Invalid("unordered exact window changes"));
            }
            last_time = Some(change.time);
        }
        if self.interval.is_none() {
            self.interval = Some(page.interval);
            self.predecessor = Some(page.predecessor.clone());
        }
        self.last_time = last_time;
        self.changes += page.changes().len();
        if !page.changes().is_empty() {
            self.pages.push(page.clone());
        }
        self.last = Some(delivery);
        Ok(true)
    }
    pub fn next(&self) -> Option<Continuation> {
        self.last.as_ref().and_then(|last| last.next)
    }
    pub fn is_complete(&self) -> bool {
        self.last.as_ref().is_some_and(|last| last.next.is_none())
    }
    pub fn coverage(&self) -> Option<Interval> {
        let interval = self.interval?;
        let end = if self.is_complete() {
            interval.end()
        } else {
            TimeBound::Tick(self.last_time.unwrap_or(interval.start()))
        };
        Some(Interval::new(interval.start(), end).unwrap())
    }
    pub fn predecessor(&self) -> Option<&Sample> {
        self.predecessor.as_ref().map(|p| &p.sample)
    }
    pub fn changes(&self) -> impl DoubleEndedIterator<Item = &Change> {
        self.pages.iter().flat_map(|page| page.changes().iter())
    }
    /// A sample is available only in proven coverage. Events have occurrences,
    /// so they never produce a held sample. Payload clones share raw storage.
    pub fn sample_at(&self, time: u64) -> Option<Sample> {
        if !self.coverage()?.contains(time) || matches!(self.predecessor()?, Sample::Event) {
            return None;
        }
        let page = self
            .pages
            .partition_point(|page| page.changes()[0].time <= time);
        if page == 0 {
            return self.predecessor().cloned();
        }
        let changes = self.pages[page - 1].changes();
        let change = changes.partition_point(|change| change.time <= time) - 1;
        Some(Sample::Known(changes[change].value.clone()))
    }
}
