//! What a folded group shows: the activity of its signals merged into one
//! row (`docs/wave_groups.html`, "What a folded group shows").
//!
//! Each pixel column counts the changes of every member inside it and
//! records whether any member is undefined there. The painter draws the
//! result in the bus shape without labels: a boundary per change, a band
//! where a column holds several, the X colour where a member is undefined.
//!
//! Views with few changes walk the members' visible changes ([`walk`]).
//! Busier views read a [`GroupSummary`]: per block of time, the members'
//! change count and whether one is undefined, built once on the load worker
//! for a folded group and shared by the panels that show it
//! (`docs/RATIONALE.md`, "Volna signal groups").

use std::sync::Arc;

use crate::data::value_view::ValueView;
use crate::data::{Bit, SignalHistory, SignalShape};
use crate::wave::viewport::Viewport;

/// A view with at most this many member changes is walked directly; a
/// folded group with more changes in total gets a [`GroupSummary`].
pub const WALK_MAX: usize = 16_384;
/// Blocks of the finest summary level, at most.
const SUMMARY_BLOCKS: u64 = 1 << 20;
/// Each coarser summary level merges this many blocks.
const FANOUT: usize = 16;

/// One pixel column of a folded group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Column {
    /// Changes of all members inside the column.
    pub changes: u32,
    /// Some member holds an undefined value somewhere in the column.
    pub undefined: bool,
}

/// Whether value `i` of `h` is an undefined logic value (`x`, `u`, `w`).
pub fn is_undefined(h: &dyn SignalHistory, i: Option<usize>) -> bool {
    match h.shape() {
        SignalShape::Event => false,
        SignalShape::Bit => h.bit(i) == Bit::X,
        _ => match h.value_view(i) {
            ValueView::Logic(l) => (0..l.width).any(|k| matches!(l.bit(k), b'x' | b'u' | b'w')),
            _ => false,
        },
    }
}

/// Every member's changes inside `vp`.
pub fn visible_changes(members: &[Arc<dyn SignalHistory>], vp: &Viewport) -> usize {
    let (start, end) = (vp.start.max(0.0) as u64, vp.end.max(0.0) as u64);
    members
        .iter()
        .map(|h| {
            let a = h.index_at(start).map_or(0, |i| i + 1);
            let b = h.index_at(end).map_or(0, |i| i + 1);
            b.saturating_sub(a)
        })
        .sum()
}

/// The column of `width` columns over `vp` that holds a change at `t`: a
/// column ends at the time under its right edge, as the bus painter samples.
fn column_of(vp: &Viewport, width: usize, t: u64) -> Option<usize> {
    let x = vp.x_of(t as f64, width as f64).ceil() - 1.0;
    (x >= 0.0 && x < width as f64).then_some(x as usize)
}

/// The merged activity of `members` in `width` pixel columns of `vp`, from
/// their changes inside it: O(members × log(changes) + visible changes).
pub fn walk(members: &[Arc<dyn SignalHistory>], vp: &Viewport, width: usize) -> Vec<Column> {
    let mut out = vec![Column::default(); width];
    if width == 0 || vp.width() <= 0.0 {
        return out;
    }
    let start = vp.time_at(0.0, width as f64);
    let end = vp.end;
    for h in members {
        let h = h.as_ref();
        let mut i = if start < 0.0 {
            None
        } else {
            h.index_at(start as u64)
        };
        // Columns from `from` on hold the value after change `i`.
        let mut from = 0usize;
        loop {
            let next = i.map_or(0, |i| i + 1);
            let t = (next < h.len())
                .then(|| h.time(next))
                .filter(|&t| t as f64 <= end);
            let to = t.map_or(Some(width - 1), |t| column_of(vp, width, t));
            if is_undefined(h, i)
                && let Some(to) = to
            {
                for c in &mut out[from.min(width)..=to.min(width - 1)] {
                    c.undefined = true;
                }
            }
            let Some(t) = t else { break };
            if let Some(x) = column_of(vp, width, t) {
                out[x].changes = out[x].changes.saturating_add(1);
                from = x;
            }
            i = Some(next);
        }
    }
    out
}

/// The members' changes and undefined stretches per block of `1 << shift`
/// ticks from `start`, with coarser levels of [`FANOUT`] blocks above.
/// Built on the load worker for a folded group whose members change more
/// than [`WALK_MAX`] times; the document shares it between the panels that
/// fold the same signals and charges it to the memory budget.
pub struct GroupSummary {
    key: Vec<usize>,
    start: u64,
    shift: u32,
    levels: Vec<SummaryLevel>,
    reservation: Option<crate::remote::memory::Reservation>,
}

struct SummaryLevel {
    /// Changes per block, saturating: only none, one and several matter.
    changes: Vec<u8>,
    /// One bit per block: some member is undefined in it.
    undefined: Vec<u64>,
}

impl SummaryLevel {
    fn new(blocks: usize) -> Self {
        Self {
            changes: vec![0; blocks],
            undefined: vec![0; blocks.div_ceil(64)],
        }
    }

    fn is_undefined(&self, b: usize) -> bool {
        self.undefined[b / 64] >> (b % 64) & 1 == 1
    }

    fn mark(&mut self, blocks: std::ops::RangeInclusive<usize>) {
        for b in blocks {
            self.undefined[b / 64] |= 1 << (b % 64);
        }
    }
}

/// Identity of a folded group's signals, in any order.
pub fn key(members: &[Arc<dyn SignalHistory>]) -> Vec<usize> {
    let mut key: Vec<usize> = members
        .iter()
        .map(super::analog::history_identity)
        .collect();
    key.sort_unstable();
    key
}

impl GroupSummary {
    /// Summarize `members` over the trace time range `range`.
    pub fn build(members: &[Arc<dyn SignalHistory>], range: (u64, u64)) -> Self {
        let (start, end) = (range.0, range.1.max(range.0));
        let span = end - start;
        let shift = (0..64)
            .find(|&s| (span >> s) < SUMMARY_BLOCKS)
            .unwrap_or(63);
        let blocks = (span >> shift) as usize + 1;
        let block = |t: u64| (t.clamp(start, end) - start) as usize >> shift;
        let mut level = SummaryLevel::new(blocks);
        for h in members {
            let h = h.as_ref();
            let mut from = block(start);
            let mut undefined = is_undefined(h, None);
            for i in 0..h.len() {
                let t = h.time(i);
                let b = block(t);
                if undefined {
                    level.mark(from..=b);
                }
                if t >= start && t <= end {
                    level.changes[b] = level.changes[b].saturating_add(1);
                }
                from = b;
                undefined = is_undefined(h, Some(i));
            }
            if undefined {
                level.mark(from..=blocks - 1);
            }
        }
        let mut levels = vec![level];
        while levels.last().is_some_and(|l| l.changes.len() > FANOUT) {
            let below = levels.last().unwrap();
            let n = below.changes.len().div_ceil(FANOUT);
            let mut above = SummaryLevel::new(n);
            for (b, chunk) in below.changes.chunks(FANOUT).enumerate() {
                above.changes[b] = chunk.iter().fold(0u8, |a, c| a.saturating_add(*c));
                if (b * FANOUT..(b * FANOUT + chunk.len())).any(|k| below.is_undefined(k)) {
                    above.mark(b..=b);
                }
            }
            levels.push(above);
        }
        Self {
            key: key(members),
            start,
            shift,
            levels,
            reservation: None,
        }
    }

    /// Charge the summary to a memory budget for as long as it lives.
    pub fn account(mut self, budget: &crate::remote::memory::MemoryBudget) -> anyhow::Result<Self> {
        self.reservation = Some(budget.reserve_object("the group summary", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        self.levels
            .iter()
            .map(|l| (l.changes.len() + l.undefined.len() * 8) as u64)
            .sum()
    }

    /// The identity of the signals it summarizes (see [`key`]).
    pub fn key(&self) -> &[usize] {
        &self.key
    }

    /// Columns of `vp` from the coarsest level with at least one block per
    /// column, or `None` when even the finest blocks are wider than a
    /// column (the view is then short enough to [`walk`]).
    pub fn columns(&self, vp: &Viewport, width: usize) -> Option<Vec<Column>> {
        if width == 0 || vp.width() <= 0.0 {
            return Some(vec![Column::default(); width]);
        }
        let per_column = vp.width() / width as f64;
        let (k, level) = self
            .levels
            .iter()
            .enumerate()
            .rev()
            .find(|(k, _)| self.block_ticks(*k) <= per_column)?;
        let ticks = self.block_ticks(k);
        let blocks = level.changes.len();
        let block_at = |t: f64| ((t - self.start as f64) / ticks).floor();
        let mut out = vec![Column::default(); width];
        for (x, column) in out.iter_mut().enumerate() {
            // Blocks that start inside the column's time span.
            let a = block_at(vp.time_at(x as f64, width as f64)) + 1.0;
            let b = block_at(vp.time_at((x + 1) as f64, width as f64));
            let (a, b) = (a.max(0.0) as usize, (b.max(-1.0) + 1.0) as usize);
            for blk in a.min(blocks)..b.min(blocks) {
                column.changes = column.changes.saturating_add(u32::from(level.changes[blk]));
                column.undefined |= level.is_undefined(blk);
            }
            // A column inside one block reads the block it lies in.
            if a >= b
                && let Some(blk) = (b.checked_sub(1)).filter(|&blk| blk < blocks)
            {
                column.undefined |= level.is_undefined(blk);
            }
        }
        Some(out)
    }

    fn block_ticks(&self, level: usize) -> f64 {
        (1u64 << self.shift) as f64 * (FANOUT as f64).powi(level as i32)
    }
}

/// A folded group's summary, as the document holds it.
pub enum SummaryLoad {
    Building,
    Ready(Arc<GroupSummary>),
    /// Refused (the memory budget); the painter walks the changes instead.
    Failed,
}

/// A folded group's value cell at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// Loaded member signals.
    pub members: usize,
    /// Members that change exactly at the cursor.
    pub changed: usize,
    /// Members whose value at the cursor is undefined.
    pub undefined: usize,
}

impl Reading {
    pub fn at(members: &[Arc<dyn SignalHistory>], cursor: u64) -> Self {
        let mut reading = Self {
            members: members.len(),
            changed: 0,
            undefined: 0,
        };
        for h in members {
            let i = h.index_at(cursor);
            reading.changed += usize::from(i.is_some_and(|i| h.time(i) == cursor));
            reading.undefined += usize::from(is_undefined(h.as_ref(), i));
        }
        reading
    }

    /// "2 of 9 changed" on a change, otherwise "9 signals".
    pub fn text(&self) -> String {
        if self.changed > 0 {
            format!("{} of {} changed", self.changed, self.members)
        } else {
            format!(
                "{} signal{}",
                self.members,
                if self.members == 1 { "" } else { "s" }
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::WaveValue;
    use crate::data::history::VecHistory;

    fn changes(shape: SignalShape, at: &[(u64, &str)]) -> Arc<dyn SignalHistory> {
        Arc::new(VecHistory {
            shape,
            times: at.iter().map(|(t, _)| *t).collect(),
            values: at
                .iter()
                .map(|(_, v)| WaveValue::Bits((*v).into()))
                .collect(),
            initial: WaveValue::Unavailable,
        })
    }

    fn vp(start: f64, end: f64) -> Viewport {
        Viewport { start, end }
    }

    #[test]
    fn every_member_change_counts_in_its_column() {
        let a = changes(SignalShape::Bit, &[(0, "0"), (10, "1"), (20, "0")]);
        let b = changes(SignalShape::Bit, &[(0, "0"), (15, "1")]);
        let cols = walk(&[a, b], &vp(0.0, 40.0), 40);
        let at: Vec<usize> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| c.changes > 0)
            .map(|(x, _)| x)
            .collect();
        // A change at time t lands in the column that ends at t.
        assert_eq!(at, [9, 14, 19]);
        assert!(cols.iter().all(|c| !c.undefined));
    }

    #[test]
    fn a_one_cycle_x_survives_a_zoom_out() {
        let bus = changes(
            SignalShape::Vector { width: 8 },
            &[
                (0, "00000000"),
                (500_100, "xxxxxxxx"),
                (500_110, "00000001"),
            ],
        );
        let cols = walk(&[bus], &vp(0.0, 1_000_000.0), 200);
        let x: Vec<usize> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| c.undefined)
            .map(|(x, _)| x)
            .collect();
        assert_eq!(x, [100]);
        assert_eq!(cols[100].changes, 2);
    }

    #[test]
    fn an_undefined_stretch_colours_every_column_it_spans() {
        let bit = changes(SignalShape::Bit, &[(0, "x"), (10, "0")]);
        let cols = walk(&[bit], &vp(0.0, 20.0), 20);
        assert!(cols[..10].iter().all(|c| c.undefined));
        assert!(cols[10..].iter().all(|c| !c.undefined));
    }

    #[test]
    fn the_summary_draws_what_a_walk_draws_within_a_column() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let members: Vec<_> = (0..3)
            .map(|_| {
                let mut at = vec![(0, "00000000".to_owned())];
                let mut t = 0;
                while t < 99_000 {
                    t += 1 + rnd(40);
                    let v = if rnd(50) == 0 {
                        "xxxxxxxx".to_owned()
                    } else {
                        format!("{:08b}", rnd(256))
                    };
                    at.push((t, v));
                }
                let at: Vec<(u64, &str)> = at.iter().map(|(t, v)| (*t, v.as_str())).collect();
                changes(SignalShape::Vector { width: 8 }, &at)
            })
            .collect();
        let summary = GroupSummary::build(&members, (0, 100_000));
        for (start, end, width) in [(0.0, 100_000.0, 500), (20_000.0, 60_000.0, 800)] {
            let vp = vp(start, end);
            let walked = walk(&members, &vp, width);
            let summed = summary.columns(&vp, width).expect("blocks fit the columns");
            let total = |c: &[Column]| c.iter().map(|c| c.changes).sum::<u32>();
            // Blocks straddle column edges, so changes may move one column.
            for x in (0..width).step_by(10) {
                let r = x.saturating_sub(1)..(x + 11).min(width);
                let (a, b) = (total(&walked[r.clone()]), total(&summed[r]));
                assert!(
                    a.abs_diff(b) <= 2 * 4,
                    "{start}..{end} column {x}: {a} vs {b}"
                );
            }
            for x in 0..width {
                let near = |c: &[Column]| {
                    c[x.saturating_sub(1)..(x + 2).min(width)]
                        .iter()
                        .any(|c| c.undefined)
                };
                if walked[x].undefined {
                    assert!(
                        near(&summed),
                        "{start}..{end} column {x}: X walked, not summed"
                    );
                }
                if summed[x].undefined {
                    assert!(
                        near(&walked),
                        "{start}..{end} column {x}: X summed, not walked"
                    );
                }
            }
        }
        // Zoomed in past the finest blocks, the summary defers to a walk.
        assert!(summary.columns(&vp(50_000.0, 50_100.0), 800).is_none());
    }

    #[test]
    fn the_value_cell_counts_changes_and_undefined_members() {
        let a = changes(SignalShape::Bit, &[(0, "0"), (10, "1")]);
        let b = changes(SignalShape::Bit, &[(0, "x"), (10, "x"), (30, "0")]);
        let c = changes(SignalShape::Bit, &[(0, "0"), (20, "1")]);
        let members = [a, b, c];
        let r = Reading::at(&members, 10);
        assert_eq!((r.members, r.changed, r.undefined), (3, 2, 1));
        assert_eq!(r.text(), "2 of 3 changed");
        let r = Reading::at(&members, 25);
        assert_eq!((r.changed, r.undefined), (0, 1));
        assert_eq!(r.text(), "3 signals");
    }
}
