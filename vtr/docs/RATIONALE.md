# VTR design rationale

VTR keeps simulation state, runtime activity, and diagnostics in one recording.
A useful viewer can follow a request through pipeline stages, inspect the signal
values around a failure, and resolve a log's owning transaction without deriving
identity from formatted text. The wire contract is in [SPEC.md](SPEC.md) and
[TRANSACTIONS.md](TRANSACTIONS.md); this document explains the design choices.

## State and occurrences have different semantics

A waveform signal represents a held value. Duplicate suppression can remove a
write that leaves its value unchanged. A transaction, stage, event, or log
represents an occurrence with identity. Two identical requests at one timestamp
remain two records. Event variables also preserve repeated occurrences rather
than deduplicating their payloads as held levels.

The two representations share a container and clock units, while retaining
encodings suited to their access patterns. Waveform blocks store per-signal
columns and starting frames. Runtime records store typed payloads and indexed
intervals or points. Treating every payload field as a waveform would lose
record boundaries, parentage, and repeated identical occurrences.

## One occurrence supports owner and location views

An instruction can own visits to fetch and execute resources. A pipeline view
loads visits by location and time; an instruction view expands one owner.
Located stages and events express both views through one global record ID,
one generator, and one owner ID. Keeping two copies would require reconciling
identity, payloads, and lifecycle between them.

Embedded events and stages describe detail local to one owner without inventing
design-location nodes. They still have global IDs, so lookup and relations can
identify them. Overlapping located visits are preserved: they may be the evidence
of a resource conflict, and discarding one would hide the problem.

Design scope parentage and runtime owner parentage are independent. Ownership
does not require time containment; temporally decoupled simulation may report a
child outside its owner's stored interval. Queries and locators therefore cannot
assume owner time bounds contain every child.

## Schemas separate repeated structure from values

A file-local datatype describes a structure, array, sequence, enumeration, or
primitive. Many generators can share a datatype. Every record supplies one
complete value; optional attributes remain available for details that do not
belong to the common schema. A schema change declares a new source or type
instead of changing the interpretation of already written data.

Canonical payloads are independent of Rust/C++ object layout. Explicit widths,
field order, byte order, and sequence lengths let the implementations exchange
values without sharing native padding, allocation strategy, or ABI. Typed native
bindings copy capture data before returning, so borrowed application values can
be changed or destroyed immediately afterward. Retained reader payloads keep
both their schema and storage alive after cache eviction or reader destruction.

Column encoders traverse the schema and separate numeric, floating, text, and
miscellaneous data. This avoids repeating names and tags on every record while
preserving signedness, integer width, NaN payloads, and aggregate structure.
Local text dictionaries keep each page independently decodable. Shared text
references can reduce repeated large strings across transaction pages, with
publication ordered before their first use.

## Pages balance compression and selective reading

Located records are stably grouped by generator into segments. Small segments
share a compressed page; large groups split only between records. Generator,
parent, ID, and time bounds prune unrelated segments before decoding. Separating
LOG pages avoids unnecessary log decompression during owner expansion.

Ordinary transactions are sorted by ID within a block and split into pages with
disjoint ID ranges. This helps owner lookup without requiring a persistent exact
ID-to-row index. Block ranges can overlap when old owners complete alongside
new ones; range filters select candidates rather than prove membership.

Owners store distinct child block numbers instead of one locator per child.
This keeps owner metadata compact while supporting expansion through parent
bounds and exact parent filtering. It does not guarantee one-page expansion;
page reads and complete-section checksum scans still have a cost.

A small fixed payload of unsigned 64-bit leaves can use scalar pages. Absolute
record fields and payload words are stored as byte planes, allowing compression
to exploit similar high bytes without per-value varint conversion. Richer
payloads, attributes, embedded detail, locators, and relations use general pages.
The optimization changes representation, not logical values or query behavior.

## Capture and output have separate clocks and budgets

Producer time is the ordering frontier for assembly. Explicit record timestamps
can differ from it. Record IDs represent creation identity; neither timestamps,
IDs, completion order, nor physical storage order substitutes for the others.

Inline mode performs all work synchronously. Threaded mode separates capture,
ordered assembly, and optional parallel encoding. Each logical producer is
exclusive. A producer's frontier prevents the assembler from publishing work
that an earlier producer operation could invalidate. Pending future work can
spill to temporary storage instead of consuming unbounded RAM when a producer
stalls. Encoding jobs own declaration snapshots and publish in submission order.

C++ uses SPSC capture rings; Rust uses bounded MPSC handoff with per-producer
byte admission. Their internal transport differs, while payload ownership,
ordering, handle origins, and completion contracts agree. Queue admission,
pending assembly, encoding jobs, and page targets are separate budgets with
separate purposes. None is a promise that total process RSS equals one option.

## Completion is explicit and recoverable

Append-only sections publish declarations before dependent data. A directory
records complete section ranges and block identities. The final trailer makes
that directory discoverable without scanning the whole file. Recovery retains
only a contiguous verified section prefix and preserves global IDs and block
numbers; it does not renumber references or salvage partial pages.

A finalized directory can still be incomplete. Crash sealing or recovery may
retain an owner whose child block was never written, or a relation whose target
is missing. Readers preserve available evidence and expose missing references.
Normal completion requires all references to resolve. Checksums establish byte
integrity; full validation also establishes semantic invariants.

The recording ID distinguishes independent runs even when length and directory
CRC happen to match. Activity indexes and caches bind to the complete fingerprint.
VTI stores exact activity ranges and can be rebuilt; its independent version and
framing are specified alongside the trace format.

The Rust Linux guard can seal a threaded writer independently of its mutable
frontend. It parks idle owners before using their writer directly. It cannot
safely borrow an inline encoder interrupted inside a call, or reuse a backend
that unwound through a panic; those cases retain the verified file prefix.
The native C++ SDK currently has no process crash guard.

## Evidence and implementation boundaries

The Rust implementation is independent of the C++ engine. Bidirectional tests
cover codecs, execution and encoding modes, typed records, waveform states,
retained schemas, recovery, and activity indexes. Independent byte-level oracles
and malformed-input tests check encodings rather than reproducing one API's
implementation. Verilator fixtures are generated by the pinned simulator fork.

The maintained [benchmark suite](../../benchmarks/README.md) verifies full reads
and queries against source replay before reporting time, size, memory, and
capture costs. [Its report](../../benchmarks/report.md) describes the measured
workloads. Small synchronous synthetic workloads do not establish simulator
speedup, cold-device latency, or every asynchronous scaling property. Changes
are assessed using comparable settings and retained slower results, rather than
selecting only workloads that improve.

VDB holds design-source mappings. Viewer state, source navigation policy,
application layout, and remote protocols belong to their consuming application.
Keeping those boundaries separate lets VTR focus on durable recorded meaning,
correct selective decoding, and predictable ownership.
