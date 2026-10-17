# VTR design rationale

This is a focused summary of the trace-library decisions inherited from the
[prototype](https://github.com/ripopov/vtr/tree/4b4fd692e7e03a7ebea4daf7a89787203eed9cb6/core/vtr).
The [specification](SPEC.md) is normative; Rust API contracts live in rustdoc.
No encoding changes are introduced by this extraction.

## Boundaries

VTR captures runtime facts and identity. A separate VDB supplies design-source
semantics and presentation. Keeping those out of the format allows several
consumers to interpret the same trace without rewriting it or introducing VDB
as a reader dependency. Log-site provenance is the narrow exception: it
identifies where a recorded runtime message originated.

The library owns its reader, writer, compression, recovery, hierarchy queries,
and derived activity sidecar. Simulator adapters, C bindings, conversion,
remote protocols, and viewers belong in separate components.

## Container and hierarchy

Append-only, self-delimiting sections permit streaming writes and recovery by
scanning complete sections. A directory and fixed trailer provide block lookup
without decoding all value data. Per-section CRC-32 detects damaged sections.
Whole-file compression was rejected because it would defeat local random access.

An interned string table and typed node attributes generalize FTR's dictionary
idea. Explicit parent IDs avoid positional hierarchy attributes. Scope and
variable type codes retain FST numbering; generators share the same
hierarchy. Chunked declarations allow scopes and signals to appear during a
run. Each block's signal counts already determine whether a late signal existed,
so a separate per-signal creation timestamp would duplicate stored information.

## Waveforms

Time-partitioned blocks, group frames, delta-coded time indexes, and dynamic
aliases draw on FST. Independently compressed column runs bound decoding for a
local query; a previous-dirty-block table finds earlier values for quiet groups.
LZ4 and Zstandard are per-blob choices, with uncompressed storage when compression
does not help. Shuffle, delta, and dictionary transforms operate on value streams
rather than time headers.

The writer can move encoding onto background threads through bounded queues.
The reader maps the container and decodes queried pieces on demand. Loaded
histories are owned, immutable, and shareable; decoded-cache eviction does not
invalidate returned histories. The mapped input itself must remain unchanged
while a reader exists.

## Transactions, logs, and clocks

FTR inspires generators, typed attributes, and relations between transactions.
Kanata supplies stage/lane and instruction-lifecycle concepts; OpenTelemetry
supplies span kinds, events, links, and status. These are runtime data, not
viewer layout or pipeline-color rules.

Logs separate static call-site metadata from dynamic arguments, following
NanoLog and binlog. Per-block text dictionaries borrow CLP's repeated-variable
idea. Records share transaction IDs and relation semantics while using a
specialized column encoding. A reader reconstructs text and can query typed
arguments without parsing formatted output.

Generators identify record sources and carry their own kinds and attributes.
They can be roots or children of ordinary scopes. A separate stream node would
force an extra parent onto standalone sources without contributing record data;
ordinary scopes already group related sources. Clocks are generators of kind
`CLOCK`, and clock associations belong to the generators that use them.

Declared clocks use ordinary transactions for steady edge stretches. They need
no extra section or value tag, and cycle numbering is derived from the stretches.

## Derived activity index

The activity sidecar stores per-signal stretches separated by long silences.
Thresholds are chosen per source block from disk and memory budgets. Queries
use the index where conclusive and resolve narrow ambiguous windows from the
trace. Source identity prevents using a sidecar for a different recording.
Derived data stays outside the trace so rebuilding an index cannot alter the
original recording. The generic source interface allows external format
adapters without adding those dependencies to VTR.

## Verification

Tests exercise multi-block round trips, aliases, event multiplicity, late
hierarchy, clocks, logs, cache ownership, truncated-section recovery, and writer
sealing. Activity queries are checked against recorded changes and builder
memory is measured with a counting allocator. Small historical Verilator traces
provide producer-output regression coverage without a simulator installation.
The prototype's benchmark suite is not imported, so this repository makes no
comparative size or speed claims.
