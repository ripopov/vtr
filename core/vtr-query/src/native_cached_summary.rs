use crate::{
    native::Summary,
    native_cache::{Cache, Lookup},
    native_history::History,
    summary::{SummaryPage, WaveBin},
    wave::Limits,
    Budget, Cancellation, Error, Grid, Result,
};
use std::sync::Arc;

// Session slots prepay this inline state; boxing would add a separately admitted
// allocation each time a history is refused and a cold cursor is created.
#[allow(clippy::large_enum_variant)]
enum Source<'a> {
    Waiting,
    Warm(Arc<History>),
    Cold(Summary<'a>),
}
pub(crate) struct CachedSummary<'a> {
    reader: &'a vtr::Reader,
    signal: u32,
    grid: Grid,
    offset: u32,
    source: Source<'a>,
    lease: bool,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
}
impl<'a> CachedSummary<'a> {
    pub(crate) fn new(
        reader: &'a vtr::Reader,
        signal: u32,
        grid: Grid,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
        cache: &mut Cache<'a>,
    ) -> Result<Self> {
        cancellation.check()?;
        if limits.records == 0 || limits.work == 0 {
            return Err(Error::Invalid("zero record or work limit"));
        }
        let kind = reader
            .signal_kind(vtr::SignalId(signal))
            .map_err(|error| Error::Backend(error.to_string()))?;
        let lease = kind != vtr::SignalKind::Real && cache.acquire(signal)?;
        let source = if lease {
            Source::Waiting
        } else {
            Source::Cold(Summary::new(
                reader,
                signal,
                grid,
                limits,
                budget.clone(),
                cancellation.clone(),
            )?)
        };
        Ok(Self {
            reader,
            signal,
            grid,
            offset: 0,
            source,
            lease,
            limits,
            budget,
            cancellation,
        })
    }
    pub(crate) fn release(&mut self, cache: &mut Cache<'a>) {
        if self.lease {
            cache.release(self.signal);
            self.lease = false;
        }
    }
    pub(crate) fn next_page(&mut self, cache: &mut Cache<'a>) -> Result<Arc<SummaryPage>> {
        self.cancellation.check()?;
        if matches!(self.source, Source::Waiting) {
            // Admit even empty progress replies before advancing construction.
            if self.limits.bytes < std::mem::size_of::<SummaryPage>() {
                return Err(Error::ResourceLimit);
            }
            let charge = self.budget.reserve(std::mem::size_of::<SummaryPage>())?;
            let lookup = cache.advance(self.signal)?;
            self.cancellation.check()?;
            match lookup {
                Lookup::Pending => {
                    return Ok(Arc::new(SummaryPage {
                        grid: self.grid,
                        offset: 0,
                        complete: false,
                        bins: Vec::new(),
                        _charge: charge,
                    }))
                }
                Lookup::Ready(history) => self.source = Source::Warm(history),
                Lookup::Cold => {
                    self.source = Source::Cold(Summary::new(
                        self.reader,
                        self.signal,
                        self.grid,
                        self.limits,
                        self.budget.clone(),
                        self.cancellation.clone(),
                    )?);
                }
            }
        }
        if let Source::Cold(cold) = &mut self.source {
            return cold.next_page();
        }
        let Source::Warm(history) = &self.source else {
            unreachable!()
        };
        let overhead = std::mem::size_of::<SummaryPage>();
        let available = self
            .limits
            .bytes
            .checked_sub(overhead)
            .ok_or(Error::ResourceLimit)?;
        // Size the first bin before allocating row slots, so a wide bin fitting
        // by itself isn't crowded out by a generous record limit.
        let first = if self.offset < self.grid.count() {
            Some(history.index().discrete_summary(
                self.grid.bin(self.offset)?,
                available.saturating_sub(std::mem::size_of::<Arc<WaveBin>>()),
                &self.budget,
            )?)
        } else {
            None
        };
        let capacity = self
            .limits
            .records
            .min(self.limits.work)
            .min((self.grid.count() - self.offset) as usize)
            .min(
                available.saturating_sub(first.as_ref().map_or(0, |bin| bin.delivery_bytes()))
                    / std::mem::size_of::<Arc<WaveBin>>(),
            );
        if capacity == 0 && first.is_some() {
            return Err(Error::ResourceLimit);
        }
        let rows = capacity * std::mem::size_of::<Arc<WaveBin>>();
        let charge = self.budget.reserve(overhead + rows)?;
        let mut bins = Vec::new();
        bins.try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        if bins.capacity() != capacity {
            return Err(Error::ResourceLimit);
        }
        let mut remaining = available - rows;
        if let Some(first) = first {
            remaining -= first.delivery_bytes();
            bins.push(first);
        }
        while bins.len() < capacity {
            self.cancellation.check()?;
            match history.index().discrete_summary(
                self.grid.bin(self.offset + bins.len() as u32)?,
                remaining,
                &self.budget,
            ) {
                Ok(bin) => {
                    remaining -= bin.delivery_bytes();
                    bins.push(bin);
                }
                Err(Error::ResourceLimit) => break,
                Err(error) => return Err(error),
            }
        }
        self.cancellation.check()?;
        let offset = self.offset;
        self.offset += bins.len() as u32;
        Ok(Arc::new(SummaryPage {
            grid: self.grid,
            offset,
            complete: self.offset == self.grid.count(),
            bins,
            _charge: charge,
        }))
    }
}
