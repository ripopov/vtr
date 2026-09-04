//! # VTR: Vibe Trace Record
//!
//! An open trace store for hardware simulation: signal waveforms, transaction
//! streams, the elaborated design hierarchy and relations between
//! transactions in one random-access file.
//!
//! * [`Writer`] — streaming writer usable from a running simulator.
//! * [`Reader`] — random-access, memory-mapped reader.
//!
//! See `docs/SPEC.md` for the file format and `docs/API_RUST.md` for a tour.

pub mod block;
pub mod codec;
pub mod container;
pub mod error;
pub mod hierarchy;
pub mod reader;
pub mod sections;
pub mod signal;
pub mod strings;
pub mod txblock;
pub mod value;
pub mod varint;
pub mod writer;

pub use codec::{Codec, Compression};
pub use error::{Error, Result};
pub use hierarchy::{Direction, Hierarchy, Node, NodeData, NodeId, NodeKind, ScopeType, SignalId, SignalKind, VarType};
pub use reader::{ReadOptions, Reader, SignalData, TxQuery};
pub use sections::{Blackout, FileType, Meta};
pub use signal::{OwnedSignalValue, SignalValue};
pub use strings::StrId;
pub use txblock::{AttrPhase, Relation, Transaction, TxAttr, TxEvent, TxId, TxKind, TxStage, TxStatus};
pub use value::Value;
pub use writer::{Writer, WriterOptions, WriterStats};
