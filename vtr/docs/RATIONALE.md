# VTR design rationale

VTR records what hardware did during a run. It captures several kinds of
observation: signal values, operations, resource occupancy, and diagnostic
messages. All of them describe the same run, so a useful trace keeps enough
identity, timing, and structure for a user to move between them directly,
without reconstructing meaning from text.

## Boundaries

VTR records runtime facts and the identities that connect them. Design-source
meaning and presentation come from a separate VDB. Keeping VDB data out of the
format lets several consumers interpret the same trace in their own way, without
rewriting it and without making VDB a dependency of every reader.

The trace library handles recording, retrieval, compression, and recovery.
Simulator integration, conversion, transport, and visualization live in separate
components. This lets one trace model serve many producers and consumers.

## Hierarchy and record sources

A hierarchy gives each observation a stable context: the module that contains a
signal, the source that issues requests, or the resource where an operation
executes. Scopes group related sources. Generators identify the sources of
records and describe the kinds of records they produce and the attributes those
records share. This split between describing a source once and recording each
occurrence follows FTR's generators, dictionaries, and typed attributes.

A generator can stand alone or belong to a scope. VTR does not require an extra
stream node between them, because that level would often carry no information;
ordinary scopes already provide grouping where it helps. Sources can also appear
during execution, so recording must not depend on knowing the full hierarchy
before the run starts.

Values are typed so that consumers can filter and interpret them. Formatting an
address, number, or enumeration as text throws away distinctions that consumers
would then have to guess. Shared declarations describe repeated record fields
once, which reduces both repetition and ambiguity. Per-occurrence attributes
capture details that vary from one operation to the next.

## Signals and occurrences

A signal history must answer two questions: what changed within a time window,
and what value was already in effect when the window began. Even a signal that
is quiet during the window therefore needs access to its earlier state.

Event signals work differently. Each record is a separate occurrence, so
coincident or repeated records must keep their count even when their payloads
are identical.

Waveform storage follows FST in using time partitioning, signal grouping, delta
encoding, and aliases. These techniques exploit repeated structure while still
supporting local queries. Compression must keep persistent state and individual
occurrences distinct: skipping an unchanged value is safe for state, but the
same optimization can erase a meaningful event.

## Transactions, stages, and events

Transactions identify operations such as instructions, requests, and transfers.
Their lifetimes, outcomes, and relationships explain execution at a higher level
than individual signal transitions. Several formats inform this model: FTR for
transaction attributes and relations; Kanata for instruction lifecycles, stages,
and lanes; and OpenTelemetry for span kinds, events, links, and status.

Users need two complementary views of the same activity. Following one
instruction shows its visits to fetch, decode, and execute. Following one
execute slot over time shows visits from many instructions. When a user switches
between the views, both must lead to the same records.

A located stage is an interval spent at a design location; a located event is a
point occurrence at one. Each has two independent links: its generator
identifies the location, and its owner identifies the transaction it belongs to.
The hierarchy describes design structure, while ownership describes runtime
activity. Record identity distinguishes repeated visits by the same transaction
to the same location.

Keeping all activity inside its owner favors transaction inspection, but
location queries must then search owners or build a separate index. Keeping a
separate copy for each view creates consistency and identity problems. Located
records instead express both relationships directly, so the storage design must
support efficient access both by owner and by location.

Unlocated stages and events describe detail that matters within one transaction
but does not identify a separate design location. A lane groups such stages
within their owner. Producers can therefore record local annotations without
inventing hierarchy nodes or resource identities for them.

Resource occupancy must keep the evidence needed to diagnose conflicts.
Overlapping visits to a resource may reveal a producer error or a hardware
problem; discarding either visit would hide that evidence. For the same reason,
coincident events and repeated visits stay distinct. Ownership, timestamp order,
and emission order each carry different information, and none can be inferred
from the others.

## Identity and relations

Stable record identities let consumers follow ownership, dependencies, and
causality across sources. A relation can connect individually identified records,
including operations and located occurrences. Identity must not depend on how
records are grouped in storage or the order in which they are retrieved: a
record selected through one query must be the same occurrence when reached
through another.

Ownership and causal relations answer different questions. An event can belong
to one instruction while also taking part in a dependency on another operation.
Explicit relations keep this distinction instead of forcing every connection
into a single parent tree.

## Logs and clocks

A log message combines a static call site with dynamic arguments. Separating the
two follows NanoLog and binlog; reusing repeated text values follows CLP. Keeping
arguments typed lets consumers filter messages without parsing formatted output.
Linking a message to the operation that produced it connects diagnostics to the
rest of the trace.

Clock cycles are a useful way to read time, but clocks can pause or change
frequency. Recording each steady stretch of edges captures this behavior
compactly while still letting cycle numbers be derived. Associating each source
with its clock supports multiple clock domains without imposing one global cycle
count.

## Storage and query tradeoffs

Trace workloads combine sustained writes with selective reads. A viewer usually
needs a few signals or locations over a limited time window, while transaction
inspection follows related activity across many locations. Grouping data for
only one of these patterns makes the other expensive. Compressing regions
independently lets a query decode only the regions it needs. Smaller regions
make reads more selective but add metadata and give compression less repeated
data to exploit.

Indexes trade write cost and space for less query work. Coarse references are
cheap to record but require more filtering during reads; fine references reduce
filtering but require more bookkeeping during recording. The right balance
depends on the full cost of a query, including owner lookup, related records,
labels, decompression, and I/O. Compression ratio alone cannot decide it.

A long-lived operation may accumulate many completed children. Recording should
be able to release completed payloads while their owner is still active.
Backpressure limits how much encoding work is queued, but active operations,
their pending detail, and navigation metadata still occupy memory. Memory
budgets must account for those lifetimes, not just for compression buffers.

## Recovery and derived data

Traces are most valuable when execution fails. Writing data incrementally in
independently delimited pieces lets complete data survive an interrupted write,
and integrity checks separate recoverable records from damaged bytes. Final
lookup structures should speed up access, but they must not be the only way to
find recorded data.

Related records may become durable at different times. Recovery must keep
whatever evidence is available and report unresolved references or incomplete
activity. It must not invent missing records or present a partial result as
complete.

Derived activity indexes help users find interesting windows in long, mostly
quiet traces. They can summarize activity coarsely and fall back to the original
records where a summary is inconclusive. Keeping derived data separate means
indexes can be rebuilt without changing the recording. Tying them to their
source trace prevents stale summaries from being applied to a different run.
