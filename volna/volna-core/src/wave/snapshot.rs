//! Immutable render input shared by displayed rows. Creation happens when query
//! state changes, never during painting. Payloads retain their query admission.
use super::{demand::Demand, exact::ExactWindow};
use crate::{Scene, Theme, data::WaveValue, geometry::Rect};
use std::sync::Arc;
use vtr_query::{
    Budget, Interval, Reservation, Result, session::SnapshotId, summary::WaveBin, wave::Sample,
};

pub struct WaveSnapshot {
    snapshot: SnapshotId,
    demand: Demand,
    bins: Vec<Arc<WaveBin>>,
    exact: Option<Arc<ExactWindow>>,
    _charge: Reservation,
}
impl WaveSnapshot {
    pub(crate) fn new(
        snapshot: SnapshotId,
        demand: Demand,
        bins: &[Arc<WaveBin>],
        exact: Option<Arc<ExactWindow>>,
        budget: &Budget,
    ) -> Result<Arc<Self>> {
        let charge = budget.reserve(std::mem::size_of::<Self>() + std::mem::size_of_val(bins))?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bins.len())
            .map_err(|_| vtr_query::Error::ResourceLimit)?;
        owned.extend(bins.iter().cloned());
        Ok(Arc::new(Self {
            snapshot,
            demand,
            bins: owned,
            exact,
            _charge: charge,
        }))
    }
    pub fn snapshot(&self) -> SnapshotId {
        self.snapshot
    }
    pub fn demand(&self) -> Demand {
        self.demand
    }
    pub fn sample_at(&self, time: u64) -> Option<Sample> {
        sample_at(&self.bins, self.exact.as_deref(), time)
    }
    pub fn value_at(&self, time: u64, max_bytes: usize) -> Result<Option<WaveValue>> {
        self.sample_at(time)
            .map(|sample| WaveValue::from_query_sample(&sample, max_bytes))
            .transpose()
    }
    pub fn paint(&self, viewport: Interval, row: Rect, theme: &Theme, scene: &mut Scene) {
        if let Some(exact) = &self.exact {
            super::bounded::paint_exact(exact, viewport, row, theme, scene);
        } else {
            super::bounded::paint_summary(&self.bins, viewport, row, theme, scene);
        }
    }
}
pub(crate) fn sample_at(
    bins: &[Arc<WaveBin>],
    exact: Option<&ExactWindow>,
    time: u64,
) -> Option<Sample> {
    if let Some(exact) = exact {
        return exact.sample_at(time);
    }
    let after = bins.partition_point(|bin| bin.interval.start() <= time);
    bins.get(after.checked_sub(1)?)
        .and_then(|bin| bin.sample_at(time))
}
