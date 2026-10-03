//! FST's mutable reader is owned by this adapter, never by a frontend.
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow, ensure};
use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalHandle, FstSignalValue};

use super::compact::CompactBuilder;
use super::history::VecHistory;
use super::{Hierarchy, SignalHistory, SignalRef, SignalShape, TraceInfo, WaveValue};
use crate::session::{Session, SignalLoads};

pub(crate) trait Input: BufRead + Seek + Send {}
impl<T: BufRead + Seek + Send> Input for T {}

pub(crate) struct FstSession {
    reader: Mutex<FstReader<Box<dyn Input>>>,
    info: TraceInfo,
    hierarchy: Hierarchy,
    shapes: BTreeMap<SignalRef, SignalShape>,
    source_bytes: u64,
    activity: Option<FstActivity>,
}

/// The activity index of an FST opened from a path, and the block scanner
/// that reads its undecided signals, mapped on the first read.
struct FstActivity {
    source: super::activity::ActivitySource,
    blocks: std::sync::OnceLock<Result<vtr_cli::fst::activity::FstTrace, String>>,
}

impl FstActivity {
    fn blocks(&self) -> anyhow::Result<&vtr_cli::fst::activity::FstTrace> {
        self.blocks
            .get_or_init(|| {
                let trace = vtr_cli::fst::activity::FstTrace::open(&self.source.path)
                    .map_err(|e| format!("{}: {e}", self.source.path.display()))?;
                if trace.identity() != self.source.identity {
                    return Err("the FST file was replaced after it was opened".into());
                }
                Ok(trace)
            })
            .as_ref()
            .map_err(|e| anyhow!("{e}"))
    }
}

impl FstSession {
    pub(crate) fn open(name: String, mut input: Box<dyn Input>) -> anyhow::Result<Self> {
        let mut source_bytes = input.seek(SeekFrom::End(0))?;
        input.rewind()?;
        validate_sections(&mut *input)?;
        let mut tag = [0];
        input.read_exact(&mut tag)?;
        if tag[0] == 254 {
            // Unwrap here so the same framing/feature checks also see the inner
            // recording. fst-reader otherwise unwraps privately into memory.
            let mut lengths = [0; 16];
            input.read_exact(&mut lengths)?;
            let expected = u64::from_be_bytes(lengths[8..].try_into().unwrap());
            source_bytes = source_bytes.saturating_add(expected);
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
        let mut hierarchy = super::hierarchy_columns::ColumnBuilder::default();
        let mut scopes = Vec::new();
        let mut shapes = BTreeMap::new();
        let mut error = None;
        let mut top = None;
        let mut enum_tables = BTreeMap::new();
        let mut pending_enum = None;
        reader
            .read_hierarchy(|entry| match entry {
                FstHierarchyEntry::Scope {
                    tpe,
                    name,
                    component,
                } => {
                    let id = hierarchy.push_scope(
                        &name,
                        vtr::ScopeType::from_code(tpe as u16).name(),
                        &component,
                        scopes.last().copied(),
                        super::source::ScopeRole::Scope,
                    );
                    scopes.push(id);
                    pending_enum = None;
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
                            hierarchy.push_scope(
                                "(top)",
                                "module",
                                "",
                                None,
                                super::source::ScopeRole::Scope,
                            )
                        })
                    });
                    hierarchy.push_var(super::source::VariableView {
                        name: &name,
                        scope,
                        shape,
                        signal,
                        enum_table: pending_enum.take(),
                        var_type: vtr::VarType::from_code(tpe as u16).name(),
                        direction: vtr::Direction::from_u8(direction as u8).into(),
                    });
                }
                FstHierarchyEntry::EnumTable { handle, .. } => {
                    let id = enum_tables.len() as u32;
                    enum_tables.insert(handle, id);
                }
                FstHierarchyEntry::EnumTableRef { handle } => {
                    pending_enum = enum_tables.get(&handle).copied();
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
            time_unit: None,
        };
        Ok(Self {
            reader: Mutex::new(reader),
            info,
            hierarchy: hierarchy.finish(),
            shapes,
            source_bytes,
            activity: None,
        })
    }

    /// Look for an activity index of the FST file at `path`, beside it or
    /// in the user cache, valid for this file.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn find_activity(&mut self, path: &std::path::Path) {
        let Ok(id) = vtr_cli::fst::activity::FstTrace::identity_of(path) else {
            return;
        };
        self.activity = Some(FstActivity {
            source: super::activity::ActivitySource::new(path, id),
            blocks: std::sync::OnceLock::new(),
        });
    }

    fn read_batch(
        &self,
        signals: &[SignalRef],
    ) -> anyhow::Result<BTreeMap<SignalRef, Arc<dyn SignalHistory>>> {
        // Requested signals in handle order, each with its history being
        // loaded; `slot` maps a handle to its place in O(1) per change.
        let mut requested: Vec<SignalRef> = signals
            .iter()
            .copied()
            .filter(|s| self.shapes.contains_key(s))
            .collect();
        requested.sort_unstable();
        requested.dedup();
        let Some(last) = requested.last() else {
            return Ok(BTreeMap::new());
        };
        let mut slot = vec![u32::MAX; last.0 as usize + 1];
        let mut histories: Vec<Loading> = Vec::with_capacity(requested.len());
        for (i, signal) in requested.iter().enumerate() {
            slot[signal.0 as usize] = i as u32;
            let shape = self.shapes[signal];
            histories.push(CompactBuilder::new(shape).map_or_else(
                || {
                    Loading::Vec(VecHistory {
                        shape,
                        times: Vec::new(),
                        values: Vec::new(),
                        initial: WaveValue::Unavailable,
                    })
                },
                Loading::Compact,
            ));
        }
        let filter = FstFilter::filter_signals(
            requested
                .iter()
                .map(|s| FstSignalHandle::from_index(s.0 as usize))
                .collect(),
        );
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| anyhow!("FST reader lock poisoned"))?;
        reader
            .read_signals(&filter, |time, handle, value| -> anyhow::Result<()> {
                let history = slot
                    .get(handle.get_index())
                    .and_then(|&i| histories.get_mut(i as usize))
                    .ok_or_else(|| anyhow!("FST returned an unrequested signal"))?;
                match (history, value) {
                    (Loading::Compact(b), FstSignalValue::Real(v)) => b.push_real(time, v),
                    (Loading::Compact(b), FstSignalValue::String(bytes)) => {
                        b.push_logic(time, bytes).map_err(|e| anyhow!("FST {e}"))
                    }
                    (Loading::Vec(h), value) => {
                        ensure!(
                            h.times.last().is_none_or(|&last| last <= time),
                            "FST signal times are not ordered"
                        );
                        h.values.push(match value {
                            FstSignalValue::Real(v) => WaveValue::Real(v),
                            FstSignalValue::String(bytes) if h.shape == SignalShape::Text => {
                                WaveValue::Bytes(bytes.to_vec())
                            }
                            // Events: one occurrence per change.
                            FstSignalValue::String(bytes) => WaveValue::Bits(
                                std::str::from_utf8(bytes)
                                    .context("FST logic value is not ASCII")?
                                    .to_owned(),
                            ),
                        });
                        h.times.push(time);
                        Ok(())
                    }
                }
            })
            .map_err(|error| anyhow!("read FST signals: {error:?}"))?;
        Ok(requested
            .into_iter()
            .zip(histories)
            .map(|(s, h)| {
                let h: Arc<dyn SignalHistory> = match h {
                    Loading::Compact(b) => Arc::new(b.finish()),
                    Loading::Vec(h) => Arc::new(h),
                };
                (s, h)
            })
            .collect())
    }
}

/// A history being loaded: packed for logic and reals, owned values for
/// text and events.
enum Loading {
    Compact(CompactBuilder),
    Vec(VecHistory),
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
    fn resident_bytes(&self) -> u64 {
        self.source_bytes
            + self.hierarchy.resident_bytes()
            + (self.shapes.len() * std::mem::size_of::<(SignalRef, SignalShape)>()) as u64
            + self
                .activity
                .as_ref()
                .map_or(0, |a| a.source.resident_bytes())
    }
    fn activity(&self) -> Option<Arc<crate::data::ActivityIndex>> {
        self.activity.as_ref().and_then(|a| a.source.index())
    }
    fn activity_build_info(&self) -> Option<super::ActivityBuildInfo> {
        self.activity.as_ref().map(|a| a.source.info())
    }
    fn activity_identity(&self) -> Option<vtr::activity::Identity> {
        self.activity
            .as_ref()
            .map(|a| &a.source)
            .map(|a| a.identity)
    }
    fn activity_image(&self, cache_dir: Option<&std::path::Path>) -> anyhow::Result<Vec<u8>> {
        #[cfg(not(target_family = "wasm"))]
        {
            self.activity
                .as_ref()
                .map(|a| &a.source)
                .ok_or_else(|| anyhow::anyhow!("this trace has no activity source"))?
                .image(cache_dir)
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = cache_dir;
            anyhow::bail!("byte-backed traces have no sidecar")
        }
    }
    fn build_activity(
        &self,
        options: &vtr::activity::BuildOptions,
        budget: &crate::remote::memory::MemoryBudget,
        cache_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<()> {
        let a = self
            .activity
            .as_ref()
            .ok_or_else(|| anyhow!("this FST cannot build an activity index"))?;
        let blocks = a.blocks()?;
        a.source
            .build(options, budget, cache_dir, self.info.signal_count, |w| {
                blocks.build(w, options)
            })
    }
    fn resolve_activity(
        &self,
        signals: &[SignalRef],
        t0: u64,
        t1: u64,
    ) -> anyhow::Result<Vec<SignalRef>> {
        let a = self
            .activity
            .as_ref()
            .ok_or_else(|| anyhow!("this FST has no activity index"))?;
        let blocks = a.blocks()?;
        let ids: Vec<vtr::SignalId> = signals.iter().map(|s| vtr::SignalId(s.0)).collect();
        Ok(blocks
            .resolve(&ids, t0, t1)?
            .into_iter()
            .map(|s| SignalRef(s.0))
            .collect())
    }
    fn format(&self) -> Option<&'static str> {
        Some("FST")
    }
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
