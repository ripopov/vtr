//! The stitch: block scans, in time order, become index sections.

use super::format::{self, BlockHead, FileWriter, Header, BLOCK_COLUMNS, KIND_BLOCK, KIND_HEADER, KIND_TAIL, TAIL_COLUMNS};
use super::scan::BlockScan;
use super::{bucket, threshold, Budget, Identity, Summary};
use crate::codec::Compressor;
use crate::error::{Error, Result};
use crate::varint::put_u64;
use std::io::Write;

/// A signal's state between blocks: its last change (0 before the first one;
/// changes come after the first time step, so never at 0), the start of its
/// open stretch and that stretch's largest gap so far. 24 bytes per signal is
/// all the builder keeps across blocks.
#[derive(Clone, Copy, Debug, Default)]
struct Sig {
    last: u64,
    open: u64,
    gap: u64,
}

/// Writes an activity index from [`BlockScan`]s added in time order.
///
/// Each [`add`](Self::add) decides the block's threshold Δ, keeps the
/// silences that end in the block and are longer than the smallest Δ of the
/// blocks their interior touches, and appends one section with the stretches
/// they close; nothing of the block stays in memory afterwards.
/// [`finish`](Self::finish) writes the final stretch of every signal, the
/// directory and the trailer. The output is a pure function of the scans:
/// rebuilding gives the same bytes.
pub struct Builder<W: Write> {
    out: FileWriter<W>,
    header: Header,
    sigs: Vec<Sig>,
    /// Cell ends of the blocks added so far.
    ends: Vec<u64>,
    /// Suffix minima of the blocks' thresholds: `(block, Δ)` with Δ strictly
    /// increasing, so the smallest Δ from block `k` on is the first entry at
    /// or after `k`.
    minima: Vec<(u32, u64)>,
    changes: u64,
    stretches: u64,
    delta: Option<(u64, u64)>,
    /// Scratch: section columns, the states the section leaves behind, the
    /// packed body.
    cols: [Vec<u8>; BLOCK_COLUMNS],
    next: Vec<Sig>,
    raw: Vec<u8>,
    body: Vec<u8>,
    compressor: Compressor,
}

impl<W: Write> Builder<W> {
    /// Starts an index of a trace with `signals` signals whose first time
    /// step is `t_min`, and writes its header.
    pub fn new(out: W, identity: Identity, signals: u32, t_min: u64, budget: Budget) -> Result<Builder<W>> {
        let header = Header { identity, signals, budget, t_min };
        let mut out = FileWriter::new(out)?;
        out.section(KIND_HEADER, &[&header.encode()], 0, 0)?;
        Ok(Builder {
            out,
            header,
            sigs: vec![Sig::default(); signals as usize],
            ends: Vec::new(),
            minima: Vec::new(),
            changes: 0,
            stretches: 0,
            delta: None,
            cols: Default::default(),
            next: Vec::new(),
            raw: Vec::new(),
            body: Vec::new(),
            compressor: Compressor::new(),
        })
    }

    /// Blocks added so far.
    pub fn blocks_added(&self) -> usize {
        self.ends.len()
    }

    /// End of the last block added, the `prev_end` of the next scan.
    pub fn prev_end(&self) -> Option<u64> {
        self.ends.last().copied()
    }

    /// Stitches the next block of the trace. Its scan must have been made
    /// with this builder's `t_min` and budget and with [`prev_end`](Self::prev_end).
    pub fn add(&mut self, mut scan: BlockScan) -> Result<()> {
        scan.close();
        let block = scan.block;
        if scan.t_min != self.header.t_min || scan.prev_end != self.prev_end() {
            return Err(Error::invalid("activity scan made for another position in the trace"));
        }
        if block.end < block.start || self.prev_end().is_some_and(|p| block.start < p) {
            return Err(Error::invalid(format!("activity block {}..{} out of time order", block.start, block.end)));
        }
        scan.rows.sort_unstable_by_key(|r| r.signal);
        for w in scan.rows.windows(2) {
            if w[0].signal == w[1].signal {
                return Err(Error::invalid(format!("signal {} fed twice in one activity block", w[0].signal)));
            }
        }
        if scan.rows.last().is_some_and(|r| r.signal as usize >= self.sigs.len()) {
            return Err(Error::invalid("activity scan names an unknown signal"));
        }

        // The silences that cross into this block join its own histogram.
        let mut hist = scan.hist;
        let mut changes = scan.changes;
        for r in &scan.rows {
            let last = self.sigs[r.signal as usize].last;
            if last != 0 && r.first > last {
                hist[bucket(r.first - last)] += 1;
            } else if last != 0 {
                // One time step recorded at the end of the previous block too.
                changes -= 1;
            }
        }
        let mut j = threshold(&hist, scan.cap, scan.j_max);
        let disk = (self.header.budget.disk.max(0.0) * block.bytes as f64) as usize;
        let (mut rows, mut closed) = self.encode(&scan, 1u64 << j)?;
        while self.body.len() > disk && j < scan.j_max {
            j += 1;
            (rows, closed) = self.encode(&scan, 1u64 << j)?;
        }

        let head = BlockHead { start: block.start, end: block.end, bytes: block.bytes, rows, closed, exponent: j as u8 };
        self.out.section(KIND_BLOCK, &[&head.encode(), &self.body], block.start, block.end)?;
        for (r, s) in scan.rows.iter().zip(&self.next) {
            self.sigs[r.signal as usize] = *s;
        }
        let delta = 1u64 << j;
        let b = self.ends.len() as u32;
        while self.minima.last().is_some_and(|m| m.1 >= delta) {
            self.minima.pop();
        }
        self.minima.push((b, delta));
        self.ends.push(block.end);
        self.changes += changes;
        self.stretches += closed as u64;
        self.delta = Some(self.delta.map_or((delta, delta), |(a, z)| (a.min(delta), z.max(delta))));
        Ok(())
    }

    /// The smallest threshold of the blocks from the cell holding `t` up to
    /// the last one added (`u64::MAX` when `t` lies after them).
    fn min_delta_from(&self, t: u64) -> u64 {
        let k = self.ends.partition_point(|&e| e < t) as u32;
        let i = self.minima.partition_point(|m| m.0 < k);
        self.minima.get(i).map_or(u64::MAX, |m| m.1)
    }

    /// Encodes the block's section for threshold `delta` into `body` and the
    /// states it leaves in `next`; returns its rows and the stretches it closes.
    fn encode(&mut self, scan: &BlockScan, delta: u64) -> Result<(u32, u32)> {
        for c in &mut self.cols {
            c.clear();
        }
        self.next.clear();
        let (mut prev_sig, mut rows, mut closed) = (0u32, 0u32, 0u32);
        for r in &scan.rows {
            let s = self.sigs[r.signal as usize];
            let (mut open, mut gap, mut n) = (s.open, s.gap, 0u32);
            let first = s.last == 0;
            if first {
                open = r.first;
                gap = 0;
            } else if r.first > s.last {
                // The silence from the last change before the block. Its
                // interior starts at `last + 1` and ends in this block.
                let len = r.first - s.last;
                if len > self.min_delta_from(s.last.saturating_add(1)).min(delta) {
                    close(&mut self.cols, open, s.last, r.first, gap);
                    n += 1;
                    open = r.first;
                    gap = 0;
                } else {
                    gap = gap.max(len);
                }
            }
            // `r.first == s.last`: one time step recorded at a shared block boundary.
            for c in &scan.cands[r.cand0 as usize..(r.cand0 + r.ncand) as usize] {
                if c.b - c.a > delta {
                    close(&mut self.cols, open, c.a, c.b, gap.max(c.pre));
                    n += 1;
                    open = c.b;
                    gap = 0;
                } else {
                    gap = gap.max(c.pre).max(c.b - c.a);
                }
            }
            self.next.push(Sig { last: r.last, open, gap: gap.max(r.tail) });
            if first || n > 0 {
                put_u64(&mut self.cols[0], (r.signal - prev_sig) as u64);
                prev_sig = r.signal;
                put_u64(&mut self.cols[1], ((n as u64) << 1) | first as u64);
                if first {
                    put_u64(&mut self.cols[2], r.first - scan.block.start);
                }
                rows += 1;
                closed += n;
            }
        }
        format::pack(&self.cols, &mut self.raw, &mut self.compressor, &mut self.body)?;
        Ok((rows, closed))
    }

    /// Writes every signal's final stretch, the directory and the trailer.
    pub fn finish(mut self) -> Result<Summary> {
        for c in &mut self.cols {
            c.clear();
        }
        let (mut prev_sig, mut rows) = (0u32, 0u32);
        for (i, s) in self.sigs.iter().enumerate() {
            if s.last != 0 {
                put_u64(&mut self.cols[0], i as u64 - prev_sig as u64);
                prev_sig = i as u32;
                put_u64(&mut self.cols[1], s.last - s.open);
                put_u64(&mut self.cols[2], s.gap);
                rows += 1;
            }
        }
        format::pack(&self.cols[..TAIL_COLUMNS], &mut self.raw, &mut self.compressor, &mut self.body)?;
        let mut head = [0u8; format::TAIL_HEAD_LEN];
        head[0..4].copy_from_slice(&rows.to_le_bytes());
        self.out.section(KIND_TAIL, &[&head, &self.body], 0, 0)?;
        let bytes = self.out.finish()?;
        Ok(Summary {
            blocks: self.ends.len() as u32,
            signals: self.header.signals,
            changes: self.changes,
            stretches: self.stretches + rows as u64,
            delta: self.delta,
            bytes,
            source_bytes: self.header.identity.length,
        })
    }
}

/// Appends a closed stretch `[open, a]` followed by the kept silence `(a, b)`.
fn close(cols: &mut [Vec<u8>; BLOCK_COLUMNS], open: u64, a: u64, b: u64, gap: u64) {
    put_u64(&mut cols[3], a - open);
    put_u64(&mut cols[4], b - a);
    put_u64(&mut cols[5], gap);
}
