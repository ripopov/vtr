//! Exact change ranges for one source block, grouped by signal.

use super::Block;

#[derive(Clone, Copy, Debug)]
pub(super) struct Stretch {
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Row {
    pub signal: u32,
    pub first: usize,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct BlockScan {
    pub(super) block: Block,
    pub(super) prev_end: Option<u64>,
    pub(super) t_min: u64,
    pub(super) rows: Vec<Row>,
    pub(super) stretches: Vec<Stretch>,
    pub(super) changes: u64,
}
impl BlockScan {
    #[must_use]
    pub fn new(block: Block, prev_end: Option<u64>, t_min: u64) -> Self {
        Self {
            block,
            prev_end,
            t_min,
            rows: Vec::new(),
            stretches: Vec::new(),
            changes: 0,
        }
    }
    #[must_use]
    pub fn estimate(rows: u64, points: u64) -> u64 {
        rows.saturating_mul(std::mem::size_of::<Row>() as u64)
            .saturating_add(points.saturating_mul(32))
    }
    pub fn push(&mut self, signal: u32, time: u64) {
        if time <= self.t_min {
            return;
        }
        if self.rows.last().is_none_or(|row| row.signal != signal) {
            self.rows.push(Row {
                signal,
                first: self.stretches.len(),
                count: 0,
            });
        }
        let row = self.rows.last_mut().unwrap();
        if row.count != 0 {
            let previous = self.stretches.last_mut().unwrap();
            if time <= previous.end {
                return;
            }
            if time - previous.end == 1 {
                previous.end = time;
                self.changes += 1;
                return;
            }
        }
        self.stretches.push(Stretch {
            start: time,
            end: time,
        });
        row.count += 1;
        self.changes += 1;
    }
}
