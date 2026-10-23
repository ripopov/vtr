use std::sync::Arc;

use super::capture::SignalRef;

use crate::{
    Compression, Error, Result,
    block::{self, BlockInput, EncodedColumns, EncoderScratch},
    codec::Compressor,
    hierarchy::SignalKind,
    signal,
    varint::{put_blob, put_u64},
};

#[derive(Clone, Copy)]
pub(super) struct Options {
    pub group_size: u32,
    pub deduplicate: bool,
    pub enabled: bool,
}

struct Encoded<'a> {
    kind: SignalKind,
    bytes: &'a [u8],
    canonical: &'a [u8],
    compact: bool,
    word: u64,
}

enum LastValue {
    Scalar(u64),
    Bytes(Vec<u8>),
}
impl LastValue {
    fn append_frame(&self, kind: SignalKind, frame: &mut Vec<u8>) {
        match self {
            Self::Scalar(word) => {
                frame.extend_from_slice(&word.to_le_bytes()[..kind.packed_len().unwrap()]);
            }
            Self::Bytes(bytes) if kind == SignalKind::VarLen => put_blob(frame, bytes),
            Self::Bytes(bytes) => frame.extend_from_slice(bytes),
        }
    }
}

#[inline]
fn append_scalar(output: &mut Vec<u8>, word: u64, length: usize) {
    let bytes = word.to_le_bytes();
    match length {
        1 => output.push(bytes[0]),
        2 => output.extend_from_slice(&bytes[..2]),
        4 => output.extend_from_slice(&bytes[..4]),
        8 => output.extend_from_slice(&bytes),
        _ => output.extend_from_slice(&bytes[..length]),
    }
}

#[derive(Default)]
struct ValueScratch {
    encoded: Vec<u8>,
    canonical: Vec<u8>,
}

struct Column {
    kind: SignalKind,
    event: bool,
    emitted: bool,
    last: LastValue,
    count: u32,
    headers: Vec<u8>,
    values: Vec<u8>,
    last_index: u32,
    last_code: u8,
}

#[derive(Default)]
pub(super) struct Waveform {
    pub kinds: Vec<SignalKind>,
    frames: Vec<Option<Vec<u8>>>,
    times: Vec<u64>,
    used_time: bool,
    progress: bool,
    columns: Vec<Column>,
    value_scratch: ValueScratch,
    records: usize,
    previous: Vec<u32>,
    blocks: u32,
    bytes: usize,
    peak_bytes: usize,
}

impl Waveform {
    pub fn validate_unsigned(&self, signal: u32) -> Result<()> {
        match self.columns.get(signal as usize).map(|column| column.kind) {
            Some(SignalKind::Bits { width, .. }) if width <= 64 => Ok(()),
            Some(_) => Err(Error::invalid(
                "integer emission requires a bit vector at most 64 bits wide",
            )),
            None => Err(Error::invalid("unknown signal")),
        }
    }
    pub fn event(&self, signal: u32) -> bool {
        self.columns[signal as usize].event
    }
    pub fn add_signal(&mut self, kind: SignalKind, event: bool) -> Result<u32> {
        if let SignalKind::Bits { width, states } = kind
            && (width == 0 || width >= 1 << 31 || !matches!(states, 2 | 4 | 9))
        {
            return Err(Error::invalid("invalid signal kind"));
        }
        let id = u32::try_from(self.kinds.len())
            .map_err(|_| Error::invalid("signal identifier exhaustion"))?;
        let value = match kind {
            SignalKind::Real
            | SignalKind::Bits {
                width: 1..=64,
                states: 2,
            } => LastValue::Scalar(0),
            _ => {
                let mut bytes = Vec::new();
                signal::default_value(kind, &mut bytes);
                LastValue::Bytes(bytes)
            }
        };
        self.kinds.push(kind);
        self.columns.push(Column {
            kind,
            event,
            emitted: false,
            last: value,
            count: 0,
            headers: Vec::new(),
            values: Vec::new(),
            last_index: 0,
            last_code: 255,
        });
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
        value: SignalRef<'_>,
        options: Options,
    ) -> Result<()> {
        let kind = self
            .columns
            .get(signal as usize)
            .ok_or_else(|| Error::invalid("unknown signal"))?
            .kind;
        match (kind, value) {
            (SignalKind::Bits { width, states: 2 }, SignalRef::Unsigned(value)) if width <= 64 => {
                let word = value & u64::MAX.checked_shr(64 - width).unwrap_or(0);
                return self.capture_unsigned(signal, time, width, word, options);
            }
            (SignalKind::Real, SignalRef::Real(bits)) => {
                let bytes = bits.to_le_bytes();
                return self.capture(
                    signal,
                    time,
                    Encoded {
                        kind,
                        bytes: &bytes,
                        canonical: &bytes,
                        compact: false,
                        word: bits,
                    },
                    options,
                );
            }
            _ => {}
        }
        let mut scratch = std::mem::take(&mut self.value_scratch);
        let result = self.emit_value(signal, time, kind, value, options, &mut scratch);
        for buffer in [&mut scratch.encoded, &mut scratch.canonical] {
            if buffer.capacity() > 64 << 10 {
                *buffer = Vec::new();
            }
        }
        self.value_scratch = scratch;
        result
    }

    fn emit_value(
        &mut self,
        signal: u32,
        time: u64,
        kind: SignalKind,
        value: SignalRef<'_>,
        options: Options,
        scratch: &mut ValueScratch,
    ) -> Result<()> {
        scratch.encoded.clear();
        scratch.canonical.clear();
        let (encoded, canonical, compact): (&[u8], &[u8], bool) = match (kind, value) {
            (SignalKind::Bits { width, states }, SignalRef::Unsigned(value)) => {
                if width > 64 {
                    return Err(Error::invalid(
                        "integer emission requires a signal at most 64 bits wide",
                    ));
                }
                scratch
                    .encoded
                    .extend_from_slice(&value.to_le_bytes()[..(width as usize).div_ceil(8)]);
                signal::trim_top_bits(&mut scratch.encoded, width);
                signal::widen(&scratch.encoded, width, 2, states, &mut scratch.canonical);
                (&scratch.encoded, &scratch.canonical, true)
            }
            (
                SignalKind::Bits { width, states },
                SignalRef::Packed {
                    alphabet,
                    width: expected,
                    bytes,
                },
            ) => {
                if expected.is_some_and(|expected| expected != width) {
                    return Err(Error::invalid(
                        "word emission width does not match its signal",
                    ));
                }
                if !matches!(alphabet, 2 | 4 | 9)
                    || bytes.len() != signal::packed_len(width, alphabet)
                {
                    return Err(Error::invalid("invalid packed signal size or alphabet"));
                }
                let primitive = match alphabet {
                    2 => crate::schema::Primitive::Bits,
                    4 => crate::schema::Primitive::Logic,
                    _ => crate::schema::Primitive::Logic9,
                };
                primitive.validate_packed(bytes, u64::from(width))?;
                if alphabet > states {
                    for index in 0..width as usize {
                        if signal::get_code(bytes, alphabet, index) >= states {
                            return Err(Error::invalid("value exceeds declared signal alphabet"));
                        }
                    }
                }
                let canonical = if alphabet == states {
                    bytes
                } else {
                    scratch
                        .canonical
                        .resize(signal::packed_len(width, states), 0);
                    for index in 0..width as usize {
                        signal::set_code(
                            &mut scratch.canonical,
                            states,
                            index,
                            signal::get_code(bytes, alphabet, index),
                        );
                    }
                    &scratch.canonical
                };
                let compact = alphabet == 2 || signal::is_two_state(canonical, width, states);
                let encoded = if alphabet == 2 {
                    bytes
                } else if compact {
                    signal::narrow_to_two_state(canonical, width, states, &mut scratch.encoded);
                    &scratch.encoded
                } else {
                    canonical
                };
                (encoded, canonical, compact)
            }
            (SignalKind::VarLen, SignalRef::Bytes(bytes)) => (bytes, bytes, false),
            _ => {
                return Err(Error::invalid(
                    "signal emission does not match its declaration",
                ));
            }
        };
        let word = if matches!(self.columns[signal as usize].last, LastValue::Scalar(_)) {
            canonical
                .iter()
                .enumerate()
                .fold(0u64, |word, (byte, &value)| {
                    word | (u64::from(value) << (byte * 8))
                })
        } else {
            0
        };
        self.capture(
            signal,
            time,
            Encoded {
                kind,
                bytes: encoded,
                canonical,
                compact,
                word,
            },
            options,
        )
    }

    fn capture(
        &mut self,
        signal: u32,
        time: u64,
        value: Encoded<'_>,
        options: Options,
    ) -> Result<()> {
        let Options {
            group_size,
            deduplicate,
            enabled,
        } = options;
        let Encoded {
            kind,
            bytes: encoded,
            canonical,
            compact,
            word,
        } = value;
        if !enabled {
            return Ok(());
        }
        self.advance(time)?;
        if deduplicate
            && !self.columns[signal as usize].event
            && self.columns[signal as usize].emitted
            && match &self.columns[signal as usize].last {
                LastValue::Scalar(previous) => *previous == word,
                LastValue::Bytes(previous) => previous == canonical,
            }
        {
            return Ok(());
        }
        let index = self.prepare_column(signal, group_size)?;
        let column = &mut self.columns[signal as usize];
        let delta = u64::from(index - column.last_index);
        match kind {
            SignalKind::Bits { width: 1, .. } => {
                let code = canonical[0] & 15;
                let header = if column.last_code <= 1 && code == column.last_code ^ 1 {
                    delta << 1
                } else {
                    (delta << 5) | (u64::from(code) << 1) | 1
                };
                put_u64(&mut column.headers, header);
                column.last_code = code;
            }
            SignalKind::Bits { .. } => {
                put_u64(&mut column.headers, (delta << 1) | u64::from(compact));
                if matches!(column.last, LastValue::Scalar(_)) {
                    append_scalar(&mut column.values, word, encoded.len());
                } else {
                    column.values.extend_from_slice(encoded);
                }
            }
            SignalKind::Real => {
                put_u64(&mut column.headers, delta);
                column.values.extend_from_slice(&word.to_le_bytes());
            }
            SignalKind::VarLen => {
                put_u64(&mut column.headers, delta);
                put_blob(&mut column.values, encoded);
            }
        }
        column.last_index = index;
        match &mut column.last {
            LastValue::Scalar(previous) => *previous = word,
            LastValue::Bytes(previous) => {
                previous.clear();
                previous.extend_from_slice(canonical);
            }
        }
        column.emitted = true;
        column.count += 1;
        self.recorded(encoded.len());
        Ok(())
    }

    #[inline]
    fn recorded(&mut self, length: usize) {
        self.records += 1;
        self.used_time = true;
        self.progress = true;
        self.bytes += 8 + length;
        self.peak_bytes = self.peak_bytes.max(self.bytes);
    }

    #[inline]
    fn capture_unsigned(
        &mut self,
        signal: u32,
        time: u64,
        width: u32,
        word: u64,
        options: Options,
    ) -> Result<()> {
        if !options.enabled {
            return Ok(());
        }
        self.advance(time)?;
        let column = &self.columns[signal as usize];
        if options.deduplicate
            && !column.event
            && column.emitted
            && matches!(column.last, LastValue::Scalar(previous) if previous == word)
        {
            return Ok(());
        }
        let index = self.prepare_column(signal, options.group_size)?;
        let column = &mut self.columns[signal as usize];
        let delta = u64::from(index - column.last_index);
        if width == 1 {
            let code = word as u8;
            let header = if column.last_code <= 1 && code == column.last_code ^ 1 {
                delta << 1
            } else {
                (delta << 5) | (u64::from(code) << 1) | 1
            };
            put_u64(&mut column.headers, header);
            column.last_code = code;
        } else {
            put_u64(&mut column.headers, (delta << 1) | 1);
            append_scalar(&mut column.values, word, width.div_ceil(8) as usize);
        }
        column.last_index = index;
        let LastValue::Scalar(previous) = &mut column.last else {
            unreachable!("scalar signal storage")
        };
        *previous = word;
        column.emitted = true;
        column.count += 1;
        self.recorded(width.div_ceil(8) as usize);
        Ok(())
    }

    #[inline]
    fn prepare_column(&mut self, signal: u32, group_size: u32) -> Result<u32> {
        let group = signal / group_size;
        if self.frames.get(group as usize).is_none_or(Option::is_none) {
            self.prepare_frame(group, group_size);
        }
        u32::try_from(self.times.len() - 1)
            .ok()
            .filter(|index| *index < block::COMPACT_FLAG)
            .ok_or_else(|| Error::invalid("waveform time table exhausted"))
    }

    #[cold]
    fn prepare_frame(&mut self, group: u32, group_size: u32) {
        if group as usize >= self.frames.len() {
            self.frames.resize_with(group as usize + 1, || None);
        }
        let mut frame = Vec::new();
        for id in group * group_size..((group + 1) * group_size).min(self.kinds.len() as u32) {
            let column = &self.columns[id as usize];
            column.last.append_frame(column.kind, &mut frame);
        }
        self.frames[group as usize] = Some(frame);
    }

    pub fn reclaim(&mut self, job: WaveJob) {
        for (signal, _, mut headers, mut values) in job.columns.0 {
            headers.clear();
            values.clear();
            self.columns[signal as usize].headers = headers;
            self.columns[signal as usize].values = values;
        }
    }

    pub fn should_flush(&self, bytes: usize, records: usize) -> bool {
        self.bytes >= bytes || self.records >= records
    }
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }

    pub fn prepare(&mut self, group_size: u32) -> Result<Option<(u64, u64, WaveJob)>> {
        if !self.progress {
            return Ok(None);
        }
        let kinds = Arc::new(self.kinds.clone());
        let columns = EncodedColumns(
            self.columns
                .iter_mut()
                .enumerate()
                .filter_map(|(signal, column)| {
                    let count = column.count;
                    if count == 0 {
                        return None;
                    }
                    column.count = 0;
                    column.last_index = 0;
                    column.last_code = 255;
                    Some((
                        signal as u32,
                        count,
                        std::mem::take(&mut column.headers),
                        std::mem::take(&mut column.values),
                    ))
                })
                .collect(),
        );
        let groups = (self.kinds.len() as u32).div_ceil(group_size);
        self.previous.resize(groups as usize, block::NO_BLOCK);
        let mut frame = Vec::new();
        let mut dirty_groups = Vec::new();
        for (group, bytes) in self.frames.iter_mut().enumerate() {
            if let Some(bytes) = bytes.take() {
                dirty_groups.push(group as u32);
                frame.extend_from_slice(&bytes);
            }
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
            .ok_or_else(|| Error::invalid("wave block count exhausted"))?;
        self.records = 0;
        self.times.push(end);
        self.used_time = false;
        self.progress = false;
        self.bytes = 0;
        Ok(Some((begin, end, WaveJob { columns, block })))
    }
}

pub(super) struct WaveJob {
    columns: EncodedColumns,
    block: BlockInput,
}
impl WaveJob {
    pub fn bytes(&self) -> usize {
        self.columns.0.capacity() * std::mem::size_of::<(u32, u32, Vec<u8>, Vec<u8>)>()
            + self
                .columns
                .0
                .iter()
                .map(|(_, _, headers, values)| headers.capacity() + values.capacity())
                .sum::<usize>()
            + self.block.times.capacity() * 8
            + self.block.frame.capacity()
            + self.block.kinds.capacity() * std::mem::size_of::<SignalKind>()
            + self.block.prev_dirty.capacity() * std::mem::size_of::<u32>()
            + self.block.dirty_groups.capacity() * std::mem::size_of::<u32>()
    }
    pub fn encode(&self, compression: Compression) -> Result<Vec<u8>> {
        Encoder::default().encode(self, compression)
    }
}

#[derive(Default)]
pub(super) struct Encoder {
    scratch: EncoderScratch,
    compressor: Compressor,
}
impl Encoder {
    pub fn encode(&mut self, job: &WaveJob, compression: Compression) -> Result<Vec<u8>> {
        self.scratch
            .encoded_columns(&job.columns, job.block.n_signals as usize);
        let mut output = Vec::new();
        block::finish_fragments(
            &job.block,
            std::slice::from_ref(&job.columns),
            compression,
            &mut self.compressor,
            &mut self.scratch,
            &mut output,
        )?;
        output.extend_from_slice(self.scratch.data());
        Ok(output)
    }
}
