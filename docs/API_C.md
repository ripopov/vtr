# VTR C API reference (`libvtr`)

This document is the reference for the C API of VTR (Vibe Trace Record). The
header `crates/vtr-capi/include/vtr.h` is the contract; this document explains
every function and struct in it, the semantics behind them, and how they map
onto the Rust implementation in `crates/vtr`.

Contents

1. [Overview](#1-overview)
2. [Conventions](#2-conventions)
3. [Writer API](#3-writer-api)
4. [Reader API](#4-reader-api)
5. [Complete examples](#5-complete-examples)
6. [Error handling and mapping to the Rust API](#6-error-handling-and-mapping-to-the-rust-api)
7. [SystemC / C++ usage](#7-systemc--c-usage)

---

## 1. Overview

### 1.1 What libvtr is

`libvtr` is a C-ABI wrapper (crate `vtr-capi`) around the Rust VTR library
(crate `vtr`). It exposes two objects:

* a **writer** (`vtr_writer`) that streams a trace file from a running
  simulator: design hierarchy, signal value changes, transactions with
  attributes/events/stages, and relations between transactions;
* a **reader** (`vtr_reader`) that memory-maps a finished (or crashed and
  recoverable) file and answers random-access queries: value of a signal at a
  time, changes in a window, full signal history, transaction queries by
  generator/stream/time, relation lookups.

The Rust crate is the implementation; the C layer adds no logic of its own
beyond argument marshalling and error translation. Everything the C API can do
is a thin call into `vtr::Writer` / `vtr::Reader` (see section 6.2).

### 1.2 Building and linking

```sh
cargo build --release -p vtr-capi
```

produces, in `target/release/`:

| Artifact      | Use                                  |
|---------------|--------------------------------------|
| `libvtr.a`    | static library (recommended)         |
| `libvtr.so`   | shared library (`libvtr.dylib`/`vtr.dll` on other platforms) |

Compile against the header and link with the library plus the system
libraries the Rust runtime needs:

```sh
cc -std=c99 -I crates/vtr-capi/include my_tool.c target/release/libvtr.a -lpthread -ldl -lm -o my_tool
# or, with the shared library:
cc -std=c99 -I crates/vtr-capi/include my_tool.c -L target/release -lvtr -lpthread -ldl -lm -o my_tool
```

The header is C99 (`<stdint.h>`, `<stddef.h>`) and has an `extern "C"` guard,
so it can be included from C++ unchanged. `cargo test -p vtr-capi` compiles
and runs `crates/vtr-capi/tests/c_smoke.c` exactly this way, with
`-Wall -Wextra -Werror`. A CMake example is in `bench/cpp/CMakeLists.txt`
(target `vtr_write`).

Compression codecs (LZ4, Zstandard) and the CRC32 implementation are compiled
into the library; there are no runtime dependencies beyond libc, libpthread,
libdl and libm.

### 1.3 ABI stability policy

* The **header is the contract**. Every exported symbol, struct layout and
  numeric code in `vtr.h` is stable for a given *file format major version*.
  The library currently writes and reads format version **1.0**
  (`vtr_meta.version_major == 1`).
* Functions are versioned by the file-format major version: an incompatible
  change to a function or struct is only made together with a major version
  bump of the file format, at which point the old symbols are kept where
  practical and the new behaviour gets new names.
* Adding new functions, new `VTR_VAL_*` tags, new scope/var type codes or new
  fields *at the end* of `vtr_writer_options` is a minor change. Callers must
  therefore initialise `vtr_writer_options` with
  `vtr_writer_options_default()` rather than by aggregate initialisation, and
  must not assume `sizeof` of any struct.
* `vtr_version()` returns the library (crate) version string, e.g. `"0.1.0"`.
  It is informational; compatibility is decided by the format version stored
  in the file (`vtr_meta.version_major/minor`), and a reader refuses files
  written by a newer major version with `VTR_ERR_VERSION`.

### 1.4 Threading model

* **No global state.** All state lives in the handles. Any number of writers
  and readers can coexist in one process.
* **Writer: one thread at a time.** A `vtr_writer` may be used from any
  thread, but not from two threads concurrently. Internally, with
  `background = 1` (the default), block encoding and compression run on a
  helper thread named `vtr-writer`; the caller's thread only pays for logging.
  Errors raised on the helper thread are reported by the next writer call
  (typically as `VTR_ERR_STATE "background writer failed"`) and by
  `vtr_writer_close`.
* **Reader: shareable.** After `vtr_reader_open` returns, the `vtr_reader`
  may be used concurrently from several threads for all read-only calls
  (everything taking `const vtr_reader *`). Internal caches are
  synchronised. Objects created per query — `vtr_value_buf`,
  `vtr_signal_data` — are *not* thread-safe and must be confined to one
  thread at a time (`vtr_value_buf_ascii` in particular mutates its buffer).
* **Thread-local last error.** `vtr_last_error()` returns the message of the
  last failure that happened *on the calling thread*.
* Callbacks (`vtr_change_cb`, `vtr_tx_cb`, `vtr_relation_cb`) are invoked on
  the thread that made the query, synchronously, before the query function
  returns.

---

## 2. Conventions

### 2.1 Status codes

Every function that can fail returns `int`: `VTR_OK` (0) on success or one of
the following. Functions that return a handle return `NULL` on failure;
functions that return an id return `VTR_NONE` on failure.

| Code                | Value | Meaning |
|---------------------|-------|---------|
| `VTR_OK`            | 0     | Success. |
| `VTR_ERR_INVALID`   | 1     | Bad argument: unknown signal/node/transaction id, time earlier than the current time, value not representable on the signal (x on a 2-state signal), wrong emit function for the signal kind, packed buffer too short, unsupported `vtr_value` tag on input, `states` not in {2,4,9}, `kind` not in {0,1,2}, `vtr_reader_enum_entry` on a node that is not an enum table. |
| `VTR_ERR_STATE`     | 2     | Call not allowed in the writer's current state: metadata set after the first flush, `vtr_writer_node_attr` on a node that has already been flushed, `vtr_writer_end_scope` without an open scope, `vtr_writer_tx_stage_attr` on a transaction without stages, or the background encoder thread has failed. |
| `VTR_ERR_IO`        | 3     | Operating-system I/O failure (create/open/read/write/mmap). |
| `VTR_ERR_CORRUPT`   | 4     | Not a VTR file, or a structural invariant of the file is violated. Also reported by `vtr_writer_create` for an unknown `codec` (as a NULL return with this message). |
| `VTR_ERR_VERSION`   | 5     | The file was written by a newer *major* format version than this library understands. |
| `VTR_ERR_CODEC`     | 6     | Compression or decompression failed. |
| `VTR_ERR_CHECKSUM`  | 7     | A section's CRC32 did not match (checked on first access of the section). |
| `VTR_ERR_NULL`      | 8     | A handle, output struct, callback or required string argument was NULL, or a string argument was not valid UTF-8. |
| `VTR_ERR_NOT_FOUND` | 9     | Lookup miss: path not found, index past the end of a list, transaction id not in the file, `vtr_writer_tx_stage_end` with no matching open stage. |

`vtr_last_error()` returns a NUL-terminated, thread-local message (never
NULL; the empty string before any failure). It is updated whenever a
library-originated error is translated to a status code or a NULL/`VTR_NONE`
return. It is **not** updated for the purely index-based misses of reader
accessors (`VTR_ERR_NOT_FOUND` from `vtr_reader_node`, `vtr_tx_attr`, ...)
nor by `vtr_writer_tx_stage_end` returning `VTR_ERR_NOT_FOUND`, and it is not
cleared on success. Always test the return code first and consult the message
only for diagnostics.

### 2.2 `VTR_NONE`

`VTR_NONE` (`0xFFFFFFFF`) is the "no id" value for all `uint32_t` ids: it is
returned by id-returning writer functions on failure, stored in
`vtr_node_info.parent` for root nodes and in `vtr_node_info.signal` for
non-variable nodes, and accepted as "no filter"/"no parent" arguments
(`vtr_writer_add_stream`, `vtr_reader_children`,
`vtr_reader_visit_transactions`).

### 2.3 Id types

| Id                | Type       | Range and meaning |
|-------------------|------------|-------------------|
| node id           | `uint32_t` | Dense, `0 .. node_count-1`, assigned in declaration order to every hierarchy node (scope, var, stream, generator, enum table). |
| signal id         | `uint32_t` | Dense, `0 .. signal_count-1`, assigned in the order `vtr_writer_add_var` created signals. Several variables (aliases) may share one signal. |
| string id         | `uint32_t` | Dense index into the file's string table. **Id 0 is always the empty string.** Ids are produced by `vtr_writer_intern` and appear in the reader wherever a name/key is returned. |
| transaction id    | `uint64_t` | Assigned by `vtr_writer_begin_tx`, **starting at 1** and increasing by one per transaction. 0 is never a valid transaction id and can be used as an "external"/"none" target in relations. |
| time              | `uint64_t` | In file time units: `10^timescale` seconds (default `10^-9`). |

### 2.4 Ownership and lifetime of returned pointers

| Pointer                                         | Owner / valid until |
|-------------------------------------------------|---------------------|
| `vtr_writer *` from `vtr_writer_create`         | Caller. Freed by `vtr_writer_close` (always, even when close reports an error). |
| `vtr_reader *` from `vtr_reader_open`           | Caller. Freed by `vtr_reader_close`. |
| `vtr_value_buf *` from `vtr_value_buf_new`      | Caller. Freed by `vtr_value_buf_free`. |
| `vtr_signal_data *` from `vtr_reader_load_signal` | Caller. Freed by `vtr_signal_data_free`. Independent of the reader (it is a copy), but see below. |
| `const char *` from `vtr_last_error`            | Library, thread-local; valid until the next failing call on the same thread. |
| `const char *` from `vtr_version`               | Static; valid forever. |
| `const char *` from `vtr_reader_str`, `vtr_reader_meta_string` | Points into the reader; valid while the reader is alive. Not NUL-terminated. |
| `const uint64_t *` from `vtr_reader_time_table` | Points into the reader; valid while the reader is alive. |
| `const uint64_t *` from `vtr_signal_data_times`, `data` in `vtr_signal_value` from `vtr_signal_data_get/_initial` | Points into the `vtr_signal_data`; valid until it is freed. |
| `data` in `vtr_signal_value` from `vtr_value_buf_get`, `const char *` from `vtr_value_buf_ascii` | Points into the `vtr_value_buf`; valid until the next `vtr_reader_value_at` / `vtr_value_buf_ascii` call on that buffer or until it is freed. |
| `data` in `vtr_value` from `vtr_reader_file_attr`, `vtr_reader_node_attr` | Points into the reader; valid while the reader is alive. |
| `const vtr_tx *` and every pointer obtained from it (`vtr_tx_attr`, `vtr_tx_event_attr`, `vtr_tx_stage_attr` values) | Valid only for the duration of the callback that received the `vtr_tx`. Copy what you need. |
| `keys`, `values` and their `data` in `vtr_relation_cb` | Valid only for the duration of the callback. |
| `const vtr_signal_value *` in `vtr_change_cb` and its `data` | Valid only for the duration of the callback. |

Nothing else needs to be freed. Input buffers (`const uint8_t *data`,
`const uint32_t *words`, strings, `vtr_value` arrays) are copied by the
writer before the call returns and may be reused immediately.

### 2.5 String encoding

* Input strings are NUL-terminated UTF-8 unless a length parameter exists
  (`vtr_writer_emit_logic_str` takes `len`, or `SIZE_MAX` for
  NUL-terminated). Invalid UTF-8 in a NUL-terminated string argument yields
  `VTR_ERR_NULL` ("null or non-UTF-8 string"); in id-returning functions it
  yields `VTR_NONE` / `0`.
* Output strings from the reader are returned as `(pointer, length)` and are
  **not** NUL-terminated (`vtr_reader_str`, `vtr_reader_meta_string`). Use
  `printf("%.*s", (int)len, s)` or copy them. The two exceptions are
  `vtr_last_error()`, `vtr_version()` and `vtr_value_buf_ascii()`, which are
  NUL-terminated.
* Variable-length signal values (`kind == 2`) and `VTR_VAL_BYTES` values are
  raw bytes, not necessarily text.

### 2.6 The `vtr_value` struct

`vtr_value` is the tagged attribute value used for file attributes, node
attributes, transaction attributes, event attributes, stage attributes and
relation attributes. It mirrors `vtr::Value`.

```c
typedef struct vtr_value {
    uint8_t  tag;      /* VTR_VAL_* */
    uint8_t  b;
    uint32_t width;
    int64_t  i;
    uint64_t u;
    double   f;
    uint32_t str_id;
    int32_t  scale;
    const uint8_t *data;
    size_t   len;
} vtr_value;
```

Always `memset` the struct to zero before filling the fields for the tag;
fields not listed for a tag are ignored on input and zero on output.

| Tag               | Value | Fields used | Meaning |
|-------------------|-------|-------------|---------|
| `VTR_VAL_NULL`    | 0     | —           | No value. |
| `VTR_VAL_BOOL`    | 1     | `b`         | `b != 0` is true. |
| `VTR_VAL_I64`     | 2     | `i`         | Signed integer. |
| `VTR_VAL_U64`     | 3     | `u`         | Unsigned integer. |
| `VTR_VAL_F64`     | 4     | `f`         | IEEE double. |
| `VTR_VAL_STR`     | 5     | `str_id`    | Interned string (a string id, see `vtr_writer_intern`). |
| `VTR_VAL_BYTES`   | 6     | `data`, `len` | Byte blob. `data` may be NULL when `len == 0`. |
| `VTR_VAL_BITS`    | 7     | `width`, `data`, `len` | 2-state bit vector, packed LSB-first, 1 bit per bit (`len >= ceil(width/8)`). |
| `VTR_VAL_LOGIC`   | 8     | `width`, `data`, `len` | 4-state vector, 2 bits per bit (`len >= ceil(width/4)`), codes 0,1,X=2,Z=3. |
| `VTR_VAL_LOGIC9`  | 9     | `width`, `data`, `len` | 9-state vector, 4 bits per bit (`len >= ceil(width/2)`), codes 0..8 (section 3.6). |
| `VTR_VAL_TIME`    | 10    | `u`         | Time in file time units. |
| `VTR_VAL_ENUM`    | 11    | `i`, `str_id` | Enumeration literal: integer value `i` and literal name `str_id`. |
| `VTR_VAL_TEXT`    | 17    | `data`, `len` | Inline UTF-8 text, not interned (log arguments; one-off strings). Input: `data`/`len`, must be valid UTF-8. |
| `VTR_VAL_POINTER` | 12    | `u`         | Opaque pointer / handle. |
| `VTR_VAL_FIXED`   | 13    | `i`, `scale` | Signed fixed point: value = `i * 2^-scale`. |
| `VTR_VAL_UFIXED`  | 14    | `u`, `scale` | Unsigned fixed point: value = `u * 2^-scale`. |
| `VTR_VAL_LIST`    | 15    | `len`       | **Read-only.** `len` = element count; elements are not exposed through the C API. Rejected on input with `VTR_ERR_INVALID`. |
| `VTR_VAL_MAP`     | 16    | `len`       | **Read-only.** `len` = entry count; entries are not exposed. Rejected on input with `VTR_ERR_INVALID`. |

Packing of `VTR_VAL_BITS/LOGIC/LOGIC9` data is the same as for signal values
(section 4.8). On output, `data` points into the owning object (section 2.4).

---

## 3. Writer API

All writer functions take a `vtr_writer *w`; a NULL `w` yields `VTR_ERR_NULL`
(status functions) or `VTR_NONE` / `0` (id functions).

### 3.1 Lifecycle

```c
typedef struct vtr_writer_options {
    uint8_t  codec;          /* 0 none, 1 lz4, 2 zstd (default) */
    int      level;          /* zstd level (default 3) */
    uint32_t group_size;     /* signals per value-change group (default 256) */
    uint64_t block_records;  /* value changes per signal block, the compression unit (default 16M) */
    uint64_t chunk_records;  /* minimum value changes per hand-off to the background encoder (default 512K) */
    uint64_t tx_block_bytes; /* row bytes per transaction block (default 4 MiB) */
    int      background;     /* encode/compress on a background thread (default 1) */
    int      dedup;          /* drop value changes equal to the current value (default 1) */
    int      checksums;      /* store a CRC32 per section (default 1); readers verify on request */
    uint32_t log_encoders;   /* helper threads encoding log blocks in background mode (default 2; 0 = sink thread) */
} vtr_writer_options;

void        vtr_writer_options_default(vtr_writer_options *o);
vtr_writer *vtr_writer_create(const char *path, const vtr_writer_options *opts);
int         vtr_writer_close(vtr_writer *w);
```

**`vtr_writer_options_default(o)`** fills `*o` with the defaults listed in the
struct comments (`codec = 2`, `level = 3`, `group_size = 256`,
`block_records = 16777216`, `chunk_records = 524288`, `tx_block_bytes = 4194304`, `background = 1`,
`dedup = 1`, `checksums = 1`). A NULL `o` is ignored.

| Option           | Notes |
|------------------|-------|
| `codec`          | 0 = no compression, 1 = LZ4 block, 2 = Zstandard. Any other value makes `vtr_writer_create` fail (NULL, message "unknown codec id"). |
| `level`          | Zstandard level (1..22). Ignored for LZ4 and none. |
| `group_size`     | Signals per value-change group; rounded up to a power of two (minimum 1). Larger groups compress better, smaller groups make single-signal reads cheaper. Fixed for the whole file. |
| `block_records`  | Value changes per signal block (the compression unit). A block is a random-access unit for the reader; more records per block = fewer, larger blocks and longer columns for the compressor. |
| `chunk_records`  | Lower bound on the value changes handed to the background encoder at a time (the pipelining unit, default 524288); the writer raises it to 16 changes per declared signal so that column fragments stay large in designs with very many signals. Each chunk is sorted and pre-encoded as soon as it arrives; only the final compression waits for the whole block. Bounded by `block_records`. |
| `tx_block_bytes` | Bytes of transaction/relation rows buffered before a transaction block is written. |
| `background`     | 1: compress on a helper thread (recommended for simulators). 0: everything happens inline on the caller's thread (deterministic, slightly slower). |
| `dedup`          | 1: an emitted value equal to the signal's current value is silently dropped (no change record). 0: every emit produces a record. |
| `checksums`      | 1: every section carries a CRC32. The Rust reader verifies it when opened with `ReadOptions::verify_crc`; `vtr_reader_open` uses the defaults (no verification). |
| `log_encoders`   | Helper threads that encode and compress log blocks in background mode (default 2, started on the first log block; 0 = the background thread does it). |

**`vtr_writer_create(path, opts)`** creates (truncates) the file at `path`
and returns a writer, or NULL with `vtr_last_error()` set. `opts` may be
NULL for defaults. The metadata "writer" string defaults to
`"vtr <library version>"`; the timescale defaults to `-9` (ns).

**`vtr_writer_close(w)`** finishes the file and **always frees the handle**,
even when it returns an error. It:

1. records every still-open transaction with status **4 (open)** and end
   time `max(current writer time, transaction begin)`; open stages of those
   transactions are closed at that end time;
2. flushes pending strings, metadata and hierarchy;
3. writes the final signal block (also when no value was ever emitted but
   `vtr_writer_set_time` was called, so the time range is preserved) and the
   final transaction block;
4. writes the blackout (dump on/off) list;
5. writes the directory and end marker, joins the background thread, and
   returns its status.

A NULL `w` returns `VTR_ERR_NULL`. There is no other way to release a writer;
a process that exits without calling close leaves a file without a
directory, which readers can still open in *recovered* mode
(`vtr_meta.recovered == 1`) by scanning the sections, losing only what was
still buffered.

### 3.2 Metadata

```c
int vtr_writer_set_timescale(vtr_writer *w, int8_t exp);   /* 10^exp seconds, default -9 */
int vtr_writer_set_time_zero(vtr_writer *w, int64_t t);
int vtr_writer_set_file_type(vtr_writer *w, uint8_t ft);
int vtr_writer_set_writer_name(vtr_writer *w, const char *s);
int vtr_writer_set_date(vtr_writer *w, const char *s);
int vtr_writer_set_comment(vtr_writer *w, const char *s);
int vtr_writer_set_file_attr(vtr_writer *w, const char *key, const vtr_value *v);
```

All metadata setters must be called **before the first flush**. A flush
happens on an explicit `vtr_writer_flush`, or automatically when
`block_records` value changes or `tx_block_bytes` of transaction rows have
accumulated. After that they return `VTR_ERR_STATE`
("metadata must be set before the first flush"). Set metadata right after
`vtr_writer_create`.

| Function | Parameters | Notes |
|----------|-----------|-------|
| `set_timescale` | `exp`: time unit is `10^exp` s (`-9` ns, `-12` ps, `-15` fs). | Default `-9`. Every `uint64_t` time in the API is in this unit. |
| `set_time_zero` | `t`: offset added by viewers when displaying absolute time (FST "timezero"). | Default 0. Does not affect stored times. |
| `set_file_type` | `ft`: 0 verilog, 1 vhdl, 2 verilog+vhdl (mixed), 3 systemc, 4 architectural (pipeline simulator), 5 software (OpenTelemetry etc.). | Any other value is stored as 255 "other". Default 0. |
| `set_writer_name` | producing tool and version | Overrides the `"vtr <version>"` default. |
| `set_date` | free-form creation date | |
| `set_comment` | free-form comment | |
| `set_file_attr` | `key` (string, interned automatically), `v` (any input-capable tag; copied) | Appends one file-level attribute. Keys are not deduplicated. |

### 3.3 Strings

```c
uint32_t vtr_writer_intern(vtr_writer *w, const char *s);
```

Interns `s` and returns its string id. Interning an already-interned string
is a hash lookup and returns the same id; the empty string is always id 0.
Returns 0 also when `w` or `s` is NULL or `s` is not UTF-8, so pass valid
arguments (there is no separate error signal).

String ids are needed for everything in the transaction API that is a
"name": attribute keys, event names, stage names and lanes, relation kinds,
and `VTR_VAL_STR` / `VTR_VAL_ENUM` values. Intern them once at setup and keep
the ids; do not intern per emitted transaction unless the set of strings is
unbounded. Hierarchy functions and `vtr_writer_node_attr` /
`vtr_writer_set_file_attr` take plain `const char *` and intern internally.

Strings are written to the file in append-only chunks at each flush; the
reader sees the concatenation, so ids are stable for the life of the file.

### 3.4 Hierarchy

The hierarchy is a forest of nodes: scopes, variables, enum tables, streams
and generators. Node ids are assigned in creation order. Scopes nest through
an internal scope stack: `vtr_writer_begin_scope` pushes, `vtr_writer_end_scope`
pops, and `add_var`, `add_alias`, `add_enum_table` attach to the innermost
open scope (or to the root when none is open). Streams and generators take
explicit parents and ignore the scope stack. Hierarchy may be declared at any
time, also after value changes have been emitted (nodes are appended to the
file at the next flush).

```c
uint32_t vtr_writer_begin_scope(vtr_writer *w, const char *name, uint16_t scope_type, const char *component);
int      vtr_writer_end_scope(vtr_writer *w);
int      vtr_writer_add_var(vtr_writer *w, const char *name, uint16_t var_type, uint8_t direction,
                            uint8_t kind, uint32_t width, uint8_t states, uint32_t *node_out, uint32_t *signal_out);
int      vtr_writer_add_alias(vtr_writer *w, const char *name, uint16_t var_type, uint8_t direction, uint32_t signal, uint32_t *node_out);
uint32_t vtr_writer_add_enum_table(vtr_writer *w, const char *name, size_t n, const char *const *literals, const char *const *values);
uint32_t vtr_writer_add_stream(vtr_writer *w, uint32_t parent, const char *name, const char *kind);
uint32_t vtr_writer_add_generator(vtr_writer *w, uint32_t stream, const char *name);
int      vtr_writer_node_attr(vtr_writer *w, uint32_t node, const char *key, const vtr_value *v);
```

**`vtr_writer_begin_scope(w, name, scope_type, component)`** opens a scope
under the current scope and returns its node id (`VTR_NONE` if `w` or `name`
is NULL). `component` is the module/entity/class name that this scope
instantiates (FST "component"); it may be NULL (stored as the empty string).
`scope_type` is one of the codes below; unknown codes are preserved verbatim.

Scope type codes (0..22 are the FST `FST_ST_*` codes; see `docs/SPEC.md`):

| Code | Scope type | Code | Scope type |
|------|------------|------|------------|
| 0  | module            | 13 | vhdl_procedure |
| 1  | task              | 14 | vhdl_function |
| 2  | function          | 15 | vhdl_record |
| 3  | begin             | 16 | vhdl_process |
| 4  | fork              | 17 | vhdl_block |
| 5  | generate          | 18 | vhdl_for_generate |
| 6  | struct            | 19 | vhdl_if_generate |
| 7  | union             | 20 | vhdl_generate |
| 8  | class             | 21 | vhdl_package |
| 9  | interface         | 22 | sv_array |
| 10 | package           | 64 | generic (no HDL meaning) |
| 11 | program           | 65 | sc_module (SystemC) |
| 12 | vhdl_architecture | 66 | resource (OpenTelemetry) |
|    |                   | 67 | instrumentation_scope (OpenTelemetry) |
|    |                   | 68 | core (CPU core / hardware thread) |

**`vtr_writer_end_scope(w)`** closes the innermost scope. `VTR_ERR_STATE` if
no scope is open. Scopes left open at close are simply left open (the
hierarchy is still valid).

**`vtr_writer_add_var(w, name, var_type, direction, kind, width, states, &node, &sig)`**
declares a variable with a **new signal** in the current scope and returns
both the node id and the signal id through the (nullable) out pointers.

* `kind` selects the value encoding:

  | `kind` | Signal kind | `width` | `states` | Emit with |
  |--------|-------------|---------|----------|-----------|
  | 0 | bit vector | number of bits (0 is treated as 1) | 2, 4 or 9 (anything else: `VTR_ERR_INVALID`) | `emit_bit`, `emit_u64`, `emit_words`, `emit_logic_str`, `emit_packed` |
  | 1 | real (IEEE double) | ignored | ignored | `emit_real` (`emit_u64` and a numeric `emit_logic_str` are converted) |
  | 2 | variable length (strings, blobs) | ignored | ignored | `emit_varlen` (`emit_logic_str` stores the raw bytes) |

  `states` is the number of logic states each bit can take: 2 (`0/1`), 4
  (`0/1/X/Z`), 9 (IEEE 1164 `0/1/X/Z/U/W/L/H/-`). It bounds what may be
  emitted and determines the storage packing (section 4.8); a 2-state value
  emitted on a 4/9-state signal is stored compactly, so declaring 4 states for
  RTL signals costs nothing while they are 2-state.
* `var_type` is a code from the table below (0..29 are the FST `FST_VT_*`
  codes); unknown codes are preserved verbatim. It is descriptive only — the
  value encoding comes from `kind`.

  | Code | Var type | Code | Var type | Code | Var type |
  |------|----------|------|----------|------|----------|
  | 0 | event | 11 | trior | 22 | bit |
  | 1 | integer | 12 | trireg | 23 | logic |
  | 2 | parameter | 13 | tri0 | 24 | int |
  | 3 | real | 14 | tri1 | 25 | shortint |
  | 4 | real_parameter | 15 | wand | 26 | longint |
  | 5 | reg | 16 | wire | 27 | byte |
  | 6 | supply0 | 17 | wor | 28 | enum |
  | 7 | supply1 | 18 | port | 29 | shortreal |
  | 8 | time | 19 | sparray | 64 | bits (generic bit vector) |
  | 9 | tri | 20 | realtime | 65 | bytes (generic blob) |
  | 10 | triand | 21 | string | | |

* `direction` (FST `FST_VD_*`): 0 implicit, 1 input, 2 output, 3 inout,
  4 buffer, 5 linkage. Other values are stored as 0.

**`vtr_writer_add_alias(w, name, var_type, direction, signal, &node)`**
declares a variable that shares the value stream of an existing `signal`
(e.g. a port connected to a net). `VTR_ERR_INVALID` if `signal` does not
exist. The reader reports such nodes with `is_alias == 1`.

**`vtr_writer_add_enum_table(w, name, n, literals, values)`** declares an
enumeration table node in the current scope with `n` entries
`(literals[i], values[i])`; `values[i]` is the encoding of the literal as a
string (typically a bit string, as in FST). NULL entries become the empty
string. Returns the node id (`VTR_NONE` on NULL `w`/`name`). Link a variable to
its table with a node attribute if desired; the format does not enforce a
link.

**`vtr_writer_add_stream(w, parent, name, kind)`** declares a transaction
stream. `parent` is a scope node id or `VTR_NONE` for a root stream. `kind` is
a free-form classification string (FTR stream kind, e.g. `"PIPELINE"`,
`"TLM"`); NULL is stored as the empty string. Returns the node id.

**`vtr_writer_add_generator(w, stream, name)`** declares a transaction
generator (transaction type) as a child of `stream` and returns its node id.
Transactions are begun on generators. `stream` is not validated here; passing
a node that is not a stream produces a hierarchy that readers will report as
is.

**`vtr_writer_node_attr(w, node, key, v)`** appends a typed attribute to a
node. The node must have been created **since the last flush** (attributes
are stored with the node and nodes are written at flush), otherwise
`VTR_ERR_STATE`. Set attributes immediately after creating the node.

### 3.5 Time

```c
int vtr_writer_set_time(vtr_writer *w, uint64_t t);
int vtr_writer_dump_off(vtr_writer *w);
int vtr_writer_dump_on(vtr_writer *w);
```

**`vtr_writer_set_time(w, t)`** advances the current time. `t` must be
**non-decreasing** across calls; an earlier `t` returns `VTR_ERR_INVALID`
("time N is earlier than current time M") and leaves the writer unchanged.
Calling it again with the same `t` is a no-op. The first call may use any
value (there is no requirement to start at 0). Before the first call the
current time is 0, and values emitted then are recorded at time 0. Every
distinct time value becomes an entry of the file's time table (section 4.13),
whether or not a value changed at it.

Transaction times (`begin_tx`, `end_tx`, events, stages) are explicit
parameters and are independent of `set_time`; only the writer's current time
is used for open transactions at close (section 3.1) and for blackout marks.

**`vtr_writer_dump_off(w)` / `vtr_writer_dump_on(w)`** record a VCD-style
`$dumpoff` / `$dumpon` mark at the current time (a "blackout" region). They
only record the marks; the writer keeps accepting value changes. Viewers use
the marks to grey out regions. Always `VTR_OK` for a valid handle.

### 3.6 Values

```c
int vtr_writer_emit_bit(vtr_writer *w, uint32_t sig, uint8_t code);
int vtr_writer_emit_u64(vtr_writer *w, uint32_t sig, uint64_t value);
int vtr_writer_emit_words(vtr_writer *w, uint32_t sig, const uint32_t *words, size_t n);
int vtr_writer_emit_logic_str(vtr_writer *w, uint32_t sig, const char *s, size_t len);
int vtr_writer_emit_packed(vtr_writer *w, uint32_t sig, uint8_t states, const uint8_t *data, size_t len);
int vtr_writer_emit_real(vtr_writer *w, uint32_t sig, double v);
int vtr_writer_emit_varlen(vtr_writer *w, uint32_t sig, const uint8_t *data, size_t len);
int vtr_writer_flush(vtr_writer *w);
```

Every emit records a value change of signal `sig` at the current time. An
unknown `sig` returns `VTR_ERR_INVALID`; using an emit function that does not
fit the signal kind returns `VTR_ERR_INVALID` (see the per-function notes).

Common rules:

* **Dedup.** With `dedup = 1` (default) a value equal to the signal's current
  value is dropped and `VTR_OK` is returned. The initial value of every signal
  is: all `0` for 2-state vectors, all `X` for 4/9-state vectors, `0.0` for
  reals, the empty byte string for variable-length signals. Emitting that
  initial value first is therefore also a no-op — which is fine, because the
  reader returns exactly that initial value before the first change.
* **Several changes at one time.** Emitting the same signal repeatedly at one
  time step records every distinct value; queries return the last one.
* **Blocks.** When `block_records` changes are buffered, a signal block is
  written automatically (on the background thread by default). Blocks are
  invisible to value semantics; they only bound the reader's decoding
  granularity and memory.

Logic codes, one per bit (used by `emit_bit`, `emit_packed` and the packed
representation on the reader side):

| Code | Logic | Code | Logic | Code | Logic |
|------|-------|------|-------|------|-------|
| 0 | `0` | 3 | `Z` | 6 | `L` (weak 0) |
| 1 | `1` | 4 | `U` (uninitialised) | 7 | `H` (weak 1) |
| 2 | `X` | 5 | `W` (weak unknown) | 8 | `-` (don't care) |

Codes 0..1 need a 2-state signal, 2..3 need at least 4 states, 4..8 need 9
states.

**`vtr_writer_emit_bit(w, sig, code)`** emits a single logic code.

* 1-bit vector signal: `code` 0..8 (values above 8 are clamped to 8). If the
  code needs more states than the signal has: `VTR_ERR_INVALID`.
* wider vector signal: the value `code & 1` is emitted as an integer
  (`emit_u64` semantics, zero-extended).
* real / varlen signal: `VTR_ERR_INVALID`.

**`vtr_writer_emit_u64(w, sig, value)`** emits a 2-state integer.

* vector signal: `value` is truncated to `width` bits (for `width < 64`) and
  zero-extended to the declared width. On 4/9-state signals the value is
  stored compactly as 2-state.
* real signal: converted with `(double)value`.
* varlen signal: `VTR_ERR_INVALID`.

**`vtr_writer_emit_words(w, sig, words, n)`** emits a 2-state value of any
width from little-endian 32-bit words: `words[0]` holds bits 0..31,
`words[1]` bits 32..63, and so on. Missing words (when `n * 32 < width`) are
zero; extra words and bits above `width` are ignored. `words` may be NULL with
`n == 0` (value 0). Non-vector signals: `VTR_ERR_INVALID`.

**`vtr_writer_emit_logic_str(w, sig, s, len)`** emits a value written as a
VCD-style ASCII string. `len` is the number of characters, or `SIZE_MAX` to
use `strlen(s)`. `s` may not be NULL (`VTR_ERR_NULL`).

* vector signal: characters `0 1 x z X Z u U w W l L h H -` are logic codes
  (case-insensitive); any other character is `X`. The string is **MSB first**:
  the last character is bit 0. Strings shorter than `width` are left-extended
  following the VCD rule — with the leftmost character if it is one of
  `x z u w l h -`, otherwise with `0` (`"zz"` on a 32-bit signal is all Z,
  `"1"` is `0…01`). Longer strings are truncated to the low `width` bits (the
  leading characters are dropped). A value containing anything other than
  `0`/`1` on a 2-state signal, or `u w l h -` on a 4-state signal, returns
  `VTR_ERR_INVALID`. Values that are all `0/1` are stored compactly.
* 1-bit signal: only the last character is used (an empty string means `X`).
* real signal: the text is parsed as a decimal floating-point number
  (surrounding whitespace ignored); `VTR_ERR_INVALID` if it does not parse.
* varlen signal: the bytes are stored verbatim (same as `emit_varlen`).

**`vtr_writer_emit_packed(w, sig, states, data, len)`** emits a vector
already packed in VTR layout (section 4.8) with `states` states per bit.
`states` must be 2 (compact 2-state value, allowed on any vector signal) or
the signal's declared states; `len` must be at least the packed length
(`ceil(width/8)`, `ceil(width/4)` or `ceil(width/2)` bytes for 2/4/9 states),
otherwise `VTR_ERR_INVALID`. Extra bytes are ignored. This is the fastest way
to emit wide vectors when the simulator already holds them in this layout.
Non-vector signals: `VTR_ERR_INVALID`.

**`vtr_writer_emit_real(w, sig, v)`** emits a double. Non-real signals:
`VTR_ERR_INVALID`.

**`vtr_writer_emit_varlen(w, sig, data, len)`** emits `len` bytes (a string
without its terminator, or a blob). `data` may be NULL when `len == 0`.
Non-varlen signals: `VTR_ERR_INVALID`.

**`vtr_writer_flush(w)`** forces a block boundary: pending strings, metadata
and hierarchy are written, buffered value changes are written as a signal
block, and buffered transactions/relations as a transaction block. Use it to
make data visible to a concurrent reader of a still-open file (readers of an
unfinished file run in recovered mode), or to bound the data lost on a crash.
It costs a block header and hurts compression if called too often. After the
first flush, metadata setters and `vtr_writer_node_attr` on earlier nodes
return `VTR_ERR_STATE`.

### 3.7 Transactions

```c
int vtr_writer_begin_tx(vtr_writer *w, uint32_t generator, uint64_t time, uint64_t *tx_out);
int vtr_writer_set_tx_parent(vtr_writer *w, uint64_t tx, uint64_t parent);
int vtr_writer_set_tx_kind(vtr_writer *w, uint64_t tx, uint8_t kind);
int vtr_writer_tx_attr(vtr_writer *w, uint64_t tx, uint32_t key, uint8_t phase, const vtr_value *v);
int vtr_writer_tx_event(vtr_writer *w, uint64_t tx, uint64_t time, uint32_t name, size_t n, const uint32_t *keys, const vtr_value *values);
int vtr_writer_tx_stage_begin(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t time);
int vtr_writer_tx_stage_end(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t time);
int vtr_writer_tx_stage(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t begin, uint64_t end, size_t n, const uint32_t *keys, const vtr_value *values);
int vtr_writer_tx_stage_attr(vtr_writer *w, uint64_t tx, uint32_t key, const vtr_value *v);
int vtr_writer_end_tx(vtr_writer *w, uint64_t tx, uint64_t time, uint8_t status);
int vtr_writer_relate(vtr_writer *w, uint32_t kind, uint64_t from, uint64_t to, size_t n, const uint32_t *keys, const vtr_value *values);
```

A transaction is a time interval `[begin, end]` on a generator, with a
status, a kind, an optional parent, typed attributes (each tagged with the
phase it was recorded in), timestamped events and named stages on lanes. It
is *open* between `begin_tx` and `end_tx`; all `tx_*` functions require an
open transaction and return `VTR_ERR_INVALID` ("transaction N is not open")
otherwise. Any number of transactions may be open at once, in any nesting or
overlap; they need not end in begin order. Transaction times are explicit
`uint64_t` parameters in file time units and are not checked against the
signal time (`set_time`); a transaction's own `end` is, however, clamped to
be no earlier than its `begin`.

All `name`, `key`, `lane` and `kind` parameters are **string ids** from
`vtr_writer_intern`. Attribute lists are passed as parallel arrays
`keys[n]` / `values[n]` (`n == 0` with NULL pointers is fine); values are
copied.

**`vtr_writer_begin_tx(w, generator, time, &tx)`** opens a transaction on
`generator` (a generator node id; `VTR_ERR_INVALID` if it is not a valid node
id) beginning at `time` and stores the new id in `*tx_out` (nullable). Ids
start at 1 and increase by one per call; they are unique in the file.

**`vtr_writer_set_tx_parent(w, tx, parent)`** records `parent` as the
structural parent of `tx` (nesting, e.g. an OpenTelemetry parent span or an
FTR parent transaction). `parent` is not validated and may be a closed
transaction. Only the last call counts.

**`vtr_writer_set_tx_kind(w, tx, kind)`** sets the OpenTelemetry span kind:
0 unspecified (default), 1 internal, 2 server, 3 client, 4 producer,
5 consumer. Other values are stored as 0.

**`vtr_writer_tx_attr(w, tx, key, phase, v)`** appends an attribute.
`phase` tells when it was recorded (FTR/SCV semantics): 0 = begin,
1 = record (during the transaction), 2 = end; other values are stored as 1.
Keys may repeat; attributes are returned in insertion order.

**`vtr_writer_tx_event(w, tx, time, name, n, keys, values)`** appends a
timestamped point event (OpenTelemetry span event, Konata label/per-cycle
detail) with `n` attributes.

**`vtr_writer_tx_stage_begin(w, tx, name, lane, time)`** opens a stage named
`name` on `lane` at `time` (Konata pipeline stage; the lane is any string,
e.g. `"0"`). **If a stage is still open on the same lane it is closed at
`max(time, its begin)` first**, so a pipeline can be recorded with one
`stage_begin` per stage. Stages on different lanes overlap freely.

**`vtr_writer_tx_stage_end(w, tx, name, lane, time)`** closes the most
recently begun *open* stage with exactly this `name` and `lane` at
`max(time, its begin)`. Returns `VTR_ERR_NOT_FOUND` (without setting
`vtr_last_error`) when no such stage is open.

**`vtr_writer_tx_stage(w, tx, name, lane, begin, end, n, keys, values)`**
records a complete stage in one call with its attributes (phase record).
`end` is clamped to `>= begin`. It does not interact with open stages.

**`vtr_writer_tx_stage_attr(w, tx, key, v)`** appends an attribute to the
**most recently begun or recorded stage** of `tx` (whether or not it is still
open). `VTR_ERR_STATE` ("transaction has no stage") if the transaction has no
stages yet.

**`vtr_writer_end_tx(w, tx, time, status)`** closes the transaction at
`max(time, begin)` with `status`:

| `status` | Meaning |
|----------|---------|
| 0 | unset — ended normally without a status (FTR, Konata retire) |
| 1 | ok — explicit success (OpenTelemetry `Ok`) |
| 2 | error — explicit failure (OpenTelemetry `Error`) |
| 3 | aborted — squashed / flushed (Konata flush) |
| 4 | open — reserved for the writer: assigned by `vtr_writer_close` to transactions never ended |

Other values are stored as 0. Stages still open on any lane are closed at the
transaction's end time. After `end_tx` the id is no longer open and any
further `tx_*` call with it returns `VTR_ERR_INVALID`; the id remains valid
as a parent or relation endpoint. The transaction row is appended to the
transaction block buffer, which is written when it reaches
`tx_block_bytes`. Transactions still open at `vtr_writer_close` are written
with status 4 and end time `max(current writer time, begin)`.

**`vtr_writer_relate(w, kind, from, to, n, keys, values)`** records a
directed relation of kind `kind` (a string id, e.g. `"depends_on"`,
`"link"`, `"parent"`) from transaction `from` to transaction `to`, with `n`
attributes. Neither endpoint is validated: they may be open, closed, not yet
begun, or **0 / any foreign id for targets outside this file** (e.g. an
OpenTelemetry link to a span in another trace — put the foreign identity in
the attributes). Relations are stored independently of the transactions and
are queried by either endpoint (section 4.15).

---

### 3.8 Logs

Log records (see `docs/LOGGING.md`, `SPEC.md` section 8) are messages of
*log sites*: a generator of a `LOG` stream that declares the format string,
severity, source location and argument types once; each message stores the
time and the argument values.

```c
uint32_t vtr_writer_add_log_stream(vtr_writer *w, uint32_t parent, const char *name);
uint32_t vtr_writer_add_log_site(vtr_writer *w, uint32_t stream, uint8_t severity, const char *fmt,
                                 const char *file, uint32_t line, const char *func,
                                 size_t n_args, const uint8_t *arg_types, const char *const *names);
uint32_t vtr_writer_log_site_node(const vtr_writer *w, uint32_t site);
int      vtr_writer_log(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent, size_t n, const vtr_value *args, uint64_t *id_out);
int      vtr_writer_log_raw(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent, const uint8_t *args, size_t len, uint64_t *id_out);
```

* `vtr_writer_add_log_stream(w, parent, name)`: `vtr_writer_add_stream` with
  kind `"LOG"`; `parent` may be `VTR_NONE`.
* `vtr_writer_add_log_site`: registers a call site and returns a dense
  **site id** (not a node id; `vtr_writer_log_site_node` gives the generator),
  or `VTR_NONE` on error (`vtr_last_error`). `severity`: 0 trace, 1 debug,
  2 info, 3 warn, 4 error, 5 fatal (other values rank by number).
  `arg_types` holds `n_args` value tags out of `VTR_VAL_BOOL`, `I64`, `U64`,
  `F64`, `STR` (an id from `vtr_writer_intern`), `BYTES`, `TIME`, `POINTER`,
  `TEXT`. `file`, `func` and `names` (argument names, `n_args` strings) may
  be NULL; unnamed arguments are called `"0"`, `"1"`, ... Register each site
  once and keep the id.
* `vtr_writer_log`: records a message; `args` are `n` values whose tags must
  equal the site's declared types in order (`VTR_ERR_INVALID` otherwise;
  `TEXT`/`BYTES` use `data`/`len`, `STR` uses `str_id`). `parent` is the
  transaction id the message belongs to, or 0. `id_out` (nullable) receives
  the record's transaction id (log records and transactions share the id
  space). `time` is independent of `vtr_writer_set_time`.
* `vtr_writer_log_raw`: the same with the values already in row encoding
  (bool: 1 byte; I64: zig-zag LEB128; U64/TIME/POINTER: LEB128; F64: 8 bytes
  little-endian; STR: LEB128 string id; TEXT/BYTES: LEB128 length then the
  bytes), in declaration order. The caller guarantees the match with the
  site; a mismatch is reported by `vtr_writer_flush`/`vtr_writer_close` as
  `VTR_ERR_CORRUPT`. This is the entry point `vtr_log.hpp` uses.

**C++**: `include/vtr_log.hpp` (header-only, C++17) wraps these into
`vtr::LogStream` and `VTR_LOG(stream, severity, time, "fmt {}", args...)`
(plus `VTR_LOG_INFO` etc.): the first execution of a statement registers the
site with `__FILE__`, `__LINE__`, `__func__` and the argument types deduced
from the C++ types, later executions encode the arguments on the stack and
call `vtr_writer_log_raw`. The placeholder count is checked against the
argument count at compile time. See section 7 and `docs/LOGGING.md`.

## 4. Reader API

Reader functions take a `const vtr_reader *r`. A NULL `r` yields
`VTR_ERR_NULL` from status functions, `0`/`VTR_NONE`/`NULL` from the others.

### 4.1 Open and close

```c
vtr_reader *vtr_reader_open(const char *path);
void        vtr_reader_close(vtr_reader *r);
```

**`vtr_reader_open(path)`** memory-maps the file and parses its directory,
metadata, string table and hierarchy; value changes and transactions are
decoded lazily, one compressed group of one block at a time, with an internal
cache. Returns NULL with `vtr_last_error()` set on failure (I/O, not a VTR
file, structural corruption, newer major version, checksum mismatch in the
eagerly-read sections). A file without a directory (writer crashed or still
running) is opened in recovered mode by scanning the sections
(`vtr_meta.recovered == 1`); the file must not be modified while a reader has
it open.

**`vtr_reader_close(r)`** unmaps and frees the reader. NULL is ignored. All
pointers obtained from the reader become invalid (section 2.4);
`vtr_signal_data` objects survive.

### 4.2 Metadata

```c
typedef struct vtr_meta {
    int8_t   timescale;
    int64_t  time_zero;
    uint8_t  file_type;
    uint32_t group_size;
    int      has_time_range;
    uint64_t time_start, time_end;
    uint16_t version_major, version_minor;
    int      recovered;
    uint32_t signal_count, node_count, string_count, signal_block_count, tx_block_count;
    uint64_t tx_count, relation_count;
    uint32_t blackout_count, file_attr_count;
} vtr_meta;

int         vtr_reader_meta(const vtr_reader *r, vtr_meta *out);
const char *vtr_reader_meta_string(const vtr_reader *r, int which, size_t *len_out);
int         vtr_reader_file_attr(const vtr_reader *r, uint32_t i, uint32_t *key_out, vtr_value *v_out);
```

**`vtr_reader_meta(r, &m)`** fills the struct:

| Field | Meaning |
|-------|---------|
| `timescale` | Time unit exponent: times are in `10^timescale` seconds. |
| `time_zero` | Display offset (`set_time_zero`). |
| `file_type` | 0 verilog, 1 vhdl, 2 mixed, 3 systemc, 4 architectural, 5 software, 255 other. |
| `group_size` | Signals per value-change group (writer option, power of two). |
| `has_time_range`, `time_start`, `time_end` | Span covered by signal blocks and transaction blocks (union of their ranges). `has_time_range == 0` (and both times 0) when the file contains neither value changes nor transactions. |
| `version_major`, `version_minor` | Format version from the file header. |
| `recovered` | 1 when the directory was rebuilt by scanning (writer did not close the file). |
| `signal_count`, `node_count`, `string_count` | Sizes of the id spaces (valid ids are `0 .. count-1`). |
| `signal_block_count`, `tx_block_count` | Number of signal / transaction blocks. |
| `tx_count`, `relation_count` | Totals from the transaction block headers. |
| `blackout_count` | Number of dump on/off marks (section 4.13). |
| `file_attr_count` | Number of file-level attributes. |

**`vtr_reader_meta_string(r, which, &len)`** returns the writer name
(`which == 0`), date (1) or comment (2, also for any other value) as a
pointer + length (not NUL-terminated, valid while the reader lives). NULL only
for a NULL reader. `len_out` may be NULL.

**`vtr_reader_file_attr(r, i, &key, &v)`** returns file attribute `i`
(`0 .. file_attr_count-1`) as a key string id and a `vtr_value`;
`VTR_ERR_NOT_FOUND` past the end. Out pointers are nullable.

### 4.3 Strings

```c
const char *vtr_reader_str(const vtr_reader *r, uint32_t id, size_t *len_out);
```

Returns the interned string `id` as pointer + length (not NUL-terminated,
valid while the reader lives). An id outside `0 .. string_count-1` returns a
pointer to an empty string with length 0. Every name, key, lane, relation
kind, `VTR_VAL_STR` and `VTR_VAL_ENUM` name in the reader API is such an id.

### 4.4 Nodes

```c
typedef struct vtr_node_info {
    uint8_t  kind;         /* 1 scope 2 var 3 stream 4 generator 5 enum table */
    uint32_t parent;       /* VTR_NONE for roots */
    uint32_t name;         /* string id */
    uint16_t type_code;    /* scope type / var type */
    uint8_t  direction;
    uint32_t signal;       /* vars only, else VTR_NONE */
    int      is_alias;
    uint32_t aux_str;      /* scope: component; stream: kind */
    uint32_t attr_count, entry_count, child_count;
} vtr_node_info;

uint32_t vtr_reader_node_count(const vtr_reader *r);
int      vtr_reader_node(const vtr_reader *r, uint32_t id, vtr_node_info *out);
int      vtr_reader_node_attr(const vtr_reader *r, uint32_t id, uint32_t i, uint32_t *key_out, vtr_value *v_out);
int      vtr_reader_enum_entry(const vtr_reader *r, uint32_t id, uint32_t i, uint32_t *literal_out, uint32_t *value_out);
size_t   vtr_reader_children(const vtr_reader *r, uint32_t id, uint32_t *out, size_t cap);
```

**`vtr_reader_node_count(r)`** is the number of nodes (same as
`vtr_meta.node_count`); node ids are `0 .. count-1` in declaration order.

**`vtr_reader_node(r, id, &info)`** describes one node; `VTR_ERR_NOT_FOUND`
for an id past the end. Field meaning by `kind`:

| `kind` | Node | `type_code` | `direction` | `signal` | `is_alias` | `aux_str` | `entry_count` |
|--------|------|-------------|-------------|----------|------------|-----------|---------------|
| 1 | scope | scope type (section 3.4 table) | 0 | `VTR_NONE` | 0 | component string id | 0 |
| 2 | var | var type (section 3.4 table) | 0..5 | signal id | 1 if declared with `add_alias` (does not own the signal) | 0 | 0 |
| 3 | stream | 0 | 0 | `VTR_NONE` | 0 | stream kind string id | 0 |
| 4 | generator | 0 | 0 | `VTR_NONE` | 0 | 0 | 0 |
| 5 | enum table | 0 | 0 | `VTR_NONE` | 0 | 0 | number of `(literal, value)` entries |

`parent` is the parent node id or `VTR_NONE` for roots, `name` a string id,
`attr_count` the number of attributes (`vtr_reader_node_attr`), `child_count`
the number of direct children (`vtr_reader_children`).

**`vtr_reader_node_attr(r, id, i, &key, &v)`** returns attribute `i` of node
`id` in insertion order; `VTR_ERR_NOT_FOUND` if either index is out of range.

**`vtr_reader_enum_entry(r, id, i, &literal, &value)`** returns entry `i` of
enum table `id` as two string ids. `VTR_ERR_INVALID` if `id` is not an enum
table; `VTR_ERR_NOT_FOUND` if `id` or `i` is out of range.

**`vtr_reader_children(r, id, out, cap)`** copies up to `cap` child ids of
node `id` — or of the forest roots when `id == VTR_NONE` — into `out`, in
declaration order, and returns the **total** number of children regardless of
`cap`. Call with `out == NULL, cap == 0` to count, then with a buffer; or
allocate `child_count` from `vtr_reader_node`. An unknown `id` returns 0.

### 4.5 Signals

```c
uint32_t vtr_reader_signal_count(const vtr_reader *r);
int      vtr_reader_signal_kind(const vtr_reader *r, uint32_t sig, uint8_t *kind_out, uint32_t *width_out, uint8_t *states_out);
uint32_t vtr_reader_signal_var(const vtr_reader *r, uint32_t sig);
```

**`vtr_reader_signal_count(r)`** is the number of signals; ids are
`0 .. count-1`.

**`vtr_reader_signal_kind(r, sig, &kind, &width, &states)`** reports how a
signal's values are encoded (out pointers nullable), `VTR_ERR_NOT_FOUND` for
an unknown id:

| `kind` | Signal | `width` | `states` |
|--------|--------|---------|----------|
| 0 | bit vector | bits | 2, 4 or 9 (declared packing) |
| 1 | real | 64 | 0 |
| 2 | variable length | 0 | 0 |

**`vtr_reader_signal_var(r, sig)`** returns the node id of the variable that
*declared* the signal (the first non-alias var), or `VTR_NONE`.

### 4.6 Lookup by path

```c
int vtr_reader_find_signal(const vtr_reader *r, const char *path, char sep, uint32_t *sig_out);
int vtr_reader_find_node(const vtr_reader *r, const char *path, char sep, uint32_t *node_out);
```

Both split `path` at every `sep` character (e.g. `'.'` or `'/'`) and walk
from the roots, matching each component exactly against the names of the
children at that level (declaration order, first match wins; names are
compared as UTF-8 byte strings). `vtr_reader_find_node` accepts any node kind
(scope, stream, generator, ...). `vtr_reader_find_signal` additionally
requires the final node to be a variable (alias or not) and returns its
signal id. Both return `VTR_ERR_NOT_FOUND` on a miss and `VTR_ERR_NULL` for a
NULL path. Each level is a linear scan; cache the ids rather than looking up
inside hot loops. Names containing `sep` cannot be addressed this way — walk
`vtr_reader_children` instead.

### 4.7 The `vtr_signal_value` struct

```c
typedef struct vtr_signal_value {
    uint8_t  kind;    /* 0 bits 1 real 2 varlen */
    uint8_t  states;  /* packing of data: 2, 4 or 9 */
    uint32_t width;
    double   real;
    const uint8_t *data;
    size_t   len;
} vtr_signal_value;
```

A borrowed view of one signal value, produced by point queries, change
iteration and `vtr_signal_data`. `data` points into the owning object
(section 2.4).

| `kind` | Fields | Meaning |
|--------|--------|---------|
| 0 | `width`, `states`, `data`, `len` | Packed bit vector. **`states` is the packing actually used for this value**, which is 2 whenever the value was stored compactly (all bits 0/1), even on a signal declared with 4 or 9 states. `len` is the packed length. |
| 1 | `real` | IEEE double (`width` is 64, `data` NULL). |
| 2 | `data`, `len` | Raw bytes of a variable-length value (no terminator). |

Values from `vtr_signal_data` are the exception: they are always in the
signal's *declared* packing (`states` equals the declared states).

### 4.8 Data packing and rendering

Bit vectors are packed **LSB first**: bit `i` of the vector (bit 0 = least
significant = the *last* character of a VCD string) is stored at

| `states` | Bits per bit | Byte index | Bit shift within the byte | Packed length |
|----------|--------------|------------|---------------------------|---------------|
| 2 | 1 | `i / 8` | `i % 8`       | `ceil(width / 8)` |
| 4 | 2 | `i / 4` | `(i % 4) * 2` | `ceil(width / 4)` |
| 9 | 4 | `i / 2` | `(i % 2) * 4` | `ceil(width / 2)` |

Unused high bits of the last byte are zero. The stored code per bit is the
logic code of section 3.6 (`0 1 X Z U W L H -` = 0..8). The same layout is
used by `vtr_writer_emit_packed`, by `VTR_VAL_BITS/LOGIC/LOGIC9` attribute
values and by every `vtr_signal_value` with `kind == 0`.

Extracting one bit and rendering a value MSB-first in C:

```c
static unsigned vtr_bit_code(const vtr_signal_value *v, uint32_t i) {
    switch (v->states) {
    case 2:  return (v->data[i >> 3] >> (i & 7)) & 1u;
    case 4:  return (v->data[i >> 2] >> ((i & 3) * 2)) & 3u;
    default: return (v->data[i >> 1] >> ((i & 1) * 4)) & 15u;
    }
}

/* out must hold width + 1 bytes; produces e.g. "0101xz" */
static void vtr_render_bits(const vtr_signal_value *v, char *out) {
    static const char ascii[] = "01xzuwlh-";
    for (uint32_t i = 0; i < v->width; i++) {
        unsigned c = vtr_bit_code(v, v->width - 1 - i);
        out[i] = ascii[c > 8 ? 8 : c];
    }
    out[v->width] = 0;
}
```

A 2-state value of at most 64 bits converts to an integer by OR-ing
`(uint64_t)vtr_bit_code(v, i) << i` for each bit; when `states == 2` and
`width <= 64` you can also read the bytes as a little-endian integer directly
(`memcpy` of `len` bytes into a zeroed `uint64_t`).

### 4.9 Point queries: value buffers

```c
typedef struct vtr_value_buf vtr_value_buf;
vtr_value_buf *vtr_value_buf_new(void);
void           vtr_value_buf_free(vtr_value_buf *b);
int            vtr_value_buf_get(const vtr_value_buf *b, vtr_signal_value *out);
const char    *vtr_value_buf_ascii(vtr_value_buf *b);

int vtr_reader_value_at(const vtr_reader *r, uint32_t sig, uint64_t time, vtr_value_buf *buf);
```

A `vtr_value_buf` owns one decoded value; allocate one per thread (or per
query loop) and reuse it.

**`vtr_reader_value_at(r, sig, time, buf)`** stores in `buf` the value of
`sig` at `time`: the last change at or before `time`, or the signal's initial
value (all 0 for 2-state, all X for 4/9-state, 0.0, empty) if it has not
changed yet. Errors: `VTR_ERR_INVALID` (unknown signal), decoding errors
(`VTR_ERR_CORRUPT`, `VTR_ERR_CODEC`, `VTR_ERR_CHECKSUM`, `VTR_ERR_IO`); on
error the buffer keeps its previous content. The query decompresses at most
one group piece of one block, so it is cheap for random access, but a full
sweep of a signal should use `vtr_reader_changes` or
`vtr_reader_load_signal`.

**`vtr_value_buf_get(b, &v)`** exposes the buffered value as a
`vtr_signal_value` (valid until the buffer is reused or freed).

**`vtr_value_buf_ascii(b)`** renders the buffered value as a NUL-terminated
string, valid until the next call on the same buffer: bit vectors as a
`0/1/x/z/u/w/l/h/-` string MSB first (`width` characters), reals in
shortest round-trip decimal (`2.5`, `0`, `NaN`, `inf`), variable-length
values as their bytes interpreted as UTF-8 (invalid sequences replaced).
Before any `vtr_reader_value_at`, a fresh buffer renders as `0`.

### 4.10 Change iteration

```c
typedef int (*vtr_change_cb)(void *user, uint64_t time, uint32_t sig, const vtr_signal_value *value);
int vtr_reader_changes(const vtr_reader *r, uint32_t sig, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);
int vtr_reader_for_each_change(const vtr_reader *r, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);
```

The callback receives the change time, the signal id and a borrowed value
that is valid only during the call. **Return 0 to continue, non-zero to
stop**; stopping is not an error (the query returns `VTR_OK`). A NULL `cb`
returns `VTR_ERR_NULL`.

**`vtr_reader_changes(r, sig, t0, t1, cb, user)`** calls `cb` for every
change of `sig` with `t0 <= time <= t1` (both inclusive), in time order,
including several changes at the same time in emission order. The initial
value is not reported; combine with `vtr_reader_value_at(sig, t0)` to know
the value at the window start. The changes of the window are decoded into a
temporary list before the callbacks run, so memory is proportional to the
number of changes in the window. `VTR_ERR_INVALID` for an unknown signal.

**`vtr_reader_for_each_change(r, t0, t1, cb, user)`** streams every change
of **every** signal with `t0 <= time <= t1`, in time order (a VCD-style
dump); several changes of one signal at the same time step keep their order,
the order of different signals within a step is deterministic but
unspecified. This decodes whole blocks at once and is the fastest way to
convert or scan a full trace; memory does not grow with the number of
changes. A non-zero callback return suppresses all further callbacks; the
function may still finish scanning the current block internally before
returning `VTR_OK`.

### 4.11 Whole-signal loads

```c
typedef struct vtr_signal_data vtr_signal_data;
vtr_signal_data *vtr_reader_load_signal(const vtr_reader *r, uint32_t sig);
int              vtr_reader_load_signals(const vtr_reader *r, const uint32_t *sigs, size_t n, vtr_signal_data **out);
vtr_signal_data *vtr_signal_data_clone(const vtr_signal_data *d);
void             vtr_signal_data_free(vtr_signal_data *d);
size_t           vtr_signal_data_len(const vtr_signal_data *d);
const uint64_t  *vtr_signal_data_times(const vtr_signal_data *d);
int              vtr_signal_data_get(const vtr_signal_data *d, size_t i, vtr_signal_value *out);
int              vtr_signal_data_initial(const vtr_signal_data *d, vtr_signal_value *out);
size_t           vtr_signal_data_index_at(const vtr_signal_data *d, uint64_t time);
```

**`vtr_reader_load_signal(r, sig)`** decodes the complete history of one
signal into a self-contained object (NULL with `vtr_last_error()` set on an
unknown signal or a decoding error). The object is an immutable handle: it does not
reference the reader, can outlive it, and can be read concurrently. Free each
handle with `vtr_signal_data_free` (NULL is ignored). Borrowed time/value
pointers remain valid until that handle is freed; they must not be modified.

**`vtr_reader_load_signals(r, sigs, n, out)`** loads a batch into `n`
caller-allocated output slots. Each slot receives a separately owned handle in
request order. Repeated signal IDs are decoded once and share all waveform
storage. Free every returned handle separately. On error, no output slots are
modified; errors match the single-signal load, plus `VTR_ERR_NULL` for a NULL
reader or a NULL input/output array when `n > 0`. For `n == 0`, the arrays
may be NULL and the call succeeds with a valid reader. Sharing is local to
the call; separate calls do not share a persistent history cache.

**`vtr_signal_data_clone(d)`** creates another handle to the same immutable
storage without copying buffers. It returns NULL for a NULL input. The clone
remains valid after the original handle and reader are freed. Storage is
released when the last handle is freed.

* `vtr_signal_data_len(d)` — number of changes `n` (0 for a NULL `d`).
* `vtr_signal_data_times(d)` — pointer to `n` change times, non-decreasing
  (equal times are same-time updates in emission order). NULL for a NULL `d`.
* `vtr_signal_data_get(d, i, &v)` — value of change `i`; `VTR_ERR_NOT_FOUND`
  for `i >= n`. Values are in the signal's declared packing.
* `vtr_signal_data_initial(d, &v)` — value before the first change.
* `vtr_signal_data_index_at(d, t)` — index of the last change at or before
  `t`, or `SIZE_MAX` if `t` precedes the first change (then use the initial
  value). Binary search.

The value at time `t` is therefore
`i = index_at(t); i == SIZE_MAX ? initial() : get(i)`.

### 4.12 Time table

```c
const uint64_t *vtr_reader_time_table(const vtr_reader *r, size_t *len_out);
```

Returns the sorted table of every distinct time step recorded by
`vtr_writer_set_time` (plus 0 if values were emitted before the first
`set_time`), across all signal blocks, and its length in `*len_out`
(nullable). Built on first use and cached; the pointer is valid while the
reader lives. NULL with `vtr_last_error()` set if a block's time table cannot
be decoded, or for a NULL reader. Transaction times are not included.

### 4.13 Blackout (dump on/off) marks

```c
int vtr_reader_blackout(const vtr_reader *r, uint32_t i, uint64_t *time_out, int *active_out);
```

Returns mark `i` (`0 .. blackout_count-1`, in the order recorded) as a time
and a flag: `active == 1` means dumping resumed at `time`
(`vtr_writer_dump_on`), `active == 0` means dumping stopped
(`vtr_writer_dump_off`). `VTR_ERR_NOT_FOUND` past the end.

### 4.14 Transactions

```c
typedef struct vtr_tx vtr_tx;   /* valid only inside the callback */
typedef struct vtr_tx_info {
    uint64_t id;
    uint32_t generator, stream;
    uint64_t begin, end;
    uint8_t  status, kind;
    int      has_parent;
    uint64_t parent;
    uint32_t attr_count, event_count, stage_count;
} vtr_tx_info;

int vtr_tx_get(const vtr_reader *r, const vtr_tx *tx, vtr_tx_info *out);
int vtr_tx_attr(const vtr_tx *tx, uint32_t i, uint32_t *key_out, uint8_t *phase_out, vtr_value *v_out);
int vtr_tx_event(const vtr_tx *tx, uint32_t i, uint64_t *time_out, uint32_t *name_out, uint32_t *attr_count_out);
int vtr_tx_event_attr(const vtr_tx *tx, uint32_t i, uint32_t j, uint32_t *key_out, vtr_value *v_out);
int vtr_tx_stage(const vtr_tx *tx, uint32_t i, uint32_t *name_out, uint32_t *lane_out, uint64_t *begin_out, uint64_t *end_out, int *has_end_out, uint32_t *attr_count_out);
int vtr_tx_stage_attr(const vtr_tx *tx, uint32_t i, uint32_t j, uint32_t *key_out, vtr_value *v_out);

typedef int (*vtr_tx_cb)(void *user, const vtr_tx *tx);  /* return non-zero to stop */
int vtr_reader_visit_transactions(const vtr_reader *r, uint32_t generator, uint32_t stream,
                                  uint64_t t0, uint64_t t1, vtr_tx_cb cb, void *user);
int vtr_reader_transaction(const vtr_reader *r, uint64_t id, vtr_tx_cb cb, void *user);
```

Transactions are delivered through a callback that receives an opaque
`const vtr_tx *`. **The handle, and every pointer obtained from it, is valid
only until the callback returns**; copy the `vtr_tx_info` and any attribute
data you need. The accessors below take the handle, not the reader, except
`vtr_tx_get`, which needs the reader to resolve the generator's stream.

**`vtr_tx_get(r, tx, &info)`** fills the summary:

| Field | Meaning |
|-------|---------|
| `id` | Transaction id (>= 1). |
| `generator` | Generator node id. |
| `stream` | Parent stream node id of the generator (`VTR_NONE` if the generator has no parent). |
| `begin`, `end` | Time interval (`end >= begin`). |
| `status` | 0 unset, 1 ok, 2 error, 3 aborted, 4 open (never ended before the writer closed). |
| `kind` | 0 unspecified, 1 internal, 2 server, 3 client, 4 producer, 5 consumer. |
| `has_parent`, `parent` | `has_parent == 1` when `vtr_writer_set_tx_parent` was called; `parent` is then the parent id (0 otherwise). |
| `attr_count`, `event_count`, `stage_count` | Sizes for the indexed accessors. |

**`vtr_tx_attr(tx, i, &key, &phase, &v)`** — attribute `i` (insertion
order): key string id, phase (0 begin, 1 record, 2 end) and value.
**`vtr_tx_event(tx, i, &time, &name, &n)`** — event `i`: time, name string
id and its attribute count; **`vtr_tx_event_attr(tx, i, j, &key, &v)`** —
attribute `j` of event `i`. **`vtr_tx_stage(tx, i, &name, &lane, &begin, &end, &has_end, &n)`**
— stage `i` in the order stages were begun/recorded: name and lane string
ids, `begin`, `end`, and `has_end` (1 when the stage has a recorded end;
otherwise `end` is set to the transaction's end). Files produced by this
library always have `has_end == 1`, because open stages are closed at the
transaction end when it is written. **`vtr_tx_stage_attr(tx, i, j, &key, &v)`**
— attribute `j` of stage `i`. All return `VTR_ERR_NOT_FOUND` for an index
past the end and accept NULL out pointers.

**`vtr_reader_visit_transactions(r, generator, stream, t0, t1, cb, user)`**
calls `cb` for every transaction matching all given filters:

* `generator` — only this generator node, or `VTR_NONE` for any;
* `stream` — only generators whose parent is this stream node, or `VTR_NONE`;
* `t0`, `t1` — only transactions **overlapping** `[t0, t1]`
  (`end >= t0 && begin <= t1`). **`t1 == 0` disables the window** (a window
  ending exactly at time 0 is not expressible; use `t1 = 1` and filter).

Transactions are visited in file order, i.e. the order in which they were
*ended* (not begun); ids are therefore not necessarily increasing. Blocks
that cannot contain a match (by time range or generator set) are skipped
without decompression. Return non-zero from `cb` to stop; the function then
returns `VTR_OK`.

**`vtr_reader_transaction(r, id, cb, user)`** looks up one transaction by id
and calls `cb` exactly once with it (the callback's return value is
ignored). `VTR_ERR_NOT_FOUND` if no transaction has that id.

### 4.15 Relations

```c
typedef int (*vtr_relation_cb)(void *user, uint32_t kind, uint64_t from, uint64_t to,
                               uint32_t n_attrs, const uint32_t *keys, const vtr_value *values);
int vtr_reader_relations(const vtr_reader *r, uint64_t id, int direction, vtr_relation_cb cb, void *user);
```

Calls `cb` for every relation whose source is `id` (`direction == 0`) or
whose target is `id` (any other `direction`), in file order. `kind` is a
string id; `keys`/`values` (`n_attrs` each) are valid only during the
callback. Return non-zero to stop (`VTR_OK` is still returned). Relations
whose other endpoint is 0 or refers to an id not in the file are reported
like any other. Blocks are skipped by their `from`/`to` id ranges, so
lookups are cheap when ids are roughly monotonic.

---

### 4.16 Logs

```c
typedef struct vtr_log_site_info { uint32_t node, stream; uint8_t severity; uint32_t fmt; uint32_t file, func; uint32_t line; uint32_t arg_count; } vtr_log_site_info;
uint32_t vtr_reader_log_site_count(const vtr_reader *r);
uint64_t vtr_reader_log_count(const vtr_reader *r);
int      vtr_reader_log_site(const vtr_reader *r, uint32_t i, vtr_log_site_info *out);
int      vtr_reader_log_site_arg(const vtr_reader *r, uint32_t i, uint32_t j, uint8_t *type_out, uint32_t *name_out);
uint32_t vtr_reader_log_site_of(const vtr_reader *r, uint32_t generator);

typedef struct vtr_log_rec { uint64_t id, time; uint64_t parent; uint32_t site, generator, stream; uint8_t severity; uint32_t arg_count; const void *inner_; } vtr_log_rec;
typedef int (*vtr_log_cb)(void *user, const vtr_log_rec *rec);
int    vtr_reader_visit_log(const vtr_reader *r, uint32_t stream, uint32_t generator, uint8_t min_severity, uint64_t t0, uint64_t t1, vtr_log_cb cb, void *user);
int    vtr_log_rec_arg(const vtr_log_rec *rec, uint32_t i, vtr_value *v_out);
size_t vtr_log_rec_format(const vtr_reader *r, const vtr_log_rec *rec, char *buf, size_t cap);
```

* Sites are indexed `0 .. vtr_reader_log_site_count - 1` in declaration
  order; `vtr_log_site_info` gives the generator and stream nodes, the
  severity, the format string id and the source location (`file`/`func`
  string ids or `VTR_NONE`, `line` 0 when unknown); `vtr_reader_log_site_arg`
  gives argument `j`'s type tag and name id. `vtr_reader_log_site_of` maps a
  generator node to its site index (`VTR_NONE` if it is not a log site).
* `vtr_reader_visit_log` visits records in time order of their blocks and
  production order within a block; `stream` and `generator` may be
  `VTR_NONE`, `min_severity` 0 selects everything, `t1 == 0` means no time
  window. Blocks whose header shows no matching site or time are skipped
  without decoding. The callback returns non-zero to stop. The `vtr_log_rec`
  is valid only inside the callback; `parent` is 0 when the record has none.
* `vtr_log_rec_arg(rec, i, &v)` reads argument `i` as a `vtr_value`
  (`VTR_ERR_NOT_FOUND` past the end). `VTR_VAL_TEXT` and `VTR_VAL_BYTES` point
  into the reader's decoded block and stay valid while the reader is alive.
* `vtr_log_rec_format(r, rec, buf, cap)` renders the message text into `buf`
  (NUL-terminated when `cap > 0`, truncated if it does not fit) and returns
  the full length like `snprintf`; `buf` may be NULL with `cap == 0` to
  measure. `vtr_log.hpp` offers `vtr::format_log(r, rec)` returning a
  `std::string` and `vtr::for_each_log(r, stream, min_severity, t0, t1, f)`.
* `vtr_meta.log_count`, `log_block_count` and `log_site_count` report the
  totals; `tx_count` includes the log records, and
  `vtr_reader_visit_transactions` / `vtr_reader_transaction` return them as
  zero-duration transactions whose attributes are the arguments (keys =
  argument names, `TEXT` values with tag 17).

## 5. Complete examples

Both programs compile with
`cc -std=c99 -Wall -Wextra -Werror -I crates/vtr-capi/include X.c target/release/libvtr.a -lpthread -ldl -lm`.

### 5.1 Simulator-side writer

```c
/* sim_write.c: writes sim.vtr with a clock, a bus, a real and a pipeline stream. */
#include "vtr.h"
#include <stdio.h>
#include <string.h>

static int die(vtr_writer *w, const char *what, int rc) {
    fprintf(stderr, "%s: error %d: %s\n", what, rc, vtr_last_error());
    vtr_writer_close(w);            /* still finishes what it can and frees the handle */
    return 1;
}
#define TRY(call) do { int rc_ = (call); if (rc_ != VTR_OK) return die(w, #call, rc_); } while (0)

int main(void) {
    vtr_writer_options o;
    vtr_writer_options_default(&o);
    o.level = 3;                                    /* zstd level; everything else default */
    vtr_writer *w = vtr_writer_create("sim.vtr", &o);
    if (!w) { fprintf(stderr, "create: %s\n", vtr_last_error()); return 1; }

    /* Metadata: only before the first flush. */
    TRY(vtr_writer_set_timescale(w, -12));          /* picoseconds */
    TRY(vtr_writer_set_file_type(w, 3));            /* systemc */
    TRY(vtr_writer_set_writer_name(w, "mysim 1.0"));
    TRY(vtr_writer_set_date(w, "2026-09-04"));

    /* Hierarchy: scope "top" (sc_module) with clk, data, temperature. */
    uint32_t top = vtr_writer_begin_scope(w, "top", 65, "top_module");
    if (top == VTR_NONE) return die(w, "begin_scope", VTR_ERR_NULL);
    uint32_t clk_n, clk, data_n, data, temp_n, temp;
    TRY(vtr_writer_add_var(w, "clk",  16, 1, 0, 1,  4, &clk_n,  &clk));   /* wire, input, 1 bit, 4-state */
    TRY(vtr_writer_add_var(w, "data", 23, 2, 0, 32, 4, &data_n, &data));  /* logic, output, 32 bits, 4-state */
    TRY(vtr_writer_add_var(w, "temperature", 3, 0, 1, 0, 0, &temp_n, &temp)); /* real */
    vtr_value unit; memset(&unit, 0, sizeof unit);
    unit.tag = VTR_VAL_STR; unit.str_id = vtr_writer_intern(w, "celsius");
    TRY(vtr_writer_node_attr(w, temp_n, "unit", &unit));                  /* before the node is flushed */
    TRY(vtr_writer_end_scope(w));

    /* Transaction stream under "top" with one generator. */
    uint32_t stream = vtr_writer_add_stream(w, top, "cpu_pipe", "PIPELINE");
    uint32_t gen    = vtr_writer_add_generator(w, stream, "instruction");
    uint32_t k_pc   = vtr_writer_intern(w, "pc");
    uint32_t k_op   = vtr_writer_intern(w, "opcode");
    uint32_t lane0  = vtr_writer_intern(w, "0");
    uint32_t st_f   = vtr_writer_intern(w, "F");
    uint32_t st_d   = vtr_writer_intern(w, "D");
    uint32_t st_x   = vtr_writer_intern(w, "X");
    uint32_t r_dep  = vtr_writer_intern(w, "depends_on");
    uint32_t v_add  = vtr_writer_intern(w, "add");

    uint64_t prev_tx = 0;
    for (uint64_t cycle = 0; cycle < 100; cycle++) {
        uint64_t t = cycle * 1000;                  /* 1 ns clock period at ps resolution */
        TRY(vtr_writer_set_time(w, t));             /* non-decreasing */
        TRY(vtr_writer_emit_bit(w, clk, 1));
        TRY(vtr_writer_emit_u64(w, data, cycle * 0x11));            /* deduplicated if unchanged */
        if (cycle % 10 == 0) TRY(vtr_writer_emit_real(w, temp, 20.0 + (double)cycle / 10.0));
        if (cycle == 50) TRY(vtr_writer_emit_logic_str(w, data, "zzzz", SIZE_MAX)); /* left-extends to 32 Z */
        TRY(vtr_writer_set_time(w, t + 500));
        TRY(vtr_writer_emit_bit(w, clk, 0));

        if (cycle % 4 == 0) {                       /* one instruction every 4 cycles */
            uint64_t tx;
            TRY(vtr_writer_begin_tx(w, gen, t, &tx));
            vtr_value pc; memset(&pc, 0, sizeof pc);
            pc.tag = VTR_VAL_U64; pc.u = 0x8000 + cycle * 4;
            TRY(vtr_writer_tx_attr(w, tx, k_pc, 0, &pc));            /* phase 0 = begin */
            TRY(vtr_writer_tx_stage_begin(w, tx, st_f, lane0, t));
            TRY(vtr_writer_tx_stage_begin(w, tx, st_d, lane0, t + 1000)); /* closes F on lane 0 */
            TRY(vtr_writer_tx_stage_begin(w, tx, st_x, lane0, t + 2000)); /* closes D */
            TRY(vtr_writer_tx_stage_end(w, tx, st_x, lane0, t + 3000));
            vtr_value op; memset(&op, 0, sizeof op);
            op.tag = VTR_VAL_STR; op.str_id = v_add;
            TRY(vtr_writer_tx_attr(w, tx, k_op, 2, &op));            /* phase 2 = end */
            if (prev_tx) TRY(vtr_writer_relate(w, r_dep, tx, prev_tx, 0, NULL, NULL));
            TRY(vtr_writer_end_tx(w, tx, t + 3000, 1));              /* status 1 = ok */
            prev_tx = tx;
        }
    }

    int rc = vtr_writer_close(w);                   /* frees w even on error */
    if (rc != VTR_OK) { fprintf(stderr, "close: error %d: %s\n", rc, vtr_last_error()); return 1; }
    return 0;
}
```

### 5.2 Reader

```c
/* sim_read.c: opens a file, queries one signal, iterates a window, lists a stream. */
#include "vtr.h"
#include <stdio.h>
#include <string.h>

static void print_str(const vtr_reader *r, uint32_t id) {
    size_t n;
    const char *s = vtr_reader_str(r, id, &n);      /* not NUL-terminated */
    printf("%.*s", (int)n, s);
}

static void print_value(const vtr_reader *r, const vtr_value *v) {
    switch (v->tag) {
    case VTR_VAL_I64:  printf("%lld", (long long)v->i); break;
    case VTR_VAL_U64:
    case VTR_VAL_TIME: printf("%llu", (unsigned long long)v->u); break;
    case VTR_VAL_F64:  printf("%g", v->f); break;
    case VTR_VAL_STR:  print_str(r, v->str_id); break;
    case VTR_VAL_BOOL: printf(v->b ? "true" : "false"); break;
    default:           printf("<tag %u>", v->tag); break;
    }
}

/* vtr_change_cb: called for every change; return non-zero to stop. */
static int on_change(void *user, uint64_t time, uint32_t sig, const vtr_signal_value *v) {
    (void)sig;
    int *count = (int *)user;
    printf("  t=%llu ", (unsigned long long)time);
    if (v->kind == 0) {                             /* bits: render MSB first */
        for (uint32_t i = v->width; i-- > 0;) {
            unsigned code;
            switch (v->states) {
            case 2:  code = (v->data[i >> 3] >> (i & 7)) & 1u; break;
            case 4:  code = (v->data[i >> 2] >> ((i & 3) * 2)) & 3u; break;
            default: code = (v->data[i >> 1] >> ((i & 1) * 4)) & 15u; break;
            }
            putchar("01xzuwlh-"[code > 8 ? 8 : code]);
        }
        putchar('\n');
    } else if (v->kind == 1) {
        printf("%g\n", v->real);
    } else {
        printf("\"%.*s\"\n", (int)v->len, (const char *)v->data);
    }
    return ++*count >= 10;                          /* stop after 10 changes */
}

/* vtr_tx_cb: the vtr_tx handle is valid only inside this function. */
static int on_tx(void *user, const vtr_tx *tx) {
    const vtr_reader *r = (const vtr_reader *)user;
    vtr_tx_info info;
    if (vtr_tx_get(r, tx, &info) != VTR_OK) return 1;
    printf("tx %llu [%llu, %llu] status=%u attrs=%u stages=%u\n", (unsigned long long)info.id,
           (unsigned long long)info.begin, (unsigned long long)info.end, info.status, info.attr_count, info.stage_count);
    for (uint32_t i = 0; i < info.attr_count; i++) {
        uint32_t key; uint8_t phase; vtr_value v;
        if (vtr_tx_attr(tx, i, &key, &phase, &v) != VTR_OK) break;
        printf("    attr "); print_str(r, key); printf(" (phase %u) = ", phase); print_value(r, &v); putchar('\n');
    }
    for (uint32_t i = 0; i < info.stage_count; i++) {
        uint32_t name, lane, n_attrs; uint64_t b, e; int has_end;
        if (vtr_tx_stage(tx, i, &name, &lane, &b, &e, &has_end, &n_attrs) != VTR_OK) break;
        printf("    stage "); print_str(r, name); printf(" lane "); print_str(r, lane);
        printf(" [%llu, %llu]\n", (unsigned long long)b, (unsigned long long)e);
    }
    return 0;                                       /* continue */
}

static int on_relation(void *user, uint32_t kind, uint64_t from, uint64_t to, uint32_t n, const uint32_t *keys, const vtr_value *values) {
    (void)keys; (void)values;
    printf("  relation "); print_str((const vtr_reader *)user, kind);
    printf(" %llu -> %llu (%u attrs)\n", (unsigned long long)from, (unsigned long long)to, n);
    return 0;
}

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "sim.vtr";
    vtr_reader *r = vtr_reader_open(path);
    if (!r) { fprintf(stderr, "open %s: %s\n", path, vtr_last_error()); return 1; }

    vtr_meta m;
    if (vtr_reader_meta(r, &m) != VTR_OK) { vtr_reader_close(r); return 1; }
    printf("format %u.%u, timescale 1e%d s, %u signals, %u nodes, %llu transactions, time [%llu, %llu]%s\n",
           m.version_major, m.version_minor, m.timescale, m.signal_count, m.node_count,
           (unsigned long long)m.tx_count, (unsigned long long)m.time_start, (unsigned long long)m.time_end,
           m.recovered ? " (recovered)" : "");

    /* Find a signal by hierarchical path. */
    uint32_t sig;
    if (vtr_reader_find_signal(r, "top.data", '.', &sig) != VTR_OK) {
        fprintf(stderr, "top.data: not found\n");
        vtr_reader_close(r);
        return 1;
    }
    uint8_t kind, states; uint32_t width;
    vtr_reader_signal_kind(r, sig, &kind, &width, &states);
    printf("top.data: signal %u kind %u width %u states %u\n", sig, kind, width, states);

    /* Value at a time. */
    vtr_value_buf *buf = vtr_value_buf_new();
    if (vtr_reader_value_at(r, sig, 25000, buf) == VTR_OK)
        printf("top.data @ 25000 = %s\n", vtr_value_buf_ascii(buf));
    else
        fprintf(stderr, "value_at: %s\n", vtr_last_error());
    vtr_value_buf_free(buf);

    /* Changes in a window. */
    printf("changes of top.data in [0, 10000]:\n");
    int n = 0;
    if (vtr_reader_changes(r, sig, 0, 10000, on_change, &n) != VTR_OK)
        fprintf(stderr, "changes: %s\n", vtr_last_error());

    /* Transactions of one stream, then the relations of the first one. */
    uint32_t stream;
    if (vtr_reader_find_node(r, "top.cpu_pipe", '.', &stream) == VTR_OK) {
        printf("transactions of top.cpu_pipe:\n");
        if (vtr_reader_visit_transactions(r, VTR_NONE, stream, 0, 0, on_tx, r) != VTR_OK)
            fprintf(stderr, "visit: %s\n", vtr_last_error());
        printf("relations into tx 1:\n");
        vtr_reader_relations(r, 1, 1, on_relation, r);
    }

    vtr_reader_close(r);
    return 0;
}
```

Running `sim_write` then `sim_read` prints, among other things,
`top.data @ 25000 = 00000000000000000000000110101001` (cycle 25, `25 * 0x11`),
the first ten changes of `top.data`, each `instruction` transaction with its
`pc` and `opcode` attributes and F/D/X stages, and the `depends_on` relation
from transaction 2 into transaction 1.

---

## 6. Error handling and mapping to the Rust API

### 6.1 Error handling guidance

* **Check every return.** Status functions return `VTR_OK` or a code from
  section 2.1; handle-returning functions return NULL; id-returning writer
  functions (`begin_scope`, `add_enum_table`, `add_stream`, `add_generator`)
  return `VTR_NONE` and `vtr_writer_intern` returns 0 only for NULL/invalid
  arguments — those cannot fail otherwise, so a wrapper can `assert` them.
* **Read the message after the code.** `vtr_last_error()` is thread-local,
  never NULL, and holds the text of the last library error on this thread; it
  is not cleared on success and not written for index-based
  `VTR_ERR_NOT_FOUND` misses, so use it for diagnostics only. Copy it before
  making another call if you need to keep it.
* **Writer errors are mostly caller bugs.** `VTR_ERR_INVALID` and
  `VTR_ERR_STATE` from the writer mean a wrong id, a time going backwards, a
  value that does not fit the signal, or a call in the wrong phase; they leave
  the writer usable and do not corrupt the file. Treat them as assertions
  during bring-up. `VTR_ERR_IO` / `VTR_ERR_CODEC` / `VTR_ERR_STATE
  "background writer failed"` mean the file can no longer be completed:
  report, call `vtr_writer_close` (to free the handle) and give up on the
  file.
* **Always close the writer**, and check the close status: with background
  encoding, I/O errors of earlier blocks may surface only here. Close is the
  only way to free the handle and the only way to get a file with a
  directory.
* **Reader errors at open** are about the file (`VTR_ERR_CORRUPT`,
  `VTR_ERR_VERSION`, `VTR_ERR_IO`, `VTR_ERR_CHECKSUM`). Reader errors during
  queries are either `VTR_ERR_NOT_FOUND` (a miss, usually not an error for
  the caller) or a decoding failure of a block that was fine to open lazily;
  the reader remains usable for other blocks.
* **In callbacks**, return non-zero to stop; do not call back into the same
  writer, and do not keep `vtr_tx`/value pointers.
* **Convert to exceptions at the boundary** in C++ (section 7); never let an
  exception propagate through a callback into the library.

### 6.2 Mapping to the Rust API

Type mapping: `uint32_t node` = `NodeId`, `uint32_t sig` = `SignalId`,
`uint32_t` string id = `StrId`, `uint64_t tx` = `TxId`, `vtr_value` =
`Value`, `vtr_signal_value` = `SignalValue`, `vtr_value_buf` =
`OwnedSignalValue`, `vtr_signal_data` = `SignalData`, `vtr_tx` =
`&Transaction`, `int` status = `Result<()>` (error variant → code, see
section 2.1: `Corrupt`→4, `UnsupportedVersion`→5, `Invalid`→1, `State`→2,
`Io`→3, `Checksum`→7, `Codec`→6).

| C function | Rust |
|------------|------|
| `vtr_last_error` | `Error::to_string()` of the last error (thread-local) |
| `vtr_version` | `env!("CARGO_PKG_VERSION")` of `vtr-capi` |
| `vtr_writer_options_default` | `WriterOptions::default()` |
| `vtr_writer_create` | `Writer::create_with(path, opts)` (`Writer::create` when `opts` is NULL) |
| `vtr_writer_close` | `Writer::close()` then drop |
| `vtr_writer_set_timescale` | `Writer::set_timescale` |
| `vtr_writer_set_time_zero` | `Writer::set_time_zero` |
| `vtr_writer_set_file_type` | `Writer::set_file_type(FileType::from_u8)` |
| `vtr_writer_set_writer_name` | `Writer::set_writer_name` |
| `vtr_writer_set_date` | `Writer::set_date` |
| `vtr_writer_set_comment` | `Writer::set_comment` |
| `vtr_writer_set_file_attr` | `Writer::set_file_attr` |
| `vtr_writer_intern` | `Writer::intern` |
| `vtr_writer_begin_scope` | `Writer::begin_scope(name, ScopeType::from_code, component)` |
| `vtr_writer_end_scope` | `Writer::end_scope` |
| `vtr_writer_add_var` | `Writer::add_var(name, VarType::from_code, Direction::from_u8, SignalKind::{Bits{width,states} / Real / VarLen})` |
| `vtr_writer_add_alias` | `Writer::add_alias` |
| `vtr_writer_add_enum_table` | `Writer::add_enum_table` |
| `vtr_writer_add_stream` | `Writer::add_stream(Option<NodeId>, name, kind)` |
| `vtr_writer_add_generator` | `Writer::add_generator` |
| `vtr_writer_node_attr` | `Writer::node_attr` |
| `vtr_writer_set_time` | `Writer::set_time` |
| `vtr_writer_dump_off` / `_on` | `Writer::dump_off` / `Writer::dump_on` |
| `vtr_writer_emit_bit` | `Writer::emit_bit` |
| `vtr_writer_emit_u64` | `Writer::emit_u64` |
| `vtr_writer_emit_words` | `Writer::emit_words` |
| `vtr_writer_emit_logic_str` | `Writer::emit_logic_str` |
| `vtr_writer_emit_packed` | `Writer::emit_packed` |
| `vtr_writer_emit_real` | `Writer::emit_real` |
| `vtr_writer_emit_varlen` | `Writer::emit_varlen` |
| `vtr_writer_flush` | `Writer::flush` |
| `vtr_writer_begin_tx` | `Writer::begin_tx` |
| `vtr_writer_set_tx_parent` | `Writer::set_tx_parent` |
| `vtr_writer_set_tx_kind` | `Writer::set_tx_kind(TxKind::from_u8)` |
| `vtr_writer_tx_attr` | `Writer::tx_attr(tx, key, AttrPhase::from_u8, &value)` |
| `vtr_writer_tx_event` | `Writer::tx_event` |
| `vtr_writer_tx_stage_begin` | `Writer::tx_stage_begin` |
| `vtr_writer_tx_stage_end` | `Writer::tx_stage_end` (`Ok(false)` → `VTR_ERR_NOT_FOUND`) |
| `vtr_writer_tx_stage` | `Writer::tx_stage` |
| `vtr_writer_tx_stage_attr` | `Writer::tx_stage_attr` |
| `vtr_writer_end_tx` | `Writer::end_tx(tx, time, TxStatus::from_u8)` |
| `vtr_writer_relate` | `Writer::relate` |
| `vtr_reader_open` | `Reader::open` |
| `vtr_reader_close` | drop `Reader` |
| `vtr_reader_meta` | `Reader::meta`, `Reader::time_range`, `Reader::version`, `Reader::recovered`, `Reader::signal_count`, `Reader::hierarchy().len()`, `Reader::strings().len()`, `Reader::block_count`, `Reader::tx_block_count`, `Reader::tx_counts`, `Reader::blackout().len()` |
| `vtr_reader_meta_string` | `Reader::meta().writer / .date / .comment` |
| `vtr_reader_file_attr` | `Reader::meta().attrs[i]` |
| `vtr_reader_str` | `Reader::str` |
| `vtr_reader_node_count` | `Reader::hierarchy().len()` |
| `vtr_reader_node` | `Hierarchy::node` (+ `Node::kind`, `NodeData`, `Hierarchy::children(id).count()`) |
| `vtr_reader_node_attr` | `Hierarchy::attrs(id)[i]` |
| `vtr_reader_enum_entry` | `Hierarchy::enum_entries(id)[i]` |
| `vtr_reader_children` | `Hierarchy::children` / `Hierarchy::roots` |
| `vtr_reader_signal_count` | `Reader::signal_count` |
| `vtr_reader_signal_kind` | `Hierarchy::signal_kind` |
| `vtr_reader_signal_var` | `Hierarchy::signal_var[sig]` |
| `vtr_reader_find_signal` | `Reader::find_signal(path, sep)` |
| `vtr_reader_find_node` | `Reader::find_node(&path.split(sep))` |
| `vtr_value_buf_new` / `_free` | `OwnedSignalValue` (owned) |
| `vtr_value_buf_get` | `OwnedSignalValue::borrow` |
| `vtr_value_buf_ascii` | `OwnedSignalValue::to_ascii` |
| `vtr_reader_value_at` | `Reader::value_at` |
| `vtr_reader_changes` | `Reader::changes` (collected, then iterated) |
| `vtr_reader_for_each_change` | `Reader::for_each_change` |
| `vtr_reader_load_signal` | `Reader::load_signal` |
| `vtr_reader_load_signals` | `Reader::load_signals` |
| `vtr_signal_data_clone` | `SignalData::clone` |
| `vtr_signal_data_free` | drop `SignalData` |
| `vtr_signal_data_len` | `SignalData::len` |
| `vtr_signal_data_times` | `SignalData::times` |
| `vtr_signal_data_get` | `SignalData::get` |
| `vtr_signal_data_initial` | `SignalData::initial` |
| `vtr_signal_data_index_at` | `SignalData::index_at` (`None` → `SIZE_MAX`) |
| `vtr_reader_time_table` | `Reader::time_table` |
| `vtr_reader_blackout` | `Reader::blackout()[i]` |
| `vtr_tx_get` | fields of `Transaction` + `Reader::generator_stream` |
| `vtr_tx_attr` | `Transaction::attrs[i]` (`TxAttr`) |
| `vtr_tx_event` / `vtr_tx_event_attr` | `Transaction::events[i]` (`TxEvent`) / `.attrs[j]` |
| `vtr_tx_stage` / `vtr_tx_stage_attr` | `Transaction::stages[i]` (`TxStage`) / `.attrs[j]` |
| `vtr_reader_visit_transactions` | `Reader::visit_transactions(&TxQuery { generator, stream, window }, cb)` |
| `vtr_reader_transaction` | `Reader::transaction(id)` |
| `vtr_reader_relations` | `Reader::relations_from(id)` (`direction == 0`) / `Reader::relations_to(id)` |

Rust-only conveniences with no C equivalent: `Writer::add_var_in` /
`add_alias_in` (explicit parent), `Writer::blackout_at`,
`Writer::current_time`, `Writer::stats`, `Reader::from_bytes`,
`Reader::open_with(ReadOptions)` (checksum verification and cache size),
`Reader::transactions` (collect),
`Reader::visit_relations`, `Reader::full_path`, `Reader::sections`, and
`Value::List` / `Value::Map` contents.

---

## 7. SystemC / C++ usage

For Verilator, `integrations/verilator` adds `--trace-vtr` (a port of
Verilator's FST backend onto this API): the model then dumps VTR through
`VerilatedVtrC` / `VerilatedVtrSc` with no user code beyond what an FST
dump needs.

The header is C++-clean. Wrap the handle in an RAII class so the file is
always closed (and therefore always gets a directory), and turn status codes
into exceptions at the boundary — but never let an exception escape a
callback into the library. For logging, `include/vtr_log.hpp` provides the
`VTR_LOG` macros over the C API (section 3.8, `docs/LOGGING.md`,
`demos/logging/`).

```cpp
#include "vtr.h"
#include <stdexcept>

class VtrWriter {
public:
    explicit VtrWriter(const char *path, const vtr_writer_options *o = nullptr) : w_(vtr_writer_create(path, o)) {
        if (!w_) throw std::runtime_error(vtr_last_error());
    }
    ~VtrWriter() { if (w_) vtr_writer_close(w_); }            // errors are lost here: call close() explicitly
    void close() { int rc = vtr_writer_close(w_); w_ = nullptr; check(rc); }
    void check(int rc) const { if (rc != VTR_OK) throw std::runtime_error(vtr_last_error()); }
    vtr_writer *get() const { return w_; }
    VtrWriter(const VtrWriter &) = delete;
    VtrWriter &operator=(const VtrWriter &) = delete;
private:
    vtr_writer *w_;
};
```

Usage: `VtrWriter tr("sim.vtr"); tr.check(vtr_writer_set_timescale(tr.get(), -12)); ... tr.close();`

SystemC notes:

* Pick the timescale to match the kernel's time resolution
  (`sc_get_time_resolution()`, default 1 ps → `-12`) and pass
  `sc_time_stamp().value()` to `vtr_writer_set_time`; it is non-decreasing by
  construction. Emit from a monitor process or from `sc_trace`-style
  callbacks, all on the SystemC kernel thread (the writer is single-threaded).
* Use scope type 65 (`sc_module`) with the module's `name()` as the scope
  name and its `kind()` as the component; use var type 64 (`bits`) / 21
  (`string`) / 3 (`real`) for `sc_uint<N>` / `std::string` / `double`
  signals.
* `sc_logic` values do not use the VTR code numbering (`SC_LOGIC_Z` is 2 and
  `SC_LOGIC_X` is 3 in SystemC). Map through `sc_logic::to_char()` and
  `vtr_writer_emit_logic_str`, or with an explicit table
  `{0, 1, 3 /*Z*/, 2 /*X*/}` and `vtr_writer_emit_bit`. `sc_lv<N>` /
  `sc_bv<N>` convert with `to_string()` + `vtr_writer_emit_logic_str`
  (MSB first, as VTR expects).
* Call `close()` from `end_of_simulation()` (or from the `VtrWriter`
  destructor of a top-level object) so that `sc_stop()` and normal exit both
  finish the file. Transactions still open at that point are stored with
  status 4 (open).
* For TLM-style transaction recording, mirror the FTR/SCV structure: one
  stream per socket or channel (`add_stream` under the module's scope), one
  generator per transaction type, attributes at phase 0/1/2 for the
  begin/record/end values, and `vtr_writer_relate` for
  parent/child and predecessor/successor links across streams.

## RTL VDB companion

The RTL VDB is a separate application library and `vtr-vdb` executable,
documented in [VDB_RTL.md](VDB_RTL.md). It adds no functions to the VTR C ABI.
A producer can set the optional `design.vdb_id` generic string file attribute
using the existing writer API; source semantics remain in the companion.

Module netlist SVG snapshots are provided by the Rust companion API and
`vtr-vdb netlist DESIGN TRACE INSTANCE --time T --output FILE.svg`. Existing C
producers supply the same immutable waveform values; the core C ABI gains no
layout or application presentation functions.
