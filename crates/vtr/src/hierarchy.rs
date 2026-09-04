//! Design hierarchy: scopes, variables, streams, generators, enum tables.
//!
//! The hierarchy is a forest of nodes. Every node has a kind, an optional
//! parent, a name and an ordered list of typed attributes. Nodes are stored
//! append-only in `Hierarchy` sections; the node id is its position.

use crate::error::{Error, Result};
use crate::strings::StrId;
use crate::value::{self, Value};
use crate::varint::{self, Reader};

/// Index of a hierarchy node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Index of a signal (value stream). Several variables may alias one signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignalId(pub u32);

/// Node kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NodeKind {
    Scope = 1,
    Var = 2,
    Stream = 3,
    Generator = 4,
    EnumTable = 5,
}

impl NodeKind {
    pub fn from_u8(v: u8) -> Result<NodeKind> {
        Ok(match v {
            1 => NodeKind::Scope,
            2 => NodeKind::Var,
            3 => NodeKind::Stream,
            4 => NodeKind::Generator,
            5 => NodeKind::EnumTable,
            _ => return Err(Error::Corrupt("unknown node kind")),
        })
    }
}

/// Scope types. Values 0..=22 are identical to FST's `FST_ST_*` codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ScopeType {
    Module = 0,
    Task = 1,
    Function = 2,
    Begin = 3,
    Fork = 4,
    Generate = 5,
    Struct = 6,
    Union = 7,
    Class = 8,
    Interface = 9,
    Package = 10,
    Program = 11,
    VhdlArchitecture = 12,
    VhdlProcedure = 13,
    VhdlFunction = 14,
    VhdlRecord = 15,
    VhdlProcess = 16,
    VhdlBlock = 17,
    VhdlForGenerate = 18,
    VhdlIfGenerate = 19,
    VhdlGenerate = 20,
    VhdlPackage = 21,
    SvArray = 22,
    /// Generic container with no HDL meaning (e.g. an OpenTelemetry resource,
    /// a SystemC module, a simulator component).
    Generic = 64,
    /// SystemC `sc_module`.
    ScModule = 65,
    /// OpenTelemetry resource.
    Resource = 66,
    /// OpenTelemetry instrumentation scope.
    InstrumentationScope = 67,
    /// A CPU core / hardware thread (pipeline traces).
    Core = 68,
    /// Any code not listed above (preserved verbatim).
    Other(u16),
}

impl ScopeType {
    pub fn code(self) -> u16 {
        match self {
            ScopeType::Other(c) => c,
            _ => self.fixed_code(),
        }
    }
    fn fixed_code(self) -> u16 {
        // Safe: all non-Other variants have explicit discriminants.
        match self {
            ScopeType::Module => 0,
            ScopeType::Task => 1,
            ScopeType::Function => 2,
            ScopeType::Begin => 3,
            ScopeType::Fork => 4,
            ScopeType::Generate => 5,
            ScopeType::Struct => 6,
            ScopeType::Union => 7,
            ScopeType::Class => 8,
            ScopeType::Interface => 9,
            ScopeType::Package => 10,
            ScopeType::Program => 11,
            ScopeType::VhdlArchitecture => 12,
            ScopeType::VhdlProcedure => 13,
            ScopeType::VhdlFunction => 14,
            ScopeType::VhdlRecord => 15,
            ScopeType::VhdlProcess => 16,
            ScopeType::VhdlBlock => 17,
            ScopeType::VhdlForGenerate => 18,
            ScopeType::VhdlIfGenerate => 19,
            ScopeType::VhdlGenerate => 20,
            ScopeType::VhdlPackage => 21,
            ScopeType::SvArray => 22,
            ScopeType::Generic => 64,
            ScopeType::ScModule => 65,
            ScopeType::Resource => 66,
            ScopeType::InstrumentationScope => 67,
            ScopeType::Core => 68,
            ScopeType::Other(c) => c,
        }
    }
    pub fn from_code(c: u16) -> ScopeType {
        use ScopeType::*;
        match c {
            0 => Module,
            1 => Task,
            2 => Function,
            3 => Begin,
            4 => Fork,
            5 => Generate,
            6 => Struct,
            7 => Union,
            8 => Class,
            9 => Interface,
            10 => Package,
            11 => Program,
            12 => VhdlArchitecture,
            13 => VhdlProcedure,
            14 => VhdlFunction,
            15 => VhdlRecord,
            16 => VhdlProcess,
            17 => VhdlBlock,
            18 => VhdlForGenerate,
            19 => VhdlIfGenerate,
            20 => VhdlGenerate,
            21 => VhdlPackage,
            22 => SvArray,
            64 => Generic,
            65 => ScModule,
            66 => Resource,
            67 => InstrumentationScope,
            68 => Core,
            c => Other(c),
        }
    }
    pub fn name(self) -> &'static str {
        use ScopeType::*;
        match self {
            Module => "module",
            Task => "task",
            Function => "function",
            Begin => "begin",
            Fork => "fork",
            Generate => "generate",
            Struct => "struct",
            Union => "union",
            Class => "class",
            Interface => "interface",
            Package => "package",
            Program => "program",
            VhdlArchitecture => "vhdl_architecture",
            VhdlProcedure => "vhdl_procedure",
            VhdlFunction => "vhdl_function",
            VhdlRecord => "vhdl_record",
            VhdlProcess => "vhdl_process",
            VhdlBlock => "vhdl_block",
            VhdlForGenerate => "vhdl_for_generate",
            VhdlIfGenerate => "vhdl_if_generate",
            VhdlGenerate => "vhdl_generate",
            VhdlPackage => "vhdl_package",
            SvArray => "sv_array",
            Generic => "generic",
            ScModule => "sc_module",
            Resource => "resource",
            InstrumentationScope => "instrumentation_scope",
            Core => "core",
            Other(_) => "other",
        }
    }
}

/// Variable types. Values 0..=29 are identical to FST's `FST_VT_*` codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarType {
    Event,
    Integer,
    Parameter,
    Real,
    RealParameter,
    Reg,
    Supply0,
    Supply1,
    Time,
    Tri,
    TriAnd,
    TriOr,
    TriReg,
    Tri0,
    Tri1,
    WAnd,
    Wire,
    WOr,
    Port,
    SparseArray,
    RealTime,
    /// Variable-length byte string.
    String,
    Bit,
    Logic,
    Int,
    ShortInt,
    LongInt,
    Byte,
    Enum,
    ShortReal,
    /// Generic bit vector (no HDL type).
    Bits,
    /// Generic byte blob (variable length).
    Bytes,
    Other(u16),
}

impl VarType {
    pub fn code(self) -> u16 {
        use VarType::*;
        match self {
            Event => 0,
            Integer => 1,
            Parameter => 2,
            Real => 3,
            RealParameter => 4,
            Reg => 5,
            Supply0 => 6,
            Supply1 => 7,
            Time => 8,
            Tri => 9,
            TriAnd => 10,
            TriOr => 11,
            TriReg => 12,
            Tri0 => 13,
            Tri1 => 14,
            WAnd => 15,
            Wire => 16,
            WOr => 17,
            Port => 18,
            SparseArray => 19,
            RealTime => 20,
            String => 21,
            Bit => 22,
            Logic => 23,
            Int => 24,
            ShortInt => 25,
            LongInt => 26,
            Byte => 27,
            Enum => 28,
            ShortReal => 29,
            Bits => 64,
            Bytes => 65,
            Other(c) => c,
        }
    }
    pub fn from_code(c: u16) -> VarType {
        use VarType::*;
        match c {
            0 => Event,
            1 => Integer,
            2 => Parameter,
            3 => Real,
            4 => RealParameter,
            5 => Reg,
            6 => Supply0,
            7 => Supply1,
            8 => Time,
            9 => Tri,
            10 => TriAnd,
            11 => TriOr,
            12 => TriReg,
            13 => Tri0,
            14 => Tri1,
            15 => WAnd,
            16 => Wire,
            17 => WOr,
            18 => Port,
            19 => SparseArray,
            20 => RealTime,
            21 => String,
            22 => Bit,
            23 => Logic,
            24 => Int,
            25 => ShortInt,
            26 => LongInt,
            27 => Byte,
            28 => Enum,
            29 => ShortReal,
            64 => Bits,
            65 => Bytes,
            c => Other(c),
        }
    }
    pub fn name(self) -> &'static str {
        use VarType::*;
        match self {
            Event => "event",
            Integer => "integer",
            Parameter => "parameter",
            Real => "real",
            RealParameter => "real_parameter",
            Reg => "reg",
            Supply0 => "supply0",
            Supply1 => "supply1",
            Time => "time",
            Tri => "tri",
            TriAnd => "triand",
            TriOr => "trior",
            TriReg => "trireg",
            Tri0 => "tri0",
            Tri1 => "tri1",
            WAnd => "wand",
            Wire => "wire",
            WOr => "wor",
            Port => "port",
            SparseArray => "sparray",
            RealTime => "realtime",
            String => "string",
            Bit => "bit",
            Logic => "logic",
            Int => "int",
            ShortInt => "shortint",
            LongInt => "longint",
            Byte => "byte",
            Enum => "enum",
            ShortReal => "shortreal",
            Bits => "bits",
            Bytes => "bytes",
            Other(_) => "other",
        }
    }
}

/// Port direction (identical to FST `FST_VD_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Direction {
    Implicit = 0,
    Input = 1,
    Output = 2,
    InOut = 3,
    Buffer = 4,
    Linkage = 5,
}

impl Direction {
    pub fn from_u8(v: u8) -> Direction {
        match v {
            1 => Direction::Input,
            2 => Direction::Output,
            3 => Direction::InOut,
            4 => Direction::Buffer,
            5 => Direction::Linkage,
            _ => Direction::Implicit,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Direction::Implicit => "implicit",
            Direction::Input => "input",
            Direction::Output => "output",
            Direction::InOut => "inout",
            Direction::Buffer => "buffer",
            Direction::Linkage => "linkage",
        }
    }
}

/// How a signal's values are encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// Bit vector with `width` bits and 2, 4 or 9 states per bit.
    Bits { width: u32, states: u8 },
    /// IEEE 754 double.
    Real,
    /// Variable-length byte string.
    VarLen,
}

impl SignalKind {
    /// Bytes used by one packed value (`None` for variable-length).
    pub fn packed_len(self) -> Option<usize> {
        match self {
            SignalKind::Bits { width, states } => Some(value::packed_len(width, states)),
            SignalKind::Real => Some(8),
            SignalKind::VarLen => None,
        }
    }
    pub fn width(self) -> u32 {
        match self {
            SignalKind::Bits { width, .. } => width,
            SignalKind::Real => 64,
            SignalKind::VarLen => 0,
        }
    }
    pub fn states(self) -> u8 {
        match self {
            SignalKind::Bits { states, .. } => states,
            _ => 0,
        }
    }
    fn encode(self, out: &mut Vec<u8>) {
        match self {
            SignalKind::Bits { width, states } => {
                out.push(match states {
                    2 => 0,
                    4 => 1,
                    _ => 2,
                });
                varint::put_u64(out, width as u64);
            }
            SignalKind::Real => out.push(3),
            SignalKind::VarLen => out.push(4),
        }
    }
    fn decode(r: &mut Reader) -> Result<SignalKind> {
        Ok(match r.u8()? {
            0 => SignalKind::Bits { width: r.u32()?, states: 2 },
            1 => SignalKind::Bits { width: r.u32()?, states: 4 },
            2 => SignalKind::Bits { width: r.u32()?, states: 9 },
            3 => SignalKind::Real,
            4 => SignalKind::VarLen,
            _ => return Err(Error::Corrupt("unknown signal kind")),
        })
    }
}

/// Kind-specific node payload.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeData {
    Scope {
        scope_type: ScopeType,
        /// Module/entity type name (FST "component"); empty if unknown.
        component: StrId,
    },
    Var {
        var_type: VarType,
        direction: Direction,
        signal: SignalId,
        /// `Some` when this var declares a new signal, `None` for aliases.
        declares: Option<SignalKind>,
    },
    Stream {
        /// Free-form stream kind (FTR "kind", e.g. "TRANSACTOR").
        kind: StrId,
    },
    Generator,
    EnumTable {
        /// (literal, value) pairs; the value is the bit-string spelling.
        entries: Vec<(StrId, StrId)>,
    },
}

impl NodeData {
    pub fn kind(&self) -> NodeKind {
        match self {
            NodeData::Scope { .. } => NodeKind::Scope,
            NodeData::Var { .. } => NodeKind::Var,
            NodeData::Stream { .. } => NodeKind::Stream,
            NodeData::Generator => NodeKind::Generator,
            NodeData::EnumTable { .. } => NodeKind::EnumTable,
        }
    }
}

/// One hierarchy node.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub name: StrId,
    pub data: NodeData,
    pub attrs: Vec<(StrId, Value)>,
}

impl Node {
    pub fn kind(&self) -> NodeKind {
        self.data.kind()
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.kind() as u8);
        varint::put_u64(out, self.parent.map(|p| p.0 as u64 + 1).unwrap_or(0));
        varint::put_u64(out, self.name.0 as u64);
        match &self.data {
            NodeData::Scope { scope_type, component } => {
                varint::put_u64(out, scope_type.code() as u64);
                varint::put_u64(out, component.0 as u64);
            }
            NodeData::Var { var_type, direction, signal, declares } => {
                varint::put_u64(out, var_type.code() as u64);
                out.push(*direction as u8);
                varint::put_u64(out, signal.0 as u64);
                match declares {
                    Some(k) => {
                        out.push(1);
                        k.encode(out);
                    }
                    None => out.push(0),
                }
            }
            NodeData::Stream { kind } => varint::put_u64(out, kind.0 as u64),
            NodeData::Generator => {}
            NodeData::EnumTable { entries } => {
                varint::put_u64(out, entries.len() as u64);
                for (l, v) in entries {
                    varint::put_u64(out, l.0 as u64);
                    varint::put_u64(out, v.0 as u64);
                }
            }
        }
        value::encode_attrs(&self.attrs, out);
    }

    pub fn decode(r: &mut Reader) -> Result<Node> {
        let kind = NodeKind::from_u8(r.u8()?)?;
        let parent = match r.u32()? {
            0 => None,
            p => Some(NodeId(p - 1)),
        };
        let name = StrId(r.u32()?);
        let data = match kind {
            NodeKind::Scope => NodeData::Scope {
                scope_type: ScopeType::from_code(r.u32()? as u16),
                component: StrId(r.u32()?),
            },
            NodeKind::Var => {
                let var_type = VarType::from_code(r.u32()? as u16);
                let direction = Direction::from_u8(r.u8()?);
                let signal = SignalId(r.u32()?);
                let declares = if r.u8()? != 0 { Some(SignalKind::decode(r)?) } else { None };
                NodeData::Var { var_type, direction, signal, declares }
            }
            NodeKind::Stream => NodeData::Stream { kind: StrId(r.u32()?) },
            NodeKind::Generator => NodeData::Generator,
            NodeKind::EnumTable => {
                let n = r.usize()?;
                if n > r.remaining() {
                    return Err(Error::Corrupt("enum table too long"));
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let l = StrId(r.u32()?);
                    let v = StrId(r.u32()?);
                    entries.push((l, v));
                }
                NodeData::EnumTable { entries }
            }
        };
        let attrs = value::decode_attrs(r)?;
        Ok(Node { parent, name, data, attrs })
    }
}

/// Reader-side hierarchy: all nodes plus derived indexes.
#[derive(Debug, Default)]
pub struct Hierarchy {
    pub nodes: Vec<Node>,
    /// Signal table, indexed by `SignalId`.
    pub signals: Vec<SignalKind>,
    /// For every signal, the first var that declared it.
    pub signal_var: Vec<NodeId>,
    /// Children in declaration order (CSR): `child_start[n]..child_start[n+1]` into `children`.
    child_start: Vec<u32>,
    children: Vec<u32>,
    roots: Vec<u32>,
    indexed: bool,
}

impl Hierarchy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a hierarchy chunk (`first_node`, `count`, nodes...).
    pub fn add_chunk(&mut self, payload: &[u8]) -> Result<()> {
        let mut r = Reader::new(payload);
        let first = r.usize()?;
        let count = r.usize()?;
        if first != self.nodes.len() {
            return Err(Error::Corrupt("hierarchy chunk out of order"));
        }
        self.nodes.reserve(count);
        for _ in 0..count {
            let node = Node::decode(&mut r)?;
            if let NodeData::Var { signal, declares, .. } = &node.data {
                if let Some(k) = declares {
                    if signal.0 as usize != self.signals.len() {
                        return Err(Error::Corrupt("signal declared out of order"));
                    }
                    self.signals.push(*k);
                    self.signal_var.push(NodeId(self.nodes.len() as u32));
                } else if signal.0 as usize >= self.signals.len() {
                    return Err(Error::Corrupt("alias of unknown signal"));
                }
            }
            if let Some(p) = node.parent {
                if p.0 as usize >= self.nodes.len() {
                    return Err(Error::Corrupt("node parent out of range"));
                }
            }
            self.nodes.push(node);
        }
        self.indexed = false;
        Ok(())
    }

    /// Builds the children index (idempotent).
    pub fn build_index(&mut self) {
        if self.indexed {
            return;
        }
        let n = self.nodes.len();
        let mut counts = vec![0u32; n + 1];
        let mut nroots = 0;
        for node in &self.nodes {
            match node.parent {
                Some(p) => counts[p.0 as usize + 1] += 1,
                None => nroots += 1,
            }
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut fill = counts.clone();
        let mut children = vec![0u32; n - nroots];
        let mut roots = Vec::with_capacity(nroots);
        for (i, node) in self.nodes.iter().enumerate() {
            match node.parent {
                Some(p) => {
                    children[fill[p.0 as usize] as usize] = i as u32;
                    fill[p.0 as usize] += 1;
                }
                None => roots.push(i as u32),
            }
        }
        self.child_start = counts;
        self.children = children;
        self.roots = roots;
        self.indexed = true;
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Top-level nodes in declaration order. Requires [`build_index`](Self::build_index).
    pub fn roots(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.roots.iter().map(|&i| NodeId(i))
    }

    /// Children of `id` in declaration order. Requires [`build_index`](Self::build_index).
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let i = id.0 as usize;
        let (a, b) = if i + 1 < self.child_start.len() {
            (self.child_start[i] as usize, self.child_start[i + 1] as usize)
        } else {
            (0, 0)
        };
        self.children[a..b].iter().map(|&c| NodeId(c))
    }

    pub fn signal_kind(&self, s: SignalId) -> Option<SignalKind> {
        self.signals.get(s.0 as usize).copied()
    }

    /// All nodes of a given kind in declaration order.
    pub fn nodes_of_kind(&self, kind: NodeKind) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().enumerate().filter(move |(_, n)| n.kind() == kind).map(|(i, _)| NodeId(i as u32))
    }
}
