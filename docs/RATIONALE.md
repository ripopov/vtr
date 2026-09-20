# Design rationale

## Repository organization

The shared VTR and VDB libraries live under `core/`, the trace CLI under
`tools/`, Rust benchmark drivers under `bench/`, and the viewer under `volna/`.
Standalone pyslang export and simulator/consumer glue belong under
`integrations/`. One workspace and lockfile keep dependencies consistent;
non-GUI default members keep platform SDKs out of the default build.

Libraries are grouped under `core/` by their shared role as foundations for
tools, viewers and integrations. Consumers, including Surfer, reference these
canonical paths directly.
See the [architecture guide](ARCHITECTURE.md) for boundaries and integration status.

## Volna hierarchy browser

Scopes and streams share the existing container tree; variables and generators
share a member list. This follows the separate-pane pattern examined in Surfer
and preserves the core's virtualized row producers. gpui-kit's `TreeState` would
duplicate core expansion, selection and reveal state, so frontends retain their
uniform lists. Generators stay out of the tree to keep large transactors shallow.

Stream kind lives once in `Scope::kind`; `ScopeRole` adds only its raw track
identity. Log-site descriptions are borrowed views over raw attributes, avoiding
a presentation schema in the protocol. Core icon mappings use bundled Lucide
assets. Search combines variables, generators and streams under one cap, and
empty selection activates only variables, avoiding accidental mass transaction
loads. Transaction activation reports a notice until the corresponding panel
exists. Iterative flattening supports deep trees without recursion. The metadata
extension uses remote protocol version 2; VTR encodings and C/Rust reader APIs
are unchanged.

## Volna pipeline panel

The panel is one `PanelKind` next to waves and settings, not a viewer mode: the
dock, focus order, links, markers and workspace files already handle kinds. Its
time axis is the document's shared `Viewport` because the Kanata importer and
the showcase write cycles as time units, so linking with the wave panels costs
nothing and needs no cycle-to-tick mapping until a trace with two time bases
exists. Rows are a second, local axis (`RowView`) with the same zoom-about,
pan, edge-space clamp and eased `Tween` as the time axis; one wheel gesture
applies one factor to both, but a linked wave panel changing the shared time
axis leaves row heights alone, so cells are square by default, not by
invariant. Locking both axes (Konata) would either break the link or resize
rows behind the user's back; a wheel that scrolls rows and zooms only with a
modifier (Konata's default) would conflict with the wave panel's wheel.

Data is the document's loaded track: rows index the resident
`LoadedGenerator` slices through prefix sums and the panel retains and
releases the track like a consumer, so splits share one load and closing the
last panel frees it; no second index, cache or wire format was added. Painting
indexes rows directly; follow activity queries the resident interval tree to
retain long overlaps and point transactions without a second index. The stream
kind is never required: any generator has stages and lifetimes, and gating on
`PIPELINE` would refuse gem5 or SystemC generators that render as well.
Painting walks only the visible rows and skips stages outside the window;
below two pixels per row it paints density steps so the quad count is bounded
by pixels, which is what keeps ten thousand rows fluid without a row index.
The stage palette is a ladder by first appearance so a trace needs no VDB to
be readable; a VDB stage table fills the same struct later.

Times are shown in the producer's named unit when the file declares one
(`time.unit`), carried in `TraceInfo` and the remote metadata (protocol
version 3): a pipeline whose cursor reads `476 s` for cycle 476 is wrong in a
way no theming fixes. Embedding Konata's renderer was rejected because it
would put viewer logic outside the toolkit-free core and would not run in the
native window.

Follow activity borrows the separate time link and local row policy from
[`follow-activity.html`](follow-activity.html). The visible effective cursor
anchors the query, falling back to the viewport center; intersecting lifetimes
nearest that anchor win, with ties resolved toward the current row center.
The middle 60% is a safe band: a candidate already there causes no movement.
Idle windows retain rows and show the direction of activity. Following changes
only row top, never height, cursor or shared time. Immediate row placement
avoids a second lagging animation while shared time itself animates.
Vertical pan, row keys and two-axis zoom suspend following; horizontal input
does not. Explicit resume and one-shot edge navigation avoid silently taking
control back after manual inspection. Follow state is saved per panel and
copied on split. Native toolbar equivalents expose the canvas edge actions to
keyboard and accessibility users. Protocols and VTR/VDB data stay unchanged.

## Volna baseline table panel

The first table release implements the reduced contract in
[`table-baseline.html`](table-baseline.html): one transaction generator, or one
fixed ordered set of signals. It deliberately does not revive the archived
full-table prototype's generic row domains, filtering, sorting, search,
relations, computed columns or multiple-generator merge. Those abstractions had
no second implemented use case and obscured the ownership boundary. A durable
`TableSource` stores declaration paths; resolved IDs, selection, viewport and
prepared text remain session state. The reduced payload is table panel version
2, so version-1 full-prototype workspaces remain explicit unsupported panels.

Rows continue to live in their immutable owners. Generator ordinals index the
resident `LoadedGenerator`; one signal uses its history directly. Multiple
signals admit one exact 8-byte-per-distinct-time merged axis, built by a heap in
bounded cooperative slices. The panel prepares only the visible rows plus two
rows of overscan on each side, capped at 256. Transaction identity is the
writer's ID and signal identity is the exact timestamp, so hiding columns or
moving the viewport cannot change selection. Scrollbar normalization is visual;
the logical row is recovered with `u128` integer arithmetic, including values
above JavaScript's exact-number range.

The design follows the existing wave and pipeline ownership model rather than
adding a table cache. Local VTR/FST session images, decoded histories and loaded
generators now reserve the same session ledger already used by remote owners;
each table reserves 4 MiB for its window/details/clipboard work, and a
multi-signal axis adds its declared storage. Repeated panels share raw `Arc`
owners. Admission failure publishes no partial axis, Retry re-enters admission,
and cancellation drops private builders and reservations. A separate native
table-only cap was rejected because it could admit a table after raw data had
already exhausted the process budget.

Details are a separate superseding request, limited by `table.detailItems` and
the panel byte reservation; closing the inspector or selecting another row
invalidates the pending identity. Copy uses fixed standard fields rather than
visible columns, refuses rows above 64 KiB before touching the host clipboard,
and keeps the complete TSV in a selectable browser dialog with Retry when a web
or VS Code host rejects the gesture. The GPUI layer owns controls, clipboard and
synthetic accessibility nodes; source semantics, navigation, preparation,
painting and persistence remain in `volna-core`. Adding the same surface to the
minimal egui frontend was rejected by the repository's frontend direction.

## Volna FST session integration

Volna uses Session and immutable SignalHistory interfaces with batched loads
and a private fst-reader adapter. The converter's numeric FST scope,
variable and direction mappings are reused through existing type names, without
depending on the CLI. Wellen's filtered batch reads and shared alias identities
inform the loading design; its global time table and waveform storage are not
the common contract. VTR histories keep their existing shared buffers.

FST ports are EVCD payloads, not ordinary bit vectors: the reference reader
reports logical width after removing strength fields, while callbacks retain the full payload. Preserve those bytes
and display escaped text rather than guessing a width from names or dropping
strengths. Event histories use a distinct shape and pixel-bounded point-marker
painting, without synthesizing pulse durations or held values.

Complete-track loading is the common transaction contract for local and remote
consumers. Capability flags distinguish unsupported sources from supported but
empty recordings. Resident `LoadedGenerator` indexes answer overlap and identity
queries after loading; exposing a separate blocking reader query facet would
permit local-only viewer behavior and duplicate the loading model. Resolved names
and typed values avoid exporting reader string-table handles. Backend extraction
is shared by native sessions and the headless service; only the latter converts
objects to the wire schema. Serialization borrows loaded transaction records and
relations rather than cloning their nested attributes into another owned payload.
The browser's incremental decoder still owns its private construction buffers
and performs wire validation before publication.

The remote direction uses complete selected signals and transaction tracks,
as described in the [simple client-server design](client-server-simple.html).
This adopts Surver's whole-signal loading boundary and accepts client memory
proportional to selected data. Reader caches are not bounded by the viewport.
Complete track objects retain immutable per-generator stores, parent-owner
references and incident relations. Relation file-order ordinals preserve
parallel edges without a second identity table. A max-end tree over sorted
transactions answers local overlap queries without losing long intervals.
`transaction_generator` and its batch counterpart `transaction_generators`
expose ownership at the reader layer without cloning transaction details merely
to resolve unloaded endpoints. Whole-track loading gathers incident endpoints
and resolves external owners in one batch. Repeating a scalar lookup per edge
rescans block headers and rows for every external reference, even when the
decoded blocks are already cached. Batching avoids that repeated traversal
without adding a permanent ownership cache.
The [TLM batch-owner A/B samples](../volna/volna/benchmarks/client-server-owners.jsonl)
and [current remote load costs](../volna/volna/VERIFICATION.md#complete-object-child-protocol)
record the measured effects.

The raw transport uses fixed little-endian bincode fields inside independently
checksummed LZ4 frames. Frames have an explicit protocol version and a 1 MiB
size limit; data chunks carry at most 256 KiB and use 64 KiB LZ4 blocks. The
sender waits for an acknowledgement before sending another frame. Independent
frames let a browser consume and yield between chunks without retaining an
entire compressed object. This is transport fragmentation, not time-window
pagination. An object becomes complete only after its declared byte count and
explicit end marker agree. Short frames, excess output, identity/sequence
mismatches and duplicated results fail the request. The LZ4 frame layout is
validated before decompression because the library accepts EOF at a block
boundary without requiring the end marker; that permissive behavior is not
appropriate for atomic object delivery.
Histories and chunks use bincode's bulk-byte serializer. With fixed-width
options it emits the same length-prefixed byte representation as `Vec<u8>`,
but avoids a serializer and write call for every byte. The RSA transfer A/B in
the verification guide measures the effect; the wire schema is unchanged.
The supported subset and limitations are in the
[viewer guide](../volna/volna/README.md); test coverage and commands are in the
[verification guide](../volna/volna/VERIFICATION.md).

This document explains the format and API decisions, their supporting
measurements, and the tradeoffs of alternative designs. It draws on FST, FTR,
Kanata, OpenTelemetry and wavepeek's reader usage through the sources under
`ext/` and public specifications. See `docs/COVERAGE.md` for the feature matrix.

## 1. Design goals

1. One file for waveforms, transactions, hierarchy and relations, with one
   time base (the FSDB model, without the VDB).
2. Beat FST and FTR on size, write speed and read/navigation speed on
   realistic workloads, and prove it with a reproducible benchmark.
3. Local queries must not read the whole file; opening must be cheap.
4. Small, obvious APIs in Rust and C; no global state; a writer that a
   simulator can call at tens of millions of changes per second.
5. Versioned and extensible from day one; readable after a crash.

## 2. Design references

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
  Measurements on a 200k-variable design give 6 ms opening time for column
  storage versus 13 ms for structs; opening contributes to every point query.

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

6. Per-column *dictionary coding* (transform 4; see the analysis in
   `docs/SOTA_REVIEW_2026.md`): a column with at
   most 256 distinct values is stored as its dictionary plus one code byte
   per entry. Although zstd's match finder captures repeats within a run,
   trials over the C910 value streams show one-byte codes compress 3x better than the matches zstd
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
optional structural parent, typed attributes with keys unique within each
item, point *events* (OpenTelemetry), sub-interval *stages* on lanes
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
* A transaction carries no name field, because a name column would repeat
  the generator's name on every row of that generator; the generator
  indirection already removes that redundancy, and FTR makes the same
  choice. Per-instance names (a Konata instruction mnemonic, a packet tag)
  are the reserved attribute `vtr.label` instead: producers that have one
  pay an interned string id per transaction, producers that do not pay
  nothing. Reserving the key rather than leaving it a convention is what
  lets a viewer caption a bar without a VDB; it stays a name, not styling,
  so the [VTR/VDB split](../README.md#vtrvdb-boundary) holds.
* `vtr.label` is prefixed rather than the bare `label` the Kanata converter
  used to write, because the importers pass foreign attribute keys through
  verbatim (`otlp.rs` interns an OTLP span attribute key as-is, as does the
  FTR importer) and only prefix VTR's own additions. A bare reservation
  would silently give caption semantics to any imported span attribute that
  happens to be called `label`. Keeping reservation a pure prefix rule also
  keeps the producer's invariant to one sentence — anything without a
  reserved prefix is yours — instead of a prefix rule plus a growing list
  of bare words. It is `label`, not `name`, because everything else VTR
  calls a name is stable identity that a VDB binds to; a per-instance
  caption is not, and an OTel span's own `name` is already the generator's.
* FTR's begin/record/end attribute phase was removed. A phase did not identify
  a separate value semantically and allowed ambiguous duplicate keys, while a
  list or map expresses an intentional multi-value attribute directly. The FTR
  converter keeps the first source key unchanged and adds `.begin`, `.record`
  or `.end` only when folding phases creates an actual collision. The file's
  attribute-tag byte now spends only its low five bits on the value tag and
  reserves the upper three bits.
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

* Presentation and design semantics (colours, roles, design source mappings,
  specialized types and driver/load relationships): the VDB layer
  (`docs/VDB_APPNOTE.md`). Log-site provenance (`log.file`, `log.line`,
  `log.func`) belongs in VTR so runtime messages retain their origin.
  Producers may also preserve raw source-stem attributes; the VDB interprets
  them as design source mappings.
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

VDB means Volna Data Base. The companion crate and CLI use `vtr-vdb`,
the exporter emits `vtr-rtl-vdb` version 2 in `.vdb.json` files, and trace
identity uses `design.vdb_id`. The reader rejects other format identifiers
explicitly; each design database must carry a matching design fingerprint.

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
producer design identity prevent speculative suffix-based attachment.

### Hierarchical netlist views

Guessing ownership from targets is wrong for child outputs, cross-module
references, and generate scopes. VDB v2 records
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

### Native Verilator VDB export

`--trace-vtr` exports the same RTL VDB v2 domain directly from Verilator's
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
unsupported-case diagnostics. The VTR/FST smoke test independently checks
recorded samples against expected values.

### Surfer VTR/VDB attachment

Surfer builds the shared Wellen hierarchy and signal store through its typed
builder/encoder APIs. Directions, components, aliases, packed ranges and enum
translations enter the same model used by FST directly, avoiding full VCD text
serialization and a second parse. Verilator's
unpadded enum values are extended to the signal width before declaration.
Waveforms and transactions use a format-independent combined document variant.
The native signal backend loads only requested immutable signal histories, retaining
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
the source index is owned by the resulting waveform document, preventing a
global index from becoming stale when a recording is replaced. Source resolution follows the
explicit binding and alias identity, not hierarchical suffix guesses. Tile
state saves a location; source text and initial-scroll tracking remain a
disposable cache.

Tests consume committed examples in `ext/surfer/examples`, including two real
Verilator designs recorded separately as VTR and FST with identical stimulus.
They compare all transitions and hierarchy metadata, use shared image goldens,
and exercise context-menu navigation and document replacement.

### Explicit reader cache eviction

Surfer's lazy-loading work needs to release decoded transaction blocks as well
as its own displayed results. Dropping viewer results alone is insufficient:
the reader's per-block caches retain decoded transactions and logs.
`Reader::clear_cache(&mut self)` and its C projection release decoded caches,
keeping the mapping and metadata. Exclusive access allows resetting the locks
without adding synchronization to queries. Owned shared histories and block
time tables survive eviction; subsequent queries repopulate caches normally.

### Surfer schematic canvas

Surfer consumes the module-local VDB netlist and runs elkrs in a background
worker with node sizes chosen for its interactive canvas. The gpui-schem
prototype informed pointer-centered zoom, panning and object selection; its
images are not reference snapshots. Surfer's PNG tests use its own renderer
and committed Verilator examples. Typed optional block source locations keep
navigation independent of human-readable labels. The default `layout` feature
in vtr-vdb is optional so consumers can supply their own layout dependency.

## Verilator log capture and virtualized Surfer browsing

The integration reuses log sites and LOG_BLOCK rather than encoding messages as
string-valued waveform signals or inventing a second log format. One stream and
one generator per severity satisfy the simulator grouping requirement. A single
Text argument preserves Verilator's SystemVerilog-specific formatting and file
output exactly; extracting typed arguments at every HDL call site would create
call-site generators rather than severity generators and duplicate its formatter.
This adapter therefore pays formatting cost on the simulation thread, unlike
VTR's structured C++ logging macros.

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
Outdated workers cannot replace newer query or recording results.

The simulator regression also exercises binary HDL output (retained as a
reversible hex string when it is not UTF-8), runtime reporting helpers, trace
reentry warnings and context ownership. Reentrant trace warnings release the
trace lock before invoking a log sink. Coverage and verification commands are
in [the integration guide](../integrations/verilator/logs/README.md).

## CHI NoC packet visualization fixture

Surfer's `examples/chi_noc.vtr` reuses the existing transaction and timed-event
model: one VTR transaction is one single-flit packet; visited routers are events
on that transaction, rather than duplicate per-router transactions. Controller
streams and opcode generators follow the existing Surfer transaction canvas
adapter. Arm's [Introducing AMBA CHI](https://documentation-service.arm.com/static/682600710aae2a5d8f044978)
informs the channel roles and naming. This is synthetic independent packet
traffic, not a protocol compliance simulator or a complete coherence exchange.
The deterministic XY mesh has six controllers, each reaching 64 overlapping
packets. A sparse burst in the same recording makes a two-stream PNG regression
readable. Topology and packet metadata contain no presentation attributes.
See [the fixture notes](../ext/surfer/examples/chi_noc.md) for regeneration,
validation and modeling boundaries. The canvas derives row assignments per
displayed stream or generator, sorted by start time and identity, using the first
available row. This keeps presentation state outside immutable reader results.

## Surfer transaction event markers

Surfer renders existing VTR point events as hollow dots on their containing
transaction bars. It reuses the viewport timestamp conversion and transaction
hover/selection path; no event stream, signal, or presentation data is added to
VTR. Cached positions retain event indices into immutable document details.
Events sharing a rounded canvas pixel share a marker but retain all hover
entries. Offscreen events are omitted instead of clamped into misleading edge
markers. Endpoint events are included; events outside the containing lifetime
are omitted. The CHI and mixed waveform snapshots exercise the result, with
geometry and hover/click regression checks.

## Stable Surfer transaction geometry and screen-space sampling

Viewport-local row assignment would move packets while panning, and binary
search over arbitrary completion times can miss long overlapping transactions.
Surfer indexes complete stream and
generator geometry separately, sorting by start time and ID and assigning rows
with active-end and free-row heaps. Draw-cache identities include the displayed
track, and vertical scrolling invalidates sampled geometry. Mixed canvases use
the transaction row height rather than the signal row height.

Per-row nonoverlap makes interval searches valid. Queries jump over transactions
within each screen bin, retain aggregate counts and the last member’s end, and
visit only visible rows. Native payload requests follow individual screen
samples and inspector selection; panning does not rescan complete-track
geometry. Keyboard navigation uses indexed chronological neighbors rather than
enumerating loaded payload IDs, preserving navigation through dense windows. Legacy FTR indexes reuse its loaded streams. Rich details stay outside
the geometry index. Initial geometry construction and the reference reader’s
transient decoded-block cache are still linear in recording data; no bounded
initial-memory or billion-record-file loading claim is made.

The one-million-record, same-process debug-profile timing comparison measured
91.1 ms to construct geometry, 573.5 ms for per-frame row packing
alone, and 1.6 ms for a 1920-column indexed query (best of three). These are not
end-to-end GUI or file-I/O measurements. A virtual billion-element row verifies
query operation/output bounds without allocating a billion payloads. See
[the rendering chapter](../ext/surfer/docs/html/transaction-rendering.html) and
its repeatable ignored timing test.

## Source tile highlighting from a static index, not a lexer or a server

Lexical highlighting cannot distinguish ports from parameters, clocks from
data inputs, or the generate branch an instance takes. Surfer's source tile
gets classification from the `source_index` section of the VDB, written
when the design is verilated by `verilator_vdb_index`, a slang-based program
that ships with the pinned Verilator fork. Surfer only reads files.

Alternatives measured against the goal of accurate, instance-aware source:

- **syntect with a Sublime grammar**: accurate keywords and comments, still no
  symbol kinds, directions, clocks or generate elaboration; rejected.
- **tree-sitter-systemverilog**: no preprocessor, no elaboration; wrong on
  macro-heavy code and generate branches; rejected.
- **sv-parser in-process**: full grammar in Rust but no symbol resolution or
  elaboration, and a large compile-time cost in the WASM build; rejected.
- **Verible's language server**: does not elaborate, so per-instance parameter
  values and generate branches are unavailable; rejected.
- **A live slang-based language server started by Surfer**: semantic tokens
  and per-instance generate queries over LSP are accurate, but require an
  installed C++ binary and a process per design, exclude WASM, and answer
  questions whose answers are fixed during a debugging run. The compiler
  frontend belongs in the build rather than the viewer.

The index is produced by a separate program rather than by linking slang into
`verilator_bin`: Verilator's own frontend must not gain a second parser in its
process, its build stays make-based while slang is a large CMake project, and a
missing or failing indexer must only cost the index (`VDBINDEX` warning), never
the model. slang lives as a submodule of the Verilator fork (`ext/slang`,
pinned to a release) so the fork is self-contained: whoever builds the
simulator gets the exact frontend the index format was validated against, and
Verilator finds the indexer beside its own executable rather than on `PATH`.

Two elaborations of one design (the simulator's, recorded in the VDB, and
slang's) must agree, so the VDB carries a structured `elaboration` record
(working directory, files, include directories, defines, library settings,
top) that the indexer reads; the flat argv string kept in `options` loses
quoting and mixes C++ flags with Verilog ones and is not parsed. slang
identifies symbols by source position, the VDB by elaborated path; the index
stores each identifier's declaration position and the viewer joins it to the
VDB symbols and instances declared at that position, choosing the one under
the viewed instance. Declaration columns from Verilator and slang agree
exactly, which the fixtures check. Storing tokens once per file (not per
instance) keeps the index linear in source size; only the uninstantiated
generate ranges are per instance.

The same section is written by the pyslang exporter, and its output is
identical to the C++ indexer's on every fixture, so both producers stay honest
against one schema. Surfer's tests open the checked-in examples and click with
real pointer events; no toolchain runs in the tests.

## Cursor values inline in the source tile, hover kept for detail

Debugging RTL
against a waveform is reading which branch fired and what its operands were at
the cursor, line after line; a value that needs a hover per identifier is a
value most lines never show. Values are always visible, the way a JetBrains
debugger annotates a stopped frame, and the hover keeps what the inline form
cannot carry: the elaborated type, the owner, and every path the token denotes
when a module is instantiated more than once.

The layout alternatives are: values trailing the code of each
line, a fixed value column at the right edge, an interlinear row above each
line with the value over its identifier, and inlay chips after each identifier.
Trailing values and inline chips are selectable from the tile header and a
palette command, with a configurable default. **Trailing** is the default because it is the only layout that leaves
the code where it is: ctrl-click and alt-click targets never move while the
cursor is dragged, and aligned port lists and case arms stay aligned. It
borrows the column's one strength: values start at the greater of the end of
the code plus four blanks and a configured column, so short lines read as a
column and long lines simply push further right. **Inline** chips are kept as
an option for dense expressions where the eye should not travel to the end of
the line; the code reflows, so the tile maps every drawn character back to its
source byte and clicks keep working. The value column was rejected for
stealing width and separating a value from its name; the interlinear row for
uneven line pitch and collisions between neighbouring identifiers, which RTL
port connections make routine.

Values take one color no token class uses, amber when the signal's last
transition is at the cursor (the cursor snaps to edges, and stepping edge by
edge is how the design is watched moving), red for undefined or high-impedance
bits, a dash when the design has the symbol but the trace did not record it.
They go through the same translators as the waveform rows and follow the radix
of a displayed row of the same signal, so the tile never disagrees with the
waveform beside it. Parameters print the elaborated constant from the VDB, so
they show even when the simulator did not trace them. A struct prints as a
brace list and a member access as that member: the translator's fields when it
has them, otherwise a slice of the recorded aggregate by the elaborated type in
the VDB, because a two-state simulator flattens packed structs in the trace. An
array element prints when the index is a constant or a parameter with one
elaborated value; otherwise the array prints its element count, since guessing
an index would put a wrong number next to the code.

Producer spelling differences require tolerant joins. Verilator counts declaration columns after
macro expansion while slang counts them before, so a declaration on a line that
also expands a macro (`parameter bit WRAP` after `` `FEAT_WIDTH ``) may not join
by position; the tile falls back to the symbol of the same name declared on
the same line. Verilator's VTR writer nests the elements of an unpacked array in
a scope named after the array while its binding spells them as plain elements;
the attachment and the variable lookup accept both spellings. These rules live
in the viewer because they are tolerances of one producer's spelling, not
format rules.

The values are computed per line and cached for one file, instance, cursor,
design and set of waveform formats; a line is recomputed only while one of its
signals is still loading. The signals a file references are requested from the
document the way a waveform row requests its signal, so the native reader loads
them in the background and releases them with the tile. Only the visible rows of
the file are laid out, which also bounds the cost of long files. Verilator
records two-state values only, so the undefined-value test writes a four-state
twin of the features recording with the VTR writer beside copies of its VDB and
sources; that keeps the case on the same design and the same code path an X
from a four-state simulator would take.

## Volna viewer

Waveform navigation borrows smooth time navigation and range zoom from Surfer,
and the simple scrolling distinction from
[Vaporview's controls](https://github.com/Lramseyer/vaporview#controls): wheel
over waves pans time; Shift-wheel or wheel over labels scrolls rows.
Ctrl/Cmd-left-drag selects a time interval in either direction, regardless of
vertical drift. Plain left-drag retains Volna's cursor scrubbing; middle/right
drag pans. Surfer's directional gesture menu is omitted because a small vertical
pointer movement should not turn range selection into fit or endpoint navigation.
Vaporview's automatic mouse/touchpad detection and plain-left-drag zoom are not
adopted: deterministic modifiers keep input predictable and preserve scrubbing.
Range selection, previews and animation remain in `volna-core`, shared by native
and web hosts. Volna retains its short cubic animation. Repeated keyboard and wheel
inputs compose against the pending viewport target, while retargeting samples
the current animation so an input between frames does not jump backwards.
This changes client presentation only; VTR, VDB and the wire format are unaffected.

`volna/volna` is the official VTR/VDB viewer, built with GPUI. Source/history
interfaces separate runtime trace access from presentation; asynchronous-load
regression tests guard against stale results. It currently provides VTR
and FST waveforms; VDB attachment and other trace domains remain future viewer work.
Presentation and static design semantics stay outside the VTR format.

Standard controls use [GPUI Kit](https://github.com/longbridge/gpui-kit)'s
`gpui-component` 0.6.1: buttons, tooltips and popup-menu interaction replace
local implementations. The frontend retains component styling, popup placement,
the filter input and its core-driven splitter and data canvases. Filter values
and menu commands stay in `volna-core`. This reduces local interaction code without introducing toolkit
types into the shared viewer. Component theme tokens are projected from the
existing core palette, and selected Lucide assets are embedded on every target.

The frontend imports GPUI, components, assets and platform APIs through the
`gpui-kit` 0.6.1 umbrella. Its web dependencies enable compiled-in threading
support and pull in `wasm_thread`'s nightly-only feature, so the workspace pins
a dated nightly toolchain. This compiler requirement does not require runtime
threads: `gpui_kit::platform::single_threaded_web()` disables worker dispatch,
and the WASM build uses ordinary unshared memory without atomics/shared-memory
linker flags or a threaded standard library. The same bundle runs in standalone
browsers and default VS Code without cross-origin isolation or extra startup
flags. Keep this explicit configuration when updating the framework.

The filter deliberately retains the same custom widget on native and web.
With the component `InputState`, `gpui-pre-web` 0.3.4 handles
`TextInputStateChange::FocusLost` by blurring its hidden textarea. Keyboard
listeners are attached to that textarea, so opening a format menu after filtering
leaves shortcuts without a receiver in the browser/VS Code webview. Keeping the
small existing input avoids a DOM-focus workaround or a fork of the web runtime;
revisit it when upstream supports this focus transition. The component menus
are focused as they join the rendered tree and sit outside the waveform key
context, so their arrow/Enter/Escape bindings route to the menu.

Volna shares the workspace lockfile and local VTR crate, but is excluded from
default members so GUI dependencies and platform SDK requirements do not enter
the default core build. Workspace development uses the nightly selected by
`rust-toolchain.toml`; individual crate minimum versions remain declared in
their manifests. A separate `viewer` profile uses thin LTO; the
release/benchmark profiles use fat LTO.

Whole-viewer workflows live in `volna/volna/tests/`, separate from internal
unit/regression tests. The feature-gated macOS harness runs on the main thread
for AppKit, reuses the production workspace, and combines interaction assertions
with optional screenshot artifacts. Timing measurements are explicitly ignored
by default so machine speed does not determine test success.

Volna's VS Code theming uses resolved webview CSS variables, following the
[official guide](https://code.visualstudio.com/api/extension-guides/webview#theming-webview-content).
JavaScript only snapshots and observes the delivered CSS. Shared Rust owns the
VS Code mapping, CSS parser, typed semantic palette, appearance enum, precomputed
surface/selection colours, fallbacks and GPUI invalidation. Supplied foreground/background pairs
are preserved; thin chart strokes promote alpha to full coverage and adjust
only lightness when needed. Marker chips retain chart RGB with separate readable
labels. Automatic WASM startup reads an optional synchronous host snapshot before
creating its first window; missing metadata uses Rust fallbacks and never gates
startup. Observation continues for late updates. Theme kinds alone cannot
reproduce custom themes; parsing theme files would duplicate VS Code's resolution rules
and miss customizations. Raw real VS Code snapshots run directly through the
production Rust adapter in unit and visual tests. A small serde-derived JSON
interface also accepts host-neutral CSS colours, using the same parser; this
avoids duplicating role names in a separate JS wire decoder. Live updates refresh
the same views without resetting trace or interaction state. The single-webview layout and native/standalone
One Dark defaults remain. See Volna's architecture and verification guide for
the boundary and tests.

## Volna user settings

Settings follow VS Code and Zed rather than Surfer's TOML: one `settings.json`
with comments holding only changed keys, edited surgically by the GUI so hand
edits and GUI edits never fight, a registry in `volna-core` that generates the
editor, the JSON Schema and VS Code's contribution block, and a ranked fuzzy
search instead of gpui-kit's substring filter. Nested objects like Zed's were
rejected because the VS Code contribution is flat and surgical edits into
nested objects need path-aware insertion; flat dotted keys map to `volna.*`
unchanged. A GUI-only store with the file as an export was rejected because it
cannot preserve comments. Re-serialising the whole document (the previous
preferences path) was replaced by a few hundred lines of span-recording parser
and edit engine, which removes a class of "my comment disappeared" complaints.

The user file deliberately has no version field, an exception to the format
versioning rule in `AGENTS.md`: a hand-edited file must degrade key by key with a
diagnostic, never fail as a whole, or one typo would reset every setting.
Renamed keys are migrated by a table. The machine-written `state.json` (recent
lists) follows the rule and is versioned and strict. Recent items left the
settings file because settings are what a user syncs to another machine and
state is what the app remembers about this one, as VS Code separates them.

The Settings tab is a dock panel kind in the core rather than a modal sheet
(a sheet hides the waves, so appearance changes cannot be previewed) and rather
than a frontend-only tab (the dock mirrors the core layout, so a tab the core
does not know would break the mirror). It is excluded from workspace files and
re-added across trace changes, the one panel kind with that treatment.

`workspace.autosave` applies on the next trace open on every host instead of
the "next save" the proposal wanted: the workspace candidates are read when a
trace opens, so switching persistence on mid-session has no target to save to.

## Volna event waveforms

Volna uses Surfer’s event glyph (`libsurfer/src/drawing_canvas.rs`,
`draw_event`): a vertical stem with a filled upward arrowhead, 5 logical
pixels wide and one fifth of the trace height. The toolkit-independent
painter emits ordinary line segments, including scanlines for the filled head,
so frontend adapters need no event-specific rendering. VTR `VarType::Event`
and FST event declarations select this rendering; recorded timestamps are
occurrences even when consecutive payloads are identical. Same-pixel
occurrences coalesce and use a distinct theme token, `wave_event_coalesced`,
so multiplicity remains visible even when timestamps are identical. No held
level is drawn. Pixel grouping counts only visible occurrences and includes
all duplicates at both viewport boundaries. Colours remain client presentation.

The original variable declaration owns each signal's type. The hierarchy's
`signal_var_type` query follows its existing signal-to-declaration index;
Volna uses this query for both variable shapes and loaded histories without
keeping an event set. Payload storage remains `SignalKind`, separate from the
variable type that identifies occurrence semantics. The C API exposes the same
query. The writer retains the declaration type in its per-signal metadata and
uses it for both deduplication and alias validation. Event/non-event alias
mismatches are rejected by writers and readers: an alias cannot retroactively
change the meaning of already-recorded values. Other alias type differences,
such as wire versus reg, remain valid.

Repeated payloads, default-valued payloads and same-timestamp emits survive
every event writer path without requiring a global dedup override. Range readers
start at the first block whose end overlaps the lower bound, rather than the
last block starting before it: several blocks may contain occurrences at the
same timestamp. Existing column encodings represent these records.

## Volna panel ownership and layout

The panel model borrows Surfer's authoritative toolkit-neutral layout and
revision-checked adapter proposals (`ext/surfer/libsurfer/src/tiles/layout.rs`)
and gpui-base's bottom-up split normalization. Core shares are fractions:
flattening a same-axis child multiplies its shares by its parent slot, while
closing a child renormalizes surviving siblings without equalizing them.
Invalid external trees are rejected before normalization rather than repaired.

Linked navigation is read-through document state. The viewport animation lives
with that viewport, so adding linked panels cannot multiply its progression or
leave followers a frame behind. Independent panels own a separate animation.
Two link flags avoid introducing synchronization groups for the current scope.
Histories remain shared immutable Arcs across panels; the panel collection
provides reuse from existing rows without a second history cache.

Durable hierarchy locators use literal segments and an optional variable
occurrence. This preserves escaped names containing dots and same-name
declarations without guessing when a scope path is ambiguous. Runtime signal
handles and toolkit widget identities are not durable names. These changes are
viewer state only; VTR/VDB formats and the raw session query boundary are unchanged.

### Workspace restore and save ownership

Workspace files live beside traces or in host storage; no viewer state enters
VTR, VDB, or the query protocol. The core owns the versioned JSON schema,
segmented signal locators, complete layout validation, and restore selection.
Preparing a restore does not touch live state. Commit replaces the view and
invalidates old signal-load results. Unresolved rows own their saved locators,
and unknown panels retain raw JSON so another viewer can recover their content.

A revision/epoch/destination ticket identifies each write. Serializing writes
also orders Save As behind earlier saves; switching the destination requires a
successful acknowledgement. Trace changes and explicit workspace opens flush
before replacing state. The one-second idle scheduler excludes hover and
animation frames; pointer comparisons inspect navigation and geometry without
serializing signal rows. Default library instances keep persistence disabled.

Fallback files retain the SHA-256 of the sidecar from which they were based.
This distinguishes a newer local arrangement from a changed shared sidecar
without trusting clocks, keeping a registry, or parsing JSON in either host.
A selected fallback stays active after permissions change. Native uses an
atomic same-directory replacement; VS Code transports opaque text through
workspace.fs or workspaceState and echoes ticket counters as decimal strings.
Its retained custom editors request a snapshot when hidden and use the last
received snapshot when disposed, matching the editor lifecycle described in
its local API and the Surfer integration.


Dock chrome is rebuilt only for panel revisions; waveform edits invalidate
panel entities directly without borrowing them during focus callbacks. A first
restored split explicitly retries after its dock bounds are measured. Native
and wasm use the same stock engine and a header wrapper; library-owned close
is disabled in favor of core commands so widget detachment cannot delete saved
content. GPUI's dialog layer is hosted explicitly for rename and restore details.

The one-panel dock has a small measured cost rather than meeting the proposal's
strict zero-regression target. This is an intentional tradeoff for a single
layout/input path; no special single-pane bypass or toolkit fork is added.
The release A/B method, current frame costs and raw samples are in
[Volna verification](../volna/volna/VERIFICATION.md#performance-checks).
Both baseline and candidate benchmarks require a canvas repaint on each timed
keyboard frame; cached frames are not navigation measurements.
