//! The stitch: block scans, in time order, become index sections.

use super::format::{
    self, BLOCK_COLUMNS, BlockHead, FileWriter, Header, KIND_BLOCK, KIND_HEADER, KIND_TAIL,
    TAIL_COLUMNS,
};
use super::scan::BlockScan;
use super::{Identity, Summary};
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
    blocks: u32,
    previous_end: Option<u64>,
    changes: u64,
    stretches: u64,
    cols: [Vec<u8>; BLOCK_COLUMNS],
    next: Vec<Sig>,
    raw: Vec<u8>,
    body: Vec<u8>,
    compressor: Compressor,
}

impl<W: Write> Builder<W> {
    pub fn new(out: W, identity: Identity, signals: u32, t_min: u64) -> Result<Builder<W>> {
        let header = Header {
            identity,
            signals,
            t_min,
        };
        let mut out = FileWriter::new(out)?;
        out.section(KIND_HEADER, &[&header.encode()], 0, 0)?;
        Ok(Builder {
            out,
            header,
            sigs: vec![Sig::default(); signals as usize],
            blocks: 0,
            previous_end: None,
            changes: 0,
            stretches: 0,
            cols: Default::default(),
            next: Vec::with_capacity(signals as usize),
            raw: Vec::new(),
            body: Vec::new(),
            compressor: Compressor::new(),
        })
    }

    pub(super) fn base_bytes(signals: u32) -> u64 {
        u64::from(signals) * 2 * std::mem::size_of::<Sig>() as u64
    }

    pub(super) fn release_workspace(&mut self) {
        self.cols = Default::default();
        self.raw = Vec::new();
        self.body = Vec::new();
    }

    pub fn blocks_added(&self) -> usize {
        self.blocks as usize
    }

    pub fn prev_end(&self) -> Option<u64> {
        self.previous_end
    }

    pub fn add(&mut self, mut scan: BlockScan) -> Result<()> {
        let block = scan.block;
        let count = self
            .blocks
            .checked_add(1)
            .ok_or(Error::invalid("too many activity blocks"))?;
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

        for stretch in &scan.stretches {
            if stretch.start < block.start || stretch.end > block.end {
                return Err(Error::invalid("activity change outside its block"));
            }
        }
        let mut changes = scan.changes;
        for row in &scan.rows {
            if self.sigs[row.signal as usize].last == scan.stretches[row.first].start {
                changes -= 1;
            }
        }
        let (rows, closed) = self.encode(&scan)?;

        let head = BlockHead {
            start: block.start,
            end: block.end,
            bytes: block.bytes,
            rows,
            closed,
            exponent: 0,
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
        self.blocks = count;
        self.previous_end = Some(block.end);
        self.changes += changes;
        self.stretches += u64::from(closed);
        Ok(())
    }

    fn encode(&mut self, scan: &BlockScan) -> Result<(u32, u32)> {
        for column in &mut self.cols {
            column.clear();
        }
        self.next.clear();
        let (mut previous_signal, mut rows, mut closed) = (0, 0, 0u32);
        for row in &scan.rows {
            let stretches = &scan.stretches[row.first..row.first + row.count];
            let initial = self.sigs[row.signal as usize].last == 0;
            let mut state = self.sigs[row.signal as usize];
            let mut count = 0u32;
            for stretch in stretches {
                if state.last == 0 {
                    state.open = stretch.start;
                } else if stretch.start > state.last {
                    if stretch.start - state.last == 1 {
                        state.gap = 1;
                    } else {
                        close(
                            &mut self.cols,
                            state.open,
                            state.last,
                            stretch.start,
                            state.gap,
                        );
                        count = count
                            .checked_add(1)
                            .ok_or(Error::invalid("too many activity stretches"))?;
                        state.open = stretch.start;
                        state.gap = 0;
                    }
                }
                if stretch.end > stretch.start {
                    state.gap = 1;
                }
                state.last = stretch.end;
            }
            self.next.push(state);
            if initial || count != 0 {
                put_u64(&mut self.cols[0], u64::from(row.signal - previous_signal));
                previous_signal = row.signal;
                put_u64(
                    &mut self.cols[1],
                    (u64::from(count) << 1) | u64::from(initial),
                );
                if initial {
                    put_u64(&mut self.cols[2], stretches[0].start - scan.block.start);
                }
                rows += 1;
                closed = closed
                    .checked_add(count)
                    .ok_or(Error::invalid("too many activity stretches"))?;
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

    pub fn finish(self) -> Result<Summary> {
        self.finish_with_generators(Default::default(), None)
    }

    pub(super) fn finish_with_generators(
        mut self,
        generators: super::generators::Ranges,
        control: Option<&super::BuildControl>,
    ) -> Result<Summary> {
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
        let mut ranges = generators.into_iter().peekable();
        while let Some(&((generator, _), _)) = ranges.peek() {
            if let Some(control) = control {
                control.check()?;
            }
            for column in &mut self.cols {
                column.clear();
            }
            let mut previous = 0;
            let mut count = 0u32;
            while count < 4096 && ranges.peek().is_some_and(|&((id, _), _)| id == generator) {
                let ((_, begin), end) = ranges.next().unwrap();
                put_u64(&mut self.cols[0], begin - previous);
                put_u64(&mut self.cols[1], end - begin);
                previous = end;
                count += 1;
            }
            format::pack(
                &self.cols[..2],
                &mut self.raw,
                &mut self.compressor,
                &mut self.body,
            )?;
            let mut head = Vec::with_capacity(8);
            head.extend_from_slice(&generator.to_le_bytes());
            head.extend_from_slice(&count.to_le_bytes());
            self.out.section(4, &[&head, &self.body], 0, 0)?;
        }
        let bytes = self.out.finish()?;
        Ok(Summary {
            blocks: self.blocks,
            signals: self.header.signals,
            changes: self.changes,
            stretches: self.stretches + u64::from(rows),
            delta: (self.blocks != 0).then_some((1, 1)),
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
