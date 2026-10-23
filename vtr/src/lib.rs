//! VTR: typed simulation records, waveforms, hierarchy, and relations.

pub mod activity;
pub mod arguments;
pub mod attributes;
pub mod block;
pub mod census;
pub mod codec;
pub mod declarations;
pub mod error;
pub mod framing;
pub mod hierarchy;
pub mod recording;
pub mod records;
pub mod schema;
pub mod signal;
pub mod trace;
pub mod transactions;
pub mod varint;
mod wire;
pub mod xform;

pub use attributes::{Attribute, Attributes, Value};
pub use census::{Census, Contributions, ScopeSizes};
pub use codec::{Codec, Compression};
pub use declarations::{Declarations, Metadata, Node, NodeData};
pub use error::{Error, Result};
pub use framing::{Completion, Fingerprint, RecordingId, recover};
pub use hierarchy::{Direction, NodeId, ScopeType, SignalId, SignalKind, VarType};
pub use recording::{
    ActiveRecord, BufferOptions, ClockSource, CrashState, EncodingOptions, Ending, ExecutionMode,
    Keyspace, OwnerRef, Producer, ProducerId, QueueStats, Recorder, Sealer, SignalUpdate, Source,
    SourceOptions, Stage, Status, Tracker, TrackerDiagnostic, Transaction, Writer, WriterOptions,
    WriterStats, ending,
};
pub use records::{GeneratorId, RecordId, RecordKind, RecordQuery, SpanKind};
pub use schema::{Payload, PayloadReader, TypeId, Types};
pub use signal::{Logic, LogicStates, OwnedSignalValue, SignalValue};
pub use trace::{
    Blackout, FormatLocale, PayloadView, ReadOptions, Reader, Record, Severity, TimeWindow,
};
pub use transactions::{Relation, TransactionQuery};
