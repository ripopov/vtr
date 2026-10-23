//! Source-bound exact activity ranges with bounded sidecar decoding.

use super::{
    Identity, container,
    format::{self, BLOCK_HEAD_LEN, BlockHead, Header, TAIL_HEAD_LEN},
};
use crate::{Error, Result, hierarchy::SignalId, trace::Reader, wire::Cursor};
use std::{future::Future, path::Path};

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
#[derive(Clone, Debug)]
pub struct Index {
    header: Header,
    blocks: Vec<IndexBlock>,
    signals: Vec<Vec<Stretch>>,
}

impl Index {
    pub fn open(path: impl AsRef<Path>, identity: &Identity) -> Result<Self> {
        Self::decode(&std::fs::read(path)?, identity)
    }
    pub fn decode(bytes: &[u8], identity: &Identity) -> Result<Self> {
        let mut future = std::pin::pin!(Self::decode_with(
            bytes,
            identity,
            |used| if used <= 256 << 20 {
                Ok(())
            } else {
                Err(Error::invalid(
                    "activity index exceeds the default memory limit",
                ))
            },
            || std::future::ready(Ok(()))
        ));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match Future::poll(future.as_mut(), &mut context) {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("synchronous sidecar decode does not yield"),
        }
    }
    pub fn decode_for(bytes: &[u8], reader: &Reader) -> Result<Self> {
        let index = Self::decode(bytes, &Identity::of(reader)?)?;
        index.verify_source(reader)?;
        Ok(index)
    }
    pub fn verify_source(&self, reader: &Reader) -> Result<()> {
        if self.identity() != Identity::of(reader)?
            || self.signal_count() != reader.signal_count()
            || self.blocks.len() != reader.block_count()
            || self.t_min()
                != reader
                    .waveform_time_range()
                    .map_or(0, |window| window.first)
        {
            return Err(Error::Corrupt("activity header does not match source"));
        }
        for (index, block) in self.blocks.iter().enumerate() {
            if (block.start, block.end) != reader.block_range(index)
                || block.bytes != reader.signal_block_bytes(index) + 24
            {
                return Err(Error::Corrupt("activity block does not match source"));
            }
        }
        Ok(())
    }
    /// Admits memory before allocation and checkpoints while initializing tables and decoding rows.
    pub async fn decode_with<F: Future<Output = Result<()>>>(
        bytes: &[u8],
        identity: &Identity,
        mut admit: impl FnMut(u64) -> Result<()>,
        mut checkpoint: impl FnMut() -> F,
    ) -> Result<Self> {
        let mut used = bytes.len() as u64;
        admit(used)?;
        let entries = container::parse(bytes, |extra| admit(used.saturating_add(extra)))?;
        used = used
            .checked_add((entries.capacity() * std::mem::size_of::<container::Entry>()) as u64)
            .ok_or(Error::Corrupt("activity memory overflow"))?;
        admit(used)?;
        let [first, rest @ .., tail] = entries.as_slice() else {
            return Err(Error::Corrupt("activity sections missing"));
        };
        if first.kind != 1
            || first.aux0 != 0
            || first.aux1 != 0
            || tail.kind != 3
            || tail.aux0 != 0
            || tail.aux1 != 0
            || rest.iter().any(|entry| entry.kind != 2)
        {
            return Err(Error::Corrupt("activity sections out of order"));
        }
        let header = Header::decode(container::payload(bytes, first)?)?;
        if header.identity != *identity {
            return Err(Error::invalid(
                "activity index belongs to another recording",
            ));
        }
        let count = header.signals as usize;
        used = used
            .checked_add((count as u64).saturating_mul(
                (std::mem::size_of::<Vec<Stretch>>() + std::mem::size_of::<Option<u64>>()) as u64,
            ))
            .and_then(|value| {
                value.checked_add((rest.len() * std::mem::size_of::<IndexBlock>()) as u64)
            })
            .ok_or(Error::Corrupt("activity memory overflow"))?;
        admit(used)?;
        let mut index = Self {
            header,
            blocks: Vec::with_capacity(rest.len()),
            signals: Vec::with_capacity(count),
        };
        let mut starts = Vec::with_capacity(count);
        for _ in 0..count {
            checkpoint().await?;
            index.signals.push(Vec::new());
            starts.push(None);
        }
        let mut decompressor = crate::codec::Decompressor::new();
        let mut raw = Vec::new();
        for entry in rest.iter().chain(std::iter::once(tail)) {
            checkpoint().await?;
            let body = container::payload(bytes, entry)?;
            let (rows, closed, columns) = if entry.kind == 2 {
                let head = BlockHead::decode(body)?;
                if head.bytes < 24
                    || (head.start, head.end) != (entry.aux0, entry.aux1)
                    || index
                        .blocks
                        .last()
                        .is_some_and(|last| head.start < last.end)
                    || (index.blocks.is_empty() && head.start != index.header.t_min)
                {
                    return Err(Error::Corrupt("invalid activity block bounds"));
                }
                index.blocks.push(IndexBlock {
                    start: head.start,
                    end: head.end,
                    bytes: head.bytes,
                    delta: 1,
                });
                let blob = &body[BLOCK_HEAD_LEN..];
                admit_blob(blob, &mut raw, used, &mut admit)?;
                (
                    head.rows,
                    head.closed,
                    format::unpack::<6>(blob, &mut decompressor, &mut raw)?,
                )
            } else {
                let rows = format::tail_rows(body)?;
                let blob = &body[TAIL_HEAD_LEN..];
                admit_blob(blob, &mut raw, used, &mut admit)?;
                let columns = format::unpack::<3>(blob, &mut decompressor, &mut raw)?;
                (
                    rows,
                    0,
                    [columns[0], columns[1], columns[2], (0, 0), (0, 0), (0, 0)],
                )
            };
            if rows as usize > count || (index.blocks.is_empty() && index.header.t_min != 0) {
                return Err(Error::Corrupt("invalid activity row count"));
            }
            let mut cols = columns.map(|(begin, end)| Cursor::new(&raw[begin..end]));
            let mut signal = 0usize;
            let mut observed_closed = 0u64;
            for row in 0..rows {
                checkpoint().await?;
                let delta = usize::try_from(cols[0].var()?)
                    .map_err(|_| Error::Corrupt("activity signal delta overflow"))?;
                signal = signal
                    .checked_add(delta)
                    .filter(|&value| value < count)
                    .ok_or(Error::Corrupt("unknown activity signal"))?;
                if row != 0 && delta == 0 {
                    return Err(Error::Corrupt("duplicate activity row"));
                }
                let head = if entry.kind == 2 { cols[1].var()? } else { 2 };
                if entry.kind == 2 {
                    if head == 0 {
                        return Err(Error::Corrupt("empty activity row"));
                    }
                    if head & 1 != 0 {
                        if starts[signal].is_some() {
                            return Err(Error::Corrupt("activity signal opened twice"));
                        }
                        let block = index.blocks.last().unwrap();
                        let first = block
                            .start
                            .checked_add(cols[2].var()?)
                            .filter(|&time| time > index.header.t_min && time <= block.end)
                            .ok_or(Error::Corrupt("activity first change outside source block"))?;
                        starts[signal] = Some(first);
                    }
                    observed_closed = observed_closed
                        .checked_add(head >> 1)
                        .filter(|&value| value <= u64::from(closed))
                        .ok_or(Error::Corrupt("activity closed count mismatch"))?;
                }
                for _ in 0..head >> 1 {
                    checkpoint().await?;
                    let begin =
                        starts[signal].ok_or(Error::Corrupt("activity stretch has no start"))?;
                    let (length, silence, gap) = if entry.kind == 2 {
                        (cols[3].var()?, Some(cols[4].var()?), cols[5].var()?)
                    } else {
                        (cols[1].var()?, None, cols[2].var()?)
                    };
                    let end = begin
                        .checked_add(length)
                        .ok_or(Error::Corrupt("activity time overflow"))?;
                    let maximum = index
                        .blocks
                        .last()
                        .ok_or(Error::Corrupt("activity rows without source blocks"))?
                        .end;
                    if end > maximum || gap > 1 || (length == 0) != (gap == 0) {
                        return Err(Error::Corrupt("invalid exact activity stretch"));
                    }
                    let list = &mut index.signals[signal];
                    if list.len() == list.capacity() {
                        let growth = list.capacity().max(1);
                        used = used
                            .checked_add(
                                (growth as u64)
                                    .saturating_mul(std::mem::size_of::<Stretch>() as u64),
                            )
                            .ok_or(Error::Corrupt("activity memory overflow"))?;
                        admit(used.saturating_add(raw.capacity() as u64))?;
                        list.try_reserve_exact(growth)
                            .map_err(|_| Error::Corrupt("activity allocation failed"))?;
                    }
                    list.push(Stretch {
                        start: begin,
                        end,
                        gap,
                    });
                    starts[signal] = match silence {
                        Some(gap) => Some(
                            end.checked_add(gap)
                                .filter(|&next| gap > 1 && next <= maximum)
                                .ok_or(Error::Corrupt("invalid activity silence"))?,
                        ),
                        None => None,
                    };
                }
            }
            if observed_closed != u64::from(closed) {
                return Err(Error::Corrupt("activity closed count mismatch"));
            }
            for col in cols {
                col.done()?;
            }
        }
        if starts.iter().any(Option::is_some) {
            return Err(Error::Corrupt("activity tail leaves stretches open"));
        }
        Ok(index)
    }
    #[must_use]
    pub fn identity(&self) -> Identity {
        self.header.identity
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
        self.signals.iter().map(|list| list.len() as u64).sum()
    }
    #[must_use]
    pub fn memory_bytes(&self) -> u64 {
        (self.signals.capacity() * std::mem::size_of::<Vec<Stretch>>()
            + self.blocks.capacity() * std::mem::size_of::<IndexBlock>()
            + self
                .signals
                .iter()
                .map(|list| list.capacity() * std::mem::size_of::<Stretch>())
                .sum::<usize>()) as u64
    }
    #[must_use]
    pub fn classify(&self, first: u64, last: u64) -> Vec<SignalId> {
        let mut classification = Vec::new();
        self.classify_range(0..self.signal_count(), first, last, &mut classification);
        classification
    }
    pub fn classify_range(
        &self,
        signals: std::ops::Range<u32>,
        first: u64,
        last: u64,
        output: &mut Vec<SignalId>,
    ) {
        if first > last {
            return;
        }
        for signal in signals.start..signals.end.min(self.signal_count()) {
            let list = &self.signals[signal as usize];
            let next = list.partition_point(|stretch| stretch.end < first);
            if list.get(next).is_some_and(|stretch| stretch.start <= last) {
                output.push(SignalId(signal));
            }
        }
    }
    pub fn stretches(&self, signal: SignalId) -> impl ExactSizeIterator<Item = Stretch> + '_ {
        self.signals
            .get(signal.0 as usize)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .copied()
    }
}

fn admit_blob(
    blob: &[u8],
    raw: &mut Vec<u8>,
    used: u64,
    admit: &mut impl FnMut(u64) -> Result<()>,
) -> Result<()> {
    let length = crate::codec::raw_len(blob)?;
    if length >= crate::wire::MAX_BLOCK_BYTES {
        return Err(Error::Corrupt("activity page exceeds format limit"));
    }
    admit(used.saturating_add(raw.capacity().max(length) as u64))?;
    raw.clear();
    raw.try_reserve_exact(length)
        .map_err(|_| Error::Corrupt("activity workspace allocation failed"))?;
    Ok(())
}
