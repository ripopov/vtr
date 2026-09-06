# VTR file format specification

Version 1.1 of the VTR (Vibe Trace Record) container and encodings.
This document is normative: an independent implementation written from it
must read every file produced by the reference implementation and produce
files the reference reader accepts.

Notation: `u8/u16/u32/u64` are unsigned little-endian fixed-width integers;
`i8`/`i64` signed likewise. `varint` is an unsigned LEB128 integer
(7 payload bits per byte, least-significant group first, high bit set on
all but the last byte, at most 10 bytes). `svarint` is a `varint` holding a
zig-zag mapped signed integer (`(v << 1) ^ (v >> 63)`). `blob` is
`varint length` followed by that many bytes. Bit `i` of a byte is
`(byte >> i) & 1`.

## 1. Data model

A VTR file holds four things:

1. **Metadata**: time unit, time-zero offset, producer, date, comment, file
   type and free-form typed attributes.
2. **Hierarchy**: an ordered forest of nodes. A node is a *scope*, a *var*
   (variable), a *stream*, a *generator* or an *enum table*, has a name,
   an optional parent and an ordered list of typed attributes. Vars refer to
   *signals*; several vars may alias one signal.
3. **Signal value changes**: for every signal, its value at every time step
   it changed. Time is a non-decreasing `u64` in the file's time unit.
4. **Transactions**: timed intervals belonging to a generator, with typed
   attributes (tagged begin/record/end), point *events*, sub-interval
   *stages* on lanes, an optional parent, a status and a kind; and
   *relations* — typed, attributed, directed edges between transactions.
5. **Log records**: timestamped messages of a *log site* (a generator that
   declares a format string and argument types), stored as the argument
   values only (section 8). A log record is a zero-duration transaction.

Nothing in the file describes presentation. Stable identity for external
databases (VDB) comes from names: full hierarchical paths of nodes, stream
and generator names, attribute keys, event names, stage names, lane names
and relation kinds are all strings chosen by the producer.

Reader result ownership is an API concern, not an on-disk encoding. The
reference reader exposes immutable loaded histories and shares their storage
for repeated requests for one signal ID. This does not change declared alias
identity or the per-block dynamic alias rules in section 6.6.

## 2. Container

```
+----------------------------+  offset 0
| file header (32 bytes)     |
+----------------------------+
| section                    |  24-byte header + payload, repeated
| ...                        |
+----------------------------+
| DIRECTORY section          |  written last by the writer
+----------------------------+
| trailer (24 bytes)         |
+----------------------------+  end of file
```

### 2.1 File header (32 bytes)

| offset | size | field |
|---:|---:|---|
| 0 | 8 | magic `89 56 54 52 0D 0A 1A 0A` (`\x89VTR\r\n\x1a\n`) |
| 8 | 2 | `u16` major version = 1 |
| 10 | 2 | `u16` minor version = 1 |
| 12 | 4 | `u32` flags, must be 0 |
| 16 | 16 | reserved, must be 0 |

A reader must reject a file whose major version is greater than the one it
implements and must accept any minor version: minor versions only add
optional sections, attribute keys and value tags (section 9). A reader that
meets a value tag it does not know must report the file as unreadable
rather than guess. Version 1.1 added value tag 17 (text) and the optional
LOG_BLOCK section; a 1.0 reader skips log blocks and rejects files that
use tag 17.

### 2.2 Section header (24 bytes)

| offset | size | field |
|---:|---:|---|
| 0 | 4 | `u32` kind (table below) |
| 4 | 4 | `u32` flags: bit 0 = *optional* (a reader that does not know `kind` may skip the section) |
| 8 | 8 | `u64` payload length |
| 16 | 4 | `u32` CRC-32 (IEEE 802.3 polynomial, as produced by zlib's `crc32`) of the payload, or 0 if not computed |
| 20 | 4 | reserved, 0 |

The payload follows immediately. Kinds:

| kind | name | payload |
|---:|---|---|
| 1 | META | section 3 |
| 2 | STRINGS | section 4 (compressed) |
| 3 | HIERARCHY | section 5 (compressed) |
| 4 | SIGNAL_BLOCK | section 6 |
| 5 | TX_BLOCK | section 7 |
| 6 | BLACKOUT | section 3.3 |
| 7 | DIRECTORY | section 2.3 |
| 8 | LOG_BLOCK | section 8 (written with the optional flag) |

Kinds below 0x1000 are reserved for this specification; a producer may
write private sections with kind >= 0x1000 and the optional flag set.
A reader must fail on an unknown kind without the optional flag.

### 2.3 Directory and trailer

DIRECTORY payload: `u64 count` then `count` entries of 40 bytes:

| offset | size | field |
|---:|---:|---|
| 0 | 4 | `u32` kind |
| 4 | 4 | `u32` flags |
| 8 | 8 | `u64` offset of the section header from the start of the file |
| 16 | 8 | `u64` payload length |
| 24 | 8 | `u64` aux0 |
| 32 | 8 | `u64` aux1 |

`aux0/aux1` are kind specific: STRINGS and HIERARCHY store the first id and
the count of the chunk; SIGNAL_BLOCK stores the block's start and end time;
TX_BLOCK stores the minimum begin time and maximum end time of its
transactions; LOG_BLOCK the minimum and maximum time of its records; other
kinds store 0. Entries appear in file order.

Trailer (last 24 bytes of the file):

| offset | size | field |
|---:|---:|---|
| 0 | 8 | `u64` offset of the DIRECTORY section header |
| 8 | 8 | `u64` total file length |
| 16 | 8 | magic `VTR_END\0` |

**Recovery.** A file without a valid trailer (the writer crashed) is still
readable: the reader walks section headers from offset 32, stopping at the
first header that does not fit in the file, and recomputes `aux0/aux1`
from the payloads. Everything up to the last complete section is
recovered. Readers must expose whether a file was recovered.

### 2.4 Section ordering rules

Sections are append-only. Producers must obey these ordering constraints so
that recovery and streaming readers work:

* Every string id used by a section is defined by a STRINGS section that
  precedes it in the file.
* Every node id / signal id used by a section is defined by a HIERARCHY
  section that precedes it.
* META precedes the first HIERARCHY, SIGNAL_BLOCK and TX_BLOCK section.
* SIGNAL_BLOCK sections appear in non-decreasing time order and their time
  ranges do not overlap except that the last time of block *k* may equal the
  first time of block *k+1*.
* Group sizes and the meaning of signal ids never change within a file.
* TX_BLOCK and LOG_BLOCK sections carry their own id and time ranges and
  may appear in any order relative to each other and to SIGNAL_BLOCKs.

## 3. Small sections

### 3.1 Compressed blob

Several payloads contain a *compressed blob*:

```
u8      codec      0 = stored, 1 = LZ4 block format, 2 = Zstandard frame
varint  raw_len    length of the decompressed data
bytes   payload    the remaining bytes of the blob
```

For codec 0 the payload is `raw_len` bytes verbatim. Codec 1 is a single
raw LZ4 block (no frame, no length prefix). Codec 2 is a Zstandard frame
(RFC 8878); the frame may omit the content size and checksum. Writers must
fall back to codec 0 when compression does not reduce the size. A blob is
always the last thing in its enclosing byte string, or is preceded by its
total length, so a reader can delimit it.

### 3.2 META (kind 1)

```
varint  layout   = 1
svarint timescale   exponent e: one time unit is 10^e seconds (-9 = ns, -12 = ps)
svarint time_zero   offset to add when displaying absolute time (FST "timezero")
u8      file_type   0 Verilog, 1 VHDL, 2 Verilog+VHDL, 3 SystemC, 4 architectural simulator,
                    5 software tracing, 255 other
blob    writer      producing tool and version (UTF-8)
blob    date        free form (UTF-8)
blob    comment     free form (UTF-8)
varint  group_size  signals per value-change group; a power of two, constant for the file
attrs               file-level attributes (section 5.3)
```

Exactly one META section is present.

### 3.3 BLACKOUT (kind 6)

Dump-off/dump-on intervals (VCD `$dumpoff`/`$dumpon`):

```
varint n
n x { u8 active (1 = recording resumed, 0 = recording stopped), varint time_delta }
```

Times are absolute after cumulative addition of the deltas. Value changes
inside a blackout region are simply absent; the frame mechanism of
section 6 still gives every signal a value at every time.

## 4. STRINGS (kind 2)

The payload is a compressed blob (3.1) containing:

```
varint first_id
varint count
count x blob   UTF-8 string
```

Chunks are appended in id order: the first chunk has `first_id = 0` and its
first string must be the empty string (id 0 is always ""). String ids are
`u32`. Strings are compared byte-wise; producers should intern each
distinct string once but readers must not assume uniqueness.

## 5. HIERARCHY (kind 3)

The payload is a compressed blob containing:

```
varint first_node
varint count
count x node
```

Node ids are dense `u32` in file order (`first_node` of a chunk equals the
number of nodes in all earlier chunks). A node:

```
u8      kind        1 scope, 2 var, 3 stream, 4 generator, 5 enum table
varint  parent      0 = top level; otherwise `this id - parent id` (the parent precedes the node)
varint  name        string id
...kind specific fields...
attrs               section 5.3
```

Ids inside a node are stored relative to the node's own position so that
they stay small: the parent as a backward distance, the signal of a var
(below) as a backward distance from the next unassigned signal id.

Kind specific fields:

| kind | fields |
|---|---|
| scope | `varint scope_type` (5.1), `varint component` string id (module/entity type; 0 = unknown) |
| var | `varint var_type` (5.2), `u8 direction` (0 implicit, 1 input, 2 output, 3 inout, 4 buffer, 5 linkage), `varint signal_back` = `next_signal - signal` where `next_signal` is the number of signals declared before this node (0 for a var that declares the next signal, `>= 1` for an alias of an earlier one), `u8 declares` then, if `declares = 1`, a signal kind: `u8 code` (0 bits/2-state, 1 bits/4-state, 2 bits/9-state, 3 real, 4 variable-length) followed by `varint width` for codes 0..2 |
| stream | `varint kind` string id (free form, e.g. FTR's "TRANSACTOR") |
| generator | none; the parent must be a stream |
| enum table | `varint n`, `n x { varint literal string id, varint value string id }` |

**Signals.** Signal ids are dense `u32`. The var with `declares = 1` for
signal *s* is the *s*-th declaring var in file order (so `signal_back`
must be 0 for it); a var with `declares = 0` is an *alias* of an earlier
signal and `signal_back` must be at least 1 and at most `next_signal`. The
signal kind gives the value encoding used everywhere else:

| code | kind | value representation |
|---:|---|---|
| 0 | bits, 2 states | `width` bits, 1 bit per bit |
| 1 | bits, 4 states | `width` bits, 2 bits per bit, codes 0,1,X,Z |
| 2 | bits, 9 states | `width` bits, 4 bits per bit, codes 0,1,X,Z,U,W,L,H,- |
| 3 | real | IEEE 754 binary64, little-endian |
| 4 | variable length | byte string |

Bit packing (all states): bit *i* of the vector (bit 0 is the least
significant bit, i.e. the rightmost character of the VCD spelling) is
stored in byte `i / 8` (2-state), `i / 4` (4-state) or `i / 2` (9-state),
at bit position `(i % 8)`, `2 * (i % 4)` or `4 * (i % 2)`. Unused high bits
of the last byte are zero. Logic codes:

| code | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|---|
| meaning | 0 | 1 | X | Z | U | W | L | H | - |

A value packed with fewer states than the signal declares (a *compact*
value) is only used where the enclosing encoding says so (section 6.4).

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

Codes 0..29 are the FST var types with identical numbering:
0 event, 1 integer, 2 parameter, 3 real, 4 real_parameter, 5 reg, 6 supply0,
7 supply1, 8 time, 9 tri, 10 triand, 11 trior, 12 trireg, 13 tri0, 14 tri1,
15 wand, 16 wire, 17 wor, 18 port, 19 sparray, 20 realtime, 21 string,
22 bit, 23 logic, 24 int, 25 shortint, 26 longint, 27 byte, 28 enum,
29 shortreal. VTR adds 64 bits (generic vector) and 65 bytes (generic
blob). The var type is informational; the signal kind decides the encoding.

### 5.3 Attributes and values

An attribute list is `varint n` followed by `n x { varint key string id,
value }`. A value is a tag byte followed by a tag-specific payload:

| tag | name | payload |
|---:|---|---|
| 0 | null | none |
| 1 | bool | `u8` 0/1 |
| 2 | i64 | `svarint` |
| 3 | u64 | `varint` |
| 4 | f64 | 8 bytes IEEE binary64 LE |
| 5 | str | `varint` string id |
| 6 | bytes | `blob` |
| 7 | bits | `varint width`, then `ceil(width/8)` bytes 2-state packed |
| 8 | logic | `varint width`, then `ceil(width/4)` bytes 4-state packed |
| 9 | logic9 | `varint width`, then `ceil(width/2)` bytes 9-state packed |
| 10 | time | `varint` in file time units |
| 11 | enum | `svarint value`, `varint` literal string id |
| 12 | pointer | `varint` |
| 13 | fixed | `svarint raw`, `svarint scale` (value = raw * 2^-scale) |
| 14 | ufixed | `varint raw`, `svarint scale` |
| 15 | list | `varint n`, `n x value` |
| 16 | map | `varint n`, `n x { varint key string id, value }` |
| 17 | text | `blob`, UTF-8; inline text that is not interned (one-off strings such as log arguments; added in 1.1) |

Attribute keys are free form. Keys beginning with `vtr.` and `log.` are
reserved for this specification (`log.*` is defined in section 8.1);
converters use tool prefixes (`fst.`, `otel.`, `kanata.`, `ftr.`) for
source-specific data. Readers must preserve unknown attributes.

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
blob time_table      compressed blob (3.1) of n_times varints: first absolute time, then deltas
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
varint n_sigs                     signals in this group (group_size, or fewer for the last group)
varint n_alias                    dynamic aliases (6.6)
n_alias x { varint local_sig, varint target_sig }
blob   frame                      compressed blob: the *frame* (6.3)
varint n_runs
n_runs x { varint n_in_run, varint xform, varint clen }   runs cover the n_sigs signals in order
run blobs                         n_runs compressed blobs of `clen` bytes each
```

Decompressed run: `n_in_run x varint column_length` followed by the
columns in the same order. A column of length 0 means the signal did not
change in this block. `xform` names the value transform (6.5) applied to
every eligible column of the run before compression: 0 none, 1 shuffle,
2 delta, 3 delta then shuffle; other values are reserved. Writers split a
group's columns into runs whose raw size does not exceed a budget (64 KiB
and at most 64 signals in the reference implementation; a single larger
column forms a run of its own) so that reading one signal decompresses a
bounded amount of data. The raw size of a run must be below 2^30 bytes.

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
default of 6.3. All changes of a signal are the concatenation of its
columns over all blocks.

The global list of time steps is the concatenation of all block time
tables with the duplicated boundary time removed.

## 7. TX_BLOCK (kind 5)

Transactions are appended to a block when they end (or when the file is
closed, with status *open*), so blocks are ordered by end time and their
begin-time ranges may overlap. Relations are appended when they are
recorded.

### 7.1 Layout

```
u64 n_tx, u64 n_rel
u64 min_id, u64 max_id            transaction ids present (0 if n_tx = 0)
u64 t_min, u64 t_max              smallest begin / largest end time
u64 rel_min_from, u64 rel_max_from, u64 rel_min_to, u64 rel_max_to
u32 n_gens                        number of distinct generators present
u32 blob_len
n_gens x u32                      sorted generator node ids present
blob                              compressed blob (3.1) of the column set, blob_len bytes
```

Decompressed column set: `varint n_cols` (= 24), `n_cols x varint len`,
then the columns concatenated. Every column is a byte stream read
sequentially; a reader decodes transaction by transaction pulling one item
from each relevant column. Columns, in order:

| # | name | item per | encoding |
|---:|---|---|---|
| 0 | id | transaction | `svarint` delta from the previous transaction's id in the block (first relative to 0) |
| 1 | generator | transaction | `varint` node id |
| 2 | begin | transaction | `svarint` delta from the previous transaction's begin time (first relative to 0) |
| 3 | duration | transaction | `varint end - begin` |
| 4 | status/kind | transaction | `u8`: `kind << 4 \| status` (7.2) |
| 5 | parent | transaction | `varint`: 0 = none, else `(zigzag(id - parent) << 1) \| 1` |
| 6 | attr count | attribute list | `varint n` — one item per attribute list, in this order: the transaction's attributes, then each event's, then each stage's; after all transactions, each relation's |
| 7 | attr key | attribute | `varint` string id |
| 8 | attr tag | attribute | `u8`: `phase << 5 \| value tag` (phase 0 begin, 1 record, 2 end) |
| 9 | attr numeric | attribute | `u8` for bool; `svarint` for i64 and enum value; `varint` for u64/time/pointer/ufixed raw; `svarint raw, svarint scale` for fixed; `varint raw, svarint scale` for ufixed |
| 10 | attr f64 | attribute | 8 bytes binary64 LE |
| 11 | attr string | attribute | `varint` string id for str and for the enum literal |
| 12 | attr misc | attribute | payload of tags 6..9 and 15..16 exactly as in 5.3 (without the tag byte) |
| 13 | event count | transaction | `varint` |
| 14 | event time | event | `svarint time - begin` |
| 15 | event name | event | `varint` string id |
| 16 | stage count | transaction | `varint` |
| 17 | stage name | stage | `varint` string id |
| 18 | stage lane | stage | `varint` string id |
| 19 | stage begin | stage | `svarint stage.begin - begin` |
| 20 | stage end | stage | `varint`: 0 = open, else `stage.end - stage.begin + 1` |
| 21 | relation kind | relation | `varint` string id |
| 22 | relation from | relation | `svarint` delta from the previous relation's `from` (first relative to 0) |
| 23 | relation to | relation | `svarint to - from` |

Attribute items are consumed from columns 7..12 in list order; for each
attribute, exactly the columns implied by its tag are read.

### 7.2 Semantics

* Transaction ids are `u64` >= 1, unique within the file; the reference
  writer assigns them in begin order. Id 0 as a relation endpoint means
  "outside this file".
* `status`: 0 unset, 1 ok, 2 error, 3 aborted (squashed / flushed),
  4 open (never ended before close; `end` is then the last time known to
  the writer). `kind`: 0 unspecified, 1 internal, 2 server, 3 client,
  4 producer, 5 consumer (OpenTelemetry span kinds).
* `end >= begin`; stage `end >= stage.begin`; an open stage is closed at the
  transaction's end when read.
* Events and stages belong to one transaction and may lie outside its
  `[begin, end]` interval only if the producer chose so.
* Relations are directed `from -> to` with a free-form kind string and
  attributes. Structural parent/child nesting uses the `parent` field;
  producers may additionally record a `parent_of` relation.
* Text logs are streams of kind `LOG`; their records are zero-duration
  transactions stored in LOG_BLOCKs (section 8), not in TX_BLOCKs.

### 7.3 Reading

*Transaction by id*: scan the blocks whose `[min_id, max_id]` contains the
id. *Transactions in a time window*: blocks with `t_min <= window end` and
`t_max >= window start`. *Relations from/to an id*: blocks whose
`rel_min/max` range contains it. *Transactions of a generator*: blocks
whose generator list contains it.

## 8. LOG_BLOCK (kind 8)

A log record is a message of a *log site*: one call site of a logging
macro in the producer (`LOG_INFO("addr={:#x} len={}", a, n)`). The site's
static facts (format string, severity, source location, argument types)
are declared once as a generator of a `LOG` stream; a record stores the
time and the argument values only. This borrows the static/dynamic split of
NanoLog and binlog and the dictionary of repeated string variables of CLP
(see `docs/RATIONALE.md`).

### 8.1 Log sites

A log site is a generator node whose parent stream has kind `LOG` and
whose attributes include `log.args`. Its name is the format string. The
attributes are:

| key | value | meaning |
|---|---|---|
| `log.args` | list of u64 | argument types in placeholder order; each is a value tag from the set 1 bool, 2 i64, 3 u64, 4 f64, 5 str (interned id), 6 bytes, 10 time, 12 pointer, 17 text |
| `log.names` | list of str | one name per argument (attribute keys when a record is read as a transaction); producers that have no names write `"0"`, `"1"`, ... |
| `log.severity` | u64 | 0 trace, 1 debug, 2 info, 3 warn, 4 error, 5 fatal; other values rank by number (default 2) |
| `log.file`, `log.line`, `log.func` | str, u64, str | source location, optional |

Format strings use `{}` placeholders with an optional index and
specification, `{2}`, `{:#010x}`, `{:>8.3}`, and `{{`/`}}` for literal
braces (the common subset of Rust `std::fmt` and C++ `std::format`; the
reference reader's `logfmt` module documents the accepted specification).
The number of placeholders should equal the number of arguments; a reader
renders a placeholder without an argument as `{?}`. Other `log.*` keys are
reserved.

### 8.2 Layout

```
u64 n_rec
u64 min_id, u64 max_id            record (transaction) ids present (0 if n_rec = 0)
u64 t_min, u64 t_max              smallest / largest record time
u32 n_gens
u32 blob_len
n_gens x u32                      generator node ids present, in order of first occurrence
blob                              compressed blob (3.1) of the column set, blob_len bytes
```

Decompressed column set: `varint n_cols` (= 9), `n_cols x varint len`,
then the columns concatenated. A reader decodes record by record, pulling
one item from each relevant column; the argument columns are consumed in
the site's declared order.

| # | name | item per | encoding |
|---:|---|---|---|
| 0 | gen | record | `varint` index into the block's generator list |
| 1 | time | record | `svarint` delta from the previous record's time (first relative to 0) |
| 2 | id | record | `svarint` delta from the previous record's id (first relative to 0) |
| 3 | parent | record | `varint`: 0 = none, else `(zigzag(id - parent) << 1) \| 1` |
| 4 | num | argument | bool: `u8`; i64: `svarint`; u64, time, pointer: `varint` |
| 5 | f64 | argument | 8 bytes binary64 LE |
| 6 | text | argument | `varint` index into the block dictionary (column 7) |
| 7 | dict | block | `varint n`, then `n x blob`: the distinct text arguments of the block in order of first occurrence |
| 8 | misc | argument | str: `varint` string id; bytes: `blob` |

### 8.3 Semantics

* Record ids share the transaction id space (section 7.2): a log record is
  a zero-duration transaction (`begin = end = time`, status *unset*, kind
  *unspecified*) whose attributes are its arguments keyed by `log.names`.
  Relations may reference record ids.
* `parent`, when present, is the transaction during which the message was
  produced (for example the bus transfer a driver was handling).
* Records within a block are stored in production order; times need not
  be monotonic. Blocks may appear in any order (section 2.4); the reference
  writer keeps production order even when it encodes on several threads, and
  the reference reader visits blocks by their first time.
* Text arguments are deduplicated per block; identical strings share one
  dictionary entry. Producers should declare repetitive strings (names,
  states, responses) as text and use `str` only when they intern
  themselves; `bytes` is for payloads that must not be interpreted.

### 8.4 Reading

*Records of a time window*: blocks with `t_min <= window end` and
`t_max >= window start`. *Records by severity, stream or site*: the
generator list in the header names every site present, so a block whose
sites are all below the wanted severity or outside the wanted stream is
skipped without decompression. *Record by id*: blocks whose
`[min_id, max_id]` contains the id, after the TX_BLOCKs.

## 9. Extensibility and versioning

* New section kinds must be marked optional unless the major version is
  bumped.
* New attribute keys never require a version change.
* New value tags, node kinds, signal kinds or column layouts require a new
  major version. Readers must reject unknown value tags and node kinds.
* Unknown scope/var type codes, status/kind codes and file types are
  preserved and passed through.
* Producers should keep `group_size` between 64 and 1024; readers must
  accept any power of two up to 2^20.

## 10. Limits

Signal, node and string ids are `u32`; transaction ids `u64`; times `u64`;
vector widths up to 2^31 - 1 bits (packed values must fit a run); a
time table has at most 2^31 - 1 entries per block; column runs are below
2^30 raw bytes.

## 11. Conformance checklist for producers

1. Header, META, at least one STRINGS chunk with id 0 = "", HIERARCHY
   chunks before the blocks that use their ids, DIRECTORY, trailer.
2. Non-decreasing time in SIGNAL_BLOCKs; block time tables sorted and
   distinct except for the boundary duplicate.
3. Every dirty group has a frame for all its signals.
4. Column runs and blobs individually decodable; CRC-32 either correct
   or 0.
5. Transaction blocks: `n_tx`/`n_rel` match the columns; attribute
   lists consumed in the specified order.
6. Log blocks: written with the optional flag; every generator in the
   header is a log site (8.1) declared in an earlier HIERARCHY chunk; the
   argument columns hold exactly the declared values per record.

## Independent RTL VDB companion

The [RTL VDB schema](VDB_RTL.md) is a separate application JSON document.
It adds no VTR section, source semantics, or format code. An optional producer
string attribute `design.vdb_id` uses the existing generic metadata mechanism
to attest which companion design was simulated. VTR readers need no VDB support.

VDB v2 adds explicit module ownership, complete ports, and static process reads
for module-scoped netlist views. Its version is independent of VTR 1.0. SVG
layout, hierarchy navigation, and timestamp annotations remain application
artifacts derived from that companion and the existing waveform reader.

The native Verilator producer uses `design.vdb_id` for the same attachment
contract and writes a separate VDB v2 JSON companion with an optional explicit
`trace_binding`. This adds no VTR section, encoding code, or format version.
The binding and its identity requirements are specified in [VDB_RTL.md](VDB_RTL.md).

### Consumer companion discovery

The pinned Surfer consumer recognizes a same-stem `.vdb` companion, with
`.vdb.json` as a fallback filename. This is a consumer convention, not a VTR
container field or version change. Identity and structural attachment follow
[the RTL VDB rules](VDB_RTL.md#surfer-source-navigation); source paths and
presentation state remain outside VTR.
