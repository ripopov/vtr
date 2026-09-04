# Feature coverage: FST, FTR, Kanata and OpenTelemetry in VTR

This document enumerates every feature of the four reference formats and
shows how VTR represents it without loss. "VTR" columns name the
construct in `docs/SPEC.md`; converter columns name what `vtr convert`
does today (`crates/vtr-cli/src/{fst,ftr,kanata,otlp}.rs`).

## 1. FST (GTKWave)

### 1.1 Header and file-level

| FST feature | VTR |
|---|---|
| start/end time | derived from block time ranges (`Reader::time_range`), also `aux0/aux1` of every block in the directory |
| timescale exponent (int8, 10^n s) | `META.timescale` (svarint, same meaning) |
| timezero (int64) | `META.time_zero` |
| writer/version string (128 bytes) | `META.writer` (unbounded UTF-8) |
| date string (119 bytes) | `META.date` |
| file type Verilog / VHDL / Verilog+VHDL | `META.file_type` 0/1/2 (VTR adds SystemC, architectural, software, other) |
| scope count, var count, max handle, VC section count | derived from the hierarchy and directory; converter also stores `fst.var_count` |
| double endian test | not needed: VTR reals are always little-endian |
| writer memory hint | intentionally dropped (informational, tool specific) |

### 1.2 Hierarchy

| FST feature | VTR |
|---|---|
| scope with type (22 types incl. VHDL and `SV_ARRAY`), name, component | scope node; `ScopeType` uses the same codes 0..22; `component` string |
| var with 30 var types, 6 directions, length, static alias | var node; `VarType` codes 0..29, `Direction` 0..5, `SignalKind` width; alias = var with `declares = 0` referencing an earlier signal |
| PORT length quirk (3n+2) | width stored as the reader reports it (fst-reader normalises); var type `port` preserved |
| REAL / REAL_PARAMETER / REALTIME / SHORTREAL | `SignalKind::Real` with the original var type |
| GEN_STRING (variable length) | `SignalKind::VarLen` with var type `string` |
| MISC attributes: comment, envvar, supvar, pathname, sourcestem, sourceistem, valuelist, enumtable, unknown | comment: `META.comment`; supvar: node attribute `fst.vhdl {type_name, var_type, data_type}`; pathname+sourcestem: node attribute `fst.source_stem` / `fst.source_istem {file, line}` with the path resolved; enum table: enum-table node with (literal, value) entries + attribute `enum_table = node id` on the following var; envvar/valuelist/unknown: preserved as node attributes by the generic attribute mechanism (`fst.attr`, planned in the converter as data arrives; the format supports arbitrary keys) |
| ARRAY / PACK / ENUM (SV) bracketing attributes with name + arg | node attributes `fst.array {name, array_type, left, right}`, `fst.pack {name, pack_type, arg}`, `fst.sv_enum {name, enum_type, arg}` on the following node; `fst.attr_end` marks the closing bracket |
| enum tables (name, literals, values, min_valbits) | enum-table node; literal and value strings verbatim (value padding preserved as the string) |
| positional enum table reference | attribute on the var that follows the reference |
| empty scope names | preserved (empty string id 0) |
| `$unnamed_scope_N` synthesis | not needed (names preserved) |

### 1.3 Values

| FST feature | VTR |
|---|---|
| 1-bit 9-state values (`0 1 x z h u w l -`) | logic codes 0..8 in a 1-bit column entry (4 bits) |
| N-bit vectors, 2-state packed or 4-state ASCII | `Bits{states}`: 2-state 1 bit/bit; 4-state 2 bits/bit; 9-state 4 bits/bit; per-change compact flag keeps 2-state values at 1 bit/bit on 4/9-state signals |
| reals (8-byte doubles) | `Real` (binary64 LE) |
| variable-length byte strings | `VarLen` (`blob`) |
| initial values ('x' / NaN) before first change | frame default: all-X for 4/9-state, 0 for 2-state, 0.0 for reals, empty for varlen (documented deviation: reals default to 0.0 rather than NaN) |
| value at block begin (frame) | frame piece of every dirty group in every block |
| same-time multiple changes in emission order | allowed: several entries with `dt = 0` |
| time table per block, delta coded | block time table (varint deltas, compressed) |
| duplicate value suppression (`FST_REMOVE_DUPLICATE_VC`) | writer option `dedup` (default on) |
| `$dumpoff`/`$dumpon` blackout list | `BLACKOUT` section (converter reads FST block type 2 directly) |
| dump size limit | not a format feature; writer users stop calling the writer |

### 1.4 Writer-side features

| FST API | VTR |
|---|---|
| `fstWriterCreate` / `Close` | `Writer::create[_with]` / `close` (`vtr_writer_create` / `vtr_writer_close`) |
| `SetPackType` zlib/lz4/fastlz, `SetRepackOnClose` | `WriterOptions.compression` (zstd level or LZ4 or none); no whole-file wrapper (kept random access) |
| `SetParallelMode` | `WriterOptions.background` (on by default) |
| `SetTimescale[FromString]`, `SetTimezero`, `SetDate`, `SetVersion`, `SetComment`, `SetFileType`, `SetEnvVar`, `SetValueList` | `set_timescale`, `set_time_zero`, `set_date`, `set_writer_name`, `set_comment`, `set_file_type`, `set_file_attr` |
| `SetScope`/`SetUpscope` | `begin_scope`/`end_scope` |
| `CreateVar` / `CreateVar2` (svt/sdt) | `add_var`, `add_alias`, `node_attr` |
| `SetAttrBegin`/`SetAttrEnd`, `SetSourceStem`, `SetSourceInstantiationStem` | `node_attr` with typed values |
| `CreateEnumTable` / `EmitEnumTableRef` | `add_enum_table` + `node_attr("enum_table", U64(node))` |
| `EmitValueChange` (ASCII), `32/64`, `Vec32/Vec64` | `emit_logic_str`, `emit_u64`, `emit_words`, `emit_packed` |
| `EmitVariableLengthValueChange` | `emit_varlen` |
| `EmitTimeChange` | `set_time` |
| `EmitDumpActive` | `dump_off` / `dump_on` / `blackout_at` |
| `FlushContext` | `flush` |
| `.hier` sidecar crash recovery | no sidecar needed: the file itself is recoverable (directory rebuilt by scanning) |

### 1.5 Reader-side features

| FST API | VTR |
|---|---|
| `fstReaderOpen` (+ gzip wrapper) | `Reader::open` (mmap, reads directory/strings/hierarchy only) |
| header getters | `meta()`, `time_range()`, `signal_count()`, `hierarchy().len()`, `version()` |
| `IterateHier` / `ProcessHier` | `Hierarchy::nodes`, `roots()`, `children()`, `full_path()`, `find_node()` |
| `SetFacProcessMask*` + `IterBlocks[2]` (stream selected signals in time order) | `load_signals(&[ids])` (per-signal) or `for_each_change(t0, t1)` (all signals in time order) |
| `SetLimitTimeRange` | window arguments of `changes` and `for_each_change`, `TxQuery.window` |
| `GetValueFromHandleAtTime` (rvat) | `value_at(sig, t)`: block lookup + one run decompression + skip-index walk |
| blackout getters | `blackout()` |
| VCD extensions in `ProcessHier` | node attributes carry the same data; `vtr dump` prints a VCD-like text |
| double endian match | not applicable |
| partial reads | every query reads only the sections and runs it needs (`SPEC.md` 6.5, 7.3) |

## 2. FTR (LWTR4SC)

| FTR / lwtr feature | VTR |
|---|---|
| info chunk: timescale exponent, creation epoch | `META.timescale`; file attribute `ftr.epoch` |
| dictionary (string ids) | `STRINGS` sections (interned strings) |
| streams (id, name, kind) | stream node; `kind` string; attribute `ftr.id` keeps the original id |
| generators (id, name, stream) | generator node under the stream; `ftr.id` |
| transaction: id, generator, start, end | transaction: writer id, generator node, begin, end; original id kept as attribute `ftr.id` |
| BEGIN / RECORD / END attributes | `AttrPhase` begin/record/end on every attribute |
| attribute types: BOOLEAN, ENUMERATION, INTEGER, UNSIGNED, FLOATING_POINT, BIT_VECTOR, LOGIC_VECTOR, FIXED_POINT, UNSIGNED_FIXED_POINT, POINTER, STRING, TIME | `Value` tags bool, enum, i64, u64, f64, bits, logic, fixed/ufixed (FTR itself stores fixed point as a double, so the converter keeps `f64`), pointer, str, time |
| nested `object` values flattened to dotted names | dotted keys preserved; VTR additionally offers `Value::Map` for native producers |
| `sc_bv`/`sc_lv` recorded as strings | converted to packed `bits`/`logic` values (width = string length) |
| relations (name, from, to, from_stream, to_stream) | relation with kind string; `ftr.from_stream`/`ftr.to_stream` attributes keep the stream ids the FTR file recorded |
| `parent_of` relation from `begin_tx(..., parent)` | relation `parent_of` (VTR producers may also use the structural `parent` field) |
| `record_event` (zero-length child transaction) | converted as it is stored (a transaction); native producers use `tx_event` |
| open transactions closed with end = start at destructor | VTR closes open transactions with `status = open` and end = last time |
| block ordering by end time; LZ4 per chunk | same ordering concept; per-block indexes make queries local |
| `tx_db` recording on/off | caller's choice (stop calling the writer) |
| text backend (`.lwtrt`) | `vtr tx` prints an equivalent listing |

## 3. Kanata (Konata pipeline logs)

| Kanata command / concept | VTR |
|---|---|
| `Kanata 0004` header | file attribute `kanata.version` |
| `C= n` absolute start cycle (discarded by Konata itself) | file attribute `kanata.start_cycle`; all times absolute cycles (`time.unit = cycle`) |
| `C n` cycle advance | absolute cycle on every transaction, stage, event |
| `I id gid tid` | transaction (begin = cycle) on stream `thread<tid>` of the `instruction` generator; `insn_id_in_sim` attribute; `line` attribute |
| `L id 0 text` label | attribute `label` (appended, `\n` unescaped) |
| `L id 1 text` detail | attribute `detail` |
| `L id 2 text` stage label | attribute `label` on the most recently started stage |
| `S id lane stage` | stage `{name, lane, begin}`; opening a stage on a lane closes the open one (Konata semantics) |
| `E id lane stage` | stage end |
| zero-length stages, repeated stage names | preserved (begin == end; repeated stages are separate entries) |
| `R id rid 0` retire / `R id rid 1` flush | end time; attribute `retire_id`; status unset / aborted; other `type` values kept in attribute `type` |
| ops never retired (EOF) | status `open` |
| `W consumer producer type` | relation `wakeup` producer -> consumer, attribute `type` (when non-zero) plus the cycle it was recorded |
| lane ids as arbitrary strings | lane name strings |
| hidden conventions (`X` = execute, `f`/`stl` = stall, colours) | not data: expressed in the KDB (`docs/KDB_APPNOTE.md`) |
| multiple threads / cores | one stream per thread; scope `cpu` (type core) — native producers can nest scopes per core |

## 4. OpenTelemetry Tracing (OTLP)

| OTel concept | VTR |
|---|---|
| Resource (attributes, dropped count, schema URL) | scope node of type `resource` with the attributes as typed node attributes; `otel.schema_url` |
| InstrumentationScope (name, version, attributes, schema URL) | stream node (kind `otel.scope`) under the resource scope; `otel.version`, attributes, `otel.schema_url` |
| Span name | generator (one per distinct name per scope) |
| trace id (16 B), span id (8 B) | attributes `otel.trace_id`, `otel.span_id` (`bytes`) |
| parent span id | transaction `parent` |
| trace state, flags (incl. is_remote bits) | `otel.trace_state` (str), `otel.flags` (u64) |
| kind INTERNAL/SERVER/CLIENT/PRODUCER/CONSUMER | `TxKind` 1..5 |
| start/end time (ns since epoch) | begin/end with `META.timescale = -9` |
| attributes: string, bool, int64, double, bytes, array, kvlist, nested | `Value` str/bool/i64/f64/bytes/list/map (recursive) |
| dropped attribute/event/link counts | `otel.dropped_*` attributes |
| events (time, name, attributes, dropped) | `TxEvent` |
| links (trace id, span id, trace state, attributes, flags) | relation `otel.link` with those attributes; target outside the file = relation to id 0 with `otel.span_id` attribute |
| status UNSET/OK/ERROR + message | `TxStatus` unset/ok/error; `otel.status_message` |
| ResourceSpans / ScopeSpans grouping | hierarchy nesting resource -> scope stream -> generator |
| SDK limits, sampling decisions, processors | out of scope (not data in the exported trace) |

## 5. Beyond the union

VTR adds what none of the four have in one place: signals and
transactions in one file with one time base; stages *and* events *and*
relations *and* a structural parent on transactions; typed attributes
(including bit vectors, time, enums, fixed point, lists and maps) on
hierarchy nodes, transactions, events, stages and relations; per-block
indexes for local queries; crash recovery without sidecar files; and a
versioned, extensible container.
