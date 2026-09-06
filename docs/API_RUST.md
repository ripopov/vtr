# VTR Rust API reference (`vtr` crate)

This document is a reference and tour of the `vtr` crate at `crates/vtr`
(workspace version 0.1.0, edition 2021, MSRV 1.80). It describes the public
API as implemented in the sources; where behaviour is subtle the exact rule
is spelled out. The on-disk format is specified in
[`docs/SPEC.md`](SPEC.md); the C binding is documented in
[`docs/API_C.md`](API_C.md).

Contents

1. [Overview and design principles](#1-overview-and-design-principles)
2. [Quick start](#2-quick-start)
3. [Writer reference](#3-writer-reference)
4. [Reader reference](#4-reader-reference)
5. [Value model](#5-value-model)
6. [Threading and performance](#6-threading-and-performance)
7. [Cross-references](#7-cross-references)

---

## 1. Overview and design principles

VTR (Vibe Trace Record) stores four things in one random-access file:

* **Waveforms**: value changes of signals (bit vectors with 2, 4 or 9 states
  per bit, IEEE doubles, variable-length byte strings).
* **Transactions**: timed intervals with typed attributes, point events,
  pipeline stages, a parent link and a status (FTR/SCV, OpenTelemetry spans and
  Konata pipeline logs all map onto them).
* **Hierarchy**: a forest of scopes, variables, transaction streams,
  generators and enumeration tables, each with typed attributes.
* **Relations**: typed, attributed edges between transactions.

The crate exposes two entry points, `Writer` and `Reader`, plus the value and
hierarchy types they share.

### 1.1 Crate layout

`vtr::*` re-exports everything an application normally needs:

| Re-export | Defined in | Purpose |
|---|---|---|
| `Writer`, `WriterOptions`, `WriterStats` | `writer` | Streaming writer |
| `Reader`, `ReadOptions`, `SignalData`, `TxQuery`, `LogQuery` | `reader` | Random-access reader |
| `LogSiteSpec`, `LogSiteId`, `LogArgType`, `LogArg`, `Severity`, `LogSite`, `LogRecord` | `logblock` | Log sites and records (`docs/LOGGING.md`) |
| `Hierarchy`, `Node`, `NodeData`, `NodeId`, `NodeKind`, `ScopeType`, `VarType`, `Direction`, `SignalId`, `SignalKind` | `hierarchy` | Design hierarchy and signal typing |
| `Value` | `value` | Typed attribute values |
| `SignalValue`, `OwnedSignalValue` | `signal` | Signal values (borrowed / owned) |
| `Transaction`, `TxAttr`, `TxEvent`, `TxStage`, `Relation`, `TxId`, `TxStatus`, `TxKind`, `AttrPhase` | `txblock` | Transaction model |
| `Meta`, `FileType`, `Blackout` | `sections` | File metadata, dump on/off intervals |
| `Codec`, `Compression` | `codec` | Compression settings |
| `StrId` | `strings` | Interned string handle |
| `Error`, `Result` | `error` | Error type and alias |

The modules themselves are public (`vtr::signal::pack_ascii`,
`vtr::value::packed_len`, `vtr::strings::StringTable`,
`vtr::container::DirEntry`, `vtr::logfmt::format_message`, ...). `block`,
`container`, `txblock`, `logblock` (encoders) and `varint` are format internals; they are public so that tools can inspect
files, but a simulator or viewer never needs them.

### 1.2 Design principles

**Small surface.** One writer type, one reader type, a handful of plain data
types. There are no traits to implement, no builders, no callbacks except the
two visitor methods on the reader.

**Explicit ownership.**

* `Writer` owns the output file and, by default, a background encoding
  thread. Dropping the writer closes the file (errors are swallowed on drop;
  call `close()` to see them). `Writer` is `Send` but not `Sync`: drive it from
  one thread at a time.
* `Reader` memory-maps the file (or wraps an in-memory `Vec<u8>`) and is
  `Send + Sync`; one reader can serve many query threads. Its internal caches
  are behind a `Mutex`/`OnceLock`.
* Values borrowed from a reader carry lifetimes: `SignalValue<'a>` borrows
  from a `SignalData` or from a decode buffer inside a callback. Point queries
  (`value_at`, `changes`) return `OwnedSignalValue` so the caller need not keep
  the reader's internal buffer alive. `Transaction`, `Relation` and `Node`
  are owned, `Clone` data.

**No global state.** No statics, no registries, no environment variables.
Every option is a field of `WriterOptions` / `ReadOptions`.

**Errors via `vtr::Error` / `vtr::Result`.** Every fallible operation returns
`Result<T, vtr::Error>`. Methods that cannot fail (hierarchy declarations,
`intern`, `dump_off`) do not return `Result`. The variants:

| Variant | Raised when |
|---|---|
| `Error::Corrupt(&'static str)` | Reader: the bytes are not a VTR file (bad magic), or a structural invariant is violated: truncated header/section/directory, unknown section kind that is not marked optional, unknown codec/node kind/value tag/signal kind, string or hierarchy chunk out of order, signal declared out of order, alias of unknown signal, parent out of range, LZ4/zstd length mismatch, column/run/blob bounds exceeded, `for_each_change` on a column run larger than 1 GiB. |
| `Error::UnsupportedVersion { major, minor, supported }` | Reader: the file's major version is greater than the library's (`container::VERSION_MAJOR` = 1). Minor versions are always accepted. |
| `Error::Invalid(String)` | A caller argument is wrong. Writer: unknown `SignalId` (emit/alias), `set_time` going backwards, a logic value not representable in the signal's states, an emit method called on the wrong `SignalKind`, unparsable real text, packed value too short or with the wrong `states`, unknown generator node id, `TxId` that is not open (`set_tx_parent`, `set_tx_kind`, `tx_attr`, `tx_event`, `tx_stage*`, `end_tx`). Reader: unknown `SignalId` in `signal_kind`, `value_at`, `changes`, `load_signal(s)`, `packed_len` returns `None` instead. |
| `Error::State(&'static str)` | Writer operation not allowed now: metadata setter after the first flush, `end_scope` without an open scope, `node_attr` on a node already flushed, `tx_stage_attr` on a transaction with no stage, background thread failed / stopped / panicked, internal consistency failures in the encoder. |
| `Error::Io(std::io::Error)` | File create/open/mmap/write/flush failures, thread spawn failure. Converts with `?` from `std::io::Error`. |
| `Error::Checksum { offset }` | Reader with `ReadOptions::verify_crc = true`: a section's CRC32 does not match. Sections written with `checksums: false` carry CRC 0 and are never checked. |
| `Error::Codec(String)` | zstd/LZ4 context creation, compression or decompression failure. |

`Error` implements `std::error::Error` and `Display` (via `thiserror`), so it
composes with `Box<dyn Error>` and `anyhow`.

**Streaming writer, random-access reader.** The writer never seeks: it
appends sections and writes a directory and trailer at close. Memory use on
the writer side is bounded by `block_records`, `tx_block_bytes` and the size
of the pending hierarchy chunk. The reader never reads sequentially: it
memory-maps the file, reads the trailer to find the directory, and then
touches only the sections a query needs.

**No need to read the whole file.** `Reader::open` reads:

* the 32-byte file header and 24-byte trailer, and the directory (40 bytes per
  section);
* every `Strings` section (decompressed into one table);
* the `Meta` section;
* every `Hierarchy` section (decompressed; the children index is built);
* the fixed header of every signal block (40 bytes) and transaction block
  (88 bytes);
* the `Blackout` section.

Value-change data and transaction rows are not touched at open. A query then
touches, at most, the block time tables it needs, the dirty index of the blocks
it visits, and the decompressed group pieces holding the requested signals
(bounded by `run_bytes` each). See section 6 for the per-query cost model.
The one exception is `ReadOptions::verify_crc`, which makes `open` hash every
section payload.

---

## 2. Quick start

Add the dependency (`vtr = { path = "crates/vtr" }` inside the workspace, or
by version once published). Both examples below were compiled and run against
the crate as-is.

### 2.1 Writing a waveform

```rust
use vtr::{Compression, Direction, ScopeType, SignalKind, Value, VarType, Writer, WriterOptions};

fn write_waveform(path: &str) -> vtr::Result<()> {
    let mut opts = WriterOptions::default();
    opts.compression = Compression::ZSTD_FAST; // level 1 zstd; default is ZSTD_DEFAULT (level 3)
    let mut w = Writer::create_with(path, opts)?;

    // Metadata: must be set before the first flush (i.e. before the first block).
    w.set_timescale(-9)?; // 1 time unit = 1 ns
    w.set_writer_name("my-sim 1.0")?;
    w.set_date("2026-09-04")?;
    let tool = w.intern("my-sim");
    w.set_file_attr("tool", Value::Str(tool))?;

    // Hierarchy: scopes nest with begin_scope/end_scope; vars declare signals.
    w.begin_scope("top", ScopeType::Module, "counter");
    let (_, clk)          = w.add_var("clk",  VarType::Wire,   Direction::Input,    SignalKind::Bits { width: 1,   states: 4 });
    let (_, rst)          = w.add_var("rst",  VarType::Wire,   Direction::Input,    SignalKind::Bits { width: 1,   states: 2 });
    let (cnt_node, cnt)   = w.add_var("cnt",  VarType::Reg,    Direction::Output,   SignalKind::Bits { width: 8,   states: 4 });
    let (_, data)         = w.add_var("data", VarType::Logic,  Direction::Implicit, SignalKind::Bits { width: 128, states: 2 });
    let (_, temp)         = w.add_var("temp", VarType::Real,   Direction::Implicit, SignalKind::Real);
    let (_, msg)          = w.add_var("msg",  VarType::String, Direction::Implicit, SignalKind::VarLen);
    w.node_attr(cnt_node, "reset_value", Value::U64(0))?;          // attribute on a pending node
    w.add_alias("cnt_alias", VarType::Wire, Direction::Implicit, cnt)?; // second name for the same signal
    w.add_enum_table("state_t", &[("IDLE", "00"), ("RUN", "01")]);
    w.end_scope()?;

    // Values: set_time first (monotonic), then emit. Unchanged values are dropped.
    for cycle in 0..1000u64 {
        w.set_time(cycle * 10)?;
        w.emit_bit(clk, (cycle & 1) as u8)?;             // logic code 0/1 (2=X, 3=Z, ...)
        if cycle == 0 { w.emit_bit(rst, 1)?; }
        if cycle == 2 { w.emit_bit(rst, 0)?; }
        if cycle & 1 == 1 {
            w.emit_u64(cnt, cycle / 2)?;                 // masked to 8 bits
            w.emit_words(data, &[cycle as u32, 0, 0, 0xdead_beef])?; // word 0 = bits 0..32
        }
        if cycle % 100 == 0 {
            w.emit_real(temp, 25.0 + cycle as f64 * 0.01)?;
            w.emit_varlen(msg, format!("cycle {cycle}").as_bytes())?;
        }
        if cycle == 500 { w.emit_logic_str(cnt, b"xxxxxxxx")?; } // VCD-style, MSB first
    }
    w.set_time(10_000)?; // extend the trace to its final time
    let stats = w.stats();
    w.close()?;          // joins the background thread; reports its errors
    println!("{} changes in {} blocks", stats.records, stats.blocks);
    Ok(())
}
```

### 2.2 Reading it back

```rust
use vtr::Reader;

fn read_waveform(path: &str) -> vtr::Result<()> {
    let rd = Reader::open(path)?; // mmap; reads directory, strings, meta, hierarchy, block headers
    println!("timescale 1e{} s, written by {:?}", rd.meta().timescale, rd.meta().writer);
    let (_t0, t1) = rd.time_range().unwrap_or((0, 0));

    // Point query: last change at or before t (initial value before the first change).
    let cnt = rd.find_signal("top.cnt", '.').expect("signal exists");
    let v = rd.value_at(cnt, 555)?;
    println!("cnt @555 = {}", v.to_ascii()); // "00011011"

    // Window query: all changes with time in [t0, t1].
    for (t, v) in rd.changes(cnt, 0, 100)? {
        println!("{t}: {}", v.to_ascii());
    }

    // Bulk load: every change of several signals, decompressing each run once.
    let clk = rd.find_signal("top.clk", '.').unwrap();
    let loaded = rd.load_signals(&[clk, cnt])?;
    let cnt_data = &loaded[1];
    println!("cnt: {} changes, initial {}", cnt_data.len(), cnt_data.initial().to_ascii());
    if let Some(i) = cnt_data.index_at(t1) {
        println!("last change at {} = {:?}", cnt_data.times()[i], cnt_data.get(i).as_u64());
    }

    // Hierarchy walk.
    let h = rd.hierarchy();
    for root in h.roots() {
        println!("{}", rd.full_path(root, "."));
        for child in h.children(root) {
            println!("  {} ({:?})", rd.name(child), h.node(child).kind());
        }
    }

    // VCD-style dump of everything in time order.
    let mut n = 0u64;
    rd.for_each_change(0, u64::MAX, |_t, _sig, _v| n += 1)?;
    println!("{n} changes total");
    Ok(())
}
```

### 2.3 Transactions

```rust
use vtr::{AttrPhase, Reader, ScopeType, TxQuery, TxStatus, Value, Writer};

fn write_tx(path: &str) -> vtr::Result<()> {
    let mut w = Writer::create(path)?;
    w.set_timescale(0)?; // cycles
    let core = w.begin_scope("cpu", ScopeType::Core, "");
    let pipe = w.add_stream(Some(core), "pipe", "PIPELINE");
    let insn = w.add_generator(pipe, "instruction");
    w.end_scope()?;
    // Pre-intern keys and names once; tx_* methods take StrId.
    let k_pc = w.intern("pc");
    let k_dep = w.intern("depends_on");
    let lane0 = w.intern("0");
    let st_f = w.intern("F");
    let st_x = w.intern("X");
    let ev_retire = w.intern("retire");
    let mut prev = None;
    for i in 0..100u64 {
        let tx = w.begin_tx(insn, i)?;
        w.tx_attr(tx, k_pc, AttrPhase::Begin, &Value::U64(0x1000 + i * 4))?;
        w.tx_stage_begin(tx, st_f, lane0, i)?;      // stage F on lane 0
        w.tx_stage_begin(tx, st_x, lane0, i + 1)?;  // closes F at i+1, opens X
        w.tx_event(tx, i + 2, ev_retire, &[])?;
        if let Some(p) = prev { w.relate(k_dep, p, tx, &[])?; }
        w.end_tx(tx, i + 3, TxStatus::Unset)?;      // closes X at i+3
        prev = Some(tx);
    }
    w.set_time(103)?;
    w.close()
}

fn read_tx(path: &str) -> vtr::Result<()> {
    let rd = Reader::open(path)?;
    let (n_tx, n_rel) = rd.tx_counts();
    println!("{n_tx} transactions, {n_rel} relations");
    let q = TxQuery { window: Some((10, 12)), ..Default::default() };
    rd.visit_transactions(&q, |tx| {
        println!("tx {} [{}, {}] gen={} status={}", tx.id, tx.begin, tx.end, rd.name(tx.generator), tx.status.name());
        for a in &tx.attrs { println!("  {} = {:?} ({:?})", rd.str(a.key), a.value, a.phase); }
        for s in &tx.stages { println!("  stage {} lane {} {}..{:?}", rd.str(s.name), rd.str(s.lane), s.begin, s.end); }
        true // keep going
    })?;
    if let Some(t) = rd.transaction(5)? {
        for r in rd.relations_to(t.id)? { println!("{} -> {} ({})", r.from, r.to, rd.str(r.kind)); }
    }
    Ok(())
}
```

---

## 3. Writer reference

```rust
pub struct Writer { /* private */ }
impl Drop for Writer { /* calls close(), ignores errors */ }
```

`Writer` is `Send` and not `Sync`. All methods take `&mut self` except the
inspectors (`options`, `string`, `current_scope`, `signal_count`,
`signal_kind`, `current_time`, `open_tx_count`, `stats`).

### 3.1 `WriterOptions`

```rust
#[derive(Clone, Debug)]
pub struct WriterOptions {
    pub compression: Compression,
    pub group_size: u32,
    pub block_records: usize,
    pub chunk_records: usize,
    pub run_bytes: usize,
    pub tx_block_bytes: usize,
    pub background: bool,
    pub log_encoders: usize,
    pub dedup: bool,
    pub checksums: bool,
}
```

| Field | Default | Effect |
|---|---|---|
| `compression` | `Compression::ZSTD_DEFAULT` (zstd level 3) | Codec and level used for every compressed payload: string and hierarchy chunks, block time tables, group frames, column runs, transaction column blobs. Any payload that does not shrink is stored raw (`Codec::None`) automatically. Note that `Compression::default()` is `ZSTD_FAST`, but `WriterOptions::default()` picks `ZSTD_DEFAULT`. |
| `group_size` | 256 | Signals per value-change group. **Rounded up to a power of two** (minimum 1) by `create_with`; the rounded value is stored in `Meta::group_size` and reflected by `options()`. Larger groups compress better; smaller groups make single-signal reads decompress less. Fixed for the file's lifetime. |
| `block_records` | `1 << 24` (16 Mi) | Value changes per signal block, the compression unit: a block is finished (columns concatenated, compressed, written) once this many changes were logged since the previous block. Larger blocks give the compressor longer columns; smaller blocks make random access touch less data. A block cannot hold more than `2^31` time steps. |
| `chunk_records` | `1 << 19` (512 Ki) | Lower bound on the value changes handed to the background encoder at a time, the pipelining unit; the writer raises it to `CHUNK_RECORDS_PER_SIGNAL` (16) changes per declared signal so that per-signal column fragments stay large in designs with hundreds of thousands of signals. Each chunk is counting-sorted by signal and pre-encoded into column fragments as soon as it arrives, so the work overlaps with the simulator; only the final concatenation and compression wait for the block to complete. Wide vectors (declared packing wider than 8 bytes) bypass the record log and travel as pre-encoded fragments with the same chunks. Bounded by `block_records`. |
| `run_bytes` | `64 << 10` (64 KiB) | Raw bytes per independently compressed column run inside a group. Consecutive signal columns are packed into a run while they fit; a single column larger than the budget forms a run of its own. Bounds how much a reader must decompress to reach one signal in one block. |
| `tx_block_bytes` | `4 << 20` (4 MiB) | Size of the buffered transaction and relation rows (in the writer's row encoding) that triggers a transaction block flush. Checked on every `end_tx` and `relate`. Log rows use the same budget for log blocks (checked on every `log`). |
| `background` | `true` | Encode and compress on a background thread named `vtr-writer`. The caller thread only buffers records; a bounded channel (4 messages) applies back-pressure, and chunk/block buffers are recycled to avoid reallocation. With `false`, chunk encoding and block finishing happen inline in the calling thread at the same points. |
| `log_encoders` | 2 | Helper threads (`vtr-logenc`) that split, dictionary-code and compress log blocks, fed by the background thread and started on the first log block; 0 encodes log blocks on the background thread itself. Blocks are written in production order whatever the count. Ignored when `background` is `false`. |
| `dedup` | `true` | Drop value changes whose value equals the signal's last value (see 3.7). |
| `checksums` | `true` | Store a CRC32 (`crc32fast`) of every section payload in the section header. Verified by the reader only when `ReadOptions::verify_crc` is set. |

### 3.2 Construction

```rust
pub fn create(path: impl AsRef<Path>) -> Result<Writer>
pub fn create_with(path: impl AsRef<Path>, opts: WriterOptions) -> Result<Writer>
pub fn options(&self) -> &WriterOptions
```

`create_with` creates (truncates) the file, writes the 32-byte file header,
spawns the background thread if requested, rounds `group_size`, and
initialises `Meta` with `timescale = -9`, `file_type = Verilog`,
`writer = "vtr <crate version>"` (for example `"vtr 0.1.0"`). Errors:
`Error::Io`.

### 3.3 Metadata

```rust
pub fn set_timescale(&mut self, exp: i8) -> Result<()>       // 10^exp seconds per time unit; default -9
pub fn set_time_zero(&mut self, t: i64) -> Result<()>        // FST "timezero" offset; default 0
pub fn set_file_type(&mut self, ft: FileType) -> Result<()>  // default FileType::Verilog
pub fn set_writer_name(&mut self, s: &str) -> Result<()>     // default "vtr <version>"
pub fn set_date(&mut self, s: &str) -> Result<()>            // free form; default ""
pub fn set_comment(&mut self, s: &str) -> Result<()>         // default ""
pub fn set_file_attr(&mut self, key: &str, value: Value) -> Result<()> // appends to Meta::attrs
```

**Before-first-flush rule.** The `Meta` section is written once, at the first
flush (explicit `flush()`, the first automatic block flush, or `close()`).
After that every setter fails with
`Error::State("metadata must be set before the first flush")`. In practice:
set metadata right after `create`. `set_file_attr` interns `key` before
checking the state, so the string is interned even when the call fails.

`FileType` values: `Verilog` (0, default), `Vhdl` (1), `VerilogVhdl` (2),
`SystemC` (3), `Architectural` (4, pipeline/architectural simulators),
`Software` (5, OpenTelemetry etc.), `Other` (255). `FileType::from_u8` maps
unknown codes to `Other`.

### 3.4 Strings

```rust
pub fn intern(&mut self, s: &str) -> StrId
pub fn string(&self, id: StrId) -> &str
```

`intern` returns a dense `StrId` (id 0 is always the empty string;
`StrId::EMPTY`). Interning an already-known string is a hash lookup and
returns the same id. All names passed as `&str` to hierarchy methods are
interned internally; transaction methods take `StrId` so hot paths do no
hashing. New strings are written as an append-only chunk at the next flush.
`string(id)` panics for an id that was never returned by `intern`.

### 3.5 Hierarchy

Nodes are numbered densely in declaration order (`NodeId(0)` is the first
node declared). Declarations are buffered as *pending nodes* and written as a
hierarchy chunk at the next flush; the writer flushes strings, meta and
pending nodes before every signal or transaction block, so a node is always in
the file before any data that refers to it.

```rust
pub fn begin_scope(&mut self, name: &str, scope_type: ScopeType, component: &str) -> NodeId
pub fn end_scope(&mut self) -> Result<()>
pub fn current_scope(&self) -> Option<NodeId>
```

`begin_scope` pushes a scope whose parent is the current scope (or none at
top level). `component` is the module/entity type name (FST "component"); pass
`""` when unknown. `end_scope` fails with
`Error::State("end_scope without begin_scope")` when no scope is open.
Scopes need not be closed before `close()`. The scope stack only affects the
parent chosen by `add_var`, `add_alias`, `add_enum_table` and nested
`begin_scope`.

```rust
pub fn add_var(&mut self, name: &str, var_type: VarType, direction: Direction, kind: SignalKind) -> (NodeId, SignalId)
pub fn add_var_in(&mut self, parent: Option<NodeId>, name: &str, var_type: VarType, direction: Direction, kind: SignalKind) -> (NodeId, SignalId)
pub fn add_bits(&mut self, name: &str, width: u32, states: u8) -> (NodeId, SignalId)
```

`add_var` declares a variable in the current scope **and** a new signal of
`kind`; `add_var_in` takes the parent explicitly (`None` = top level).
`SignalId`s are dense and increasing in declaration order. `add_bits` is
`add_var(name, VarType::Wire, Direction::Implicit, SignalKind::Bits { width, states })`.
`states` must be 2, 4 or 9 (any other value is encoded as 9). Signals may be
declared at any time, including after value changes have been emitted; a
signal's group is `id / group_size`.

```rust
pub fn add_alias(&mut self, name: &str, var_type: VarType, direction: Direction, signal: SignalId) -> Result<NodeId>
pub fn add_alias_in(&mut self, parent: Option<NodeId>, name: &str, var_type: VarType, direction: Direction, signal: SignalId) -> Result<NodeId>
```

Declare a second variable that refers to an existing signal (the VCD/FST
"same id code" case). Fails with `Error::Invalid` for an unknown `SignalId`.
On the reader side the alias is a `NodeData::Var` with `declares: None`.

```rust
pub fn add_enum_table(&mut self, name: &str, entries: &[(&str, &str)]) -> NodeId
```

Declares an enumeration table under the current scope. Each entry is
`(literal, value)` where `value` is the bit-string spelling (FST enum table
convention). Associate it with a variable by convention through a node
attribute; the library does not link them.

```rust
pub fn add_stream(&mut self, parent: Option<NodeId>, name: &str, kind: &str) -> NodeId
pub fn add_generator(&mut self, stream: NodeId, name: &str) -> NodeId
```

Transaction streams (FTR `tx_stream`) live under a scope or at top level;
`kind` is a free-form string such as `"TRANSACTOR"` or `"PIPELINE"`.
Generators (FTR `tx_generator`, "transaction type") live under a stream and
are the node that `begin_tx` takes.

```rust
pub fn node_attr(&mut self, node: NodeId, key: &str, value: Value) -> Result<()>
```

Appends a `(key, value)` attribute to a node. **Pending-node rule:** the node
must not have been flushed yet, i.e. it was created after the last flush
(explicit or automatic) and before this call; otherwise
`Error::State("node attributes must be added before the node is flushed")`.
Add attributes immediately after creating the node.

```rust
pub fn signal_count(&self) -> u32
pub fn signal_kind(&self, s: SignalId) -> Option<SignalKind>
```

### 3.6 Time

```rust
pub fn set_time(&mut self, t: u64) -> Result<()>
pub fn current_time(&self) -> u64
pub fn dump_off(&mut self)
pub fn dump_on(&mut self)
pub fn blackout_at(&mut self, time: u64, active: bool)
```

**Monotonic rule.** Once any time step has been recorded, `t` must be
`>= current_time()`; otherwise `Error::Invalid("time T is earlier than current
time C")`. Calling `set_time` with the current time again is a no-op (same
time step). The very first `set_time` may be any value, including one below
the initial `current_time()` of 0.

Every distinct `set_time` value becomes an entry in the current block's time
table; value changes are stored as indices into that table. Changes emitted
**before the first `set_time`** are attributed to the first time step later
established (the first `set_time` value, or time 0 if `set_time` is never
called). Call `set_time` before emitting.

After a block flush, the new block's time table starts with the current time,
so the same time step can be the last entry of one block and the first of the
next; `Reader::time_table` merges these.

`dump_off` / `dump_on` record a `Blackout { time: current_time, active }`
marker (VCD `$dumpoff` / `$dumpon`); `blackout_at` records one at an explicit
time (for converters replaying VCD). The markers are written as one section at
close and returned verbatim by `Reader::blackout()`; they are **not**
validated, sorted or merged, and **the writer keeps recording values during a
blackout**. The caller decides what to emit.

### 3.7 Value emission

All emit methods validate the `SignalId` (`Error::Invalid("unknown signal
N")`), apply deduplication, append a record stamped with the current time
step, and flush a block when `block_records` is reached. Logic codes are
`0`=0, `1`=1, `2`=X, `3`=Z, `4`=U, `5`=W, `6`=L, `7`=H, `8`=- (constants
`vtr::signal::{L0, L1, LX, LZ, LU, LW, LL, LH, LDASH}`).

A multi-state signal value that contains only 0/1 bits is stored in
**compact** form (2-state packing, one bit per bit) with a flag; the reader
reports it as `SignalValue::Bits { states: 2, .. }`. This is why `emit_u64`
and `emit_words` are the cheapest paths.

```rust
pub fn emit_bit(&mut self, sig: SignalId, code: u8) -> Result<()>
```

| Signal kind | Behaviour |
|---|---|
| `Bits { width: 1, states }` | `code` is clamped to `min(code, 8)`. If `states_for_code(code) > states` (X/Z on a 2-state, U/W/L/H/- on a 4-state signal) fails with `Error::Invalid`. Codes 0 and 1 are stored compact. |
| `Bits { width > 1, .. }` | Equivalent to `emit_u64(sig, (code & 1) as u64)`. |
| `Real`, `VarLen` | `Error::Invalid("emit_bit on a non-bit signal")`. |

```rust
pub fn emit_u64(&mut self, sig: SignalId, value: u64) -> Result<()>
```

| Signal kind | Behaviour |
|---|---|
| `Bits { width, .. }` with `width < 64` | `value` is masked to the low `width` bits (`value & ((1 << width) - 1)`); higher bits are silently dropped. |
| `Bits { width >= 64, .. }` | `value` is used whole and zero-extended to `width` bits. |
| any `Bits` | Always stored compact (2-state packing), whatever `states` is. |
| `Real` | Stored as `value as f64`. |
| `VarLen` | `Error::Invalid("emit_u64 on a variable-length signal")`. |

```rust
pub fn emit_words(&mut self, sig: SignalId, words: &[u32]) -> Result<()>
```

2-state value from little-endian 32-bit words: `words[0]` holds bits 0..32,
`words[1]` bits 32..64, and so on. Missing words are zero; extra words and
bits above `width` are ignored (bits above `width` in the last byte are
cleared). For `width <= 64` this is `emit_u64(words[0] | words[1] << 32)`
(masked). Non-`Bits` signals: `Error::Invalid("emit_words on a non-bit
signal")`. `vtr::signal::pack_words64` exists for 64-bit words but has no
`emit_*` wrapper; use `emit_packed(sig, 2, ..)` with it.

```rust
pub fn emit_logic_str(&mut self, sig: SignalId, s_ascii: &[u8]) -> Result<()>
```

VCD-style ASCII value, MSB first (leftmost character is the highest bit).

* Character set: `0 1 x X z Z u U w W l L h H -`. **Any other character maps
  to X** (`signal::code_from_ascii`).
* Width 1: only the **last** character is used; an empty string means X. A
  code not representable in the signal's states fails with `Error::Invalid`.
* Width > 1, string shorter than `width`: **left-extended** with X, Z, U, W,
  L, H or - when the first character is one of those (case-insensitive),
  otherwise with 0 (VCD extension rule: `"x"` on 4 bits is `xxxx`, `"1"` is
  `0001`). Empty string = all zeros.
* Width > 1, string longer than `width`: only the **last `width`** characters
  are used (high bits truncated).
* Rejection: any code above 1 on a `states: 2` signal fails with
  `Error::Invalid("x/z value on a 2-state signal")`; any of U/W/L/H/- on a
  `states: 4` signal fails with `Error::Invalid("9-state value on a 4-state
  signal")`. Because unknown characters map to X they are also rejected on
  2-state signals.
* A value consisting only of 0/1 is stored compact; otherwise in declared
  packing.
* `Real` signal: the bytes must be UTF-8 and parse (after trimming) as an
  `f64` (`Error::Invalid("real value is not UTF-8")` /
  `Error::Invalid("cannot parse real value")`).
* `VarLen` signal: identical to `emit_varlen(sig, s_ascii)`.

```rust
pub fn emit_packed(&mut self, sig: SignalId, states: u8, data: &[u8]) -> Result<()>
```

Value already packed in VTR layout (section 5.2) with `states` states per bit.
`states` must be 2 (compact value) or the signal's declared states, otherwise
`Error::Invalid("packed value states must be 2 or the declared states")`.
`data.len()` must be at least `packed_len(width, states)`
(`Error::Invalid("packed value too short")`); extra bytes are ignored. For a
width-1 signal only the low nibble of `data[0]` is used as the code. No check
is made that codes fit the declared states; a caller emitting 9-state packed
data on a 4-state signal gets garbage back. Non-`Bits`:
`Error::Invalid("emit_packed on a non-bit signal")`.

```rust
pub fn emit_real(&mut self, sig: SignalId, value: f64) -> Result<()>
pub fn emit_varlen(&mut self, sig: SignalId, bytes: &[u8]) -> Result<()>
```

`emit_real` accepts only `SignalKind::Real`
(`Error::Invalid("emit_real on a non-real signal")`); the value is stored as
its IEEE bits (NaN payloads and -0.0 survive). `emit_varlen` accepts only
`SignalKind::VarLen` (`Error::Invalid("emit_varlen on a fixed-width
signal")`); `bytes` may be any length including 0 and need not be UTF-8.

**Deduplication (`dedup: true`).** Each signal remembers its last value and
the form it was stored in (compact or declared packing). An emit whose value
*and* form equal the last one is dropped without producing a record. The
initial "last value" is the signal's default initial value in **declared**
packing and non-compact form:

* `Real`: 0.0 - a first `emit_real(sig, 0.0)` is dropped.
* `VarLen`: empty - a first `emit_varlen(sig, b"")` is dropped.
* `Bits` with `states` 4 or 9: all X in declared packing - a first
  `emit_logic_str(sig, b"xxxx")` (or an all-X `emit_packed` with the declared
  states) is dropped.
* `Bits` with `states: 2`: all zeros, but recorded as non-compact, while every
  emit path on a 2-state signal stores compact; a first zero is therefore
  **not** dropped.

Consequently the reader's `SignalData::initial()` / `value_at` before the first
change equals the value the writer would have deduplicated, so readers see a
consistent waveform either way. Two emits of *different* values at the same
time step are both recorded (in emission order). Set `dedup: false` to record
every emit verbatim (for example to preserve event-like pulses: with dedup on,
a 1-bit event signal receiving `1` twice without an intervening `0` records
only the first pulse).

Reals compare by bit pattern, so `-0.0` after `0.0` is recorded and NaN after
the same NaN is dropped.

### 3.8 Flushing

```rust
pub fn flush(&mut self) -> Result<()>
```

Writes pending strings, the `Meta` section (first time only), pending
hierarchy nodes, then the buffered value changes as one signal block (if any)
and the buffered transaction/relation rows as one transaction block (if any).
Forces a block boundary; frequent explicit flushes make many small blocks and
hurt compression and read speed. Flushing is otherwise automatic
(`block_records`, `tx_block_bytes`, `close`). After the first flush metadata
setters fail (3.3) and `node_attr` fails for nodes declared earlier (3.5).

With `background: true`, `flush` returns as soon as the block has been handed
to the encoder thread (it blocks only when the 3-message channel is full).
Errors from the background thread surface on the *next* call that talks to the
sink (`flush`, an automatic flush inside `emit_*`/`end_tx`/`relate`, or
`close`).

### 3.9 Transactions

Transaction ids (`TxId = u64`) are assigned by the writer, unique in the
file, dense and increasing from 1. A transaction is *open* between `begin_tx`
and `end_tx`; every mutating method takes the id of an open transaction and
fails with `Error::Invalid("transaction N is not open")` otherwise. Open
transactions are kept in a hash-addressed table that grows as needed; memory
per open transaction is proportional to its attributes, events and stages.

```rust
pub fn begin_tx(&mut self, gen: NodeId, time: u64) -> Result<TxId>
```

Starts a transaction of generator `gen` at `time`. Only `gen.0 < node count`
is checked (`Error::Invalid("unknown generator node N")`); passing a node
that is not a generator is not detected. `time` is independent of `set_time`
(transactions carry their own timestamps; the writer does not require them to
be monotonic).

```rust
pub fn set_tx_parent(&mut self, tx: TxId, parent: TxId) -> Result<()>
pub fn set_tx_kind(&mut self, tx: TxId, kind: TxKind) -> Result<()>
```

`parent` is a structural nesting link (OpenTelemetry parent span, FTR
nested transaction); it is not validated and may refer to a transaction that
has already ended. `TxKind`: `Unspecified` (0, default), `Internal` (1),
`Server` (2), `Client` (3), `Producer` (4), `Consumer` (5).

```rust
pub fn tx_attr(&mut self, tx: TxId, key: StrId, phase: AttrPhase, value: &Value) -> Result<()>
```

Appends an attribute. `AttrPhase` records *when* the attribute was captured
relative to the transaction's lifetime (FTR semantics): `Begin` (0),
`Record` (1, default), `End` (2). The phase is a tag only; attributes are
returned in call order regardless of phase. Keys are not deduplicated: calling
`tx_attr` twice with the same key stores two attributes.

```rust
pub fn tx_event(&mut self, tx: TxId, time: u64, name: StrId, attrs: &[(StrId, Value)]) -> Result<()>
```

Appends a timestamped point event (OpenTelemetry span event) with its own
attribute list. `time` is not validated against the transaction's interval.

```rust
pub fn tx_stage_begin(&mut self, tx: TxId, name: StrId, lane: StrId, time: u64) -> Result<()>
pub fn tx_stage_end(&mut self, tx: TxId, name: StrId, lane: StrId, time: u64) -> Result<bool>
pub fn tx_stage(&mut self, tx: TxId, name: StrId, lane: StrId, begin: u64, end: u64, attrs: &[(StrId, Value)]) -> Result<()>
pub fn tx_stage_attr(&mut self, tx: TxId, key: StrId, value: &Value) -> Result<()>
```

Stages are named sub-intervals on a *lane* (Konata pipeline stages; lanes are
arbitrary interned strings such as `"0"`).

* `tx_stage_begin` first closes the most recently opened still-open stage on
  the same `lane` at `max(time, that_stage.begin)`, then opens a new stage
  `[time, open)`.
* `tx_stage_end` closes the most recent still-open stage with the same
  `name` **and** `lane` at `max(time, stage.begin)`; returns `Ok(false)` (not
  an error) when no such stage is open.
* `tx_stage` records a complete stage with `end` clamped to `>= begin` and
  its attributes (all tagged `AttrPhase::Record`).
* `tx_stage_attr` appends an attribute to the **most recently begun** stage
  (open or not); `Error::State("transaction has no stage")` if there is none.

```rust
pub fn end_tx(&mut self, tx: TxId, time: u64, status: TxStatus) -> Result<()>
```

Ends the transaction. **Clamping:** the stored end is `max(time, begin)`;
every still-open stage is closed at `max(end, stage.begin)`. The transaction
row is appended to the pending transaction block and the id is removed from
the open table (it can no longer be modified; `relate` still accepts it).
`TxStatus`: `Unset` (0, ended normally without an explicit status),
`Ok` (1), `Error` (2), `Aborted` (3, squashed/flushed), `Open` (4, reserved
for transactions still open at close; passing it explicitly is allowed but
misleading).

```rust
pub fn relate(&mut self, kind: StrId, from: TxId, to: TxId, attrs: &[(StrId, Value)]) -> Result<()>
```

Records a directed relation of interned kind `kind` (for example
`"wakeup"`, `"parent"`, `"follows_from"`). `from`/`to` are **not** validated:
they may be open, ended, or (if the caller is careless) nonexistent. Relations
go into the same blocks as transactions.

```rust
pub fn open_tx_count(&self) -> usize
```

Number of transactions begun but not ended.

**At close**, every transaction still open is written with
`status = TxStatus::Open`, `end = max(current_time(), begin)`, and its open
stages closed at that end. They are appended in unspecified order after all
normally ended transactions. Transactions appear in the file in `end_tx` call
order (not id order), which is the order `visit_transactions` reports.

### 3.10 Close and drop

```rust
pub fn close(&mut self) -> Result<()>
```

Idempotent (a second call returns `Ok(())`). Performs, in order: write rows
for open transactions; flush strings/meta/hierarchy; flush the final signal
block if there are buffered records **or** if no block has been written yet
but at least one time step exists (so a trace with `set_time` calls and no
changes still gets one block and a `time_range`); flush the transaction block;
write the `Blackout` section if any marker was recorded; write the directory
and trailer; flush the `BufWriter`. With `background: true` it sends a close
message, joins the thread and returns the thread's result.

**Error reporting from the background thread.** The thread stores the first
error it hits in a shared slot and sets a failed flag; the thread exits. The
next `Writer` call that sends to the sink returns that error (or
`Error::State("background writer failed")` / `"background writer stopped"` if
the slot was already taken), and `close` returns it as well. A panic in the
thread is reported as `Error::State("background writer panicked")`. There is
no way to resume after a sink failure; the file is left without a directory
(the reader will still recover what was completely written, see 4.2).

**Drop.** `Drop` calls `close()` and discards the result. Always call
`close()` explicitly when you care about I/O errors. Dropping the `Writer`
after a successful `close()` does nothing.

```rust
pub fn stats(&self) -> WriterStats
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub blocks: u32,        // signal blocks handed to the encoder so far
    pub records: u64,       // value changes recorded (after dedup), including those still buffered
    pub transactions: u64,  // transactions ended with end_tx (open ones and those closed at close() are not counted)
    pub signals: u32,       // declared signals
    pub nodes: u32,         // declared hierarchy nodes
}
```

`stats()` is cheap and may be called at any time before or after `close`.

---

### 3.11 Logs

Log records are messages of *log sites* (see `docs/LOGGING.md` and
`SPEC.md` section 8): the format string, severity, source location and
argument types are declared once, each message stores the time and the
argument values.

```rust
pub fn add_log_stream(&mut self, parent: Option<NodeId>, name: &str) -> NodeId
pub fn add_log_site(&mut self, spec: &LogSiteSpec) -> LogSiteId
pub fn log_site_node(&self, site: LogSiteId) -> Option<NodeId>
pub fn log_site_count(&self) -> u32
```

`add_log_stream` is `add_stream(parent, name, "LOG")`. `add_log_site`
creates a generator of `spec.stream` named by the format string and carrying
`log.severity`, `log.args` (the types), `log.names` (the argument names, or
`"0"`, `"1"`, ... when `spec.names` is empty), and `log.file` / `log.line` /
`log.func` when given. It returns a dense `LogSiteId` (the handle `log`
takes); the generator node is `log_site_node`.

```rust
pub struct LogSiteSpec<'a> { pub stream: NodeId, pub severity: Severity, pub fmt: &'a str, pub args: &'a [LogArgType],
                             pub names: &'a [&'a str], pub file: &'a str, pub line: u32, pub func: &'a str }
impl LogSiteSpec<'_> { pub fn new(stream, severity, fmt, args) -> Self; pub fn names(self, &[&str]) -> Self;
                       pub fn location(self, file: &str, line: u32) -> Self; pub fn func(self, &str) -> Self }
pub enum Severity { Trace, Debug, Info, Warn, Error, Fatal, Other(u8) }   // ordered by code(); from_name("warning")
pub enum LogArgType { Bool, I64, U64, F64, Str, Bytes, Time, Pointer, Text }  // = value tags 1 2 3 4 5 6 10 12 17
pub enum LogArg<'a> { Bool(bool), I64(i64), U64(u64), F64(f64), Str(StrId), Bytes(&'a [u8]), Time(u64), Pointer(u64), Text(&'a str) }
```

`LogArg` implements `From` for `i8..i64`/`isize` (as `I64`),
`u8..u64`/`usize` (as `U64`), `f32`/`f64`, `bool`, `&str` and `&String`
(as `Text`), `&[u8]` (as `Bytes`) and `StrId` (as `Str`), so
`&[a.into(), name.into()]` builds an argument list without allocation.

```rust
pub fn log(&mut self, site: LogSiteId, time: u64, args: &[LogArg]) -> Result<TxId>
pub fn log_with_parent(&mut self, site: LogSiteId, time: u64, parent: Option<TxId>, args: &[LogArg]) -> Result<TxId>
pub fn log_raw(&mut self, site: LogSiteId, time: u64, parent: Option<TxId>, args: &[u8]) -> Result<TxId>
```

`log` records one message. `args` must match the site's declared types in
number and order (`Error::Invalid` otherwise: `"log site N expects 3
arguments, got 2"`, `"argument 1 is declared u64 but a i64 was passed"`);
`time` is the message time in file units and is independent of `set_time`
and of the previous message. The record gets the next transaction id (log
records and transactions share the id space) and `parent` links it to the
transaction being processed. Cost: the argument bytes plus four varints
appended to the log row buffer, no allocation; a log block is flushed to the
encoder when the buffer reaches `tx_block_bytes`. `log_raw` takes the
argument values already in row encoding (bool one byte, i64 zig-zag varint,
u64/time/pointer varint, f64 8 bytes little-endian, str varint id, text and
bytes varint length plus bytes); the caller is responsible for the match with
the site, a mismatch surfaces as `Error::Corrupt` from `flush`/`close`. It
exists for front ends that encode themselves (`vtr_log.hpp`).

`WriterStats::log_records` counts the records written (they are not included
in `transactions`).

## 4. Reader reference

```rust
pub struct Reader { /* private */ }   // Send + Sync
```

### 4.1 `ReadOptions`

```rust
#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    pub verify_crc: bool,          // default false
    pub group_cache: Option<usize>, // default None = 256 entries
}
```

* `verify_crc`: at open, every section payload with a non-zero CRC is hashed
  and compared (`Error::Checksum { offset }` on mismatch), including all
  signal and transaction blocks, so open becomes O(file size). Transaction
  blocks are verified again when first decoded; signal block payloads are not
  re-verified later.
* `group_cache`: number of decompressed group pieces (frame pieces and column
  runs) kept in an LRU cache shared by all queries. `Some(0)` disables
  caching. Each entry holds at most `run_bytes` of raw column data (or one
  group's frames) plus lazily built skip indexes.

### 4.2 Opening

```rust
pub fn open(path: impl AsRef<Path>) -> Result<Reader>
pub fn open_with(path: impl AsRef<Path>, opts: ReadOptions) -> Result<Reader>
pub fn from_bytes(bytes: Vec<u8>) -> Result<Reader>   // in-memory image, default options
```

`open` memory-maps the file read-only (with `MADV_RANDOM` on Unix). The file
must not be modified while mapped; doing so is undefined behaviour at the OS
level and is the caller's responsibility. What is read at open is listed in
section 1.2. A file without a `Meta` section (writer crashed before its first
flush) yields `Meta::default()`. Unknown section kinds are skipped when the
section carries `container::SECTION_FLAG_OPTIONAL`, otherwise
`Error::Corrupt("unknown required section kind")`.

```rust
pub fn meta(&self) -> &Meta
pub fn version(&self) -> (u16, u16)   // (major, minor) from the file header
pub fn recovered(&self) -> bool
```

**Crash recovery.** If the trailer is missing or inconsistent (no `VTR_END`
magic, wrong file length, or the directory it points to does not end exactly
where the trailer begins), the reader scans section headers from the start of
the file, stops at the first section whose length runs past the end (the
partially written tail), recomputes the directory `aux` fields from each
payload, and sets `recovered() = true`. Everything fully written before the
crash is readable: hierarchy and strings flushed before the last complete
block, all complete signal and transaction blocks. Data still buffered in the
writer (up to `block_records` changes, pending nodes, open transactions,
blackout markers) is lost. `Reader::from_bytes(b"not a vtr file".to_vec())`
returns `Error::Corrupt("not a VTR file (bad magic)")`.

`Meta`:

```rust
pub struct Meta {
    pub timescale: i8,          // 10^timescale seconds per unit; default -9
    pub time_zero: i64,         // display offset (FST timezero); default 0
    pub file_type: FileType,
    pub writer: String,
    pub date: String,
    pub comment: String,
    pub group_size: u32,        // signals per group, as rounded by the writer
    pub attrs: Vec<(StrId, Value)>,
}
```

### 4.3 Strings

```rust
pub fn strings(&self) -> &StringTable
pub fn str(&self, id: StrId) -> &str
```

`str` returns `""` for an out-of-range id (it never panics). `StringTable`
(`vtr::strings`) offers `len()`, `is_empty()`, `get(id) -> &str` (same
fallback), `try_get(id) -> Option<&str>` and `find(&str) -> Option<StrId>`
(linear scan; meant for tools and tests, not hot loops). All strings are
validated UTF-8 at open.

### 4.4 Hierarchy

```rust
pub fn hierarchy(&self) -> &Hierarchy
pub fn signal_count(&self) -> u32
pub fn signal_kind(&self, s: SignalId) -> Result<SignalKind>   // Error::Invalid for unknown ids
pub fn name(&self, n: NodeId) -> &str                          // panics on an out-of-range NodeId
pub fn full_path(&self, n: NodeId, sep: &str) -> String        // names from the root, joined with sep
pub fn find_node(&self, path: &[&str]) -> Option<NodeId>
pub fn find_signal(&self, path: &str, sep: char) -> Option<SignalId>
pub fn streams(&self) -> impl Iterator<Item = NodeId> + '_      // all Stream nodes, declaration order
pub fn generators(&self) -> impl Iterator<Item = NodeId> + '_   // all Generator nodes, declaration order
pub fn generator_stream(&self, gen: NodeId) -> Option<NodeId>   // parent of a generator node
```

`find_node` walks from the roots matching one path component per level by a
linear scan of that level's children (first match wins; nodes of any kind
participate, so a stream or enum table can be found too). `find_signal`
splits `path` on `sep`, calls `find_node`, and returns the signal of the
resulting node if it is a `Var` (alias or declaration), else `None`. Both are
O(path length x fan-out); for repeated lookups build your own map from
`full_path`.

`Hierarchy` (`vtr::hierarchy`) stores the nodes column-wise (a few bytes per
node: kind, parent, name, two kind-specific words; enum tables and attributes
in side tables), so opening a file with hundreds of thousands of variables
costs a few milliseconds. Fields are reached through accessors; `node` builds
a `Node` value on demand:

```rust
pub struct Hierarchy {
    pub signals: Vec<SignalKind>,  // indexed by SignalId
    pub signal_var: Vec<NodeId>,   // for every signal, the Var node that declared it
    /* private node columns and children index */
}
impl Hierarchy {
    pub fn node(&self, id: NodeId) -> Node                           // by value; panics when out of range
    pub fn kind(&self, id: NodeId) -> NodeKind
    pub fn parent(&self, id: NodeId) -> Option<NodeId>
    pub fn name(&self, id: NodeId) -> StrId
    pub fn signal_of(&self, id: NodeId) -> Option<SignalId>          // vars only
    pub fn enum_entries(&self, id: NodeId) -> Option<&[(StrId, StrId)]>
    pub fn attrs(&self, id: NodeId) -> &[(StrId, Value)]
    pub fn attr_count(&self) -> usize
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool
    pub fn ids(&self) -> impl Iterator<Item = NodeId>                 // all nodes, declaration order
    pub fn roots(&self) -> impl Iterator<Item = NodeId> + '_          // top-level nodes, declaration order
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ // declaration order
    pub fn signal_kind(&self, s: SignalId) -> Option<SignalKind>
    pub fn nodes_of_kind(&self, kind: NodeKind) -> impl Iterator<Item = NodeId> + '_
    // construction (used by the reader): new(), add_chunk(&[u8]), push(Node), build_index()
}
```

A `Reader` always returns an indexed hierarchy; `roots`/`children` are O(1)
per item (CSR layout). `node(id)` copies enum-table entries and attributes,
so prefer the accessors in tight loops.

```rust
pub struct Node {
    pub parent: Option<NodeId>,
    pub name: StrId,
    pub data: NodeData,
    pub attrs: Vec<(StrId, Value)>,
}
impl Node { pub fn kind(&self) -> NodeKind }

pub enum NodeData {
    Scope { scope_type: ScopeType, component: StrId },
    Var { var_type: VarType, direction: Direction, signal: SignalId, declares: Option<SignalKind> },
    Stream { kind: StrId },
    Generator,
    EnumTable { entries: Vec<(StrId, StrId)> },   // (literal, bit-string value)
}
impl NodeData { pub fn kind(&self) -> NodeKind }
```

`declares` is `Some(kind)` for the variable that created the signal and
`None` for aliases.

`NodeKind` codes (as stored): `Scope` = 1, `Var` = 2, `Stream` = 3,
`Generator` = 4, `EnumTable` = 5 (`NodeKind::from_u8` -> `Result`).

`ScopeType` (`code()`, `from_code(u16)`, `name()`); codes 0..=22 equal FST
`FST_ST_*`:

| Code | Variant | `name()` | Code | Variant | `name()` |
|---|---|---|---|---|---|
| 0 | `Module` | module | 15 | `VhdlRecord` | vhdl_record |
| 1 | `Task` | task | 16 | `VhdlProcess` | vhdl_process |
| 2 | `Function` | function | 17 | `VhdlBlock` | vhdl_block |
| 3 | `Begin` | begin | 18 | `VhdlForGenerate` | vhdl_for_generate |
| 4 | `Fork` | fork | 19 | `VhdlIfGenerate` | vhdl_if_generate |
| 5 | `Generate` | generate | 20 | `VhdlGenerate` | vhdl_generate |
| 6 | `Struct` | struct | 21 | `VhdlPackage` | vhdl_package |
| 7 | `Union` | union | 22 | `SvArray` | sv_array |
| 8 | `Class` | class | 64 | `Generic` | generic |
| 9 | `Interface` | interface | 65 | `ScModule` | sc_module |
| 10 | `Package` | package | 66 | `Resource` | resource (OpenTelemetry) |
| 11 | `Program` | program | 67 | `InstrumentationScope` | instrumentation_scope |
| 12 | `VhdlArchitecture` | vhdl_architecture | 68 | `Core` | core (CPU core / hardware thread) |
| 13 | `VhdlProcedure` | vhdl_procedure | other | `Other(u16)` | other (code preserved verbatim) |
| 14 | `VhdlFunction` | vhdl_function | | | |

`VarType` (`code()`, `from_code(u16)`, `name()`); codes 0..=29 equal FST
`FST_VT_*`:

| Code | Variant | `name()` | Code | Variant | `name()` |
|---|---|---|---|---|---|
| 0 | `Event` | event | 16 | `Wire` | wire |
| 1 | `Integer` | integer | 17 | `WOr` | wor |
| 2 | `Parameter` | parameter | 18 | `Port` | port |
| 3 | `Real` | real | 19 | `SparseArray` | sparray |
| 4 | `RealParameter` | real_parameter | 20 | `RealTime` | realtime |
| 5 | `Reg` | reg | 21 | `String` | string |
| 6 | `Supply0` | supply0 | 22 | `Bit` | bit |
| 7 | `Supply1` | supply1 | 23 | `Logic` | logic |
| 8 | `Time` | time | 24 | `Int` | int |
| 9 | `Tri` | tri | 25 | `ShortInt` | shortint |
| 10 | `TriAnd` | triand | 26 | `LongInt` | longint |
| 11 | `TriOr` | trior | 27 | `Byte` | byte |
| 12 | `TriReg` | trireg | 28 | `Enum` | enum |
| 13 | `Tri0` | tri0 | 29 | `ShortReal` | shortreal |
| 14 | `Tri1` | tri1 | 64 | `Bits` | bits (generic bit vector) |
| 15 | `WAnd` | wand | 65 | `Bytes` | bytes (generic blob) |
| | | | other | `Other(u16)` | other |

`VarType` is descriptive only; the storage format is fixed by `SignalKind`.

`Direction` (`from_u8`, `name()`; equals FST `FST_VD_*`): `Implicit` = 0,
`Input` = 1, `Output` = 2, `InOut` = 3, `Buffer` = 4, `Linkage` = 5; unknown
codes decode as `Implicit`.

`SignalKind`:

```rust
pub enum SignalKind {
    Bits { width: u32, states: u8 },  // states = 2, 4 or 9
    Real,                             // IEEE 754 double
    VarLen,                           // variable-length bytes
}
impl SignalKind {
    pub fn packed_len(self) -> Option<usize> // bytes per value; None for VarLen
    pub fn width(self) -> u32                // 64 for Real, 0 for VarLen
    pub fn states(self) -> u8                // 0 for Real/VarLen
}
```

Stored codes: 0 = `Bits` 2-state, 1 = `Bits` 4-state, 2 = `Bits` 9-state
(each followed by the width), 3 = `Real`, 4 = `VarLen`.

### 4.5 Time and blocks

```rust
pub fn time_range(&self) -> Option<(u64, u64)>
pub fn block_count(&self) -> usize
pub fn block_range(&self, i: usize) -> (u64, u64)          // (start_time, end_time); panics if i out of range
pub fn block_times(&self, i: usize) -> Result<Arc<Vec<u64>>> // decoded time table of block i, cached
pub fn time_table(&self) -> Result<&[u64]>                  // all distinct time steps, ascending, cached
pub fn blackout(&self) -> &[Blackout]
```

`time_range` is the union of `[start_time, end_time]` over signal blocks with
at least one time step and `[t_min, t_max]` over transaction blocks with at
least one transaction; `None` for a file with neither. It is computed from the
headers read at open (no I/O).

`block_times` decompresses a block's time table on first use and keeps it for
the reader's lifetime (`OnceLock`). `time_table` concatenates all block tables,
dropping the duplicate boundary entry between consecutive blocks, and caches
the result.

`Blackout { time: u64, active: bool }`: `active = false` means dumping
stopped at `time`, `true` means it resumed. Returned in the order recorded.

### 4.6 Signal queries

```rust
pub fn value_at(&self, sig: SignalId, t: u64) -> Result<OwnedSignalValue>
```

The value of the last change at or before `t`, or the default initial value
(all X for 4/9-state vectors, all 0 for 2-state vectors, `0.0`, empty bytes)
when the signal has not changed by `t` (including when `t` precedes the first
block). Errors: `Error::Invalid` for an unknown signal; `Error::Corrupt` /
`Error::Codec` for damaged data. Bit vectors come back with `states: 2` when
the stored value was compact, otherwise with the declared states; use
`OwnedSignalValue::to_ascii()` or `borrow().as_u64()` to be packing-agnostic.

```rust
pub fn changes(&self, sig: SignalId, t0: u64, t1: u64) -> Result<Vec<(u64, OwnedSignalValue)>>
```

All changes of `sig` with `t0 <= time <= t1` (both inclusive), in time order,
same-time updates in emission order. Visits only blocks whose range
intersects `[t0, t1]` and only the run containing `sig` in each.

```rust
pub fn load_signal(&self, sig: SignalId) -> Result<SignalData>
pub fn load_signals(&self, sigs: &[SignalId]) -> Result<Vec<SignalData>>   // same order as sigs
```

Load the complete change history. `load_signals` deduplicates signal IDs and
sorts them by `(group, signal)`. For every block it decompresses the frame piece (only for
signals seen for the first time) and each column run holding at least one
requested signal exactly once, so loading all signals of a group costs one
decompression per run per block. Signals from groups that are not dirty in a
block cost only a binary search of that block's dirty index. Duplicate ids in
`sigs` are allowed: output order and multiplicity match the request, but repeated
IDs share one immutable history. Declared aliases resolve to the same signal ID
and benefit from the same sharing. Distinct IDs with per-block dynamic aliases
remain distinct histories. An empty request returns an empty vector; any invalid
ID or decoding error fails the whole call.

Each unique history is built once in mutable internal buffers and then frozen.
Cloning a `SignalData` shares all of its storage without copying buffers. Handles
own that storage independently of the reader, can outlive it, and are `Send + Sync`.
There is no persistent history cache: separate load calls materialize independently,
and the last handle releases the storage. Consumers needing mutable data explicitly
copy it.

```rust
#[derive(Clone, Debug)]
pub struct SignalData { /* private shared immutable storage */ }
impl SignalData {
    pub fn kind(&self) -> SignalKind                   // declared kind and packing
    pub fn times(&self) -> &[u64]                      // non-decreasing; same-time emission order
    pub fn len(&self) -> usize
    pub fn is_empty(&self) -> bool
    pub fn get(&self, i: usize) -> SignalValue<'_>        // change i (panics when i >= len)
    pub fn initial(&self) -> SignalValue<'_>
    pub fn index_at(&self, t: u64) -> Option<usize>       // last change with time <= t
    pub fn value_at(&self, t: u64) -> SignalValue<'_>     // get(index_at(t)) or initial()
}
```

Unlike the reader's point queries, `SignalData` always widens compact values
to the declared packing, so `get(i)` reports `states` equal to
`kind().states()`. Packed buffers and variable-length offsets are private;
use `get(i)` for read-only value access. `index_at`/`value_at` are binary
searches over `times()`.

```rust
pub fn for_each_change(&self, t0: u64, t1: u64, f: impl FnMut(u64, SignalId, SignalValue<'_>)) -> Result<()>
```

Streams every change of every signal with `t0 <= time <= t1`, VCD-dump
style. Per block it decompresses all runs of all dirty groups (bypassing the
group cache) and opens a cursor on every non-empty column. Blocks with up to
32 Ki active columns are merged through a linked list per time step (the
scheme of GTKWave's block iterator: no sorting, no per-change memory); larger
blocks are walked in windows of time steps whose entries are counting-sorted
by time index. **Ordering guarantees:**

* times are non-decreasing across the whole call;
* multiple changes of the same signal at the same time step keep emission
  order;
* within one time step the order of *different* signals is deterministic for
  a given file but otherwise unspecified;
* a time step that closes one block and opens the next delivers the first
  block's entries before the second's.

Values passed to `f` borrow a per-block buffer; call `to_owned()` to keep one.
Bit vectors are reported with `states: 2` when stored compact. Peak memory is
the decompressed size of the largest block plus a few dozen bytes per active
column; it does not grow with the number of changes.

```rust
pub fn packed_len(&self, s: SignalId) -> Option<usize>
```

`Some(bytes)` for `Bits` (declared packing) and `Real` (8); `None` for
`VarLen` or an unknown signal.

### 4.7 Transactions

```rust
#[derive(Clone, Debug, Default)]
pub struct TxQuery {
    pub generator: Option<NodeId>,   // only this generator
    pub stream: Option<NodeId>,      // only generators whose parent is this stream
    pub window: Option<(u64, u64)>,  // only transactions overlapping [t0, t1]: end >= t0 && begin <= t1
}
```

```rust
pub fn tx_block_count(&self) -> usize
pub fn tx_counts(&self) -> (u64, u64)   // (transactions, relations) summed from block headers; no decoding
pub fn visit_transactions(&self, q: &TxQuery, f: impl FnMut(&Transaction) -> bool) -> Result<()>
pub fn transactions(&self, q: &TxQuery) -> Result<Vec<Transaction>>
pub fn transaction(&self, id: TxId) -> Result<Option<Transaction>>
pub fn relations_from(&self, id: TxId) -> Result<Vec<Relation>>
pub fn relations_to(&self, id: TxId) -> Result<Vec<Relation>>
pub fn visit_relations(&self, f: impl FnMut(&Relation) -> bool) -> Result<()>
```

`visit_transactions` walks blocks in file order and, within a block,
transactions in row order (= `end_tx` order, with transactions closed at
`close()` last). Block-level pruning uses header data only: the `window`
filter skips blocks whose `[t_min, t_max]` does not intersect it, and the
`generator` filter skips blocks whose sorted generator list (stored
uncompressed in the header) does not contain the generator. The `stream`
filter has no block-level index and decodes every candidate block. Return
`false` from the callback to stop early. `transactions` collects clones.

`transaction(id)` decodes only blocks whose `[min_id, max_id]` covers `id`
(ids are dense and increasing, so normally exactly one block).
`relations_from`/`relations_to` similarly use the per-block
`[rel_min_from, rel_max_from]` / `[rel_min_to, rel_max_to]` ranges; a
relation whose endpoints are far apart in id space makes those ranges wide and
the pruning weaker. `visit_relations` visits every relation in file order.

Decoded transaction blocks are cached for the reader's lifetime (`OnceLock`
per block), so repeated lookups are in-memory; a reader that has touched every
block holds all transactions decoded (`tx_block_bytes` of rows each, expanded
into `Transaction` structs).

Transaction model (`vtr::txblock`, all `Clone + Debug + PartialEq`):

```rust
pub type TxId = u64;                         // unique in the file, starts at 1

pub struct Transaction {
    pub id: TxId,
    pub generator: NodeId,
    pub begin: u64,
    pub end: u64,                            // >= begin
    pub status: TxStatus,
    pub kind: TxKind,
    pub parent: Option<TxId>,
    pub attrs: Vec<TxAttr>,                  // call order
    pub events: Vec<TxEvent>,                // call order
    pub stages: Vec<TxStage>,                // call order
}
pub struct TxAttr  { pub key: StrId, pub phase: AttrPhase, pub value: Value }
pub struct TxEvent { pub time: u64, pub name: StrId, pub attrs: Vec<(StrId, Value)> }
pub struct TxStage { pub name: StrId, pub lane: StrId, pub begin: u64, pub end: Option<u64>, pub attrs: Vec<(StrId, Value)> }
pub struct Relation { pub kind: StrId, pub from: TxId, pub to: TxId, pub attrs: Vec<(StrId, Value)> }
```

`TxStage::end` is `None` only for rows encoded with a still-open stage; the
`Writer` always closes stages at `end_tx`/`close`, so files written by this
crate always yield `Some(end)`. Code tables: `TxStatus` `Unset` = 0, `Ok` = 1,
`Error` = 2, `Aborted` = 3, `Open` = 4 (`from_u8` maps unknown codes to
`Unset`; `name()` gives `"unset" | "ok" | "error" | "aborted" | "open"`);
`TxKind` `Unspecified` = 0, `Internal` = 1, `Server` = 2, `Client` = 3,
`Producer` = 4, `Consumer` = 5; `AttrPhase` `Begin` = 0, `Record` = 1,
`End` = 2 (unknown codes decode as `Record`).

### 4.8 Sections (tools)

```rust
pub fn sections(&self) -> &[DirEntry]
```

The directory as read (or rebuilt). `vtr::container::DirEntry { kind: u32,
flags: u32, offset: u64, len: u64, aux0: u64, aux1: u64 }` with
`payload_offset()`; `kind` matches `vtr::container::SectionKind` (`Meta` = 1,
`Strings` = 2, `Hierarchy` = 3, `SignalBlock` = 4, `TxBlock` = 5,
`Blackout` = 6, `Directory` = 7; kinds `>= 0x1000` are free for private
extensions, and `flags & SECTION_FLAG_OPTIONAL` lets readers skip a kind they
do not know). `aux0`/`aux1` are `(first_id, count)` for string and hierarchy
chunks and `(first_time, last_time)` for blocks. Use with
`Container::payload(bytes, entry, verify)` from `vtr::container` to get raw
payload bytes for inspection.

---

### 4.9 Logs

```rust
pub struct LogQuery { pub stream: Option<NodeId>, pub generator: Option<NodeId>, pub min_severity: Severity, pub window: Option<(u64, u64)> }
pub fn log_sites(&self) -> &[LogSite]
pub fn log_site(&self, gen: NodeId) -> Option<&LogSite>
pub fn log_count(&self) -> u64
pub fn log_block_count(&self) -> usize
pub fn visit_log(&self, q: &LogQuery, f: impl FnMut(&LogRecord) -> bool) -> Result<()>
```

`log_sites` lists every generator of a `LOG` stream that carries `log.args`
(built at open from the hierarchy):

```rust
pub struct LogSite { pub index: u32, pub node: NodeId, pub stream: NodeId, pub severity: Severity, pub fmt: StrId,
                     pub file: Option<StrId>, pub line: Option<u32>, pub func: Option<StrId>, pub args: Vec<LogArgType>, pub names: Vec<StrId> }
```

`visit_log` walks the log blocks in order of their first time (production
order for a producer with monotonic time) and the records of a block in
production order. A block is skipped without decompression when its time
range misses `window` or when none of the sites listed in its header passes
the `stream`, `generator` and `min_severity` filters; decoded blocks are
cached like transaction blocks. The callback receives a borrowed record:

```rust
pub struct LogRecord<'a> { pub id: TxId, pub time: u64, pub parent: Option<TxId>, pub site: &'a LogSite, .. }
impl LogRecord<'_> {
    pub fn severity(&self) -> Severity;
    pub fn arg_count(&self) -> usize;
    pub fn arg(&self, i: usize) -> Option<LogArg<'_>>;          // typed value, Text/Bytes borrow the block
    pub fn args(&self) -> impl Iterator<Item = LogArg<'_>>;
    pub fn format_into(&self, strings: &StringTable, out: &mut String);  // the message text
    pub fn format(&self, strings: &StringTable) -> String;
    pub fn to_transaction(&self) -> Transaction;               // zero-duration, attrs keyed by log.names
}
```

Formatting uses the site's format string parsed once at open
(`vtr::logfmt`, the `{}` / `{:spec}` subset shared by Rust and C++
`std::format`; a placeholder without an argument renders `{?}`). About ten
million lines per second on the benchmark log.

Log records are also transactions: `visit_transactions`, `transactions` and
`transaction(id)` return them (`begin == end == time`, status `Unset`, kind
`Unspecified`, attributes = arguments keyed by `log.names`, `Text` arguments
as `Value::Text`), interleaved with the transaction blocks in file order;
`tx_counts().0` includes them and `log_count` gives their number alone.

## 5. Value model

### 5.1 `Value` (attributes)

`Value` is the universal typed attribute used on hierarchy nodes, file
metadata, transactions, events, stages and relations. It is a superset of the
FTR/SCV attribute types, the OpenTelemetry `AnyValue` model and FST attribute
payloads.

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(StrId),
    Bytes(Vec<u8>),
    Bits   { width: u32, data: Vec<u8> },
    Logic  { width: u32, data: Vec<u8> },
    Logic9 { width: u32, data: Vec<u8> },
    Time(u64),
    Enum { value: i64, name: StrId },
    Pointer(u64),
    Fixed  { raw: i64, scale: i32 },
    UFixed { raw: u64, scale: i32 },
    List(Vec<Value>),
    Map(Vec<(StrId, Value)>),
    Text(String),
}
impl Value { pub fn tag(&self) -> ValueTag; /* encode/decode helpers */ }
```

| Variant | Tag (`ValueTag`) | Meaning / encoding |
|---|---|---|
| `Null` | 0 | Absent value. |
| `Bool` | 1 | One byte. |
| `I64` | 2 | Zig-zag LEB128. |
| `U64` | 3 | LEB128. |
| `F64` | 4 | 8 bytes little-endian IEEE double. |
| `Str` | 5 | Interned string id (intern with `Writer::intern`, resolve with `Reader::str`). |
| `Bytes` | 6 | Opaque blob, length-prefixed. |
| `Bits` | 7 | 2-state vector, `width` bits, packed LSB-first (bit i at byte i/8, bit i%8); `data.len() == packed_len(width, 2)`. |
| `Logic` | 8 | 4-state vector, 2 bits per bit (codes 0,1,X,Z), LSB-first; `packed_len(width, 4)` bytes. |
| `Logic9` | 9 | 9-state vector (IEEE 1164), 4 bits per bit, LSB-first; `packed_len(width, 9)` bytes. |
| `Time` | 10 | Time in the file's timescale units. |
| `Enum` | 11 | Integer value plus interned literal name. |
| `Pointer` | 12 | Opaque handle. |
| `Fixed` | 13 | Signed fixed point: value = `raw * 2^-scale`. |
| `UFixed` | 14 | Unsigned fixed point. |
| `List` | 15 | Ordered list of values (nesting allowed). |
| `Map` | 16 | Ordered key/value pairs, keys interned. |
| `Text` | 17 | Inline UTF-8 text, not interned (one-off strings; log arguments of type `Text` appear as this when a record is read as a transaction). Added in format 1.1. |

`F64` participates in `PartialEq` by IEEE comparison (`NaN != NaN`), so
`Value` is not `Eq`/`Hash`. In transaction blocks the scalar variants are
stored column-wise (numbers, floats, string ids and the rest in separate
columns) which is why numeric attributes compress well; this is transparent
to the API.

### 5.2 Signal values and packing

Logic codes: `0`=0, `1`=1, `2`=X, `3`=Z, `4`=U, `5`=W, `6`=L, `7`=H, `8`=-
(`signal::CODE_ASCII = b"01xzuwlh-"`). A vector of `width` bits is packed
**LSB-first**: bit `i` (bit 0 = least significant = the last character of a
VCD string) lives at

| `states` | bits per code | location of bit `i` | `packed_len(width, states)` |
|---|---|---|---|
| 2 | 1 | byte `i/8`, bit `i%8` | `ceil(width/8)` |
| 4 | 2 | byte `i/4`, bits `2*(i%4)..+2` | `ceil(width/4)` |
| 9 | 4 | byte `i/2`, bits `4*(i%2)..+4` | `ceil(width/2)` |

codes occupying the low-to-high bit positions of each byte; unused high bits
of the last byte are zero. `vtr::value::packed_len(width, states)` computes the
byte count (any `states` other than 2 or 4 is treated as 9).

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignalValue<'a> {
    Bits { width: u32, states: u8, data: &'a [u8] },  // states = packing actually used (2 when compact)
    Real(f64),
    VarLen(&'a [u8]),
}
impl<'a> SignalValue<'a> {
    pub fn to_ascii(&self) -> String        // MSB-first bit string / Display of f64 / lossy UTF-8 of bytes
    pub fn as_u64(&self) -> Option<u64>     // Some for Bits with width <= 64 whose codes are all 0/1
    pub fn to_owned(&self) -> OwnedSignalValue
}

#[derive(Clone, Debug, PartialEq)]
pub enum OwnedSignalValue {
    Bits { width: u32, states: u8, data: Vec<u8> },
    Real(f64),
    VarLen(Vec<u8>),
}
impl OwnedSignalValue {
    pub fn borrow(&self) -> SignalValue<'_>
    pub fn to_ascii(&self) -> String
}
```

`as_u64` returns `None` for reals, varlen, vectors wider than 64 bits and
vectors containing any X/Z/U/W/L/H/- code. Equality of `SignalValue` compares
`states` too: the same 0/1 vector in compact and declared packing is not
`==`; compare `to_ascii()` or `as_u64()` instead, or widen.

### 5.3 Helpers in `vtr::signal`

| Function | Purpose |
|---|---|
| `code_from_ascii(c: u8) -> u8` | VCD/VHDL character to code; unknown characters give X. |
| `states_for_code(code: u8) -> u8` | Minimum states (2, 4 or 9) able to hold a code. |
| `get_code(data, states, i) -> u8` | Read code of bit `i` from a packed vector. |
| `set_code(data, states, i, c)` | OR code `c` into bit `i` (target bits must be zero). |
| `pack_ascii(s, width, states, out) -> bool` | Pack an MSB-first ASCII string (extension/truncation rules of 3.7); returns `true` when all codes were 0/1. `out` must be `packed_len` bytes and is zeroed first. |
| `unpack_ascii(data, width, states, out: &mut Vec<u8>)` | Append the MSB-first ASCII spelling. |
| `widen(data, width, from, to, out: &mut Vec<u8>)` | Re-pack from `from` states to `to >= from` states (2->4 and 2->9 use lookup tables). |
| `narrow_to_two_state(data, width, states, out)` | Re-pack to 2-state; valid only when `is_two_state`. |
| `is_two_state(data, width, states) -> bool` | True when every code is 0 or 1. |
| `pack_words32(words: &[u32], width, out)` / `pack_words64(words: &[u64], width, out)` | Little-endian words to 2-state packing (`out` >= `ceil(width/8)` bytes); bits above `width` cleared. |
| `trim_top_bits(out, width)` | Clear bits above `width` in the last byte of a 2-state vector. |
| `default_value(kind, out: &mut Vec<u8>)` | Append the initial value: all X (4/9 states), zeros (2 states), `0.0`, or nothing for `VarLen`. |
| constants `L0 L1 LX LZ LU LW LL LH LDASH`, `CODE_ASCII` | Logic codes and their spelling. |

### 5.4 Compression types

```rust
pub enum Codec { None = 0, Lz4 = 1, Zstd = 2 }          // Codec::from_u8 -> Result
pub struct Compression { pub codec: Codec, pub level: i32 } // level: zstd 1..=22; ignored otherwise
impl Compression {
    pub const NONE: Compression;         // no compression
    pub const LZ4: Compression;          // LZ4 block format
    pub const ZSTD_FAST: Compression;    // zstd level 1 (Compression::default())
    pub const ZSTD_DEFAULT: Compression; // zstd level 3 (WriterOptions::default())
    pub const ZSTD_HIGH: Compression;    // zstd level 9
}
```

`vtr::codec::Compressor` / `Decompressor` are the reusable contexts used
internally; every compressed blob is self-describing
(`[codec:u8][raw_len:varint][payload]`), so a file may mix codecs and a
reader needs no configuration.

---

## 6. Threading and performance

### 6.1 Writer

* **Hot path.** `emit_u64` / `emit_bit` on a narrow signal (packed value
  `<= 8` bytes: any 2-state vector up to 64 bits, 4-state up to 32 bits,
  9-state up to 16 bits, and reals) is a bounds check, a compare against the
  last value, and a 16-byte push. No allocation, no hashing. Wide vectors
  bypass the record log: each change is appended, already in column format,
  to a per-signal header/value buffer pair. Varlen values copy their bytes
  into a chunk-local heap.
* **Background thread.** With `background: true` (default) the simulator
  thread never sorts, encodes or compresses. Every `chunk_records` changes it
  hands the record log (plus the wide-vector buffers) to the `vtr-writer`
  thread through a bounded channel, which counting-sorts the chunk by signal
  and pre-encodes column fragments immediately; when `block_records` changes
  have been handed over, the block is finished (fragments concatenated per
  signal, frames and runs compressed, section written). Buffers come back
  through a recycle channel, so steady state allocates nothing. The only
  blocking happens when four messages are already queued (the encoder is
  slower than the producer) or at `close`.
* **Dedup is free** in the sense that it replaces a record push with a
  compare; leave it on unless every emit must be preserved.
* **Block size** trades compression against read granularity: the encoder
  keeps up to `block_records` records worth of column fragments (a few
  tens of MiB at the default, since fragments are already delta-coded)
  before compressing; longer columns compress better,
  and the reader's `value_at` must skip through at most one block's column
  for the signal (with a skip index every 256 entries). `chunk_records`
  only controls how early the background thread starts working.
* **Guidance for simulator integration.**
  * Pre-intern attribute keys, event names, stage names and lanes once
    (`intern`) and keep the `StrId`s; `tx_attr`/`tx_event`/`tx_stage*` take
    ids and never touch the string table.
  * Use `emit_u64` / `emit_words` (or `emit_packed` with `states = 2`) for
    values known to be 0/1; use `emit_logic_str` only when converting textual
    formats. It parses every character and validates against the declared
    states.
  * Batch time: call `set_time` once per time step, then emit all changes;
    repeated `set_time` with the same value is a no-op but still a call.
  * Declare all signals before the first time step where possible; declaring
    signals later works but creates a new hierarchy chunk at the next flush.
  * Set metadata immediately after `create`; add node attributes immediately
    after declaring the node.
  * Keep the number of concurrently open transactions bounded: the open table
    holds their attributes/events/stages in memory until `end_tx`.
  * Call `close()` and check its result; `Drop` hides errors.

### 6.2 Reader

* **Sharing.** `Reader` is `Sync`; wrap it in an `Arc` and query from any
  number of threads. The group cache and per-piece skip indexes are behind
  mutexes held only for a lookup/insert; decompression runs outside the lock.
  Block time tables and decoded transaction blocks are `OnceLock`s (first
  decoder wins; concurrent first access may decode twice and drop one).
* **Cost of `value_at(sig, t)`.**
  1. Binary search over the in-memory block headers for the last block with
     `start_time <= t`.
  2. Binary search in that block's dirty index (mmap'd, 16 bytes per dirty
     group) for the signal's group; if absent, one 4-byte read of the
     prev-dirty table jumps directly to the last block in which the group
     changed (no scan through intermediate blocks).
  3. Parse the group container header (varints, no decompression).
  4. If the change is in the same block: the block's time table (decompressed
     once per block, cached) and a binary search for the time index.
  5. Decompress **one column run** (at most `run_bytes` raw, default 64 KiB)
     unless it is in the group cache.
  6. If the signal's column is larger than 4 KiB, build (once, cached with the
     piece) a skip index with a checkpoint every 256 entries and seek to the
     last checkpoint `<= t`.
  7. Walk forward at most ~256 entries to the last change `<= t`.
  8. If the signal has no change in the block up to `t`, decompress the
     group's frame piece (cached) and return the block-start value.

  Repeated `value_at` calls on signals of the same group and block are served
  from the cache; a viewer painting N signals at one cursor time costs at most
  one run decompression per distinct run.
* **`changes` / `load_signals`** touch only the blocks whose time range
  intersects the request (`changes`) or all blocks (`load_signals`), and
  within each block only the runs containing requested signals plus the
  frame piece. Loading every signal of a group is one decompression per run
  per block, i.e. the same work as decompressing the group once.
* **`for_each_change`** is the sequential path: it decompresses whole blocks
  (all runs, bypassing the cache) and merges the columns by time (linked
  list per time step, or windowed counting sort for blocks with very many
  active columns). Use it for VCD export and full-trace statistics, not for
  random access.
* **Group cache.** Keyed by `(block, group, piece)` where piece 0 is the
  frames and piece `k+1` is run `k`; LRU with a linear scan over at most
  `group_cache` entries (default 256). Memory is bounded by roughly
  `group_cache * run_bytes` plus skip indexes. Raise it for viewers that keep
  many signals visible; lower or disable it for one-shot batch jobs.
* **Memory mapping.** Nothing is read until touched; the OS page cache does
  the rest. For files on slow network storage prefer reading into memory and
  `Reader::from_bytes`.
* **`ReadOptions::verify_crc`** turns `open` into a full-file read; use it in
  validation tools, not in viewers.
* Hierarchy lookups by path (`find_node`, `find_signal`) are linear per level;
  build a `HashMap<String, SignalId>` from `full_path` for repeated lookups.

---

## 7. Cross-references

* [`docs/SPEC.md`](SPEC.md) - the VTR file format: container layout,
  section kinds and headers, string/hierarchy chunk encoding, signal block
  layout (time table, dirty index, prev-dirty table, group containers, frames,
  column runs, column entry encoding), transaction block columns, value
  encoding, LEB128/zig-zag, CRC and recovery rules. The `block`, `container`,
  `txblock`, `sections`, `strings`, `value` and `varint` modules of this crate
  are its reference implementation.
* [`docs/API_C.md`](API_C.md) - the C API in `crates/vtr-capi`
  (`vtr_*` functions, header `crates/vtr-capi/include/vtr.h`), a thin wrapper
  over the `Writer` and `Reader` described here with the same semantics:
  identical option fields, error categories (reported through
  `vtr_last_error`), logic codes, packing rules and query behaviour.
* Real-world usage inside this repository: `crates/vtr-bench/src/write.rs`
  (replaying a VCD/FST workload into the writer with `emit_u64`,
  `emit_packed`, `emit_logic_str`, `emit_real`, `emit_varlen`),
  `crates/vtr-cli/src/kanata.rs` (Konata pipeline logs to streams,
  generators, transactions, stages, events and relations), and
  `crates/vtr/tests/roundtrip.rs` (round-trip tests covering every emit path,
  transactions, crash recovery, error cases and skip-index checkpoints).

## RTL VDB companion API

The separate `vtr-vdb` crate exports `Database::open(path)`,
`Debugger::attach(&database, &reader, prefix)`, and
`Debugger::trace(symbol, time, depth) -> Result<TraceNode, String>`.
`TraceNode` exposes values, moments, source locations, reasons, diagnostic notes
and children; `render()` returns the same plain-text tree used by the CLI.
`Database::trace_binding` optionally holds a `TraceBinding { prefix, signals }`
from a simulator recording. With this binding, pass an empty prefix to
`Debugger::attach`; it uses the exact recorded paths and requires a matching
`design.vdb_id`. A conflicting explicit prefix is an error. Unbound standalone
exports retain structural matching and the existing explicit-prefix behavior.

The debugger borrows the immutable database and reader and caches loaded
histories per signal ID. See [VDB_RTL.md](VDB_RTL.md) for its schema and
semantic contract. A read-only `SignalData::times()` accessor supports the
companion; the VTR storage format is unchanged.


The companion also exposes `vtr_vdb::netlist`:

```rust,ignore
let index = vtr_vdb::netlist::NetlistIndex::new(&database)?;
let view = index.module("top.u0")?.layout()?;
let svg = view.svg(&mut debugger, 26)?;
std::fs::write("stage.svg", svg)?;
```

`NetlistIndex` borrows the database and indexes module ownership. `module`
returns an owned `Netlist` containing `blocks` (IDs, titles, source details,
child instance paths, named pins) and `wires` (block/pin endpoints). `layout`
consumes it and returns a `LaidOutNetlist` with public netlist and ELK geometry.
`svg` borrows that layout and samples the attached debugger; repeated times
reuse geometry and cached immutable histories. All operations return
`Result<_, String>`. VDB v2 requires explicit ownership, ports, and static reads;
v1 exports must be regenerated. The companion requires Rust 1.88+ and is validated with Rust 1.88 and 1.96.

The pinned Surfer adapter uses these attachment rules to associate a same-stem
`.vdb` (or `.vdb.json`) with a local VTR document and navigate from recorded
signals to RTL declarations. See [Surfer source navigation](VDB_RTL.md#surfer-source-navigation)
for user behavior and the example-based verification scope. This introduces no
new public VTR or VDB library API.
