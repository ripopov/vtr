# VTR design rationale

VTR records what a design did during a run: signal values, requests, resource
occupancy, instruction lifetimes, and diagnostics. A user investigating a failed
instruction should be able to follow its stages, inspect the surrounding signal
values, and find the messages it produced. Those observations need common time
units and explicit identities so the reader can follow their connections without
reconstructing them from text.

This document sets the direction and targets we aim for in VTR. It explains the
problems the design should solve, the reasons for its choices, and the tradeoffs
used to evaluate alternatives. It does not document the current state of the
codebase or indicate which features are implemented. The models and requirements
below guide development and provide criteria for judging the result.

## Signals, records, and transactions

A **signal** describes a value that persists until it changes, such as a wire or
register in a VCD waveform. Assigning `0` to a signal already holding `0` adds no
transition. A history can suppress that assignment while preserving the state.

A **record** is an individual captured observation with its own identity. It
describes a point occurrence, such as an event or log message, or an interval,
such as a stage. Two requests issued at the same timestamp with identical
address, byte-enable, and operation fields still produce two distinct records.
Repeated messages likewise remain distinct even when their timestamps and
arguments match. Verilog event variables (`VCD_EVENT`) also preserve repeated
occurrences rather than applying held-value deduplication.

A **transaction** is a logical unit of work with a lifetime and stable identity.
While active, it can accumulate attributes, embedded events and stages, and
attached located events and stages. Attributes describe a transaction or record;
they are not themselves records. A transaction's payload describes the work,
its events and stages describe progress, and relations connect it to other work.

Transactions can be nested to represent work at different scales. In a GPU
simulator, for example, a dispatch can own warps, and each warp can own
instructions. An instruction is a transaction; its retry event and execute stage
are records belonging to it. It can visit fetch, decode, and execute resources
before it retires.

![A held signal value, two identical request occurrences, and an instruction lifetime with fetch, decode, execute, and retire activity.](diagrams/signals-records-transactions.drawio.svg)

Waveforms and records share a recording, but serve different purposes. A request
record keeps its fields together and distinguishes one request from another,
even when their values match. Storing only changes to those fields would lose
that distinction. A waveform tracks the value each signal holds over time.
Storing each signal's changes together lets a reader inspect that signal without
scanning unrelated activity. Saved snapshots of signal values provide starting
points for reading later parts of the recording without replaying its entire
history. Signals with identical changes can share stored data. Keeping these
representations separate preserves request identity while making signal
histories compact and efficient to read.

## Design hierarchy and runtime ownership

The **design hierarchy** is a tree of scopes and sources. It answers where an
observation belongs: the module containing a signal, the source issuing
requests, or the execute slot occupied by an instruction. A generator identifies
a source of transactions or records. It can be a root or belong to an ordinary
scope. Unlike FTR, VTR has no special transaction stream (TX Stream) scopes:
ordinary scopes already provide grouping, so a separate stream node would add
no meaning. Declarations can also appear during execution, so capture does not
require a complete hierarchy at startup.

The **ownership hierarchy** describes runtime work. An execute visit belongs to
an instruction even though many instructions use that execute resource over
time. A dependency on another instruction is a separate relation: causal
connections need not fit inside the ownership tree.

![Dispatches, warps, and instructions form an ownership tree; one execute visit also references its generator in the design hierarchy.](diagrams/logical-hierarchy.drawio.svg)

A **located stage** is an interval at a design location; a **located event** is a
point occurrence there. The generator identifies the location and the owner ID
identifies the transaction. Selecting a resource over time and expanding one
instruction must reach the same occurrence, with the same record ID and payload.
Recording separate copies for those views would require reconciling their
identity, updates, and lifetime.

**Embedded**, or unlocated, stages and events describe detail local to one owner.
A retry annotation or an internal phase does not always justify a resource in
the design hierarchy. Such records still have global IDs, so lookup and
relations can reach them. A lane groups local stages within their owner. Names
and lanes give these annotations meaning without adding design-location nodes.

Ownership does not imply time containment. Temporally decoupled simulation may
report a visit outside the owner's stored interval. Queries cannot use the
owner's interval as proof that no child exists outside it. Overlapping resource
visits also remain intact: they may be the evidence of a scheduling conflict,
and removing one would hide the problem.

## Datatypes describe values independently of sources

A reusable datatype describes a primitive, structure, array, sequence, or
enumeration. Several request generators can share the same address and operation
fields. Declaring those fields once saves repeated names and tags and gives
consumers a consistent structure for filtering and comparison.

The datatype answers how to decode a value. A generator answers where
transactions or records come from. Ownership answers which runtime work they
belong to. These are different identities. Changing a name, moving a source in
the design hierarchy, or choosing a different logger should not require
inventing a different argument layout.

Canonical payloads specify field order, widths, byte order, and sequence lengths
independently of Rust/C++ object layout. They preserve integer signedness,
enumerations, floating-point bits, nested values, and repeated identical records.
Native padding, allocation strategy, and ABI are not part of the file contract.
Optional tagged attributes remain useful for exceptional details that do not
belong in a common payload schema.

Declarations are immutable. A changed argument layout needs a new datatype;
payloads already written retain their original interpretation. Ordinary typed
sources can bind a fixed datatype. Log messages select their datatypes
independently, allowing one logger to carry messages with different layouts.

Typed native bindings must finish capture before returning. Borrowed text or
arrays can then be changed or destroyed without affecting captured data.
Reader payload views retain both their schema and storage after cache eviction
or reader destruction. Ownership of capture data and ownership of retained read
data are explicit parts of the interface, not consequences of a convenient
buffer implementation.

## Logging

A subsystem produces a stream of diagnostic messages from many statements. A
shader scheduler and a memory controller may use different loggers in the same
VTR file. Each logger should remain one generator, regardless of how many log
statements it contains or how many argument layouts those statements use.

### Logger, datatype, and message are separate identities

The model separates three things:

| Entity | Meaning | Declared or captured information |
| --- | --- | --- |
| Logger generator | The named source of a message stream | Name, design scope, and logger metadata |
| Metadata-bearing datatype | A logging frontend's description of a statement | Argument type reference and attributes for format, severity, file, line, and function |
| Message record | One execution of that statement | Record ID, logger reference, datatype reference, timestamp, argument values, and optional owner |

The logger selects a stream. The datatype selects the payload layout; logging
helpers interpret its attributes to render one message.
Severity belongs to the statement, while a logger's enabled level is a capture
policy. A logger is also independent of an execution thread: one producer can
use several loggers, and several producers can contribute to the same logger.
Queue ownership must therefore follow producers rather than logger count.

For example, a request statement and a retry statement can contribute to one
shader logger. The request helper can also be used by the memory logger:

| Record | Logger | Tick | Statement datatype | Arguments |
| --- | --- | ---: | --- | --- |
| R501 | shader | 100 | D27: request | `addr=0x80, bytes=64` |
| R502 | shader | 100 | D28: retry | `bank=3` |
| R503 | memory | 104 | D27: request | `addr=0xc0, bytes=32` |

There are two generators and two metadata-bearing datatypes. Repeated
executions of the statement described by D27 create new records;
they do not create new declarations. Moving through a logger stream must show
all its messages, with each message retaining its datatype metadata.

### Share datatype declarations

Datatype declarations can carry arbitrary tagged attributes and reference other
datatypes. A logging frontend can declare one structure for its argument layout,
then declare wrappers referencing that layout with metadata such as
`kind = "callsite"`, format, severity and provenance. The same datatype traversal,
canonical encoding, allocation limits and retained payload inspection apply to
these values as to transaction, event and stage payloads. A one-field wrapper
adds no canonical payload bytes.

Several statement datatypes can share one argument datatype while carrying
different metadata. One logger can emit records using unrelated datatypes.
Argument order determines positional formatting; field names remain available
for structured queries. Neither names nor type tags need to be resent with
each value.

Statement metadata must not mutate a shared argument datatype. Separate immutable
wrapper declarations preserve reuse and distinguish statements, including when
provenance is unavailable or several statements report the same file and line.
Record references identify those declarations explicitly. VTR stores generic
datatype IDs and attributes; recognizing a logging statement is a frontend
convention. The design hierarchy remains a tree of scopes and observation
sources.

### Capture values and defer presentation

An illustrative logging statement is:

```cpp
void log_request(Logger& logger, std::uint64_t addr, std::uint32_t bytes) {
    LOG_INFO(logger, "request addr={:#x} bytes={}", addr, bytes);
}
```

![Datatype D27 references its argument layout and carries format, severity and provenance; repeated records reference D27 and supply timestamps and values.](diagrams/logging-call-site.drawio.svg)

The format, severity, provenance, and argument schema are static at this
statement. Invocations need only capture references to the logger and datatype,
the observation time, and the values. That timestamp belongs to the invocation,
not to the later backend drain. Global record identity and an optional
owning transaction preserve navigation into the rest of the trace. A message
without an owner remains valid; a parent is runtime context, not a substitute
for the logger.

Separating static information from dynamic values follows the designs of
NanoLog, binlog, and Quill. VTR keeps the resulting record structured and
self-describing on disk. Reading or formatting it must not require the
producer executable, its address space, or its debug symbols. Process
pointers can identify immutable descriptors during capture, but only file-
local references belong in the recording.

The repeated capture path should check whether the statement is enabled before
evaluating its arguments. A disabled log must not call an expensive helper,
allocate an argument, register a source, or trigger argument side effects.
Literal formats should be checked against argument types during compilation
where the language supports it. For enabled logs, argument-pack encoders should
be reusable across callsites with the same argument types; making every source
line part of the encoder's template identity adds code without changing capture.

Registration may happen before a timed loop or on first use. After binding, a
call should use a stable handle or direct lookup rather than repeat string
interning, metadata construction, a hash-table search, or runtime type checks.
Registration and type erasure needed for heterogeneous descriptors belong on
the cold path. Shared immutable descriptors must not introduce a global lock
into every message capture.

This optimization must preserve recording lifetimes. A static C++ callsite can
outlive several writers, while logger and datatype IDs are local to
one recording. Caching the first writer's handle in that static object would
reuse stale identity in a later file. Writer-bound bindings must stay with their
writer or have explicit recording identity and invalidation. Reusing a writer's
memory address is not proof that it is the same recording.

Arguments must be encoded or copied before return, including text borrowed from
temporary or reusable simulator buffers. A queued pointer into mutable caller
storage is not a deferred message. Small captures should avoid producer
allocations; large captures still need owned storage and bounded admission.
Formatting, compression, and output can run later without borrowing simulator
state. Exact values remain available even when a presentation cannot be rendered.

### Store schemas efficiently without turning them into tracks

A logger stream contains messages with different argument schemas. Encoding
must therefore obtain the payload datatype through the callsite, rather than
assume that all messages in the generator have one layout. Pages may group
messages by logger and callsite to traverse homogeneous columns efficiently.
Such groups are storage units, not additional source generators. A logical
callsite reference can be represented by a group header or dictionary index;
the requirement is that every decoded message resolves its callsite unambiguously.

Logger and time bounds should prune unrelated messages before decompression.
The callsite catalogue can select severity, provenance, and argument layouts
without reading every payload. Typed fields support queries such as requests
above an address or retries for one bank without parsing `log.message` text.
An owner query selects that transaction's messages across callsites and logger
streams; logs remain separate from its located EVENT/STAGE child locators.

Static format reuse and repeated argument text are different compression
opportunities. Formats and schema field names belong in declarations. Dynamic
text still needs a value representation; bounded page dictionaries can reuse
it without interning every runtime string forever. CLP is a useful
reference for exploiting repetition in log content. Any shared dictionary must
be available before dependent data, while pages retain independent decoding
boundaries.

Neither storage grouping nor a formatted message defines record identity or
promises chronological iteration. A reader can merge messages for display using
their timestamps, with an explicit policy for ties. It must preserve distinct
records that have identical times and arguments.

### Publication and validation preserve meaning

Rust and C++ writers, readers, validation, recovery, and inspection must agree on
logger identity, callsite references, and argument datatype lookup. A logger's
name identifies its stream; each message's callsite identifies its format and
argument layout. The fixed datatype bindings of ordinary transactions and
located events and stages remain independent of that choice.

Declaration dependencies must precede their first persisted use. A missing
callsite or datatype cannot be repaired by a declaration written after the
dependent block. Format changes need an explicit version change and regenerated
fixtures so that readers cannot silently reinterpret old bytes. Supporting
obsolete logging models would add complexity without advancing the design goals.

Validation must reject invalid logger/callsite/type references, malformed
metadata, incompatible argument payloads, and incomplete payload consumption.
Tests need messages from several callsites in one logger, one callsite used by
several loggers, shared argument types, repeated identical messages, optional
owners, borrowed-input mutation, disabled argument side effects, concurrent
first use, and sequential recordings. Cross-language and recovery tests must
verify the same semantics. A viewer should expose the logger as the source and
retain the callsite as message metadata; hiding extra generators in the viewer
would leave the underlying stream model unresolved.

## Time and ordering describe different facts

Global IDs identify transaction and record creations. Observation timestamps
place activity in simulated time. Producer time certifies progress for assembly.
Completion order determines when an owner becomes ready to write. Physical
storage order reflects bounded buffers and compression policy. None of these
orders can substitute for another.

A producer advancing to T promises that no more capture operations below T will
arrive from it. With several producers, the minimum frontier determines which
recording time is complete. Explicit historical observations can carry earlier
timestamps without moving that frontier backward. A wall-clock grace period,
useful for merging delayed application logs, cannot establish simulation progress.

Clock cycles are another view of time. Clocks can pause or change frequency, and
a trace can contain several clock domains. Constant-period stretches represent
edges compactly and allow cycle numbers to be derived. Associating a source
with a clock avoids pretending that one global cycle count applies everywhere.

## Signal histories need a starting value

A waveform query asks both what changed inside a window and what value was
already held at its beginning. Even a signal that is quiet throughout the window
needs access to its preceding state. Starting frames, time partitions, signal
groups, delta encoding, and aliases support this access pattern. FST provides a
reference for that family of techniques.

Waveform progress also matters when no value changes. An explicitly advanced
quiet interval still belongs to the recording's time range. Deduplication of
held levels must not erase that progress or change event-variable occurrence
counts.

## Storage supports location and owner queries

A resource query usually selects one source and a limited time window. An owner
query can follow one instruction through many locations. Storing activity only
inside its owner would make resource queries scan unrelated owners or require
another index. Storing only by location would make owner expansion equally
expensive without navigation metadata.

Located records are grouped by generator into segments with generator, parent,
ID, and time bounds. Small segments share a compressed page; large groups split
between records. These bounds prune candidates before decoding, but a range
match does not prove that a particular owner or ID is present. Exact filtering
still follows. Keeping LOG payloads on separate pages avoids decompressing them
while expanding ordinary located stages and events.

An owner stores distinct child block numbers instead of one locator per child.
This keeps navigation metadata compact while allowing completed child payloads
to leave memory before a long-lived owner ends. The tradeoff is additional
parent filtering and sometimes several page reads. Checksumming a complete
section can also cost more than decoding one selected page; query measurements
must include the actual integrity policy.

Ordinary transactions are sorted by ID within a block and split into pages with
disjoint ID ranges. This supports owner lookup without a persistent exact
ID-to-row index. Different blocks can have overlapping ID ranges when old owners
complete alongside newer ones. Global IDs remain stable regardless of that
physical arrangement.

Schema traversal separates numeric, floating, text, and miscellaneous values
into columns. Payload keys, tags, and fixed aggregate structure come from the
datatype instead of being repeated in every row. Page-local text dictionaries
keep decoding independent of other pages. Shared text references can save space
for repeated large values when their declarations precede the dependent pages.

Small fixed payloads of unsigned 64-bit leaves can use scalar pages. Byte planes
exploit repeated high bytes without converting each word to a varint. Payloads
with richer structure, attributes, embedded detail, locators, or relations use
general pages. This optimization changes representation while preserving values
and query behavior.

Smaller pages reduce unnecessary decoding but add metadata and give compression
less repetition to exploit. Larger pages favor sustained output and compression
while increasing selective read costs. File size alone cannot choose the right
balance; owner lookup, related records, labels, checksums, and I/O all contribute.

## Capture and output have separate clocks and budgets

Most tracing work should run away from simulator threads. Their repeated job is
to capture observations with stable ownership and publish them through a cheap
handoff. The backend assembles transactions and records, encodes columns,
compresses pages, and writes sections. Optional encoding workers can increase
throughput while output publication remains ordered.

Each logical producer exclusively owns its capture queue and recording time.
Its physical thread can change through an explicit release/attachment protocol.
Several logger streams can use the same queue without becoming the same source.
Mutable frontend caches belong to that producer; sharing immutable logger and
callsite descriptions must not accidentally share an unsynchronized cache.

Bounded SPSC byte rings give each producer an independent handoff to the
backend. Ordinary captures reuse ring storage; oversized captures use owned
storage. Full queues apply backpressure rather than silently dropping
observations. Ingestion continues even when another producer's frontier
prevents serialization, and pending data can spill beyond its memory budget.
Otherwise a fast producer could fill its queue before reaching a simulator
barrier that the slow producer needs in order to advance.

Queue admission, pending assembly, active owners, payload reuse, encoding jobs,
and page targets have separate lifetimes and budgets. Bounding the capture ring
does not bound the process's total RSS. Long-lived owners and accumulated
navigation metadata need measurement alongside compression buffers.

Inline mode remains useful for a caller that prefers simple synchronous
execution or already serializes recording. It creates no transport or worker
threads. Capture, assembly, compression, and output then run on that caller,
with the same payload, lifetime, and reader semantics as threaded execution.

## Completion preserves evidence after failure

Traces are especially valuable when execution fails. Append-only sections carry
their own boundaries and integrity checks, and declarations precede dependent
data. A final directory and trailer accelerate ordinary opening without being
the only way to discover recorded sections.

Successful capture guarantees ownership of the input, not durable storage.
Asynchronous failures must surface on a later call or completion barrier, and
explicit close is needed to observe completion errors. Draining buffered output
does not promise `fsync` durability. Retrying an already accepted message can
duplicate the occurrence rather than repair the recording.

Recovery retains the contiguous verified section prefix. It does not salvage
individual rows from an incomplete page, invent missing transactions or records,
or renumber surviving identities. An owner, its child block, and a relation
endpoint can become durable at different times. Incomplete recordings preserve
available evidence and report unresolved references; complete recordings require
those references to resolve. Checksums establish byte integrity, while full
validation also checks semantic invariants.

A log is ordinary evidence, not a completion protocol. Neither a fatal message
nor the absence of a final message proves that a run was sealed successfully.
Completion and recovery state belong to the container. Borrowing an interrupted
inline encoder or a backend unwinding through a panic cannot be made safe merely
by logging an ending. Crash sealing should operate independently of an interrupted
frontend when the backend remains usable. When safe sealing is impossible, the
verified prefix must remain recoverable.

A recording identity distinguishes independent runs even when their lengths and
directory checksums match. Derived activity indexes and caches bind to the full
fingerprint. Activity summaries help find interesting windows without changing
the original observations, and can be rebuilt when absent. Recovery produces a
new fingerprint, so stale summaries cannot silently apply to the recovered file.

## Performance claims need complete measurements

VTR aims for compact waveform and transaction recordings with low simulator
capture cost and useful selective access. FST and FTR provide relevant
compressed-format comparisons; VCD provides a simpler write-speed reference.
These are measurement objectives, not conclusions from an API shape or a
compression ratio.

Benchmarks must verify full reads and queries against source replay before
reporting results, and record their workloads and settings. Comparisons need
equivalent observations, comparable hardware and CPU budgets, repeated runs,
complete final drain and close, and the same integrity policy. Write and read
time, encoded size, peak memory, selective-query latency, and producer cost
all matter. A faster producer that leaves unbounded work queued has not made
recording faster overall.

Logging measurements also need several callsites per logger, shared argument
packs, many loggers, repeated and large text, disabled levels, first-use
registration, and steady capture. They must distinguish frontend latency from
backend throughput and shutdown cost. Small synthetic results do not establish
simulator speedup, cold-device latency, or every asynchronous scaling property.
Retain slower results and investigate them rather than selecting only improved
workloads.

Rust and C++ implementations should be checked independently through
bidirectional interoperability tests, byte-level decoding oracles, malformed
inputs, and failure tests. These provide different kinds of evidence and should
agree on values, references, query results, and recovery behavior without
depending on the same encoder internals.

## Boundaries and reference designs

VTR owns durable runtime facts, their identities, and their retrieval. VDB holds
design-source mappings. A callsite's runtime file and line explain where a
diagnostic statement originated; they do not replace VDB's mapping to HDL or
other design sources. Simulator integration, conversion, viewer layout, source
navigation policy, and remote protocols belong to their consuming components.
These boundaries allow one recording to serve several tools without rewriting
its meaning or making every reader depend on a particular application.

The following designs inform different parts of the model:

- FTR / LWTR4SC: generators, transaction attributes, and explicit relations.
- FST / libfst: compact signal histories and grouped waveform storage.
- Kanata / Konata: instruction lifecycles, stages, and lanes.
- OpenTelemetry: span kinds, events, links, and completion status.
- NanoLog: separating static statement information from dynamic values.
- binlog: typed binary logging with reusable event metadata.
- Quill: static callsite metadata, argument capture, and deferred backend work.
- CLP: exploiting repeated structure and text in log storage and queries.
