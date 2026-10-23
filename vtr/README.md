# VTR for Rust

VTR records typed simulation transactions, stages, events, logs, clock stretches,
waveforms, hierarchy, and relations in one file. This crate independently reads
and writes the same VTR 3.2 format as the [C++20 SDK](../vtr-cpp/README.md).
Older recordings must be regenerated.

## Recording

Create a `Writer` with `WriterOptions`, declare scopes and typed sources, then
record through the `Recorder` trait. `Payload` binds a native Rust type to a
file-local datatype and encodes/decodes its padding-free canonical value.
Primitives, `String`, arrays, and `Vec<T>` have bindings. The `schema` module also
provides `StringId`, `TimeValue`, `Address`, `Fixed`, `UFixed`, `Bytes`, `Bits`,
`Logic4`, and `Logic9`. `Bytes` records a byte blob; `Vec<u8>` records a sequence.
`Cow<str>` and `Cow<[T]>` capture borrowed text and sequences without first making
an owned copy; decoding them returns owned data. Their schemas match `String`
and `Vec<T>`, respectively. `Bytes<Cow<[u8]>>` similarly captures a borrowed blob
with the same datatype as owned `Bytes`.

Use `impl_payload!(Packet { pc, description })` to bind a named structure without
repeating its field types or encoding logic. `impl_payload_enum!(Operation {
"load" => -7, "store" => 9 })` binds an integer tuple wrapper and retains unknown
numeric values. An optional first argument overrides either schema name.
Automatic bindings reuse identical datatype declarations. For specialized
encodings, implement `Payload::bind`, `encode`, and `decode` directly.
See [the logging example](examples/logging.rs) and the macro API documentation.
`Writer::bind::<T>()` exposes a stable file-local datatype ID before creating
sources. Recursive automatic bindings and schemas deeper than 64 levels fail.

`begin` returns a typed transaction handle. `begin_stage` creates a located stage
owned by an ordinary transaction. `event` and `log` create points. The `_at`
variants accept explicit observation timestamps. `advance_time` moves the logical
producer frontier and waveform time forward; it does not constrain historical
record timestamps. Source and owner handles belong to their originating recording.
Sources also carry their category in their Rust type: transactions, stages,
events, and logs cannot be passed to one another's recording methods.

Use `begin_deferred` or `begin_stage_deferred` when the payload is known later.
`payload(handle, value)` replaces the captured value of either active record;
`end_with_payload` and `end_stage_with_payload` supply the final value while
ending it. These methods also have explicit-time variants. Payloads are required
before ending a record or its owner and before normal close. Unit and other
fixed zero-size datatypes are implicitly ready; an empty sequence still requires
its encoded length.

`dump(false)` suppresses waveform samples while transactions, clocks, and logs
continue. Inputs remain validated. `dump(true)` resumes capture using the last
recorded values for deduplication; the first sample of each signal is always
recorded. State transitions produce blackout markers, visible after `flush`.
`blackout(time, active)` only annotates gaps when importing existing waveforms.
`emit_batch(&[SignalUpdate { signal, value }, ...])` copies scalar updates into
one producer capture. It validates every signal before recording the first
sample, preserves update order and repeated events, and accepts bit vectors up
to 64 bits wide.
`emit_words` accepts least-significant words first, requires enough input words
and the signal's exact declared width, and masks unused high bits.

Captures own their values before returning. Handles emit nothing on destruction.
End records explicitly or let writer completion mark live owners/stages/clocks
open when their payloads are ready. Incomplete sealing omits records with missing
payloads and preserves valid records; their references to omitted owners appear
in the reader's missing-reference report. Ordinary child transactions have
independent lifetimes. Optional attributes, embedded stages/events, and relations
supplement the typed payload.

Threaded recording is the native default. `add_producer` returns an exclusive
logical `Producer`, which can move between threads and implements `Recorder`.
Each producer advances its own frontier and calls `finish` when done. Finish every
explicit producer before normal writer close. Byte-bounded capture, pending spill,
and ordered encoding workers have separate options and costs.
Each recording call publishes its owned capture to the transport before
returning. With multiple producers, captures at their shared frontier wait until
every active producer advances past it. A single producer can process its current
timestamp. Accepted captures may report backend validation or output failures
later through `flush`, producer completion, or close; backend failures poison
recording. Transport acceptance does not imply flushed or durable output.

`Tracker` manages unit-payload transactions through named keyspaces. It supports
aliases and groups, oldest-first binding, independent stage lanes, events,
attributes, labels, relations, and selective aborts. Pass a writer or explicit
producer to each recording operation; advance that producer's time separately.
Keyspaces retain their identity when the tracker moves. Invalid keys/keyspaces
produce aggregated diagnostics; recording failures return errors. Close, abort,
or `detach` live items to publish pending attributes before dropping the tracker.
Detach keeps records and stage visits unfinished. Item and key-group storage is
reused, and the string cache is bounded; temporary selections and payload capture
can allocate. See [the tracking example](examples/tracking.rs).

`ExecutionMode::Inline` uses one implicit producer and performs capture, encoding,
and output on the caller. It rejects extra producers and encoding workers.
WASM uses inline execution. `flush` publishes eligible completed data; explicit
`close` reports failures and writes a complete directory. `seal` retains incomplete
evidence. `Drop` attempts sealing but cannot report its result. Buffered writes
do not promise `fsync` durability. The optional [Linux guard](../vtr-guard/README.md)
can rescue threaded recordings without borrowing an interrupted frontend.

`stats` reports live payload data, buffer admission peaks, published block counts,
and transport/encoding queue activity. Transport counts owned captures, excluding
declaration and control messages; completion means a capture left its input queue.
Encoding completion means its section was published. Captured vector capacity,
nested attributes, and schema snapshots contribute to admission charges; shared
schemas are conservatively charged per job. These charges exclude map bookkeeping
and encoder scratch rather than representing total process memory.
`backend_cpu_nanoseconds` samples the
collector and encoder threads without waiting for ingestion or blocked output.
Linux retains worker CPU totals after exit; unavailable worker clocks return
`None`, and inline execution returns zero. Queue wait durations are wall time
under backpressure; worker CPU excludes sleep and producer work.

## Reading

`Reader::open` maps a file; do not modify or truncate it while mapped. `from_bytes`
owns its input. Checksum verification is enabled by default. Read-only queries
are `Send + Sync`; the bounded owner cache synchronizes its internal state.

`find_signal` and `find_generator` select the requested declaration category, so
a clock generator and its dumped waveform can share a path. Path traversal
follows grouping scopes even when a signal has the same name as a scope.

`transactions` selects ordinary owners and clocks. `records` selects located
stages, events, and logs. Query structs filter generator, time, parent, and IDs.
`lookup` also finds embedded children. `expand` follows owner child-block locators
and exposes missing blocks for incomplete traces. Creation order and physical
storage order need not be chronological. `child_transactions` selects ordinary
runtime children, and `logs` selects one owner's logs in a time window. Hierarchy
children are available separately through `children`. Visitors return false to stop.
`relations` selects one endpoint and direction. `scan_relations` visits all stored
edges in block/page order, preserving duplicate edges and outside-recording endpoints.

A `Record` or `PayloadView` retains its schema and storage after cache eviction
or reader destruction. Use `field`, `at`, `scalar`, or typed `decode` to inspect
values. Use `children` to traverse fields and collection elements sequentially
without rescanning earlier variable-length siblings. `Record::status` and `span_kind` return the shared `Status` and `SpanKind`
enums used by recording APIs. Decode checks schema equivalence and allocation
limits. String IDs remain file-local IDs; resolve them through the reader's string
table.

`format_log_with_locale` and `format_log_bytes_with_locale` accept `FormatLocale`
for digit grouping, punctuation, and boolean labels in `L` presentations. Calls
without a locale use classic punctuation. Binary output remains available when
locale bytes or integer `c` presentations are not valid UTF-8.

Waveform APIs include `value_at`, `load_signal`, `changes`, and
`ordered_changes`. Ordinary change visitors follow storage order; the ordered
visitor buffers one decoded block to provide chronological traversal. Event
variables preserve identical repeated occurrences. Aliases share signal IDs.

`time_range` includes waveform progress, owners, located records, and embedded
detail outside owner intervals. It decompresses pages containing embedded detail
and scans their time columns, without materializing payloads or attributes or
filling the owner cache. Pages without embedded detail use indexed bounds.
`waveform_time_range` remains a metadata-only query for waveform timestamps.
Owner-interval queries retain their recorded lifetime semantics.

`validate` checks every page and sample plus global references, ownership, locators,
and clock consistency. A normal complete recording resolves every nonzero runtime
reference. An incomplete or unfinished trace exposes surviving records and missing
references. `recover` writes the verified prefix to a different file, preserves
recording identity, and creates a new incomplete fingerprint.

`activity` builds and loads source-bound exact VTI 3.0 sidecars. A cache is valid
only for the recording's identity and finalized fingerprint. Build admission and
decode allocation budgets are described in the module and format specification.

## Examples and documentation

Run from the repository root; output paths are optional:

```sh
cargo run -p vtr --example waveform -- /tmp/waveform.vtr
cargo run -p vtr --example transactions -- /tmp/transactions.vtr
cargo run -p vtr --example tracking -- /tmp/tracking.vtr
cargo run -p vtr --example logging -- /tmp/logging.vtr
```

- [Waveforms](examples/waveform.rs): declarations, packed logic, time, and point queries.
- [Transactions](examples/transactions.rs): typed instructions, embedded stages, and relations.
- [Tracking](examples/tracking.rs): keys, grouped retirement, labels, and detach.
- [Logging](examples/logging.rs): custom schemas, severity metadata, formatting, and retained decoding.
- [Format](docs/SPEC.md): container, hierarchy, waveforms, recovery, and VTI.
- [Typed record format](docs/TRANSACTIONS.md): datatypes, pages, identity, and queries.
- [Rationale](docs/RATIONALE.md): design choices and implementation boundaries.
- [Logging guide](docs/LOGGING.md): metadata, rendering, and run endings.
- [Benchmarks](../benchmarks/README.md): reproducible complete-write/read/query comparisons.

## Build and verification

The standalone crate requires Rust 1.97.1+ and a C compiler for Zstandard.
The workspace pins its development toolchain. Run from the repository root:

```sh
cargo test -p vtr --all-targets --locked
cargo fmt --all -- --check
cargo clippy -p vtr --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p vtr --no-deps --locked
```

Tests cover independent wire fixtures, typed round trips, malformed input,
producer ordering and spill, bounded decoding, retained records, waveform states,
late declarations, clocks, recovery, sealing, and exact activity queries.
[Verilator fixtures](tests/fixtures/verilator/README.md) are generated by the
pinned simulator fork in both execution modes; reading them needs no simulator.
The private `typed_interop` example is the Rust endpoint for native interoperability
CTest checks, rather than a separate user tutorial.
