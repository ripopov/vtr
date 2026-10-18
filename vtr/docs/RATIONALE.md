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

An ergonomic simulator API can bind typed stage and event sites to a module once,
then reuse them for each instruction. This SystemC module excerpt uses an
illustrative API; `SystemCModule` extends `sc_core::sc_module` with a built-in
`trace` scope and simulation-time capture. `VTR_O(name)` declares a site member,
binds it to that scope, and uses its identifier as the site name. The instruction
already has a transaction, `inst.trace`, that follows it through the pipeline.

```cpp
// C++26 reflection derives VTR schemas and serialization from these aggregates.
struct OperandFetch { std::uint32_t operands; };
struct BankConflict { std::uint32_t bank; std::uint32_t requests; };

struct OperandCollector : vtr::SystemCModule {
    using SystemCModule::SystemCModule;

    sc_core::sc_in<bool> clk{"clk"};
    vtr::StageSite<OperandFetch> VTR_O(operand_fetch);
    vtr::EventSite<BankConflict> VTR_O(bank_conflict);

    auto fetch(Instruction& inst) -> void {
        const auto visit = operand_fetch.enter(
            inst.trace, {.operands = inst.operand_count()});
        while (const auto conflict = try_fetch_operands(inst)) {
            bank_conflict.emit(inst.trace, *conflict);
            wait(clk.posedge_event());
        }
    }

    auto try_fetch_operands(Instruction& inst) -> std::optional<BankConflict>;
};
```

Here, `fetch` runs in a SystemC thread, and `try_fetch_operands` returns conflict
details when fetching must retry, or `std::nullopt` on success. The same typed
result becomes the event payload without copying fields into a separate trace
structure. The stage opens on entry and closes when `visit` leaves scope,
spanning any clock waits. Each conflict is an instantaneous event during that
visit. Stages and events belong to the instruction and identify their module
through their sites. Payload field names and types are declared once; visits and
events supply only their values.

## Logging

A log message combines a static call site with dynamic arguments. Separating the
two follows [NanoLog][nanolog] and [binlog][binlog]; reusing repeated text values
follows [CLP][clp]. Linking a message to the transaction that produced it
connects diagnostics to the rest of the trace.

An illustrative C++ macro captures call-site metadata automatically:

```cpp
void log_request(Logger& log,
                 std::uint64_t addr, std::uint32_t bytes) {
    LOG_INFO(log, "request addr={:#x} bytes={}", addr, bytes);
}
```

![A call site stores the format string, severity, source provenance, and argument names and types once. A table of log records contains only identities, timestamps, site references, and argument values; its column names come from the shared metadata.](diagrams/logging-call-site.drawio.svg)

The macro captures the format string, severity, source provenance, and argument
names and types once per call site. Each invocation references that metadata and
records its own timestamp and argument values; format strings and argument names
are not repeated. Formatting can wait until the trace is read, keeping that work
off the recording path.

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

## Performance requirements

VTR aims to **outperform FST for waveform tracing and FTR for transaction
tracing, while approaching VCD write speed**. Comparisons should use equivalent
observations and the same host resource budget, with compressed size and peak
memory reported alongside runtime. VCD's simpler encoding makes its write speed
an ambitious reference for a format that also compresses and organizes data for
selective queries.

A single-core C910 CoreMark run with Verilator 5.048 illustrates the tradeoff.
Tracing the SoC hierarchy and on-chip memories, excluding external RAM contents,
produced about **71.4 kB of VCD changes per clock** and **2.14 kB of FST output
per clock**, the latter including metadata and initial state. Single-thread
simulation took **13.7 s without tracing, 77.0 s with VCD, and 102.1 s with
FST**. FST's file was 33.6 times smaller, but recording took 33% longer.
These were single runs with buffered writes; timings included final flush and
close. About 89% of VCD's added runtime above dumping-disabled execution
remained when output was discarded, showing that capture and encoding costs
matter alongside storage bandwidth. Encoded file sizes do not directly measure
binary input volume or signal-change counts.

Required bandwidth is data per simulated clock multiplied by simulated clocks
per wall-clock second. Larger simulated designs produce more data per clock
but generally simulate more slowly; design size alone cannot determine
writer throughput. Small cores running at 2 MHz leave only 500 ns per cycle,
so fixed capture costs also matter. System-level models need separate budgets
for transaction owners, stages, events, and payloads, including GPU lane detail.
Use measured producer rates to set capacity targets.

Validate performance with repeated complete runs covering both large designs
and fast small cores. Compare waveform and transaction workloads separately,
then together, including capture, compression, indexing, final drain, and close.
Keep queues bounded and expose backpressure without silently losing records.

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
