/*
 * vtr.h - C API for VTR (Volna Trace Record) trace files.
 *
 * This header is the complete C API reference. Include it and link libvtr
 * (libvtr.a or the platform shared-library equivalent). The API has no
 * hidden global state; vtr_last_error() is its only thread-local state.
 *
 * A minimal waveform writer looks like this (check every status in real code):
 *
 *   vtr_writer *w = vtr_writer_create("trace.vtr", NULL);
 *   uint32_t node, signal;
 *   vtr_writer_begin_scope(w, "top", VTR_SCOPE_MODULE, NULL);
 *   vtr_writer_add_var(w, "clk", VTR_VAR_WIRE, VTR_DIR_INPUT,
 *                      VTR_SIGNAL_BITS, 1, 4, &node, &signal);
 *   vtr_writer_end_scope(w);
 *   vtr_writer_set_time(w, 0);
 *   vtr_writer_emit_bit(w, signal, VTR_LOGIC_0);
 *   vtr_writer_set_time(w, 5);
 *   vtr_writer_emit_bit(w, signal, VTR_LOGIC_1);
 *   int rc = vtr_writer_close(w);  // always frees w; check rc
 *
 * General conventions:
 *   - Status functions return VTR_OK or VTR_ERR_*. Pointer-returning
 *     constructors return NULL on failure. Id-returning constructors return
 *     VTR_NONE, except vtr_writer_intern(), where 0 is both the empty-string
 *     id and the failure result for invalid input. Inspect vtr_last_error()
 *     immediately after a failure; success does not clear it, and ordinary
 *     NOT_FOUND index misses may leave it unchanged.
 *   - The caller owns opaque handles until the matching *_close or *_free.
 *     vtr_writer_close() frees the writer even if finalization fails, so call
 *     it exactly once and check its result. Void destructors ignore NULL;
 *     status-returning functions, including writer_close(), report it.
 *   - One thread at a time may use a writer or mutable buffer. A reader and
 *     immutable vtr_signal_data handles support concurrent read-only calls.
 *     vtr_reader_clear_cache() instead requires exclusive reader access.
 *   - Input strings are NUL-terminated UTF-8 unless a length is supplied.
 *     Writer calls copy all input strings, values and arrays before returning.
 *     Reader strings are pointer/length pairs and are not NUL-terminated.
 *   - Borrowed callback objects and their pointers expire when the callback
 *     returns. Other reader pointers live until their documented owner is
 *     reused, cleared or freed. Never modify a returned const buffer.
 *   - Node, signal and string ids are dense uint32_t values. VTR_NONE means
 *     "no id". Transaction ids are uint64_t values starting at 1; 0 means no
 *     transaction when accepted by an argument.
 *   - All times are uint64_t file units: 10^timescale seconds (default ns).
 */
#ifndef VTR_H
#define VTR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- status codes ---------------------------------------------------- */
#define VTR_OK            0
#define VTR_ERR_INVALID   1   /* bad argument (unknown id, non-monotonic time, ...) */
#define VTR_ERR_STATE     2   /* call not allowed in the current writer state */
#define VTR_ERR_IO        3
#define VTR_ERR_CORRUPT   4   /* not a VTR file / structural error */
#define VTR_ERR_VERSION   5   /* file written by a newer major version */
#define VTR_ERR_CODEC     6
#define VTR_ERR_CHECKSUM  7
#define VTR_ERR_NULL      8   /* NULL handle or string */
#define VTR_ERR_NOT_FOUND 9

#define VTR_NONE 0xFFFFFFFFu

/* NUL-terminated. The error string is valid until the next failure on this
 * thread; the version string has static lifetime. Neither is ever NULL. */
const char *vtr_last_error(void);
const char *vtr_version(void);

/* ---- common codes ---------------------------------------------------- */
/* Compression codecs. */
#define VTR_CODEC_NONE 0
#define VTR_CODEC_LZ4  1
#define VTR_CODEC_ZSTD 2

/* File classifications. Unknown input values are stored as VTR_FILE_OTHER. */
#define VTR_FILE_VERILOG       0
#define VTR_FILE_VHDL          1
#define VTR_FILE_MIXED         2
#define VTR_FILE_SYSTEMC       3
#define VTR_FILE_ARCHITECTURAL 4
#define VTR_FILE_SOFTWARE      5
#define VTR_FILE_OTHER         255

/* Hierarchy node kinds returned by vtr_reader_node(). */
#define VTR_NODE_SCOPE      1
#define VTR_NODE_VAR        2
#define VTR_NODE_STREAM     3
#define VTR_NODE_GENERATOR  4
#define VTR_NODE_ENUM_TABLE 5

/* Signal storage kinds. */
#define VTR_SIGNAL_BITS   0
#define VTR_SIGNAL_REAL   1
#define VTR_SIGNAL_VARLEN 2

/* Variable directions. Unknown input values become VTR_DIR_IMPLICIT. */
#define VTR_DIR_IMPLICIT 0
#define VTR_DIR_INPUT    1
#define VTR_DIR_OUTPUT   2
#define VTR_DIR_INOUT    3
#define VTR_DIR_BUFFER   4
#define VTR_DIR_LINKAGE  5

/* Scope types. Values 0..22 match FST; 64+ are VTR extensions. Unknown
 * values are preserved. */
#define VTR_SCOPE_MODULE                0
#define VTR_SCOPE_TASK                  1
#define VTR_SCOPE_FUNCTION              2
#define VTR_SCOPE_BEGIN                 3
#define VTR_SCOPE_FORK                  4
#define VTR_SCOPE_GENERATE              5
#define VTR_SCOPE_STRUCT                6
#define VTR_SCOPE_UNION                 7
#define VTR_SCOPE_CLASS                 8
#define VTR_SCOPE_INTERFACE             9
#define VTR_SCOPE_PACKAGE              10
#define VTR_SCOPE_PROGRAM              11
#define VTR_SCOPE_VHDL_ARCHITECTURE    12
#define VTR_SCOPE_VHDL_PROCEDURE       13
#define VTR_SCOPE_VHDL_FUNCTION        14
#define VTR_SCOPE_VHDL_RECORD          15
#define VTR_SCOPE_VHDL_PROCESS         16
#define VTR_SCOPE_VHDL_BLOCK           17
#define VTR_SCOPE_VHDL_FOR_GENERATE    18
#define VTR_SCOPE_VHDL_IF_GENERATE     19
#define VTR_SCOPE_VHDL_GENERATE        20
#define VTR_SCOPE_VHDL_PACKAGE         21
#define VTR_SCOPE_SV_ARRAY             22
#define VTR_SCOPE_GENERIC              64
#define VTR_SCOPE_SC_MODULE            65
#define VTR_SCOPE_RESOURCE             66
#define VTR_SCOPE_INSTRUMENTATION_SCOPE 67
#define VTR_SCOPE_CORE                 68

/* Variable types. Values 0..29 match FST; 64+ are generic VTR types.
 * The type is descriptive; VTR_SIGNAL_* selects the actual encoding. */
#define VTR_VAR_EVENT          0
#define VTR_VAR_INTEGER        1
#define VTR_VAR_PARAMETER      2
#define VTR_VAR_REAL           3
#define VTR_VAR_REAL_PARAMETER 4
#define VTR_VAR_REG            5
#define VTR_VAR_SUPPLY0        6
#define VTR_VAR_SUPPLY1        7
#define VTR_VAR_TIME           8
#define VTR_VAR_TRI            9
#define VTR_VAR_TRIAND        10
#define VTR_VAR_TRIOR         11
#define VTR_VAR_TRIREG        12
#define VTR_VAR_TRI0          13
#define VTR_VAR_TRI1          14
#define VTR_VAR_WAND          15
#define VTR_VAR_WIRE          16
#define VTR_VAR_WOR           17
#define VTR_VAR_PORT          18
#define VTR_VAR_SPARSE_ARRAY  19
#define VTR_VAR_REALTIME      20
#define VTR_VAR_STRING        21
#define VTR_VAR_BIT           22
#define VTR_VAR_LOGIC         23
#define VTR_VAR_INT           24
#define VTR_VAR_SHORTINT      25
#define VTR_VAR_LONGINT       26
#define VTR_VAR_BYTE          27
#define VTR_VAR_ENUM          28
#define VTR_VAR_SHORTREAL     29
#define VTR_VAR_BITS          64
#define VTR_VAR_BYTES         65

/* Logic codes, from strongest/common values through IEEE 1164 extensions. */
#define VTR_LOGIC_0 0
#define VTR_LOGIC_1 1
#define VTR_LOGIC_X 2
#define VTR_LOGIC_Z 3
#define VTR_LOGIC_U 4
#define VTR_LOGIC_W 5
#define VTR_LOGIC_L 6
#define VTR_LOGIC_H 7
#define VTR_LOGIC_DONT_CARE 8

/* Transaction attributes, status and OpenTelemetry span kind. */
#define VTR_TX_PHASE_BEGIN  0
#define VTR_TX_PHASE_RECORD 1
#define VTR_TX_PHASE_END    2
#define VTR_TX_STATUS_UNSET   0
#define VTR_TX_STATUS_OK      1
#define VTR_TX_STATUS_ERROR   2
#define VTR_TX_STATUS_ABORTED 3
#define VTR_TX_STATUS_OPEN    4 /* assigned at close to an unended tx */
#define VTR_TX_KIND_UNSPECIFIED 0
#define VTR_TX_KIND_INTERNAL    1
#define VTR_TX_KIND_SERVER      2
#define VTR_TX_KIND_CLIENT      3
#define VTR_TX_KIND_PRODUCER    4
#define VTR_TX_KIND_CONSUMER    5

/* Reader selectors and relation directions. */
#define VTR_META_WRITER  0
#define VTR_META_DATE    1
#define VTR_META_COMMENT 2
#define VTR_RELATIONS_FROM 0
#define VTR_RELATIONS_TO   1

/* Log severity is ordered; a minimum severity filter includes larger values. */
#define VTR_SEVERITY_TRACE 0
#define VTR_SEVERITY_DEBUG 1
#define VTR_SEVERITY_INFO  2
#define VTR_SEVERITY_WARN  3
#define VTR_SEVERITY_ERROR 4
#define VTR_SEVERITY_FATAL 5

/* ---- values ---------------------------------------------------------- */
#define VTR_VAL_NULL     0
#define VTR_VAL_BOOL     1   /* b */
#define VTR_VAL_I64      2   /* i */
#define VTR_VAL_U64      3   /* u */
#define VTR_VAL_F64      4   /* f */
#define VTR_VAL_STR      5   /* str_id (interned) */
#define VTR_VAL_BYTES    6   /* data, len */
#define VTR_VAL_BITS     7   /* width, data (2-state packed, LSB first) */
#define VTR_VAL_LOGIC    8   /* width, data (4-state, 2 bits per bit) */
#define VTR_VAL_LOGIC9   9   /* width, data (9-state, 4 bits per bit) */
#define VTR_VAL_TIME     10  /* u (file time units) */
#define VTR_VAL_ENUM     11  /* i = value, str_id = literal name */
#define VTR_VAL_POINTER  12  /* u */
#define VTR_VAL_FIXED    13  /* i = raw, scale */
#define VTR_VAL_UFIXED   14  /* u = raw, scale */
#define VTR_VAL_LIST     15  /* read-only: len = element count (contents not exposed) */
#define VTR_VAL_MAP      16  /* read-only: len = entry count */
#define VTR_VAL_TEXT     17  /* data, len: inline UTF-8 text (not interned) */

/* A tagged attribute value. Zero-initialize this struct, set tag, then set
 * the fields named beside VTR_VAL_* above. The writer copies pointed-to data.
 * BITS/LOGIC/LOGIC9 are packed LSB-first using 1/2/4 bits per logic bit;
 * len must cover width. FIXED values mean raw * 2^-scale. LIST and MAP are
 * returned for inspection but cannot be supplied to writer functions. */
typedef struct vtr_value {
    uint8_t  tag;
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

/* ---- writer ---------------------------------------------------------- */
typedef struct vtr_writer vtr_writer;

typedef struct vtr_writer_options {
    uint8_t  codec;          /* VTR_CODEC_*, default ZSTD */
    int      level;          /* zstd level (default 3) */
    uint32_t group_size;     /* signals per value-change group (default 256) */
    uint64_t block_records;  /* value changes per signal block, the compression unit (default 16M) */
    uint64_t chunk_records;  /* value changes per hand-off to the background encoder (default 512K) */
    uint64_t tx_block_bytes; /* row bytes per transaction block (default 4 MiB) */
    int      background;     /* encode/compress on a background thread (default 1) */
    int      dedup;          /* drop unchanged non-event values (default 1); events are never dropped */
    int      checksums;      /* store per-section CRC32 (default 1); C open does not verify it */
    uint32_t log_encoders;   /* helper threads encoding log blocks in background mode (default 2; 0 = sink thread) */
} vtr_writer_options;

/* Initialize all fields before overriding individual options. NULL is ignored.
 * create() truncates path and uses defaults when opts is NULL. close() writes
 * the directory and pending data, closes open transactions with status OPEN,
 * and always frees w, including on error. Background I/O failures can first
 * appear at close, so its result must be checked. */
void        vtr_writer_options_default(vtr_writer_options *o);
vtr_writer *vtr_writer_create(const char *path, const vtr_writer_options *opts /* nullable */);
int         vtr_writer_close(vtr_writer *w);   /* finishes and frees; returns status */

/* Metadata must be set before the first explicit or automatic flush.
 * time_zero is a display offset; stored event times are unchanged. File
 * attribute values are copied and keys are interned. */
int vtr_writer_set_timescale(vtr_writer *w, int8_t exp);        /* 10^exp seconds, default -9 */
int vtr_writer_set_time_zero(vtr_writer *w, int64_t t);
int vtr_writer_set_file_type(vtr_writer *w, uint8_t ft);        /* VTR_FILE_* */
int vtr_writer_set_writer_name(vtr_writer *w, const char *s);
int vtr_writer_set_date(vtr_writer *w, const char *s);
int vtr_writer_set_comment(vtr_writer *w, const char *s);
int vtr_writer_set_file_attr(vtr_writer *w, const char *key, const vtr_value *v);
/* Intern once during setup and reuse the returned id for transaction names,
 * attribute keys, lanes, relation kinds and VTR_VAL_STR values. String id 0
 * is always the empty string. */
uint32_t vtr_writer_intern(vtr_writer *w, const char *s);

/* Hierarchy is a forest. begin_scope() adds under the current scope and pushes
 * it; end_scope() pops it. Variables and enum tables use the current scope or
 * become roots. Streams take an explicit scope parent (or VTR_NONE), and
 * generators take a stream. Id-returning functions return VTR_NONE on error.
 *
 * add_var() creates both a hierarchy node and a signal. For BITS, width is
 * the bit width (0 becomes 1) and states is 2, 4, or 9. REAL and VARLEN ignore
 * width/states. add_alias() creates another variable node for an existing
 * signal. Output pointers are optional. add_enum_table() takes parallel
 * literal/value string arrays. Set node attributes immediately: attributes
 * cannot be added after their node has been flushed. */
uint32_t vtr_writer_begin_scope(vtr_writer *w, const char *name, uint16_t scope_type, const char *component /* nullable */);
int      vtr_writer_end_scope(vtr_writer *w);
int      vtr_writer_add_var(vtr_writer *w, const char *name, uint16_t var_type, uint8_t direction,
                            uint8_t kind, uint32_t width, uint8_t states, uint32_t *node_out, uint32_t *signal_out);
int      vtr_writer_add_alias(vtr_writer *w, const char *name, uint16_t var_type, uint8_t direction, uint32_t signal, uint32_t *node_out);
uint32_t vtr_writer_add_enum_table(vtr_writer *w, const char *name, size_t n, const char *const *literals, const char *const *values);
uint32_t vtr_writer_add_stream(vtr_writer *w, uint32_t parent /* or VTR_NONE */, const char *name, const char *kind);
uint32_t vtr_writer_add_generator(vtr_writer *w, uint32_t stream, const char *name);
int      vtr_writer_node_attr(vtr_writer *w, uint32_t node, const char *key, const vtr_value *v);

/* Signal time is non-decreasing and independent of explicit transaction times.
 * dump_off/on record display blackout marks but do not suppress writes.
 *
 * Every emit records at the current signal time. With dedup enabled, unchanged
 * non-event values are omitted; VTR_VAR_EVENT signals retain every emit.
 * Initial values are 0 for 2-state vectors, X for 4/9-state vectors, 0.0 for
 * reals and empty for variable-length values; emitting the initial value may
 * therefore deduplicate.
 * emit_u64 truncates/zero-extends to the vector width. words[0] contains bits
 * 0..31. logic strings are MSB-first and accept 01xzuwlh-;
 * SIZE_MAX means NUL-terminated. Packed vectors are LSB-first: 1, 2, or 4
 * bits per logic bit for states 2, 4, or 9. emit_varlen() accepts raw bytes.
 * Input buffers are copied. flush() forces a block boundary, which exposes
 * pending data to crash-recovery readers but can reduce compression. */
int vtr_writer_set_time(vtr_writer *w, uint64_t t);             /* non-decreasing */
int vtr_writer_dump_off(vtr_writer *w);
int vtr_writer_dump_on(vtr_writer *w);
int vtr_writer_emit_bit(vtr_writer *w, uint32_t sig, uint8_t code);
int vtr_writer_emit_u64(vtr_writer *w, uint32_t sig, uint64_t value);
int vtr_writer_emit_words(vtr_writer *w, uint32_t sig, const uint32_t *words, size_t n); /* word 0 = bits 0..31 */
int vtr_writer_emit_logic_str(vtr_writer *w, uint32_t sig, const char *s, size_t len /* or SIZE_MAX */); /* "01xz..." MSB first */
int vtr_writer_emit_packed(vtr_writer *w, uint32_t sig, uint8_t states, const uint8_t *data, size_t len);
int vtr_writer_emit_real(vtr_writer *w, uint32_t sig, double v);
int vtr_writer_emit_varlen(vtr_writer *w, uint32_t sig, const uint8_t *data, size_t len);
int vtr_writer_flush(vtr_writer *w);                             /* force a block boundary */

/* Transactions are intervals on a generator. Any number may overlap. Their
 * names, attribute keys, stage names/lanes and relation kinds are interned
 * string ids. Attribute/event/relation arrays are parallel and may be NULL
 * when n == 0. Times are explicit; end times before begin are clamped.
 *
 * stage_begin() closes an open stage on the same lane before opening the new
 * one. stage_end() returns NOT_FOUND if the named stage/lane is not open.
 * stage_attr() targets the most recently added stage. end_tx() closes all
 * remaining stages. Relation endpoints need not exist in this file. */
int vtr_writer_begin_tx(vtr_writer *w, uint32_t generator, uint64_t time, uint64_t *tx_out);
int vtr_writer_set_tx_parent(vtr_writer *w, uint64_t tx, uint64_t parent);
int vtr_writer_set_tx_kind(vtr_writer *w, uint64_t tx, uint8_t kind);
int vtr_writer_tx_attr(vtr_writer *w, uint64_t tx, uint32_t key, uint8_t phase, const vtr_value *v);
int vtr_writer_tx_event(vtr_writer *w, uint64_t tx, uint64_t time, uint32_t name, size_t n, const uint32_t *keys, const vtr_value *values);
int vtr_writer_tx_stage_begin(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t time);
int vtr_writer_tx_stage_end(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t time); /* VTR_ERR_NOT_FOUND if none open */
int vtr_writer_tx_stage(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t begin, uint64_t end, size_t n, const uint32_t *keys, const vtr_value *values);
int vtr_writer_tx_stage_attr(vtr_writer *w, uint64_t tx, uint32_t key, const vtr_value *v);
int vtr_writer_end_tx(vtr_writer *w, uint64_t tx, uint64_t time, uint8_t status);
int vtr_writer_relate(vtr_writer *w, uint32_t kind, uint64_t from, uint64_t to, size_t n, const uint32_t *keys, const vtr_value *values);

/* Logs. A log stream (kind "LOG") holds one generator per call site ("log site"): the
 * format string plus severity, argument types and source location. Each message is a
 * zero-duration transaction storing only the argument values. Severities are
 * VTR_SEVERITY_* (other ordered values are allowed).
 * arg_types: VTR_VAL_BOOL/I64/U64/F64/STR/BYTES/TIME/POINTER/TEXT. file, func,
 * and names are nullable. add_log_site() returns a dense site id, not its
 * generator node id; use log_site_node() for the latter. */
uint32_t vtr_writer_add_log_stream(vtr_writer *w, uint32_t parent /* or VTR_NONE */, const char *name);
uint32_t vtr_writer_add_log_site(vtr_writer *w, uint32_t stream, uint8_t severity, const char *fmt, const char *file, uint32_t line,
                                 const char *func, size_t n_args, const uint8_t *arg_types, const char *const *names); /* VTR_NONE on error */
uint32_t vtr_writer_log_site_node(const vtr_writer *w, uint32_t site);
int      vtr_writer_log(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent /* tx id or 0 */, size_t n, const vtr_value *args, uint64_t *id_out /* nullable */);
/* Same with the argument values already in row encoding (bool: 1 byte; I64: zig-zag LEB128; U64/TIME/POINTER: LEB128;
 * F64: 8 bytes LE; STR: LEB128 string id; TEXT/BYTES: LEB128 length + bytes), in declaration order. The caller
 * guarantees the encoding matches the site (vtr_log.hpp does this by construction); a mismatch is reported at flush/close. */
int      vtr_writer_log_raw(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent, const uint8_t *args, size_t len, uint64_t *id_out);

/* ---- reader ---------------------------------------------------------- */
typedef struct vtr_reader vtr_reader;

/* open() memory-maps path and returns NULL on failure. close(NULL) is allowed.
 * Query decoding is lazy, so a corrupt block can fail after a successful open. */
vtr_reader *vtr_reader_open(const char *path);
void        vtr_reader_close(vtr_reader *r);
/* Exclusive access required. Invalidates borrowed time tables; owned signal
 * data and metadata pointers remain valid. NULL is ignored. */
void        vtr_reader_clear_cache(vtr_reader *r);

typedef struct vtr_meta {
    int8_t   timescale;
    int64_t  time_zero;
    uint8_t  file_type;
    uint32_t group_size;
    int      has_time_range;
    uint64_t time_start, time_end;
    uint16_t version_major, version_minor;
    int      recovered;           /* 1 when the directory was rebuilt (writer crashed) */
    uint32_t signal_count, node_count, string_count, signal_block_count, tx_block_count;
    uint64_t tx_count, relation_count;
    uint32_t blackout_count, file_attr_count;
    uint32_t log_block_count;
    uint64_t log_count;           /* log records; also counted in tx_count */
    uint32_t log_site_count;
} vtr_meta;

/* Metadata and string pointers are borrowed from r until close. Strings are
 * UTF-8 pointer/length pairs, not NUL-terminated. Indexed accessors return
 * NOT_FOUND when the index is out of range; output values are then unchanged. */
int         vtr_reader_meta(const vtr_reader *r, vtr_meta *out);
const char *vtr_reader_meta_string(const vtr_reader *r, int which /* VTR_META_* */, size_t *len_out);
int         vtr_reader_file_attr(const vtr_reader *r, uint32_t i, uint32_t *key_out, vtr_value *v_out);
const char *vtr_reader_str(const vtr_reader *r, uint32_t id, size_t *len_out); /* not NUL-terminated */

typedef struct vtr_node_info {
    uint8_t  kind;         /* VTR_NODE_* */
    uint32_t parent;       /* VTR_NONE for roots */
    uint32_t name;         /* string id */
    uint16_t type_code;    /* scope type / var type */
    uint8_t  direction;
    uint32_t signal;       /* vars only, else VTR_NONE */
    int      is_alias;
    uint32_t aux_str;      /* scope: component; stream: kind */
    uint32_t attr_count, entry_count, child_count;
} vtr_node_info;

/* Nodes and signals have dense ids from zero. children() returns the total
 * count even when cap is smaller; pass out == NULL, cap == 0 to measure.
 * find_* splits path on sep and matches components from a root; cache the
 * result instead of repeating path lookup in hot loops. */
uint32_t vtr_reader_node_count(const vtr_reader *r);
int      vtr_reader_node(const vtr_reader *r, uint32_t id, vtr_node_info *out);
int      vtr_reader_node_attr(const vtr_reader *r, uint32_t id, uint32_t i, uint32_t *key_out, vtr_value *v_out);
int      vtr_reader_enum_entry(const vtr_reader *r, uint32_t id, uint32_t i, uint32_t *literal_out, uint32_t *value_out);
size_t   vtr_reader_children(const vtr_reader *r, uint32_t id /* or VTR_NONE for roots */, uint32_t *out, size_t cap); /* returns total count */
uint32_t vtr_reader_signal_count(const vtr_reader *r);
int      vtr_reader_signal_kind(const vtr_reader *r, uint32_t sig, uint8_t *kind_out, uint32_t *width_out, uint8_t *states_out);
/* Variable type code of the original declaration; type_out is required. */
int vtr_reader_signal_var_type(const vtr_reader *r, uint32_t sig, uint16_t *type_out);
uint32_t vtr_reader_signal_var(const vtr_reader *r, uint32_t sig);
int      vtr_reader_find_signal(const vtr_reader *r, const char *path, char sep, uint32_t *sig_out);
int      vtr_reader_find_node(const vtr_reader *r, const char *path, char sep, uint32_t *node_out);

typedef struct vtr_signal_value {
    uint8_t  kind;    /* VTR_SIGNAL_* */
    uint8_t  states;  /* data packing: 2, 4 or 9 states */
    uint32_t width;
    double   real;
    const uint8_t *data;
    size_t   len;
} vtr_signal_value;

/* A reusable owner for one point-query value. value_at() returns the last
 * change at or before time, or the signal's initial value. get() borrows data
 * from b until the next value_at(); ascii() returns NUL-terminated text until
 * the next value_at() or ascii() on b. Point queries and callbacks may use
 * compact 2-state packing for a 4/9-state signal; whole-signal data below uses
 * declared packing. Allocate one buffer per concurrent query. */
typedef struct vtr_value_buf vtr_value_buf;
vtr_value_buf *vtr_value_buf_new(void);
void           vtr_value_buf_free(vtr_value_buf *b);
int            vtr_value_buf_get(const vtr_value_buf *b, vtr_signal_value *out);
const char    *vtr_value_buf_ascii(vtr_value_buf *b);   /* "0101xz" MSB first / real / bytes */

int vtr_reader_value_at(const vtr_reader *r, uint32_t sig, uint64_t time, vtr_value_buf *buf);

typedef int (*vtr_change_cb)(void *user, uint64_t time, uint32_t sig, const vtr_signal_value *value); /* return non-zero to stop */
/* Inclusive [t0,t1]. changes() visits one signal in time/emission order;
 * for_each_change() visits all signals. Initial values are not callbacks.
 * value and its data are valid only during the callback. Stopping is VTR_OK. */
int vtr_reader_changes(const vtr_reader *r, uint32_t sig, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);
int vtr_reader_for_each_change(const vtr_reader *r, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);

/* Complete immutable history. Handles may outlive the reader and be read
 * concurrently. Borrowed time/value pointers remain valid until the handle is
 * freed. Values use the signal's declared packing. index_at() returns the last
 * change at or before time, or SIZE_MAX; call initial() in the latter case. */
typedef struct vtr_signal_data vtr_signal_data;
vtr_signal_data *vtr_reader_load_signal(const vtr_reader *r, uint32_t sig);
/* Caller allocates n output slots; free each returned handle separately.
 * Duplicate IDs share immutable storage. On error, out is unchanged.
 * For n == 0, sigs and out may be NULL. */
int              vtr_reader_load_signals(const vtr_reader *r, const uint32_t *sigs, size_t n, vtr_signal_data **out);
vtr_signal_data *vtr_signal_data_clone(const vtr_signal_data *d); /* shares storage; NULL in -> NULL out */
void             vtr_signal_data_free(vtr_signal_data *d);
size_t           vtr_signal_data_len(const vtr_signal_data *d);
const uint64_t  *vtr_signal_data_times(const vtr_signal_data *d);
int              vtr_signal_data_get(const vtr_signal_data *d, size_t i, vtr_signal_value *out);
int              vtr_signal_data_initial(const vtr_signal_data *d, vtr_signal_value *out);
size_t           vtr_signal_data_index_at(const vtr_signal_data *d, uint64_t time); /* SIZE_MAX if before first change */

/* Distinct signal times, built lazily. The pointer expires at clear_cache() or
 * close(). Transaction times are not included. blackout active=0 means dump
 * off and active=1 means dump on. */
const uint64_t *vtr_reader_time_table(const vtr_reader *r, size_t *len_out);
int             vtr_reader_blackout(const vtr_reader *r, uint32_t i, uint64_t *time_out, int *active_out);

/* Transactions. The tx and all values borrowed from it are valid only inside
 * the callback. Indexed accessors accept nullable outputs and return NOT_FOUND
 * past the end. status, kind and attribute phase use VTR_TX_* constants. */
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
/* Filters are ANDed; VTR_NONE disables generator/stream filters. Transactions
 * overlapping inclusive [t0,t1] match; t1 == 0 disables the time window.
 * Visit order is file/end order, not transaction-id order. A nonzero callback
 * result stops successfully. transaction() calls cb once or returns NOT_FOUND. */
int vtr_reader_visit_transactions(const vtr_reader *r, uint32_t generator /* or VTR_NONE */, uint32_t stream /* or VTR_NONE */,
                                  uint64_t t0, uint64_t t1 /* 0 = no window */, vtr_tx_cb cb, void *user);
int vtr_reader_transaction(const vtr_reader *r, uint64_t id, vtr_tx_cb cb, void *user);
/* Owning generator; output is written only on success. */
int vtr_reader_transaction_generator(const vtr_reader *r, uint64_t id, uint32_t *out_generator);
/* Resolve owners in input order, including duplicates; missing IDs yield
 * VTR_NONE. Arrays have len elements; both may be NULL when len is zero.
 * Outputs remain unchanged on error. */
int vtr_reader_transaction_generators(const vtr_reader *r, const uint64_t *ids, size_t len, uint32_t *out_generators);

typedef int (*vtr_relation_cb)(void *user, uint32_t kind, uint64_t from, uint64_t to, uint32_t n_attrs, const uint32_t *keys, const vtr_value *values);
/* Callback arrays and values expire on return. A nonzero result stops
 * successfully. */
int vtr_reader_relations(const vtr_reader *r, uint64_t id, int direction /* VTR_RELATIONS_* */, vtr_relation_cb cb, void *user);

/* Logs. Records are also visible through vtr_reader_visit_transactions as zero-duration
 * transactions whose attribute keys are the argument names. */
typedef struct vtr_log_site_info {
    uint32_t node, stream;
    uint8_t  severity;
    uint32_t fmt;                 /* string id of the format string */
    uint32_t file, func;          /* string ids or VTR_NONE */
    uint32_t line;                /* 0 = unknown */
    uint32_t arg_count;
} vtr_log_site_info;

uint32_t vtr_reader_log_site_count(const vtr_reader *r);
uint64_t vtr_reader_log_count(const vtr_reader *r);
int      vtr_reader_log_site(const vtr_reader *r, uint32_t i, vtr_log_site_info *out);
int      vtr_reader_log_site_arg(const vtr_reader *r, uint32_t i, uint32_t j, uint8_t *type_out, uint32_t *name_out);
uint32_t vtr_reader_log_site_of(const vtr_reader *r, uint32_t generator);   /* site index or VTR_NONE */

typedef struct vtr_log_rec {    /* valid only inside the callback */
    uint64_t id, time;
    uint64_t parent;              /* transaction id or 0 */
    uint32_t site, generator, stream;
    uint8_t  severity;
    uint32_t arg_count;
    const void *inner_;
} vtr_log_rec;

typedef int (*vtr_log_cb)(void *user, const vtr_log_rec *rec);  /* return non-zero to stop */
/* VTR_NONE disables stream/generator filters; t1 == 0 disables the time
 * window. rec is callback-borrowed. format() behaves like snprintf: it always
 * returns the full length, NUL-terminates when cap > 0, and accepts NULL/0 to
 * measure. TEXT/BYTES arguments borrow storage from the reader. */
int    vtr_reader_visit_log(const vtr_reader *r, uint32_t stream /* or VTR_NONE */, uint32_t generator /* or VTR_NONE */,
                            uint8_t min_severity, uint64_t t0, uint64_t t1 /* 0 = no window */, vtr_log_cb cb, void *user);
int    vtr_log_rec_arg(const vtr_log_rec *rec, uint32_t i, vtr_value *v_out);  /* TEXT/BYTES point into the reader */
size_t vtr_log_rec_format(const vtr_reader *r, const vtr_log_rec *rec, char *buf, size_t cap); /* snprintf-like: returns full length */

#ifdef __cplusplus
}
#endif
#endif /* VTR_H */
