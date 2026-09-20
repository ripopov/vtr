//! [`Session`] over a VTR file in this process: memory-mapped from a path
//! natively, parsed from an in-memory image on wasm.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use vtr::{NodeData, Reader, SignalData, SignalKind, SignalValue};

use super::history::SignalHistory;
use super::source::{Generator, Hierarchy, ScopeRole, SignalRef, TraceInfo, Variable};
use super::transactions::TrackRef;
use super::value::{Bit, SignalShape, WaveValue};
use crate::session::Session;

pub struct LocalSession {
    pub(super) reader: Reader,
    pub(super) tracks: Vec<super::transactions::Track>,
    info: TraceInfo,
    hierarchy: Hierarchy,
    source_bytes: u64,
}

impl LocalSession {
    #[cfg(not(target_family = "wasm"))]
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let source_bytes = std::fs::metadata(path)?.len();
        let reader = Reader::open(path).with_context(|| format!("open {}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_reader(name, reader, source_bytes)
    }

    pub fn from_bytes(name: impl Into<String>, bytes: Vec<u8>) -> anyhow::Result<Self> {
        let source_bytes = bytes.capacity() as u64;
        let reader = Reader::from_bytes(bytes).context("parse VTR image")?;
        Self::from_reader(name.into(), reader, source_bytes)
    }

    fn from_reader(name: String, reader: Reader, source_bytes: u64) -> anyhow::Result<Self> {
        let hierarchy = build_hierarchy(&reader);
        let file_text = |name: &str| {
            reader.meta().attrs.iter().find_map(|(key, value)| {
                if reader.strings().get(*key) == name
                    && let vtr::Value::Str(value) = value
                {
                    Some(reader.strings().get(*value).to_owned())
                } else {
                    None
                }
            })
        };
        let info = TraceInfo {
            name,
            design_id: file_text("design.vdb_id"),
            timescale: reader.meta().timescale,
            time_range: reader.time_range().unwrap_or((0, 0)),
            signal_count: reader.signal_count() as usize,
            change_count: None,
            time_unit: file_text("time.unit").filter(|unit| !unit.is_empty()),
        };
        Ok(LocalSession {
            tracks: super::vtr_transactions::tracks(&reader),
            reader,
            info,
            hierarchy,
            source_bytes,
        })
    }
}

fn shape_of(kind: SignalKind, var_type: vtr::VarType) -> SignalShape {
    if var_type == vtr::VarType::Event {
        return SignalShape::Event;
    }
    match kind {
        SignalKind::Bits { width: 1, .. } => SignalShape::Bit,
        SignalKind::Bits { width, .. } => SignalShape::Vector {
            width: width.max(2),
        },
        SignalKind::Real => SignalShape::Real,
        SignalKind::VarLen => SignalShape::Text,
    }
}

fn build_hierarchy(reader: &Reader) -> Hierarchy {
    let h = reader.hierarchy();
    let mut out = Hierarchy::default();
    // Map VTR node ids to our scope ids while walking in declaration order
    // (parents are declared before children, so a plain map suffices).
    let mut scope_of_node: HashMap<usize, usize> = HashMap::new();
    for id in h.ids() {
        let node = h.node(id);
        let parent_scope = node
            .parent
            .and_then(|p| scope_of_node.get(&(p.0 as usize)).copied());
        match node.data {
            NodeData::Scope {
                scope_type,
                component,
            } => {
                let sid = out.push_scope(
                    reader.str(node.name).to_string(),
                    scope_type.name().to_string(),
                    parent_scope,
                );
                scope_of_node.insert(id.0 as usize, sid);
                out.scopes[sid].component = reader.str(component).to_owned();
            }
            NodeData::Stream { kind } => {
                let sid = out.push_scope(
                    reader.str(node.name).to_owned(),
                    reader.str(kind).to_owned(),
                    parent_scope,
                );
                out.scopes[sid].role = ScopeRole::Stream {
                    track: TrackRef(id.0),
                };
                scope_of_node.insert(id.0 as usize, sid);
            }
            NodeData::Generator => {
                if let Some(stream) = parent_scope {
                    let gid = out.generators.len();
                    out.generators.push(Generator {
                        name: reader.str(node.name).to_owned(),
                        stream,
                        track: TrackRef(id.0),
                        attributes: node
                            .attrs
                            .iter()
                            .map(|(k, v)| {
                                (
                                    reader.str(*k).to_owned(),
                                    super::vtr_transactions::value(reader, v),
                                )
                            })
                            .collect(),
                    });
                    out.scopes[stream].generators.push(gid);
                }
            }
            NodeData::Var {
                var_type,
                direction,
                signal,
                ..
            } => {
                let Some(kind) = h.signal_kind(signal) else {
                    continue;
                };
                // Variables outside any scope get a synthetic root scope.
                let sid = match parent_scope {
                    Some(p) => p,
                    None => root_scope(&mut out),
                };
                let vid = out.vars.len();
                out.vars.push(Variable {
                    name: reader.str(node.name).to_string(),
                    scope: sid,
                    shape: shape_of(kind, h.signal_var_type(signal).expect("declared signal")),
                    var_type: var_type.name().to_string(),
                    direction: direction.into(),
                    signal: SignalRef(signal.0),
                    enum_table: node.attrs.iter().find_map(|(key, value)| {
                        if reader.str(*key) == "enum_table"
                            && let vtr::Value::U64(id) = value
                        {
                            u32::try_from(*id).ok()
                        } else {
                            None
                        }
                    }),
                });
                out.scopes[sid].vars.push(vid);
            }
            // Streams, generators and enum tables are not waveforms.
            _ => {}
        }
    }
    out
}

fn root_scope(h: &mut Hierarchy) -> usize {
    if let Some(&r) = h.roots.iter().find(|&&r| h.scopes[r].name == "(top)") {
        return r;
    }
    h.push_scope("(top)".into(), "module".into(), None)
}

impl Session for LocalSession {
    fn resident_bytes(&self) -> u64 {
        self.source_bytes
    }
    fn load_track(
        &self,
        track: super::transactions::TrackRef,
    ) -> anyhow::Result<super::loaded_tracks::LoadedTrack> {
        super::vtr_transactions::load_track(self, track)
    }
    fn tracks(&self) -> &[super::transactions::Track] {
        &self.tracks
    }
    fn capabilities(&self) -> crate::session::Capabilities {
        crate::session::Capabilities {
            waveforms: true,
            transactions: true,
            relations: true,
        }
    }
    fn info(&self) -> &TraceInfo {
        &self.info
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        let data = self
            .reader
            .load_signal(vtr::SignalId(signal.0))
            .with_context(|| format!("load signal {}", signal.0))?;
        Ok(Arc::new(VtrHistory {
            shape: shape_of(
                data.kind(),
                self.reader.signal_var_type(vtr::SignalId(signal.0))?,
            ),
            data,
        }))
    }
}

struct VtrHistory {
    shape: SignalShape,
    data: SignalData,
}

fn to_wave_value(v: SignalValue<'_>) -> WaveValue {
    match v {
        SignalValue::Bits { .. } => WaveValue::Bits(v.to_ascii()),
        SignalValue::Real(r) => WaveValue::Real(r),
        SignalValue::VarLen(bytes) => WaveValue::Bytes(bytes.to_vec()),
    }
}

fn first_bit(v: SignalValue<'_>) -> Bit {
    match v {
        SignalValue::Bits { states, data, .. } => {
            let code = vtr::signal::get_code(data, states, 0);
            match code {
                0 => Bit::Zero,
                1 => Bit::One,
                2 | 4 | 5 => Bit::X,
                3 => Bit::Z,
                _ => Bit::Other,
            }
        }
        _ => Bit::Other,
    }
}

impl SignalHistory for VtrHistory {
    fn value_view(&self, i: Option<usize>) -> super::value_view::ValueView<'_> {
        use super::value_view::{LogicView, ValueView};
        match i.map_or_else(|| self.data.initial(), |i| self.data.get(i)) {
            SignalValue::Bits {
                width,
                states,
                data,
            } => ValueView::Logic(LogicView::packed_lsb(width, states, data)),
            SignalValue::Real(value) => ValueView::Real(value),
            SignalValue::VarLen(bytes) => ValueView::Bytes(bytes.into()),
        }
    }
    fn resident_bytes(&self) -> u64 {
        let value_bytes = match self.shape {
            SignalShape::Bit | SignalShape::Event => 1,
            SignalShape::Vector { width } => u64::from(width).div_ceil(2).max(1),
            SignalShape::Real => 8,
            // Variable-length data has backend-owned offsets as well as bytes;
            // use a conservative fixed floor when the opaque reader does not
            // expose allocation capacities.
            SignalShape::Text => 24,
        };
        (self.data.len() as u64).saturating_mul(8 + value_bytes)
    }

    fn shape(&self) -> SignalShape {
        self.shape
    }
    fn len(&self) -> usize {
        self.data.len()
    }
    fn time(&self, i: usize) -> u64 {
        self.data.times()[i]
    }
    fn index_at(&self, t: u64) -> Option<usize> {
        self.data.index_at(t)
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        match i {
            None => to_wave_value(self.data.initial()),
            Some(i) => to_wave_value(self.data.get(i)),
        }
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        match i {
            None => first_bit(self.data.initial()),
            Some(i) => first_bit(self.data.get(i)),
        }
    }
}
