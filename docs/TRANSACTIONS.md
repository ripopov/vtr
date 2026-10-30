# Typed records and payloads

This is the normative typed-record portion of **VTR 3.4**. [SPEC.md](SPEC.md)
defines the container, compression, metadata, hierarchy, tagged attributes,
waveforms, recovery, and activity sidecars. Together these documents describe
the implemented format.
[RATIONALE.md](RATIONALE.md) explains the storage and API choices.

## 1. Identity, sources, and ownership

Every ordinary transaction, clock stretch, located stage/event/log, and embedded
stage/event has one nonzero global `u64` record ID. IDs are allocated in creation
order, independently of timestamps, completion order, and producer scheduling.
They are never reused or wrapped. Two recordings may reuse numeric IDs; recording
identity disambiguates them. Relations use zero for an endpoint outside the file.

A generator is a hierarchy node with one kind string. Ordinary transaction,
event and stage generators bind one payload datatype; clocks and logger
generators declare unit. Each log segment selects a datatype independently of the logger.
Datatypes can carry arbitrary tagged attributes, including application metadata.
Declarations are immutable and precede every block that uses them. New sources and
datatypes may be declared during capture. Source metadata describes
the source once; it is not a default for missing payload fields.

| Generator kind | Records | Storage |
| --- | --- | --- |
| Producer-defined | Ordinary interval transactions | TX_BLOCK |
| CLOCK | Constant-period clock stretches, unit payload | TX_BLOCK |
| EVENT | Located point records | RECORD_BLOCK |
| STAGE | Located intervals | RECORD_BLOCK |
| LOG | Located log points with datatype-selected arguments | RECORD_BLOCK |

EVENT/STAGE/LOG/CLOCK are reserved kind strings. A generator may be a root or a
child of any scope. Only an ordinary transaction can own child records. Its
optional parent is another ordinary transaction active when the child is created.
A present parent ID is smaller than the child ID, preventing ownership cycles.
Ending a parent does not end its ordinary child transactions.

Ownership does not imply temporal containment. A located stage or event can lie
outside its owner's interval. A located record identifies its design location
through its generator; an embedded record has a name/lane inside the owner and
no independent generator. Every present runtime reference resolves in a complete
file; incomplete recordings may expose missing targets.

An ordinary transaction stores ID, generator, begin/end, status, span kind,
optional parent, canonical typed payload, optional attributes, embedded events
and stages, and sorted distinct located-child block numbers. Status codes are
0 unset, 1 ok, 2 error, 3 aborted, 4 open. Span-kind codes are 0 unspecified, 1 internal, 2 server, 3 client,
4 producer, and 5 consumer; they are independent of generator kind. Intervals require `end >= begin`.

Embedded events store global ID, timestamp, name string ID, and attributes.
Embedded stages store global ID, name/lane string IDs, begin, optional end, and
attributes. An empty lane name selects the default stage lane. The C++ API
accepts an omitted lane or `{}` for this case; the Rust API uses string ID `0`.
An absent end means open; a present end is at least begin.
Relations store kind string ID, source/destination global IDs or zero, and
attributes. Relation endpoint existence is checked at full-file validation;
forward references are permitted.

Owners list RECORD_BLOCK numbers containing their located EVENT/STAGE children.
LOG records do not add locators even when they have that owner as a parent.
The list is fixed before owner serialization and equals the actual set of child
blocks in a complete recording. Logs need no parent. Located events and stages
always have one. Ending an owner finalizes its remaining located stages before
serializing the owner and its complete locator list.

## 2. Datatype declarations (kind 10)

The DATATYPES payload is uncompressed: `varint first_type`, `varint count`, then
that many declarations. IDs 0–22 are predefined. Declared IDs start at 23 and
extend the dense table in order; references within a declaration must identify
an earlier type. Each declaration begins with `u8 kind_and_flags`, `blob UTF-8 name`, then
the body selected by the low seven bits:

| Kind | Declaration body |
| ---: | --- |
| 1 structure | `varint field_count`, then `blob field_name`, `varint field_type` per field |
| 2 fixed array | `varint element_type`, `varint element_count` |
| 3 sequence | `varint element_type` |
| 4 enumeration | `varint underlying_type`, `varint literal_count`, then `u64 value_bits`, `blob literal_name` per literal |

Bit `0x80` indicates that the declaration body is followed by a nonempty tagged
attribute list using [SPEC section 5.3](SPEC.md#53-attributes-and-values). Without
that bit there are no attribute bytes. Empty metadata therefore adds no bytes
to ordinary declarations. Unknown kinds and empty flagged attribute lists are
invalid. Attribute key IDs are unique; every key and nested string reference must
be declared before the DATATYPES section that uses it.

Attributes are immutable metadata on the declared datatype, independent of its
payload layout. VTR validates their generic encoding and references. Applications
interpret their keys and values. Logging conventions are defined in
[LOGGING.md](LOGGING.md); they introduce no special datatype or hierarchy kind.
Predefined primitive IDs have no mutable metadata. A structure can wrap a
primitive or a shared argument datatype and carry its own attributes. A one-field
structure adds no canonical payload bytes. Native type caches reuse unannotated
argument schemas without merging declarations that carry different metadata.

Field and enum literal names are nonempty and unique within the declaration.
Type names may be empty and need not be unique. Identical declarations may have
different IDs; automatic bindings can reuse them. Enum underlying types are signed/unsigned integer
primitives; values fit that type, sign-extended to 64 bits when signed. Multiple
names may represent the same numeric value. Enum payloads preserve the underlying
integer, including unnamed values. Nesting depth is at most 64, and computed
minimum/fixed sizes fit `u32`. There are no recursive types or native ABI layouts.

## 3. Canonical typed payloads

A source requires exactly one complete value of its datatype per record. Unit
means no bytes. Structures concatenate fields in declaration order without
padding or alignment. Arrays concatenate their declared number of elements.
Sequences begin with a fixed little-endian `u32` element count, then elements.
Enumerations use the canonical representation of their underlying integer.

| Type ID | Primitive | Canonical representation |
| ---: | --- | --- |
| 0 | unit | Empty |
| 1 | bool | One byte, 0 or 1 |
| 2–5 | i8/i16/i32/i64 | Fixed-width little-endian two's complement |
| 6–9 | u8/u16/u32/u64 | Fixed-width little-endian unsigned |
| 10–11 | f32/f64 | Four/eight IEEE bytes, preserving all bits |
| 12 | str | `u32` container string ID |
| 13 | text | `u32` byte length, then UTF-8 |
| 14 | bytes | `u32` byte length, then opaque bytes |
| 15–17 | bits/logic/logic9 | `u64` bit width, then packed states from SPEC section 5 |
| 18 | time | `u64` timestamp or duration |
| 19 | enum slot | Requires a declared enumeration; not a standalone payload type |
| 20 | pointer | `u64` address |
| 21 | fixed | `i64 raw`, `i64 scale` |
| 22 | ufixed | `u64 raw`, `i64 scale` |

Fixed-point value is `raw * 2^-scale`. Canonical text length and sequence count
are fixed-width values, distinct from their compressed-column encodings. Payload
bytes, reconstructed lengths, padding, UTF-8, bool values, and all string IDs are
validated. Native Rust/C++ padding, object pointers, and host endianness never
appear on disk. A canonical value must consume its payload exactly.

Recording APIs may defer or replace a transaction or located-stage payload while
the record is active. The final value must be present before ending that record,
ending its owning transaction, or closing normally. A fixed zero-size datatype
is already complete without supplied bytes; variable-length types still require
their lengths, even when empty. Only the final complete value is serialized.

Typed payloads are separate from optional tagged attributes. Attribute widths,
keys, tags, and nested list/map values follow [SPEC section 5.3](SPEC.md#53-attributes-and-values).
A payload field named `vtr.label` has type str or text. It supplies an instance
label without replacing source identity or its optional attributes.

## 4. Column framing and schema traversal

A general column set is `varint column_count`, then one `varint byte_length`
per column, followed by the streams in order. Every stream must be consumed
exactly. Delta state resets for each RECORD_BLOCK segment and TX_BLOCK page.

Signed ID, parent, and time differences use wrapping 64-bit subtraction,
interpreted as signed and ZigZag-encoded. Decode uses inverse wrapping addition.
Only those deltas and embedded time offsets wrap. Duration addition is checked.
An optional parent is varint zero when absent, otherwise `record_id-parent_id`;
a present distance is strictly between zero and record ID.

A present embedded-stage end stores `end-begin+1`, while zero means open.
The one exceptional value `2^64`, for begin zero/end u64::MAX, is nine bytes
`80` followed by `02` in hexadecimal. Other varints remain within `u64`.

Schema traversal visits structure fields and array elements in order; a sequence
first consumes its element count, then recursively visits its elements. Enums
use their integer underlying type. The five argument streams are:

| Stream | Encoded values |
| --- | --- |
| Numeric | Bool byte; signed integers as svarint; unsigned integers/time/pointer as varint; sequence counts as varint; fixed raw and scale as numeric varints |
| Floating | f32/f64 IEEE bytes without widening |
| Text references | One varint per text value, interpreted using the dictionary descriptor |
| Dictionary | Descriptor and local UTF-8 entries below |
| Miscellaneous | str ID as varint; bytes as blob; packed bit width as varint followed by packed states |

Payload keys, tags, and structure/array counts are derived from the datatype and
not repeated in these streams. Sequences store their runtime count. Unit fields
consume no stream bytes. Numeric ranges and reconstructed allocation sizes are
validated against the datatype and reader budgets.

The dictionary begins with `descriptor = (local_entry_count << 1) | shared`.
Then come local UTF-8 blobs, distinct and in first-use order. Every local entry
must be used when its complete column set is consumed. With shared zero, text
references are local indexes. With shared one, an even reference is a local
index shifted left one; an odd reference is a container string ID shifted left
one, with its low bit set. Shared references are permitted only in TX pages and
must refer to strings declared before that block. No page depends on another
page's dictionary or delta state. Canonical decoded text contains its original
length and UTF-8 bytes regardless of dictionary choice.

The format permits different shared-text admission policies. The C++ writer
admits text of at least 256 bytes within a 4 MiB shared-text budget, then reuses
admitted IDs. Rust currently uses local dictionaries. Both produce the same
canonical payloads and accept either encoding. Shared declarations are assigned
before jobs are encoded and published before dependent blocks.

## 5. RECORD_BLOCK (kind 9)

A RECORD_BLOCK may contain any combination of EVENT, STAGE, and LOG generators.
Each segment contains records from one generator and, for logs, one datatype.
The generator kind selects time, parent and duration encodings. The segment
datatype selects log payload decoding; other records use their generator datatype.
No per-record kind tag is stored.

The section payload has the following layout:

```text
u64 block_number
u64 record_count
u64 min_id, max_id
u64 t_min, t_max
u32 segment_count, page_count

segment_count * {
    u32 generator_node_id
    u32 payload_datatype_id  (LOG only)
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
per-record generator column is stored. LOG headers include a datatype
ID; EVENT/STAGE headers retain their 68-byte layout and have no datatype selector.
LOG headers are 72 bytes. The selected datatype must be available before the
block, and its schema-guided columns must consume every argument byte.
Segment ID bounds describe its records.
Parent bounds are the minimum and maximum present parent IDs. They are `(0, 0)`
when all parents are absent; absent LOG parents do not lower a nonempty range.
For EVENT/STAGE, every parent is present. These ranges permit conservative
owner filtering; different segments' parent ranges can overlap.
Segment time bounds are minimum and maximum timestamps for LOG and EVENT, or
minimum begin and maximum end for STAGE. Block bounds are the extrema over all
segments, including when kinds are mixed. The block record count is the sum of
segment record counts.

Page indexes are zero-based. Each page blob uses [SPEC.md section 3.1](SPEC.md#31-compressed-blob) framing
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
| 4–8 | arguments | Numeric, floating, text references, dictionary, miscellaneous (section 4) |

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
owner expansion. Preserve assignment order within each generator/datatype group.

Readers must accept both mixed and single-kind pages and blocks. Separate LOG
block buffers are an optional policy: they can reduce log-driven block turnover
and header storage, but larger EVENT/STAGE blocks can increase the child rows
scanned for one owner. The comparison report measures complete navigation with LOG segments on
separate pages.

The reference defaults to Zstandard level 1; each blob may use any
container-supported codec and settings. Store uncompressed bytes when compression does not reduce
the page size. Codec choice remains a writer policy; the complete old/new comparisons use
LZ4 on both sides to isolate layout and implementation costs.

## 6. TX_BLOCK (kind 5)

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

Page offsets are relative to the start of the page-blob area. Blobs use [SPEC.md section 3.1](SPEC.md#31-compressed-blob) framing, occur in page-table order, and consume that area without
gaps or overlaps. Each raw length agrees with the blob framing and decompressed
length. The `u32` field limits and checked range arithmetic from section 5
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
### 6.1 General pages

A general decompressed page contains one column set with 33 columns:

| Index | Column | Encoding |
|---|---|---|
| 0 | id | Svarint transaction ID delta |
| 1 | generator | Varint block-local generator index |
| 2 | begin | Svarint begin delta |
| 3 | duration | Varint `end - begin` |
| 4 | status/kind | Byte `(span_kind << 4) \| status` |
| 5 | parent | Optional-parent encoding from section 4 |
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
| 24–28 | arguments | Five schema-guided streams (section 4) |
| 29 | located child block count | Varint per transaction |
| 30 | located child block number | Varint first number, then positive deltas within that owner's list |
| 31 | embedded event ID | Svarint delta from previous embedded event ID in the page |
| 32 | embedded stage ID | Svarint delta from previous embedded stage ID in the page |

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
is inline, while schema payload text uses the argument dictionary or shared
string table (section 4). For an explicit unlocated
stage end, encode `end - begin + 1` without wrapping to the open marker. When
`begin = 0` and `end = u64::MAX`, use the exceptional code from section 4.
A codec can test for that duration before incrementing, and reconstruct it
without a wider integer type. Decoded end addition must still fit in `u64`.

TX_BLOCK uses the same configurable codec policy as RECORD_BLOCK.
Stable generator grouping applies only to RECORD_BLOCK; TX_BLOCK page packing
preserves the block's sorted transaction-ID sequence.

### 6.2 Scalar pages

The TX_BLOCK header, sorted generator table, page headers, independently
compressed blobs, and query bounds are unchanged. Each decompressed page begins
with either varint column count 33, or discriminator byte 2 for the
layout below. Other discriminators are invalid.

```text
u8  discriminator = 2
u32 payload_type_id
u32 row_count
repeat 6 + payload_word_count times:
    u8 width
    width byte planes, each containing row_count bytes
```

Integers in this description use little-endian fixed width unless a field says
otherwise. The payload datatype is a fixed aggregate of zero or more scalar
leaves whose effective primitive is `u64`, `time`, or `pointer`; enums with an
unsigned-64 underlying type qualify. Its canonical byte size is a multiple of
eight and at most 2,048 bytes. `payload_word_count` is that size divided by eight,
derived from the declaration and not repeated in the page.

| Column | Unsigned value |
|---|---|
| 0 | Absolute global record ID |
| 1 | Index in this block's generator table |
| 2 | Absolute begin time |
| 3 | Duration, `end - begin` |
| 4 | Absolute parent ID, or zero |
| 5 | `(span_kind << 4) \| status` |
| 6 onward | Canonical payload words in declaration order |

All page rows are ordinary transactions with the declared payload type. Optional
attributes, embedded events/stages, and child-block lists are empty; the page
contains no relations. A parent need not be absent. Generators, ownership,
classification, times, and payloads obey all the usual validation rules.
Transactions with located-child locators use the general layout.

`width` is the smallest number of bytes sufficient to represent the largest
value in that column, from zero through eight. Zero means all values are zero
and stores no plane bytes. For nonzero width, the last plane must contain a
nonzero byte. Plane j contains byte j of every row's value, in row order:

```text
plane[j][i] = (column[i] >> (8 * j)) & 255
```

Rows have strictly increasing IDs within and across this block's pages. Widths,
counts, type eligibility, generator/type agreement, duration overflow,
classification, parentage, and decoded page bounds are validated. The body must
be consumed exactly. The reader retains plane views and reconstructs one record
at a time; it does not materialize a page-sized matrix of decoded transactions.

The C++ and Rust writers select this layout when a complete TX input batch qualifies
and contains no relations. Otherwise it uses general pages. For K columns, its
conservative row target is
`max(1, floor((page_bytes - min(page_bytes, 9 + K)) / (8 * K)))`.
This bounds ordinary scalar pages by the raw target without repeatedly scanning
column lengths. As with general pages, one row may exceed a deliberately tiny
configured page target.

## 7. Queries, validation, and lifetime

A valid closed query window `[a,b]` has `a <= b`. Block and page ranges are
conservative filters. A located positive-duration stage intersects when
`begin <= b && end > a`; a zero-duration stage or point matches when
`a <= time <= b`. Ordinary transaction and clock queries use inclusive interval
intersection `begin <= b && end >= a`. Embedded record times do not expand owner
bounds. Locators select children regardless of owner/child temporal containment.

Track queries select generator/datatype segments and matching time/parent/ID bounds
before decompressing pages. A page may contain unrelated segments; decompression
is page-sized, while decoding can be segment-specific. Full section CRC checking
still reads the entire compressed section. Independent pages bound decompression,
not all checksum I/O.

Expanding an owner selects its child block numbers and EVENT/STAGE segments whose
parent bounds contain the ID, then filters exact parents. LOG segments are
excluded. Missing child blocks make an incomplete expansion; their missing
record count cannot be inferred. ID lookup prunes block/segment/page ID ranges;
ranges across blocks can overlap. TX pages within a block have disjoint sorted
ID ranges, but broad ranges may still select unrelated rows. Retained payloads
own their schema and storage independently of reader caches and reader lifetime.

Creation IDs, timestamps, completion order, and physical storage order are
separate. Stable grouping preserves each located generator/datatype group's sequence
within its block. TX pages sort IDs within a block. Queries do not promise chronological
order across generators; clients sort explicitly when needed. Waveform readers
provide a chronological visitor separately from storage-order visitation.

Full validation checks every page and sample, global record uniqueness, datatype
availability, valid classifications, ownership, locators, relation endpoints,
and clock consistency. A complete file has no unresolved nonzero references.
An unfinished or incomplete file reports surviving records and missing references.
Invalid data remains an error in either case; incomplete is not permission to
ignore malformed payloads or duplicate IDs.

## 8. Clock semantics

A CLOCK source has unit payload. A run records a nonzero period and first edge;
stopping it at t retains edges `first + k*period <= max(t,first)`. The stored
interval ends at the last retained edge. A single-edge stretch has no period
attribute; a longer one has `vtr.period` of type time, nonzero and dividing its
end-minus-begin. Stretches are disjoint in edge time. Starting a new period
while running closes the previous run before the new first edge.

An active clock is finalized at recording close as open, like other live owners.
`vtr.clock` source metadata associates another generator with a clock via a u32
node ID. Clock edges and cycle indexes are derived from the stretches; they do
not allocate one global record per edge.

## 9. Implementation contracts

The C++ `Schema<T>` and Rust `Payload` traits bind native types to immutable file
schemas. Rust's `impl_payload!` and `impl_payload_enum!` macros provide declarative
bindings, including integer enum wrappers that preserve unnamed values.
Capture owns payload values before returning. Typed source and owner
handles retain recording identity and reject use with a different writer. Rust
uses compact process-local ownership tokens, separate from the persistent file
ID; tokens are never reused, including after a writer is dropped.
Producer time advances independently from explicit record timestamps.

Inline mode runs capture, assembly, encoding, and output synchronously and uses
one implicit producer. Threaded mode separates bounded capture, time-ordered
assembly, and optional encoding workers. Each logical producer has exclusive
mutable access. Assembly respects producer frontiers and preserves per-producer
order; stalled frontiers can spill pending owned operations instead of growing
RAM without bound. Encoder results are published in submission order.

C++ and Rust use bounded SPSC capture rings with independent producer byte
admission. They have the same ownership, ordering, and completion
contracts, without sharing implementation code. The implementations live in
[`vtr-cpp`](../vtr-cpp/src/engine/) and Rust
[`recording`](../vtr-rs/src/recording/mod.rs). Queue budgets and encoding job budgets
are distinct from encoded page targets. A single oversized capture can exceed
its nominal queue byte target while alone; encoded field limits still apply.

Rust reuses payload buffers within `EncodingOptions::payload_pool_bytes` (16 MiB
by default; zero disables reuse). Encoding workers return buffers in batches.
`WriterStats::payload_pool_bytes` reports retained spare capacity separately from
active payload capacity and encoding admission. Reader payload views retain their
record's storage; fields and collection elements share it without copying bytes.

Normal close requires every explicit producer to finish and reports errors.
It finalizes remaining live owners/stages/clocks as open. Sealing marks the
recording incomplete. The Rust [process guard](../vtr-guard/) can
independently stop new captures, wake blocked producers, and seal accepted
operations while producer handles remain live. A panicked mutable backend or
an inline call interrupted in place retains only a verified file prefix.
The C++ SDK currently has no native process crash guard.
