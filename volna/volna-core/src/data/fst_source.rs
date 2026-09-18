//! FST's mutable reader is owned by this adapter, never by a frontend.
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow, ensure};
use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalHandle, FstSignalValue};

use super::history::VecHistory;
use super::{Hierarchy, SignalHistory, SignalRef, SignalShape, TraceInfo, Variable, WaveValue};
use crate::session::{Session, SignalLoads};

pub(crate) trait Input: BufRead + Seek + Send {}
impl<T: BufRead + Seek + Send> Input for T {}

pub(crate) struct FstSession {
    reader: Mutex<FstReader<Box<dyn Input>>>,
    info: TraceInfo,
    hierarchy: Hierarchy,
    shapes: BTreeMap<SignalRef, SignalShape>,
}

impl FstSession {
    pub(crate) fn open(name: String, mut input: Box<dyn Input>) -> anyhow::Result<Self> {
        validate_sections(&mut *input)?;
        let mut tag = [0];
        input.read_exact(&mut tag)?;
        if tag[0] == 254 {
            // Unwrap here so the same framing/feature checks also see the inner
            // recording. fst-reader otherwise unwraps privately into memory.
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
            input.rewind()?;
        }
        let mut reader = FstReader::open(input).context("parse FST")?;
        let header = reader.get_header();
        let mut hierarchy = Hierarchy::default();
        let mut scopes = Vec::new();
        let mut shapes = BTreeMap::new();
        let mut error = None;
        let mut top = None;
        reader
            .read_hierarchy(|entry| match entry {
                FstHierarchyEntry::Scope { tpe, name, .. } => {
                    let id = hierarchy.push_scope(
                        name,
                        vtr::ScopeType::from_code(tpe as u16).name().into(),
                        scopes.last().copied(),
                    );
                    scopes.push(id);
                }
                FstHierarchyEntry::UpScope if scopes.pop().is_none() => {
                    error = Some(anyhow!("FST hierarchy scope underflow"));
                }
                FstHierarchyEntry::Var {
                    tpe,
                    direction,
                    name,
                    length,
                    handle,
                    ..
                } => {
                    let signal = SignalRef(handle.get_index() as u32);
                    let shape = if tpe == fst_reader::FstVarType::Event {
                        SignalShape::Event
                    } else if tpe.is_real() {
                        SignalShape::Real
                    } else if matches!(
                        tpe,
                        fst_reader::FstVarType::GenericString | fst_reader::FstVarType::Port
                    ) {
                        // EVCD ports carry values and strength fields. Keep the
                        // complete payload rather than interpreting it as bits.
                        SignalShape::Text
                    } else if length <= 1 {
                        SignalShape::Bit
                    } else {
                        SignalShape::Vector { width: length }
                    };
                    if let Some(previous) = shapes.insert(signal, shape)
                        && previous != shape
                    {
                        error = Some(anyhow!("FST alias has incompatible shape"));
                    }
                    let scope = scopes.last().copied().unwrap_or_else(|| {
                        *top.get_or_insert_with(|| {
                            hierarchy.push_scope("(top)".into(), "module".into(), None)
                        })
                    });
                    let id = hierarchy.vars.len();
                    hierarchy.vars.push(Variable {
                        name,
                        scope,
                        shape,
                        signal,
                        var_type: vtr::VarType::from_code(tpe as u16).name().into(),
                        direction: vtr::Direction::from_u8(direction as u8).into(),
                    });
                    hierarchy.scopes[scope].vars.push(id);
                }
                _ => {}
            })
            .context("read FST hierarchy")?;
        if let Some(error) = error {
            return Err(error);
        }
        ensure!(scopes.is_empty(), "FST hierarchy has unclosed scopes");
        let info = TraceInfo {
            design_id: None,
            name,
            timescale: header.timescale_exponent,
            time_range: (header.start_time, header.end_time),
            signal_count: shapes.len(),
            change_count: None,
        };
        Ok(Self {
            reader: Mutex::new(reader),
            info,
            hierarchy,
            shapes,
        })
    }

    fn read_batch(
        &self,
        signals: &[SignalRef],
    ) -> anyhow::Result<BTreeMap<SignalRef, Arc<dyn SignalHistory>>> {
        let mut histories = BTreeMap::new();
        for signal in signals {
            let Some(&shape) = self.shapes.get(signal) else {
                continue;
            };
            histories.entry(*signal).or_insert_with(|| VecHistory {
                shape,
                times: Vec::new(),
                values: Vec::new(),
                initial: WaveValue::Unavailable,
            });
        }
        if histories.is_empty() {
            return Ok(BTreeMap::new());
        }
        let filter = FstFilter::filter_signals(
            histories
                .keys()
                .map(|s| FstSignalHandle::from_index(s.0 as usize))
                .collect(),
        );
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| anyhow!("FST reader lock poisoned"))?;
        reader
            .read_signals(&filter, |time, handle, value| -> anyhow::Result<()> {
                let signal = SignalRef(handle.get_index() as u32);
                let history = histories
                    .get_mut(&signal)
                    .ok_or_else(|| anyhow!("FST returned an unrequested signal"))?;
                ensure!(
                    history.times.last().is_none_or(|&last| last <= time),
                    "FST signal times are not ordered"
                );
                let value = match value {
                    FstSignalValue::Real(value) => WaveValue::Real(value),
                    FstSignalValue::String(bytes) => {
                        if history.shape == SignalShape::Text {
                            WaveValue::Bytes(bytes.to_vec())
                        } else {
                            let text = std::str::from_utf8(bytes)
                                .context("FST logic value is not ASCII")?
                                .to_owned();
                            ensure!(
                                text.len() == history.shape.width() as usize
                                    || history.shape == SignalShape::Event,
                                "FST logic value width differs from its declaration"
                            );
                            ensure!(
                                text.bytes().all(|b| b"01xXzZuUwWlLhH-".contains(&b)),
                                "FST logic value contains an unsupported state"
                            );
                            WaveValue::Bits(text)
                        }
                    }
                };
                history.times.push(time);
                history.values.push(value);
                Ok(())
            })
            .map_err(|error| anyhow!("read FST signals: {error:?}"))?;
        Ok(histories
            .into_iter()
            .map(|(s, h)| (s, Arc::new(h) as Arc<dyn SignalHistory>))
            .collect())
    }
}

// fst-reader seeks over sections while discovering metadata. A seek beyond
// EOF succeeds on files/cursors, so check framing before handing over ownership.
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
                    "FST nonzero time offsets are not yet supported by the viewer"
                );
            }
            2 => {
                // The blackout payload starts with an unsigned varint count.
                // A zero count has the canonical one-byte spelling 0.
                ensure!(size >= 9, "truncated FST dump-activity metadata");
                let mut count = [0];
                input.read_exact(&mut count)?;
                ensure!(
                    count[0] == 0,
                    "FST dump-off regions are not yet supported by the viewer"
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

impl Session for FstSession {
    fn info(&self) -> &TraceInfo {
        &self.info
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        self.load_signals(&[signal]).pop().unwrap().1
    }
    fn load_signals(&self, signals: &[SignalRef]) -> SignalLoads {
        match self.read_batch(signals) {
            Ok(histories) => signals
                .iter()
                .map(|&s| {
                    (
                        s,
                        histories
                            .get(&s)
                            .cloned()
                            .ok_or_else(|| anyhow!("unknown FST signal {}", s.0)),
                    )
                })
                .collect(),
            Err(error) => signals
                .iter()
                .map(|&s| (s, Err(anyhow!("{error:#}"))))
                .collect(),
        }
    }
}
