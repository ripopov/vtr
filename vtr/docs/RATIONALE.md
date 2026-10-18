# VTR design rationale

VTR records what hardware did during a run. It captures several kinds of
observation: signal values, transactions, resource occupancy, and diagnostic
messages. All of them describe the same run, so a useful trace keeps enough
identity, timing, and structure for a user to move between them directly,
without reconstructing meaning from text.

## Signals, Records and Transactions

A **signal** represents the value on a wire or in a register, as in a VCD
waveform. Its value persists until it changes. A value-change history therefore
records changes in state: writing `0` when the value is already `0` adds no
transition.

A **record** captures an individual occurrence. Instantaneous records, such as
log messages and events, have a single timestamp. Their payloads can contain
aggregate values: structs, arrays, and arrays of structs. Every occurrence
matters, even when its payload matches the previous one. Two requests sent at
the same timestamp with identical `{addr, byte_en, type}` payloads must remain
two records. Treating them as signal values would lose the second request.
Event signals follow the same occurrence semantics as instantaneous records.

A **transaction** is a record representing a logical unit of computation in the
design. It has an identity and a lifetime that can span many cycles. Examples
include a GPU grid dispatch, the lifetime of an individual warp, and the
execution of an individual instruction with its pipeline stages. Stages and
events describe progress within that lifetime; relations connect it to other
activity.

![A signal records state changes, two identical request records remain distinct at the same timestamp, and an instruction transaction spans several cycles with pipeline stages.](diagrams/signals-records-transactions.drawio.svg)

## Physical hierarchy and record sources

The **physical hierarchy** is the design tree of scopes. It gives each
observation a context: the module that contains a signal, the source that issues
requests, or the resource where a transaction executes. Scopes group related
sources. Generators identify the sources of transactions and other records,
with attributes describing each source.

A generator can stand alone or belong to a scope. VTR does not require an extra
stream node between them (unlike [FTR][ftr]), because that level would often carry
no information; ordinary scopes already provide grouping where it helps. Sources
can also appear during execution, so recording must not depend on knowing the
full hierarchy before the run starts.

## Logical hierarchy

The **logical hierarchy** follows runtime ownership among transactions and
records. For example, a GPU grid dispatch can contain warps, each with its own
instructions and associated records. This tree describes how work is organized
during a run.

Ownership and causal relations answer different questions. An event can belong
to one instruction while also taking part in a dependency on another
transaction. Explicit relations connect records across the ownership tree
without forcing every connection into a parent-child relationship.

Keeping both physical and logical hierarchy lets users follow a unit of work
through the design or inspect the work occurring at one location. Following one
instruction shows its visits to fetch, decode, and execute; following one
execute slot over time shows visits from many instructions. Both views must lead
to the same records.

Record identity must not depend on storage grouping or retrieval order: a record
selected through one query must be the same occurrence when reached through
another. Stable identities let consumers follow ownership, dependencies, and
causality across sources.

![A grid dispatch owns warps and instructions, a dependency crosses the ownership tree, and one execute stage links to both its owning instruction and its source generator in the physical hierarchy.](diagrams/logical-hierarchy.drawio.svg)

## Transactions, stages, and events

Several formats inform the transaction model: [FTR][ftr] for transaction
attributes and relations; [Kanata][kanata] for instruction lifecycles, stages,
and lanes; and [OpenTelemetry][otel] for span kinds, events, links, and status.

A located stage is an interval spent at a design location; a located event is a
point occurrence at one. Each has two independent links: its generator
identifies the location, and its owner identifies the transaction it belongs to.

Recording separate copies for the owner and location views would create
consistency and identity problems. Located records express both relationships
on the same occurrence.

Unlocated stages and events describe detail that matters within one transaction
but does not identify a separate design location. A lane groups such stages
within their owner. Producers can therefore record local annotations without
inventing hierarchy nodes or resource identities for them.

Resource occupancy must keep the evidence needed to diagnose conflicts.
Overlapping visits to a resource may reveal a producer error or a hardware
problem; discarding either visit would hide that evidence.

## Typed payloads

Typed payloads describe the data carried by logs, located stages and events, and
regular transactions. Preserving values as numbers, addresses, and enumerations
lets consumers filter and interpret them without parsing formatted text.

Payload datatypes are reusable and independent of generators. For example,
several generators sending requests to a NoC can share one request datatype.
Declaring its fields and types once reduces repeated metadata and gives
consumers a consistent structure across sources.

A transaction can also carry additional named attributes alongside its typed
payload. These describe the transaction as a whole and capture extra details
without changing the shared payload datatype.

## Logging

A log message combines a static call site with dynamic arguments. Separating the
two follows [NanoLog][nanolog] and [binlog][binlog]; reusing repeated text values
follows [CLP][clp]. Linking a message to the transaction that produced it
connects diagnostics to the rest of the trace.

## Time

Clock cycles are a useful way to read time, but clocks can pause or change
frequency. Recording each steady stretch of edges captures this behavior
compactly while still letting cycle numbers be derived. Associating each source
with its clock supports multiple clock domains without imposing one global cycle
count.

Ownership, timestamp order, and emission order each carry different information,
and none can be inferred from the others.

## Boundaries

VTR records runtime facts and the identities that connect them. Design-source
meaning and presentation come from a separate VDB. Keeping VDB data out of the
format lets several consumers interpret the same trace in their own way, without
rewriting it and without making VDB a dependency of every reader.

The trace library handles recording, retrieval, compression, and recovery.
Simulator integration, conversion, transport, and visualization live in separate
components. This lets one trace model serve many producers and consumers.

## Signal history and storage

A signal history must answer two questions: what changed within a time window,
and what value was already in effect when the window began. Even a signal that
is quiet during the window therefore needs access to its earlier state.

Waveform storage follows [FST][fst] in using time partitioning, signal grouping,
delta encoding, and aliases. These techniques exploit repeated structure while
still supporting local queries. Value-change encoding applies to persistent
state; event histories must preserve occurrence counts.

## Storage and query tradeoffs

Trace workloads combine sustained writes with selective reads. Queries by
location and by owner favor different data groupings. Location queries usually
cover a limited time window; owner queries can span many locations. Storing
activity only under its owner makes location queries search owners or rely on
an index. Compressing regions independently lets a query decode only the regions
it needs. Smaller regions make reads more selective but add metadata and give
compression less repeated data to exploit.

Indexes trade write cost and space for less query work. Coarse references are
cheap to record but require more filtering during reads; fine references reduce
filtering but require more bookkeeping during recording. The right balance
depends on the full cost of a query, including owner lookup, related records,
labels, decompression, and I/O. Compression ratio alone cannot decide it.

Derived activity indexes help users find interesting windows in long, mostly
quiet traces. They can summarize activity coarsely and fall back to the original
records where a summary is inconclusive. Keeping derived data separate means
indexes can be rebuilt without changing the recording. Tying them to their
source trace prevents stale summaries from being applied to a different run.

## Recording and recovery

A long-lived transaction may accumulate many completed children. Recording
should be able to release completed payloads while their owner is still active.
Backpressure limits how much encoding work is queued, but active transactions,
their pending detail, and navigation metadata still occupy memory. Memory
budgets must account for those lifetimes, not just for compression buffers.

Traces are most valuable when execution fails. Writing data incrementally in
independently delimited pieces lets complete data survive an interrupted write,
and integrity checks separate recoverable records from damaged bytes. Final
lookup structures should speed up access, but they must not be the only way to
find recorded data.

Related records may become durable at different times. Recovery must keep
whatever evidence is available and report unresolved references or incomplete
activity. It must not invent missing records or present a partial result as
complete.

## References

- [FTR / LWTR4SC][ftr]: generators, shared dictionaries, typed transaction
  attributes, and explicit relations between transactions.
- [FST / libfst][fst]: time partitioning, signal grouping, delta encoding, and
  aliases for compact waveform storage and local queries.
- [Kanata / Konata][kanata]: instruction lifecycles, stages, and lanes for
  describing progress within a transaction. Konata documents and reads the Kanata
  trace format.
- [OpenTelemetry specification][otel]: span kinds, timestamped events, causal
  links, and completion status.
- [NanoLog][nanolog]: separating static log-site information from dynamic
  arguments and deferring message formatting.
- [binlog][binlog]: recording shared log-site metadata once and preserving
  arguments as typed binary values.
- [CLP][clp]: dictionary reuse of repeated string variables in log messages.

[ftr]: https://github.com/Minres/LWTR4SC
[fst]: https://github.com/gtkwave/libfst
[kanata]: https://github.com/shioyadan/Konata
[otel]: https://github.com/open-telemetry/opentelemetry-specification
[nanolog]: https://github.com/PlatformLab/NanoLog
[binlog]: https://github.com/morganstanley/binlog
[clp]: https://github.com/y-scope/clp
