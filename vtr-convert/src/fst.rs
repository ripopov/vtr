//! FST decoding and conversion, independent of the viewer's recording loaders.

use std::collections::{BTreeMap, BinaryHeap};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalValue};
use vtr::{Direction, LogicStates, ScopeType, SignalId, SignalKind, VarType, Writer};

trait Input: BufRead + Seek {}
impl<T: BufRead + Seek> Input for T {}

pub fn convert_fst(
    input: impl AsRef<Path>,
    writer: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    let input = std::io::BufReader::new(std::fs::File::open(input)?);
    convert_input(input, writer, progress)
}

pub fn convert_input(
    input: impl BufRead + Seek + 'static,
    writer: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    progress.begin(vtr_progress::Phase::Preparing, None);
    convert_reader(Box::new(input), writer, progress)
}

fn convert_reader(
    mut input: Box<dyn Input>,
    writer: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    validate_sections(&mut *input)?;
    let mut tag = [0];
    input.read_exact(&mut tag)?;
    if tag[0] == 254 {
        let mut lengths = [0; 16];
        input.read_exact(&mut lengths)?;
        let expected = u64::from_be_bytes(lengths[8..].try_into().unwrap());
        let limit = expected
            .checked_add(1)
            .ok_or_else(|| anyhow!("FST wrapper length overflow"))?;
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(input)
            .take(limit)
            .read_to_end(&mut bytes)
            .context("decompress FST wrapper")?;
        ensure!(bytes.len() as u64 == expected, "FST wrapper size mismatch");
        ensure!(
            bytes.first() == Some(&0),
            "FST wrapper must contain a recording header"
        );
        input = Box::new(std::io::Cursor::new(bytes));
        validate_sections(&mut *input)?;
    } else {
        ensure!(tag[0] == 0, "expected an FST recording");
        input.rewind()?;
    }
    let mut reader = FstReader::open(input).context("parse FST")?;
    let header = reader.get_header();
    writer.set_timescale(vtr::Timescale::Exponent(header.timescale_exponent))?;
    let mut scopes = Vec::new();
    let mut top = None;
    let mut signals = BTreeMap::new();
    let mut result = Ok(());
    reader
        .read_hierarchy(|entry| {
            if result.is_err() {
                return;
            }
            result = (|| -> Result<()> {
                match entry {
                    FstHierarchyEntry::Scope {
                        tpe,
                        name,
                        component,
                    } => {
                        let id = writer.add_scope(
                            scopes.last().copied(),
                            &name,
                            ScopeType::from_code(tpe as u16),
                            &component,
                        )?;
                        scopes.push(id);
                    }
                    FstHierarchyEntry::UpScope => {
                        ensure!(scopes.pop().is_some(), "FST hierarchy scope underflow");
                    }
                    FstHierarchyEntry::Var {
                        tpe,
                        direction,
                        name,
                        length,
                        handle,
                        ..
                    } => {
                        let kind = if tpe.is_real() {
                            SignalKind::Real
                        } else if matches!(
                            tpe,
                            fst_reader::FstVarType::GenericString | fst_reader::FstVarType::Port
                        ) {
                            SignalKind::VarLen
                        } else {
                            SignalKind::bits(
                                if tpe == fst_reader::FstVarType::Event {
                                    1
                                } else {
                                    length.max(1)
                                },
                                LogicStates::Nine,
                            )
                        };
                        let var_type = VarType::from_code(tpe as u16);
                        let direction = match Direction::from_u8(direction as u8) {
                            d @ (Direction::Input | Direction::Output | Direction::InOut) => d,
                            _ => Direction::Implicit,
                        };
                        let scope = if let Some(&scope) = scopes.last() {
                            scope
                        } else if let Some(scope) = top {
                            scope
                        } else {
                            let scope = writer.add_scope(None, "(top)", ScopeType::Module, "")?;
                            top = Some(scope);
                            scope
                        };
                        match signals.entry(handle.get_index()) {
                            std::collections::btree_map::Entry::Occupied(e) => {
                                let history: &History = e.get();
                                ensure!(
                                    history.kind == kind
                                        && history.event == (var_type == VarType::Event),
                                    "FST alias has incompatible shape"
                                );
                                writer.add_alias(
                                    Some(scope),
                                    &name,
                                    var_type,
                                    direction,
                                    history.signal,
                                )?;
                            }
                            std::collections::btree_map::Entry::Vacant(e) => {
                                let (_, signal) = writer.add_var(
                                    Some(scope),
                                    &name,
                                    var_type,
                                    direction,
                                    kind,
                                )?;
                                e.insert(History::new(signal, kind, var_type == VarType::Event));
                            }
                        }
                    }
                    _ => {}
                }
                Ok(())
            })();
        })
        .context("read FST hierarchy")?;
    result?;
    ensure!(scopes.is_empty(), "FST hierarchy has unclosed scopes");
    let reading = progress.begin(
        vtr_progress::Phase::Reading,
        header.end_time.checked_sub(header.start_time),
    );
    reader
        .read_signals(&FstFilter::all(), |time, handle, value| -> Result<()> {
            reading.set(time.saturating_sub(header.start_time));
            signals
                .get_mut(&handle.get_index())
                .ok_or_else(|| anyhow!("FST returned an undeclared signal"))?
                .push(time, value)
        })
        .map_err(|error| anyhow!("read FST signals: {error:?}"))?;
    drop(reader);
    let histories: Vec<_> = signals.into_values().collect();
    let writing = progress.begin(
        vtr_progress::Phase::Writing,
        Some(histories.iter().map(|h| h.times.len() as u64).sum()),
    );
    let mut heap = BinaryHeap::new();
    for (i, history) in histories.iter().enumerate() {
        if let Some(&time) = history.times.first() {
            heap.push(std::cmp::Reverse((time, i, 0)));
        }
    }
    while let Some(std::cmp::Reverse((time, i, offset))) = heap.pop() {
        let history = &histories[i];
        writer.set_time(time)?;
        history.emit(offset, writer)?;
        writing.advance(1);
        if let Some(&time) = history.times.get(offset + 1) {
            heap.push(std::cmp::Reverse((time, i, offset + 1)));
        }
    }
    writer.set_time(header.end_time)?;
    Ok(())
}

struct History {
    signal: SignalId,
    kind: SignalKind,
    event: bool,
    times: Vec<u64>,
    data: Vec<u8>,
    offsets: Vec<usize>,
}

impl History {
    fn new(signal: SignalId, kind: SignalKind, event: bool) -> Self {
        Self {
            signal,
            kind,
            event,
            times: Vec::new(),
            data: Vec::new(),
            offsets: vec![0],
        }
    }

    fn push(&mut self, time: u64, value: FstSignalValue<'_>) -> Result<()> {
        ensure!(
            self.times.last().is_none_or(|&last| last <= time),
            "FST signal times are not ordered"
        );
        match (self.kind, value) {
            (SignalKind::Real, FstSignalValue::Real(value)) => {
                self.data.extend_from_slice(&value.to_le_bytes());
            }
            (SignalKind::VarLen, FstSignalValue::String(bytes)) => {
                self.data.extend_from_slice(bytes);
                self.offsets.push(self.data.len());
            }
            (SignalKind::Bits { width, .. }, FstSignalValue::String(bytes)) => {
                ensure!(
                    bytes.len() == width as usize,
                    "FST logic value width differs from its declaration"
                );
                ensure!(
                    bytes.iter().all(|b| b"01xXzZuUwWlLhH-".contains(b)),
                    "FST logic value contains an unsupported state"
                );
                let start = self.data.len();
                self.data
                    .resize(start + vtr::value::packed_len(width, 9), 0);
                vtr::signal::pack_ascii(bytes, width, 9, &mut self.data[start..]);
            }
            _ => anyhow::bail!("FST value differs from its declaration"),
        }
        self.times.push(time);
        Ok(())
    }

    fn emit(&self, index: usize, writer: &mut Writer) -> Result<()> {
        match self.kind {
            SignalKind::Real => writer.emit_real(
                self.signal,
                f64::from_le_bytes(self.data[index * 8..index * 8 + 8].try_into()?),
            )?,
            SignalKind::VarLen => writer.emit_varlen(
                self.signal,
                &self.data[self.offsets[index]..self.offsets[index + 1]],
            )?,
            SignalKind::Bits { width, .. } => {
                let stride = vtr::value::packed_len(width, 9);
                writer.emit_packed(
                    self.signal,
                    9,
                    &self.data[index * stride..(index + 1) * stride],
                )?;
            }
        }
        Ok(())
    }
}

fn validate_sections(input: &mut dyn Input) -> anyhow::Result<()> {
    let length = input.seek(SeekFrom::End(0))?;
    let mut offset = 0u64;
    while offset < length {
        ensure!(
            length - offset >= 9,
            "truncated FST section header at {offset}"
        );
        input.seek(SeekFrom::Start(offset))?;
        let mut header = [0; 9];
        input.read_exact(&mut header)?;
        let size = u64::from_be_bytes(header[1..].try_into().unwrap());
        ensure!(size >= 8, "invalid or unfinished FST section at {offset}");
        let end = offset
            .checked_add(1)
            .and_then(|o| o.checked_add(size))
            .ok_or_else(|| anyhow!("FST section length overflow at {offset}"))?;
        ensure!(end <= length, "truncated FST section at {offset}");
        match header[0] {
            0 => {
                ensure!(size >= 329, "short FST header");
                input.seek(SeekFrom::Start(offset + 322))?;
                let mut time_zero = [0; 8];
                input.read_exact(&mut time_zero)?;
                ensure!(
                    i64::from_be_bytes(time_zero) == 0,
                    "FST nonzero time offsets are not supported by this reader"
                );
            }
            2 => {
                ensure!(size >= 9, "truncated FST dump-activity metadata");
                let mut count = [0];
                input.read_exact(&mut count)?;
                ensure!(
                    count[0] == 0,
                    "FST dump-off regions are not supported by this reader"
                );
            }
            254 => ensure!(
                offset == 0 && size >= 16 && end == length,
                "invalid FST gzip wrapper"
            ),
            _ => {}
        }
        offset = end;
    }
    input.rewind()?;
    Ok(())
}
