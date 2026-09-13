//! Exact half-open intervals and canonical power-of-two aggregation grids.

use crate::{Error, Result};

const AFTER_MAX: u128 = 1u128 << 64;

/// An exclusive endpoint can include the final representable timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimeBound {
    Tick(u64),
    AfterMax,
}

impl TimeBound {
    pub fn wide(self) -> u128 {
        match self {
            Self::Tick(t) => u128::from(t),
            Self::AfterMax => AFTER_MAX,
        }
    }

    pub fn from_wide(t: u128) -> Result<Self> {
        match t {
            AFTER_MAX => Ok(Self::AfterMax),
            t if t < AFTER_MAX => Ok(Self::Tick(t as u64)),
            _ => Err(Error::Invalid("time exceeds AfterMax")),
        }
    }
}

/// `[start,end)`. An empty interval is valid and never overlaps a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Interval {
    start: u64,
    end: TimeBound,
}

impl Interval {
    pub fn new(start: u64, end: TimeBound) -> Result<Self> {
        if u128::from(start) > end.wide() {
            return Err(Error::Invalid("interval end precedes start"));
        }
        Ok(Self { start, end })
    }

    pub fn start(self) -> u64 {
        self.start
    }

    pub fn end(self) -> TimeBound {
        self.end
    }

    pub fn is_empty(self) -> bool {
        u128::from(self.start) == self.end.wide()
    }

    pub fn contains(self, time: u64) -> bool {
        self.start <= time && u128::from(time) < self.end.wide()
    }

    /// Positive-duration raw records use half-open overlap. Point records use
    /// containment, so a point at the query end is excluded.
    pub fn overlaps_record(self, begin: u64, end: u64) -> Result<bool> {
        if end < begin {
            return Err(Error::Invalid("record end precedes begin"));
        }
        Ok(!self.is_empty()
            && if begin == end {
                self.contains(begin)
            } else {
                u128::from(begin) < self.end.wide() && end > self.start
            })
    }
}

/// A validated grid. Bins are always aligned from tick zero and never clipped
/// to a recording's range. The level and global bin index are cache identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Grid {
    start: u64,
    level: u8,
    count: u32,
}

impl Grid {
    pub fn new(start: u64, level: u8, count: u32) -> Result<Self> {
        if level > 64 || count == 0 {
            return Err(Error::Invalid("grid needs level 0..64 and nonzero count"));
        }
        let width = 1u128 << level;
        if u128::from(start) % width != 0 {
            return Err(Error::Invalid("grid start is unaligned"));
        }
        let end = u128::from(start) + width * u128::from(count);
        if end > AFTER_MAX {
            return Err(Error::Invalid("grid exceeds AfterMax"));
        }
        Ok(Self {
            start,
            level,
            count,
        })
    }

    /// Round coverage outward at the finest level fitting `max_bins`.
    /// Empty demand returns no grid and should issue no query.
    pub fn covering(interval: Interval, max_bins: u32) -> Result<Option<Self>> {
        if max_bins == 0 {
            return Err(Error::Invalid("grid budget must be nonzero"));
        }
        if interval.is_empty() {
            return Ok(None);
        }
        for level in 0..=64 {
            let width = 1u128 << level;
            let first = u128::from(interval.start) / width;
            let after = interval.end.wide().div_ceil(width);
            let count = after - first;
            if count <= u128::from(max_bins) {
                return Self::new((first * width) as u64, level, count as u32).map(Some);
            }
        }
        unreachable!("one level-64 bin covers the entire time domain")
    }

    pub fn start(self) -> u64 {
        self.start
    }

    pub fn level(self) -> u8 {
        self.level
    }

    pub fn count(self) -> u32 {
        self.count
    }

    pub fn first_bin(self) -> u64 {
        (u128::from(self.start) >> self.level) as u64
    }

    pub fn interval(self) -> Interval {
        let end = u128::from(self.start) + (u128::from(self.count) << self.level);
        Interval {
            start: self.start,
            end: TimeBound::from_wide(end).unwrap(),
        }
    }

    pub fn bin(self, offset: u32) -> Result<Interval> {
        if offset >= self.count {
            return Err(Error::Invalid("bin offset is outside grid"));
        }
        let start = u128::from(self.start) + (u128::from(offset) << self.level);
        Interval::new(
            start as u64,
            TimeBound::from_wide(start + (1u128 << self.level))?,
        )
    }
}
