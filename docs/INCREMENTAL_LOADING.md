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

This is an implementation proposal, not evidence that a new format is necessary.
The [reuse assessment](#49-can-an-existing-format-replace-vtr) compares existing
formats with the broader FSDB/VWDB replacement goal and defines the experiments
needed before choosing a storage backend.

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

The examples below show logical observations and representative stored records,
not byte layouts or screenshots of vendor viewers. Each SVG includes its editable
diagrams.net model. Example timestamps use a small relative scale for readability;
they do not establish the format's simulation-time precision.

A trace can describe several different kinds of evidence:

- **Occurrences:** a request was issued, an allocation was sampled, or a log
  message was emitted. Repeated identical occurrences must remain distinct.
- **Intervals:** work ran between two times. The file may store one completed
  interval, or separate lifecycle events which an analyzer joins.
- **State observations:** a counter or symbolic state changed. Inferring a held
  value between observations requires a defined state model and an initial value.
- **Relationships:** an event belongs to a thread, a stage belongs to a request,
  or one operation caused another. Thread IDs, nesting stacks, activity IDs,
  and directed relations express different relationships.
- **Decoding context:** schemas, strings, clocks, threads, stacks, and object
  descriptions. These are records too, and a reader needs them to interpret
  observations that reference them.

All five can record application-defined information. An arbitrary payload's
ability to carry bits is weaker than an existing reader knowing those bits are
a 512-bit HDL bus with `X`/`Z`, aliases, and a value held until the next change.
The comparison below separates those two levels of support.

### 4.1 Perfetto: early span evidence and recoverable sequence state

Perfetto records system and application behavior for performance analysis and
debugging. Its native protobuf format can combine application work, operating
system activity, profiles, resource measurements, and diagnostic context in one
recording. `TrackEvent` is its general application timeline model, not the whole
format. [Perfetto overview](https://perfetto.dev/docs/)

This capability review covers upstream schemas and documentation reviewed
2026-10-29. The format is extensible: the catalog below covers its defined feature
families and extension mechanisms, while linked schemas specify individual
fields. Producer, SDK, Trace Processor, and UI versions determine which features
work end to end. The packet schema distinguishes stable public interfaces from
context-specific and experimental payloads; merely finding a message in that
schema is not a stability guarantee.
[Packet schema and stability boundaries](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto)

#### Recording, transport, and timeline vocabulary

The terms below introduce the stored data first, then clocks and timestamps,
timeline tracks, writers, and live capture roles. Perfetto encodes its records
using Protocol Buffers, a binary encoding whose fields are defined by schemas.

| Concept | Definition |
| --- | --- |
| **Trace** | The whole recording: captured activity and the declarations needed to interpret it. It can combine data from several processes and machines. |
| **Trace packet (`TracePacket`)** | One unit of the recording. It can contain a declaration, an application event, or a batch of operating-system events, together with fields such as timestamps and source identifiers. One packet is not necessarily one transaction or observation. The file's top-level `Trace` schema stores a list of these packets in its repeated `packet` field. Writers can append entries and readers can process them as they arrive, without first assembling the whole recording in memory. |
| **Clock / clock domain** | An identified timebase: it defines the origin and units of timestamp values. Two clocks can assign different numbers to the same moment. Perfetto supports built-in host clocks and application-defined clocks; a clock here is a way to interpret time, not an HDL signal toggling between 0 and 1. |
| **Timestamp unit / tick period** | The duration represented by one timestamp count. The default is 1 ns. A clock can specify an integer `unit_multiplier_ns`: with a multiplier of 1000, a timestamp of 3 means 3000 ns, or 3 µs. This is a unit scale, not a modeled oscillator's period. The field cannot express fractional nanoseconds, such as a 1 ps simulation tick, or an arbitrary rational clock frequency. |
| **Timestamp** | A numeric reading of a clock. `TracePacket.timestamp` is an unsigned 64-bit integer; `timestamp_clock_id` selects its clock, with defaults available when omitted. Values are absolute readings by default; a custom clock can instead use differences from earlier readings to save space. The timestamp describes when the recorded activity happened, not when its packet reached the file. |
| **Host elapsed time** | Time measured on the computer running the application. Built-in clocks include `CLOCK_MONOTONIC`, which excludes system suspend, and `CLOCK_BOOTTIME`, which includes it. These measure elapsed time rather than calendar dates. Simulator execution costs can be measured on this axis. |
| **Wall-clock time** | Calendar time, represented by `CLOCK_REALTIME` as time since the Unix epoch. It can be adjusted by clock synchronization or the user, so it is distinct from a monotonic elapsed-time clock. A trace need not use wall-clock time. |
| **Simulation / model time** | Time advanced by the simulator, independently of how long the simulation takes to execute. Perfetto has no dedicated simulation-time type. An exporter can supply model-time timestamps on a synthetic recording timeline or an application-defined clock. Such values do not acquire a relationship to host or wall-clock time automatically. Exact sub-nanosecond timing needs a separate representation decision because of the unit-scale limit above. |
| **Clock snapshot (`ClockSnapshot`)** | A record pairing readings from several clocks at a synchronization point: for example, elapsed time 5 s corresponds to a particular wall-clock date. An analyzer uses these records to translate timestamps between clocks. A snapshot can also declare a clock's timestamp unit and whether later timestamps use differences. |
| **Trace time** | The common timeline onto which Perfetto's analyzer, **Trace Processor**, converts timestamps for comparison and duration calculations. Multiple input clocks do not create independent time axes in the viewer. Current upstream supports a synthetic trace-file clock (`BUILTIN_CLOCK_TRACE_FILE`); host traces commonly use `CLOCK_BOOTTIME`, and clock metadata can select the primary clock. Clock snapshots must provide the relationships needed to align different input clocks. |
| **Track** | An identified timeline lane for activity or measurements: for example, an OS thread, a device queue, a particular request, a numeric measurement, or a symbolic state. A track identifies what activity belongs to; it need not correspond to a host thread. |
| **Track descriptor (`TrackDescriptor`)** | A declaration giving a track its unique identifier, name, optional parent track, and other properties. As with a VTR declaration, later observations refer to the identifier rather than repeating the description. Different tracks can have the same display name. |
| **Track event (`TrackEvent`)** | An observation assigned to a track: an operation begins or ends, something happens at one point in time, or a value changes. For example, begin and end events on `DMA engine 0` can describe a transfer from model time 10 ns to 40 ns. Such a timed operation is called a **slice**, with a duration of 30 ns in this example. A track event is one possible packet payload; other packet types describe other kinds of data. |
| **Trace writer** | A component that emits packets. An instrumented process can have several writers, for example one per worker thread. A writer can report activity on several tracks, and several writers can report activity on the same track if they preserve consistent timestamps and begin/end ordering. |
| **Packet sequence** | The ordered packets emitted by one writer. They can share encoding state: for example, an earlier packet defines a short numeric reference for `DMA transfer`, and later packets use that reference instead of repeating the string. The reader must remember that definition. Packets from different writers can interleave; `trusted_packet_sequence_id` distinguishes their sequences. A sequence identifies which writer's definitions are needed to decode a packet, while a track identifies where the activity belongs. |
| **Tracing service** | Coordinates live recording sessions, tells processes what to collect, and gathers their data into buffers or output. A simulator can generate a valid Perfetto file directly without using this service. |
| **Producer** | A participant supplying data to the tracing service, usually an application process with tracing instrumentation. It uses trace writers to emit packets. |
| **Data source** | A named collection facility offered by a producer, such as application events, kernel events, or a profiler. Recording configuration selects and configures these facilities. One producer can offer several data sources, and one data source can report activity on many tracks. |
| **Consumer** | A client that starts or controls a recording session and reads data from the tracing service. A viewer opening an already saved file does not need to take this role. |

Sources: [trace file schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace.proto),
[packet schema and sequence fields](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto),
[track descriptors](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_descriptor.proto),
[track event generation](https://perfetto.dev/docs/reference/synthetic-track-event),
[clock synchronization](https://perfetto.dev/docs/concepts/clock-sync),
[clock metadata](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/clock_snapshot.proto),
[built-in and trace-file clocks](https://github.com/google/perfetto/blob/main/protos/perfetto/common/builtin_clock.proto),
[host clock semantics](https://man7.org/linux/man-pages/man2/clock_gettime.2.html),
[service model](https://perfetto.dev/docs/concepts/service-model).

**Implication for VTR.** A simulation-only export can show model time from
simulation start without assigning any UTC date. Combining it with simulator performance
data requires more care: advancing the model by 1 µs may take 100 ms of host time,
and a paused simulator advances no model time at all. Clock synchronization does
not make these two axes interchangeable. A VTR mapping must decide which axis
defines event positions and durations, and how to retain observations relating
model progress to execution time. Likewise, an HDL clock with a 10 ns period is
modeled activity on the simulation timeline; declaring a Perfetto timestamp clock
does not record that signal's edges or implement VTR clock-stretch records.

#### Timeline primitives: what an observation means

![Nested frame and layout slices, a flow to worker decode, a queue-depth counter, and symbolic player states, with representative Perfetto packets.](diagrams/reference-perfetto.drawio.svg)

| Primitive | Definition and representation | Example |
| --- | --- | --- |
| **Slice** | A timed operation formed by `TYPE_SLICE_BEGIN` and `TYPE_SLICE_END` on one track. Pairing follows that track's nesting stack; the end does not need to repeat the beginning's name. | `RenderFrame` contains a `Layout` slice. |
| **Instant** | A point occurrence with no duration, represented by `TYPE_INSTANT`. It may occur inside a slice but does not update a held value or close the enclosing slice. | A `vsync` notification. |
| **Counter** | A numeric observation on a counter track, represented by `TYPE_COUNTER`. Values can be signed 64-bit integers or doubles. A sample is not an individual request or an HDL vector declaration. | Queue depth changes from 0 to 2 to 1. |
| **State** | A symbolic held value on a state track. Each `TYPE_STATE` update replaces the previous state; clearing the state marks the track idle. The value persists until another transition. | `Buffering` becomes `Playing`, then `Paused`. |

[TrackEvent schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_event.proto),
[state-track semantics](https://perfetto.dev/blog/perfetto-v57).

A **thread track** associates synchronous activity with an OS thread. A **process
track** associates activity with a process. A **custom track** describes a logical
resource or operation without requiring either association. Asynchronous work
can begin on one host thread and end on another by emitting both observations
on the same custom track. Multiple independently overlapping requests need
separate tracks or another unambiguous allocation scheme; correlation IDs do
not change begin/end pairing. Slice nesting on one track requires children to
end before their enclosing slices.
[Track associations and async generation](https://perfetto.dev/docs/reference/synthetic-track-event)

Modern `TrackEvent` intentionally represents slices as begin/end evidence rather
than a single complete-duration event. This allows the beginning to be recorded
while work is active. It does not make every Perfetto payload a begin/end pair:
specialized schemas can store durations directly, and legacy event fields can
carry complete events. For example, `GpuRenderStageEvent` carries a duration,
an event ID, and dependencies on other GPU events. Therefore, a restriction of
`TrackEvent` is not a restriction of the entire protobuf format.
[TrackEvent design](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_event.proto),
[GPU render-stage records](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/gpu/gpu_render_stage_event.proto)

A **counter descriptor** supplies units, scaling, optional built-in meaning such
as thread CPU time or instruction count, and display properties such as a shared
y-axis key. Some counters can use sequence-local deltas instead of absolute
values. A slice or instant can also carry **extra counter values** sampled at its
timestamp, avoiding separate event packets for each observation. Counter units
and timestamp clock units are different declarations.
[Counter descriptor](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/counter_descriptor.proto)

#### Identities, grouping, and causality

These identifiers are not interchangeable:

| Concept | What it identifies or connects | What it does not establish |
| --- | --- | --- |
| **Track UUID (`track_uuid`)** | The timeline receiving the event. `TrackDescriptor.uuid` declares it. Zero denotes the implicit trace-global track. | The identity of each slice on that track. |
| **Parent track UUID (`parent_uuid`)** | A structural association between tracks, used for grouping and inherited context. | Runtime ownership of one transaction by another, or temporal containment of their intervals. |
| **Slice nesting** | An enclosing/inner operation relationship determined by the begin/end stack on one track. | An arbitrary parent pointer between independent records on different tracks. |
| **Correlation ID** | An opaque label grouping slices belonging to the same logical operation. It can be a `uint64`, a string, or an interned string reference. | An execution order, causal edge, begin/end matching key, or universally unique event ID. |
| **Flow ID** | A trace-global identifier connecting an ordered chain of participating events through `flow_ids`. Timestamp order determines the direction, allowing a handoff across tracks or threads. | Transaction ownership, an arbitrary relation kind, or evidence that uninstrumented intermediate work was captured. |
| **Terminating flow ID** | An entry in `terminating_flow_ids` marking the end of a flow. After termination the ID can be reused for another flow. | Completion of the enclosing slice or every slice sharing a correlation ID. |
| **Interned ID (`iid`)** | A lookup key into a particular interned-data table in the relevant sequence and decoding-state lifetime. | A track UUID, correlation value, or durable recording-wide record identity. |
| **PID/TID and machine ID** | Execution context identifying processes, threads, and machines. | A request identity; one request can move between threads and machines. |

[Identity and relationship fields](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_event.proto),
[descriptor relationships](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_descriptor.proto),
[interned tables](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/interned_data/interned_data.proto).

For example, consider two phases of request `req-7` and an independent diagnostic
observation about that request. The following is a logical event listing, not
literal protobuf syntax:

| Time | Track | Observation | Meaning |
| ---: | --- | --- | --- |
| 10 | Main | Begin `Submit`, correlation `req-7`, flow 42 | Starts a slice and a causal handoff chain. |
| 12 | Main | End | Closes `Submit` through the Main track's stack. |
| 20 | Worker | Begin `Decode`, correlation `req-7`, terminating flow 42 | Starts a different slice; the flow connects it to the submitting work. |
| 30 | Worker | End | Closes `Decode`; no request ID is needed for pairing. |
| 35 | Diagnostics | Instant `Inspect`, argument `request="req-7"` | Attaches request context without claiming it caused either phase. |

The shared correlation ID groups `Submit` and `Decode` as one logical operation.
Flow 42 additionally asserts a handoff between them. Another slice may share
`req-7` without joining that flow. A diagnostic argument containing the same
string is ordinary payload data, not automatically a correlation-field value.
Applications choose their identifier namespaces; none of these fields by itself
provides VTR's validated global record-ID and typed-relation model. Trace
Processor's imported row identities should not be substituted for producer-issued
record IDs in an interchange contract.

Descriptors also carry names, descriptions, child/process/thread ordering hints,
sibling merge policies and keys, and custom **dimensions** such as resource rank
or stage. Dimensions classify tracks and participate in their identity/merging
rules. They differ from per-event arguments. Display merging may put several
logical tracks in one visual group without changing the original event meaning.
Some process/thread associations override simple parent-based nesting; a
`parent_uuid` is not a promise of a particular UI tree layout.
[Track grouping, ordering, merging, and dimensions](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_descriptor.proto)

#### Names, categories, arguments, callstacks, and extensions

An **event name** labels an observation, such as `Decode`. A **category** groups
instrumentation for capture configuration, such as `render` or `memory`; it is
not an ownership hierarchy. Names and categories can be inline or interned.
**Arguments** describe the individual observation: request address, byte count,
status, or structured application data.
[Track event instrumentation](https://perfetto.dev/docs/instrumentation/track-events)

There are two general routes for application payloads:

- **Debug annotations** are named values supporting booleans, signed/unsigned
  integers, doubles, pointers, strings, and nested dictionaries/arrays. They are
  useful without defining a new protobuf schema; some values can be interned.
  [Debug annotations](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/debug_annotation.proto)
- **Typed protobuf extensions** attach application-defined fields and nested
  messages to `TrackEvent`. An `ExtensionDescriptor` packet can embed the protobuf
  descriptor set, making the schema travel with the recording. With descriptors
  available, Trace Processor decodes these fields into its `args` table and the
  UI exposes them in event details. This does not require a custom importer for
  each field. Automatic argument decoding currently applies to `TrackEvent`
  extensions, not arbitrary extensions to every other message.
  Extension field numbers must avoid collisions with other schemas in the trace.
  [Typed extensions and descriptor delivery](https://perfetto.dev/docs/instrumentation/extensions)

A **callstack** describes the executing code associated with an event. It can be
inline function/source frames or a reference to interned frames, mappings, build
IDs, and symbol strings. An optional **callstack weight** attributes a numeric
amount to that occurrence for weighted analysis. A captured callstack explains
where execution was observed; it is not the same relationship as slice nesting
or a request's ownership tree.
[Callstack generation and weights](https://perfetto.dev/docs/reference/synthetic-track-event),
[profiling references](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/interned_data/interned_data.proto)

The schema also contains specialized arguments for task execution, log messages,
source locations, screenshots, Chrome scheduler/frame state, IPC, histograms,
application state, and other producer-specific observations. Many are explicitly
outside the stable generic interface. A simulator should use supported public
fields and its own typed extension rather than assigning hardware meanings to
an unrelated Chrome-specific field.
[Specialized and legacy TrackEvent arguments](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_event.proto)

For VTR reuse, typed extensions are more capable than storing a transaction as
one opaque string. They can preserve structured fields, arrays, and packed bytes
for generic inspection. A schema defining a 512-bit four-state bus still needs
to specify width, packing, aliases, and held-value semantics. Descriptor-driven
argument display does not automatically supply a waveform renderer or a
signal/time index.

#### Record families beyond application track events

The native packet format also carries the following families. Some payloads
have specialized interval, identity, and relationship semantics of their own.
Support for a family means there is a representation and matching tooling for
particular producers; it does not mean every capture records every family.

| Family | Information represented and schema examples |
| --- | --- |
| **Kernel and scheduler activity** | Linux `FtraceEventBundle`, Windows `EtwTraceEventBundle`, and generic kernel task-state, task-rename, process-tree, CPU/GPU-frequency records. These can reconstruct scheduling and execution context alongside application slices. [Generic task schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/generic_kernel/generic_task.proto) |
| **System, process, and hardware observations** | Process/thread inventories and sampled process statistics; system/CPU/GPU/interrupt descriptions; system counters, memory maps (`SmapsPacket`), cgroup memory statistics, inode/file mappings, and CPU attribution. [System statistics schemas](https://github.com/google/perfetto/tree/main/protos/perfetto/trace/sys_stats), [process schemas](https://github.com/google/perfetto/tree/main/protos/perfetto/trace/ps) |
| **Execution and weighted stack profiles** | `PerfSample`, streaming profiles, and transport-neutral `StackSample`. Samples can associate a task or asynchronous context, callstack, execution context, and measurement weights such as cycles, bytes, faults, or counts. A sample is not proof of continuous execution between sample times. [StackSample schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/profiling/stack_sample.proto) |
| **Allocations, frees, and memory graphs** | Aggregated heap snapshots (`ProfilePacket`), a streaming allocation/free representation marked for local testing, and app-reported `MemoryTrackerSnapshot` graphs with attribution edges. Managed heap graphs also exist through Android extensions. These memory-ownership edges have model-specific meaning. [Heap/profile records](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/profiling/profile_packet.proto), [memory attribution graph](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/memory_graph.proto) |
| **Symbols, runtimes, and generated code** | Module symbols, deobfuscation mappings, ART metadata, V8 JavaScript/internal/WebAssembly/regular-expression code descriptions and code moves, plus interned frames and executable mappings. These relate sampled addresses to code. [V8 schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/chrome/v8.proto), [symbolization](https://perfetto.dev/docs/learning-more/symbolization) |
| **GPU, graphics, and compute** | GPU counters, timed render/compute stages, queue/context descriptions, graphics frames, Vulkan API and memory events, GPU memory totals, and GPU logs. Render-stage records can include event IDs and explicit wait dependencies. [GPU schemas](https://github.com/google/perfetto/tree/main/protos/perfetto/trace/gpu) |
| **Power, energy, and residency** | Battery measurements, power rails, energy-estimation breakdowns, entity-state residency, frequency/power observations, and kernel/application wakelocks. [Power schemas](https://github.com/google/perfetto/tree/main/protos/perfetto/trace/power) |
| **Logs and diagnostic messages** | Android log packets, systemd journal records, GPU logs, Chrome logging arguments, and platform-specific structured logs. They retain source-specific priority, context, or payload conventions. [Android logs](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/android/android_log.proto), [journald](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/linux/journald_event.proto) |
| **Browser and platform context** | Chrome event bundles, benchmark/application metadata, display state, Android package/user inventories, properties, flags, game interventions, statsd atoms, per-UID CPU data, and modem events/token dictionaries. These provide interpretation and environment details rather than generic request lifecycles. [Android schemas](https://github.com/google/perfetto/tree/main/protos/perfetto/trace/android), [Chrome schemas](https://github.com/google/perfetto/tree/main/protos/perfetto/trace/chrome) |
| **Platform extensions** | Android heap graphs, network packets, frame timelines, input events, Bluetooth, wakelocks, and Winscope layer/transaction/shell-transition/ProtoLog data. These use extension schemas, some maintained outside the core tree; a SurfaceFlinger transaction is a platform object with its own schema, not VTR's generic transaction. [Mirrored Android schemas](https://github.com/google/perfetto/tree/main/protos/third_party/android) |
| **Recording context and control evidence** | Trace UUID, configuration, attributes and provenance; trigger/clone-trigger records; tracing-service and concurrent-session events; loss/buffer statistics; recovered-trace information; Perfetto's own metatrace and experimental producer statistics. These explain how a recording was obtained and what may be absent. [Recording/context payloads](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto) |
| **Analysis and presentation context** | UI state, track-event ranges of interest, and translation tables used to interpret/display recorded identifiers. These help a consumer present data; they are not additional waveform values or a portable SQL query index. [UI state](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/ui_state.proto), [translations](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/translation/translation_table.proto) |

The [packet payload registry](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto)
and [schema tree](https://github.com/google/perfetto/tree/main/protos/perfetto/trace)
are the field-level references for these families. Reserved tags and test-only
messages do not add application capabilities. New registered extensions can add
further families without turning them into generic `TrackEvent` semantics.

#### Clocks, encoding, and decoder state

The introductory table defines clocks, units, and trace time. At the encoding
level, `timestamp_clock_id` can be set per packet or inherited from
`TracePacketDefaults`; the packet schema specifies `CLOCK_BOOTTIME` when neither
provides it. Custom clock IDs 64–127 belong to one packet sequence, so the same
number in another writer's packets can identify a different clock. Incremental
timestamps use the preceding reading on that sequence and clock, starting from
the snapshot's absolute reading. This requires retained decoder state, just as
string references do. Timestamp deltas, counter deltas, and sampled CPU-time
counters are three different quantities.
[Packet clock selection](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto),
[clock defaults](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet_defaults.proto),
[clock encoding](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/clock_snapshot.proto)

Machine IDs and remote clock synchronization support multi-machine recordings.
A simulator must still validate exact fine-tick handling through its writer and
analyzer; relabeling picosecond ticks as nanoseconds changes physical units even
if ordering survives. The primary trace clock does not support
`unit_multiplier_ns`.
[Clock synchronization](https://perfetto.dev/docs/concepts/clock-sync),
[clock schema constraints](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/clock_snapshot.proto)

**Interning** stores repeated strings or structured descriptors once and refers
to them by ID. **Packet defaults** omit repeated fields such as the default
track or timestamp clock. Together with delta encoding, these reduce capture
size but require retained decoder state. The reader must use the correct
sequence, interned table, and reset generation; the same numeric IID can mean
something different elsewhere.
[Interned-data definitions](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/interned_data/interned_data.proto),
[packet defaults](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet_defaults.proto)

An **incremental-state reset** (`SEQ_INCREMENTAL_STATE_CLEARED`) starts a fresh
decoding-state lifetime. `SEQ_NEEDS_INCREMENTAL_STATE` identifies dependent
packets. **Loss evidence** (`previous_packet_dropped`) invalidates prior state;
current schemas can describe loss causes. `first_packet_on_sequence` helps
separate a writer's actual beginning from a retained suffix after missing data.
Decoding can resume from a suitable reset, but missing slice beginnings, held
states, and application history are not reconstructed by resetting a dictionary.
[Sequence state and loss fields](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto)

The format also supports **compressed packet batches**, using deflate or Zstd,
and **synchronization markers** that help partition long traces. A marker is
not a complete checkpoint of decoding state or open slices. Compression operates
on packet batches, not VTR-style independently indexed signal columns.
[Compression support](https://perfetto.dev/blog/perfetto-v58),
[packet framing and markers](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto)

The upstream packet schema additionally carries **ProtoVM program/state
snapshots**, allowing the corresponding decoder to reconstruct full-state
packets from patches. This is a specialized stateful encoding mechanism, not a
promise of arbitrary HDL waveform support. **Trusted source fields** record
service-assigned producer UID/PID and sequence identity. Deprecated process/thread
descriptors, legacy timestamp/CPU-time fields, and `LegacyEvent` phase/duration
fields remain part of the readable schema surface, but new application mappings should use
the supported current interfaces.
[Encoding, attribution, and legacy fields](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace_packet.proto),
[legacy event schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/track_event/track_event.proto)

The reviewed `Trace`/`TracePacket` layout has no VTR-style final directory/trailer
or mandatory payload checksum on each packet. Protobuf packet boundaries and
compression framing are useful building blocks, but do not establish VTR's
validation, finalized-file, or selective-query contracts. A reader of a growing
file still needs to distinguish a partial final packet from a completed recording.
[File wrapper](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/trace.proto)

#### Incremental consumption and implications for VTR

**Publication** means capture buffers have made data available to the consumer;
it is separate from the event's timestamp. **Retention** determines which history
survives in ring/discard buffers or streamed output. **Flush** asks producers to
commit buffered data, including sparse sources that otherwise retain old events.
These are capture policies. File length, a recent event timestamp, and an
interning reset are not interchangeable completion certificates.
[Buffering and flush behavior](https://perfetto.dev/docs/concepts/buffers)

**Trace Processor** is the analyzer importing packets into queryable tables;
**PerfettoSQL** is its query/analysis interface; **Perfetto UI** renders that
interpreted model. These are substantial reuse opportunities, not additional
on-disk event types. The processor supports repeated `Parse`, explicit `Flush`
of its sorting queues, queries over loaded data, and a distinct
`NotifyEndOfFile`. A processor flush differs from a producer/service flush.
Neither interface alone establishes that the browser tails an arbitrary growing
file. Importing other formats such as Chrome JSON also does not make every
feature of those formats a native Perfetto record.
[Trace Processor interface](https://github.com/google/perfetto/blob/main/include/perfetto/trace_processor/trace_processor.h),
[analysis API](https://perfetto.dev/docs/analysis/trace-processor)

Packet order across writers is not global timestamp order. Trace Processor's
normal bounded sorting window can reject sufficiently late observations; full
sorting trades additional memory for wider reordering. This matters for VTR
transactions completed long after their beginning or supplied with past times.
[Ordering and sorting limits](https://perfetto.dev/docs/concepts/buffers)

**Implication for VTR:** Perfetto already supplies named resource timelines,
async work through custom tracks, numeric and symbolic histories, causal flows,
operation grouping, profiles, and inspectable typed extensions. A reuse prototype
should exploit those capabilities before inventing equivalents. It still needs
an explicit mapping for VTR record IDs, runtime ownership, non-contained stages,
relation kinds, HDL values/aliases, exact simulation time, arbitrary late arrivals,
and selective signal/time queries.

For incremental loading, publish owner beginnings and stable application IDs
before completion when active work must be visible. Pair late attachment with
required declarations and an active-work/held-state snapshot. A decoder reset
alone is insufficient. Keep source finalization separate from temporary EOF,
and distinguish complete durable records from provisional observations. These
contracts remain necessary whether VTR keeps its current container or adopts
Perfetto as a backend.

### 4.2 CTF and LTTng live: packet discovery, metadata gates, and idle progress

CTF is an interchange format for timestamped execution events described by
metadata, allowing recording and analysis tools to share traces. LTTng uses CTF
to record Linux kernel and user-space activity for investigating performance,
concurrency, and failures. [LTTng documentation](https://lttng.org/docs/v2.13/)

**Records and example.** CTF separates metadata describing event classes and
binary layouts from streams containing packets of event instances. An event's
type ID selects its schema; context can identify CPU, process, and thread, while
payload fields hold observations such as an address or result. CTF 1.8.3 supports
integers, floating point, enumerations, structures, variants, arrays, sequences,
and strings. A field named `request_id` has no built-in transaction meaning:
instrumentation and analysis must agree on that meaning.
[CTF 1.8.3 specification](https://github.com/efficios/ctf/blob/master/common-trace-format-specification.md)

![Request start and end events on two CTF streams reconstruct one interval by request ID; scheduling events and schema-defined fields share the trace.](diagrams/reference-ctf.drawio.svg)

Here `request_start(id=42, address=...)` and `request_end(id=42, result=...)`
occur in different streams; the analyzer derives the request interval. A
`sched_switch` is another event class, not a request phase. An application could
instead define a completed-span event with explicit begin/end fields, or a
signal-change event with signal ID and packed logic. Those encodings can use
ordinary CTF fields; a generic reader would expose fields rather than supplying
hardware waveforms, runtime ownership, or relations.

Packet time coverage and discarded-event counts are distinct from a snapshot
of held signal values. A hardware profile would need declarations, initial
values, logic packing, same-time ordering, and absence-of-capture semantics.
That is a possible CTF application profile, rather than a limit on what its bytes
can carry. The live relay below is additional infrastructure; a custom CTF writer
does not automatically become an LTTng live producer.

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

**Records and example.** The central observation is a committed typed event:
an instantaneous event or a completed duration, with fields describing the
operation. Examples include garbage collections, file/socket operations,
allocation or execution samples, exceptions, and custom application events.
Recorded events can refer to the committing thread and a captured stack trace;
the event type describes their fields.
[Recorded event interface](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/consumer/RecordedEvent.html),
[event model](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/package-summary.html)

![Overlapping completed JFR duration events, a stack sample, and commit points; file-read and garbage-collection records carry start time and duration.](diagrams/reference-jfr.drawio.svg)

The example's file read and collection overlap without requiring a nesting stack
or shared transaction ID. Each is a separate committed event. A stack sample
is an observation at one instant, not proof that a method occupied the whole
interval between samples. On disk, chunks also carry metadata and constant-pool
checkpoints used to decode referenced data; these describe interpretation rather
than application work.
[OpenJDK chunk reader](https://github.com/openjdk/jdk/blob/master/src/jdk.jfr/share/classes/jdk/jfr/internal/consumer/ChunkHeader.java)

The Java custom-event API accepts primitive fields, strings, threads, and classes;
it does not persist arbitrary object graphs or arrays. This is an API constraint,
not a claim that independently written JFR bytes can contain only those types.
Wide logic vectors and nested VTR payloads need an explicit encoding and matching
tooling. A custom `requestId` or `parentId` field is application data, rather than
automatic VTR ownership or referential validation.
[Custom event field types](https://docs.oracle.com/en/java/javase/25/jfapi/data-types.html)

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

**Records and example.** Distinguish EventPipe, the capture mechanism, from
NetTrace, the file format. NetTrace v6 explicitly targets uses beyond .NET. Its
five logical tables hold events, event metadata, stacks, threads, and label lists.
Events reference decoding and correlation context; stack rows contain instruction
addresses rather than a universal source-code database. Labels can carry activity
and related-activity identifiers.
[NetTrace table model](https://github.com/microsoft/perfview/blob/main/src/TraceEvent/EventPipe/NetTraceFormat.md)

![Request start and stop events correlate across threads, alongside runtime collection events and samples; NetTrace tables describe events and their context.](diagrams/reference-nettrace.drawio.svg)

In the example, a provider's request start/stop convention produces an interval;
garbage-collection events belong to another provider. Correlation follows work
between threads, while stack samples explain where execution was observed. An
opcode or event name does not establish a universal interval schema: the producer
and analyzer must agree which fields identify the work. EventSource supports
typed application parameters, including serializable structures and arrays.
[EventSource instrumentation](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/eventsource-instrumentation)

A standalone NetTrace writer could define declaration, value-change, completed
transaction, and relation events without launching .NET. That reuses framing,
payload descriptions, and reference management. It still needs a hardware profile,
waveform checkpoints, selective indexes, and a reader interpreting them. Listing
those events with a standard decoder does not supply those database operations.

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

**Records and example.** FXT has explicit event subtypes for instants, counters,
durations, asynchronous work, and flows. Other records describe providers,
strings, threads, userspace/kernel objects, scheduling, logs, and binary blobs.
Providers have separate decoding contexts: a provider ID helps interpret local
references, rather than identifying an application transaction globally.
[Record types](https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#record-types)

![FXT synchronous durations, an asynchronous request spanning threads, a flow between work items, and a counter; records also describe objects and logs.](diagrams/reference-fxt.drawio.svg)

An ordinary duration represents thread-local nested work. An asynchronous duration
uses a correlation ID and can span threads; flow records describe movement of
work between events rather than its duration. These distinctions make FXT useful
for software-like transaction timelines.

Arguments are typed named values, including numbers, strings, pointers, object
IDs, booleans, and blobs. Ordinary event records have a four-bit argument count,
so large structured transactions cannot assume unlimited independent arguments.
A blob can hold such data, but its schema and hardware meaning need their own
agreement. Object annotations label resources; they do not supply HDL scope
kinds, aliases, or signal-history indexes.
[Arguments](https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#argument-types)

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

These ideas can be applied to the current VTR container without a format change.
The broader choice of retaining that encoding or adopting a reference format also
requires the hardware capability and reuse assessment below.

### 4.7 The FSDB and VWDB replacement baseline

Replacing a waveform database requires more than recording spans. The target
workflow includes finding a design object, jumping to a late simulation time,
reading its current value and nearby transitions, and following a transaction
back to its fields, stages, related work, and surrounding signals. Capture must
preserve what the simulator observed, including unknown logic and repeated
occurrences. A trace encoding and its reader together must meet that contract.

FSDB and VWDB are vendor databases with public product descriptions, rather than
public wire specifications in the cited material. The table records documented
workflows; **unverified** means the reviewed public sources do not establish a
specific storage or live-reader contract. It does not mean the product lacks it.
No claim about their internal block layouts follows from these descriptions.

| Capability | FSDB / Synopsys Verdi evidence | VWDB / Cadence Verisium evidence |
| --- | --- | --- |
| Digital waveform capture and analysis | FSDB stores simulation signal activity for Verdi waveform debug and replay. [Waveform reuse](https://www.synopsys.com/verification/resources/datasheets/verdi-idx.html) | VWDB is the waveform database used in Cadence simulation/emulation debug flows. [VWDB introduction](https://community.cadence.com/cadence_blogs_8/b/fv/posts/what-makes-next-generation-debug-solution) |
| Selected signals, scopes, and time windows | WaveUtils documents on-demand reads, signal/event reports, and scope/time extraction. [WaveUtils](https://www.synopsys.com/blogs/chip-design/verdi-waveform-utilities.html) | Python APIs expose design and waveform interrogation; exact on-disk seek/index contracts are unverified. [Python access](https://community.cadence.com/cadence_blogs_8/b/fv/posts/verisium-debug-python-app-store) |
| Transactions and testbench observations | Synopsys explicitly associates enhanced FSDB with UVM transaction recording, logs, and transaction views. [Verdi features](https://www.synopsys.com/verification/debug/verdi.html) | Verisium documents UVM/testbench debug and SmartLog; which transaction/relation records reside in VWDB itself is unverified. [Verisium features](https://www.cadence.com/en_US/home/tools/system-design-and-verification/ai-driven-verification/verisium-debug.html) |
| Design/source correlation | Verdi combines FSDB with other databases, including KDB; the waveform file is not the entire debug environment. [Platform databases](https://www.synopsys.com/content/dam/synopsys/verification/datasheets/verdi-ds.pdf) | Palladium's flow pairs VWDB waveforms with an IXCOM-derived design database. [Emulation and design data](https://community.cadence.com/cadence_blogs_8/b/fv/posts/verisium-debug-for-palladium) |
| Specialized HDL, low-power, and mixed-signal debug | Documented platform capabilities; exact logic-state, strength, delta-region, and analog encodings require SDK/manual validation. [Supported workflows](https://www.synopsys.com/content/dam/synopsys/verification/datasheets/verdi-ds.pdf) | Documented platform capabilities; exact representations in VWDB require SDK/manual validation. [Supported workflows](https://www.cadence.com/en_US/home/tools/system-design-and-verification/ai-driven-verification/verisium-debug.html) |
| Attach to an existing growing recording | Interactive debug is documented, but the cited public sources do not specify an independent reader's publication cursor, late-attach bootstrap, or partial-tail rules. | Interactive and postprocess modes are documented, but the cited public sources do not specify these independent-reader contracts. [Operation modes](https://login.cadence.com/content/cadence-www/global/en_US/home/training/all-courses/86320.html) |

The initial replacement scope should be VTR's documented data and query model:

- HDL scopes and variables, stable IDs, static and dynamic aliases, declared
  widths, 2/4/9-state vectors, real values, bytes, and event occurrences.
- Exact integer simulation ticks in a declared decimal timescale; preserve
  multiple changes and occurrences at the same tick without converting through
  floating-point seconds.
- Completed transactions, independently identified events/stages/logs, nested
  payloads, runtime ownership, directed relations, and constant-period clocks.
  Intervals can overlap, and record timestamps can arrive late.
- Compressed, selective signal/time and record queries, bounded decoding, crash
  recovery, and the incremental publication contract proposed here.

These are grounded in [SPEC.md](SPEC.md), [TRANSACTIONS.md](TRANSACTIONS.md),
and [RATIONALE.md](RATIONALE.md). They do **not** establish full parity with
commercial debug platforms. VTR's [VDB boundary](../README.md#trace-and-application-boundaries)
keeps source/netlist knowledge separate. Signal strength, an explicit simulator
scheduler-region/delta-cycle coordinate, full dynamic-object histories, assertion
semantics, power-intent semantics, and analog solver-specific data need separate
requirements and support audits. Preserving ordered same-time changes is useful,
but does not itself preserve which simulator scheduling region produced them.
Neither this proposal nor the current VTR format proves complete FSDB/VWDB parity.

### 4.8 Capability comparison against that baseline

**Built-in** below means the reviewed representation carries the stated meaning.
**Profile** means standard fields can encode it, but producers and readers need a
hardware-specific agreement. This is reusable serialization, not turnkey waveform
support. Entries concern the native formats and APIs reviewed above, rather than
what an arbitrary opaque blob could contain.

| Record capability needed by the replacement | Perfetto | CTF / LTTng | JFR | NetTrace v6 | FXT |
| --- | --- | --- | --- | --- | --- |
| Wide packed HDL values, including `X`/`Z` and nine-state logic | Profile; counters/state tracks alone do not declare this | Profile using fields/arrays | Profile; ordinary Java event API needs flattening or another encoding | Profile using typed fields/arrays | Profile using arguments/blobs |
| HDL scope kinds, variables, aliases, declaration growth | Profile over track/argument metadata | Profile over schema/events | Profile over metadata/events | Profile over metadata/events | Profile over object/argument records |
| Numeric observations | Built-in counters and typed arguments | Typed event fields | Typed event fields | Typed event fields | Built-in counters and arguments |
| Held symbolic state | Built-in state tracks | Profile | Profile | Profile | Profile |
| Span representation | Built-in track-local begin/end | Profile: start/stop or completed event | Built-in committed duration event | Profile: provider-defined events | Built-in begin/end, complete, async |
| Overlapping work independent of thread nesting | Separate custom tracks and correlation | Profile with IDs or completed rows | Independent committed duration events | Profile with activity labels/IDs | Async IDs |
| Structured payloads | Nested annotations or protobuf fields | Schema-defined structures/arrays/sequences | Java API has scalar/reference restrictions; binary writer is a separate choice | Metadata describes objects and arrays | Flat typed arguments; structured blob needs schema |
| VTR's global record IDs, ownership, embedded/located stages, typed relations | Profile; flows and correlation help but have different semantics | Profile | Profile | Profile; labels help but do not validate ownership | Profile; async/flow IDs have narrower roles |
| Repeated identical observations | Distinct events | Distinct events | Distinct events, subject to recording configuration | Distinct events, subject to recording configuration | Distinct events |
| Constant-period clock stretches and signal clock association | Profile | Profile | Profile | Profile | Profile |

A profile can remain wire-compatible with the host format. However, representing
VTR semantics as `signal_declare`, `signal_change`, `transaction_complete`, and
`relation` events is still domain-model design. The savings are in reusing the
container, schema machinery, and tools, rather than eliminating that work.

**Time is a separate compatibility check.** A simulator's time advances according
to the simulated design, independently of the host clock. A tracing API which
measures elapsed wall time is not interchangeable with one accepting simulation
timestamps.

| Candidate | Native time representation and consequence |
| --- | --- |
| Perfetto | Packet timestamps default to nanoseconds. Custom clock units use an integer `unit_multiplier_ns`; that mechanism does not express a fractional nanosecond. Exact picosecond/femtosecond simulation time needs a mapping, an additional tick field, or adapted tooling. Treating one simulation tick as one displayed nanosecond preserves ordering but changes the physical unit. [Clock schema](https://github.com/google/perfetto/blob/main/protos/perfetto/trace/clock_snapshot.proto) |
| CTF | Clock metadata defines a frequency and integer cycle values. A simulation clock can therefore retain fine ticks. Consumers must use raw cycles where needed: Babeltrace also offers conversion to signed nanoseconds, which is a different range/precision boundary. [Clock snapshots](https://babeltrace.org/docs/v2.1/libbabeltrace2/group__api-tir-cs.html) |
| JFR | Chunks describe start ticks and ticks per second, but standard recorded-event time access exposes instants and nanosecond durations. A custom native writer's raw resolution does not establish end-to-end subnanosecond support through JFR consumers. The ordinary `begin/end` API measures execution time, not supplied simulation time. [Chunk reader](https://github.com/openjdk/jdk/blob/master/src/jdk.jfr/internal/consumer/ChunkHeader.java), [duration access](https://docs.oracle.com/en/java/javase/25/docs/api/jdk.jfr/jdk/jfr/consumer/RecordedEvent.html) |
| NetTrace | Trace metadata supplies tick frequency; event timestamps use signed 64-bit ticks. A standalone writer can choose simulation ticks, but must account for VTR's unsigned range and check consumer conversions. The EventPipe capture API's runtime timestamps are a separate constraint. [Trace and event layouts](https://github.com/microsoft/perfview/blob/main/src/TraceEvent/EventPipe/NetTraceFormat.md) |
| FXT | Initialization supplies ticks per second and events carry unsigned 64-bit ticks. The representation can describe a fine simulation clock; SDKs and viewers still need validation for supplied timestamps and exact unit conversion. [Timestamps](https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#timestamps) |

For every candidate, specify a deterministic tie-break for same-tick changes
across producers. File order within a stream does not create a simulator-wide
scheduler order. Keep late transaction begin/end fields distinct from capture
time where a transport or sorter requires monotonic progress. In particular,
CTF's stream clock and a completed transaction's application begin field can
serve different purposes.

**Database access is the largest unproven part of reuse.** The reviewed formats
provide packet, chunk, or block framing and useful software-trace semantics.
They do not prescribe VTR's per-signal compressed histories, dirty-group frames,
previous-dirty links, and alias-aware selective decoding. Packet time indexes
can accelerate seeking without answering the held value of a particular signal
at the seek point. A writer must provide a checkpoint, a signal index, or enough
history for that question. Whole-file compression alone does not provide
independently decodable signal/time ranges.

| Candidate | Stored organization useful for reuse | What a waveform query still needs |
| --- | --- | --- |
| Perfetto | Packet sequences, interned data, and imported query tables | A measured alternative to importing all relevant observations before querying a few signals |
| CTF | Metadata plus packet streams; packet time coverage supports seeking | Signal declarations and held-value checkpoints/indexes beyond packet time indexes |
| JFR | Chunks with metadata and constant pools; event streaming | Signal-specific lookup and state reconstruction independent of event iteration |
| NetTrace | Typed blocks, event-block time bounds, and reference-cache epochs | Signal-specific locators and checkpointed values; block time bounds alone are insufficient |
| FXT | Sequential size-framed records with provider-local reference state | Seek bootstrap, signal indexes, and held-value checkpoints |

These are deductions from the layouts reviewed in sections 4.1–4.5. They are
not assertions that third-party tools cannot add indexes or that one layout is
slower. Index construction and decoding costs need measurement on the same data.

Perfetto supplies a substantial query engine and viewer, so it could save more
application work than a serialization-only choice. Its normal analysis path
imports observations into query tables; that must be compared with VTR's direct
file queries. Its documented sorting window can reject sufficiently late data;
full sorting increases memory use. Arbitrarily late VTR record timestamps need
an explicit ingestion strategy.
[Trace Processor memory model](https://perfetto.dev/docs/analysis/batch-trace-processor),
[ordering and flush behavior](https://github.com/google/perfetto/blob/main/docs/concepts/buffers.md)

**Live support and losslessness are also separate.** The five live paths in the
first comparison table are reusable designs, but none implies the exact same
local-file refresh API. CTF's live route needs a compatible relay producer; JFR's
route follows repositories; EventPipe normally starts a runtime capture session;
FXT's provider streaming transfers buffers. Perfetto's packet ingestion does not
by itself refresh a browser view. Every evaluation needs late attachment with
metadata and active work, explicit temporary EOF, and history retention tests.
A profiling configuration that samples, filters short events, or drops full
buffers is not acceptable as silent behavior for an exhaustive waveform dump.
Loss policies belong to the writer/system configuration as well as the format.

### 4.9 Can an existing format replace VTR?

**For a transaction-only or simulator-performance view, reuse is credible now.**
Perfetto is the strongest first experiment when the goal is to reuse a working
timeline, query engine, counters, states, and causal flows. FXT is attractive for
compact explicit durations and asynchronous work. CTF and NetTrace offer typed
event interchange with broad payload descriptions. JFR is especially useful when
correlating JVM behavior with application events. These are suitability judgments
from the reviewed interfaces, not measured performance rankings.

**For the combined FSDB/VWDB replacement scope, no candidate is established as an
as-is replacement by this research.** There is no evidence here that the task is
impossible with an existing format. In particular, CTF and standalone NetTrace
could carry a lossless hardware profile without changing their basic framing.
The unresolved work is the hardware schema, exact-time path, publication/bootstrap
contract, and indexed storage/query behavior at waveform scale.

| Candidate | Reuse that could remove VTR work | Remaining work for the combined replacement | Assessment |
| --- | --- | --- | --- |
| Perfetto | Track/state/counter model, SDKs, SQL analysis, timeline UI, flows | Exact simulation-time mapping, HDL profile, arbitrary overlaps/late arrivals, selective waveform storage and rendering | Best prototype for reusing analysis/UI; do not assume its imported tables replace a waveform database. |
| CTF / LTTng | Metadata-described binary types, packet streams, Babeltrace ecosystem, live protocol design | Hardware profile, simulator writer, relay integration if used, signal checkpoints/indexes and viewer | Strong candidate for reusing a generic trace format; evaluate independently of adopting the whole LTTng capture stack. |
| JFR | Typed committed durations, metadata/pools, repository following, JVM diagnostics tools | Native simulator writer and timestamp control, richer payload mapping, active previews, waveform indexes/UI | Good JVM integration/export target; more adaptation for this hardware database scope. The binary format is not restricted to JVM-originated bytes. |
| NetTrace v6 | Typed event blocks, object/array payload metadata, reference epochs, loss detection | Standalone writer/reader version validation, hardware profile, clock/range policy, waveform indexes/UI, live bootstrap | Strong generic event-storage candidate alongside CTF; v6's general-purpose intent deserves an actual prototype. |
| FXT | Compact duration/async/flow records, object/log records, simple framing | Large structured payload profile, HDL declarations, selective indexes, simulator-time and live-reader integration | Useful transaction interchange; embedding custom waveform blocks in blobs would retain most waveform-format design work. |

There are three different reuse decisions:

1. **Adopt an existing format end to end.** Generate supported observations and
   use its current readers/viewers. Accept this only if their semantics, precision,
   and resource use meet the required workflow.
2. **Adopt its wire format plus a hardware profile and external indexes.** Keep
   valid CTF or NetTrace, for example, while implementing hardware semantics and
   selective query acceleration. Count profile, sidecar, and ingestion complexity
   in the cost comparison. This can be a successful replacement even though a
   generic viewer cannot render every hardware record.
3. **Retain a waveform-specialized backend and integrate existing analysis.**
   Export transactions, counters, and selected signals to Perfetto/FXT, or join
   software traces using explicit clock mappings. This delivers interoperability
   while keeping lossless waveform data in its indexed representation.

An exporter is not automatically a lossless archive converter. Publish its
mapping for IDs, ownership, relations, open spans, same-time ordering, precision,
and unsupported values. A subset export is useful when its omissions are explicit.

Before treating VTR's own encoding as the preferred long-term choice, evaluate:

- A **CTF hardware profile** and a **NetTrace v6 hardware profile** against the
  same representative recordings. Start with declarations, logic changes, complete
  transactions, payloads, and relations; add indexes only where query measurements
  require them. Validate the output with an independent existing decoder.
- A **Perfetto transaction/state export** opened in the existing viewer, including
  overlapping operations, stages outside owner bounds, causal relations, and an
  operation active at attachment. Check what works without custom importer/UI code.
- Exact round trips for 512/4096-bit buses with `X`/`Z` and nine-state values,
  aliases, repeated same-tick events, fine ticks, large timestamps, nested payloads,
  forward relations, and late completed transactions. Include interrupted writes
  and loss reporting. Do not allow approximate timestamps or stringified values
  to pass as equivalent typed data.
- Cold late-time reads of a few signals from a large recording, value-at-time
  queries on quiet signals, long intervals intersecting short windows, and repeated
  refresh with a slow consumer. Measure bytes read, decompression, memory, time to
  first useful view, steady capture overhead, and file plus index size. Include any
  required import pass and index build in the totals.
- If licensed FSDB/VWDB SDKs are available, the same observable queries and dump
  configuration through their supported tools. Verify logic/strength/delta and
  transaction capabilities with those SDKs rather than inferring wire features
  from product pages. Vendor performance claims are not a controlled comparison.

The reuse prototypes and vendor checks are proposed work, not results of this
research. This document justifies the incremental contracts independently of the
backend; it does not prove that bespoke VTR serialization is faster, smaller, or
cheaper to maintain than these alternatives.

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
