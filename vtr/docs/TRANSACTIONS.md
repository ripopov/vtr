# VTR transactions

Status: proposed; production format not implemented.

This document specifies the transaction model, encoding, and reader and writer
behavior for format version 3.0. [SPEC.md](SPEC.md) defines the current version-2
container, compression, string table, and hierarchy framing. This proposal
inherits that framing except where explicitly replaced below. TX_BLOCK retains
section kind 5; RECORD_BLOCK uses kind 9 and replaces LOG_BLOCK storage.
Adoption requires regeneration of older trace assets.

Every transaction record has a file-wide ID. Events and stages associated with
design locations are stored separately from their owning transactions, grouped
by generator and packed into independently compressed pages. Each owner lists
the blocks containing its children. The recommended writer settings are 64 KiB
pages for both block kinds and Zstandard level 1. Section 12 gives the performance rationale.

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
    arguments: Vec<ArgumentDeclaration>,
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

### 2.1 Mandatory arguments

```rust
type ArgumentType = u8;

struct ArgumentDeclaration {
    name: StringId,
    value_type: ArgumentType,
}
```

Every generator declares an ordered list of arguments. An empty list is
valid. Names are unique within the schema. Each record supplies exactly
one value of the declared type for every argument, in declaration order.
There are no optional positions or implicit default values.

`ArgumentType` accepts only the tags below. Values use the corresponding
`Value` variant from section 2.2.

| Tag | Type | Value variant | Meaning |
|---|---|---|---|
| 1 | bool | `Bool` | Boolean |
| 2 | i8 | `I8` | Signed 8-bit integer |
| 3 | i16 | `I16` | Signed 16-bit integer |
| 4 | i32 | `I32` | Signed 32-bit integer |
| 5 | i64 | `I64` | Signed 64-bit integer |
| 6 | u8 | `U8` | Unsigned 8-bit integer |
| 7 | u16 | `U16` | Unsigned 16-bit integer |
| 8 | u32 | `U32` | Unsigned 32-bit integer |
| 9 | u64 | `U64` | Unsigned 64-bit integer |
| 10 | f32 | `F32` | IEEE binary32 |
| 11 | f64 | `F64` | IEEE binary64 |
| 12 | str | `Str` | Interned string ID |
| 13 | text | `Text` | UTF-8 text, deduplicated within one column set (section 8.1) |
| 14 | bytes | `Bytes` | Opaque byte sequence |
| 18 | time | `Time` | Unsigned value in file time units |
| 20 | pointer | `Pointer` | Unsigned address |

The generator-specific hierarchy payload is `varint kind_string_id`, then
`varint argument_count`, then that many `{varint name_string_id, u8 type_tag}`
pairs. The enclosing hierarchy record supplies its ID, name, parent and metadata.
Names and kind strings use the file string table. A schema change requires a
new generator declaration with a distinct node ID.

Arguments are exposed as named attributes by readers. Ordinary transactions
can also carry optional keyed attributes; these must not duplicate a declared
argument name. Located events, located stages, and logs carry only their
schema-defined arguments. Their encoded records omit argument counts, keys,
and type tags.

Readers can determine each generator's argument columns from its declaration
without reading records. Tables combining generators must account for their
different schemas.

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
    arguments: Vec<Value>,
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
lists may be empty; the argument list must match the generator's schema.
Ordinary parentage does not require an inverse child list. Located child block
locators cover only EVENT and STAGE records.

### 3.1 Unlocated events and stages

```rust
struct UnlocatedEvent {
    time: Timestamp,
    name: StringId,
    attributes: Attributes,
}

struct UnlocatedStage {
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

These records have no independent transaction ID. They cannot be relation
endpoints or runtime parents. Their order preserves insertion order rather
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
    arguments: Vec<Value>,
}

struct LocatedStage {
    id: TransactionId,
    generator: GeneratorId,
    begin: Timestamp,
    end: Timestamp,
    parent: TransactionId,
    arguments: Vec<Value>,
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

The viewer detects overlapping positive-duration intervals on a located stage
track and renders the conflicting portions as `XXX`, retaining every record
for inspection. Zero-duration stages do not occupy an interval. Coincident
EVENT records are valid and must remain individually accessible.

## 5. Logs

A LOG generator represents a logging call site. Its name is the message format
string. Its argument schema supplies argument names and types in argument index
order, starting at zero. Producers without argument names use `"0"`, `"1"`, and
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
    arguments: Vec<Value>,
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

### 5.1 Run ending

The root scope `vtr.run` is reserved for run-ending logs. An abnormal ending
is described by one record from a FATAL LOG generator under this scope, at
the current writer time, as the last log created, with the greatest log ID:

| Generator format string | Ordered arguments | Meaning |
|---|---|---|
| `run stopped by signal {}` | signal: i64 | Stop request followed by normal close |
| `run exited with status {}` | status: i64 | Program exited without closing |
| `run crashed by signal {} (code {}, address {}, thread {}, sealed {})` | signal: i64, code: i64, address: pointer, thread: u64, sealed: bool | Fatal signal; sealed means only data already handed to the encoder was retained |
| `run recovered by scanning, {} bytes dropped` | dropped: u64 | Recovery rewrote the file |
| `run sealed after a panic in the writer` | Empty | Writer failed mid-call and retained encoded data |

Signal numbers belong to the producing platform. A reader reports the record
with the greatest ID under `vtr.run`. Without such a record, a file with a final
directory indicates normal close; a file recovered without a directory has an
unknown ending and is reported as recovered.

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
`Transaction` structure from section 3. Its schema is empty. A stretch begins
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

The generator metadata `vtr.clock`, of type `str`, associates any generator
with a clock by its hierarchical path joined with `.`. An absent association
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

Both block kinds carry a nonzero `u64 block_number`, unique across RECORD_BLOCK
and TX_BLOCK within the file. The writer assigns increasing numbers when
records enter a block, before background encoding. Numbers never wrap or
get reused and need not match physical section order.

Version 3 DIRECTORY entries occupy 48 bytes, replacing SPEC.md section 2.3's
40-byte entries. The entry count and trailer framing are unchanged:

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
`block_number` and retain their SPEC.md auxiliary-field meanings. Entries remain
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
| numeric | bool as byte; i8/i16/i32/i64 as svarint; u8/u16/u32/u64, time, pointer as varint |
| floating | f32 as 4 little-endian bytes; f64 as 8 little-endian bytes |
| text | Varint index into this column set's text dictionary |
| dictionary | Varint entry count, then length-prefixed UTF-8 entries |
| miscellaneous | str as varint string ID; bytes as length-prefixed bytes |

Dictionary entries are distinct text values in first-occurrence order. Empty
dictionaries encode a zero count. A dictionary belongs to one segment or one
TX_BLOCK page; no page depends on another page's dictionary or delta state.
Argument counts, keys, and type tags come from the generator declaration and
are omitted from records.

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
scanned for one owner. Section 12.4 measures this tradeoff.

Zstandard level 1 is recommended. Each blob may use any container-supported
codec and settings. Store uncompressed bytes when compression does not reduce
the page size. Section 12 compares compression and throughput.

### 8.3 TX_BLOCK

The section payload has the following layout:

```text
u64 block_number
u64 transaction_count, relation_count
u64 min_id, max_id                 ordinary and clock IDs; zero if none
u64 t_min, t_max                   smallest begin, largest end; zero if none
u64 rel_min_from, rel_max_from, rel_min_to, rel_max_to
u32 generator_count, page_count
generator_count * u32 generator_node_id

page_count * {
    u64 transaction_count, relation_count
    u64 min_id, max_id             zero when this page has no transactions
    u64 t_min, t_max               owner/clock bounds; zero if none
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
also apply here.

Block counts are sums of page counts. Block ID/time bounds are extrema over
pages containing transactions; relation endpoint bounds are extrema over pages
containing relations. Endpoint ranges include zero. All four endpoint bounds
are zero when their relation count is zero. A relation-only block has no
generators or transaction bounds. Blocks and pages with neither transactions
nor relations are not emitted.

Each decompressed page contains one column set with 31 columns:

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

TX_BLOCK uses the same recommended Zstandard level-1 policy as RECORD_BLOCK.
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
transaction-page ID ranges selects at most one page for one ID. Multiple IDs
should be batched, with each selected page decompressed once. Relation lookup
uses block and page endpoint ranges.

Ranges across blocks can overlap, including when a long-lived owner finishes
alongside recently created owners. Even a sorted page containing one old owner
and several recent owners can have a broad range. These headers select
candidates; they are not an exact global ID-to-row map. One lookup may require
pages from several blocks. Readers cache decoded owners and stop searching
once every requested owner has been resolved.

Readers build header-based block-number and generator/segment indexes. They
cache decoded pages and records on demand. Section CRC verification still
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
the block to a background encoder. The recommended buffer policy uses one
ordered stream of bounded RECORD_BLOCK buffers for EVENT, STAGE, and LOG and
another for TX_BLOCK. Separate LOG buffers may use the same block-number
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
Bounded encoder queues apply backpressure when producers outrun encoding.
TX_BLOCK uses a separate byte budget, also 4 MiB, so large embedded
details and relations also count toward its memory budget.

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

The writer checks schema arity and types, key uniqueness, generator kind,
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
and generator kinds, unsupported schema types, malformed counts, invalid
status/kind codes, and out-of-range indexes. Page and segment byte ranges must
partition their containers as specified. Loaded column sets must consume every
column exactly, and decoded record counts and bounds must agree with headers.
Lazy queries validate the data they load; full validation additionally checks
unloaded pages and aggregate block bounds. TX_BLOCK transaction pages have
ordered, disjoint ID ranges and sorted transaction IDs. Parent bounds agree
with the present parents in each loaded RECORD_BLOCK segment. Directory block
numbers agree with their section headers; lookup must not rely on physical
section order.

Globally unique record IDs and block numbers, acyclic ownership, relation
endpoint existence, and owner-locator consistency are complete-file invariants.
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
Records or blocks appended during recovery must use identifiers above all
surviving or referenced values in their respective namespaces, including
unresolved references to missing data.

Readers expose arguments under their declared names, preserve optional
attributes, distinguish embedded records from located records, and retain all
records involved in resource overlaps. Cached decoded results remain valid
for callers while held, even if the reader evicts its own cache entry.

## 11. Performance and validation criteria

Performance evaluation covers writer CPU and memory, compressed storage, and
reader latency for both track queries and transaction inspection.

Before adoption, establish reproducible benchmarks covering the complete
writer, reader, and container, including owner payloads, argument dictionaries,
checksums, directory/header costs, and caches. Measure writer throughput and
peak memory, complete scans, uncached track loading, owner expansion, parent
label lookup, and repeated navigation. Report decoded-cache state separately
from filesystem-cache state and actual disk or remote I/O.

Exercise ordinary transactions with and without schemas, mixed generators,
skewed generator traffic, large arguments, out-of-order timestamps, long-lived
owners, repeated visits, overlaps, coincident points, open records, logs,
clocks, relations, and forward references. Include blocks and pages mixing
EVENT, STAGE, and LOG generators, and verify that a log whose parent is an
owner is excluded from that owner's located children. Verify segment splits reset all
delta and dictionary state; one page decodes independently; owner finalization
assigns every child block before serialization; and asynchronous completion
does not change locators or per-generator order. Test truncation after an
owner block but before its referenced child blocks, and the reverse order.

Persistent owner-to-page and exact ID indexes require evidence that faster
uncached owner expansion and label lookup justify their write, memory, and
storage costs.

## 12. Performance rationale

Prototype measurements support the recommended layout and writer settings.
The stage-only prototypes in sections 12.1–12.3 use different headers from
section 8 and omit full container framing, integrity checks, and directory
costs. Section 12.4 adds a file-backed mixed-workload study. These results
describe their workloads and do not establish production throughput.

### 12.1 Workloads and method

The synthetic model contains 200,000 instructions from a 4-wide out-of-order
pipeline, with seven located stages per instruction and 58 STAGE generators.
Execute latency and issue wait vary. Owners carry PC and opcode arguments;
stages carry no arguments. IDs follow creation order, stages enter blocks on
completion, and blocks hold at most 65,536 records. This model is not a
cycle-accurate resource-occupancy simulation.

The Kanata comparison extracts 631,118 stages from the first 100,000
instructions of a qsort trace. Generators correspond to nine distinct
`(thread, lane, stage name)` tuples; these are overlapping logical locations,
not reconstructed physical slots. Sixteen active stages close at the sample
cutoff. Owner payloads, labels, and relations are excluded from native timings.

Native results use an optimized Rust prototype, VTR's compression and varint
code, one pinned core of an Intel Core Ultra 7 265K, and Zstandard level 1.
Each reported timing is the median of five passes after a warm-up. Write time
includes grouping, optional dictionary construction and sorting, serialization,
and compression. Reads include decompression and record reconstruction with
compressed input in memory, reusable buffers, and no decoded-record cache.
All layouts passed record round trips and sampled query-result comparisons.

Track latency is the average per query within the median pass over 128
generator/block selections. A query reads one generator's records from one
located block. Full scans read all stages. Neither measures the public writer
interface, disk I/O, or owner/label resolution.

### 12.2 Stage storage and speed

Synthetic results, with compressed storage reported per stage:

| Layout | Bytes/stage | Write, million stages/s | Full read, million stages/s | Track query, microseconds |
|---|---:|---:|---:|---:|
| Mixed generators in one frame | 2.381 | 32.79 | 141.46 | 402.60 |
| Stable generator groups, one frame | 1.845 | 40.99 | 167.93 | 124.56 |
| Groups with an owner-ID dictionary | 1.626 | 26.08 | 174.57 | 133.46 |
| Owner dictionary and creation-ID sorting | 1.577 | 24.95 | 184.71 | 129.45 |
| **Stable groups, 64 KiB pages** | **2.001** | **37.38** | **154.47** | **35.59** |
| Pages with an owner-ID dictionary | 1.740 | 25.24 | 164.87 | 50.94 |
| One frame per generator | 2.276 | 34.64 | 127.29 | 9.85 |

Compared with one grouped frame, 64 KiB pages use about 8% more space, retain
91% of write throughput, and reduce track-query latency by a factor of 3.5.
Owner dictionaries save more space but substantially reduce write throughput.
Sorting by ID adds another encoding pass with little benefit for track queries.

Kanata stage-only results:

| Layout | Bytes/stage | Write, million stages/s | Full read, million stages/s | Track query, microseconds |
|---|---:|---:|---:|---:|
| Mixed generators in one frame | 0.632 | 53.39 | 191.42 | 298.23 |
| Stable generator groups, one frame | 0.487 | 44.37 | 222.71 | 114.51 |
| Groups with an owner-ID dictionary | 0.455 | 23.67 | 193.65 | 137.06 |
| **Stable groups, 64 KiB pages** | **0.558** | **39.79** | **205.66** | **42.99** |

On this more compressible trace, grouping slows writing relative to mixed
records. Pages still reduce storage and track-query latency. An owner
dictionary nearly halves grouped write throughput for a smaller storage saving
than on synthetic data. Direct parent deltas offer a better balance here.

### 12.3 Owners, codecs, and deferred alternatives

A separate Python encoding estimate for the synthetic owners gave 3.05 bytes
per stage for owners containing individual `(generator ID, child ID)` pairs,
and 0.45 for owners containing distinct child block locators. These values
include owner fields and arguments divided across seven stages. They do not
measure owner write throughput or the complete paged format. The locator
choice removes substantial repeated data without adding per-child dictionary
construction or changing global record identity.

Locators do not eliminate owner-expansion work. The native synthetic queries
that scan one located block for a parent's children averaged 339 microseconds
with one grouped frame, 363 with pages, and 451 with one frame per generator.
Those timings start with the correct child block already identified and omit
owner loading. Per-generator frames favor a narrow track query at the expense
of compression and some broader reads; bounded pages provide the chosen
compromise.

For synthetic pages, Zstandard level 3 reduced stage storage from 2.001 to
1.971 bytes but reduced write throughput from 37.38 to 33.42 million stages/s.
LZ4 used 2.632 bytes/stage, wrote 41.18 million stages/s, and reduced track
latency to 16.97 microseconds. On Kanata, LZ4 pages used 0.951 bytes/stage and
reduced track latency to 36.97 microseconds. These results support Zstandard
level 1 as the recommended setting, with LZ4 available when throughput takes
priority over storage size.

The RECORD_BLOCK encoding omits owner-ID dictionaries, ID sorting, and adaptive
integer bit-packing. TX_BLOCK sorts owners for page lookup as specified in
section 8.3. The tested bit-packing variant reduced raw size but did
not improve compressed size on both workloads. Encoding times in units of a
shared time quantum also saved little after compression. These transforms
need further evidence to justify their encoding cost and format complexity.

Global IDs remain on every located record. An owner-local child ordinal could
replace them and still support relations if relation endpoints were redesigned
as compound identities. That would change the common identity model and lose
global creation order unless stored separately. The available size estimates
do not justify this added complexity.

### 12.4 File-backed mixed workloads

A standalone Rust study compared 15 layout variants over five simulated
workloads. Each file contains about 10.1 million ordinary transactions,
located stages/events, logs, relations, and signal changes. There are
128–192 interleaved ordinary-transaction generators representing cores, NoC
endpoints, and accelerators, with different argument schemas. Four waveform
counters per producer use the production signal codec in the same file.
The workloads cover normal traffic, long-lived owners, log bursts,
out-of-order timestamps/completions, and 128 KiB incompressible attributes.

The screen wrote and validated 75 files. A confirmation batch compared the
original monolithic-TX proposal, the selected layout in section 8, and an
ablation without parent bounds, using three seeds for each workload:
45 more files and 120 trials overall. Complete scans checked source digests,
global IDs, ownership, relation endpoints, exact child-block locator sets,
page partitions, header bounds, schemas, and signal changes. Sampled track,
owner, label, relation, and window queries also matched source oracles.

The machine was an Intel Core Ultra 7 265K on Linux with NVMe ext4 storage,
Rust 1.97.1, Zstandard level 1, CPU affinity 2/3/4, and two bounded encoder
workers. Write timing includes simulation, allocation, checksums, file I/O,
and fsync. Queries use filesystem-warm input with an empty decoded cache.
The following are median three-trial values; query cells are medians of
trial medians. Each cell is original proposal → selected layout.

| Workload | File, MB | Write, seconds | Owner lookup, ms | Expansion including owner, ms | Pipeline with labels, ms |
|---|---:|---:|---:|---:|---:|
| balanced | 51.13 → 54.48 | 3.60 → 3.57 | 4.14 → 0.26 | 9.15 → 2.77 | 116.96 → 10.51 |
| longtail | 52.49 → 56.00 | 3.35 → 3.83 | 4.19 → 0.27 | 10.17 → 3.03 | 174.38 → 16.52 |
| logburst | 102.76 → 105.40 | 4.53 → 5.08 | 5.36 → 0.35 | 9.38 → 1.20 | 166.68 → 15.23 |
| disordered | 63.42 → 66.29 | 4.04 → 4.44 | 15.25 → 1.00 | 23.97 → 8.11 | 156.57 → 26.96 |
| large | 571.29 → 572.80 | 3.74 → 4.47 | 4.49 → 0.16 | 7.49 → 2.55 | 145.12 → 11.17 |

Across workloads, geometric-mean speedups were 17.5-fold for full owner lookup,
3.8-fold for expansion including owner loading, and 9.9-fold for pipeline loading.
Files grew 4.1%, write wall time grew 10.8%, and writer user-plus-system
CPU time grew 39.9%. The prototype re-encodes column sets while splitting pages;
that encoder cost is included. A production encoder may improve it.

Paging without ID sorting gave only 1.36-fold owner-lookup and 1.43-fold
pipeline speedups in the screen. Sorting before slicing into pages is
important; paging alone leaves broad candidate ranges. Parent bounds cut
child-row decoding by 61.9% on logburst and 34.1% on disordered traffic.
In confirmation, adding parent bounds to the otherwise selected layout
improved logburst expansion by 1.79-fold; benefits were smaller elsewhere.
Narrower fields offset much of the extra header cost: the screen saved
1.8% of total file bytes geometrically and 4.4% on logburst. A segment with
parent bounds occupies 68 bytes rather than 80 with the original widths.

Separate LOG pages reduced irrelevant decompression without increasing
child-row counts. Separate LOG blocks saved more storage but increased
large-owner expansion from 61,712 to 81,087 child rows per query, slowing
that case. This supports a page-separation policy rather than a universal
block-separation rule. Directory block numbers removed all block-header
reads needed solely to build the block map; independent audits observed
6,803 physical block-number descents across the retained files.

The timestamp cap, integer-width normalization, and first-occurrence TX
generator tables provided no material storage benefit. Normalization also
lost attribute-width identity. A stage-end bitmap saved 0.8% of file bytes
geometrically without a stable latency benefit. The single full-duration
code can instead be handled with `u64` operations as in section 8.1.
Expanded embedded-time bounds benefited an additional embedded-activity
search, but are not needed for the owner-interval queries in section 8.4.

With full section CRC checking, pipeline queries were 6.6-fold faster.
That does not imply page-sized I/O: each selected section still needs its
complete compressed payload for CRC verification. Per-file cache-eviction
advice before opening was best effort; query input was filesystem warm.
Host timing noise was visible, so small one-pass timing differences did
not justify format changes. The study uses experimental container framing
and a full-record reader. Label projection, parent filtering without
materializing unrelated records, cold-device I/O, and production writer
integration can change absolute timings and policy tradeoffs.

### 12.5 Comparison with FTR

A file-backed comparison replayed the same synthetic and Kanata stage records
from section 12.1 and the five mixed workloads from section 12.4 through the
selected VTR prototype and the upstream
[LWTR4SC FTR writer](https://github.com/Minres/LWTR4SC/blob/8dca86925ad64b0ca73825fcefd7308215049627/src/ftr/ftr_writer.h).
The FTR header was used unchanged at that revision, through a C interface;
SystemC frontend overhead is excluded. FTR used its synchronous LZ4 backend,
default 64 KiB per-stream transaction buffers, and one stream per generator.
VTR used 64 KiB pages, Zstandard level 1, and the selected layout.

Measurements used the same Core Ultra 7 265K and NVMe ext4 filesystem,
Rust 1.97.1, GCC 15.2.0 with `-O3`, and system LZ4 1.10.0. Each cell below
is **VTR / FTR**, using the median of three isolated trials. Stage-only
processes were pinned to CPU 2. Mixed processes were pinned to CPUs 2/3/4;
VTR used two bounded encoder workers, while FTR encoded synchronously.
Writes include encoding, checksums where provided, file output, flush, and
fsync. Mixed writes additionally include simulation and format adaptation;
stage-only input preparation is excluded for both formats.

The stage-only files omit owner payloads, as in section 12.2, but include
container metadata and preserve each stage's ID, generator, parent, and times.
FTR stores the parent as a named unsigned attribute. These are complete-file
encoding and scan measurements, not the in-memory microbenchmarks above.
Throughput is in millions of stages per second; storage includes all file bytes.

| Workload | Stages | Bytes/stage | Write, million stages/s | Scan, million stages/s |
|---|---:|---:|---:|---:|
| Synthetic | 1,400,000 | 2.045 / 15.762 | 16.58 / 10.16 | 45.57 / 16.23 |
| Kanata | 631,118 | 0.572 / 12.388 | 16.81 / 10.21 | 49.58 / 17.37 |

The mixed comparison preserves the source model through explicit FTR
conventions. Arguments are named attributes, with original types identified
in their keys. Status, span kind, parent, and embedded event/stage fields
become attributes. Nested values use typed JSON text; byte arrays use base64
text. Relation attributes are carried in a reversible suffix on the native
relation kind. A metadata stream stores the source schema. A reader needs
these conventions to reconstruct VTR semantics from the FTR records.

FTR has no waveform encoding, so its four counters per producer use the same
production VTR signal codec in a companion file. FTR storage and timings below
include that file; VTR stores the counters in its main file. Both include the
same signal changes. MB means 1,000,000 bytes. Each mixed trial contains about
10.1 million independently identified records, relations, and signal changes;
embedded details are additional payload.

| Workload | File, MB | Write, seconds | Full scan, seconds |
|---|---:|---:|---:|
| balanced | 54.48 / 158.99 | 3.74 / 5.24 | 0.855 / 1.378 |
| longtail | 56.00 / 161.87 | 3.72 / 5.19 | 0.871 / 1.372 |
| logburst | 105.40 / 215.06 | 4.91 / 12.46 | 2.741 / 3.526 |
| disordered | 66.29 / 167.93 | 4.27 / 6.31 | 0.885 / 1.379 |
| large | 572.80 / 844.45 | 4.45 / 7.91 | 1.523 / 2.461 |

Across mixed workloads, geometric means give VTR 2.30-fold smaller files,
1.67-fold lower write wall time, and 1.52-fold faster scans. Writer
user-plus-system CPU time is 55% higher for VTR. A separate three-trial
balanced comparison pinned both implementations to CPU 2: VTR took
8.11 seconds to write, versus 5.05 for FTR. The mixed write wall-time advantage
therefore depends on parallel encoding; it does not establish lower writer
CPU cost. The prototype's repeated encoding during page splitting contributes
to that cost, as discussed in section 12.4.

A matched-codec control changed only VTR transaction-page compression to LZ4;
declarations and waveforms retained their shared Zstandard policy. VTR still
used 1.95-fold less total mixed-file space, wrote in 1.71-fold less wall time
with two workers, and scanned 1.58-fold faster geometrically. With one CPU,
the balanced LZ4 VTR write took 7.85 seconds, versus FTR's 5.05. On the
stage-only inputs, LZ4 VTR used 2.674 and 0.966 bytes per stage, versus FTR's
15.762 and 12.388. The storage advantage therefore extends beyond codec choice.

Reads open and scan the complete file, reconstruct source records, and compute
the same count/sum/XOR digests for transactions, located records, relations, and
signals. Every trial matched its pre-encoding source oracle and the other
formats for the same seed. The 63 main trials and 27 single-core control trials
all passed. Independent CBOR/LZ4 decoding also checked native FTR framing and
attribute types; an existing Rust FTR parser independently checked stage records.
The measured readers are specialized experimental readers, rather than a
comparison of production reader libraries. VTR verifies section CRCs; FTR
provides no equivalent checksums. These scans omit the full ownership/locator
validation performed in section 12.4, so their read times are not interchangeable.

Per-file cache-eviction advice before each scan was best effort, not proof of
cold-device I/O. Host timing variation was visible, including an outlying
logburst write; all trials were retained and medians are reported. The large
workload additionally measures base64 adaptation for a value type FTR does not
natively provide. These results compare the stated implementations and mappings;
they do not isolate an intrinsic format-only throughput limit or measure random
track queries, owner expansion, or label resolution against FTR.
