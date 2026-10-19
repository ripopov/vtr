# VTR transactions

Status: proposed; validated C++20 reference in `atemp/vtr3-reference`.
Production readers and writers still use version 2.

This document specifies the transaction model, encoding, and reader and writer
behavior for format version 3.0. [SPEC.md](SPEC.md) defines the current version-2
container, compression, string table, and hierarchy framing. This proposal
inherits that framing except where explicitly replaced below. TX_BLOCK retains
section kind 5; RECORD_BLOCK uses kind 9 and replaces LOG_BLOCK storage.
DATATYPES uses kind 10. The header, directory and activity-sidecar changes below
are also part of version 3.0.
Adoption requires regeneration of older trace assets.

Every transaction record has a file-wide ID. Events and stages associated with
design locations are stored separately from their owning transactions, grouped
by generator and packed into independently compressed pages. Each owner lists
the blocks containing its children. The reference defaults are 64 KiB raw
pages for both block kinds and Zstandard level 1. Complete layout comparisons
select fast LZ4 for both old and new writers.
[VTR3_COMPARISON.md](VTR3_COMPARISON.md) reports complete old/new measurements
and the remaining performance limitations.

## Notation and common types

Rust-style declarations describe the logical data model. They do not prescribe
a programming interface, memory layout, or serialization order. Section 8
defines the byte layout.

`i8`, `i16`, `i32`, and `i64` are signed integers; `u8`, `u16`, `u32`, and
`u64` are unsigned integers. The suffix gives the width in bits. A signed
N-bit integer ranges from `-2^(N-1)` to `2^(N-1) - 1`; an unsigned N-bit
integer ranges from zero to `2^N - 1`. `bool` is Boolean; `f32` and `f64` are
IEEE 754 binary32 and binary64, respectively.
`Vec<T>` is an ordered, variable-length sequence of `T`; `Option<T>` is either
absent or one `T`. `(A, B)` is a pair. Collections may be empty unless a
constraint says otherwise. An `enum` selects one named variant and its payload.

```rust
type TransactionId = u64;
type NodeId = u32;
type GeneratorId = NodeId;
type StringId = u32;
type TypeId = u32;
type Timestamp = u64;
type BlockNumber = u64;
type Bytes = Vec<u8>;
type Utf8Text = Vec<u8>;
```

`TransactionId` and `BlockNumber` are nonzero identifiers in separate
namespaces. `NodeId` indexes the container hierarchy; `GeneratorId` identifies
a generator node in that hierarchy. `StringId` indexes the file string table.
Hierarchy and string indexes are zero-based. `Timestamp` uses the file's time
units. `Bytes` is opaque data; `Utf8Text` must contain valid UTF-8.
Record-specific types and their constraints are defined below.

## Problem statement

A trace viewer must efficiently display both transactions and the pipeline
stages they pass through. These require two complementary views of the same
execution:

- A transaction view follows an instruction or other transaction through its
  lifetime, showing its stages, events, and attributes.
- A pipeline view follows a design location over time, showing which
  transactions occupy a stage or produce events there.

For example, selecting an instruction should reveal its fetch, decode, and
execute visits; selecting an execute slot should show visits from all
instructions within the visible time window. Users must be able to navigate
from either view to the corresponding records in the other.

An *owner* is the ordinary transaction to which an event or stage belongs.
A *located* event or stage has its own generator, identifying a design
location. An *unlocated* event or stage is embedded in its owner and is
accessible through that owner.

Located records support direct queries by location and time. Owners list the
blocks containing their located children, limiting the search needed to
reconstruct a transaction's activity. Together, these structures support both
views without duplicating child records.

Readers select blocks by generator and time, derive argument columns from
generator declarations, and resolve related records on demand. Writers encode
arguments without repeating their schema and release completed child payloads
promptly. Producers that need only transaction-local detail can use unlocated
events and stages.

![The same stage visits grouped by instruction above and by pipeline location below. Stage 103 belongs to instruction 42 and occupies execute.slot0 from time 4 to 7 in both views.](diagrams/transaction-views.drawio.svg)

The highlighted visit is one stored record. The transaction view finds it in
the blocks listed by its owner and matches its parent ID; the pipeline view
queries its generator.

## 1. Identity, time, and ownership

A transaction is a runtime record emitted by a generator. Ordinary transactions
represent intervals such as instructions, requests, transfers, or operations.
Located stages represent visits to design locations; located events and logs
represent points in time. Clock transactions represent steady stretches of
clock edges.

Transaction IDs, also called record IDs, are nonzero `u64` values unique within
the file across all record types. The writer allocates strictly increasing IDs
in creation order, independently of timestamps and completion order. IDs never
wrap or get reused; ID exhaustion is a writer error. A parent is identified by
transaction ID alone. Relations can use ID zero to mean an endpoint outside
the file; zero is never a real transaction or parent ID.

Times are unsigned `u64` values in the file's time units. Ordinary intervals
and located stages require `end >= begin`. Occupancy of a positive-duration
stage is `[begin, end)`; zero-duration stages and events are point records.
Timestamps need not be monotonic, including within a block. Record timestamps
are independent of the writer's waveform time.

Hierarchy parentage describes design structure. Transaction parentage describes
runtime ownership. Neither implies the other, and runtime ownership does not
require temporal containment. Every present parent must be an ordinary
transaction that is active when the child is created. Parentage is fixed at
creation, so the parent ID is smaller than the child ID and cycles are
impossible. Ending a parent does not finalize its ordinary child transactions.
Physical record order does not determine ownership, and references may point
forward in the file.

## 2. Generators and declarations

A generator is a hierarchy node whose path identifies a source or design
location. It may be a root or a child of an ordinary scope. Declarations are
immutable and must precede blocks that use them. Additional generators may
be declared during a run.

```rust
struct Generator {
    id: GeneratorId,
    name: StringId,
    parent_scope: Option<NodeId>,
    kind: StringId,
    metadata: Attributes,
    payload_type: TypeId,
}
```

`Attributes` is defined in section 2.2. `parent_scope`, when present, must
identify an ordinary scope.

| Generator kind | Records | Storage |
|---|---|---|
| Producer-defined, such as `INSTRUCTION` or `TRANSACTION` | Ordinary transactions | TX_BLOCK |
| `CLOCK` | Clock stretches | TX_BLOCK |
| `EVENT` | Located events | RECORD_BLOCK |
| `STAGE` | Located stages | RECORD_BLOCK |
| `LOG` | Log messages | RECORD_BLOCK |

The four special kind strings are reserved. Other kinds are producer-defined.
A generator's kind is separate from the per-record span kind of an ordinary
transaction. Scope membership does not imply a record kind or inheritance of
metadata or argument schemas.

Generator metadata consists of keyed typed values describing the source.
Argument declarations describe values supplied by every record. Metadata is
not copied into individual records and is not a default for missing arguments.

### 2.1 Reusable payload datatypes

Every generator references one immutable payload datatype. Several generators
can share that datatype without redeclaring fields. Unit (type 0) means no
payload. Every occurrence supplies exactly one value of its datatype; there are
no omitted fields or implicit defaults. `TypedPayload` below denotes that logical
value, whose shape follows its datatype rather than a per-occurrence tag list.

The generator-specific hierarchy payload is `varint kind_string_id`, then
`varint payload_type_id`. The enclosing hierarchy record supplies its ID, name,
parent and metadata. A schema change requires a new generator with a distinct
node ID. Datatypes may be added during recording, before their first use.

Builtin datatype IDs 0–22 match the value tags in section 2.2: unit, bool,
i8/i16/i32/i64, u8/u16/u32/u64, f32/f64, str, text, bytes, bits, logic, logic9,
time, enum, pointer, fixed and ufixed. ID 19 is reserved: an enumeration requires
a declared datatype with an integer underlying type. Lists and maps are optional
attribute values; typed aggregates use the declarations below. Datatype IDs and
attribute tags have separate namespaces.

Declared datatype IDs start at 23. They form an immutable DAG: every referenced
type must have a smaller ID, and nesting depth including scalar leaves is at
most 64. A DATATYPES section is an uncompressed payload:

```text
varint first_type_id, type_count
repeat type_count times:
    u8 kind
    length-prefixed UTF-8 type_name
    kind 1 (struct):
        varint field_count
        repeat field_count: length-prefixed UTF-8 field_name, varint type_id
    kind 2 (fixed array): varint element_type_id, element_count
    kind 3 (sequence): varint element_type_id
    kind 4 (enum):
        varint underlying_integer_type_id, literal_count
        repeat literal_count: u64 value_bits, length-prefixed UTF-8 literal_name
```

Chunks append contiguous IDs. Counts fit `u32`; the section is consumed exactly.
Struct fields have unique, nonempty names; enumerators have unique nonempty
names and unique values representable in their underlying type. Signed enum
literal bits are sign-extended to `u64`; unsigned values are zero-extended. Type and field
names are inline UTF-8, independent of the container string table. Unknown enum
numeric values remain valid and preserve their underlying value.

Struct fields are visited in declaration order; arrays and sequences in element
order. A struct or fixed array adds no per-occurrence keys, tags or counts. A
sequence adds its `u32`-valued element count to the numeric stream. Enumerations
consume their underlying scalar, with names supplied by the declaration.
Section 8.1 defines the scalar streams. These rules allow structs, arrays,
arrays of structs and nested aggregates without interpreting formatted text.

Optional ordinary-transaction attributes must not duplicate a top-level payload
field name. Located events, located stages and logs carry their typed payload
without an additional per-record attribute list. Readers expose declared names
and types alongside values and can traverse aggregate fields.

The C++20 reference binds member names once with `Record<T>`/`Enum<T>` and member
pointers. Capture copies values into owned canonical bytes without object
padding: fixed-width little-endian scalars, `u32` text/byte/sequence lengths,
`u64` packed-bit widths, then declaration-order values. This is an internal
handoff representation, not an additional file payload. C++26 reflection and
SystemC site/lifetime wrappers can supply the same schemas in a separate
integration layer; they are not dependencies of the format.

### 2.2 Attributes and labels

Metadata, optional transaction attributes, unlocated record attributes, and
relation attributes are lists of unique string keys and typed values. A writer
rejects duplicate keys within an item. Readers preserve unknown keys.

In the proposed format, the value tags and payloads in this section replace
SPEC.md section 5.3 throughout the file, including META and attributes of every
HIERARCHY node kind. Attribute-list framing remains a varint count followed by
string-ID keys and tagged values.

```rust
type Attributes = Vec<(StringId, Value)>;

enum Value {
    Null,
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(StringId),
    Text(Utf8Text),
    Bytes(Bytes),
    Bits { width: u64, packed: Bytes },
    Logic { width: u64, packed: Bytes },
    Logic9 { width: u64, packed: Bytes },
    Time(u64),
    Enum { value: i64, literal: StringId },
    Pointer(u64),
    Fixed { raw: i64, scale: i64 },
    Ufixed { raw: u64, scale: i64 },
    List(Vec<Value>),
    Map(Attributes),
}
```

`Time` holds a timestamp or duration in file time units, according to the
field's meaning. `Pointer` holds an unsigned address. `Bits`, `Logic`, and
`Logic9` hold packed signal states; `width` counts signal bits. Their packed
lengths and the wire tags for all variants are defined below.

| Tag | Value | Payload after tag |
|---|---|---|
| 0 | null | None |
| 1 | bool | Byte 0 or 1 |
| 2 | i8 | Signed varint |
| 3 | i16 | Signed varint |
| 4 | i32 | Signed varint |
| 5 | i64 | Signed varint |
| 6 | u8 | Unsigned varint |
| 7 | u16 | Unsigned varint |
| 8 | u32 | Unsigned varint |
| 9 | u64 | Unsigned varint |
| 10 | f32 | 4 bytes, little-endian IEEE binary32 |
| 11 | f64 | 8 bytes, little-endian IEEE binary64 |
| 12 | str | Varint string ID |
| 13 | text | Length-prefixed UTF-8 bytes |
| 14 | bytes | Length-prefixed bytes |
| 15 | bits | Varint width, then `ceil(width/8)` packed bytes |
| 16 | logic | Varint width, then `ceil(width/4)` packed bytes |
| 17 | logic9 | Varint width, then `ceil(width/2)` packed bytes |
| 18 | time | Unsigned varint |
| 19 | enum | Signed varint value, varint literal string ID |
| 20 | pointer | Unsigned varint |
| 21 | fixed | Signed varint raw, signed varint scale; value is `raw * 2^-scale` |
| 22 | ufixed | Unsigned varint raw, signed varint scale |
| 23 | list | Varint count, then tagged values |
| 24 | map | Varint count, then string-ID keys and tagged values |

Tags group signed integers, unsigned integers, and floating-point values into
contiguous ranges, with increasing width within each family. Argument schemas
and tagged attributes use the same assignments.

Integer types preserve both signedness and width. Signed values use the
64-bit ZigZag encoding after sign extension; unsigned values use unsigned
varints. Writers and readers reject values outside the tagged or declared
type's range; narrowing must not truncate or wrap. Integer width determines
the permitted range, not a fixed payload length. These rules also apply to
values nested in lists and maps.

Floating-point payloads preserve their IEEE 754 bit patterns, including signed
zero, infinities, and NaN payloads. An `f32` value occupies four bytes and is
not widened to `f64`.

Packed signal-state codes follow the container's value definitions. A
length-prefixed byte sequence is a varint byte length followed by those bytes.
Maps have unique keys. Unknown type tags are errors.

Keys beginning with `vtr.` and `log.` are reserved. Imported keys must not be
rewritten into reserved namespaces. Producer-specific keys should use a tool
prefix. The reserved `vtr.label` attribute or declared argument must have type
`str` or `text`; it supplies an instance label. Otherwise a transaction uses
its generator name, an unlocated event or stage uses its own name, and a
relation uses its kind. Labels carry no color, layout, or ordering semantics.

## 3. Ordinary transactions

```rust
type TransactionStatus = u8;
type SpanKind = u8;

struct Transaction {
    id: TransactionId,
    generator: GeneratorId,
    begin: Timestamp,
    end: Timestamp,
    status: TransactionStatus,
    span_kind: SpanKind,
    parent: Option<TransactionId>,
    payload: TypedPayload,
    attributes: Attributes,
    events: Vec<UnlocatedEvent>,
    stages: Vec<UnlocatedStage>,
    child_blocks: Vec<BlockNumber>,
}
```

`TransactionStatus` accepts only the following codes:

| Status | Code | Meaning |
|---|---|---|
| unset | 0 | No status supplied |
| ok | 1 | Completed successfully |
| error | 2 | Completed with an error |
| aborted | 3 | Cancelled, squashed, or flushed |
| open | 4 | Unfinished when the file closed |

`SpanKind` accepts `unspecified` (0), `internal` (1), `server` (2), `client` (3),
`producer` (4), and `consumer` (5). They classify an individual operation and
are independent of the generator's kind string. Child and optional attribute
lists may be empty; the payload must match the generator's datatype.
Ordinary parentage does not require an inverse child list. Located child block
locators cover only EVENT and STAGE records.

### 3.1 Unlocated events and stages

```rust
struct UnlocatedEvent {
    id: TransactionId,
    time: Timestamp,
    name: StringId,
    attributes: Attributes,
}

struct UnlocatedStage {
    id: TransactionId,
    name: StringId,
    lane: StringId,
    begin: Timestamp,
    end: Option<Timestamp>,
    attributes: Attributes,
}
```

Each unlocated event or stage belongs to one ordinary transaction. Names and
lanes are interned strings. A lane groups stage intervals within a transaction;
it is not a hierarchy node or an independently queryable generator.

These records have global record IDs allocated at creation, greater than their
owner ID. They can be relation endpoints and are resolvable through global ID
lookup. They cannot be runtime parents. Their order preserves insertion order rather
than timestamp order. Repeated names, repeated lanes, overlapping intervals,
and records outside the owner's time interval are permitted.

An absent stage end means open through the owner's end. Its effective end is
`max(stage.begin, owner.end)`. Explicit ends must be at least the stage begin.
Readers preserve the distinction between an absent end and an explicit end.

### 3.2 Located child block locators

An owner stores the sorted, distinct block numbers of every RECORD_BLOCK
containing at least one of its located EVENT or STAGE children. Block numbers
identify sections within the file as defined in section 8; they are not
transaction IDs or byte offsets. The list contains no TX_BLOCK locators and
no blocks without a matching child. LOG records do not contribute locators,
even when their parent is this owner. A log's parent supports navigation from
the log to the operation that produced it; this list indexes located activity
for transaction expansion. It is not an inverse index of every parent field.
Finding all logs associated with an owner requires a separate log search or
reader-built index.

To expand an owner, the reader selects EVENT and STAGE segments in the listed
blocks, then selects records whose parent equals the owner's ID. LOG segments
are excluded. Every located child must be covered by its owner's list.
Multiple visits to one generator remain distinct records with
distinct global IDs. Sorting the selected children by ID reconstructs their
creation order, which is also their insertion order within the owner. A reader
can expose separate event and stage lists by generator kind.

A located child's owner is an ordinary transaction; LOG, EVENT, STAGE, and
CLOCK records do not own located children. Each located child is created after
its owner, so its parent ID is smaller than its own ID. This does not impose
timestamp order or temporal containment.

For example, owner 42 may list blocks `[12, 15]`. Block 12 contains stage 103
with generator 18 and parent 42; block 15 contains event 104 with generator 19
and parent 42. Either block may also contain logs or other owners' records;
these are excluded from the result. Neither the generator IDs nor the
individual child IDs are repeated in owner 42.

Locators identify blocks, not individual pages or rows. Section 8.4 defines
how readers retrieve an owner's children.

## 4. Located events and stages

```rust
struct LocatedEvent {
    id: TransactionId,
    generator: GeneratorId,
    time: Timestamp,
    parent: TransactionId,
    payload: TypedPayload,
}

struct LocatedStage {
    id: TransactionId,
    generator: GeneratorId,
    begin: Timestamp,
    end: Timestamp,
    parent: TransactionId,
    payload: TypedPayload,
}
```

`LocatedEvent` uses an EVENT generator; `LocatedStage` uses a STAGE generator.
Both require an ordinary transaction as their parent. Arguments follow the
generator's declared schema.

Each generator represents a specific design location. Repeated activity at
that location produces separate records. A general transaction interface
exposes EVENT with equal begin and end, and both types with status `unset`
and span kind `unspecified`. These implied fields are not stored. Unlocated
and located records may coexist within one owner.

STAGE records always store a finite end. Ending an owner closes any active
located stages at `max(stage.begin, owner.end)`. This does not encode whether
that end was explicit or imposed by owner finalization. Producers needing
that distinction must declare an argument for it.

### 4.1 Resource occupancy

Producers are responsible for exclusivity when a STAGE generator represents
one resource slot. Simultaneous independent slots use separate generators.
Overlaps do not invalidate the file and are not rejected by the writer or
reader. Neither retains completed stage history solely to check exclusivity.

Consumers can detect overlapping positive-duration intervals from the stored
facts; presentation is outside VTR. Zero-duration stages do not occupy an
interval. Coincident
EVENT records are valid and must remain individually accessible.

## 5. Logs

A LOG generator represents a logging call site. Its name is the message format
string. Its payload datatype is normally a struct whose declaration supplies argument
names and types in field order, starting at zero. Producers without argument names use `"0"`, `"1"`, and
so on. Placeholders may repeat or reorder these indexes.

| Metadata | Type | Meaning |
|---|---|---|
| `log.severity` | u64 | Trace 0, debug 1, info 2, warn 3, error 4, fatal 5; default 2 |
| `log.file` | str | Optional source file |
| `log.line` | u64 | Optional source line |
| `log.func` | str | Optional source function |

Other severity values rank numerically. Source metadata identifies the log
call site. Design-source mappings and presentation remain outside VTR.

```rust
struct Log {
    id: TransactionId,
    generator: GeneratorId,
    time: Timestamp,
    parent: Option<TransactionId>,
    payload: TypedPayload,
}
```

`generator` identifies a LOG generator, and arguments follow its declared
schema. A parent identifies the operation during which the message was
produced. Logs have zero duration, implied status `unset`, and implied span
kind `unspecified`. They can participate in relations and be resolved through
global ID lookup.
They have no embedded events, stages, or located child block locators.

Placeholders are `{}`, `{N}`, `{:spec}`, or `{N:spec}`, where `N` is a zero-based
decimal argument index. An omitted index uses the next automatic index, starting
at zero; an explicit index does not advance it. `{{` and `}}` render literal
braces. For example, `"{1} {} {}"` uses argument indexes 1, 0, and 1.

Format specifications such as `#010x` and `>8.3` are reader-defined presentation;
the reference formatter's conventions are described in
[LOGGING.md](LOGGING.md#format-strings). Writers store format strings verbatim
without validating their syntax. Malformed formats and unsupported format
specifications do not invalidate a file; their rendering is reader-defined.

Argument count is determined by the schema, not by parsing the format. A
placeholder index outside the schema renders as `{?}`; a missing record argument
is still a writer error. Readers expose the original format and typed named
arguments independently of message formatting.

Text arguments are deduplicated within each generator segment. Use `text` for
dynamic or repeated message strings, `str` for strings explicitly interned in
the file string table, and `bytes` for opaque payloads.

### 5.1 Run ending and provenance

Integrations may emit typed diagnostic LOG sites under the reserved root scope
`vtr.run`, for example to record a stop signal, exit status or fault address.
These are observations supplied by the integration, using ordinary LOG schemas
and severity metadata. The reference does not install simulator signal handlers
or synthesize such observations after a failure.

Completion comes from container provenance. A normally finalized directory has
flags zero; a sealed or recovered directory remains explicitly incomplete.
Without a final directory, a reader scans the surviving complete prefix and
reports incompleteness and discarded bytes. Missing diagnostics do not imply
normal termination, and an ending log cannot make an incomplete trace complete.
Recovery preserves observations and rebuilds lookup/provenance data without
inventing a recovery log, owner or payload.

## 6. Relations

```rust
type RelationEndpoint = u64;

struct Relation {
    kind: StringId,
    source: RelationEndpoint,
    destination: RelationEndpoint,
    attributes: Attributes,
}
```

A relation is a directed edge. A `RelationEndpoint` is either zero, meaning
outside the file, or a nonzero `TransactionId` referring to any of the globally
identified record types. Relations have no separate
transaction ID or timestamp and are stored in TX_BLOCK independently of
ordinary transaction records.

Kinds are producer-defined, such as dependency or causality classifications.
Structural ownership uses parent fields. A producer may additionally emit a
`parent_of` relation, but readers do not require it to interpret parentage.
Relations may refer to records stored later in the file.

## 7. Clocks

A CLOCK generator records steady stretches of rising edges using the
`Transaction` structure from section 3. Its payload datatype is unit. Starting a
stretch allocates and returns its global record ID, usable in relations. A stretch begins
at its first edge and ends at its last edge. For multiple edges, `vtr.period`
is a positive `time` attribute and `end - begin` must be an exact multiple of the period.
A single-edge stretch has equal begin and end and omits the period.

A stretch has no parent, events, stages, or located child block locators. Its
span kind is `unspecified` and its status is `unset`, or `open` if still
running at file close. Stretches of the same clock never share an edge:
each begins after the preceding stretch's last edge.

A stretch with period `p` has edges `begin + k*p`, for
`0 <= k <= (end - begin)/p`. The first recorded edge is cycle zero; numbering
continues across stretches in chronological order. At time `t`, the cycle
is the number of the last edge at or before `t`, or absent before the first
edge. Cycle numbers are computed rather than stored.

The generator metadata `vtr.clock`, of type `u32`, associates any generator
with the node ID of a CLOCK generator. Node zero is a valid association; absence
means no declared clock. Clock generators are normally named after the clock
net and placed in its scope to permit path-based waveform matching.

Writers should place a clock's stretches in blocks containing only that
generator. At close, an active stretch ends at its last edge at or before
`max(begin, current_waveform_time)`. If writer time precedes the first edge,
the result is a single-edge stretch at `begin`.

## 8. Storage

The container supplies section framing, CRC, compression codecs, directory
entries, and string and hierarchy declarations. All referenced strings and
generators must be declared before their block. RECORD_BLOCK stores located
events, located stages, and logs in a paged layout. TX_BLOCK stores ordinary
transactions, clocks, and relations in independently compressed pages.

Types and strings are checked against declarations preceding each block, not
against declarations appearing later in the final file.

Both block kinds carry a nonzero `u64 block_number`, unique across RECORD_BLOCK
and TX_BLOCK within the file. The writer assigns increasing numbers when
records enter a block, before background encoding. Numbers never wrap or
get reused and need not match physical section order.

The 32-byte file header retains the magic and version fields. Version is 3.0;
flags at offsets 12–15 are zero. Offsets 16–31 contain a nonzero random 128-bit
recording ID. A new recording or rewrite of observations gets a new ID;
byte-preserving copies retain it. Recovery retains the execution's ID.
This identity is not a checksum or content authentication mechanism.

DIRECTORY starts with `u64 entry_count, u64 recovered_dropped_bytes`. Flag bit 1
(value 2) marks incomplete sealed/recovered output; all other directory flags
are zero. Normal close has flags zero and dropped bytes zero. The dropped count
records bytes discarded by that recovery operation. A rebuilt directory does
not make incomplete observations complete. The trailer framing is unchanged.

Version 3 DIRECTORY entries occupy 48 bytes, replacing SPEC.md section 2.3's
40-byte entries:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | `u32 kind` |
| 4 | 4 | `u32 flags` |
| 8 | 8 | `u64 section_header_offset` |
| 16 | 8 | `u64 section_payload_length` |
| 24 | 8 | `u64 aux0` |
| 32 | 8 | `u64 aux1` |
| 40 | 8 | `u64 block_number` |

For RECORD_BLOCK and TX_BLOCK, `aux0/aux1` are their `t_min/t_max`, and
`block_number` identifies the block. Other section kinds store zero in
`block_number` and retain their SPEC.md auxiliary-field meanings. DATATYPES uses
`aux0/aux1` for first type ID and declared count. Entries remain
in physical file order. Block numbers are unique across both block kinds.

Readers construct the block-number-to-section map directly from DIRECTORY,
without reading individual block headers. Generator, ID-range, segment, and page
metadata still comes from block headers and may be loaded on demand. Recovery
reads block numbers and bounds from surviving block headers when rebuilding
DIRECTORY; it preserves numbers and never infers them from section order.

A *page* is an independently compressed blob. In RECORD_BLOCK it contains one
or more complete *segments*: sequences of records from one generator, each with
its own column set and delta state. A generator may span several segments and
pages within a block. Segment headers select generators, parent ranges, and
time ranges before decompression; segment byte offsets select columns after
their page is decoded. A TX_BLOCK page instead contains one complete column
set for its transactions and relations, as defined in section 8.3.

### 8.1 Column sets and scalar encodings

Fixed-width integers are little-endian. `varint` means unsigned base-128;
`svarint` means ZigZag followed by varint. A column set starts with
`varint column_count`, then that many varint column byte lengths, followed by
the concatenated column streams. All columns must be consumed exactly.
Unknown columns or invalid lengths are errors for this format version.

![A RECORD_BLOCK segment contains one column set: framing gives column count and byte lengths, followed by time, ID, parent, duration, and five argument streams.](diagrams/transaction-columns.drawio.svg)

Delta state resets to zero at each RECORD_BLOCK segment or TX_BLOCK page.
Signed ID, parent, and time differences use wrapping 64-bit subtraction,
interpreted as signed, with inverse wrapping addition on decode. This permits
the full unsigned domain and negative differences. Duration addition must not
overflow: `begin + duration` must fit in `u64`.

An optional parent is an unsigned varint: zero for absent, otherwise
`id - parent`. A present parent satisfies `0 < parent < id`, so its distance is
positive and subtraction does not wrap. A reader rejects a nonzero distance
that is greater than or equal to the record ID. This encoding applies to
ordinary transactions and logs. Located events and stages use the mandatory
signed parent delta column in section 8.2.

Unsigned scalar varints are limited to 64 bits, except for the single unlocated
stage-end value `2^64` in section 8.3. Writers use the shortest base-128
representation. Only signed delta and offset reconstruction wraps.

The exceptional value has nine bytes `80` followed by `02`, in hexadecimal.
A codec can recognize that code directly and use ordinary `u64` varints for
every other value; it does not need a general 65-bit integer decoder. No other
value above `u64::MAX` is valid. Timestamps retain the full `u64` domain.

Each column set has these five argument streams. Records consume arguments
in declaration order, selecting streams by the declared type:

| Stream | Contents |
|---|---|
| numeric | bool as byte; signed integers as svarint; unsigned integers, time, pointer and sequence counts as varint; fixed/ufixed raw then signed scale |
| floating | f32 as 4 little-endian bytes; f64 as 8 little-endian bytes |
| text | Varint index into this column set's text dictionary |
| dictionary | Varint entry count, then length-prefixed UTF-8 entries |
| miscellaneous | str as varint string ID; bytes as length-prefixed bytes; bits/logic/logic9 as varint width then packed states |

Dictionary entries are distinct text values in first-occurrence order. Empty
dictionaries encode a zero count. A dictionary belongs to one segment or one
TX_BLOCK page; no page depends on another page's dictionary or delta state.
Payload traversal comes from the shared datatype declaration. Counts for struct
fields and fixed arrays, field names and scalar tags are omitted from records.
Unit consumes no bytes. Scalar widths and value ranges remain declared,
including exact binary32/binary64 bits, fixed-point signed scale, packed-logic
width, zero padding and valid state codes.

### 8.2 RECORD_BLOCK

A RECORD_BLOCK may contain any combination of EVENT, STAGE, and LOG generators.
Each segment contains records from one generator, whose declared kind selects
the time, parent, and duration encodings. No per-record kind tag is stored.

![A RECORD_BLOCK has uncompressed headers and independently compressed pages. Page 0 holds segments for generators 18 and 19; page 1 holds segments for generators 18 and 20. Generator 18 spans both pages.](diagrams/transaction-pages.drawio.svg)

The section payload has the following layout:

```text
u64 block_number
u64 record_count
u64 min_id, max_id
u64 t_min, t_max
u32 segment_count, page_count

segment_count * {
    u32 generator_node_id
    u32 page_index
    u32 record_count
    u32 raw_offset, raw_length
    u64 min_id, max_id
    u64 min_parent, max_parent
    u64 t_min, t_max
}

page_count * {
    u32 blob_offset, blob_length
    u32 raw_length
}

page blobs
```

Blocks, segments, and pages are nonempty. Only EVENT, STAGE, and LOG generators
are permitted. The segment table gives complete generator membership; no
per-record generator column is stored. Segment ID bounds describe its records.
Parent bounds are the minimum and maximum present parent IDs. They are `(0, 0)`
when all parents are absent; absent LOG parents do not lower a nonempty range.
For EVENT/STAGE, every parent is present. These ranges permit conservative
owner filtering; different segments' parent ranges can overlap.
Segment time bounds are minimum and maximum timestamps for LOG and EVENT, or
minimum begin and maximum end for STAGE. Block bounds are the extrema over all
segments, including when kinds are mixed. The block record count is the sum of
segment record counts.

Page indexes are zero-based. Each page blob uses SPEC.md section 3.1 framing
and is independently decompressible. Blob offsets are relative to the start
of the page-blob area. Pages occur in index order, with no gaps or overlaps;
their lengths consume the remainder of the section. The page table's raw
length must agree with both the blob framing and the decompressed length.

Segment offsets refer to decompressed bytes in their page. The segment table
is ordered by page index and then raw offset. Segments partition each page
without gaps or overlaps, and each segment contains exactly one column set.
Repeated entries for a generator are allowed and need not be adjacent, as in
the illustration. Their table order defines its record sequence within the
block. Each segment's record count, raw offset, and raw length, and each page's
blob offset, blob length, and raw length must fit their `u32` fields. Lengths
include column or blob framing as appropriate. Writers reject a record or
block that cannot be represented. Readers compute range sums in a wider type
and check them against the containing page or section before indexing or
allocating. The 64 KiB page target is distinct from these encoded limits.

Every segment uses nine columns. The generator kind determines their contents:

| Index | Column | Encoding |
|---|---|---|
| 0 | time | LOG/EVENT: svarint timestamp delta; STAGE: svarint `begin - previous_end` |
| 1 | id | Svarint record ID delta |
| 2 | parent | LOG: optional-parent encoding; EVENT/STAGE: svarint parent ID delta |
| 3 | duration | STAGE: varint `end - begin`; LOG/EVENT: empty column |
| 4–8 | arguments | Numeric, floating, text, dictionary, miscellaneous |

For a STAGE segment, `previous_end` initially equals zero and advances to the
decoded end of each record. Negative gaps encode overlaps and out-of-order
times without a special case. Mandatory parent deltas are relative to the
previous parent in that segment, initially zero. A zero delta repeats that
parent; it never means absent. The reconstructed parent must be nonzero and
smaller than the child ID.

The recommended writer policy targets 64 KiB of decompressed bytes per page,
including column framing and text dictionaries. Split large generator groups
between records into independently encoded segments, and pack smaller segments
together up to that target. A record cannot span segments or pages. A record
whose column set alone exceeds the target occupies a page by itself; the
target is not a format limit. Readers accept other page sizes. The writer must
account for framing and dictionary bytes when deciding whether a page fits.

The recommended writer policy places LOG segments in separate pages from
EVENT/STAGE segments. After stable grouping, partition the generator buckets
by this distinction and close the current page at the boundary. This adds at
most one partial page per block and avoids decompressing LOG payloads during
owner expansion. Preserve each generator's assignment order.

Readers must accept both mixed and single-kind pages and blocks. Separate LOG
block buffers are an optional policy: they can reduce log-driven block turnover
and header storage, but larger EVENT/STAGE blocks can increase the child rows
scanned for one owner. The comparison report measures complete navigation with LOG segments on
separate pages.

The reference defaults to Zstandard level 1; each blob may use any
container-supported codec and settings. Store uncompressed bytes when compression does not reduce
the page size. Codec choice remains a writer policy; the complete old/new comparisons use
LZ4 on both sides to isolate layout and implementation costs.

### 8.3 TX_BLOCK

The section payload has the following layout:

```text
u64 block_number
u64 transaction_count, relation_count
u64 min_id, max_id                 ordinary and clock IDs; zero if none
u64 t_min, t_max                   smallest begin, largest end; zero if none
u64 min_parent, max_parent         present ordinary parents; zero if none
u64 min_embedded, max_embedded     embedded event/stage IDs; zero if none
u64 rel_min_from, rel_max_from, rel_min_to, rel_max_to
u32 generator_count, page_count
generator_count * u32 generator_node_id

page_count * {
    u64 transaction_count, relation_count
    u64 min_id, max_id             zero when this page has no transactions
    u64 t_min, t_max               owner/clock bounds; zero if none
    u64 min_parent, max_parent     present ordinary parents; zero if none
    u64 min_embedded, max_embedded embedded IDs; zero if none
    u64 rel_min_from, rel_max_from, rel_min_to, rel_max_to
    u32 blob_offset, blob_length, raw_length
}

page blobs
```

The generator list is unique, sorted by node ID, and names the ordinary
transactions and clocks physically present. Located child blocks do not add
their generators to this list. The encoder collects the generator IDs and
assigns their local indexes before encoding transaction columns. The bounded
transaction buffer already fixes block membership; single-pass emission of
columns before collecting that membership is not required.

Each page is one independently compressed column set. All pages use the same
block-local generator table, but reset their delta and argument-dictionary
state. One transaction, its arguments, attributes, embedded details, and child
block list remain in one page. Relations do not span pages. Page packing targets
64 KiB of decompressed bytes, including column framing and dictionaries; an
oversized item occupies a page alone, subject to the encoded field limits.

Within a block, transactions and clock stretches are sorted by ID before
splitting them into pages. Transaction ID ranges are therefore ordered and
disjoint within that block. Relation-only pages follow pages containing
transactions. A page may contain both transactions and relations. Relations
retain their sequence independently of transaction sorting. Sorting never moves
an item to a different numbered block.

Page offsets are relative to the start of the page-blob area. Blobs use SPEC.md
section 3.1 framing, occur in page-table order, and consume that area without
gaps or overlaps. Each raw length agrees with the blob framing and decompressed
length. The `u32` field limits and checked range arithmetic from section 8.2
also apply here. Independently compressed raw blobs must be smaller than 1 GiB
under the inherited compression framing. This is a format limit, separate from
page targets and reader resource budgets.

Block counts are sums of page counts. Block ID/time bounds are extrema over
pages containing transactions; present-parent and embedded-ID bounds are
extrema over their present values, or zero when absent; relation endpoint bounds are extrema over pages
containing relations. Endpoint ranges include zero. All four endpoint bounds
are zero when their relation count is zero. A relation-only block has no
generators or transaction bounds. Blocks and pages with neither transactions
nor relations are not emitted.

The fixed block header is 128 bytes and each page header is 124 bytes.
Each decompressed page contains one column set with 33 columns:

| Index | Column | Encoding |
|---|---|---|
| 0 | id | Svarint transaction ID delta |
| 1 | generator | Varint block-local generator index |
| 2 | begin | Svarint begin delta |
| 3 | duration | Varint `end - begin` |
| 4 | status/kind | Byte `(span_kind << 4) \| status` |
| 5 | parent | Optional-parent encoding from section 8.1 |
| 6 | attribute count | Varint count per keyed attribute list |
| 7 | attribute key | Varint string ID |
| 8 | attribute tag | Byte value tag, upper three bits zero |
| 9 | attribute numeric | bool byte; signed/unsigned varints according to type |
| 10 | attribute floating | f32 as 4 little-endian bytes; f64 as 8 little-endian bytes |
| 11 | attribute string | Varint string ID for str or enum literal |
| 12 | attribute miscellaneous | Payload for tags 13–17 and 23–24 |
| 13 | unlocated event count | Varint per transaction |
| 14 | unlocated event time | Svarint offset from owner begin |
| 15 | unlocated event name | Varint string ID |
| 16 | unlocated stage count | Varint per transaction |
| 17 | unlocated stage name | Varint string ID |
| 18 | unlocated stage lane | Varint string ID |
| 19 | unlocated stage begin | Svarint offset from owner begin |
| 20 | unlocated stage end | Varint 0 for open, otherwise `end - begin + 1` |
| 21 | relation kind | Varint string ID |
| 22 | relation from | Svarint source ID delta |
| 23 | relation to | Svarint destination offset from source ID |
| 24–28 | arguments | Numeric, floating, text, dictionary, miscellaneous |
| 29 | located child block count | Varint per transaction |
| 30 | located child block number | Varint first number, then positive deltas within that owner's list |
| 31 | unlocated event ID | Svarint event ID delta within the page |
| 32 | unlocated stage ID | Svarint stage ID delta within the page |

The located block list is sorted and distinct. Its delta state resets for each
owner, independently of other columns. Zero first numbers and zero subsequent
deltas are invalid; reconstruction must not overflow. CLOCK records have a
zero block count. No individual located child ID or generator ID is stored in
the owner.

Keyed attribute lists are consumed for each transaction, then each of its
unlocated events, then each of its unlocated stages; relation attribute lists
follow all transactions in that page. Arguments are consumed once per
transaction using its generator schema. Embedded child lists retain insertion
order.

Attribute numeric payloads follow the value table. An enum consumes its
signed numeric value and its literal from the string stream. Fixed and ufixed
consume raw and scale from the numeric stream. Floating-point attributes share
the floating stream; each value's tag determines whether it consumes four or
eight bytes. Argument floating widths are determined by the generator schema.
Lists and maps in the miscellaneous stream retain nested tags. Attribute text
is inline, while
schema argument text uses the page's dictionary. For an explicit unlocated
stage end, encode `end - begin + 1` without wrapping to the open marker. When
`begin = 0` and `end = u64::MAX`, use the exceptional code from section 8.1.
A codec can test for that duration before incrementing, and reconstruct it
without a wider integer type. Decoded end addition must still fit in `u64`.

TX_BLOCK uses the same configurable codec policy as RECORD_BLOCK.
Stable generator grouping applies only to RECORD_BLOCK; TX_BLOCK page packing
preserves the block's sorted transaction-ID sequence.

### 8.4 Order and queries

The writer assigns ordinary transactions and stages to blocks on finalization;
events and logs are assigned on emission. RECORD_BLOCK has one assignment
sequence shared by EVENT, STAGE, and LOG under the recommended buffer policy;
a writer using separate LOG buffers has separate located-child and log
sequences. TX_BLOCK has a separate sequence for ordinary transactions, clocks,
and relations. Blocks contain consecutive portions of their sequence, with block numbers increasing along that sequence.
Background encoders may complete out of order. Declarations must still
precede their uses in the physical file, and readers must not infer
record order from file offsets.

Within each RECORD_BLOCK, stable grouping preserves assignment order for each
generator. Segment splitting also preserves that order. Across generators,
physical order has no semantic meaning and does not recover stage completion
order. Global IDs recover creation order, including log and event production
order. TX_BLOCK stores transactions in ID order within each block; its physical
row order does not preserve finalization order. Neither creation nor
finalization order implies timestamp order.

For a closed query window `[a, b]`, where `a <= b`, candidate blocks and segments
satisfy `t_min <= b` and `t_max >= a`. A positive-duration stage intersects the
window when `begin <= b` and `end > a`; a point matches when
`a <= time <= b`. Ordinary transaction queries use `begin <= b` and `end >= a`.
Clock queries select stretches by these bounds and derive edges from the
period. TX_BLOCK page headers further prune ordinary and clock queries.
Embedded event and stage times do not expand these bounds: these queries select
owner intervals, and embedded records are accessed through selected owners.
An embedded record outside its owner's interval does not make that owner match
an owner-interval query. A reader offering a separate time search over embedded
activity must inspect otherwise excluded owner blocks or build another index.

A located-track query selects matching generator segments by their headers,
decompresses their pages once each, and decodes only the selected segments.
It then filters individual records by time. Pages may contain unrelated
segments, which incur decompression but not record-decoding work. A scope query
resolves descendant generators from the hierarchy. Log severity queries filter
generators by their declared metadata. Owner payloads are not required to load
located intervals, points, or their declared arguments.

Expanding an owner uses its block locators to select EVENT and STAGE segments
whose parent ranges contain the owner ID, decompresses their pages once each,
and selects children by exact parent ID. LOG segments are skipped even if their records have the same parent. Pages holding
only LOG segments need no decompression for this query; mixed pages may still
require decompressing log data. Owner time bounds cannot filter children
because ownership does not require temporal containment. A reader may
build a transient parent-to-record index while decoding a block and reuse it
for later expansions. The format does not contain an owner-to-page index;
parent bounds only provide conservative pruning. Uncached owner expansion
may still read every page containing EVENT/STAGE segments in a referenced block
when their parent ranges all contain the owner ID.

![Owner 42 lists child blocks 12 and 15. The reader visits those RECORD_BLOCKs and selects EVENT and STAGE records whose parent is 42, excluding LOG and other owners' records.](diagrams/transaction-child-blocks.drawio.svg)

Global ID lookup filters block ranges, then RECORD_BLOCK segment ranges or
TX_BLOCK page ranges. Within one TX_BLOCK, binary search over the ordered
transaction-page ID ranges selects at most one page for one ID. A reader can batch multiple IDs,
with each selected page decompressed once; the reference currently exposes
individual owning lookups. Relation lookup
uses block and page endpoint ranges.

Ranges across blocks can overlap, including when a long-lived owner finishes
alongside recently created owners. Even a sorted page containing one old owner
and several recent owners can have a broad range. These headers select
candidates; they are not an exact global ID-to-row map. One lookup may require
pages from several blocks. Readers cache decoded owners and stop searching
once every requested owner has been resolved.

The reference builds a block-number map and parses segment/page headers at
open. It caches decoded owners and waveform histories on demand, with entry/byte
budgets for owners, and does not retain a cross-query record-page cache.
Consumers may add bounded page or parent indexes and must charge their setup
and memory in comparisons. Section CRC verification still
covers the entire section payload and may require reading the other compressed
pages; independent pages bound decompression, not the I/O needed for full
section integrity verification.

## 9. Writer lifecycle

The writer declares a generator and its schema before emitting records.
Creating an ordinary transaction allocates its ID and opens an owner handle.
Creating a located child requires an active owner and allocates its global ID.
Events and logs supply complete arguments when created and are submitted
immediately; stages can be submitted once their ends and arguments are known.
Required arguments must be supplied before finalization, including at file
close. Missing arguments are a writer error; close must not fabricate values
or emit an incomplete argument list.

The submission path assigns each finalized record to a block before handing
the block to a background encoder. The reference uses one bounded RECORD_BLOCK stream for EVENT/STAGE/LOG,
with LOG segments separated into pages, and a TX_BLOCK stream for owners and
relations. Clock finalization flushes before and after its stretch, giving it a
separate block. A writer using separate LOG buffers may share the block-number
allocator; the EVENT/STAGE stream must still reserve increasing numbers.
A block's first record reserves its block number. For a located child,
assignment also records that number in the active owner. A record must
never migrate to another numbered block during grouping, page packing, or
compression: its owner may already have been serialized with that locator.

The writer maintains one block list for each active owner. EVENT/STAGE block
numbers increase along their submission stream, so a located EVENT or
STAGE child appends a number only when it differs from that owner's last
entry. Logs do not update the list. The list is already sorted and distinct
at owner finalization, without a merge, per-child hash set, or scan of
completed records.

Ending an owner closes remaining active stages at
`max(stage.begin, owner.end)` and submits them to numbered blocks before
serializing the owner. It then fixes the owner's child block list. No children
may be attached after finalization. The owner need not wait for child page
encoding or physical I/O; forward block references are valid. The owner retains
no completed child payload after submission. Buffer and queue ownership keeps
that payload alive until encoding finishes.

The recommended RECORD_BLOCK buffer limits are 65,536 finalized records and
4 MiB of buffered record payload; reaching either limit flushes the block.
An oversized record that fits the encoded limits is accepted alone. Byte
accounting must include
variable-length arguments.
TX_BLOCK uses a separate 4 MiB target; embedded detail, optional attributes and
relations count toward it. Source waveform capture has a 16 MiB logical byte
target and a 16,777,216-observation limit. A split may fall between observations
at the same timestamp;
all occurrences and next-block initial state must survive. One observation can
exceed a target and is accepted alone when it fits format limits. These are
capture/accounting targets, not exact allocator or RSS ceilings.

The public writer serializes producer calls, copies borrowed payloads before
returning, assigns IDs/block numbers and fixes owner locators on capture.
A signal-update span amortizes synchronization over a time step without
coalescing event occurrences; its handles/kinds are preflighted before changes.
An owning final-payload overload validates readiness before moving the bytes and
ending the owner under one lock, avoiding an extra copy and lock acquisition.
The typed `end_value` overload serializes inline into reusable owning storage
and ends under the same lock. Borrowed metadata/type views require external
serialization; configure metadata immediately after construction, before binding
types or declaring sources, because these operations commit META. Mutation
methods serialize internally.
Immutable datatype/generator snapshots remain valid for already submitted jobs
when declarations are added. Owning waveform batches retain initial frames,
time/value streams, signal kinds and predecessor state; workers never borrow
mutable simulator or declaration state.

Worker mode performs record transposition/grouping, page packing, waveform
alias/transform/run construction, time-table encoding, compression, checksums
and output. Capture still performs payload copying/validation, waveform value
normalization/deduplication, time-index header packing and current-state updates.
The reference is not a claim that all encoding or allocation has left simulator
threads. Synchronous mode uses the same codecs and remains available for equal
single-CPU comparisons.

Bounded queues retain charges through queued, encoding, ready and writing states,
until output buffers are released after section output. Both job count and a
conservative byte charge limit admission; output follows submission order even
when encoding completes in reverse order. Declarations precede uses and wave
blocks retain capture order. A single individually oversized job is admitted
only to an empty queue and excludes later submissions until written. Its count
and peak charge are exposed. Producers block under backpressure; records are
never silently dropped. Workers do not acquire the producer mutex while a
producer waits. Queue statistics expose waits, current/peak jobs and charges.
Codec workspace, producer current buffers, active state and global metadata are
separate from those queue limits.

The reference recycles payload and completed-row storage after encoding has
produced an independent owning output buffer. Its retained-capacity budget
defaults to 16 MiB in worker mode and zero synchronously; explicit budgets
override either choice. Source-local spares and shared worker returns each
receive half the budget, including cached vector metadata. Full caches release
spare storage without dropping observations. Active input/output, allocator
overhead and codec workspace are outside this budget. Recycling does not release
a job’s queue charge before writing finishes.

The first unexpected allocation, encoding or I/O failure poisons the writer,
wakes blocked producers and prevents normal close. Shutdown joins workers before
closing output state and cancels unprocessed jobs. No normal directory/trailer
is appended after failure. Explicit seal or destruction with unfinished records
retains completed observations with incomplete provenance; it never fabricates
mandatory payloads. Missing owner/stage payloads are rejected before end/close
changes active records. Argument errors are retryable when they have not poisoned
the writer. Flush drains submitted work; close drains before the final directory.
Stdio close does not imply fsync or durable storage.

RECORD_BLOCK encoding gathers records into generator buckets in first-occurrence
order, preserving order within each bucket. This linear pass requires no ID or
timestamp sorting. Partition the buckets into EVENT/STAGE and LOG groups for
the recommended page policy, preserve bucket order within each group, then
split buckets into segments and pack pages as specified in section 8.2.
TX_BLOCK encoding sorts buffered transactions by ID, collects the generator
table, and splits the owner and relation column sets into independent pages.
Grouping and encoding state, including argument dictionaries and pending pages,
is discarded after the block is encoded.

At file close, unfinished ordinary transactions receive status `open` and end
`max(begin, current_waveform_time)`. Their active located stages close at that
effective owner end, clamped to their begins. Unlocated stage ends may remain
absent and are interpreted through the owner's end. Explicit writer end calls
clamp an end earlier than its begin; malformed on-disk negative durations are
not valid. Clock finalization follows the last-edge rule in section 7.

The writer checks complete payload shape and types, key uniqueness, generator kind,
active-owner validity, and local duration constraints. It does not retain
completed records to check cross-record properties such as stage exclusivity.

Memory consists of declarations, active records, distinct block locators of
active owners, bounded record/encoding buffers, and bounded queued work.
Per-owner locator memory grows with the number of touched blocks. In the worst
case of one child per block it still grows with child count. Large individual
records and active owners' embedded details can exceed the normal buffer
targets; memory is not bounded independently of active owner size.

## 10. Reader validation and recovery

Readers reject invalid encodings, undeclared generators, mismatched record
and generator kinds, unsupported datatype definitions, malformed counts, invalid
status/kind codes, and out-of-range indexes. Page and segment byte ranges must
partition their containers as specified. Loaded column sets must consume every
column exactly, and decoded record counts and bounds must agree with headers.
Lazy queries validate the data they load; full validation additionally checks
unloaded pages and aggregate block bounds. TX_BLOCK transaction pages have
ordered, disjoint ID ranges and sorted transaction IDs. Parent bounds agree
with the present parents in each loaded RECORD_BLOCK segment. Directory block
numbers agree with their section headers; lookup must not rely on physical
section order.

Full waveform validation also checks time-table bounds, dirty-group directory
partitions and exact predecessor links, legal logic states and padding, and
checkpoint frames against the preceding decoded values. Event-signal duplicate
occurrences remain present. A checksum alone cannot establish frame/history
consistency.

Globally unique record IDs and block numbers, acyclic ownership, relation
endpoint existence, clock associations/periods/overlaps, label types, payload and optional
attribute collisions and owner-locator consistency are complete-file invariants.
For each owner, its locator list must equal the set of RECORD_BLOCKs containing
EVENT or STAGE records with that parent. LOG records are excluded from this
invariant. Full validation may check these separately; loading one track must
not trigger a whole-file ownership scan.

Forward references are valid. A complete, normally finalized file must resolve
every nonzero relation endpoint, parent, and located block locator. A growing,
sealed, or recovered file may lack referenced records or blocks whose sections
were never written, even if recovery or sealing appended a final directory.
Readers preserve available records and expose missing owners, endpoints, and
child blocks. A missing child block makes that owner's child set incomplete;
its missing child IDs and count cannot be inferred from the locator alone.

Recovery retains complete container sections with valid integrity checks.
Pages do not introduce partial-section recovery: an incomplete block is dropped
as a whole. Recovery preserves surviving block numbers and does not renumber
them when rebuilding the directory, so existing locators remain meaningful.
Recovery does not append observations, invent missing owners, or fill mandatory
payloads. A rewrite that changes observations is a new recording with a new ID.

Readers expose arguments under their declared names, preserve optional
attributes, distinguish embedded records from located records, and retain all
records involved in resource overlaps. Cached decoded results remain valid
for callers while held, even if the reader evicts its own cache entry.

## 11. Derived activity indexes

Derived activity remains separate from the trace. A reusable source fingerprint
contains the recording ID, finalized file length and CRC-32 of the DIRECTORY
payload. A reader without a finalized directory exposes no reusable fingerprint.
The binding assumes immutable observations after finalization; it is not a full
content digest. Recovery preserves recording identity but changes the directory's
incomplete/drop provenance and therefore invalidates the original fingerprint.

The reference sidecar is VTI version 3.0. File-header flags/reserved bytes are
zero. Its HEADER extends the previous 40-byte payload with the source recording
ID at offsets 40–55. Source format is 1; signal count and first waveform time
must match; disk/memory budget fields are zero. BLOCK, TAIL, 40-byte sidecar
DIRECTORY entries and trailer keep their previous framing. HEADER, one BLOCK per
source wave block in order, TAIL and DIRECTORY are required; every CRC is checked.
Identity is checked before decoding activity rows. Older sidecars are regenerated.

The reference uses threshold one/exponent zero: adjacent integer-time activity
points merge, other silences remain explicit. Queries are exact; the first wave
time is excluded and repeated same-time occurrences count as one activity point.
The decoder validates source block/time bounds, stretches, counts, directory
entries and its configured budget for input, decoded rows and raw workspace.
Adaptive summaries, cache selection and atomic publication belong to the cache
layer rather than this byte API.

## 12. Reference implementation and evaluation

The C++20 reference in `atemp/vtr3-reference` implements the model, full container,
waveform codec, typed aggregates, both record blocks, bounded worker handoff,
reader navigation, failure/recovery handling and source-bound activity sidecars.
It is experimental; production version-2 support is unchanged. Regenerate older
assets when adopting version 3 rather than adding compatibility machinery.

The default record policy uses 64 KiB raw pages, 4 MiB record block targets,
a 16 MiB waveform capture target, 65,536 completed records per block, separate
log pages/clock blocks and Zstandard
level 1. Codec and waveform batching are policies, not changes to identity or
query semantics. A synchronous mode establishes format costs on one CPU; worker
counts and shared CPU affinity must be stated for asynchronous measurements.

[VTR3_COMPARISON.md](VTR3_COMPARISON.md) records source/build provenance,
complete repeated comparisons with version 2, FST, FTR and VCD, all existing
benchmark metrics, located-stage/event workloads and cold/warm navigation costs.
Its reported limits are adoption gates: compression or narrow-query wins do not
by themselves demonstrate the write-speed and producer-cost targets in
[RATIONALE.md](RATIONALE.md). The current reference still has material write
regressions relative to version 2; asynchronous speedups must be judged alongside
CPU use, capture stalls, final drain and memory.

Validation includes independent Python schema/column decoders, an independent
Rust waveform reader, an independent producer replay for every measured read
and query, malformed input/full-file invariants, payload expansion limits,
concurrent producers with late declarations and mutable caller buffers,
reordered/saturated worker queues, bounded buffer reuse across owner-slot
lifetimes, injected allocation and I/O failures, truncation/recovery and ASan/UBSan/ThreadSanitizer runs. Independent waveform
checking adapts only container header/directory framing; it does not re-encode
waveform sections. Reference reader limits default to 64 MiB of reconstructed
canonical primary payload per record and a 4,096-entry/32 MiB accounted owner
cache; they are configurable and are not a global process-memory ceiling.

No simulator-specific capacity claim follows from these synthetic runs. Validate
real producer rates and large/fast design traces before setting integration
budgets. SystemC site wrappers, formatting, VDB interpretation, transport and
visualization remain separate components.
