# VTR query contracts

Foundations for the [native and VS Code query design](../../docs/client-server.md).
This package provides exact time/coverage types, allocation admission,
cooperative cancellation, optional stdio framing, native exact waveform windows
and resumable cold-path waveform summaries. Native metadata operations include
child pages, substring search, batched literal path resolution and string byte
parts, dispatched through a typed native session. A Protobuf schema and bounded
request decoder and borrowed-result encoder cover these operations. Reply decoding, the remaining query
families, asynchronous host adapters
and viewer integration are not implemented here yet.

`Interval` uses `[start,end)` with `TimeBound::AfterMax` for the exclusive end
after `u64::MAX`. `Grid` validates power-of-two alignment and bounds before use.
`Grid::covering` finds the finest complete grid fitting the requested bin count;
empty demand returns `None`.

`Budget::reserve` admits capacity before its allocation. Store the returned
`Reservation` with that allocation. Clone it only with shared backing: its last
owner releases the charge. Reservations cover application data; allocator
bookkeeping and process overhead still need separate measurements. Cooperative
`Cancellation` requires workers to check between bounded units of work.

The `wire` feature provides `FrameDecoder` and native `read_frame`/`write_frame`.
A frame is a little-endian `u32` payload length followed by opaque bytes. Zero
length and over-limit frames fail the stream before payload allocation. A
decoder returns at most one frame per call and reports consumed bytes, so the
caller can withhold further parsing until delivery credit is available. Retained
frames share their payload reservation. Framing alone does not bound Protobuf
decoding, reader work or extension message queues.

Wire-enabled builds require `protoc` on PATH (or selected by `PROTOC`). The
schema is `schema/query.proto`; Prost generates its Rust types during the build.
Only `wire` enables the generator and runtime codec dependencies. The query
crate requires Rust 1.85; the workspace's pinned toolchain satisfies this.

`wire_request::decode` validates the request schema before invoking Prost. It
rejects wrong-direction/unknown fields, duplicate singular fields, conflicting
oneof alternatives, scalar overflow, invalid UTF-8, truncated fields and excess
structural or decoded-allocation budgets. It then checks required fields,
protocol version, exact snapshot identities, query limits and normalized time
types. This strict version-one contract does not silently accept unknown fields.
The returned request retains its conservative decode reservation until dropped.
Raw generated message decoding bypasses this admission and must not be used on
untrusted input. `wire_encode` counts encoded bytes before allocating one admitted
output buffer and writes directly from borrowed typed results. Reply-side decoding
is still under development.

```sh
cargo test -p vtr-query --all-features
cargo clippy -p vtr-query --all-features --all-targets -- -D warnings
cargo check -p vtr-query --all-features --target wasm32-unknown-unknown
```

Default features are empty. A native session must use typed results directly;
it must not encode and decode frames to imitate the remote path. The foundation
has no VDB, GUI toolkit or JavaScript dependency. `native-engine` enables the VTR
reader only on native targets; WASM excludes it even with all features enabled.

`native::Window` traverses a half-open interval with a strict predecessor and
returns one immutable `wave::WindowPage` per `next_page` call. Pages retain byte
reservations, shared predecessor storage and exact packed values; real values
preserve their IEEE bits. Record/work/response-byte limits are independent.
When a page fills, the reader leaves its next event unconsumed, including at
the same timestamp. A work-limited page can be empty without proving exhaustion.
`complete` is the only end-of-window indication; the requested interval alone
is not a complete coverage claim. Cancellation is checked between scan batches.

These limits bound response allocations, not reader caches or decoder scratch.
In particular, predecessor lookup still uses the reader's owned point-query
result. Oversized individual values return `ResourceLimit`; paged large-value
retrieval, blackout coverage, raw summary indexes and wire continuations remain
required before the viewer can adopt the complete bounded-session contract.

`native::Summary` scans directly into a constant-sized `summary::BinBuilder`;
it does not materialize exact pages or retain a history. Each response contains
only completed canonical bins and an offset into the requested grid. An empty,
incomplete response means the current bin needs more scan work. In-progress
accumulators survive work-budget boundaries, while completed bins are delivered
before attempting further allocations that cannot fit the shared budget.

Bins preserve strict entry samples, source occurrence counts, first/last changes
and exit samples. Real extrema cover finite held values with nonzero duration
inside the bin; intermediate same-time values do not affect extrema. NaN and
positive/negative infinity counters count source changes, excluding the entry
sample, so those counters compose across bin boundaries. Events have counts
and first/last occurrences but no held exit value. `WaveBin::merge` composes
complete aligned siblings for the same source; the caller must match snapshot
and raw selector identity. Transaction overlap counts cannot use this reducer.

The native summary implementation is a cold scan, not the proposed reusable
warm index. Reader byte admission, blackout/unavailable coverage, large-value
references and performance gates still apply before deployment readiness.

`native_metadata::Children` uses the reader's adjacency index and borrowed node
views. Headers carry raw types, aliases, signal identities, child counts and
attribute/enum counts without cloning detail collections. Large names, or names
that do not fit the current page budget, use explicit snapshot-local string
references. `text_part` retrieves byte ranges; ranges may split UTF-8 and must
be assembled before text decoding.

`Search` matches Unicode scalar lowercase expansions without normalization,
preserves declaration order and has no hidden result-count cutoff. It retains a
bounded pattern and KMP state, not lowercase copies of all names. Character
comparisons and ancestry traversal consume work units; empty incomplete pages
preserve progress through large names or deep scopes. `ResolvePaths` accepts
batches of literal segments and walks siblings under the same work model.
Intermediate scopes must be unique; an optional zero-based terminal occurrence
selects duplicate declarations. Results distinguish found, missing and ambiguous.
Neither operation requires client round trips for each hierarchy level.

`session::Query`, `Reply` and `Delivery` are shared typed contracts. The native
`native_session::Session` dispatches them without encoding and must run on a
host worker. Each opened session has a fresh snapshot ID; operation IDs are
never reused within it. Continuations validate snapshot, operation and page
sequence before touching query state. A retry returns the identical shared
delivery until the caller advances or releases the operation. Advancing
acknowledges the previous page and permits its eviction; no unbounded response
history is retained. Completed operations also need release, which is idempotent.

Operation slots are fixed-capacity and admitted to the shared budget before
allocation. Each slot retains at most one retryable reply plus its bounded query
state. A host-held cancellation token can stop work while the session worker is
busy; dropping the session cancels all operations. This is the execution layer,
not the asynchronous viewer adapter or the complete wire protocol. Reader cache
and scratch admission are still needed in addition to response accounting.
