//! Exact change ranges for one source block, grouped by signal.

use super::Block;
use super::memory;
use crate::Result;

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
    pub changes: u64,
}

#[derive(Clone, Debug)]
pub struct BlockScan {
    pub(super) block: Block,
    pub(super) prev_end: Option<u64>,
    pub(super) t_min: u64,
    pub(super) rows: Vec<Row>,
    pub(super) stretches: Vec<Stretch>,
    limit: u64,
    workspace: u64,
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
            limit: u64::MAX,
            workspace: 0,
        }
    }
    #[must_use]
    pub fn estimate(rows: u64, points: u64) -> u64 {
        rows.saturating_mul(std::mem::size_of::<Row>() as u64)
            .saturating_add(points.saturating_mul(32))
    }
    pub(super) fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    pub(super) fn heap_bytes(&self) -> u64 {
        memory::bytes(&self.rows).saturating_add(memory::bytes(&self.stretches))
    }

    /// Admits temporary scan buffers in addition to the retained activity ranges.
    pub fn workspace(&mut self, bytes: u64) -> Result<()> {
        memory::check(self.heap_bytes().saturating_add(bytes), self.limit)?;
        self.workspace = bytes;
        Ok(())
    }

    #[inline]
    pub fn push(&mut self, signal: u32, time: u64) -> Result<()> {
        if time <= self.t_min {
            return Ok(());
        }
        if self.rows.last().is_none_or(|row| row.signal != signal) {
            let available = self
                .limit
                .saturating_sub(self.workspace)
                .saturating_sub(memory::bytes(&self.stretches));
            let length = self.rows.len() + 1;
            memory::reserve(&mut self.rows, length, available)?;
            self.rows.push(Row {
                signal,
                first: self.stretches.len(),
                count: 0,
                changes: 0,
            });
        }
        let row = self.rows.last_mut().unwrap();
        if row.count != 0 {
            let previous = self.stretches.last_mut().unwrap();
            if time <= previous.end {
                return Ok(());
            }
            if time - previous.end == 1 {
                previous.end = time;
                row.changes += 1;
                return Ok(());
            }
        }
        if self.stretches.len() == self.stretches.capacity() {
            let available = self
                .limit
                .saturating_sub(self.workspace)
                .saturating_sub(memory::bytes(&self.rows));
            let length = self.stretches.len() + 1;
            memory::reserve(&mut self.stretches, length, available)?;
        }
        let row = self.rows.last_mut().unwrap();
        self.stretches.push(Stretch {
            start: time,
            end: time,
        });
        row.count += 1;
        row.changes += 1;
        Ok(())
    }

    pub(super) fn aliases(&mut self, aliases: &[(u32, u32)]) -> Result<()> {
        // Canonical rows stay sorted and immutable while aliases append shared slices.
        let canonical = self.rows.len();
        let available = self
            .limit
            .saturating_sub(self.workspace)
            .saturating_sub(memory::bytes(&self.stretches));
        memory::reserve(
            &mut self.rows,
            canonical.saturating_add(aliases.len()),
            available,
        )?;
        for &(signal, target) in aliases {
            if let Ok(index) =
                self.rows[..canonical].binary_search_by_key(&target, |row| row.signal)
            {
                let row = self.rows[index];
                self.rows.push(Row { signal, ..row });
            }
        }
        Ok(())
    }
}
