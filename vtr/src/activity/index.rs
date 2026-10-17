//! A loaded sidecar: every signal's busy stretches in one flat table.

use super::format::{
    self, BLOCK_COLUMNS, BLOCK_HEAD_LEN, BlockHead, Header, KIND_BLOCK, KIND_HEADER, KIND_TAIL,
    TAIL_COLUMNS, TAIL_HEAD_LEN,
};
use super::{Budget, Identity, MAGIC, STRETCH_BYTES, VERSION};
use crate::codec::Decompressor;
use crate::container::{Container, DirEntry};
use crate::error::{Error, Result};
use crate::hierarchy::SignalId;
use crate::varint;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexBlock {
    pub start: u64,
    pub end: u64,
    pub bytes: u64,
    pub delta: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub start: u64,
    pub end: u64,
    pub gap: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Classification {
    pub active: Vec<SignalId>,
    pub undecided: Vec<SignalId>,
}

#[derive(Clone, Debug)]
pub struct Index {
    header: Header,
    blocks: Vec<IndexBlock>,
    off: Vec<u32>,
    start: Vec<u64>,
    end: Vec<u64>,
    gap: Vec<u32>,
}

const GAP_SATURATED: u32 = u32::MAX;

fn corrupt<T>(msg: &'static str) -> Result<T> {
    Err(Error::Corrupt(msg))
}

impl Index {
    pub fn open(path: impl AsRef<Path>, trace: &Identity) -> Result<Index> {
        Self::decode(&std::fs::read(path)?, trace)
    }

    pub fn decode(bytes: &[u8], trace: &Identity) -> Result<Index> {
        let mut decode = std::pin::pin!(Self::decode_with(
            bytes,
            trace,
            |_| Ok(()),
            || std::future::ready(Ok(()))
        ));
        let waker = std::task::Waker::noop();
        match std::future::Future::poll(decode.as_mut(), &mut std::task::Context::from_waker(waker))
        {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("synchronous decoding never yields"),
        }
    }

    pub async fn decode_with<F: std::future::Future<Output = Result<()>>>(
        bytes: &[u8],
        trace: &Identity,
        mut admit: impl FnMut(u64) -> Result<()>,
        mut checkpoint: impl FnMut() -> F,
    ) -> Result<Index> {
        admit(bytes.len() as u64)?;
        let ((major, minor), entries) = Container::parse_closed(bytes, &MAGIC)?;
        if major != VERSION {
            return Err(Error::UnsupportedVersion {
                major,
                minor,
                supported: VERSION,
            });
        }
        let payload = |e: &DirEntry| Container::payload(bytes, e, true);
        let [first, rest @ .., tail] = entries.as_slice() else {
            return corrupt("activity index sections missing");
        };
        if first.kind != KIND_HEADER
            || tail.kind != KIND_TAIL
            || rest.iter().any(|e| e.kind != KIND_BLOCK)
        {
            return corrupt("activity index sections out of order");
        }
        let header = Header::decode(payload(first)?)?;
        if header.identity != *trace {
            return Err(Error::invalid(
                "the activity index was built for another trace",
            ));
        }
        let n = header.signals as usize;
        let base = (n as u64 + 1) * 4 + (rest.len() * std::mem::size_of::<IndexBlock>()) as u64;
        let directory = (entries.len() * std::mem::size_of::<DirEntry>()) as u64;
        format::tail_rows(payload(tail)?)?;
        let mut scratch = crate::codec::raw_len(&payload(tail)?[TAIL_HEAD_LEN..])? as u64;
        for e in rest {
            let p = payload(e)?;
            BlockHead::decode(p)?;
            scratch = scratch.max(crate::codec::raw_len(&p[BLOCK_HEAD_LEN..])? as u64);
            checkpoint().await?;
        }
        let peak = base
            .checked_add(directory)
            .and_then(|n| n.checked_add(scratch.saturating_mul(2)))
            .ok_or(Error::Corrupt("activity decode memory overflow"))?;
        admit(peak)?;
        let tail = payload(tail)?;
        let (mut d, mut raw) = (Decompressor::new(), Vec::new());

        let mut off = vec![0u32; n + 1];
        let mut blocks = Vec::with_capacity(rest.len());
        for e in rest {
            checkpoint().await?;
            let p = payload(e)?;
            let head = BlockHead::decode(p)?;
            if blocks
                .last()
                .is_some_and(|b: &IndexBlock| head.start < b.end)
            {
                return corrupt("activity index blocks out of time order");
            }
            blocks.push(IndexBlock {
                start: head.start,
                end: head.end,
                bytes: head.bytes,
                delta: 1 << head.exponent,
            });
            let c = format::unpack::<BLOCK_COLUMNS>(&p[BLOCK_HEAD_LEN..], &mut d, &mut raw)?;
            let (mut sigs, mut heads) = (col(&raw, c[0]), col(&raw, c[1]));
            let mut s = 0u64;
            for _ in 0..head.rows {
                checkpoint().await?;
                s += sigs.u64()?;
                let closed = heads.u64()? >> 1;
                let slot = off
                    .get_mut(s as usize + 1)
                    .filter(|_| s < n as u64)
                    .ok_or(Error::Corrupt("activity index names an unknown signal"))?;
                *slot = u32::try_from(closed)
                    .ok()
                    .and_then(|c| slot.checked_add(c))
                    .ok_or(Error::Corrupt("activity index too large"))?;
            }
        }
        let c = format::unpack::<TAIL_COLUMNS>(&tail[TAIL_HEAD_LEN..], &mut d, &mut raw)?;
        let mut sigs = col(&raw, c[0]);
        let mut s = 0u64;
        for _ in 0..format::tail_rows(tail)? {
            checkpoint().await?;
            s += sigs.u64()?;
            let slot = off
                .get_mut(s as usize + 1)
                .filter(|_| s < n as u64)
                .ok_or(Error::Corrupt("activity index names an unknown signal"))?;
            *slot = slot
                .checked_add(1)
                .ok_or(Error::Corrupt("activity index too large"))?;
        }
        for i in 0..n {
            checkpoint().await?;
            off[i + 1] = off[i]
                .checked_add(off[i + 1])
                .ok_or(Error::Corrupt("activity index too large"))?;
        }

        let total = off[n] as usize;
        admit(
            peak.checked_add((total as u64).saturating_mul(STRETCH_BYTES))
                .and_then(|v| v.checked_add(n as u64 * 5))
                .ok_or(Error::Corrupt("activity decode memory overflow"))?,
        )?;
        let (mut start, mut end, mut gap) =
            (vec![0u64; total], vec![0u64; total], vec![0u32; total]);
        let mut cursor = off[..n].to_vec();
        let mut opened = vec![false; n];
        for (e, blk) in rest.iter().zip(&blocks) {
            checkpoint().await?;
            let p = payload(e)?;
            let head = BlockHead::decode(p)?;
            let c = format::unpack::<BLOCK_COLUMNS>(&p[BLOCK_HEAD_LEN..], &mut d, &mut raw)?;
            let [
                mut sigs,
                mut heads,
                mut firsts,
                mut lens,
                mut sils,
                mut gaps,
            ] = c.map(|r| col(&raw, r));
            let mut s = 0usize;
            let mut closed_total = 0u64;
            for _ in 0..head.rows {
                checkpoint().await?;
                s += sigs.usize()?;
                let h = heads.u64()?;
                let i = cursor[s] as usize;
                if h & 1 != 0 {
                    if opened[s] || i >= off[s + 1] as usize {
                        return corrupt("activity index opens a signal twice");
                    }
                    opened[s] = true;
                    start[i] = blk
                        .start
                        .checked_add(firsts.u64()?)
                        .ok_or(Error::Corrupt("activity index time overflow"))?;
                } else if !opened[s] {
                    return corrupt(
                        "activity index closes a stretch of a signal that never changed",
                    );
                }
                for k in 0..h >> 1 {
                    checkpoint().await?;
                    let i = i + k as usize;
                    if i + 1 >= off[s + 1] as usize {
                        return corrupt("activity index stretch counts disagree");
                    }
                    let (len, sil) = (lens.u64()?, sils.u64()?);
                    let a = start[i].checked_add(len);
                    let b = a.and_then(|a| a.checked_add(sil).filter(|&b| b > a));
                    let (Some(a), Some(b)) = (a, b) else {
                        return corrupt("activity index stretch out of order");
                    };
                    end[i] = a;
                    gap[i] = gaps.u64()?.min(u64::from(GAP_SATURATED)) as u32;
                    start[i + 1] = b;
                }
                cursor[s] += (h >> 1) as u32;
                closed_total += h >> 1;
            }
            if closed_total != u64::from(head.closed)
                || !(sigs.is_empty()
                    && heads.is_empty()
                    && firsts.is_empty()
                    && lens.is_empty()
                    && sils.is_empty()
                    && gaps.is_empty())
            {
                return corrupt("activity index block columns disagree with its head");
            }
        }
        let c = format::unpack::<TAIL_COLUMNS>(&tail[TAIL_HEAD_LEN..], &mut d, &mut raw)?;
        let [mut sigs, mut lens, mut gaps] = c.map(|r| col(&raw, r));
        let mut s = 0usize;
        for _ in 0..format::tail_rows(tail)? {
            checkpoint().await?;
            s += sigs.usize()?;
            let i = cursor[s] as usize;
            if !opened[s] || i + 1 != off[s + 1] as usize {
                return corrupt("activity index tail disagrees with its blocks");
            }
            end[i] = start[i]
                .checked_add(lens.u64()?)
                .ok_or(Error::Corrupt("activity index time overflow"))?;
            gap[i] = gaps.u64()?.min(u64::from(GAP_SATURATED)) as u32;
            cursor[s] += 1;
        }
        for s in 0..n {
            checkpoint().await?;
            if cursor[s] != off[s + 1] || opened[s] != (off[s + 1] > off[s]) {
                return corrupt("activity index leaves a stretch open");
            }
        }
        Ok(Index {
            header,
            blocks,
            off,
            start,
            end,
            gap,
        })
    }

    #[must_use]
    pub fn identity(&self) -> Identity {
        self.header.identity
    }

    #[must_use]
    pub fn budget(&self) -> Budget {
        self.header.budget
    }

    #[must_use]
    pub fn t_min(&self) -> u64 {
        self.header.t_min
    }

    #[must_use]
    pub fn signal_count(&self) -> u32 {
        self.header.signals
    }

    #[must_use]
    pub fn blocks(&self) -> &[IndexBlock] {
        &self.blocks
    }

    #[must_use]
    pub fn stretch_count(&self) -> u64 {
        self.start.len() as u64
    }

    #[must_use]
    pub fn memory_bytes(&self) -> u64 {
        self.start.len() as u64 * STRETCH_BYTES
            + self.off.len() as u64 * 4
            + (self.blocks.len() * std::mem::size_of::<IndexBlock>()) as u64
    }

    #[must_use]
    pub fn exact_width(&self, t0: u64, t1: u64) -> u64 {
        let cell = |t: u64| {
            self.blocks
                .partition_point(|b| b.end < t)
                .min(self.blocks.len().saturating_sub(1))
        };
        if self.blocks.is_empty() {
            u64::MAX
        } else {
            self.blocks[cell(t0)..=cell(t1.max(t0))]
                .iter()
                .map(|b| b.delta)
                .min()
                .unwrap()
        }
    }

    #[must_use]
    pub fn classify(&self, t0: u64, t1: u64) -> Classification {
        let mut out = Classification::default();
        self.classify_range(0..self.header.signals, t0, t1, &mut out);
        out
    }

    pub fn classify_range(
        &self,
        signals: std::ops::Range<u32>,
        t0: u64,
        t1: u64,
        out: &mut Classification,
    ) {
        if t0 > t1 {
            return;
        }
        let width = (t1 - t0).saturating_add(1);
        let exact = self.exact_width(t0, t1) <= width;
        for s in signals {
            let (a, b) = (
                self.off[s as usize] as usize,
                self.off[s as usize + 1] as usize,
            );
            let j = a + self.end[a..b].partition_point(|&e| e < t0);
            if j == b || self.start[j] > t1 {
                continue;
            }
            let decided = exact
                || self.start[j] >= t0
                || self.end[j] <= t1
                || (self.gap[j] != GAP_SATURATED && u64::from(self.gap[j]) <= width);
            if decided {
                &mut out.active
            } else {
                &mut out.undecided
            }
            .push(SignalId(s));
        }
    }

    pub fn stretches(&self, signal: SignalId) -> impl ExactSizeIterator<Item = Stretch> + '_ {
        let s = signal.0 as usize;
        (self.off[s] as usize..self.off[s + 1] as usize).map(move |i| Stretch {
            start: self.start[i],
            end: self.end[i],
            gap: if self.gap[i] == GAP_SATURATED {
                u64::MAX
            } else {
                u64::from(self.gap[i])
            },
        })
    }
}

fn col(raw: &[u8], (a, b): (usize, usize)) -> varint::Reader<'_> {
    varint::Reader::new(&raw[a..b])
}
