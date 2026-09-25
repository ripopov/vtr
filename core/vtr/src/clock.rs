//! Clocks: declared steady stretches of rising edges (SPEC section 7.4).
//!
//! A clock is a stream of kind [`STREAM_KIND`] with one generator,
//! [`GENERATOR`]. Each transaction of that generator is a *stretch*: `begin`
//! is its first edge, `end` its last edge, and the attribute [`KEY_PERIOD`]
//! the spacing between them (absent for a stretch of a single edge). Edge
//! counts and cycle numbers are computed here, never stored.

use crate::error::{Error, Result};
use crate::hierarchy::NodeId;

/// Stream kind of a clock.
pub const STREAM_KIND: &str = "CLOCK";
/// Name of a clock stream's only generator.
pub const GENERATOR: &str = "edges";
/// Stretch attribute: spacing of its edges (a *time* value).
pub const KEY_PERIOD: &str = "vtr.period";
/// Stream attribute: path of the clock that the stream's stages are counted in (a *str*).
pub const KEY_CLOCK: &str = "vtr.clock";

/// A clock: an index into the writer's clocks or into [`Reader::clocks`](crate::Reader::clocks).
/// Both follow declaration order, so one file gives the same ids on both sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClockId(pub u32);

/// A clock declared in a file.
#[derive(Clone, Debug)]
pub struct ClockInfo {
    pub id: ClockId,
    /// The CLOCK stream; its name is the clock's name and its path the clock's path.
    pub stream: NodeId,
    /// The stream's `edges` generator.
    pub generator: NodeId,
    /// Full path of the stream joined with '.'.
    pub path: String,
}

/// One steady stretch: edges at `begin, begin + period, ..., end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub begin: u64,
    pub end: u64,
    /// Edge spacing; 0 for a stretch of a single edge.
    pub period: u64,
    /// Cycle number of `begin`: the number of edges in all earlier stretches.
    pub first_cycle: u64,
}

impl Stretch {
    /// Number of edges in the stretch.
    pub fn edges(&self) -> u64 {
        (self.end - self.begin).checked_div(self.period).map_or(1, |n| n + 1)
    }

    fn edge(&self, k: u64) -> u64 {
        self.begin + k * self.period
    }
}

/// The cycle that contains a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CycleAt {
    /// Number of the last edge at or before the time; the first recorded edge is cycle 0.
    pub cycle: u64,
    /// Time of that edge.
    pub edge: u64,
    /// Time of the following recorded edge, if any.
    pub next_edge: Option<u64>,
    /// Position within the cycle, in `[0, 1)`; 0 when there is no next edge.
    pub fraction: f64,
    /// The clock is not running in this cycle: it is the last edge of a clock that was
    /// stopped, or a gap between stretches longer than twice the periods on both sides.
    pub stopped: bool,
}

/// The complete, immutable history of one clock.
#[derive(Clone, Debug, Default)]
pub struct ClockTimeline {
    stretches: Vec<Stretch>,
    /// The last stretch was still running when the file was closed.
    open: bool,
}

impl ClockTimeline {
    /// Builds a timeline from `(begin, end, period)` stretches in any order.
    /// `open` marks the last one as running at close.
    pub fn new(mut raw: Vec<(u64, u64, u64)>, open: bool) -> Result<ClockTimeline> {
        raw.sort_unstable();
        let mut stretches = Vec::with_capacity(raw.len());
        let mut cycle = 0u64;
        let mut prev_end: Option<u64> = None;
        for (begin, end, period) in raw {
            if end < begin || (period == 0 && end != begin) || (period != 0 && (end - begin) % period != 0) {
                return Err(Error::Corrupt("clock stretch is not a whole number of periods"));
            }
            if prev_end.is_some_and(|p| begin <= p) {
                return Err(Error::Corrupt("clock stretches overlap"));
            }
            let s = Stretch { begin, end, period, first_cycle: cycle };
            cycle += s.edges();
            prev_end = Some(end);
            stretches.push(s);
        }
        Ok(ClockTimeline { open: open && !stretches.is_empty(), stretches })
    }

    pub fn stretches(&self) -> &[Stretch] {
        &self.stretches
    }

    /// True when the clock was still running when the file was closed.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Number of recorded edges.
    pub fn edge_count(&self) -> u64 {
        self.stretches.last().map_or(0, |s| s.first_cycle + s.edges())
    }

    /// Index of the stretch holding the last edge at or before `t`.
    fn stretch_at(&self, t: u64) -> Option<usize> {
        self.stretches.partition_point(|s| s.begin <= t).checked_sub(1)
    }

    /// Edge number within stretch `s` of the last edge at or before `t` (`t >= s.begin`).
    fn index_in(s: &Stretch, t: u64) -> u64 {
        (t - s.begin).checked_div(s.period).map_or(0, |k| k.min(s.edges() - 1))
    }

    /// Time of the edge after edge `k` of stretch `i`, if recorded.
    fn following(&self, i: usize, k: u64) -> Option<u64> {
        let s = &self.stretches[i];
        if k + 1 < s.edges() {
            Some(s.edge(k + 1))
        } else {
            self.stretches.get(i + 1).map(|n| n.begin)
        }
    }

    /// The cycle containing `t`, or `None` before the first edge.
    pub fn cycle_at(&self, t: u64) -> Option<CycleAt> {
        let i = self.stretch_at(t)?;
        let s = &self.stretches[i];
        let k = Self::index_in(s, t);
        let edge = s.edge(k);
        let next_edge = self.following(i, k);
        let fraction = match next_edge {
            Some(n) => (t - edge) as f64 / (n - edge) as f64,
            None => 0.0,
        };
        let stopped = if k + 1 < s.edges() {
            false
        } else {
            match self.stretches.get(i + 1) {
                Some(n) => {
                    let gap = n.begin - s.end;
                    (s.period == 0 || gap > 2 * s.period) && (n.period == 0 || gap > 2 * n.period)
                }
                None => !self.open,
            }
        };
        Some(CycleAt { cycle: s.first_cycle + k, edge, next_edge, fraction, stopped })
    }

    /// Time of edge `cycle`.
    pub fn edge(&self, cycle: u64) -> Option<u64> {
        let i = self.stretches.partition_point(|s| s.first_cycle <= cycle).checked_sub(1)?;
        let s = &self.stretches[i];
        let k = cycle - s.first_cycle;
        (k < s.edges()).then(|| s.edge(k))
    }

    /// First edge strictly after `t`.
    pub fn next_edge(&self, t: u64) -> Option<u64> {
        match self.stretch_at(t) {
            None => self.stretches.first().map(|s| s.begin),
            Some(i) => self.following(i, Self::index_in(&self.stretches[i], t)),
        }
    }

    /// Last edge strictly before `t`.
    pub fn prev_edge(&self, t: u64) -> Option<u64> {
        self.cycle_at(t.checked_sub(1)?).map(|c| c.edge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(tl: &ClockTimeline) -> Vec<u64> {
        tl.stretches().iter().flat_map(|s| (0..s.edges()).map(move |k| s.begin + k * s.period)).collect()
    }

    #[test]
    fn queries_match_brute_force() {
        let tl = ClockTimeline::new(vec![(20272, 49772, 500), (400, 19772, 334), (50772, 99772, 1000), (120000, 120000, 0)], false).unwrap();
        let all = edges(&tl);
        assert_eq!(tl.edge_count(), all.len() as u64);
        for (c, &e) in all.iter().enumerate() {
            assert_eq!(tl.edge(c as u64), Some(e));
        }
        assert_eq!(tl.edge(all.len() as u64), None);
        for t in (0..121000).step_by(37) {
            let last = all.iter().rposition(|&e| e <= t);
            match tl.cycle_at(t) {
                None => assert!(last.is_none()),
                Some(c) => {
                    let l = last.unwrap();
                    assert_eq!((c.cycle, c.edge, c.next_edge), (l as u64, all[l], all.get(l + 1).copied()));
                }
            }
            assert_eq!(tl.next_edge(t), all.iter().copied().find(|&e| e > t));
            assert_eq!(tl.prev_edge(t), all.iter().copied().rfind(|&e| e < t));
        }
    }

    #[test]
    fn stopped_follows_the_display_rule() {
        let tl = ClockTimeline::new(vec![(0, 100, 10), (105, 205, 10), (1000, 1100, 10)], true).unwrap();
        assert!(!tl.cycle_at(95).unwrap().stopped);
        assert!(!tl.cycle_at(102).unwrap().stopped, "a short gap is an ordinary cycle");
        assert!(tl.cycle_at(500).unwrap().stopped, "a gap longer than two periods is stopped");
        assert!(!tl.cycle_at(1100).unwrap().stopped, "an open clock still runs at close");
        let closed = ClockTimeline::new(vec![(0, 100, 10)], false).unwrap();
        assert!(closed.cycle_at(150).unwrap().stopped);
        assert!((tl.cycle_at(15).unwrap().fraction - 0.5).abs() < 1e-12);
    }

    #[test]
    fn rejects_inconsistent_stretches() {
        assert!(ClockTimeline::new(vec![(0, 15, 10)], false).is_err());
        assert!(ClockTimeline::new(vec![(0, 10, 0)], false).is_err());
        assert!(ClockTimeline::new(vec![(0, 100, 10), (100, 200, 10)], false).is_err());
    }
}
