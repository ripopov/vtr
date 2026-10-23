use std::{collections::BTreeMap, sync::Arc};

use crate::{
    Compression, Error, Result,
    block::{self, BlockInput, ChunkEnc, ChunkInput, EncoderScratch, Record},
    codec::Compressor,
    hierarchy::SignalKind,
    signal,
    varint::put_blob,
};

#[derive(Default)]
pub(super) struct Waveform {
    pub kinds: Vec<SignalKind>,
    pub event: Vec<bool>,
    last: Vec<Vec<u8>>,
    emitted: Vec<bool>,
    frames: BTreeMap<u32, Vec<u8>>,
    times: Vec<u64>,
    used_time: bool,
    progress: bool,
    records: Vec<Record>,
    heap: Vec<u8>,
    counts: Vec<u32>,
    previous: Vec<u32>,
    blocks: u32,
    bytes: usize,
    peak_bytes: usize,
}

impl Waveform {
    pub fn validate_unsigned(&self, signal: u32) -> Result<()> {
        match self.kinds.get(signal as usize) {
            Some(SignalKind::Bits { width, .. }) if *width <= 64 => Ok(()),
            Some(_) => Err(Error::invalid(
                "integer emission requires a bit vector at most 64 bits wide",
            )),
            None => Err(Error::invalid("unknown signal")),
        }
    }
    pub fn add_signal(&mut self, kind: SignalKind, event: bool) -> Result<u32> {
        if let SignalKind::Bits { width, states } = kind
            && (width == 0 || width >= 1 << 31 || !matches!(states, 2 | 4 | 9))
        {
            return Err(Error::invalid("invalid signal kind"));
        }
        let id = u32::try_from(self.kinds.len())
            .map_err(|_| Error::invalid("signal identifier exhaustion"))?;
        let mut value = Vec::new();
        signal::default_value(kind, &mut value);
        self.kinds.push(kind);
        self.event.push(event);
        self.last.push(value);
        self.emitted.push(false);
        self.counts.push(0);
        Ok(id)
    }

    pub fn advance(&mut self, time: u64) -> Result<()> {
        if self.times.last().is_some_and(|last| *last > time) {
            return Err(Error::invalid("waveform time moved backwards"));
        }
        if self.times.last() == Some(&time) {
            return Ok(());
        }
        if !self.used_time && self.times.len() > 1 {
            *self.times.last_mut().unwrap() = time;
        } else {
            self.times.push(time);
        }
        self.used_time = false;
        self.progress = true;
        Ok(())
    }

    pub fn emit(
        &mut self,
        signal: u32,
        time: u64,
        value: super::SignalInput,
        group_size: u32,
        deduplicate: bool,
        enabled: bool,
    ) -> Result<()> {
        let kind = *self
            .kinds
            .get(signal as usize)
            .ok_or(Error::invalid("unknown signal"))?;
        let mut encoded = Vec::new();
        let mut canonical = Vec::new();
        let mut compact = false;
        match (kind, value) {
            (SignalKind::Bits { width, states }, super::SignalInput::Unsigned(value)) => {
                if width > 64 {
                    return Err(Error::invalid(
                        "integer emission requires a signal at most 64 bits wide",
                    ));
                }
                encoded.extend_from_slice(&value.to_le_bytes()[..(width as usize).div_ceil(8)]);
                signal::trim_top_bits(&mut encoded, width);
                compact = true;
                if states == 2 {
                    canonical.clone_from(&encoded);
                } else {
                    signal::widen(&encoded, width, 2, states, &mut canonical);
                }
            }
            (
                SignalKind::Bits { width, states },
                super::SignalInput::Packed {
                    alphabet,
                    width: expected_width,
                    bytes,
                },
            ) => {
                if expected_width.is_some_and(|expected| expected != width) {
                    return Err(Error::invalid(
                        "word emission width does not match its signal",
                    ));
                }
                if !matches!(alphabet, 2 | 4 | 9)
                    || bytes.len() != crate::signal::packed_len(width, alphabet)
                {
                    return Err(Error::invalid("invalid packed signal size or alphabet"));
                }
                let primitive = match alphabet {
                    2 => crate::schema::Primitive::Bits,
                    4 => crate::schema::Primitive::Logic,
                    _ => crate::schema::Primitive::Logic9,
                };
                primitive.validate_packed(&bytes, u64::from(width))?;
                for index in 0..width as usize {
                    if signal::get_code(&bytes, alphabet, index) >= states {
                        return Err(Error::invalid("value exceeds declared signal alphabet"));
                    }
                }
                if alphabet == states {
                    canonical = bytes;
                } else {
                    canonical.resize(crate::signal::packed_len(width, states), 0);
                    for index in 0..width as usize {
                        signal::set_code(
                            &mut canonical,
                            states,
                            index,
                            signal::get_code(&bytes, alphabet, index),
                        );
                    }
                }
                compact = signal::is_two_state(&canonical, width, states);
                if compact {
                    signal::narrow_to_two_state(&canonical, width, states, &mut encoded);
                } else {
                    encoded = canonical.clone();
                }
            }
            (SignalKind::Real, super::SignalInput::Real(bits)) => {
                encoded.extend_from_slice(&bits.to_le_bytes());
                canonical.clone_from(&encoded);
            }
            (SignalKind::VarLen, super::SignalInput::Bytes(bytes)) => {
                encoded = bytes;
                canonical.clone_from(&encoded);
            }
            _ => {
                return Err(Error::invalid(
                    "signal emission does not match its declaration",
                ));
            }
        }
        if !enabled {
            return Ok(());
        }
        self.advance(time)?;
        if deduplicate
            && !self.event[signal as usize]
            && self.emitted[signal as usize]
            && self.last[signal as usize] == canonical
        {
            return Ok(());
        }
        let group = signal / group_size;
        if !self.frames.contains_key(&group) {
            let mut frame = Vec::new();
            for id in group * group_size..((group + 1) * group_size).min(self.kinds.len() as u32) {
                if self.kinds[id as usize] == SignalKind::VarLen {
                    put_blob(&mut frame, &self.last[id as usize]);
                } else {
                    frame.extend_from_slice(&self.last[id as usize]);
                }
            }
            self.frames.insert(group, frame);
        }
        let payload = if kind == SignalKind::VarLen {
            let offset = self.heap.len() as u64;
            put_blob(&mut self.heap, &encoded);
            offset
        } else if kind.packed_len().unwrap() <= 8 {
            let mut bytes = [0; 8];
            bytes[..encoded.len()].copy_from_slice(&encoded);
            u64::from_le_bytes(bytes)
        } else {
            let offset = self.heap.len() as u64;
            self.heap.extend_from_slice(&encoded);
            offset
        };
        self.last[signal as usize] = canonical;
        self.emitted[signal as usize] = true;
        self.counts[signal as usize] += 1;
        let index = u32::try_from(self.times.len() - 1)
            .ok()
            .filter(|index| *index < block::COMPACT_FLAG)
            .ok_or(Error::invalid("waveform time table exhausted"))?;
        self.records.push(Record {
            sig: signal,
            tidx: index | if compact { block::COMPACT_FLAG } else { 0 },
            payload,
        });
        self.used_time = true;
        self.progress = true;
        self.bytes += 16 + encoded.len();
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        Ok(())
    }

    pub fn should_flush(&self, bytes: usize, records: usize) -> bool {
        self.bytes >= bytes || self.records.len() >= records
    }
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }

    pub fn prepare(&mut self, group_size: u32) -> Result<Option<(u64, u64, WaveJob)>> {
        if !self.progress {
            return Ok(None);
        }
        let kinds = Arc::new(self.kinds.clone());
        let input = ChunkInput {
            records: std::mem::take(&mut self.records),
            heap: std::mem::take(&mut self.heap),
            sig_counts: self.counts.clone(),
            kinds: kinds.clone(),
            wide: vec![],
            times: vec![],
            frame: vec![],
            frame_parts: vec![],
            ends_block: true,
        };
        let groups = (self.kinds.len() as u32).div_ceil(group_size);
        self.previous.resize(groups as usize, block::NO_BLOCK);
        let mut frame = Vec::new();
        let dirty_groups: Vec<_> = self.frames.keys().copied().collect();
        for bytes in self.frames.values() {
            frame.extend_from_slice(bytes);
        }
        let begin = self.times[0];
        let end = *self.times.last().unwrap();
        let block = BlockInput {
            block_index: self.blocks,
            times: std::mem::take(&mut self.times),
            kinds,
            n_signals: self.kinds.len() as u32,
            group_size,
            dirty_groups: dirty_groups.clone(),
            frame,
            prev_dirty: self.previous.clone(),
            run_budget: 64 << 10,
        };
        for group in dirty_groups {
            self.previous[group as usize] = self.blocks;
        }
        self.blocks = self
            .blocks
            .checked_add(1)
            .ok_or(Error::invalid("wave block count exhausted"))?;
        self.frames.clear();
        self.counts.fill(0);
        self.times.push(end);
        self.used_time = false;
        self.progress = false;
        self.bytes = 0;
        Ok(Some((begin, end, WaveJob { input, block })))
    }
}

pub(super) struct WaveJob {
    input: ChunkInput,
    block: BlockInput,
}
impl WaveJob {
    pub fn bytes(&self) -> usize {
        self.input.records.capacity() * std::mem::size_of::<Record>()
            + self.input.heap.capacity()
            + self.block.times.capacity() * 8
            + self.block.frame.capacity()
            + self.block.kinds.capacity() * std::mem::size_of::<SignalKind>()
            + self.input.sig_counts.capacity() * std::mem::size_of::<u32>()
            + self.block.prev_dirty.capacity() * std::mem::size_of::<u32>()
            + self.block.dirty_groups.capacity() * std::mem::size_of::<u32>()
    }
    pub fn encode(self, compression: Compression) -> Result<Vec<u8>> {
        let mut scratch = EncoderScratch::default();
        scratch.begin_block();
        let mut chunk = ChunkEnc::default();
        block::encode_chunk(&self.input, &self.block.kinds, &mut scratch, &mut chunk)?;
        let mut output = Vec::new();
        block::finish_block(
            &self.block,
            &[chunk],
            compression,
            &mut Compressor::new(),
            &mut scratch,
            &mut output,
        )?;
        output.extend_from_slice(scratch.data());
        Ok(output)
    }
}
