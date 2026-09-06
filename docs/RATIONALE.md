# Design rationale

This document records what was researched, what VTR borrowed, what it
rejected, and why the format and API look the way they do. The research
notes behind it (feature inventories of FST, FTR, Kanata, OpenTelemetry and
of wavepeek's reader usage) were produced from the sources under `ext/`
and the public specifications; the resulting feature matrix is in
`docs/COVERAGE.md`.

## 1. Goals that shaped the design

1. One file for waveforms, transactions, hierarchy and relations, with one
   time base (the FSDB model, without the VDB).
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
* **NanoLog**, **binlog**, **Quill**, **CLP** (`ext/`): how binary loggers
  separate static call-site facts from dynamic values, pack and queue the
  values, and how CLP decomposes finished text into log types and variable
  dictionaries; the measurements are in section 7 and `BENCHMARK_RESULTS.md`.
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
  scope/var/direction numbering (so converters are trivial and VDBs can
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
  0.2 ms.
* Node ids inside the file are relative (distance to the parent, distance
  from the next signal id), which makes them one byte each, and the reader
  keeps nodes column-wise (kind, parent, name and two words per node, with
  attributes and enum tables in side tables) instead of as 72-byte structs.
  A 200k-variable design now opens in 6 ms instead of 13 ms, a constant
  that dominated every point query on such designs.

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

4. Per-run *value transforms* borrowed from columnar databases (Blosc's
   byte shuffle, BtrBlocks/DuckDB-style delta coding with sampled scheme
   selection): before a run is compressed, the writer trial-compresses a
   1 KiB sample of its fixed-width value streams under none / shuffle /
   delta / delta+shuffle with fast zstd and applies the winner to every
   eligible column of the run when it is at least 4% smaller. Measured on
   the final files: -14% on SCR1, -23% on its 8-copy replica, -21% on
   `long_sparse`, -10% on `many_active`, 0 on the high-entropy RSA-256
   and wide-bus data (where the trial keeps plain values). Per-column
   decisions were only 1-2% better than per-run ones and per-column
   estimators without trial compression (order-0 entropy, zero counts)
   lost most of the gain, so the run-level trial is the design. XOR-delta
   was also tried and rarely won against arithmetic delta.
5. 1-bit columns encode the *toggle* implicitly: an entry is `dt << 1`
   when the value flipped (the usual case for a two-state signal), and an
   escaped form carries an explicit logic code otherwise. This costs
   nothing to decode and saves 0.5% (SCR1) to 4% (`long_sparse`) over
   storing the bit.

6. Per-column *dictionary coding* (transform 4, added after the 2026
   literature review in `docs/SOTA_REVIEW_2026.md`): a column with at
   most 256 distinct values is stored as its dictionary plus one code byte
   per entry. It was first rejected on the assumption that zstd's match
   finder captures repeats within a run; a trial over the C910 value
   streams showed one-byte codes compress 3x better than the matches zstd
   finds in 2-23 byte entries. The sample trial is optimistic for it (the
   32-entry segments favour codes over the long-range matches zstd finds in
   plain or shuffled values), so the dictionary must win the trial by 20%:
   at that margin the file shrinks on every workload (C910 -2.6%, SCR1
   -0.6%, its 8-copy replica -0.2%). Columns are hashed whole only after a
   dictionary built from the sample alone already wins, which keeps the
   encoder cost within noise on designs where it does not apply. Decoding
   is a table lookup per entry, cheaper than undoing delta.

Rejected: bit-packing of narrow values (no gain after zstd); bit-plane
reorganisation, frame-of-reference and XOR/mask delta (measured on C910,
`docs/SOTA_REVIEW_2026.md`).

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
  the same 64 KiB run. The hash covers only the lengths, entry count and
  the first and last 64 bytes of each stream: a full hash cost 0.1 s per
  block on a 200k-signal design, and candidates are verified byte for
  byte anyway.
* Runs also hold at most 64 signals. In designs whose columns are tiny
  (200k signals with a few hundred changes each) a 64 KiB run spans 200
  columns, and loading 1000 random signals decompressed most of the file;
  the cap makes that query 1.8x faster at a 1% size cost on that
  workload and changes nothing elsewhere.
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
* The chunk grows with the design: at least 16 changes per declared
  signal per chunk. With 200k signals a 512K-record chunk held 2-3
  changes per signal, and assembling a block from 60 chunks of tiny
  fragments cost more than compressing it (0.16 s of 1.1 s); 16 changes
  per signal cut the block finish by half.
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
* Streaming every change uses the scheme of GTKWave's block iterator: a
  cursor per column and a linked list per time step (each cursor is
  re-linked at the time index of its next entry), so nothing is sorted
  and memory is proportional to columns plus time steps. It runs at ~7 ns
  per change on SCR1 (0.17 s for 21M changes, versus 0.37 s for
  fst-reader). With very many active columns the cursors no longer fit
  the cache and every visit misses; blocks with more than 32 Ki active
  columns are instead walked in windows of time steps sized so that each
  column visit yields at least 16 entries, and each window's 12-byte
  entries are counting-sorted by time index. A heap merge (4x slower) and
  a whole-block radix sort (O(changes) memory, DRAM-bound scatter) were
  the earlier designs.
* `load_signals` decompresses each run only once per block for any number
  of requested signals in it, and materialises wellen-style
  (`times`, packed data) results so a wavepeek backend is a thin adapter.
* Loaded histories are immutable `SignalData` handles over shared storage.
  The reader sorts and deduplicates requested signal IDs, decodes each unique
  history into a private builder, then returns handles in request order.
  Clones and repeated IDs share time, value, initial-value and offset buffers;
  the last handle releases them, independently of the reader's lifetime.
  Freezing retains the builder's vector allocations behind the private shared
  handle; shrinking them into boxed slices can copy entire histories.
  This targets read-only consumers and avoids decoding and allocating the
  same waveform repeatedly when several hierarchy names resolve to one ID.
  Public mutable vectors and cloning entire histories were rejected: they
  impose copying to support mutation that the reader does not need. A
  persistent history cache was also left out to avoid retaining unbounded
  waveform data. Dynamic aliases remain per-block encoding references;
  distinct signal IDs are not assumed to have identical complete histories.
  The C API projects batch loading and cheap handle cloning directly.
  Validation separates unique-signal loads from requests sampled with
  replacement (the existing load-N benchmark). See the `vtr-bench load`
  modes in `docs/BENCHMARKS.md`; published results require full-suite
  confirmation, including the Linux C910 toolchain.

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

## 7. Logs

Simulator text logs (UVM reports, `$display`, `LOG_INFO` macros) were the
last trace domain without a home in VTR. Four systems were studied
(`ext/NanoLog`, `ext/binlog`, `ext/quill`, `ext/clp`, fetched by
`bench/log/fetch_refs.sh`); the comparison harness is `bench/log/`, the numbers below are from `sim_log_1m` /
`sim_log_10m` on an Apple M5 (`docs/BENCHMARK_RESULTS.md`).

**What the references do.**

* *NanoLog* (Yang, Park, Ousterhout; ATC'18) extracts the static part of a
  log statement (format string, file, line, severity, argument types) at
  compile time (preprocessor) or first use (C++17 `constexpr` analysis),
  writes it once to the file as a dictionary fragment, and stores per message
  a 1-4 byte format id, a packed TSC delta and the arguments packed to their
  minimal byte width (a 4-bit "nibble" per integer says how many bytes
  follow). A background thread compacts and writes with POSIX AIO. Hot path
  ~7 ns; no general compression; strings verbatim. Linux/x86-64 only.
* *binlog* (Morgan Stanley) generalises the idea without a preprocessor: an
  `EventSource` entry (severity, category, function, file, line, format,
  mserialize type tags) is written lazily on first use of each call site, and
  an event is `u32 size, u64 source id, u64 clock, arguments` copied at
  native width into a per-thread SPSC queue that a consumer drains to any
  stream. Self-describing, any clock, no compression (52 bytes per message
  on the benchmark before zstd), text via `bread`.
* *Quill* keeps the same static-metadata idea (`MacroMetadata` per call
  site, arguments encoded by `Codec<T>` into a thread-local SPSC queue) but
  its backend formats text: the output is a text file. The hot path is the
  fastest of the four here (27 ns for the whole loop) but the backend then
  needs 6x longer than the loop to format and write 66 bytes per message.
* *CLP* (Rodrigues, Luo, Stumm; OSDI'21) starts from finished text: it
  tokenises each message into a *logtype* (the message with variables
  replaced by placeholders), *dictionary variables* (tokens that are not
  plain numbers) and *encoded variables* (integers and decimals kept as
  64-bit values), stores the dictionaries once and the per-message ids and
  values in columns compressed with zstd, and searches the compressed
  archive without decompressing it. Its IR stream is the same decomposition
  per message plus zstd, used by its logging-library plugins.

The common core is one idea: separate the static call site from the dynamic
values, store the static part once and the values raw. CLP adds the second
idea that matters for simulator logs: repeated string variables belong in a
dictionary and numbers should stay numbers. VTR already had the structures
for both (generators with attributes, transactions, string tables, columnar
blocks), so log support is a specialised encoding of a transaction, not a
new data model.

**Chosen.**

* A *log site* is a generator of a `LOG` stream: name = format string,
  attributes `log.severity`, `log.args` (types), `log.names`, `log.file`,
  `log.line`, `log.func`. Registration is lazy on the C++ side (a static per
  call site, as binlog and Quill do) and explicit on the Rust side; the
  hierarchy chunk that declares a site is flushed before the first block
  that uses it, which the existing writer already guaranteed for streams
  and generators.
* A *log record* is a zero-duration transaction: it takes the next
  transaction id, may have a parent transaction and can be a relation
  endpoint, and every existing consumer (`visit_transactions`, `vtr tx`,
  VDB viewers) sees it without new code. Its encoding is its own section
  kind, `LOG_BLOCK`, because a generic transaction row spends most of its
  bytes on things a log record does not need (attribute keys and tags,
  duration, status, event and stage counts). Writing the benchmark's
  messages as ordinary transactions with one attribute per argument costs
  11.9 MB and 71 ns per message on the caller's thread; the log block costs
  9.6 MB and 28 ns (`vtr-bench log-write --as-tx`).
* Columns: site index, time delta, id delta, parent, then one column per
  value class (integers as LEB128, floats as 8 bytes, text as dictionary
  indexes, str ids and bytes as they are), the whole set compressed with the
  file codec. The per-block *text dictionary* is CLP's variable dictionary
  built at encode time: the hot path copies the string, the encoder hashes
  it once per block, and a repeated component name costs one byte. Global
  interning (`Str`) stays available for producers that intern themselves;
  it was not made the default because it puts a hash lookup on the hot path
  and grows the string table every reader loads at open.
* A new value tag, `text` (17), for one-off UTF-8 strings that are not
  interned: log arguments need it when a record is read as a transaction,
  and converters can use it for unique attribute strings that today bloat
  the string table. Format version 1.1; a 1.0 reader skips log blocks
  (optional flag) and rejects tag 17.
* The hot path is a memcpy: the C++ header encodes the arguments on the
  stack from compile-time known types and calls `vtr_writer_log_raw`; the
  Rust API validates the types against the site and writes the same bytes.
  27-31 ns per message for three arguments, versus 23 ns for NanoLog, 27 ns
  for Quill and 40 ns for binlog on the same machine.
* Log blocks are encoded on helper threads (`log_encoders`, default 2)
  because splitting rows into columns plus zstd costs about 60 ns per
  message, twice the hot path; with two encoders the total (loop plus
  flush) is 38 ms per million messages against 66 ms with one. Blocks are
  written in production order so a monotonic log reads back in order.

**Measured** (1M / 10M messages, best of 3): file size VTR 9.6 MB / 95.7 MB;
NanoLog 26.2 / 267 MB; binlog 52 / 520 MB (14.3 MB after zstd at 1M);
Quill and text 66 / 672 MB (16.8 MB after zstd); CLP IR + zstd 14.6 /
146 MB. Loop plus flush per million: VTR 0.038 s, NanoLog 0.023 s, binlog
0.041 s, Quill 0.185 s, text 0.143 s, CLP 0.375 s (of which 0.12 s is the
text formatting it starts from). Reading back to text: VTR 10 M lines/s,
CLP 14 M lines/s, bread 2.6 M lines/s, NanoLog's decompressor 1.4 M lines/s.
The text rendered from the VTR file is byte-identical to the fprintf log.
So VTR is the smallest by 1.5x over CLP and 2.7x over NanoLog, within 1.7x
of NanoLog's total time and faster than the rest, and second only to CLP
when rendering text.

**Rejected or bounded.**

* Formatted text as a `bytes`/`text` attribute (the first proposal): the
  text log compressed with zstd is 16.8 bytes per message against 9.6 for
  the decomposed record, and every message would carry a formatting call on
  the simulator's thread (the text baseline's loop is 143 ms per million,
  five times the log call).
* A variable-length string signal (the FST/VCD `$display` idiom): flat text,
  no severity or per-site structure, and the writer-wide value dedup drops an
  identical repeated line.
* Compressing rows without transposition (what CLP's IR stream is, one
  message after another): 14.6 MB versus 9.6 MB for the same data; the
  columns are worth 35%.
* zstd level 1 instead of 3 for log blocks: 40 ms instead of 61 ms of
  encoder time per million but 10.05 MB instead of 9.52 MB (`vtr-bench
  log-encode`); LZ4: 34 ms and 12.5 MB. With two encoder threads level 3
  already keeps up with the producer, so the file codec is used unchanged.
* A faster dictionary hasher: replacing SipHash by a multiply-rotate hash
  changed the column split from 32 to 29 ns per message; the cost is in the
  varint decode/encode and column appends, not the hashing. Kept because
  it is free.
* Pre-parsing format strings once per site instead of per record took the
  read-back from 4.5 to 10 M lines/s.

**Known limitations.** The format-string subset is the intersection of
Rust and C++ `std::format` (no named arguments, no `%`-style specs; binlog's
plain `{}` is a subset of it). Records from several producer threads would
have to be merged by the caller (the writer is single-threaded, as for
signals and transactions). There is no full-text index; a search is a
`visit_log` with a filter, which the block headers prune by site and time
but not by argument value.

## 8. API

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

* **Simulator integration as a port, not a new path** (`integrations/verilator`):
  the Verilator backend is a line-for-line port of Verilator's own FST
  backend onto the C API, so the generated trace code, the hierarchy
  names (`name[index]`, `name [msb:lsb]`), aliases and enum tables are
  identical between `--trace-fst` and `--trace-vtr`, and a file written by
  one can be checked against the other (the benchmark's `vs_sim` parity
  run). The only VTR-specific choice is to switch the writer's own value
  deduplication off, because Verilator's generated code already emits
  only changed values.

## 9. Things deliberately left out of the format

* Presentation and semantics (colours, roles, source locations): the VDB
  layer (`docs/VDB_APPNOTE.md`). Producers may record source stems as
  attributes when they have them, but nothing depends on it.
* Whole-file compression wrappers, sidecar files, in-band viewer state.
* A query language: VTR is a store; queries are code.

## 10. Known limitations and future work

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
  addressed; separate files plus a VDB that references both is the
  intended pattern.
* `docs/SOTA_REVIEW_2026.md` ranks the remaining ideas from the 2025-2026
  literature against measurements on the benchmark files (dictionary
  transform, second encoder thread, narrowest-packing loads).

## 11. RTL VDB companion

VDB means Vibe Data Base. The companion crate and CLI use `vtr-vdb`,
the exporter emits `vtr-rtl-vdb` version 2 in `.vdb.json` files, and trace
identity uses `design.vdb_id`. This naming change leaves the VTR binary
format and the companion schema structure unchanged. Re-export existing
design databases to use the new identifier and matching design fingerprint;
the reader rejects other format identifiers explicitly.

The independent [RTL VDB](VDB_RTL.md) borrows resolved symbols, context-sized
expressions, parameter specialization and port connectivity from slang 11.0.0
([manual](https://www.sv-lang.com/user-manual.html),
[Python bindings](https://www.sv-lang.com/building.html)). A pinned Python
exporter keeps the C++ frontend out of the Rust runtime and exports a small,
versioned semantic IR. A source-text/regex parser was rejected because it would
lose elaboration and assignment sizing. Directly consuming slang AST JSON was
rejected in favor of an explicit application schema independent of pointer IDs.

Temporal tracing searches triggering events, not only register value changes:
an enabled assignment can write the same value, and pipeline stages sample
pre-edge values. Explicit execution order handles blocking temporaries and
last-write NBA semantics. Unsupported semantics and missing scheduler detail
remain diagnostics. Exact paths, widths, optional module names and an optional
producer design identity prevent speculative suffix-based attachment. No
reader decode, writer, or encoding changes are needed; no efficiency claims
or benchmark result changes accompany this companion.


### Hierarchical netlist views

VDB v1 synthesized port assignments for temporal tracing but omitted module
ownership and top/unconnected ports. Guessing ownership from targets is wrong
for child outputs, cross-module references, and generate scopes. VDB v2 records
slang's containing module explicitly, complete ordered ports, and separate RTL
versus connection process origins. Static reads survive unsupported statement
bodies, so opaque processes retain their input connectivity. Replication stays
compact. Net declaration initializers are continuous drivers.

A reusable ownership index builds only the selected module's view; immediate
children stay opaque. A regression adds 2,000 descendants and requires identical
parent geometry. Per-instance semantics remain in the export; module-definition
deduplication is not claimed. Single assignments lower to operator graphs while
procedural blocks retain their ordered RTL semantics rather than approximating
synthesis. Recorded signal values annotate the resulting graph independently of
layout; missing samples stay diagnostic.

The [elkrs fork](https://github.com/ripopov/elkrs), pinned as `ext/elkrs`, provides
native layered layout with fixed ports and orthogonal routes. The local
`gpui-schem` implementation informed fixed-port placement and preserving
application metadata outside the layout engine. We keep separate typed netlist
metadata and ELK coordinates rather than relying on unknown JSON fields surviving
ELK serialization. SVG snapshots, geometry invariants, and raster review check
our integration independently of elkrs's unavailable upstream golden corpus.
No VTR writer, encoding, or decode-path changes or performance claims accompany
this use case; benchmark results are unchanged by this work.

### Native Verilator VDB export

`--trace-vtr` now exports the same RTL VDB v2 domain directly from Verilator's
elaborated AST. It reuses resolved parameter/type information and the native
cell hierarchy instead of re-elaborating with a second frontend, parsing emitted
C++, or inferring drivers from optimized traces. Export runs immediately after
`V3Width`, before `V3WidthCommit`: the latter removes `always @*` sensitivity
information. Exporting after commitment was tested and rejected because such
processes became indistinguishable from unsupported bare `always` processes.
Generated-loop localparams are retained explicitly for source browsing.

A module index separates declarations from per-instance export and resolves
relative/absolute hierarchical references against elaborated symbol paths.
Unsupported bodies retain static reads and targets. Native normalized selects
remain explicit; width-generated truncations use the existing conversion IR.
The runtime writes the embedded document alongside the trace and appends actual
trace-declaration mappings. This avoids a dependency on build-directory files
and accommodates aliases, wrapper names and filtering. Both files carry the
same design hash. Recording bindings require identity rather than accepting
structural-only attachment. Slang remains an independent standalone adapter.

The integration suite uses actual Verilator waveforms to validate source paths,
netlists and temporal provenance, including an enabled pipeline with a held
edge, resets, signed/context-sized operations, generate/instance arrays and
unsupported-case diagnostics. The pre-existing VTR/FST smoke test still agrees
with its expected samples. This changes compile/open-time companion metadata;
VTR encoding and per-dump value emission are unchanged. No performance or size
improvement is claimed, and benchmark results are not updated.

### Surfer VTR/VDB attachment

Surfer builds the shared Wellen hierarchy and signal store through its typed
builder/encoder APIs. The prototype's full VCD text serialization and second
parse were removed; directions, components, aliases, packed ranges and enum
translations now enter the same model used by FST directly. Verilator's
unpadded enum values are extended to the signal width before declaration.
Waveforms and transactions use a format-independent combined document variant.
The initial adapter materialized waveform data while loading. The native
signal backend now loads only requested immutable signal histories, retaining
Wellen hierarchy identities and the shared renderer. Canonical demand from
visible tabs controls eviction; late results are filtered against current
demand. FST export explicitly converts only selected loaded histories.
Transactions open as metadata and load on background workers for visible
generator/time-window requests and selected inspector records. Overlapping
requests merge, stale results are rejected, and reader caches are released
after each batch. Hidden waveform items release analog caches; hidden tiles
release drawing, memory and framebuffer caches. No performance claim is made.

Companion selection uses the exact trace stem (`.vdb`, then `.vdb.json`). A
mismatched preferred file is diagnosed rather than bypassed. Validation and
source indexing happen with the recording's reader on the load worker, and
the source index is owned by the resulting waveform document. This fixes the
prototype's separate global index, which could become stale and was populated
in a loading branch local VTR files did not use. Source resolution follows the
explicit binding and alias identity, not hierarchical suffix guesses. Tile
state saves a location; source text and initial-scroll tracking remain a
disposable cache.

Tests consume committed examples in `ext/surfer/examples`, including two real
Verilator designs recorded separately as VTR and FST with identical stimulus.
They compare all transitions and hierarchy metadata, use shared image goldens,
and exercise context-menu navigation and document replacement. No VTR format,
writer encoding or core reader decode path changes are involved.

### Explicit reader cache eviction

Surfer's lazy-loading work needs to release decoded transaction blocks as well
as its own displayed results. Dropping viewer results alone is insufficient:
the reader's per-block caches retain decoded transactions and logs.
`Reader::clear_cache(&mut self)` and its C projection release decoded caches,
keeping the mapping and metadata. Exclusive access allows resetting the locks
without adding synchronization to queries. Owned shared histories and block
time tables survive eviction. Existing decoding and cache lookup paths are
unchanged; subsequent queries repopulate caches normally. This is a lifetime
control API, with no encoding change or measured speedup claim.

### Surfer schematic canvas

Surfer consumes the module-local VDB netlist and runs elkrs in a background
worker with node sizes chosen for its interactive canvas. The gpui-schem
prototype informed pointer-centered zoom, panning and object selection; its
images are not reference snapshots. Surfer's PNG tests use its own renderer
and committed Verilator examples. Typed optional block source locations keep
navigation independent of human-readable labels. The default `layout` feature
in vtr-vdb is optional so consumers can supply their own layout dependency.
This changes neither the VTR nor the VDB serialized format and makes no
compression, throughput or memory performance claim.

## Verilator log capture and virtualized Surfer browsing

The integration reuses log sites and LOG_BLOCK rather than encoding messages as
string-valued waveform signals or inventing a second log format. One stream and
one generator per severity satisfy the simulator grouping requirement. A single
Text argument preserves Verilator's SystemVerilog-specific formatting and file
output exactly; extracting typed arguments at every HDL call site would create
call-site generators rather than severity generators and duplicate its formatter.
This adapter therefore pays formatting cost on the simulation thread, unlike
VTR's structured C++ logging macros. No encoding/reader decode path changed.

Severity is carried explicitly through diagnostic lowering, not guessed from
message prefixes. In VTR mode the compiler preserves reporting-call boundaries
rather than merging adjacent writes. The context dispatches timestamped records
on the main evaluation thread to trace-owned sinks. Trace opening/closing owns
registration, and worker messages capture time before dispatch.

The Surfer tile borrows its fixed-height viewport virtualization from egui and
case-insensitive fuzzy matching from the existing Skim matcher dependency.
An immutable per-recording index amortizes decode/format/sort across tiles and
queries. This retains full formatted text in memory; it is not a bounded-memory
or lazy log-text reader. A time interval is found by binary search, filtering
runs in a cancellable worker, and rendering uses only visible matching rows.
Outdated workers cannot replace newer query or recording results. This is a
viewer architecture change, with no claims of improved VTR encoding performance.

The simulator regression also exercises binary HDL output (retained as a
reversible hex string when it is not UTF-8), runtime reporting helpers, trace
reentry warnings and context ownership. Reentrant trace warnings release the
trace lock before invoking a log sink. Measurements and validation limits are
recorded in [the integration notes](../integrations/verilator/logs/README.md).
