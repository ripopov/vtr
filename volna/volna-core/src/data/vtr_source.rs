//! [`Session`] over a VTR file in this process: memory-mapped from a path
//! natively, parsed from an in-memory image on wasm.

use std::sync::Arc;

use anyhow::Context as _;
use vtr::{NodeData, Reader, SignalData, SignalKind, SignalValue};

use super::history::SignalHistory;
use super::source::{Direction, Hierarchy, Scope, SignalRef, TraceInfo, Variable};
use super::value::{Bit, SignalShape, WaveValue};
use crate::session::Session;

pub struct LocalSession {
    pub(super) reader: Arc<Reader>,
    pub(super) tracks: Vec<super::transactions::Track>,
    info: TraceInfo,
    hierarchy: Hierarchy,
}

impl LocalSession {
    #[cfg(not(target_family = "wasm"))]
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let reader = Reader::open(path).with_context(|| format!("open {}", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_reader(name, reader)
    }

    pub fn from_bytes(name: impl Into<String>, bytes: Vec<u8>) -> anyhow::Result<Self> {
        let reader = Reader::from_bytes(bytes).context("parse VTR image")?;
        Self::from_reader(name.into(), reader)
    }

    fn from_reader(name: String, reader: Reader) -> anyhow::Result<Self> {
        let hierarchy = build_hierarchy(&reader);
        let info = TraceInfo {
            name,
            design_id: reader.meta().attrs.iter().find_map(|(key, value)| {
                if reader.strings().get(*key) == "design.vdb_id"
                    && let vtr::Value::Str(value) = value
                {
                    Some(reader.strings().get(*value).to_owned())
                } else {
                    None
                }
            }),
            timescale: reader.meta().timescale,
            time_range: reader.time_range().unwrap_or((0, 0)),
            signal_count: reader.signal_count() as usize,
            change_count: None,
        };
        Ok(LocalSession {
            tracks: super::vtr_transactions::tracks(&reader),
            reader: Arc::new(reader),
            info,
            hierarchy,
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
    let mut scope_of_node: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    for id in h.ids() {
        let node = h.node(id);
        let parent_scope = node
            .parent
            .and_then(|p| scope_of_node.get(&(p.0 as usize)).copied());
        match node.data {
            NodeData::Scope { scope_type, .. } => {
                let sid = out.scopes.len();
                out.scopes.push(Scope {
                    synthetic: false,
                    name: reader.str(node.name).to_string(),
                    kind: scope_type.name().to_string(),
                    parent: parent_scope,
                    children: Vec::new(),
                    vars: Vec::new(),
                });
                match parent_scope {
                    Some(p) => out.scopes[p].children.push(sid),
                    None => out.roots.push(sid),
                }
                scope_of_node.insert(id.0 as usize, sid);
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
                    direction: match direction {
                        vtr::Direction::Input => Direction::Input,
                        vtr::Direction::Output => Direction::Output,
                        vtr::Direction::InOut => Direction::InOut,
                        _ => Direction::None,
                    },
                    signal: SignalRef(signal.0),
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
    if let Some(&r) = h.roots.iter().find(|&&r| h.scopes[r].synthetic) {
        return r;
    }
    let sid = h.scopes.len();
    h.scopes.push(Scope {
        synthetic: true,
        name: "(top)".into(),
        kind: "module".into(),
        parent: None,
        children: Vec::new(),
        vars: Vec::new(),
    });
    h.roots.push(sid);
    sid
}

impl Session for LocalSession {
    #[cfg(not(target_family = "wasm"))]
    fn open_queries(
        &self,
        budget: vtr_query::Budget,
    ) -> Option<vtr_query::Result<vtr_query::local_session::OpenFuture>> {
        Some(vtr_query::local_session::LocalSession::from_reader(
            self.reader.clone(),
            budget,
            4,
        ))
    }

    fn transactions(&self) -> Option<&dyn super::transactions::TransactionQueries> {
        Some(self)
    }
    fn relations(&self) -> Option<&dyn super::transactions::RelationQueries> {
        Some(self)
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
