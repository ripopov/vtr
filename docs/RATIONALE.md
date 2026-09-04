# Design rationale

This document records what was researched, what VTR borrowed, what it
rejected, and why the format and API look the way they do. The research
notes behind it (feature inventories of FST, FTR, Kanata, OpenTelemetry and
of wavepeek's reader usage) were produced from the sources under `ext/`
and the public specifications; the resulting feature matrix is in
`docs/COVERAGE.md`.

## 1. Goals that shaped the design

1. One file for waveforms, transactions, hierarchy and relations, with one
   time base (the FSDB model, without the KDB).
2. Beat FST and FTR on size, write speed and read/navigation speed on
   realistic workloads, and prove it with a reproducible benchmark.
3. Local queries must not read the whole file; opening must be cheap.
4. Small, obvious APIs in Rust and C; no global state; a writer that a
   simulator can call at tens of millions of changes per second.
5. Versioned and extensible from day one; readable after a crash.

## 2. What was studied

* **FST** (`fstapi.c`, libfstwriter, fst-reader): block structure,
  per-signal chains with delta-coded time indexes, frames, dynamic alias
  tables, the hierarchy byte stream, attribute conventions, the reader's
  random-access path (`fstReaderGetValueFromHandleAtTime`).
* **FTR / LWTR4SC**: CBOR chunk stream, dictionary chunks, tx blocks
  ordered by end time, attribute type codes, relations, the lwtr frontend
  API and its callback design.
* **Kanata / Konata**: the nine commands, lane/stage semantics, retire and
  flush, dependencies, what the viewer derives versus stores.
* **OpenTelemetry**: spans, kinds, events, links, status, resources and
  instrumentation scopes, `AnyValue`.
* **wavepeek** and **wellen**: the query mix of an LLM-driven waveform
  CLI (open, hierarchy listing, sample-at-time, change windows,
  condition scans), how wellen opens FST (all block headers and time
  tables up front) and loads signals (re-walk every block).
* Columnar and trace formats for encoding and indexing ideas: Parquet
  (column chunks, run-level statistics, dictionary and delta encodings),
  Arrow (struct-of-arrays), Perfetto (packet-based, incremental interning,
  crash-tolerant appends), Chrome Trace Event Format (flat JSON events;
  rejected as a model for anything but export), and the wellen in-memory
  representation (per-signal `time_indices` + packed data).

## 3. Container

**Chosen**: a flat sequence of self-delimiting sections with a directory
written last and a fixed trailer; recovery by scanning when the trailer is
missing.

* Borrowed from FST/Perfetto: append-only sections, streaming-friendly.
* Rejected FST's "no table of contents" (readers scan and patch): the
  directory gives O(1) access to every block and its time range, which is
  what makes `value_at` a two-lookup operation.
* Rejected FST's `.hier` sidecar and end-of-file patching of the header:
  VTR needs no seek-back at all (`FST_BL_SKIP` placeholders and
  `fseek` failure latches disappear), so it works on pipes and
  append-only storage, and a crashed run leaves a readable file.
* Rejected the whole-file gzip wrapper (`ZWRAPPER`): it destroys random
  access; VTR compresses at run granularity instead.
* Per-section CRC-32, an *optional* flag on unknown sections and reserved
  private kinds give extensibility without a version bump.

## 4. Strings and hierarchy

**Chosen**: an interned string table (FTR's dictionary idea, generalised to
every name and attribute key) and a flat, append-only node list with
explicit parents, kinds and typed attributes.

* FST stores names inline and binds attributes by position in a byte
  stream; enum tables are escaped space-separated strings. VTR keeps FST's
  scope/var/direction numbering (so converters are trivial and KDBs can
  reason about HDL types) but replaces positional attributes with typed
  key/value lists on the node they belong to, and enum tables with
  first-class nodes.
* Streams and generators (FTR) and resources/instrumentation scopes
  (OpenTelemetry) are ordinary nodes in the same tree, so a transaction
  stream can sit under the RTL module it instruments.
* Hierarchy chunks may be appended during the run: FTR needs this
  (generators appear late) and FST cannot do it.
* The hierarchy is compressed as one blob per chunk; opening decompresses
  it once (about 30 KB for the SCR1 design). FST's gzip-of-LZ4 hierarchy
  took most of wellen's 2.7 ms open time on the same design; VTR opens in
  0.25 ms.

## 5. Signal values

### 5.1 Encoding

**Chosen**: FST's proven per-signal chain idea (time-table index deltas +
values), with three changes.

1. Bit-packed 4- and 9-state values (2 and 4 bits per bit) instead of one
   ASCII byte per bit, plus a per-change *compact* flag so that 2-state
   values on 4-state signals cost 1 bit per bit. FST packs only pure
   2-state vectors and expands anything with an X to a byte per bit.
2. Entry headers and values are separate streams inside a column
   (Parquet's lesson: homogeneous streams compress better). On the RSA-256
   workload with high-entropy 256-bit values this alone recovered ~3%,
   turning a 0.5% loss against FST-zlib into a win.
3. Values are never converted to text on the write path; the C API takes
   integers, word arrays, packed bits or ASCII, whichever the simulator
   has.

Rejected: XOR-delta against the previous value (no gain on the RTL traces
measured, and it doubles the work on the read path); per-value dictionary
coding (zstd's match finder already captures repeats within a run).

### 5.2 Blocks, groups and runs

**Chosen**: time-partitioned blocks (as FST) whose signals are partitioned
into fixed groups of 256 ids; each group stores a *frame* (values at block
start) and its columns split into independently compressed *runs* of at
most 64 KiB raw; a per-block dirty index and a prev-dirty table.

* FST compresses every chain separately (small chains stay raw, no
  cross-signal context) and stores one monolithic zlib frame per block.
  VTR's runs give the compressor tens of kilobytes of neighbouring
  signals (same module, correlated activity) while bounding what a single
  `value_at` must decompress. Measured on SCR1: whole-group blobs were
  2.4 MiB but a random value query cost 15 us; 64 KiB runs are 2.2 MiB
  and 1.1 us per warm query.
* Groups are fixed ranges of ids so that the prev-dirty table can point a
  reader to the last block in which a signal changed; FST readers must
  walk backwards block by block.
* Runs dominated by wide values switch to zstd level 1, and runs whose
  4 KiB sample zstd-1 cannot shrink by 4% skip zstd altogether (LZ4 bails
  out fast on such data): same size, half the encoder time on
  wide-datapath designs. An order-0 entropy probe was tried first and
  misclassified structured-but-high-entropy data.
* FST's dynamic aliasing is borrowed at block level: columns are hashed
  at block finish and byte-identical ones (verified, not trusted to the
  hash) are stored once. On SCR1 this removes another 19% of the
  compressed file and brings the uncompressed file within a few percent
  of uncompressed FST; zstd alone only catches duplicates that land in
  the same 64 KiB run.
* In-file skip indexes for long columns were tried and rejected: even
  delta-coded, they cost 10% of the file on SCR1. The reader instead
  builds a sparse index (one checkpoint per 256 entries) the first time
  it touches a long column and caches it with the decompressed run.

### 5.3 Writer pipeline

**Chosen**: the simulator thread appends 16-byte records to a log (plus
pre-encoded entries for wide vectors) and hands 512K-record chunks to a
background thread that counting-sorts them by signal and encodes column
fragments; blocks of 16M records are then concatenated per signal,
compressed and written.

* FST's writer maintains per-signal linked chains in a memory-mapped
  temp file on the caller's thread and compresses inline; libfstwriter
  keeps per-variable vectors. Both cost ~19 ns/change on the caller. VTR's
  log append costs ~10 ns/change and the rest moves off the simulator
  thread. With the background thread off, VTR is still at or below
  libfstwriter's time on every workload measured.
* Chunk size and block size are decoupled on purpose: small chunks give
  early pipelining (a 1.2M-change run finishes in 0.08 s instead of
  0.10 s), large blocks keep long columns for the compressor. Column
  length per block is what matters: SCR1 grows 58% with 1M-record blocks,
  and the 8-copy SCR1 trace (8x more changes per time step) shrank from
  36.5 MiB to 22.2 MiB when the block grew from 4M to 16M records, at a
  5% write-time cost. Fragments are compact, so a 16M-record block costs
  the encoder tens of MiB, not the 256 MiB the raw log would take.
* Duplicate suppression is on by default (FST's is a compile-time option
  that GTKWave does not enable). It is cheap because the writer keeps the
  current value of every signal anyway to build frames.

### 5.4 Reader

**Chosen**: memory-mapped, `Sync`, lazily decoding; one small LRU of
decompressed pieces; a global time table built on demand.

* `value_at` = binary search over block start times, dirty-index lookup,
  one run decompression, skip-index seek, short walk. wellen must load a
  signal from every block first; fstapi's rvat path inflates the block's
  time table, frame and position table.
* Streaming every change uses a single decode pass into
  self-describing 16-byte entries bucketed by coarse time, then an exact
  counting sort per bucket; this replaced a heap merge (0.9 s → 0.19 s on
  21M changes, versus 0.34 s for fst-reader).
* `load_signals` decompresses each run only once per block for any number
  of requested signals in it, and materialises wellen-style
  (`times`, packed data) results so a wavepeek backend is a thin adapter.

## 6. Transactions

**Chosen**: transactions carry begin/end, generator, status, kind, an
optional structural parent, typed attributes with FTR's begin/record/end
phases, point *events* (OpenTelemetry), sub-interval *stages* on lanes
(Kanata), and typed *relations* live separately. Blocks are ordered by
end time (as FTR) but carry id, time and relation ranges so lookups are
local; the contents are stored as 24 columns compressed as one blob.

* FTR stores each attribute as a CBOR array with a repeated type code and
  each transaction with absolute 64-bit times; measured 40 B per
  transaction after LZ4. VTR's columns delta-code ids and times and turn
  repeated keys/tags into near-zero-entropy streams: 12.7 B per
  transaction with 4.7 attributes and a relation on the TLM workload, 3x
  smaller than FTR-LZ4.
* Stages exist because Kanata's per-lane stage model is the dominant
  pipeline-viewer use case; encoding them as child transactions (the FTR
  way) costs an id, a relation and a second lookup per stage (5x larger
  files in the benchmark).
* Events exist because OpenTelemetry needs them and because a point in
  time with attributes is a different thing from an interval.
* The structural `parent` field is separate from relations: parent/child
  nesting is universal (OpenTelemetry, SCV, Kanata converters) and worth a
  column; arbitrary edges stay relations with attributes (links,
  dependencies, `pred`).
* Relations are stored where they are recorded, with per-block from/to
  ranges; a full reverse index was rejected as a write-time cost with no
  measured need (the per-block ranges make `relations_to` local for the
  common case of relations between temporally close transactions).
* Ids are assigned by the writer (dense, monotonic) so id lookups can
  use per-block ranges; producer ids are attributes. FTR's global counter
  works the same way; OpenTelemetry's 16+8-byte ids would defeat
  delta coding.

## 7. API

* **Rust first, C second, same shape.** The C API is a one-to-one
  projection of the Rust one (`vtr_writer_*`, `vtr_reader_*`) with opaque
  handles, integer status codes and a thread-local last error, so C++ and
  SystemC users need no Rust knowledge and can wrap it in RAII.
* **No global state**: FTR's backend singletons and FST's process-wide
  temp files were explicit anti-goals.
* **Values in, values out**: no ASCII round trips on the hot path; ASCII
  is offered for VPI-style simulators and for humans.
* **Pre-interned keys**: attribute keys, event, stage, lane and relation
  names are interned once and passed as ids, which keeps the transaction
  path allocation-free and hash-free per call.
* **Local queries**: the reader exposes exactly the query shapes wavepeek
  and pipeline viewers need (`value_at`, `changes`, `load_signals`,
  `for_each_change`, windowed transaction visits, relation lookups) and
  documents their cost model.
* **Small surface**: the Rust writer has 40 public methods, the reader 35;
  the C header has 82 functions. FSDB's public API is several hundred
  functions; fstapi has about 90 with many mode flags.

## 8. Things deliberately left out of the format

* Presentation and semantics (colours, roles, source locations): the KDB
  layer (`docs/KDB_APPNOTE.md`). Producers may record source stems as
  attributes when they have them, but nothing depends on it.
* Whole-file compression wrappers, sidecar files, in-band viewer state.
* A query language: VTR is a store; queries are code.

## 9. Known limitations and future work

* Block time tables cap at 2^31 entries per block; the writer would need
  to split blocks by time as well as by record count for runs with
  billions of distinct time steps between flushes (not reached by any
  benchmark; a 16M-record block cannot exceed 16M time steps unless
  `set_time` is called without changes, which is rare).
* Relations of transactions that are far apart in time may require
  visiting several blocks; a global relation index section is a
  backward-compatible addition (optional section kind) if a workload needs
  it.
* The VCD converter goes through wellen and is therefore not lossless for
  VHDL/GHW metadata; the FST path is.
* Multi-writer merging (several simulators into one file) is not
  addressed; separate files plus a KDB that references both is the
  intended pattern.
