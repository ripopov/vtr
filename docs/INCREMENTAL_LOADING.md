# Incremental loading: rationale and implementation proposal

Status: proposed. The interfaces and live protocols below are design sketches,
not implemented contracts. [SPEC.md](SPEC.md) and
[TRANSACTIONS.md](TRANSACTIONS.md) remain authoritative for the current format.

VTR should let a viewer attach to a running simulation, inspect available
waveforms and records, and incorporate later publications without reopening the
recording or decoding its entire history. Closing or sealing the recording
should turn that same session into an ordinary finalized recording.

Implement this first with the existing VTR container. Add explicit writer
publication, a reader that extends a verified prefix, and changes that consumers
can apply to their loaded data. Preserve the ordinary writer defaults and the
finalized reader's memory-mapped access. Treat active transaction previews and
fast attachment to very large unfinished recordings as separate, optional work.

## 1. Goals and acceptance criteria

Incremental loading should provide:

- Attachment before the first data block, including an initially empty file.
- Early hierarchy discovery and later declaration growth without changing IDs.
- Refresh work proportional to new bytes and affected indexes, with bounded
  work per call and no full-file rescan on an idle poll.
- Waveform and record updates that preserve repeated occurrences, same-time
  ordering, typed payloads, aliases, relations, and unresolved references.
- Queries against a consistent snapshot while another thread refreshes.
- Retained records, payloads, and histories that remain valid across refreshes.
- Predictable publication latency when producers and output can make progress,
  with diagnostics when they cannot.
- Recovery after producer failure without treating missing data as completion.
- Equivalent Rust and C++ contracts and interoperability in both directions.
- A path for remote consumers to use the same generations and changes without
  polling or transferring the entire file.

Two performance requirements must be evaluated separately. With live support
disabled, existing recording and postmortem workloads must have no unexplained
regression. With live support enabled, latency has a measurable cost: more
frequent block closure can increase file size, index overhead, and encoding work.
Report that tradeoff rather than promising identical output size at every
publication interval.

## 2. Existing foundations and limitations

The [container](SPEC.md#2-container) already has append-only sections with lengths
and payload checksums. Declarations precede their users. A reader can discover
complete sections without the final directory and trailer. Waveform blocks
contain time tables, dirty-group indexes, frames, and previous-dirty links;
record blocks contain selective query indexes. These structures already support
useful queries against a completed prefix.

The current implementation opens an immutable snapshot:

- Rust's [container](../vtr/src/framing.rs) maps the file once and requires it
  to stay immutable while open.
- The [reader](../vtr/src/trace/mod.rs) builds declarations, hierarchy navigation,
  waveform indexes, and block lookup tables during construction.
- The [writer](../vtr/src/recording/mod.rs) exposes a blocking `flush()`.
  Its [output module](../vtr/src/recording/output.rs) drains encoding jobs and
  flushes output, but does not return a publication receipt.
- Automatic block closure uses byte and record thresholds, which do not bound
  wall-clock visibility for sparse recordings.
- Active owners and stages remain mutable in memory. Owners are submitted to
  transaction encoding when they end; active clock stretches also need explicit
  treatment. Flushing completed records does not publish all active state.
- Persistent [activity indexes](SPEC.md#9-activity-index-sidecar-vti-30) require a
  finalized fingerprint.

Prefix recovery is not an incremental session. Recovery stops at the first
invalid section and writes a separate incomplete recording. A live reader must
wait for a short suffix, retain its position, and resume without recovering or
modifying the source file.

## 3. Lessons from FST

FST provides useful implementation patterns, rather than evidence that every
viewer performs efficient in-place refresh.

| Upstream behavior | Implication for VTR |
| --- | --- |
| libfst writes a provisional `FST_BL_SKIP` tag and replaces it after completing and flushing a block. | Define publication independently of physical file length. VTR can retain immutable section headers and advertise a complete prefix. |
| `fstWriterFlushContext()` requests a flush handled at a subsequent time transition. | Separate a publication request from a completion barrier; choose a safe recording point. |
| The writer interface exposes pending-flush state. | Expose pending work and publication progress instead of making consumers infer them from file growth. |
| Value-change blocks contain checkpoints, signal-chain indexes, and time tables. | Keep useful indexes in each published block; avoid a mandatory final directory for live queries. |
| libfst can read intermediate hierarchy from a `.hier` file. | Publish declarations early. VTR's existing declaration sections can provide this without a required hierarchy sidecar. |

Sources: upstream [publication and flush implementation](https://github.com/gtkwave/libfst/blob/main/src/fstapi.c),
[writer interface](https://github.com/gtkwave/libfst/blob/main/src/fstapi.h), and
[block layout](https://github.com/gtkwave/libfst/blob/main/doc/block_format.txt),
reviewed 2026-10-28.

VTR should preserve its sparse group frames and previous-dirty links rather than
adding a full signal snapshot at every live update. Full snapshots would make
frequent publication expensive for large designs with little activity. FST also
does not supply VTR's semantics for provisional owners, deferred payloads, and
relations; those require a separate design.

## 4. Review of five binary span tracing formats

The five most useful reference designs for VTR are Perfetto, Common Trace Format
(CTF) with LTTng live, Java Flight Recorder (JFR), EventPipe/NetTrace, and Fuchsia
Trace Format (FXT). This selection covers explicit span lifecycles, live
multi-stream delivery, active file repositories, bounded ordering and reference
state, and protected metadata during streaming. It is a selection for design
relevance, not a ranking by adoption or benchmark performance. FST remains the
waveform reference reviewed above.

Research reviewed 2026-10-28. The descriptions below distinguish the binary
representation from its writer, transport, and reader. Receiving new trace bytes
does not establish that an existing viewer refreshes incrementally. Starting a
new tracing session against a running application also differs from joining a
session already recording. No design can reconstruct discarded history or an
unseen span beginning without additional retained state.

| Format and tracing system | Representation of a span | Documented live route | Main idea to evaluate for VTR |
| --- | --- | --- | --- |
| Perfetto | Track-local slice begin/end records | Service buffer reads or streaming output; incremental Trace Processor ingestion | Publish span beginnings and make decoder-state recovery explicit. |
| CTF / LTTng live | Schema-defined events, with interval correlation supplied by instrumentation/analysis | Relay protocol serving packet indexes, metadata, and packets during recording | Distinguish idle streams, missing metadata, new streams, and completion. |
| JFR | Committed typed events with start time and duration | EventStream follows an active recording repository | Provide coherent batches and metadata updates without requiring global timestamp order. |
| EventPipe / NetTrace | Typed events; activity/start-stop conventions supply interval meaning | Diagnostics client attaches to a running process and receives an event stream | Bound sorting and transient reference state with explicit checkpoints. |
| FXT / Fuchsia tracing | Duration begin/end, complete durations, and correlated async begin/end | Providers hand buffers to the trace manager during streaming | Protect decoding metadata and acknowledge buffer ownership transfers. |

### 4.1 Perfetto: early span evidence and recoverable sequence state

Perfetto is a system and application performance tracing suite, originally built
for Android and Chrome. Its protobuf trace format combines application activity,
OS scheduling, counters, and profiling data on a common timeline for diagnosing
functional and performance problems. [Perfetto overview](https://perfetto.dev/docs/)

Perfetto's protobuf `TrackEvent` represents a slice with `TYPE_SLICE_BEGIN` and
`TYPE_SLICE_END`. The schema deliberately avoids a native complete-duration
event: waiting for completion delays publication, reorders evidence, and loses
unfinished slices when a trace ends. These slices nest on a track; arbitrary
overlapping transactions must not be forced into one track's nesting stack.
This directly exposes the tradeoff between compact final records and early
evidence. [TrackEvent schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_event.proto)

Incremental interned data and defaults belong to a packet sequence.
`SEQ_INCREMENTAL_STATE_CLEARED` establishes a fresh decoding state;
`SEQ_NEEDS_INCREMENTAL_STATE` identifies dependent packets. After packet loss,
readers skip dependent packets until state becomes valid again. This supports
recovery of subsequent evidence, not recovery of missing slice beginnings.
[TracePacket schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto)

The tracing service can drain buffers during recording, including writing
streaming output. Its dataflow is explicitly optimized for capture overhead,
not minimal read latency. Separately, the Trace Processor interface supports
repeated `Parse` calls, `Flush` to expose sorting-buffer contents, queries over
loaded data, and a distinct `NotifyEndOfFile`. These mechanisms do not by
themselves promise that the browser UI tails an arbitrary growing file.
[Buffer dataflow](https://perfetto.dev/docs/concepts/buffers),
[Trace Processor interface](https://github.com/google/perfetto/blob/main/include/perfetto/trace_processor/trace_processor.h)

**Implication for VTR:** make early transaction evidence an explicit capability.
The optional overlay should publish a stable owner ID and beginning before the
owner finishes. A late subscriber needs an active-owner snapshot in addition
to declarations; a dictionary checkpoint alone cannot reconstruct active spans.
Keep VTR's durable complete records for postmortem access, and measure the
additional preview work. Add a resumable decoder-state token to any live
transport that uses transient interning. Preserve a separate end-of-input
operation so a temporary EOF cannot close active spans or finalize a session.

### 4.2 CTF and LTTng live: packet discovery, metadata gates, and idle progress

CTF is an interchange format for timestamped execution events described by
metadata, allowing recording and analysis tools to share traces. LTTng uses CTF
to record Linux kernel and user-space activity for investigating performance,
concurrency, and failures. [LTTng documentation](https://lttng.org/docs/v2.13/)

CTF supplies binary event streams whose payload meanings come from metadata.
Span reconstruction is a schema/instrumentation convention, rather than a
universal interval row built into the container. LTTng live is the additional
protocol that permits reading an ongoing recording through a relay. The viewer
attaches to a live-enabled session, obtains streams and metadata, then requests
packet indexes and packets. Attachment can start at retained history or at the
next arriving packet. Replies can require fetching new metadata or discovering new streams
before decoding proceeds. Inactivity information lets readers advance past an
idle stream up to a stated time. [LTTng live protocol](https://github.com/lttng/lttng-tools/blob/master/doc/live-reading-protocol.md)

Babeltrace's live source connects to a local or remote relay and merges its
streams. Its documentation explicitly distinguishes waiting for an absent
session from ending the iterator. It also distinguishes CTF versions: CTF 2
support requires the LTTng 2.15 live protocol and the corresponding Babeltrace
message-interchange support. Do not infer transport support solely from a CTF
wire-format version. [Babeltrace live source](https://github.com/efficios/babeltrace/blob/master/doc/man/babeltrace2-source.ctf.lttng-live.7.txt)

**Implication for VTR:** separate publication discovery from data transfer.
A remote publication response should distinguish new data, no eligible data yet,
required declaration extent, source replacement, and terminal state. Idle
progress can help explain producer-frontier stalls without emitting fake events.
If a timestamp guarantee is offered, state whether it applies to producer
operations, waveform observations, or record event time. LTTng-style inactivity
information must not become a false promise that every VTR transaction beginning
before that timestamp has arrived. Keep ordinary local-file following available
without requiring a relay.

### 4.3 JFR: stream an active repository with coherent batches

JFR is the JVM's structured event recording facility for diagnosing Java
applications. Its binary recordings associate timestamps and durations with
typed payloads, capturing runtime behavior and custom application operations
for performance analysis and failure investigation.
[Flight Recorder overview](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/package-summary.html)

JFR's `EventStream.openRepository(Path)` follows a recording repository,
including one outside the current JVM. By default it starts with the next event
flushed, which is a different contract from replaying retained history.
`onMetadata` announces event types before their events; `onFlush` identifies
delivered batches. Ordering is guaranteed within a batch by end time, not across
all batches. Event objects may be reused unless reuse is disabled, so retaining
a callback object requires an explicit lifetime choice.
[EventStream interface](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/consumer/EventStream.html)

JFR events become recorded data through `commit()`. `begin()` and `end()` measure
duration on the event object; they are not themselves a pair of published span
lifecycle records. Streaming committed events therefore does not automatically
show an operation that is still running.
[Event interface](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/Event.html)

The OpenJDK reader refreshes a chunk's published extent, metadata position, and
constant-pool position using file-state checks around header reads. It waits
while the header is being updated. This is an example of a specific consistency
protocol for a growing recording, not permission to read mutable headers without
coordination. [ChunkHeader implementation](https://github.com/openjdk/jdk/blob/master/src/jdk.jfr/share/classes/jdk/jfr/internal/consumer/ChunkHeader.java)

**Implication for VTR:** specify whether attachment means replay from the beginning,
resume from a cursor, or follow future publications; make the choice explicit.
Commit declarations and query-visible changes as one consumer generation, and
keep snapshot/payload ownership explicit. JFR reinforces that complete duration
records can coexist with live consumption, provided consumers accept late
timestamps. Adopt its coherent publication principle through immutable prefixes
and receipts rather than copying its mutable chunk-header mechanism. Active
VTR owners still need previews if they must appear before completion.

### 4.4 EventPipe and NetTrace: bounded sorting, loss detection, and cache epochs

EventPipe is .NET's cross-platform runtime tracing mechanism; NetTrace is its
binary event serialization format. Together they capture runtime events, such
as garbage collection and JIT compilation, alongside custom application events
for profiling, monitoring, and diagnostics. [EventPipe overview](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/eventpipe)

Microsoft's diagnostics client can target a running process, start an EventPipe
session, and parse its event stream immediately with `EventPipeEventSource`.
This is a supported live collection path, rather than an assertion about tailing
any `.nettrace` file produced by any writer.
[Diagnostics client examples](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/diagnostics-client-library)

The current NetTrace specification identifies version 6. It defines typed,
length-prefixed blocks, references to previously defined metadata, and an
`EndOfStreamBlock`. Events carry provider-defined meaning, including activity
correlation; the container is not a universal span table. `SequencePointBlock`
provides a temporal checkpoint and bounds the lifetime of stack and label-list
references, with optional thread/metadata cache resets. Per-thread sequence
information supports detection of dropped events. The event `IsSorted` flag
provides an earlier-timestamp bound for subsequent events, allowing buffered
events older than that bound to be emitted before end of stream.
[NetTrace specification](https://github.com/microsoft/perfview/blob/main/src/TraceEvent/EventPipe/NetTraceFormat.md)

**Implication for VTR:** use distinct fields for publication completion, decoder
state, and ordering guarantees. A time-order certificate is valid only if the
writer enforces it for the relevant timestamps. VTR currently permits explicit
past record timestamps and late owner completion, so copying NetTrace's event-time
checkpoint would require a stronger producer contract. Apply bounded reference
epochs to optional transport dictionaries and preview revisions, while keeping
durable VTR IDs stable. Report missing preview revisions as a gap requiring
resynchronization; do not silently join unmatched begin/end evidence. Treat
transport closure and successful container finalization as separate outcomes.

### 4.5 Fuchsia FXT: multiple duration encodings and protected streaming metadata

FXT is Fuchsia's compact binary format for collecting, transmitting, and storing
system and application trace records. It combines events from multiple providers
so analysis tools can examine execution and interactions across threads and
processes. [Format overview](https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#overview)

FXT supports thread-local duration begin/end, complete-duration records carrying
an end timestamp, and async begin/end records joined by a correlation ID across
threads. Archives consist of size-framed records with provider context and
previously registered string/thread references. Timestamped records are not
guaranteed to arrive chronologically.
[FXT specification](https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format)

Fuchsia's streaming providers use a protected metadata buffer and two rolling
buffers for event data. When a rolling buffer fills, the provider switches and
notifies the trace manager. That buffer is unavailable for reuse until the
manager acknowledges saving it. If both rolling buffers are unavailable, events
are dropped instead of waiting. The protected area holds decoding information
such as strings and thread references; exhausting it stops that provider.
Here, "durable buffer" is a buffering term, not an `fsync` guarantee.
[Provider buffering modes](https://fuchsia.dev/fuchsia-src/concepts/kernel/tracing-provider-buffering-modes)

**Implication for VTR:** distinguish bytes that an encoder owns, bytes written
to the output buffer, and bytes advertised to a reader. Recycle storage only
after the relevant owner has released it, and prioritize declaration availability
over optional previews. FXT demonstrates that early begin/end records and compact
completed durations can be separate representations with different purposes.
Borrow its explicit ownership handoff, while preserving VTR's existing bounded
recording/backpressure contract. A preview subscriber can miss updates without
causing the durable recording to lose observations. Streaming buffer handoff
alone does not establish arbitrary late attachment to an already running
collection; VTR must specify that bootstrap explicitly.

### 4.6 Decisions informed by the comparison

These are VTR proposals inferred from the reference designs, not capabilities
claimed for those systems:

1. **Define attachment modes.** Offer replay of the retained verified prefix,
   resume from a physical cursor, and future-publications-only following. Return
   the actual starting prefix/generation and declare any unavailable history.
   Starting at the current byte offset is insufficient for decoding references
   or reconstructing owners already active at attachment.
2. **Make bootstrap coherent.** Pair a publication receipt with the required
   declaration state, decoder epoch if applicable, and optional active-owner
   snapshot. Send that state before dependent deltas. Resetting transport
   dictionaries must never renumber durable records or make missing owner
   beginnings appear complete.
3. **Expose waiting and loss separately.** Return explicit progress when producers
   are idle, and explicit revision gaps when optional previews fall behind.
   Neither an idle notification nor a transport disconnect ends a transaction.
   Durable source corruption remains an error rather than a resumable loss epoch.
4. **Separate receipt time from event time.** Keep byte publication, waveform
   closure, and transaction timestamps distinct. Make every stronger ordering
   guarantee conditional on a documented producer contract. Test records with
   past timestamps arriving after several otherwise idle publications.
5. **Keep final and provisional representations distinct.** Store normal completed
   VTR records as today; publish lightweight active-owner state only when that
   capability is requested. Benchmark an overlay against an event-lifecycle
   representation rather than assuming either is free. Preserve open/unknown
   state after loss or crash and replace previews atomically with final records.
6. **Measure visibility through the whole pipeline.** Include producer buffers,
   encoding, output flushing, parsing, ordering queues, and consumer generation
   updates. A writer flush interval is not a capture-to-visible latency bound.
   Add late-attachment bootstrap cost, gap recovery, metadata pressure, and
   slow-consumer behavior to the existing incremental performance gates.

Together these ideas support the existing-format-first proposal. The additional
contracts belong initially in the incremental session, publication receipt, and
optional live protocols. None of the surveyed designs establishes that VTR needs
to replace its final directory or complete transaction blocks to provide live
consumption.

## 5. Distinguish bytes, observations, and time

Use the following terms in the writer and reader interfaces.

| Term | Meaning |
| --- | --- |
| Recording identity | The existing file identity. Every cursor and publication belongs to one recording. |
| Verified prefix | The contiguous bytes accepted by a reader through complete sections and their required validation. |
| Publication | A writer-confirmed section prefix whose encoding and output flush have completed. |
| Publication generation | A monotonic writer-local sequence identifying publication receipts for one recording. It is not an existing VTR wire field. |
| Snapshot generation | A reader-local sequence identifying visible query state. Different readers can batch the same sections differently. |
| Capture cut | A per-producer position in accepted capture order, used to define the scope of a publication request. |
| Producer frontier | Progress in recording-operation order, constrained by active producers. It is distinct from explicit record timestamps. |
| Waveform closure | An optional exclusive time bound `T`: later publications cannot add waveform observations at times less than `T`. |
| Observed time range | The bounds of currently visible evidence. Its maximum is not a completeness certificate. |
| Finalization | Acceptance of a valid final directory and trailer, including complete/incomplete provenance and a finalized fingerprint. |

A byte offset establishes which sections are visible. It cannot establish that
every transaction beginning before some time has arrived. Explicit record times
can precede producer time, and an owner can finish long after it begins. Never
derive transaction completeness from waveform closure, a producer frontier,
the largest record ID, or the maximum observed timestamp.

An unchanged file can mean idle execution, blocked publication, a paused process,
or a crashed process. Only the container's finalization establishes completion.
Process liveness is separate information.

## 6. Writer publication

### 6.1 Keep ordinary recording unchanged

Live publication is an opt-in writer policy. Retain current block/page thresholds,
compression selection, scalar-page selection, shared-text encoding, and final
directory construction by default. Do not introduce a clock read, notification,
or extra serialization operation for every captured value when live mode is off.

The writer owns publication policy behind a small interface. Simulator and
viewer integrations should not coordinate encoder queues, declaration flushing,
or section offsets themselves.

### 6.2 Request and observe publication

An illustrative interface is:

```text
request_publish() -> Ticket
publication(ticket) -> Pending | Published(Receipt) | Failed(Error)
wait_publication(ticket, cancellation) -> Receipt | Error

Receipt {
    recording_identity,
    publication_generation,
    prefix_end,
    section_count,
    waveform_closed_before: optional timestamp,
    deferred_captures,
    finalization: optional { file_end, fingerprint, completion },
}
```

`prefix_end` is exclusive and lies after a complete section. A receipt becomes
visible only after all encoding jobs through that position have been written
in submission order and the output buffer has been flushed successfully. It is
not an `fsync` promise. A receipt may cover declarations without data, and may
report no new sections. Failure must not advance the advertised prefix.

`prefix_end` and `section_count` describe the data/declaration sections before
DIRECTORY. Close and seal publish terminal status only after flushing the
directory and trailer; `finalization` then supplies the exact final file extent
and fingerprint. This lets a remote follower finish without inferring completion
from silence. The reader independently verifies terminal bytes.

A request captures accepted producer positions using the channels' publication
ordering. It ingests through those positions, drains operations eligible under
the producer frontier, and selects an encoding cut. Later captures cannot keep
extending the request indefinitely. Operations held beyond the frontier remain
deferred and are reported as such; the request does not force their processing.
Producer registration and completion must participate in this ordering.

`Published` therefore means the eligible prefix selected for that request is
available. It does not mean every operation accepted anywhere in the process
has reached the file. If a simulator needs all observations before a particular
time, it must advance every participating producer and then request publication.

Coalesce overlapping requests and bound retained tickets. A newer receipt may
satisfy several requests, but must not claim a request's capture cut before
ingestion reaches it. Preserve `flush()` as the explicit blocking operation;
document its exact relationship to publication rather than silently turning it
into a request that can still be pending on return.

### 6.3 Safe points and same-time updates

Prefer normal publication at a producer frontier that closes earlier waveform
times. Do not advance a producer, fabricate a sample, end a transaction, or split
a clock stretch merely to satisfy a viewer timer.

The format permits same-time updates in emission order, including an adjacent
block starting at its predecessor's end time. Explicit publication of an eligible
current timestamp can therefore expose a prefix without certifying that timestamp
as closed. The reader must support this case. Report waveform closure only when
producer progress and the published encoding cut jointly prove it.

This distinction allows a paused simulation to expose available observations
without promising future stability at its current timestamp. For multiple
producers, stalled progress still limits eligible observations. Publication must
respect the same assembly ordering as ordinary recording.

### 6.4 Bounded latency without tiny blocks everywhere

Add a configurable maximum age for eligible buffered data, alongside existing
size thresholds. Start with a tunable policy; choose defaults from measurements.
An interval on the order of a few hundred milliseconds is an experiment, not a
format guarantee.

In threaded mode, the worker services publication deadlines during idle periods
as well as capture activity. No subsequent signal change should be required to
flush eligible data or pending declarations. Encoder completion and flushing of
ready output must also progress while capture is idle.

Inline mode creates no worker thread. Its caller must service publication at
simulation safe points or explicitly flush before pausing. State this difference
instead of promising autonomous wall-clock publication from an inactive caller.

Avoid holding the simulation thread while compressing a requested update in
threaded mode. Retain bounded encoding admission, submission-order publication,
and error propagation. Requests cannot overtake a slow earlier encoding job.
Expose queueing delay, frontier stalls, oldest eligible data age, and last
publication age so consumers can explain missed latency targets.

Apply backpressure through the existing bounded recording machinery. A slow
viewer must not create an unbounded notification queue or block ordinary file
recording. Notifications can be coalesced to the newest publication receipt.

## 7. Incremental reader

### 7.1 Separate mutable ingestion from immutable queries

Introduce an incremental session that produces immutable query snapshots:

```text
open_incremental(source, options) -> Session
Session.refresh(work_budget) -> Waiting | Unchanged | Advanced(ChangeSet)
Session.snapshot() -> optional Snapshot
```

Errors, cancellation, and source replacement are explicit outcomes in the actual
language bindings. The session owns scanning, declaration extension, index
updates, and finalization. A snapshot owns everything needed to keep its views
valid. Before the header and META are available, the session has no queryable
snapshot. Serialize refresh calls within a session; immutable snapshots can be
queried concurrently. Rust and C++ should expose equivalent ownership semantics.

Keep ordinary finalized opening as a direct path through the existing directory
and memory mapping. Share parsing and validation implementation where useful,
without forcing finalized queries through refresh bookkeeping or growing-file
storage.

### 7.2 Read growing files safely

Start with positional reads and a bounded cache of immutable byte chunks. Retain
the opened file handle and address data by section offset. Do not remap the
whole file on every refresh or expose pointers into storage that refresh can
resize. Retained payloads own their bytes or a storage handle with a sufficient
lifetime.

The source contract is one serialized append writer, no modification of existing
bytes, and no preallocation that exposes unwritten section bytes as file length.
Sequential file length is an availability bound, not a publication receipt.
Checksums and structural validation still determine accepted evidence.

This first implementation targets local files with coherent positional reads.
Document the supported filesystem behavior. Remote consumers should use a source
adapter with explicit immutable ranges and publication receipts rather than
assuming every shared filesystem has those semantics.

At refresh, compare the path's file identity and recording header with the opened
source. Replacement, truncation, identity mismatch, and unsupported versions
require an explicit new session or error; never splice a new recording into an
old one. Handle a missing path distinctly from the still-open file. Positional
I/O converts truncation into a read error rather than a mapping fault. Arbitrary
in-place modification violates the source contract; metadata checks do not
pretend to detect every such modification.

### 7.3 Resume a contiguous verified prefix

Maintain a scan offset, pending section state, declaration extents, block
ordinals, and validation state. A refresh samples an available extent, optionally
bounded by a writer receipt, and advances from the previous offset. A terminal
receipt permits inspecting through its final file extent rather than stopping
at its data prefix:

1. Wait if a file header, META dependency, section header, or payload is short.
   Initial attachment before META must not manufacture metadata or fail merely
   because the writer has not published it yet.
2. Check section kind, flags, length arithmetic, limits, and dependency order
   before allocating or decoding.
3. Verify the section checksum when present and apply required structural
   validation. CRC zero retains its existing meaning; live mode does not claim
   stronger integrity than the recorded format provides.
4. Stage declaration and index changes. Expose the section only when all of its
   required updates succeed. Advance the visible prefix at section granularity.
5. Stop at the work budget or unavailable suffix. Return committed changes even
   when the following section still needs more bytes.

Wait for the declared payload extent before starting whole-section checksum work,
or retain a checksum cursor when verification spans several budgeted calls.
Do not reread the same partial payload on every timer tick. A large valid section
must be processable across several calls without being rejected just because it
exceeds one call's budget. Memory limits, decompressed payload limits, and work
budgets are separate controls.

With the append contract satisfied, a fully available section with a bad checksum
or invalid payload is an error. A writer receipt makes that error conclusive
within its advertised prefix. Never skip the section, wait forever for it to be
rewritten, or publish pages salvaged from it. Keep the last good snapshot
available for inspection and report the failing offset.

Existing sections are not reverified on every refresh. Full-file validation
remains a separate operation. Do not weaken checksum policy to obtain a faster
incremental benchmark: checking each new section once still reads its compressed
payload, even if only one signal is selected.

### 7.4 Extend indexes and retain lifetimes

Declaration chunks extend dense ID namespaces. Validate each chunk against the
prefix in which it appears; a later declaration cannot repair an earlier forward
reference. Extend hierarchy child lists, source catalogs, datatype lookup, and
signal tables without rebuilding all previous declarations.

Preserve waveform ordinals among SIGNAL_BLOCK sections and record block numbers
across transaction and located-record blocks. Previous-dirty links remain valid
when more waveform blocks arrive. Runtime references unresolved in one snapshot
may resolve in a later snapshot without altering the referenced records.

Use shared immutable chunks or another append-efficient representation for
snapshots. Rebuilding full vectors, hierarchy tables, or schema copies on every
publication would turn small updates into quadratic work. Retaining many
snapshots has a cost: account shared storage once, expose memory use, and release
old chunks when readers release their snapshots.

Immutable decoded records and pages remain cacheable. Negative lookups, missing
parent/child results, relation searches, and time-window query results depend on
the snapshot generation. Invalidate or extend them when relevant sections arrive.
Never retain an unresolved-reference result forever merely because its record ID
has not changed.

### 7.5 Finalization without reopening

DIRECTORY is a terminal candidate. If its payload or trailer is incomplete,
retain the prefix and wait; do not scan directory bytes as subsequent sections.
Accept finalization only when its checksum, entries, flags, exact file length,
and trailer agree with the observed sections and recording identity. Check the
existing directory invariants, not just the trailer magic.

Publish finalization as a snapshot change even if no new data records arrived.
Expose `Complete` or `Incomplete`, dropped-byte provenance, and the finalized
fingerprint. Run the required complete-recording reference checks before claiming
semantic validation; acceptance of a directory is not itself a full validation
report. An unsealed crashed writer remains `Unfinished`.

A partial final directory does not make its bytes permanently dropped. Preserve
the distinction between a waiting suffix and bytes discarded by explicit recovery.
After accepted finalization, additional bytes violate the recording contract.

## 8. Changes consumed by viewers and remote clients

A change set should describe progress in physical recording order, with bounded
summaries of affected data:

```text
ChangeSet {
    recording_identity,
    previous_snapshot_generation,
    snapshot_generation,
    verified_prefix_end,
    added_declaration_ranges,
    added_waveform_block_range,
    added_record_sections,
    affected_time_ranges,
    completion_change,
}
```

These are interface fields, not new VTR wire fields. A writer publication
generation is optional source metadata and is never substituted for the reader's
snapshot generation. Whole signal/generator sets can be derived lazily from new
block indexes; do not decode every page just to announce that it arrived.

Consumers apply each change once against the matching previous generation. If
notifications are lost or coalesced, obtain a change set covering the missing
physical range or rebuild the affected view from a current snapshot. Use section
and block positions as cursors, not timestamps or the largest record ID.

### Waveforms

Load the initial selected signal history normally, then decode only added blocks
that can affect it. Store updates in immutable chunks so appending a small suffix
does not copy the full signal history. Preserve emission order for same-time
updates across block edges, including event occurrences and repeated values when
duplicate suppression is disabled. A repeated refresh must add nothing twice.

Use existing group frames and previous-dirty links to initialize a newly selected
signal or a query beginning late in the recording. Handle signals declared after
earlier blocks, aliases, new signals joining an existing group, and blackout
intervals according to the existing specification. New blocks do not justify
inventing changes for an unchanged signal.

### Transactions, located records, logs, and relations

New sections can introduce evidence anywhere in the displayed time range. Merge
records by stable identity and update time indexes; invalidate affected windows
even when their timestamps precede the visible right edge. Do not append only
records whose timestamps exceed the last refresh time.

Resolve newly available parents, child blocks, relation endpoints, and clocks.
Previously unresolved references remain explicitly missing until the evidence
arrives. Preserve storage-order visitor contracts; a chronological table can
maintain its own ordering. Notifications may describe conservative affected
ranges rather than forcing all payloads to be decoded during refresh.

### Consumer integration

The viewer's session interface needs a refresh result and incremental signal and
track loading. Update trace metadata, hierarchy counts, loaded histories, table
indexes, and navigation as one consumer generation. Retain user selection and
viewport; following the newest time is an explicit viewing policy.

For remote viewing, send publication/change metadata and request new immutable
ranges or query pages under a snapshot token. Bound outstanding requests, support
cancellation, and reject replies belonging to an old recording or generation.
Reconnect through recording identity and a verified physical cursor, with a new
session when the source has changed. A raw-file client can work without any
writer notifications by polling and scanning the suffix.

Live activity summaries belong to a separately accounted, generation-bound
session cache. The existing persistent VTI identity requires a finalized
fingerprint. Do not fabricate one for an unfinished recording. After finalization,
reuse only verified reusable work or build the normal sidecar; do not label a
prefix summary as a complete final index.

## 9. Optional previews of active state

Incremental loading of published facts is useful without showing active owners.
For interactive transaction debugging, add an optional live overlay for owners,
stages, and clock stretches that have not yet been finalized into normal blocks.

Prefer a separate live channel or sidecar over inserting mutable records into
the main recording. Keep ordinary transaction encoding and duplicate-ID rules
unchanged. A provisional view is not another durable record with the same ID.
Its interface should provide:

- Recording identity, a publication receipt, and a monotonic overlay revision.
- Stable record IDs, source and parent references, and explicit active state.
- Payload availability, including mandatory payloads deferred until completion.
- Changed embedded details, child locators, and clock parameters when available.
- An observation cutoff for drawing an active interval; this is not its end time.
- A bounded initial active-state snapshot followed by updates and removals, with
  a resnapshot operation after a revision gap.

Capture an overlay revision consistently with the backend's selected publication
cut. It must not reference declarations unavailable in its paired snapshot.
If newer active state is intentionally exposed, publish its declarations first
and specify the corresponding later cut rather than mixing snapshots implicitly.

When an owner ends, its final record may still wait for encoding. Retain enough
provisional state to avoid a disappearing interval, and remove it only when the
paired main-file prefix includes the final record. When consumers apply that
prefix and overlay revision, replacement by the final record is atomic. Keep
finished-but-unpublished previews bounded; under pressure report unavailable
preview state and require resynchronization rather than blocking recording.

For clocks, derive preview edges only through the paired observation cutoff.
Do not extrapolate execution beyond observed progress, create one record per
edge, or split a compact final clock stretch for each refresh.

If the simulator crashes, previews remain provisional evidence. They never
become completed transactions because the process disappeared. The main-file
close, seal, and recovery contracts continue to determine durable provenance.
Preview retention and crash capture require an explicit policy; a live overlay
is not automatically part of an archived trace.

## 10. Optional publication discovery and fast late attachment

Basic live reading requires no sidecar. A reader that already has a cursor scans
only the suffix. An initial attachment to a very large unfinished recording is
different: it lacks the final directory and may need to inspect a large prefix.

Measure that case before adding an index protocol. If needed, introduce an
optional publication sidecar with directory-like entries for completed sections
and a small publication head. Its design must include:

- Independent magic and version, recording identity, publication generation,
  section count, and exclusive main-file prefix extent.
- Entries in physical order with section kind, offset, length, bounds, and block
  number, checked against the main-file headers and declaration ordering.
- Checksummed, bounded delta batches. Never append a full cumulative directory
  at every publication.
- A self-contained initial index and bounded lookup strategy for a late reader;
  replacing the initial VTR scan with an unbounded index-log replay is insufficient.
- An atomic publication head, such as a checked temporary file renamed into
  place, only after the corresponding main-file flush completes.
- Explicit handling of stale, missing, truncated, corrupt, and mismatched
  sidecars, with fallback to main-file scanning.

An index of immutable pages with a published root is one candidate. Another is
delta batches with geometrically spaced checkpoints, so cumulative checkpoint
work stays proportional to index growth. Compare their lookup, write amplification,
and crash behavior before selecting a sidecar layout.

A sidecar is an accelerator, not independent proof that data is valid. It can
avoid payload reads for section discovery, but cannot remove whole-section CRC
I/O under the same eager verification policy. If lazy payload verification is
introduced, name that integrity policy and compare it separately; do not claim
identical verification with lower I/O.

Keep this sidecar distinct from VTI activity summaries and active-state previews.
They answer different questions and have different lifetimes. They can share a
publication token without requiring consumers to load all three.

## 11. Format decisions and rejected alternatives

The initial implementation changes interfaces and writer policy, not VTR bytes.
Keep the 24-byte section header, payload checksums, declaration ordering,
waveform frames, transaction pages, final directory, and trailer unchanged.

| Alternative | Reason to defer or reject |
| --- | --- |
| Reopen and reconstruct a reader on every update | Repeats verification, declaration decoding, and index construction; invalidates consumer state. Useful only as a test oracle. |
| Remap the entire growing file on every update | Risks view lifetimes and makes truncation hazardous; old mappings can accumulate while snapshots survive. |
| Rewrite section headers to mark publication | Unnecessary with complete-section validation and optional receipts; introduces mutation into an append-only contract. |
| Emit a full directory and trailer at every publication | Conflicts with terminal-directory semantics and adds repeated cumulative metadata. |
| Replace compact transactions with begin/update/end operations | Adds reconstruction and mutable-record complexity to ordinary postmortem queries. |
| Shorten every block globally | Charges all recordings for live latency and can reduce compression and query efficiency. |
| Copy all signal values at every publication | Penalizes sparse designs; existing group frames and links already support selective initialization. |
| Treat a checksum failure as a temporary tail forever | Hides actual corruption and can prevent progress indefinitely. |
| Disable checksums for live mode | Changes evidence integrity and obscures the real cost of the design. |
| Infer transaction completeness from a time watermark | Incorrect for explicit past timestamps, delayed owners, and later references. |

A later wire change requires a measured benefit and a normative specification
update. For example, independently checked subblocks could reduce the latency
and verification cost of very large sections, but add framing and integrity
overhead. Do not add them before measuring the existing section-size tradeoff.

Version every new live protocol independently. If VTR or VTI bytes eventually
change, bump the relevant format version and regenerate old assets according to
project policy; do not add a compatibility reader for historical formats.

## 12. Implementation sequence

1. **Specify and test prefix ingestion.** Extract reusable section/declaration
   validation, implement budgeted positional scanning and initial waiting states,
   and establish immutable snapshots. Preserve finalized opening as a direct path.
2. **Make queries incremental.** Extend hierarchy and block indexes, retain schema
   and payload lifetimes, and expose physical change sets. Demonstrate that work
   does not grow with the already loaded prefix on idle or small refreshes.
3. **Add writer publication.** Implement capture cuts, receipts, asynchronous
   requests, a completion barrier, bounded request state, and optional deadlines.
   Verify Rust/C++ ordering, idle progress, errors, and producer stalls.
4. **Integrate consumers.** Extend loaded signals and tracks, handle old-time
   arrivals and resolved references, publish finalization, and add remote resume.
5. **Add active previews.** Implement coherent revisions and handoff to final
   records through a separately versioned optional live protocol.
6. **Optimize measured bottlenecks.** Add late-attachment discovery or other
   format work only when measurements justify its complexity and overhead.

Each stage should be independently useful. Basic attach and refresh must not
depend on provisional transactions, a running writer connection, or an activity
sidecar. Publish the supported capabilities so a viewer can explain whether it
is showing finalized facts, active previews, or a waiting prefix.

## 13. Verification and performance gates

### Behavioral and interoperability tests

Exercise both language writers with both readers through public interfaces.
Compare each incremental snapshot with a fresh immutable reader over the same
verified prefix, and compare the final session with an ordinary finalized open.
Use semantic equivalence rather than byte identity when flush schedules change
block membership.

Cover the following nontrivial cases:

- Empty input, partial file header, missing META, declarations without data, and
  each byte cut through section headers, payloads, directory, and trailer.
- Repeated idle refresh, canceled budgeted verification, oversized valid sections,
  malformed lengths, checksum failures, unknown optional/required kinds, and
  semantic errors that appear only in later sections.
- Same-time samples across publications, repeated event occurrences, duplicate
  suppression on/off, 2-/4-/9-state signals, reals, variable-length values,
  aliases, sparse groups, and later signals joining existing groups.
- Scalar/general transaction pages, shared text, logs, relations, owners ending
  out of timestamp order, located children preceding owners, and references
  resolving after negative results were cached.
- Delayed encoders, out-of-order encoder completion, stalled producers, producer
  registration/completion races, concurrent capture during a request, idle
  deadlines, inline pause, backpressure, and output errors during publication.
- Old snapshots and retained payloads surviving refresh, stable declaration IDs,
  bounded request retention, cancellation, and release of shared storage.
- Source replacement, truncation, stale notifications, sidecar corruption,
  revision gaps, remote reconnect, and cache rejection across recording identities.
- Complete close, incomplete seal, crash without sealing, partial finalization,
  and final-only invariant failures such as unresolved required references.
- Active clocks and deferred owner payloads, completion waiting for encoding,
  atomic preview replacement, and crash provenance when previews are enabled.

Randomize capture, publication, refresh, and cancellation schedules while keeping
the producer ordering contract valid. Repeated refresh must be idempotent, and
every observed prefix must preserve the order and identity of its evidence.

### Measurements

Run the full default native interoperability and benchmark suites required by
[AGENTS.md](../AGENTS.md) whenever implementation or format work lands. Compare
every existing workload and metric against the committed baseline, including
producer cost, write/read throughput, file size, peak memory, selective queries,
and final drain/close. Investigate unexpected regressions and repeat noisy runs.

Add incremental workloads covering dense and sparse waveforms, large mostly idle
hierarchies, declaration growth, long-lived owners and clocks, past-time record
arrivals, large payloads, slow readers, and several producer/encoder counts.
Measure cold and warm attachment, idle polling, and a reader falling far behind.

Report publication latency and capture-to-visible latency separately, including
median and tail distributions, deadline misses and their causes. Also measure
bytes read per refresh, already verified bytes reread, decoding work, allocations,
snapshot retention, preview/index memory, simulator slowdown, notification traffic,
and recovery/finalization time. Use equal checksum policies and include all worker
CPU, output flushing, and final completion costs.

Acceptance requires evidence that:

- Live-disabled defaults retain existing postmortem performance and semantics.
- Idle polling performs bounded metadata work without rereading old payloads.
- Refresh cost tracks new evidence and affected indexes, not total history size.
- Loaded signal extension does not repeatedly copy the full signal history.
- Memory stays within configured cache/queue budgets plus explicitly retained
  snapshots, declarations, indexes, and active state; those costs are reported.
- Live-enabled recordings remain ordinary valid postmortem files, with the
  measured latency/compression/throughput tradeoff documented.
- Finalized query results match the same source observations under different
  valid publication schedules in both independent implementations.
