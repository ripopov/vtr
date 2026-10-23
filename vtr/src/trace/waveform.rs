use std::{collections::BTreeMap, sync::Arc};

use super::Reader;
use crate::{
    Error, Result,
    block::{self, BlockHeader},
    declarations::{Declarations, Metadata},
    framing::{Container, Section},
    hierarchy::{SignalId, SignalKind},
    signal::{self, OwnedSignalValue, SignalValue},
    wire::{self, Cursor},
    xform::Xform,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeWindow {
    pub first: u64,
    pub last: u64,
}

impl Default for TimeWindow {
    fn default() -> Self {
        Self {
            first: 0,
            last: u64::MAX,
        }
    }
}

impl TimeWindow {
    pub(crate) fn check(self) -> Result<()> {
        if self.first > self.last {
            Err(Error::invalid("invalid time window"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
pub struct SignalData {
    pub kind: SignalKind,
    pub initial: OwnedSignalValue,
    pub times: Vec<u64>,
    pub values: Vec<OwnedSignalValue>,
}

impl SignalData {
    #[must_use]
    pub fn kind(&self) -> SignalKind {
        self.kind
    }
    #[must_use]
    pub fn initial(&self) -> SignalValue<'_> {
        self.initial.borrow()
    }
    #[must_use]
    pub fn times(&self) -> &[u64] {
        &self.times
    }
    #[must_use]
    pub fn get(&self, index: usize) -> SignalValue<'_> {
        self.values[index].borrow()
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.times.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
    #[must_use]
    pub fn value_at(&self, time: u64) -> SignalValue<'_> {
        let index = self.times.partition_point(|t| *t <= time);
        if index == 0 {
            self.initial.borrow()
        } else {
            self.values[index - 1].borrow()
        }
    }
}

pub(super) struct WaveBlock {
    pub section: Section,
    pub header: BlockHeader,
}

fn parse_header(payload: &[u8]) -> Result<BlockHeader> {
    let mut cursor = Cursor::new(payload);
    let header = BlockHeader {
        start_time: cursor.u64()?,
        end_time: cursor.u64()?,
        n_times: cursor.u32()?,
        n_signals: cursor.u32()?,
        group_size: cursor.u32()?,
        n_dirty: cursor.u32()?,
        n_groups: cursor.u32()?,
        tt_len: cursor.u32()?,
    };
    let size = (header.tt_len as usize)
        .checked_add(40)
        .and_then(|n| n.checked_add((header.n_dirty as usize).checked_mul(16)?))
        .and_then(|n| n.checked_add((header.n_groups as usize).checked_mul(4)?))
        .ok_or(Error::Corrupt("signal block index size overflow"))?;
    if size > payload.len() {
        return Err(Error::Corrupt("signal block index exceeds section"));
    }
    Ok(header)
}

pub(super) fn parse_blocks(
    container: &Container,
    declarations: &Declarations,
    metadata: &Metadata,
) -> Result<Vec<WaveBlock>> {
    let mut waves: Vec<WaveBlock> = Vec::new();
    let mut previous = Vec::new();
    let mut available_signals = 0;
    for section in container.sections() {
        if section.kind == 3 {
            available_signals = declarations.nodes[..usize::try_from(section.aux0 + section.aux1)
                .map_err(|_| Error::Corrupt("hierarchy count overflow"))?]
                .iter()
                .filter(|node| {
                    matches!(
                        node.data,
                        crate::declarations::NodeData::Signal { alias: false, .. }
                    )
                })
                .count();
        }
        if section.kind != 4 {
            continue;
        }
        let payload = container.payload(section, false)?;
        let header = parse_header(payload)?;
        if header.start_time > header.end_time
            || header.n_times == 0
            || header.n_signals as usize > available_signals
            || header.group_size != metadata.group_size
            || header.n_groups != header.n_signals.div_ceil(metadata.group_size)
            || header.n_dirty > header.n_groups
            || header.start_time != section.aux0
            || header.end_time != section.aux1
            || waves.last().is_some_and(|previous| {
                previous.header.end_time > header.start_time
                    || previous.header.n_signals > header.n_signals
            })
        {
            return Err(Error::Corrupt(
                "invalid signal block header or declaration order",
            ));
        }
        let mut end = 0u64;
        let mut last_group = None;
        for (group, length, offset) in block::dirty_groups(payload, &header) {
            if group >= header.n_groups
                || last_group.is_some_and(|last| last >= group)
                || length == 0
                || offset != end
            {
                return Err(Error::Corrupt("invalid wave group partition"));
            }
            end = end
                .checked_add(u64::from(length))
                .ok_or(Error::Corrupt("wave group length overflow"))?;
            last_group = Some(group);
        }
        if end != (payload.len() - header.data_off()) as u64 {
            return Err(Error::Corrupt("wave groups do not cover the section"));
        }
        previous.resize(header.n_groups as usize, block::NO_BLOCK);
        for group in 0..header.n_groups {
            if block::prev_dirty(payload, &header, group) != previous[group as usize] {
                return Err(Error::Corrupt("invalid wave predecessor"));
            }
        }
        for (group, _, _) in block::dirty_groups(payload, &header) {
            previous[group as usize] = waves.len() as u32;
        }
        waves.push(WaveBlock {
            section: *section,
            header,
        });
    }
    Ok(waves)
}

struct Run<'a> {
    first: u32,
    count: u32,
    transform: Xform,
    blob: &'a [u8],
}
struct Group<'a> {
    first: u32,
    count: u32,
    aliases: BTreeMap<u32, u32>,
    frames: &'a [u8],
    runs: Vec<Run<'a>>,
}

fn group<'a>(
    payload: &'a [u8],
    header: &BlockHeader,
    id: u32,
    kinds: &[SignalKind],
) -> Result<Option<Group<'a>>> {
    let Some((length, offset)) = block::find_group(payload, header, id) else {
        return Ok(None);
    };
    let mut cursor = Cursor::new(block::group_container(payload, header, length, offset)?);
    let first = id
        .checked_mul(header.group_size)
        .ok_or(Error::Corrupt("wave group index overflow"))?;
    let count = cursor.id()?;
    if count == 0 || count != header.group_size.min(header.n_signals - first) {
        return Err(Error::Corrupt("invalid wave group signal count"));
    }
    let alias_count = cursor.id()?;
    if alias_count > count || alias_count as usize > cursor.remaining() / 2 {
        return Err(Error::Corrupt("invalid wave alias count"));
    }
    let mut aliases = BTreeMap::new();
    let mut previous = None;
    for _ in 0..alias_count {
        let local = cursor.id()?;
        let target = cursor.id()?;
        if local >= count
            || target >= header.n_signals
            || previous.is_some_and(|old| old >= local)
            || target == first + local
            || kinds[target as usize] != kinds[(first + local) as usize]
        {
            return Err(Error::Corrupt("invalid wave alias"));
        }
        previous = Some(local);
        aliases.insert(first + local, target);
    }
    let frames = cursor.blob()?;
    let run_count = cursor.id()?;
    if run_count > count || run_count as usize > cursor.remaining() / 3 {
        return Err(Error::Corrupt("invalid wave run count"));
    }
    let mut entries = Vec::new();
    let mut signal = first;
    for _ in 0..run_count {
        let size = cursor.id()?;
        let transform =
            Xform::from_u8(cursor.byte()?).ok_or(Error::Corrupt("unknown wave transform"))?;
        let length = usize::try_from(cursor.var()?)
            .map_err(|_| Error::Corrupt("wave run size exceeds usize"))?;
        if size == 0 || size > first + count - signal {
            return Err(Error::Corrupt("invalid wave run range"));
        }
        entries.push((signal, size, transform, length));
        signal += size;
    }
    if signal != first + count {
        return Err(Error::Corrupt("wave runs do not cover group"));
    }
    let mut runs = Vec::new();
    for (first, count, transform, length) in entries {
        runs.push(Run {
            first,
            count,
            transform,
            blob: cursor.take(length)?,
        });
    }
    cursor.done()?;
    Ok(Some(Group {
        first,
        count,
        aliases,
        frames,
        runs,
    }))
}

fn times(payload: &[u8], header: &BlockHeader) -> Result<Vec<u64>> {
    let raw = wire::decompress_any(&payload[40..header.index_off()])?;
    let mut cursor = Cursor::new(&raw);
    if header.n_times as usize > cursor.remaining() {
        return Err(Error::Corrupt("invalid wave time count"));
    }
    let mut out = Vec::with_capacity(header.n_times as usize);
    let mut time = 0u64;
    for index in 0..header.n_times {
        let delta = cursor.var()?;
        if index != 0 && delta == 0 {
            return Err(Error::Corrupt("duplicate wave timestamp"));
        }
        time = time
            .checked_add(delta)
            .ok_or(Error::Corrupt("wave timestamp overflow"))?;
        out.push(time);
    }
    cursor.done()?;
    if out.first() != Some(&header.start_time) || out.last() != Some(&header.end_time) {
        return Err(Error::Corrupt("wave time table bounds disagree"));
    }
    Ok(out)
}

fn run_columns(run: &Run<'_>) -> Result<(Vec<u8>, Vec<std::ops::Range<usize>>)> {
    let raw = wire::decompress_any(run.blob)?;
    let mut cursor = Cursor::new(&raw);
    if run.count as usize > raw.len() {
        return Err(Error::Corrupt("wave run count exceeds data"));
    }
    let mut lengths = Vec::new();
    for _ in 0..run.count {
        lengths.push(cursor.var()?);
    }
    let mut ranges = Vec::new();
    for length in lengths {
        let start = raw.len() - cursor.remaining();
        cursor.take(
            usize::try_from(length).map_err(|_| Error::Corrupt("column size exceeds usize"))?,
        )?;
        ranges.push(start..raw.len() - cursor.remaining());
    }
    cursor.done()?;
    Ok((raw, ranges))
}

fn packed(kind: SignalKind, states: u8, bytes: &[u8]) -> Result<()> {
    if let SignalKind::Bits {
        width,
        states: declared,
    } = kind
    {
        if states != 2 && states != declared {
            return Err(Error::Corrupt("wave alphabet exceeds declaration"));
        }
        let primitive = match states {
            2 => crate::schema::Primitive::Bits,
            4 => crate::schema::Primitive::Logic,
            9 => crate::schema::Primitive::Logic9,
            _ => return Err(Error::Corrupt("invalid wave alphabet")),
        };
        if bytes.len() != primitive.packed_bytes(u64::from(width))? {
            return Err(Error::Corrupt("wave packed size mismatch"));
        }
        primitive.validate_packed(bytes, u64::from(width))?;
    }
    Ok(())
}

fn read_frame<'a>(cursor: &mut Cursor<'a>, kind: SignalKind) -> Result<SignalValue<'a>> {
    Ok(match kind {
        SignalKind::Bits { width, states } => {
            let bytes = cursor.take(kind.packed_len().unwrap())?;
            packed(kind, states, bytes)?;
            SignalValue::Bits {
                width,
                states,
                data: bytes,
            }
        }
        SignalKind::Real => SignalValue::Real(f64::from_bits(cursor.u64()?)),
        SignalKind::VarLen => SignalValue::VarLen(cursor.blob()?),
    })
}

fn column(
    column: &[u8],
    kind: SignalKind,
    transform: Xform,
    times: &[u64],
    window: TimeWindow,
    visit: &mut impl FnMut(u64, SignalValue<'_>) -> bool,
) -> Result<bool> {
    if column.is_empty() {
        return Ok(true);
    }
    let mut time_index = 0u64;
    if let SignalKind::Bits { width: 1, states } = kind {
        let mut cursor = Cursor::new(column);
        let mut code = 255u8;
        while cursor.remaining() != 0 {
            let entry = cursor.var()?;
            let delta = if entry & 1 == 0 {
                if code >= 2 {
                    return Err(Error::Corrupt("invalid wave bit toggle"));
                }
                code ^= 1;
                entry >> 1
            } else {
                code = ((entry >> 1) & 15) as u8;
                entry >> 5
            };
            time_index = time_index
                .checked_add(delta)
                .ok_or(Error::Corrupt("wave time index overflow"))?;
            let time = *times
                .get(
                    usize::try_from(time_index)
                        .map_err(|_| Error::Corrupt("wave time index exceeds usize"))?,
                )
                .ok_or(Error::Corrupt("wave time index exceeds table"))?;
            if code >= states {
                return Err(Error::Corrupt("wave logic exceeds alphabet"));
            }
            if time >= window.first
                && time <= window.last
                && !visit(
                    time,
                    SignalValue::Bits {
                        width: 1,
                        states,
                        data: &block::CODE_BYTES[code as usize..=code as usize],
                    },
                )
            {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let mut raw = column.to_vec();
    let mut cursor = Cursor::new(&raw);
    let descriptor = cursor.var()?;
    let headers_len = usize::try_from(descriptor >> 2)
        .map_err(|_| Error::Corrupt("wave header length exceeds usize"))?;
    let headers = cursor.take(headers_len)?;
    let mut header_cursor = Cursor::new(headers);
    let mut count = 0usize;
    while header_cursor.remaining() != 0 {
        header_cursor.var()?;
        count += 1;
    }
    if descriptor & 2 != 0 {
        let width = match kind {
            SignalKind::Bits { width, .. } => {
                if descriptor & 1 != 0 {
                    kind.packed_len().unwrap()
                } else {
                    (width as usize).div_ceil(8)
                }
            }
            SignalKind::Real => 8,
            SignalKind::VarLen => {
                return Err(Error::Corrupt("variable column cannot be transformed"));
            }
        };
        let expected = count
            .checked_mul(width)
            .ok_or(Error::Corrupt("wave value length overflow"))?;
        if expected >= wire::MAX_BLOCK_BYTES {
            return Err(Error::Corrupt("wave values exceed format limit"));
        }
        if transform != Xform::Dict && cursor.remaining() != expected {
            return Err(Error::Corrupt("fixed wave value length mismatch"));
        }
        block::untransform_column_vec(&mut raw, kind, transform, &mut Vec::new())?;
        let mut values = Cursor::new(&raw);
        values.var()?;
        values.take(headers_len)?;
        if values.remaining() != expected {
            return Err(Error::Corrupt("transformed wave value length mismatch"));
        }
    }
    let mut values = Cursor::new(&raw);
    values.var()?;
    let mut headers = Cursor::new(values.take(headers_len)?);
    while headers.remaining() != 0 {
        let header = headers.var()?;
        let delta = if matches!(kind, SignalKind::Bits { .. }) {
            header >> 1
        } else {
            header
        };
        time_index = time_index
            .checked_add(delta)
            .ok_or(Error::Corrupt("wave time index overflow"))?;
        let time = *times
            .get(
                usize::try_from(time_index)
                    .map_err(|_| Error::Corrupt("wave time index exceeds usize"))?,
            )
            .ok_or(Error::Corrupt("wave time index exceeds table"))?;
        let value = match kind {
            SignalKind::Bits { width, states } => {
                let compact = header & 1 != 0;
                if states == 2 && !compact {
                    return Err(Error::Corrupt("noncompact binary wave value"));
                }
                let encoded_states = if compact { 2 } else { states };
                let length = crate::signal::packed_len(width, encoded_states);
                let data = values.take(length)?;
                packed(kind, encoded_states, data)?;
                SignalValue::Bits {
                    width,
                    states: encoded_states,
                    data,
                }
            }
            SignalKind::Real => SignalValue::Real(f64::from_bits(values.u64()?)),
            SignalKind::VarLen => SignalValue::VarLen(values.blob()?),
        };
        if time >= window.first && time <= window.last && !visit(time, value) {
            return Ok(false);
        }
    }
    values.done()?;
    Ok(true)
}

fn default_value(kind: SignalKind) -> OwnedSignalValue {
    let mut bytes = Vec::new();
    signal::default_value(kind, &mut bytes);
    block::frame_value(kind, &bytes).to_owned()
}

impl Reader {
    pub fn signal_count(&self) -> u32 {
        self.declarations.signals.len() as u32
    }
    pub fn signal_kind(&self, signal: SignalId) -> Result<SignalKind> {
        self.declarations
            .signals
            .get(signal.0 as usize)
            .copied()
            .ok_or(Error::invalid("unknown signal"))
    }
    pub fn waveform_time_range(&self) -> Option<TimeWindow> {
        Some(TimeWindow {
            first: self.waves.first()?.header.start_time,
            last: self.waves.last()?.header.end_time,
        })
    }
    pub fn block_count(&self) -> usize {
        self.waves.len()
    }
    pub fn block_range(&self, index: usize) -> (u64, u64) {
        let h = &self.waves[index].header;
        (h.start_time, h.end_time)
    }
    pub fn block_times(&self, index: usize) -> Result<Arc<Vec<u64>>> {
        let wave = self
            .waves
            .get(index)
            .ok_or(Error::invalid("unknown wave block"))?;
        Ok(Arc::new(times(
            self.container.payload(&wave.section, false)?,
            &wave.header,
        )?))
    }
    pub fn signal_block(&self, index: usize) -> Result<(&[u8], BlockHeader, &[SignalKind])> {
        let wave = self
            .waves
            .get(index)
            .ok_or(Error::invalid("unknown wave block"))?;
        Ok((
            self.container.payload(&wave.section, false)?,
            wave.header,
            &self.declarations.signals[..wave.header.n_signals as usize],
        ))
    }
    pub fn signal_block_bytes(&self, index: usize) -> u64 {
        self.waves[index].section.length
    }
    pub fn group_first(&self, group: u32) -> u32 {
        group * self.metadata.group_size
    }

    fn selected_column(
        &self,
        index: usize,
        signal: SignalId,
        window: TimeWindow,
        visit: &mut impl FnMut(u64, SignalValue<'_>) -> bool,
    ) -> Result<bool> {
        let wave = &self.waves[index];
        let payload = self.container.payload(&wave.section, false)?;
        let kinds = &self.declarations.signals[..wave.header.n_signals as usize];
        if signal.0 >= wave.header.n_signals {
            return Ok(true);
        }
        let Some(owner) = group(
            payload,
            &wave.header,
            signal.0 / wave.header.group_size,
            kinds,
        )?
        else {
            return Ok(true);
        };
        let target = owner.aliases.get(&signal.0).copied().unwrap_or(signal.0);
        let owner = if target == signal.0 {
            owner
        } else {
            let target_group = group(
                payload,
                &wave.header,
                target / wave.header.group_size,
                kinds,
            )?
            .ok_or(Error::Corrupt("wave alias target group missing"))?;
            if target_group.aliases.contains_key(&target) {
                return Err(Error::Corrupt("wave alias target is aliased"));
            }
            target_group
        };
        let run = owner
            .runs
            .iter()
            .find(|run| run.first <= target && target < run.first + run.count)
            .ok_or(Error::Corrupt("wave alias target column missing"))?;
        let (raw, columns) = run_columns(run)?;
        let range = columns[(target - run.first) as usize].clone();
        column(
            &raw[range],
            kinds[signal.0 as usize],
            run.transform,
            &times(payload, &wave.header)?,
            window,
            visit,
        )
    }

    /// Change callbacks borrow the current column and follow storage order.
    pub fn changes(
        &self,
        selected: Option<SignalId>,
        window: TimeWindow,
        mut visit: impl FnMut(u64, SignalId, SignalValue<'_>) -> bool,
    ) -> Result<()> {
        window.check()?;
        if let Some(signal) = selected {
            self.signal_kind(signal)?;
        }
        for (index, wave) in self.waves.iter().enumerate() {
            if wave.header.start_time > window.last {
                break;
            }
            if wave.header.end_time < window.first {
                continue;
            }
            if let Some(signal) = selected {
                if !self.selected_column(index, signal, window, &mut |time, value| {
                    visit(time, signal, value)
                })? {
                    return Ok(());
                }
                continue;
            }
            if !self.block_changes(index, window, &mut visit)? {
                return Ok(());
            }
        }
        Ok(())
    }

    fn block_changes(
        &self,
        index: usize,
        window: TimeWindow,
        visit: &mut impl FnMut(u64, SignalId, SignalValue<'_>) -> bool,
    ) -> Result<bool> {
        let wave = &self.waves[index];
        let payload = self.container.payload(&wave.section, false)?;
        let times = times(payload, &wave.header)?;
        let kinds = &self.declarations.signals[..wave.header.n_signals as usize];
        let mut aliases = Vec::new();
        for (id, _, _) in block::dirty_groups(payload, &wave.header) {
            let group = group(payload, &wave.header, id, kinds)?.unwrap();
            aliases.extend(group.aliases.keys().copied());
            for run in &group.runs {
                let (raw, columns) = run_columns(run)?;
                for (local, range) in columns.into_iter().enumerate() {
                    let signal = run.first + local as u32;
                    if group.aliases.contains_key(&signal) {
                        if !range.is_empty() {
                            return Err(Error::Corrupt("aliased wave signal has its own column"));
                        }
                        continue;
                    }
                    if !column(
                        &raw[range],
                        kinds[signal as usize],
                        run.transform,
                        &times,
                        window,
                        &mut |time, value| visit(time, SignalId(signal), value),
                    )? {
                        return Ok(false);
                    }
                }
            }
        }
        for signal in aliases {
            if !self.selected_column(index, SignalId(signal), window, &mut |time, value| {
                visit(time, SignalId(signal), value)
            })? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Visits in timestamp order while retaining decoded changes from one wave block.
    pub fn ordered_changes(
        &self,
        window: TimeWindow,
        mut visit: impl FnMut(u64, SignalId, SignalValue<'_>) -> bool,
    ) -> Result<()> {
        window.check()?;
        for (index, wave) in self.waves.iter().enumerate() {
            if wave.header.start_time > window.last {
                break;
            }
            if wave.header.end_time < window.first {
                continue;
            }
            let mut changes = Vec::new();
            self.block_changes(index, window, &mut |time, signal, value| {
                changes.push((time, signal, value.to_owned()));
                true
            })?;
            changes.sort_by_key(|(time, signal, _)| (*time, *signal));
            for (time, signal, value) in changes {
                if !visit(time, signal, value.borrow()) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    pub fn value_at(&self, signal: SignalId, time: u64) -> Result<OwnedSignalValue> {
        let kind = self.signal_kind(signal)?;
        let Some(mut index) = self
            .waves
            .iter()
            .rposition(|wave| wave.header.start_time <= time)
        else {
            return Ok(default_value(kind));
        };
        if signal.0 >= self.waves[index].header.n_signals {
            return Ok(default_value(kind));
        }
        let group_id = signal.0 / self.metadata.group_size;
        let wave = &self.waves[index];
        let payload = self.container.payload(&wave.section, false)?;
        if block::find_group(payload, &wave.header, group_id).is_none() {
            let previous = block::prev_dirty(payload, &wave.header, group_id);
            if previous == block::NO_BLOCK {
                return Ok(default_value(kind));
            }
            index = previous as usize;
        }
        let wave = &self.waves[index];
        let payload = self.container.payload(&wave.section, false)?;
        let group = group(payload, &wave.header, group_id, &self.declarations.signals)?
            .ok_or(Error::Corrupt("wave predecessor is not dirty"))?;
        let raw = wire::decompress_any(group.frames)?;
        let mut frames = Cursor::new(&raw);
        let mut value = None;
        for id in group.first..group.first + group.count {
            let frame = read_frame(&mut frames, self.declarations.signals[id as usize])?;
            if id == signal.0 {
                value = Some(frame.to_owned());
            }
        }
        frames.done()?;
        self.selected_column(
            index,
            signal,
            TimeWindow {
                first: 0,
                last: time,
            },
            &mut |_, observed| {
                value = Some(observed.to_owned());
                true
            },
        )?;
        Ok(value.unwrap())
    }

    pub fn load_signal(&self, signal: SignalId) -> Result<SignalData> {
        let kind = self.signal_kind(signal)?;
        let mut data = SignalData {
            kind,
            initial: default_value(kind),
            times: Vec::new(),
            values: Vec::new(),
        };
        self.changes(
            Some(signal),
            TimeWindow::default(),
            |time, value_signal, value| {
                debug_assert_eq!(signal, value_signal);
                data.times.push(time);
                data.values.push(value.to_owned());
                true
            },
        )?;
        Ok(data)
    }

    pub(super) fn validate_waveforms(&self) -> Result<()> {
        let mut last: Vec<_> = self
            .declarations
            .signals
            .iter()
            .copied()
            .map(default_value)
            .collect();
        for (index, wave) in self.waves.iter().enumerate() {
            let payload = self.container.payload(&wave.section, true)?;
            let kinds = &self.declarations.signals[..wave.header.n_signals as usize];
            times(payload, &wave.header)?;
            for (id, _, _) in block::dirty_groups(payload, &wave.header) {
                let group = group(payload, &wave.header, id, kinds)?.unwrap();
                let raw = wire::decompress_any(group.frames)?;
                let mut cursor = Cursor::new(&raw);
                for signal in group.first..group.first + group.count {
                    let frame = read_frame(&mut cursor, kinds[signal as usize])?.to_owned();
                    if !same_value(&last[signal as usize], &frame) {
                        return Err(Error::Corrupt("wave frame differs from preceding value"));
                    }
                }
                cursor.done()?;
                for signal in group.first..group.first + group.count {
                    self.selected_column(
                        index,
                        SignalId(signal),
                        TimeWindow::default(),
                        &mut |_, value| {
                            last[signal as usize] = canonical_value(value, kinds[signal as usize]);
                            true
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn canonical_value(value: SignalValue<'_>, kind: SignalKind) -> OwnedSignalValue {
    match (value, kind) {
        (
            SignalValue::Bits {
                width,
                states,
                data,
            },
            SignalKind::Bits { states: target, .. },
        ) if states != target => {
            let mut out = Vec::new();
            signal::widen(data, width, states, target, &mut out);
            OwnedSignalValue::Bits {
                width,
                states: target,
                data: out,
            }
        }
        _ => value.to_owned(),
    }
}

fn same_value(a: &OwnedSignalValue, b: &OwnedSignalValue) -> bool {
    match (a, b) {
        (OwnedSignalValue::Real(a), OwnedSignalValue::Real(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}
