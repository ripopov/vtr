//! Design hierarchy: scopes, variables, generators, enum tables.

use crate::error::{Error, Result};
use crate::strings::StrId;
use crate::value::{self, Value};
use crate::varint::{self, Reader};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignalId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NodeKind {
    Scope = 1,
    Var = 2,
    Generator = 3,
    EnumTable = 4,
}

impl NodeKind {
    pub fn from_u8(v: u8) -> Result<NodeKind> {
        Ok(match v {
            1 => NodeKind::Scope,
            2 => NodeKind::Var,
            3 => NodeKind::Generator,
            4 => NodeKind::EnumTable,
            _ => return Err(Error::Corrupt("unknown node kind")),
        })
    }
}

#[must_use]
pub fn parent_allowed(_child: NodeKind, parent: Option<NodeKind>) -> bool {
    matches!(parent, None | Some(NodeKind::Scope))
}

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
    Generic = 64,
    ScModule = 65,
    Resource = 66,
    InstrumentationScope = 67,
    Core = 68,
    Other(u16),
}

impl ScopeType {
    #[must_use]
    pub fn code(self) -> u16 {
        match self {
            ScopeType::Other(c) => c,
            _ => self.fixed_code(),
        }
    }
    fn fixed_code(self) -> u16 {
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
    #[must_use]
    pub fn from_code(c: u16) -> ScopeType {
        use ScopeType::{
            Begin, Class, Core, Fork, Function, Generate, Generic, InstrumentationScope, Interface,
            Module, Other, Package, Program, Resource, ScModule, Struct, SvArray, Task, Union,
            VhdlArchitecture, VhdlBlock, VhdlForGenerate, VhdlFunction, VhdlGenerate,
            VhdlIfGenerate, VhdlPackage, VhdlProcedure, VhdlProcess, VhdlRecord,
        };
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
    #[must_use]
    pub fn name(self) -> &'static str {
        use ScopeType::{
            Begin, Class, Core, Fork, Function, Generate, Generic, InstrumentationScope, Interface,
            Module, Other, Package, Program, Resource, ScModule, Struct, SvArray, Task, Union,
            VhdlArchitecture, VhdlBlock, VhdlForGenerate, VhdlFunction, VhdlGenerate,
            VhdlIfGenerate, VhdlPackage, VhdlProcedure, VhdlProcess, VhdlRecord,
        };
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
    String,
    Bit,
    Logic,
    Int,
    ShortInt,
    LongInt,
    Byte,
    Enum,
    ShortReal,
    Bits,
    Bytes,
    Other(u16),
}

impl VarType {
    #[must_use]
    pub fn code(self) -> u16 {
        use VarType::{
            Bit, Bits, Byte, Bytes, Enum, Event, Int, Integer, Logic, LongInt, Other, Parameter,
            Port, Real, RealParameter, RealTime, Reg, ShortInt, ShortReal, SparseArray, String,
            Supply0, Supply1, Time, Tri, Tri0, Tri1, TriAnd, TriOr, TriReg, WAnd, WOr, Wire,
        };
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
    #[must_use]
    pub fn from_code(c: u16) -> VarType {
        use VarType::{
            Bit, Bits, Byte, Bytes, Enum, Event, Int, Integer, Logic, LongInt, Other, Parameter,
            Port, Real, RealParameter, RealTime, Reg, ShortInt, ShortReal, SparseArray, String,
            Supply0, Supply1, Time, Tri, Tri0, Tri1, TriAnd, TriOr, TriReg, WAnd, WOr, Wire,
        };
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
    #[must_use]
    pub fn name(self) -> &'static str {
        use VarType::{
            Bit, Bits, Byte, Bytes, Enum, Event, Int, Integer, Logic, LongInt, Other, Parameter,
            Port, Real, RealParameter, RealTime, Reg, ShortInt, ShortReal, SparseArray, String,
            Supply0, Supply1, Time, Tri, Tri0, Tri1, TriAnd, TriOr, TriReg, WAnd, WOr, Wire,
        };
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
    #[must_use]
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
    #[must_use]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalKind {
    Bits { width: u32, states: u8 },
    Real,
    VarLen,
}

impl SignalKind {
    #[must_use]
    pub const fn bits(width: u32, states: crate::signal::LogicStates) -> Self {
        Self::Bits {
            width,
            states: states as u8,
        }
    }

    #[must_use]
    pub fn packed_len(self) -> Option<usize> {
        match self {
            SignalKind::Bits { width, states } => Some(value::packed_len(width, states)),
            SignalKind::Real => Some(8),
            SignalKind::VarLen => None,
        }
    }
    #[must_use]
    pub fn width(self) -> u32 {
        match self {
            SignalKind::Bits { width, .. } => width,
            SignalKind::Real => 64,
            SignalKind::VarLen => 0,
        }
    }
    #[must_use]
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
                varint::put_u64(out, u64::from(width));
            }
            SignalKind::Real => out.push(3),
            SignalKind::VarLen => out.push(4),
        }
    }
    fn decode(r: &mut Reader) -> Result<SignalKind> {
        Ok(match r.u8()? {
            0 => SignalKind::Bits {
                width: r.u32()?,
                states: 2,
            },
            1 => SignalKind::Bits {
                width: r.u32()?,
                states: 4,
            },
            2 => SignalKind::Bits {
                width: r.u32()?,
                states: 9,
            },
            3 => SignalKind::Real,
            4 => SignalKind::VarLen,
            _ => return Err(Error::Corrupt("unknown signal kind")),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeData {
    Scope {
        scope_type: ScopeType,
        component: StrId,
    },
    Var {
        var_type: VarType,
        direction: Direction,
        signal: SignalId,
        declares: Option<SignalKind>,
    },
    Generator {
        kind: StrId,
    },
    EnumTable {
        entries: Vec<(StrId, StrId)>,
    },
}

impl NodeData {
    #[must_use]
    pub fn kind(&self) -> NodeKind {
        match self {
            NodeData::Scope { .. } => NodeKind::Scope,
            NodeData::Var { .. } => NodeKind::Var,
            NodeData::Generator { .. } => NodeKind::Generator,
            NodeData::EnumTable { .. } => NodeKind::EnumTable,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub name: StrId,
    pub data: NodeData,
    pub attrs: Vec<(StrId, Value)>,
}

impl Node {
    #[must_use]
    pub fn kind(&self) -> NodeKind {
        self.data.kind()
    }

    #[must_use]
    pub fn declares_signal(&self) -> bool {
        matches!(
            self.data,
            NodeData::Var {
                declares: Some(_),
                ..
            }
        )
    }

    pub fn encode(&self, index: u32, next_signal: u32, out: &mut Vec<u8>) {
        out.push(self.kind() as u8);
        varint::put_u64(out, self.parent.map_or(0, |p| u64::from(index - p.0)));
        varint::put_u64(out, u64::from(self.name.0));
        match &self.data {
            NodeData::Scope {
                scope_type,
                component,
            } => {
                varint::put_u64(out, u64::from(scope_type.code()));
                varint::put_u64(out, u64::from(component.0));
            }
            NodeData::Var {
                var_type,
                direction,
                signal,
                declares,
            } => {
                varint::put_u64(out, u64::from(var_type.code()));
                out.push(*direction as u8);
                varint::put_u64(out, u64::from(next_signal - signal.0));
                match declares {
                    Some(k) => {
                        out.push(1);
                        k.encode(out);
                    }
                    None => out.push(0),
                }
            }
            NodeData::Generator { kind } => varint::put_u64(out, u64::from(kind.0)),
            NodeData::EnumTable { entries } => {
                varint::put_u64(out, entries.len() as u64);
                for (l, v) in entries {
                    varint::put_u64(out, u64::from(l.0));
                    varint::put_u64(out, u64::from(v.0));
                }
            }
        }
        value::encode_attrs(&self.attrs, out);
    }

    pub fn decode(r: &mut Reader, index: u32, next_signal: u32) -> Result<Node> {
        let kind = NodeKind::from_u8(r.u8()?)?;
        let parent = match r.u32()? {
            0 => None,
            d if d <= index => Some(NodeId(index - d)),
            _ => return Err(Error::Corrupt("node parent out of range")),
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
                let back = r.u32()?;
                if back > next_signal {
                    return Err(Error::Corrupt("alias of unknown signal"));
                }
                let signal = SignalId(next_signal - back);
                let declares = if r.u8()? != 0 {
                    Some(SignalKind::decode(r)?)
                } else {
                    None
                };
                if declares.is_some() && back != 0 {
                    return Err(Error::Corrupt("signal declared out of order"));
                }
                if declares.is_none() && back == 0 {
                    return Err(Error::Corrupt("alias of unknown signal"));
                }
                NodeData::Var {
                    var_type,
                    direction,
                    signal,
                    declares,
                }
            }
            NodeKind::Generator => NodeData::Generator {
                kind: StrId(r.u32()?),
            },
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
        Ok(Node {
            parent,
            name,
            data,
            attrs,
        })
    }
}

const NO_NODE: u32 = u32::MAX;

#[derive(Debug, Default)]
pub struct Hierarchy {
    kind: Vec<u8>,
    parent: Vec<u32>,
    name: Vec<u32>,
    w0: Vec<u32>,
    w1: Vec<u32>,
    enum_tables: Vec<Vec<(StrId, StrId)>>,
    attr_node: Vec<u32>,
    attr_kv: Vec<(StrId, Value)>,
    pub signals: Vec<SignalKind>,
    pub signal_var: Vec<NodeId>,
    child_start: Vec<u32>,
    children: Vec<u32>,
    roots: Vec<u32>,
    indexed: bool,
}

impl Hierarchy {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_chunk(&mut self, payload: &[u8]) -> Result<()> {
        let mut r = Reader::new(payload);
        let first = r.usize()?;
        let count = r.usize()?;
        if first != self.len() {
            return Err(Error::Corrupt("hierarchy chunk out of order"));
        }
        self.kind.reserve(count);
        for v in [&mut self.parent, &mut self.name, &mut self.w0, &mut self.w1] {
            v.reserve(count);
        }
        self.signals.reserve(count);
        self.signal_var.reserve(count);
        for _ in 0..count {
            let node = Node::decode(&mut r, self.len() as u32, self.signals.len() as u32)?;
            if !parent_allowed(node.kind(), node.parent.map(|p| self.kind(p))) {
                return Err(Error::Corrupt("node parent has the wrong kind"));
            }
            if let NodeData::Var {
                var_type,
                signal,
                declares: None,
                ..
            } = node.data
            {
                let declared = self
                    .signal_var_type(signal)
                    .ok_or(Error::Corrupt("alias references unknown signal"))?;
                if (var_type == VarType::Event) != (declared == VarType::Event) {
                    return Err(Error::Corrupt(
                        "alias event type differs from signal declaration",
                    ));
                }
            }
            self.push(node);
        }
        self.indexed = false;
        Ok(())
    }

    #[inline]
    pub fn push(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.len() as u32);
        self.kind.push(node.kind() as u8);
        self.parent.push(node.parent.map_or(NO_NODE, |p| p.0));
        self.name.push(node.name.0);
        let (w0, w1) = match node.data {
            NodeData::Scope {
                scope_type,
                component,
            } => (u32::from(scope_type.code()), component.0),
            NodeData::Var {
                var_type,
                direction,
                signal,
                declares,
            } => {
                if let Some(k) = declares {
                    self.signals.push(k);
                    self.signal_var.push(id);
                }
                (
                    u32::from(var_type.code())
                        | (direction as u32) << 16
                        | u32::from(declares.is_some()) << 24,
                    signal.0,
                )
            }
            NodeData::Generator { kind } => (kind.0, 0),
            NodeData::EnumTable { entries } => {
                self.enum_tables.push(entries);
                (self.enum_tables.len() as u32 - 1, 0)
            }
        };
        self.w0.push(w0);
        self.w1.push(w1);
        if !node.attrs.is_empty() {
            self.attr_node
                .resize(self.attr_node.len() + node.attrs.len(), id.0);
            self.attr_kv.extend(node.attrs);
        }
        self.indexed = false;
        id
    }

    pub fn build_index(&mut self) {
        if self.indexed {
            return;
        }
        let n = self.len();
        let mut counts = vec![0u32; n + 1];
        let mut nroots = 0;
        for &p in &self.parent {
            if p == NO_NODE {
                nroots += 1;
            } else {
                counts[p as usize + 1] += 1;
            }
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut fill = counts.clone();
        let mut children = vec![0u32; n - nroots];
        let mut roots = Vec::with_capacity(nroots);
        for (i, &p) in self.parent.iter().enumerate() {
            if p == NO_NODE {
                roots.push(i as u32);
            } else {
                children[fill[p as usize] as usize] = i as u32;
                fill[p as usize] += 1;
            }
        }
        self.child_start = counts;
        self.children = children;
        self.roots = roots;
        self.indexed = true;
    }

    #[must_use]
    pub fn node_data(&self, id: NodeId) -> NodeData {
        let i = id.0 as usize;
        let (w0, w1) = (self.w0[i], self.w1[i]);
        match self.kind(id) {
            NodeKind::Scope => NodeData::Scope {
                scope_type: ScopeType::from_code(w0 as u16),
                component: StrId(w1),
            },
            NodeKind::Var => NodeData::Var {
                var_type: VarType::from_code(w0 as u16),
                direction: Direction::from_u8((w0 >> 16) as u8),
                signal: SignalId(w1),
                declares: if w0 >> 24 != 0 {
                    self.signals.get(w1 as usize).copied()
                } else {
                    None
                },
            },
            NodeKind::Generator => NodeData::Generator { kind: StrId(w0) },
            NodeKind::EnumTable => NodeData::EnumTable {
                entries: self.enum_tables[w0 as usize].clone(),
            },
        }
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> Node {
        Node {
            parent: self.parent(id),
            name: self.name(id),
            data: self.node_data(id),
            attrs: self.attrs(id).to_vec(),
        }
    }

    #[must_use]
    pub fn kind(&self, id: NodeId) -> NodeKind {
        NodeKind::from_u8(self.kind[id.0 as usize]).expect("stored node kind is valid")
    }

    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        match self.parent[id.0 as usize] {
            NO_NODE => None,
            p => Some(NodeId(p)),
        }
    }

    #[must_use]
    pub fn name(&self, id: NodeId) -> StrId {
        StrId(self.name[id.0 as usize])
    }

    pub(crate) fn child_ids(&self, id: NodeId) -> &[u32] {
        let i = id.0 as usize;
        &self.children[self.child_start[i] as usize..self.child_start[i + 1] as usize]
    }

    #[must_use]
    pub fn signal_of(&self, id: NodeId) -> Option<SignalId> {
        (self.kind(id) == NodeKind::Var).then(|| SignalId(self.w1[id.0 as usize]))
    }

    #[must_use]
    pub fn enum_entries(&self, id: NodeId) -> Option<&[(StrId, StrId)]> {
        (self.kind(id) == NodeKind::EnumTable)
            .then(|| self.enum_tables[self.w0[id.0 as usize] as usize].as_slice())
    }

    #[must_use]
    pub fn attrs(&self, id: NodeId) -> &[(StrId, Value)] {
        let a = self.attr_node.partition_point(|&n| n < id.0);
        let b = self.attr_node.partition_point(|&n| n <= id.0);
        &self.attr_kv[a..b]
    }

    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        use std::mem::size_of;
        (self.kind.capacity()
            + [
                self.parent.capacity(),
                self.name.capacity(),
                self.w0.capacity(),
                self.w1.capacity(),
                self.attr_node.capacity(),
                self.child_start.capacity(),
                self.children.capacity(),
                self.roots.capacity(),
            ]
            .iter()
            .sum::<usize>()
                * 4
            + self.signals.capacity() * size_of::<SignalKind>()
            + self.signal_var.capacity() * size_of::<NodeId>()
            + self.attr_kv.capacity() * size_of::<(StrId, Value)>()
            + self.enum_tables.capacity() * size_of::<Vec<(StrId, StrId)>>()
            + self
                .enum_tables
                .iter()
                .map(|table| table.capacity() * size_of::<(StrId, StrId)>())
                .sum::<usize>()) as u64
    }

    #[must_use]
    pub fn attr_count(&self) -> usize {
        self.attr_kv.len()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    pub fn roots(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.roots.iter().map(|&i| NodeId(i))
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let i = id.0 as usize;
        let (a, b) = if i + 1 < self.child_start.len() {
            (
                self.child_start[i] as usize,
                self.child_start[i + 1] as usize,
            )
        } else {
            (0, 0)
        };
        self.children[a..b].iter().map(|&c| NodeId(c))
    }

    #[must_use]
    pub fn signal_kind(&self, s: SignalId) -> Option<SignalKind> {
        self.signals.get(s.0 as usize).copied()
    }

    #[must_use]
    pub fn signal_var_type(&self, s: SignalId) -> Option<VarType> {
        let node = self.signal_var.get(s.0 as usize)?;
        Some(VarType::from_code(self.w0[node.0 as usize] as u16))
    }

    pub fn ids(&self) -> impl Iterator<Item = NodeId> + use<> {
        (0..self.len() as u32).map(NodeId)
    }

    pub fn nodes_of_kind(&self, kind: NodeKind) -> impl Iterator<Item = NodeId> + '_ {
        self.kind
            .iter()
            .enumerate()
            .filter(move |&(_, &k)| k == kind as u8)
            .map(|(i, _)| NodeId(i as u32))
    }
}
