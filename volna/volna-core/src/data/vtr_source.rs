//! [`Session`] over a VTR file in this process: memory-mapped from a path
//! natively, parsed from an in-memory image on wasm.

use std::sync::Arc;

use anyhow::Context as _;
use vtr::{NodeData, Reader, SignalData, SignalKind, SignalValue};

use super::history::SignalHistory;
use super::source::{Generator, Hierarchy, ScopeRole, SignalRef, TraceInfo};
use super::transactions::TrackRef;
use super::value::{Bit, SignalShape, WaveValue};
use crate::session::Session;

pub struct LocalSession {
    pub(super) reader: Arc<Reader>,
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
        let reader = Arc::new(reader);
        let hierarchy = build_hierarchy(Arc::clone(&reader));
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

/// The reader owns declaration columns and names; only dense identity and
/// membership indexes belong to this view.
struct ReaderHierarchy {
    reader: Arc<Reader>,
    scopes: vtr::hierarchy_index::NodeIndex,
    vars: vtr::hierarchy_index::NodeIndex,
    generators: vtr::hierarchy_index::NodeIndex,
    roots: Vec<u32>,
    root_owner: Option<usize>,
    root_vars: Vec<u32>,
    top: Option<usize>,
}
impl std::fmt::Debug for ReaderHierarchy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaderHierarchy")
            .field("scopes", &self.scopes.len())
            .field("vars", &self.vars.len())
            .finish()
    }
}
impl ReaderHierarchy {
    fn node(&self, id: usize) -> vtr::NodeId {
        self.scopes
            .node(id - usize::from(self.top.is_some_and(|top| id > top)))
    }
    fn scope_id(&self, node: vtr::NodeId) -> Option<usize> {
        self.scopes
            .ordinal(node)
            .map(|id| id + usize::from(self.top.is_some_and(|top| id >= top)))
    }
}
impl super::source::HierarchySource for ReaderHierarchy {
    fn scope_count(&self) -> usize {
        self.scopes.len() + usize::from(self.top.is_some())
    }
    fn var_count(&self) -> usize {
        self.vars.len()
    }
    fn roots(&self) -> super::source::Ids<'_> {
        super::source::Ids::from_slice(&self.roots)
    }
    fn scope(&self, id: usize) -> super::source::ScopeView<'_> {
        use super::source::ScopeView;
        let (name, kind, component, role) = if Some(id) == self.top {
            ("(top)", "module", "", ScopeRole::Scope)
        } else {
            let node = self.node(id);
            let (kind, component, role) = match self.reader.hierarchy().node_data(node) {
                NodeData::Scope {
                    scope_type,
                    component,
                } => (
                    scope_type.name(),
                    self.reader.str(component),
                    ScopeRole::Scope,
                ),
                NodeData::Stream { kind } => (
                    self.reader.str(kind),
                    "",
                    ScopeRole::Stream {
                        track: TrackRef(node.0),
                    },
                ),
                _ => unreachable!(),
            };
            (
                self.reader.str(self.reader.hierarchy().name(node)),
                kind,
                component,
                role,
            )
        };
        ScopeView {
            name,
            kind,
            component,
            role,
            parent: if Some(id) == self.top {
                None
            } else {
                self.reader
                    .hierarchy()
                    .parent(self.node(id))
                    .and_then(|p| self.scope_id(p))
            },
            children: if Some(id) == self.top {
                super::source::Ids::default()
            } else {
                super::source::Ids::reader(
                    self.scopes.children(self.reader.hierarchy(), self.node(id)),
                )
                .shifted(self.top)
            },
            vars: if Some(id) == self.root_owner {
                super::source::Ids::from_slice(&self.root_vars)
            } else {
                super::source::Ids::reader(
                    self.vars.children(self.reader.hierarchy(), self.node(id)),
                )
            },
            generators: if Some(id) == self.top {
                super::source::Ids::default()
            } else {
                super::source::Ids::reader(
                    self.generators
                        .children(self.reader.hierarchy(), self.node(id)),
                )
            },
        }
    }
    fn var(&self, id: usize) -> super::source::VariableView<'_> {
        let node = self.vars.node(id);
        let h = self.reader.hierarchy();
        let NodeData::Var {
            var_type,
            direction,
            signal,
            ..
        } = h.node_data(node)
        else {
            unreachable!()
        };
        super::source::VariableView {
            name: self.reader.str(h.name(node)),
            scope: h
                .parent(node)
                .and_then(|p| self.scope_id(p))
                .or(self.root_owner)
                .expect("variable scope"),
            shape: shape_of(
                h.signal_kind(signal).expect("signal"),
                h.signal_var_type(signal).expect("signal type"),
            ),
            var_type: var_type.name(),
            direction: direction.into(),
            signal: SignalRef(signal.0),
            enum_table: h.attrs(node).iter().find_map(|(key, value)| {
                if self.reader.str(*key) == "enum_table"
                    && let vtr::Value::U64(id) = value
                {
                    u32::try_from(*id).ok()
                } else {
                    None
                }
            }),
        }
    }
    fn signal(&self, id: usize) -> SignalRef {
        SignalRef(
            self.reader
                .hierarchy()
                .signal_of(self.vars.node(id))
                .expect("variable")
                .0,
        )
    }
    fn resident_bytes(&self) -> u64 {
        self.scopes.resident_bytes()
            + self.vars.resident_bytes()
            + self.generators.resident_bytes()
            + ((self.roots.capacity() + self.root_vars.capacity()) * 4) as u64
    }
}

fn build_hierarchy(reader: Arc<Reader>) -> Hierarchy {
    use vtr::{NodeKind, hierarchy_index::NodeIndex};
    let h = reader.hierarchy();
    let scopes = NodeIndex::new(h, &[NodeKind::Scope, NodeKind::Stream]);
    let vars = NodeIndex::new(h, &[NodeKind::Var]);
    let generator_index = NodeIndex::new(h, &[NodeKind::Generator]);
    let mut root_vars: Vec<_> = h
        .roots()
        .filter_map(|id| vars.ordinal(id).map(|id| id as u32))
        .collect();
    let first_root_var = root_vars.first().map(|&id| vars.node(id as usize));
    // The synthetic scope enters declaration order at the first root variable,
    // exactly as the owned model did. A later literal "(top)" stays distinct.
    let existing_top = h
        .roots()
        .filter(|&node| first_root_var.is_some_and(|first| node.0 < first.0))
        .find(|&node| scopes.ordinal(node).is_some() && reader.str(h.name(node)) == "(top)")
        .and_then(|node| scopes.ordinal(node));
    let top = first_root_var
        .filter(|_| existing_top.is_none())
        .map(|first| {
            (0..scopes.len())
                .take_while(|&id| scopes.node(id).0 < first.0)
                .count()
        });
    let scope_id = |node| {
        scopes
            .ordinal(node)
            .map(|id| id + usize::from(top.is_some_and(|top| id >= top)))
    };
    let mut roots: Vec<_> = h
        .roots()
        .filter_map(|node| scope_id(node).map(|id| id as u32))
        .collect();
    if let Some(top) = top {
        roots.insert(roots.partition_point(|&id| id < top as u32), top as u32);
    }
    let root_owner = existing_top.or(top);
    if let Some(id) = existing_top {
        root_vars.extend(vars.children(h, scopes.node(id)).iter().map(|id| id as u32));
        root_vars.sort_unstable();
    }
    let generators: Vec<_> = (0..generator_index.len())
        .map(|id| {
            let node = generator_index.node(id);
            Generator {
                name: reader.str(h.name(node)).into(),
                stream: scope_id(h.parent(node).expect("stream")).expect("stream scope"),
                track: TrackRef(node.0),
                attributes: h
                    .attrs(node)
                    .iter()
                    .map(|(key, value)| {
                        (
                            reader.str(*key).into(),
                            super::vtr_transactions::value(&reader, value),
                        )
                    })
                    .collect(),
            }
        })
        .collect();
    Hierarchy {
        source: Arc::new(ReaderHierarchy {
            reader,
            scopes,
            vars,
            generators: generator_index,
            roots,
            root_owner,
            root_vars,
            top,
        }),
        generators: Arc::new(generators),
    }
}

impl Session for LocalSession {
    fn resident_bytes(&self) -> u64 {
        self.source_bytes
            + self.reader.hierarchy().resident_bytes()
            + self.reader.strings().resident_bytes()
            + self.hierarchy.resident_bytes()
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
    fn format(&self) -> Option<&'static str> {
        Some("VTR")
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
                8 => Bit::DontCare,
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
    fn always_normal(&self) -> bool {
        !matches!(self.data.kind(), vtr::SignalKind::Bits { states, .. } if states > 2)
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
