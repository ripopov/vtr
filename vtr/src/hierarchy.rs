//! Design hierarchy: scopes, variables, generators, enum tables.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignalId(pub u32);

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
    #[inline]
    pub fn packed_len(self) -> Option<usize> {
        match self {
            SignalKind::Bits { width, states } => Some(crate::signal::packed_len(width, states)),
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
}
