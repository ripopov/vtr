//! Ordered raw waveform reduction. These bins are never exact histories and
//! transaction overlap counts must not use this waveform composition rule.
use crate::wave::{Change, Kind, Sample, Value};
use crate::{Budget, Error, Interval, Reservation, Result};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealSummary {
    /// IEEE bits of finite held-value extrema. Zero-duration intermediate
    /// values at a repeated timestamp do not contribute to these extrema.
    pub finite_min: Option<u64>,
    pub finite_max: Option<u64>,
    /// Counts classify source changes, excluding the entry sample; they are
    /// additive across adjacent bins, unlike counts of held segments.
    pub nan_changes: u64,
    pub positive_infinite_changes: u64,
    pub negative_infinite_changes: u64,
}
impl RealSummary {
    fn held(&mut self, bits: u64) {
        let value = f64::from_bits(bits);
        if !value.is_finite() {
            return;
        }
        if self
            .finite_min
            .is_none_or(|old| value.total_cmp(&f64::from_bits(old)).is_lt())
        {
            self.finite_min = Some(bits);
        }
        if self
            .finite_max
            .is_none_or(|old| value.total_cmp(&f64::from_bits(old)).is_gt())
        {
            self.finite_max = Some(bits);
        }
    }
    fn change(&mut self, bits: u64) -> Result<()> {
        let value = f64::from_bits(bits);
        let count = if value.is_nan() {
            &mut self.nan_changes
        } else if value == f64::INFINITY {
            &mut self.positive_infinite_changes
        } else if value == f64::NEG_INFINITY {
            &mut self.negative_infinite_changes
        } else {
            return Ok(());
        };
        *count = count.checked_add(1).ok_or(Error::ResourceLimit)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct WaveBin {
    pub interval: Interval,
    pub entry: Sample,
    pub exit: Sample,
    pub changes: u64,
    pub first: Option<Change>,
    pub last: Option<Change>,
    pub real: Option<RealSummary>,
    _charge: Reservation,
}
impl WaveBin {
    /// Conservative delivery accounting: shared value storage may appear in
    /// several fields but is charged only once by its owning Budget lease.
    pub fn delivery_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.entry.retained_bytes())
            .saturating_add(self.exit.retained_bytes())
            .saturating_add(self.first.as_ref().map_or(0, |c| c.value.retained_bytes()))
            .saturating_add(self.last.as_ref().map_or(0, |c| c.value.retained_bytes()))
    }

    /// Compose two complete, equally sized aligned child bins in time order.
    pub fn merge(left: &Self, right: &Self, budget: &Budget) -> Result<Arc<Self>> {
        let start = u128::from(left.interval.start());
        let middle = left.interval.end().wide();
        let end = right.interval.end().wide();
        let width = middle - start;
        if middle != u128::from(right.interval.start())
            || width == 0
            || end - middle != width
            || !width.is_power_of_two()
            || start % (2 * width) != 0
            || domain(&left.entry) != domain(&right.entry)
        {
            return Err(Error::Invalid("bins are not compatible aligned siblings"));
        }
        let changes = left
            .changes
            .checked_add(right.changes)
            .ok_or(Error::ResourceLimit)?;
        let real = match (&left.real, &right.real) {
            (Some(a), Some(b)) => {
                let mut out = a.clone();
                if let Some(value) = b.finite_min {
                    out.held(value);
                }
                if let Some(value) = b.finite_max {
                    out.held(value);
                }
                out.nan_changes = a
                    .nan_changes
                    .checked_add(b.nan_changes)
                    .ok_or(Error::ResourceLimit)?;
                out.positive_infinite_changes = a
                    .positive_infinite_changes
                    .checked_add(b.positive_infinite_changes)
                    .ok_or(Error::ResourceLimit)?;
                out.negative_infinite_changes = a
                    .negative_infinite_changes
                    .checked_add(b.negative_infinite_changes)
                    .ok_or(Error::ResourceLimit)?;
                Some(out)
            }
            (None, None) => None,
            _ => return Err(Error::Invalid("incompatible real summaries")),
        };
        let charge = budget.reserve(std::mem::size_of::<Self>())?;
        Ok(Arc::new(Self {
            interval: Interval::new(left.interval.start(), right.interval.end())?,
            entry: left.entry.clone(),
            exit: right.exit.clone(),
            changes,
            first: left.first.clone().or_else(|| right.first.clone()),
            last: right.last.clone().or_else(|| left.last.clone()),
            real,
            _charge: charge,
        }))
    }
}

// Packing may vary between compact and declared bit values; width must agree.
fn domain(sample: &Sample) -> (u8, u32) {
    match sample {
        Sample::Event => (0, 0),
        Sample::Known(Value::Real(_)) | Sample::BackendDefault(Kind::Real) => (1, 0),
        Sample::Known(Value::Bytes(_)) | Sample::BackendDefault(Kind::Bytes) => (2, 0),
        Sample::Known(Value::Bits { width, .. })
        | Sample::BackendDefault(Kind::Bits { width, .. }) => (3, *width),
    }
}

/// A constant-sized accumulator, independent of the number of scanned events.
/// It retains only entry, first, last and current values, sharing their storage.
pub struct BinBuilder {
    bin: WaveBin,
    held_since: u64,
}
impl BinBuilder {
    pub fn new(interval: Interval, entry: Sample, budget: &Budget) -> Result<Self> {
        if interval.is_empty() {
            return Err(Error::Invalid("summary bin is empty"));
        }
        let real = (domain(&entry).0 == 1).then(RealSummary::default);
        let charge = budget.reserve(std::mem::size_of::<WaveBin>())?;
        Ok(Self {
            held_since: interval.start(),
            bin: WaveBin {
                interval,
                exit: entry.clone(),
                entry,
                changes: 0,
                first: None,
                last: None,
                real,
                _charge: charge,
            },
        })
    }
    fn held(&mut self) {
        if let Some(real) = &mut self.bin.real {
            match &self.bin.exit {
                Sample::Known(Value::Real(bits)) => real.held(*bits),
                Sample::BackendDefault(Kind::Real) => real.held(0.0f64.to_bits()),
                _ => {}
            }
        }
    }
    pub fn push(&mut self, time: u64, value: Value) -> Result<()> {
        if !self.bin.interval.contains(time) || time < self.held_since {
            return Err(Error::Invalid("summary changes must be ordered inside bin"));
        }
        let sample = Sample::Known(value.clone());
        let event = matches!(self.bin.entry, Sample::Event);
        if !event && domain(&sample) != domain(&self.bin.entry) {
            return Err(Error::Invalid("summary value type changed"));
        }
        let count = self
            .bin
            .changes
            .checked_add(1)
            .ok_or(Error::ResourceLimit)?;
        if time > self.held_since {
            self.held();
        }
        if let (Some(real), Value::Real(bits)) = (&mut self.bin.real, &value) {
            real.change(*bits)?;
        }
        if !event {
            self.bin.exit = sample;
        }
        let change = Change { time, value };
        if self.bin.first.is_none() {
            self.bin.first = Some(change.clone());
        }
        self.bin.last = Some(change);
        self.bin.changes = count;
        self.held_since = time;
        Ok(())
    }
    pub fn finish(mut self) -> Arc<WaveBin> {
        if self.bin.interval.end().wide() > u128::from(self.held_since) {
            self.held();
        }
        Arc::new(self.bin)
    }
}

/// A bounded page of complete canonical bins. An empty, incomplete page means
/// more work is needed on the current bin, never that its interval is empty.
#[derive(Debug)]
pub struct SummaryPage {
    pub grid: crate::Grid,
    pub offset: u32,
    pub complete: bool,
    pub(crate) bins: Vec<Arc<crate::summary::WaveBin>>,
    pub(crate) _charge: crate::Reservation,
}
impl SummaryPage {
    pub fn bins(&self) -> &[Arc<crate::summary::WaveBin>] {
        &self.bins
    }
}
