//! VTR: random-access storage for simulation waveforms, transactions, and hierarchy.

pub mod activity;
pub mod block;
pub mod census;
pub mod clock;
pub mod codec;
pub mod container;
pub mod ending;
pub mod error;
pub mod hierarchy;
pub mod hierarchy_index;
pub mod logblock;
pub mod logfmt;
pub mod reader;
pub mod sections;
pub mod signal;
pub mod strings;
pub mod txblock;
pub mod value;
pub mod varint;
pub mod writer;
pub mod xform;

pub use census::{Census, Contributions, ScopeSizes};
pub use clock::GENERATOR_KIND as CLOCK_GENERATOR_KIND;
pub use clock::{ClockId, ClockInfo, ClockTimeline, CycleAt, Stretch};
pub use codec::{Codec, Compression};
pub use ending::Ending;
pub use error::{Error, Result};
pub use hierarchy::{
    Direction, Hierarchy, Node, NodeData, NodeId, NodeKind, ScopeType, SignalId, SignalKind,
    VarType,
};
pub use logblock::GENERATOR_KIND as LOG_GENERATOR_KIND;
pub use logblock::{LogArg, LogArgType, LogRecord, LogSite, LogSiteId, LogSiteSpec, Severity};
pub use reader::{LogQuery, ReadOptions, Reader, SignalData, TxQuery};
pub use sections::{Blackout, FileType, Meta, Timescale};
pub use signal::{Logic, LogicStates, OwnedSignalValue, SignalValue};
pub use strings::StrId;
pub use txblock::{Relation, Transaction, TxAttr, TxEvent, TxId, TxKind, TxStage, TxStatus};
pub use value::Value;
pub use writer::{CrashState, Sealer, Writer, WriterBuilder, WriterOptions, WriterStats, recover};
