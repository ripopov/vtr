//! # VTR: Volna Trace Record
//!
//! An open trace store for hardware simulation: signal waveforms, transaction
//! streams, the elaborated design hierarchy and relations between
//! transactions in one random-access file.
//!
//! * [`Writer`] — streaming writer usable from a running simulator
//!   ([`writer`] has the ordering rules and value semantics).
//! * [`Reader`] — random-access, memory-mapped reader, `Send + Sync`
//!   ([`reader`] has the query guide and cost model).
//!
//! The data model:
//! * **Hierarchy** ([`hierarchy`]): a forest of scopes, variables, streams,
//!   generators and enum tables with typed [`Value`] attributes. A variable
//!   names a signal ([`SignalId`]); aliases share one. [`census`] counts
//!   the distinct signals below each scope.
//! * **Waveforms** ([`signal`]): value changes of 2/4/9-state bit vectors,
//!   reals and variable-length byte strings.
//! * **Transactions** ([`txblock`]): intervals of a generator with
//!   attributes, events, pipeline stages, a parent and a status, plus typed
//!   relations between them. Log records ([`logblock`], `docs/LOGGING.md`)
//!   and clock stretches ([`clock`]) are transactions too.
//!
//! Strings are interned ([`StrId`]); every fallible call returns
//! [`Result`]. There is no global state: all settings are in
//! [`WriterOptions`] and [`ReadOptions`]. `block`, `container`, `varint` and
//! `xform` are format internals, public for inspection tools. The file format
//! is `docs/SPEC.md`; the C API (`core/vtr-capi/include/vtr.h`) is a
//! one-to-one projection of this crate.
//!
//! # Writing and reading a waveform
//!
//! ```
//! use vtr::{Direction, Reader, ScopeType, SignalKind, VarType, Writer};
//! # fn main() -> vtr::Result<()> {
//! # let dir = std::env::temp_dir().join(format!("vtr-doc-wave-{}", std::process::id()));
//! # std::fs::create_dir_all(&dir)?;
//! # let path = dir.join("wave.vtr");
//! let mut w = Writer::create(&path)?;
//! w.set_timescale(-9)?; // metadata first: 1 unit = 1 ns
//! let top = Some(w.add_scope(None, "top", ScopeType::Module, "counter")?);
//! let (_, clk) = w.add_var(top, "clk", VarType::Wire, Direction::Input, SignalKind::Bits { width: 1, states: 4 })?;
//! let (_, cnt) = w.add_var(top, "cnt", VarType::Reg, Direction::Output, SignalKind::Bits { width: 8, states: 4 })?;
//! for cycle in 0..100u64 {
//!     w.set_time(cycle * 10)?; // then the values of this time step
//!     w.emit_bit(clk, (cycle & 1) as u8)?;
//!     w.emit_u64(cnt, cycle / 2)?; // unchanged values are dropped
//! }
//! w.emit_logic_str(cnt, b"xxxxxxxx")?;
//! w.close()?; // check errors; Drop would discard them
//!
//! let rd = Reader::open(&path)?;
//! let cnt = rd.find_signal("top.cnt", '.').unwrap();
//! assert_eq!(rd.value_at(cnt, 555)?.to_ascii(), "00011011");
//! assert_eq!(rd.changes(cnt, 0, 30)?.len(), 2); // at 0 and 20
//! let data = rd.load_signal(cnt)?; // whole history, owned and shareable
//! assert_eq!(data.value_at(990).to_ascii(), "xxxxxxxx");
//! for child in rd.hierarchy().children(rd.hierarchy().roots().next().unwrap()) {
//!     println!("{}", rd.full_path(child, "."));
//! }
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Transactions
//!
//! ```
//! use vtr::{Reader, ScopeType, TxQuery, TxStatus, Value, Writer};
//! # fn main() -> vtr::Result<()> {
//! # let dir = std::env::temp_dir().join(format!("vtr-doc-tx-{}", std::process::id()));
//! # std::fs::create_dir_all(&dir)?;
//! # let path = dir.join("tx.vtr");
//! let mut w = Writer::create(&path)?;
//! let core = w.add_scope(None, "cpu", ScopeType::Core, "")?;
//! let pipe = w.add_stream(Some(core), "pipe", "PIPELINE")?;
//! let insn = w.add_generator(pipe, "instruction")?;
//! // Intern keys and names once; transaction methods take StrId.
//! let (k_pc, k_dep, lane, st_f, st_x) = (w.intern("pc"), w.intern("depends_on"), w.intern("0"), w.intern("F"), w.intern("X"));
//! let mut prev = None;
//! for i in 0..100u64 {
//!     let tx = w.begin_tx(insn, i)?;
//!     w.tx_attr(tx, k_pc, &Value::U64(0x1000 + i * 4))?;
//!     w.tx_stage_begin(tx, st_f, lane, i)?;
//!     w.tx_stage_begin(tx, st_x, lane, i + 1)?; // closes F
//!     if let Some(p) = prev { w.relate(k_dep, p, tx, &[])?; }
//!     w.end_tx(tx, i + 3, TxStatus::Ok)?; // closes X
//!     prev = Some(tx);
//! }
//! w.close()?;
//!
//! let rd = Reader::open(&path)?;
//! assert_eq!(rd.tx_counts(), (100, 99));
//! let q = TxQuery { window: Some((10, 12)), ..Default::default() };
//! rd.visit_transactions(&q, |tx| {
//!     for s in &tx.stages { println!("{} {}: {}..{:?}", tx.id, rd.str(s.name), s.begin, s.end); }
//!     true // keep going
//! })?;
//! assert_eq!(rd.relations_to(5)?[0].from, 4);
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok(())
//! # }
//! ```

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

pub use census::{Census, ScopeSizes};
pub use clock::{ClockId, ClockInfo, ClockTimeline, CycleAt, Stretch};
pub use clock::STREAM_KIND as CLOCK_STREAM_KIND;
pub use codec::{Codec, Compression};
pub use ending::Ending;
pub use error::{Error, Result};
pub use hierarchy::{Direction, Hierarchy, Node, NodeData, NodeId, NodeKind, ScopeType, SignalId, SignalKind, VarType};
pub use logblock::STREAM_KIND as LOG_STREAM_KIND;
pub use logblock::{LogArg, LogArgType, LogRecord, LogSite, LogSiteId, LogSiteSpec, Severity};
pub use reader::{LogQuery, ReadOptions, Reader, SignalData, TxQuery};
pub use sections::{Blackout, FileType, Meta};
pub use signal::{OwnedSignalValue, SignalValue};
pub use strings::StrId;
pub use txblock::{Relation, Transaction, TxAttr, TxEvent, TxId, TxKind, TxStage, TxStatus};
pub use value::Value;
pub use writer::{recover, CrashState, Sealer, Writer, WriterOptions, WriterStats};
