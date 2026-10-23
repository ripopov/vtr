# VTR file format specification

This document and [TRANSACTIONS.md](TRANSACTIONS.md) specify the current VTR
wire format, **3.2**, including scalar-page and shared-text encodings.
The C++ package version is independently 4.0. Activity
sidecars use VTI 3.0. File versions describe bytes, not API or package revisions.
Readers and recovery tools reject other VTR header versions. Regenerate older
recordings; there is no compatibility reader.

The independent [Rust library](../README.md) and [C++20 SDK](../../vtr-cpp/README.md)
implement this format. [RATIONALE.md](RATIONALE.md) explains its design choices.
API lifetimes, capture queues, and default buffer sizes are implementation
contracts described in those libraries, rather than additional wire fields.

Notation: `u8/u16/u32/u64` are unsigned little-endian integers; signed fixed-width
integers use two's complement. `varint` is canonical unsigned LEB128, at most ten
bytes for `u64`; its final group must be nonzero unless the value is zero, and
its tenth byte is at most 1. `svarint` applies ZigZag to a signed 64-bit value,
then varint. `blob` is a varint byte length followed by bytes. Text is UTF-8.
Lengths and offsets must be checked before allocation, addition, or slicing.

## 1. Data model

A recording contains metadata, an ordered design hierarchy, typed runtime
records, signal histories, and optional blackout markers. Scope parentage
identifies design structure. Record parentage identifies runtime ownership.
They are separate relationships. Names connect recorded objects to external
VDB design information; VTR does not prescribe viewer layouts or remote APIs.

Node, signal, string, and datatype IDs are separate zero-based `u32` namespaces.
Record IDs are nonzero file-wide `u64` values, including embedded children.
Transaction and located-record blocks share a separate nonzero `u64` block-number
namespace. IDs never wrap or get reused. A recording has its own nonzero
128-bit identity; an ID alone does not identify an object across recordings.

Timestamps are `u64` in the metadata time unit. Waveform producer time is
nondecreasing. Transaction, stage, event, and log timestamps may be out of
order and need not match producer time. Their encodings and query boundaries
are defined in [TRANSACTIONS.md](TRANSACTIONS.md).

## 2. Container

File order is a 32-byte header, complete append-only sections, a final DIRECTORY
section, and a 24-byte trailer. Sections have no alignment padding. The directory
and trailer are absent while a file is being written or after an interruption.

### 2.1 File header (32 bytes)

| Offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 8 | Magic `89 56 54 52 0d 0a 1a 0a` |
| 8 | 2 | Major version = 3 |
| 10 | 2 | Minor version = 2 |
| 12 | 4 | Flags = 0 |
| 16 | 16 | Nonzero recording identity, opaque bytes |

### 2.2 Section header (24 bytes)

| Offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 4 | Kind |
| 4 | 4 | Flags |
| 8 | 8 | Payload byte length |
| 16 | 4 | IEEE CRC-32 of the payload; zero disables the section check |
| 20 | 4 | Reserved = 0 |

Flag bit 0 marks an optional section. An unknown required kind is an error;
an unknown optional kind may be skipped. DIRECTORY alone uses bit 1 to mark
an incomplete recording. Other reserved flags are zero.

| Kind | Payload | DIRECTORY aux0 / aux1 |
| ---: | --- | --- |
| 1 | META | 0 / 0 |
| 2 | STRINGS | First string ID / count |
| 3 | HIERARCHY | First node ID / count |
| 4 | SIGNAL_BLOCK | First time / last time |
| 5 | TX_BLOCK | Minimum begin / maximum end, or 0 / 0 without transactions |
| 6 | BLACKOUT | 0 / 0 |
| 7 | DIRECTORY | Not indexed in itself |
| 9 | RECORD_BLOCK | Minimum begin / maximum end |
| 10 | DATATYPES | First datatype ID / count |

TX_BLOCK and RECORD_BLOCK entries carry their block number. Every other kind
has block number zero. A block's directory bounds and number must agree with
its payload header. Declaration chunk ranges must agree with their payloads.

### 2.3 Directory and trailer

DIRECTORY payload:

```text
u64 entry_count
u64 dropped_bytes
entry_count * {
    u32 kind, flags
    u64 section_header_offset, payload_length
    u64 aux0, aux1
    u64 block_number
}
```

Each entry is 48 bytes. Entries are in physical file order and partition all
bytes between the file header and DIRECTORY, including section headers, with
no gaps or overlaps. Entries exclude DIRECTORY. Its payload CRC is always
verified. The section's incomplete flag distinguishes a normal complete trace
from a sealed or recovered trace. `dropped_bytes` is zero for a complete trace.
It records discarded input bytes after recovery; an incomplete trace may also
have zero dropped bytes.

Trailer: `u64 directory_section_offset`, `u64 total_file_length`, then eight
bytes `56 54 52 5f 45 4e 44 00` (`VTR_END\0`). Both the recorded length and
directory extent must match the physical file exactly.

A finalized fingerprint is `(recording identity, total length, directory CRC)`.
The CRC covers the directory payload, not the section header. Identity prevents
two different recordings of equal length and directory CRC from sharing caches.

### 2.4 Publication and recovery

There is exactly one META. Strings used by metadata must precede it. HIERARCHY,
DATATYPES, and recording sections follow META. Incremental declaration chunks
may appear later, but every string, datatype, node, and signal must be declared
before the section that uses it. Later declarations cannot repair a forward
reference in an earlier record block. Runtime record references may point to
records or blocks written later.

Without a valid final directory/trailer, recovery scans from byte 32 and stops
at the first incomplete, invalid, or checksum-failing section, or at DIRECTORY.
Only the verified contiguous section prefix is retained. It does not salvage
individual pages from a partial section. Recovery reconstructs declaration
ranges, time bounds, and block numbers from surviving payloads and preserves
the recording identity. It writes a new incomplete directory and fingerprint.
The source and destination must be different files, including hard links.

Complete files must resolve every nonzero runtime reference. Incomplete and
unfinished files may contain missing parents, relation endpoints, or child
blocks. Readers expose that missing information rather than inventing records.
`Unfinished` means no accepted final directory; `Incomplete` means an explicitly
sealed or recovered directory. Neither proves the recording contains every
simulation operation.

## 3. Small sections

### 3.1 Compressed blob

```text
u8 codec                  0 stored, 1 raw LZ4 block, 2 Zstandard frame
varint decompressed_bytes
bytes encoded_payload
```

Stored payload length equals the declared decompressed length. Other codecs
must produce exactly that length and consume their entire bounded blob. Blobs
are delimited by their enclosing length or container. The Zstandard frame may
omit content size and checksum. Writers store plain bytes when compression
would not reduce size. Compressed raw blocks are smaller than `2^30` bytes.

### 3.2 META (kind 1)

```text
varint layout = 1
svarint timescale          signed i8 exponent; one tick is 10^timescale seconds
svarint time_zero          i64 display offset
u8 file_type              0 Verilog, 1 VHDL, 2 mixed, 3 SystemC,
                          4 architectural, 5 software, 255 other
blob writer, date, comment UTF-8
varint group_size         power of two, 1 through 2^20
attributes                section 5.3
```

The default unit is picoseconds (`-12`). Group size is constant throughout one
recording. It partitions signal IDs for waveform storage, not hierarchy nodes.

### 3.3 BLACKOUT (kind 6)

`varint count`, then `count` pairs of `u8 active` and `varint time_delta`.
Active is 0 for recording stopped, 1 for resumed. Cumulative times are
nondecreasing and must not overflow. Missing samples during a blackout do not
change waveform frame/default rules.

## 4. STRINGS (kind 2)

A compressed blob containing `varint first_id`, `varint count`, then `count`
UTF-8 blobs. Chunks extend the dense string table in order. ID zero is the empty
string. Producers intern strings; readers must still accept duplicate text at
different IDs. String equality is bytewise. IDs and counts must fit `u32`.

## 5. HIERARCHY (kind 3)

A compressed blob containing `varint first_node`, `varint count`, then nodes.
Chunks extend the dense node table in order. Each node begins with `u8 kind`,
`varint parent_distance`, and `varint name_string_id`, followed by the
kind-specific fields below and an attribute list (5.3).

A zero parent distance means root; otherwise parent ID is `node_id - distance`.
Every parent is an earlier scope. Later chunks may add children anywhere in the
existing forest. Parentage is immutable.

| Kind | Fields |
| ---: | --- |
| 1 scope | `varint scope_type`, `varint component_string_id` |
| 2 variable | `varint var_type`, `u8 direction`, `varint signal_back`, `u8 declares`, then a signal kind when declaring |
| 3 generator | `varint kind_string_id`, `varint payload_datatype_id` |
| 4 enum table | `varint count`, pairs of `varint literal_string_id`, `varint value_string_id` |

Direction codes are 0 implicit, 1 input, 2 output, 3 inout, 4 buffer, 5 linkage.
For a variable, `signal_id = next_signal_id - signal_back`. A declaring variable
has `declares=1`, back distance zero, and allocates the next signal ID. An alias
has `declares=0`, a positive valid back distance, and no signal-kind payload.
Aliases share signal identity; they do not allocate another history.

Generator kinds `CLOCK`, `EVENT`, `STAGE`, and `LOG` are reserved; other strings
identify ordinary transaction sources. Each source has one immutable payload
datatype, detailed in [TRANSACTIONS.md](TRANSACTIONS.md). Scope grouping does not
inherit record kind or metadata.

A declaring signal stores `u8 encoding`: 0 bits/2-state, 1 bits/4-state,
2 bits/9-state, 3 binary64 real, 4 opaque variable-length bytes. Codes 0–2 append
`varint width`, with `0 < width < 2^31`. Packed signal bit i occupies the low-to-high
positions in byte `i/8`, `i/4`, or `i/2`, respectively. State codes in order are
`0, 1, X, Z, U, W, L, H, -`; two-state uses codes 0–1, four-state 0–3.
Unused high padding bits are zero. A compact waveform value uses two-state
packing only when its column header says so.

### 5.1 Scope types

Codes 0..22 are the FST/VCD scope types with identical numbering:
0 module, 1 task, 2 function, 3 begin, 4 fork, 5 generate, 6 struct,
7 union, 8 class, 9 interface, 10 package, 11 program, 12 vhdl_architecture,
13 vhdl_procedure, 14 vhdl_function, 15 vhdl_record, 16 vhdl_process,
17 vhdl_block, 18 vhdl_for_generate, 19 vhdl_if_generate, 20 vhdl_generate,
21 vhdl_package, 22 sv_array. VTR adds 64 generic, 65 sc_module (SystemC),
66 resource (OpenTelemetry), 67 instrumentation_scope (OpenTelemetry),
68 core (a CPU core / hardware thread). Other codes are preserved verbatim.

### 5.2 Var types

For event variables (code 0), each signal record denotes an occurrence, not a
held level. Identical payloads and repeated timestamps remain distinct records
in emission order. Writers must preserve every emitted occurrence; value
deduplication does not apply to event signals. The signal's original variable
declaration determines whether it is an event. Every alias must agree on event
versus non-event status; readers reject conflicting declarations. Other variable
types may differ between aliases. These occurrences use the existing
signal column encodings, including zero time deltas; no separate event payload
encoding is required.

Codes 0..29 are the FST var types with identical numbering:
0 event, 1 integer, 2 parameter, 3 real, 4 real_parameter, 5 reg, 6 supply0,
7 supply1, 8 time, 9 tri, 10 triand, 11 trior, 12 trireg, 13 tri0, 14 tri1,
15 wand, 16 wire, 17 wor, 18 port, 19 sparray, 20 realtime, 21 string,
22 bit, 23 logic, 24 int, 25 shortint, 26 longint, 27 byte, 28 enum,
29 shortreal. VTR adds 64 bits (generic vector) and 65 bytes (generic
blob). The var type is informational; the signal kind decides the encoding.

### 5.3 Attributes and values

An attribute list is `varint count`, then pairs of `varint key_string_id` and a
tagged value. Keys are unique within each list, including nested maps. Each
value starts with one tag byte and the payload below. Unlike canonical typed
payloads, tagged integers use varints. All fixed-width integer ranges are
validated without narrowing or wrapping.

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


A length-prefixed byte sequence in this table is a `blob`. Packed widths are
`u64`, with the same state codes and zero-padding rules as signals. A map uses
attribute-list framing; a list stores tagged values. Nesting is limited to 64.
Floating values preserve their exact IEEE bits, including signed zero and NaNs.
Every referenced string must already exist.

Keys starting with `vtr.` or `log.` are reserved. `vtr.label`, when present, is
`str` or `text`. `vtr.clock` is a `u32` generator ID referring to a CLOCK source.
Log source metadata is specified in [LOGGING.md](LOGGING.md).

## 6. SIGNAL_BLOCK (kind 4)

A signal block holds all value changes of a contiguous time range. Blocks
partition the run in time. Within a block, signals are partitioned into
*groups* of `group_size` consecutive ids (group *g* holds ids
`g*group_size .. (g+1)*group_size-1`). A group is *dirty* in a block when
at least one of its signals changed there; only dirty groups are stored.

### 6.1 Layout

```
u64  start_time      first time of the block's time table
u64  end_time        last time of the block's time table
u32  n_times         entries in the time table
u32  n_signals       number of signals known when the block was written
u32  group_size
u32  n_dirty         number of dirty groups
u32  n_groups        ceil(n_signals / group_size)
u32  tt_len          byte length of the time table blob
bytes[tt_len] time_table   compressed blob (3.1) of n_times varints: first absolute time, then deltas
n_dirty x { u32 group, u32 clen, u64 offset }   dirty index, ascending by group;
                                                offset is relative to the start of the group area
n_groups x u32       prev-dirty table: for every group, the index of the previous
                     block in which the group was dirty, or 0xFFFFFFFF
group containers     concatenated; `clen` bytes each
```

Block indices are 0-based positions among the SIGNAL_BLOCK sections of the
file. The prev-dirty table lets a reader locate the most recent block that
holds a signal's data without scanning.

### 6.2 Group container

```
varint n_sigs                     signals in this group: min(group_size, n_signals - first id of the group)
varint n_alias                    dynamic aliases (6.6)
n_alias x { varint local_sig, varint target_sig }
blob   frame                      compressed blob: the *frame* (6.3)
varint n_runs
n_runs x { varint n_in_run, u8 xform, varint clen }   runs cover the n_sigs signals in order
run blobs                         n_runs compressed blobs of `clen` bytes each
```

Decompressed run: `n_in_run x varint column_length` followed by the
columns in the same order. A column of length 0 means the signal did not
change in this block. `xform` names the value transform (6.5) applied to
every eligible column of the run before compression: 0 none, 1 shuffle,
2 delta, 3 delta then shuffle, 4 dictionary; other values are reserved. Writers split a
group's columns into runs whose raw size does not exceed a budget (64 KiB
and at most 64 signals in the reference implementation; a single larger
column forms a run of its own) so that reading one signal decompresses a
bounded amount of data. The raw size of a run must be below 2^30 bytes.

A block holds only the signals known when it was written. A signal *s*
with `s >= n_signals` of the block, which is also outside every group
container's `n_sigs`, was declared later: it has no changes in the block
and no frame entry, and its value throughout the block is the default of
6.3. This is the normal case for a signal that joins a group after
earlier blocks stored that group.

### 6.3 Frame

The decompressed frame holds, for every signal of the group in id order,
its value at `start_time` *before* any change of this block, in the
signal's declared packing: `ceil(width/8|4|2)` bytes for bits, 8 bytes
for reals, `blob` for variable-length signals. A signal's value before its
first change in the file is: all bits 0 for 2-state signals, all bits X for
4- and 9-state signals, 0.0 for reals, the empty string for variable-length
signals. (The first dirty block's frame therefore holds these defaults for
signals that had not changed yet.)

### 6.4 Columns

A column holds the changes of one signal in time order. `dt` is the
difference between an entry's time-table index and the previous entry's
index in the same column (the first entry is relative to index 0).
Several entries may share one index (same-time updates); they are kept in
emission order.

*1-bit signals*: the column is a sequence of varint entries. Bit 0 is an
escape flag. `dt << 1` (flag 0) means the value *toggled*: the new logic
code is the previous entry's code XOR 1, which is only valid when the
previous code was 0 or 1. `(dt << 5) | (code << 1) | 1` (flag 1) carries
the logic code (0 ... 8) explicitly; it is used for the first entry of a
column and whenever the value did not simply toggle (repeated value with
duplicate suppression off, X/Z/U/W/L/H/- codes, or a previous code above
1). A two-state signal in a running simulation therefore costs one byte
per change for deltas up to 63.

*All other kinds*: the column is `varint header_word`, then `header_len`
bytes of entry headers, then the values concatenated in the same order,
where `header_word = (header_len << 2) | (eligible << 1) | full`.
Keeping headers and values in separate streams lets the compressor see
homogeneous data. `eligible = 1` marks a column whose entries all have the
same value length (a *fixed-width* column: every entry compact, or every
entry in the declared packing, `full` telling which; reals always) and
whose value stream therefore carries the run's transform (6.5); a
variable-length column is never eligible.

| signal kind | header | value |
|---|---|---|
| bits, width > 1 | `varint (dt << 1) \| compact` | if `compact = 1`, `ceil(width/8)` bytes in 2-state packing (every bit is 0 or 1); else `ceil(width/4)` or `ceil(width/2)` bytes in the declared 4- or 9-state packing. For 2-state signals `compact` is always 1. |
| real | `varint dt` | 8 bytes binary64 LE |
| variable length | `varint dt` | `blob` |

The number of entries is implied by the header stream; a reader decodes
headers and values with two cursors that advance together.

### 6.5 Value transforms

Before compression a writer may apply one transform to the value streams
of all eligible columns of a run (the run's `xform`, 6.2). With `n`
entries of `w` bytes each (`w` = `ceil(width/8)` for compact entries,
`packed_len(width, states)` for full ones, 8 for reals), viewed as
little-endian integers:

| xform | encoding |
|---:|---|
| 1 shuffle | byte transposition: output byte `b * n + i` is input byte `i * w + b` (all first bytes, then all second bytes, ...) |
| 2 delta | entry `i` (i >= 1) replaced by `entry[i] - entry[i-1]` modulo `2^(8w)`; entry 0 unchanged |
| 3 delta + shuffle | delta, then shuffle |
| 4 dictionary | per column: `varint n_dict`, then `n_dict * w` bytes of distinct entries in order of first occurrence (`1 <= n_dict <= 256`), then one code byte per entry (index into the dictionary). `n_dict = 0` means the column's `n * w` plain value bytes follow instead (the column had more than 256 distinct entries). |

Transforms 1-3 keep the value stream's length; transform 4 changes it, so
the column lengths of the run (6.2) describe the transformed columns.
Transforms are inverted (unshuffle, then prefix sum; dictionary lookup)
after decompression and before the column is decoded. They exist because
general-purpose compressors see counters, addresses, slowly changing
high-order bytes and small value sets far better after such a pass: the
reference writer picks the transform per run by trial-compressing a small
sample of the run's value streams with each candidate and keeps plain
values unless a candidate is at least 4% smaller (20% for the dictionary,
whose sample estimate is optimistic). Header streams are never
transformed.

### 6.6 Dynamic aliases

When two signals of the same kind have byte-identical columns in a block
(typical for clock trees and fan-out nets that the simulator did not
declare as aliases), the producer may store the column once: the later
signal is listed in its group's alias table with `local_sig` (its index
within the group) and `target_sig` (the absolute id of the signal whose
column it shares, which is never itself an alias) and its own column has
length 0. Aliases are per block; frames are never aliased. A reader must
resolve an alias before decoding the column (the target may live in
another dirty group of the same block).

### 6.7 Reading

*Value of signal s at time t*: find the last block whose `start_time <= t`
(binary search over directory `aux0`); let `g = s / group_size`; if `g` is
in the block's dirty index decode its run for `s` (following an alias,
6.6, and undoing the run's transform, 6.5) and take the last entry
whose time-table index maps to a time `<= t`, else take the frame value;
if `g` is not dirty there, follow the prev-dirty table to the previous
block in which it is dirty and take the last entry of its column (or the
frame value when the column is empty); if there is none, the value is the
default of 6.3. When the block found this way does not hold `s`
(`s >= n_signals`, 6.2), the value is also the default of 6.3: the
signal did not exist yet and had no changes up to that block. All changes
of a signal are the concatenation of its columns over the blocks that
hold it; its initial value is the frame of the first dirty block that
holds it.

The global list of time steps is the concatenation of all block time
tables with the duplicated boundary time removed.

## 7. Typed records

[TRANSACTIONS.md](TRANSACTIONS.md) defines DATATYPES, canonical payloads,
TX_BLOCK, RECORD_BLOCK, ownership, embedded children, relations, clocks, and
query semantics. Logs use ordinary typed LOG generators and RECORD_BLOCK pages;
there is no separate log-block encoding.

## 8. Conformance and limits

All decoders validate discriminators, flags, ranges, UTF-8, declared counts,
column consumption, and type availability. Unknown attribute tags, codec IDs,
node kinds, or datatype kinds are errors. A reader may enforce tighter resource
budgets than the wire permits, but must report rejection rather than truncate
values or silently drop records. Public payloads and encoded page fields that
use `u32` lengths must fit that domain.

Lazy queries check the pages they load. Full validation additionally checks
all waveform samples and records, exact block/page bounds, global record and
block identity, ownership, references, locators, and clock associations.
Checksums cover bytes; semantic validation checks their meaning. Disabling
checksums does not disable structural validation.

The independent Rust/C++ tests cover both writer/reader directions, codecs,
producer and encoding modes, retained schemas, waveform aliases and states,
recovery, and VTI identity. Exact-byte fixtures cover low-level schema and page
encodings. These checks complement the full application and benchmark suites.

## 9. Activity index sidecar (VTI 3.0)

An activity index records exact signal-change times after the first waveform
time `t_min`. Multiple values at the same timestamp count once for activity;
event occurrences remain distinct in the trace itself. Consecutive integer
timestamps coalesce into a stretch `[first,last]`. Its gap is 0 for one timestamp,
1 otherwise. A signal is active in a closed query window exactly when one of
its stretches intersects it; there is no approximate or undecided result.

A source must have a finalized fingerprint. Sidecar identity includes its format,
recording ID, total trace length, and trace directory CRC. Matching source
metadata additionally checks signal count, t_min, every waveform block's bounds,
and its section-header-plus-payload size. A sidecar for another recording is
rejected, even when its length and CRC match. Cached sidecars are derived data
and can be rebuilt.

### 9.1 Container and header

VTI has magic `89 56 54 49 0d 0a 1a 0a`, major 3, minor 0, and twenty zero bytes
after the version fields in its 32-byte file header. Its 24-byte section and
trailer framing follow section 2, but the DIRECTORY payload is `u64 entry_count`
followed by **40-byte** entries: kind/flags, offset/length, aux0/aux1. It has no
dropped-byte field or block-number column. `VTR_END\0` remains the trailer magic.
Every VTI payload and the final directory have a CRC. A sidecar requires its
complete directory and trailer; prefix recovery is not supported.

Sections are one HEADER (kind 1), one BLOCK (kind 2) per source waveform block
in order, one TAIL (kind 3), DIRECTORY (kind 7), and trailer. Only BLOCK has
nonzero aux fields, equal to start/end. Header payload is exactly 56 bytes:

| Offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 8 | Source trace length |
| 8 | 4 | Source directory payload CRC |
| 12 | 1 | Source format = 1 (VTR) |
| 13 | 3 | Reserved zero |
| 16 | 4 | Signal count |
| 20 | 12 | Reserved zero |
| 32 | 8 | t_min |
| 40 | 16 | Source recording identity |

### 9.2 Block and tail payloads

BLOCK's 40-byte prefix is `u64 start, end, source_section_bytes`,
`u32 row_count, closed_stretch_count`, `u8 exponent=0`, and seven reserved zero
bytes. It is followed by a compressed blob containing six varint byte lengths
and the six streams below. Unlike transaction column sets, VTI omits the column
count. All stream values are canonical unsigned varints.

1. Signal-ID delta per row, first relative to zero; rows strictly increase.
2. Head per row: `(closed_count << 1) | first_change_flag`; not zero.
3. First-change offset from source block start, only for a row with its flag set.
4. Length `last-first` per closed stretch.
5. Silence `next_first-last` following each closed stretch; greater than 1.
6. Gap per closed stretch: 0 when length is zero, otherwise 1.

A signal's initial stretch opens at its flagged first change, strictly after
t_min. Later stretches open after the preceding kept silence. A stretch can
cross source block boundaries; its start is retained by the decoder until it
closes. Declared closed counts and all column contents must be consumed exactly.

TAIL is `u32 row_count`, four reserved zero bytes, then a compressed blob of
three varint-length streams: signal deltas, final stretch lengths, final gaps.
It closes the remaining stretch for every signal that changed. The tail leaves
no open stretch. All decoded times must fit the source waveform bounds.

### 9.3 Building, loading, and caching

Builders write a temporary file and publish it by rename after finishing the
container. Cancellation or failure does not publish a partial sidecar. Parallel
scans are stitched in source order. Scanner admission uses conservative workspace
estimates; configured memory is not a process-wide allocator cap. Decoders admit
input, decompression workspace, and retained range storage before allocating,
and support cooperative yielding/cancellation.

The adjacent name is `<trace>.index`. Cache fallback names contain the source
format, recording ID, length, and directory CRC. The default cache directory is
`$XDG_CACHE_HOME/vtr/index`, or `~/.cache/vtr/index`. An identity mismatch causes
rebuilding rather than reuse. See `vtr::activity` for build and cache APIs.
