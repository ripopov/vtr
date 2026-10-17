//! The stitch: block scans, in time order, become index sections.

use super::format::{
    self, BLOCK_COLUMNS, BlockHead, FileWriter, Header, KIND_BLOCK, KIND_HEADER, KIND_TAIL,
    TAIL_COLUMNS,
};
use super::scan::BlockScan;
use super::{Budget, Identity, Summary, bucket, threshold};
use crate::codec::Compressor;
use crate::error::{Error, Result};
use crate::varint::put_u64;
use std::io::Write;

#[derive(Clone, Copy, Debug, Default)]
struct Sig {
    last: u64,
    open: u64,
    gap: u64,
}

pub struct Builder<W: Write> {
    out: FileWriter<W>,
    header: Header,
    sigs: Vec<Sig>,
    ends: Vec<u64>,
    minima: Vec<(u32, u64)>,
    changes: u64,
    stretches: u64,
    delta: Option<(u64, u64)>,
    cols: [Vec<u8>; BLOCK_COLUMNS],
    next: Vec<Sig>,
    raw: Vec<u8>,
    body: Vec<u8>,
    compressor: Compressor,
}

impl<W: Write> Builder<W> {
    pub fn new(
        out: W,
        identity: Identity,
        signals: u32,
        t_min: u64,
        budget: Budget,
    ) -> Result<Builder<W>> {
        let header = Header {
            identity,
            signals,
            budget,
            t_min,
        };
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

    pub fn blocks_added(&self) -> usize {
        self.ends.len()
    }

    pub fn prev_end(&self) -> Option<u64> {
        self.ends.last().copied()
    }

    pub fn add(&mut self, mut scan: BlockScan) -> Result<()> {
        scan.close();
        let block = scan.block;
        if scan.t_min != self.header.t_min || scan.prev_end != self.prev_end() {
            return Err(Error::invalid(
                "activity scan made for another position in the trace",
            ));
        }
        if block.end < block.start || self.prev_end().is_some_and(|p| block.start < p) {
            return Err(Error::invalid(format!(
                "activity block {}..{} out of time order",
                block.start, block.end
            )));
        }
        scan.rows.sort_unstable_by_key(|r| r.signal);
        for w in scan.rows.windows(2) {
            if w[0].signal == w[1].signal {
                return Err(Error::invalid(format!(
                    "signal {} fed twice in one activity block",
                    w[0].signal
                )));
            }
        }
        if scan
            .rows
            .last()
            .is_some_and(|r| r.signal as usize >= self.sigs.len())
        {
            return Err(Error::invalid("activity scan names an unknown signal"));
        }

        let mut hist = scan.hist;
        let mut changes = scan.changes;
        for r in &scan.rows {
            let last = self.sigs[r.signal as usize].last;
            if last != 0 && r.first > last {
                hist[bucket(r.first - last)] += 1;
            } else if last != 0 {
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

        let head = BlockHead {
            start: block.start,
            end: block.end,
            bytes: block.bytes,
            rows,
            closed,
            exponent: j as u8,
        };
        self.out.section(
            KIND_BLOCK,
            &[&head.encode(), &self.body],
            block.start,
            block.end,
        )?;
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
        self.stretches += u64::from(closed);
        self.delta = Some(
            self.delta
                .map_or((delta, delta), |(a, z)| (a.min(delta), z.max(delta))),
        );
        Ok(())
    }

    fn min_delta_from(&self, t: u64) -> u64 {
        let k = self.ends.partition_point(|&e| e < t) as u32;
        let i = self.minima.partition_point(|m| m.0 < k);
        self.minima.get(i).map_or(u64::MAX, |m| m.1)
    }

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
            self.next.push(Sig {
                last: r.last,
                open,
                gap: gap.max(r.tail),
            });
            if first || n > 0 {
                put_u64(&mut self.cols[0], u64::from(r.signal - prev_sig));
                prev_sig = r.signal;
                put_u64(&mut self.cols[1], (u64::from(n) << 1) | u64::from(first));
                if first {
                    put_u64(&mut self.cols[2], r.first - scan.block.start);
                }
                rows += 1;
                closed += n;
            }
        }
        format::pack(
            &self.cols,
            &mut self.raw,
            &mut self.compressor,
            &mut self.body,
        )?;
        Ok((rows, closed))
    }

    pub fn finish(mut self) -> Result<Summary> {
        for c in &mut self.cols {
            c.clear();
        }
        let (mut prev_sig, mut rows) = (0u32, 0u32);
        for (i, s) in self.sigs.iter().enumerate() {
            if s.last != 0 {
                put_u64(&mut self.cols[0], i as u64 - u64::from(prev_sig));
                prev_sig = i as u32;
                put_u64(&mut self.cols[1], s.last - s.open);
                put_u64(&mut self.cols[2], s.gap);
                rows += 1;
            }
        }
        format::pack(
            &self.cols[..TAIL_COLUMNS],
            &mut self.raw,
            &mut self.compressor,
            &mut self.body,
        )?;
        let mut head = [0u8; format::TAIL_HEAD_LEN];
        head[0..4].copy_from_slice(&rows.to_le_bytes());
        self.out.section(KIND_TAIL, &[&head, &self.body], 0, 0)?;
        let bytes = self.out.finish()?;
        Ok(Summary {
            blocks: self.ends.len() as u32,
            signals: self.header.signals,
            changes: self.changes,
            stretches: self.stretches + u64::from(rows),
            delta: self.delta,
            bytes,
            source_bytes: self.header.identity.length,
        })
    }
}

fn close(cols: &mut [Vec<u8>; BLOCK_COLUMNS], open: u64, a: u64, b: u64, gap: u64) {
    put_u64(&mut cols[3], a - open);
    put_u64(&mut cols[4], b - a);
    put_u64(&mut cols[5], gap);
}
