//! Clocks: declared steady stretches of rising edges (SPEC section 7.4).

use crate::error::{Error, Result};
use crate::hierarchy::NodeId;

pub const GENERATOR_KIND: &str = "CLOCK";
pub const KEY_PERIOD: &str = "vtr.period";
pub const KEY_CLOCK: &str = "vtr.clock";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClockId(pub u32);

#[derive(Clone, Debug)]
pub struct ClockInfo {
    pub id: ClockId,
    pub generator: NodeId,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub begin: u64,
    pub end: u64,
    pub period: u64,
    pub first_cycle: u64,
}

impl Stretch {
    #[must_use]
    pub fn edges(&self) -> u64 {
        (self.end - self.begin)
            .checked_div(self.period)
            .map_or(1, |n| n + 1)
    }

    fn edge(&self, k: u64) -> u64 {
        self.begin + k * self.period
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CycleAt {
    pub cycle: u64,
    pub edge: u64,
    pub next_edge: Option<u64>,
    pub fraction: f64,
    pub stopped: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ClockTimeline {
    stretches: Vec<Stretch>,
    open: bool,
}

impl ClockTimeline {
    pub fn new(mut raw: Vec<(u64, u64, u64)>, open: bool) -> Result<ClockTimeline> {
        raw.sort_unstable();
        let mut stretches = Vec::with_capacity(raw.len());
        let mut cycle = 0u64;
        let mut prev_end: Option<u64> = None;
        for (begin, end, period) in raw {
            if end < begin
                || (period == 0 && end != begin)
                || (period != 0 && (end - begin) % period != 0)
            {
                return Err(Error::Corrupt(
                    "clock stretch is not a whole number of periods",
                ));
            }
            if prev_end.is_some_and(|p| begin <= p) {
                return Err(Error::Corrupt("clock stretches overlap"));
            }
            let s = Stretch {
                begin,
                end,
                period,
                first_cycle: cycle,
            };
            cycle += s.edges();
            prev_end = Some(end);
            stretches.push(s);
        }
        Ok(ClockTimeline {
            open: open && !stretches.is_empty(),
            stretches,
        })
    }

    #[must_use]
    pub fn stretches(&self) -> &[Stretch] {
        &self.stretches
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    #[must_use]
    pub fn edge_count(&self) -> u64 {
        self.stretches
            .last()
            .map_or(0, |s| s.first_cycle + s.edges())
    }

    fn stretch_at(&self, t: u64) -> Option<usize> {
        self.stretches
            .partition_point(|s| s.begin <= t)
            .checked_sub(1)
    }

    fn index_in(s: &Stretch, t: u64) -> u64 {
        (t - s.begin)
            .checked_div(s.period)
            .map_or(0, |k| k.min(s.edges() - 1))
    }

    fn following(&self, i: usize, k: u64) -> Option<u64> {
        let s = &self.stretches[i];
        if k + 1 < s.edges() {
            Some(s.edge(k + 1))
        } else {
            self.stretches.get(i + 1).map(|n| n.begin)
        }
    }

    #[must_use]
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
        Some(CycleAt {
            cycle: s.first_cycle + k,
            edge,
            next_edge,
            fraction,
            stopped,
        })
    }

    #[must_use]
    pub fn edge(&self, cycle: u64) -> Option<u64> {
        let i = self
            .stretches
            .partition_point(|s| s.first_cycle <= cycle)
            .checked_sub(1)?;
        let s = &self.stretches[i];
        let k = cycle - s.first_cycle;
        (k < s.edges()).then(|| s.edge(k))
    }

    #[must_use]
    pub fn next_edge(&self, t: u64) -> Option<u64> {
        match self.stretch_at(t) {
            None => self.stretches.first().map(|s| s.begin),
            Some(i) => self.following(i, Self::index_in(&self.stretches[i], t)),
        }
    }

    #[must_use]
    pub fn prev_edge(&self, t: u64) -> Option<u64> {
        self.cycle_at(t.checked_sub(1)?).map(|c| c.edge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(tl: &ClockTimeline) -> Vec<u64> {
        tl.stretches()
            .iter()
            .flat_map(|s| (0..s.edges()).map(move |k| s.begin + k * s.period))
            .collect()
    }

    #[test]
    fn queries_match_brute_force() {
        let tl = ClockTimeline::new(
            vec![
                (20272, 49772, 500),
                (400, 19772, 334),
                (50772, 99772, 1000),
                (120_000, 120_000, 0),
            ],
            false,
        )
        .unwrap();
        let all = edges(&tl);
        assert_eq!(tl.edge_count(), all.len() as u64);
        for (c, &e) in all.iter().enumerate() {
            assert_eq!(tl.edge(c as u64), Some(e));
        }
        assert_eq!(tl.edge(all.len() as u64), None);
        for t in (0..121_000).step_by(37) {
            let last = all.iter().rposition(|&e| e <= t);
            match tl.cycle_at(t) {
                None => assert!(last.is_none()),
                Some(c) => {
                    let l = last.unwrap();
                    assert_eq!(
                        (c.cycle, c.edge, c.next_edge),
                        (l as u64, all[l], all.get(l + 1).copied())
                    );
                }
            }
            assert_eq!(tl.next_edge(t), all.iter().copied().find(|&e| e > t));
            assert_eq!(tl.prev_edge(t), all.iter().copied().rfind(|&e| e < t));
        }
    }

    #[test]
    fn stopped_follows_the_display_rule() {
        let tl =
            ClockTimeline::new(vec![(0, 100, 10), (105, 205, 10), (1000, 1100, 10)], true).unwrap();
        assert!(!tl.cycle_at(95).unwrap().stopped);
        assert!(
            !tl.cycle_at(102).unwrap().stopped,
            "a short gap is an ordinary cycle"
        );
        assert!(
            tl.cycle_at(500).unwrap().stopped,
            "a gap longer than two periods is stopped"
        );
        assert!(
            !tl.cycle_at(1100).unwrap().stopped,
            "an open clock still runs at close"
        );
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
