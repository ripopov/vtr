/*
 * vtr.h - C API for VTR (Vibe Trace Record) trace files.
 *
 * Conventions
 *   - Functions that can fail return int: VTR_OK (0) or a VTR_ERR_* code.
 *     vtr_last_error() returns a thread-local message for the last failure.
 *   - Handles are opaque and owned by the caller until *_close / *_free.
 *     There is no global state; handles may be used from any single thread
 *     at a time (readers may be shared between threads for read-only calls).
 *   - Input strings are NUL-terminated UTF-8 unless a length is given.
 *   - Reader accessors return pointers that stay valid while the reader (or
 *     the buffer / data object) is alive. Nothing needs to be freed except
 *     objects created by *_new / *_load_* functions.
 *   - Ids: node ids, signal ids and string ids are dense u32 handles;
 *     VTR_NONE (0xFFFFFFFF) means "none". Transaction ids are u64 (>= 1).
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

const char *vtr_last_error(void);   /* never NULL */
const char *vtr_version(void);

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
#define VTR_VAL_TEXT     17  /* data, len: inline UTF-8 text (not interned) */
#define VTR_VAL_MAP      16  /* read-only: len = entry count */

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
    uint8_t  codec;          /* 0 none, 1 lz4, 2 zstd (default) */
    int      level;          /* zstd level (default 3) */
    uint32_t group_size;     /* signals per value-change group (default 256) */
    uint64_t block_records;  /* value changes per signal block, the compression unit (default 16M) */
    uint64_t chunk_records;  /* value changes per hand-off to the background encoder (default 512K) */
    uint64_t tx_block_bytes; /* row bytes per transaction block (default 4 MiB) */
    int      background;     /* encode/compress on a background thread (default 1) */
    int      dedup;          /* drop value changes equal to the current value (default 1) */
    int      checksums;      /* store a CRC32 per section (default 1); readers verify on request */
    uint32_t log_encoders;   /* helper threads encoding log blocks in background mode (default 2; 0 = sink thread) */
} vtr_writer_options;

void        vtr_writer_options_default(vtr_writer_options *o);
vtr_writer *vtr_writer_create(const char *path, const vtr_writer_options *opts /* nullable */);
int         vtr_writer_close(vtr_writer *w);   /* finishes and frees; returns status */

/* Metadata (before the first flush). */
int vtr_writer_set_timescale(vtr_writer *w, int8_t exp);        /* 10^exp seconds, default -9 */
int vtr_writer_set_time_zero(vtr_writer *w, int64_t t);
int vtr_writer_set_file_type(vtr_writer *w, uint8_t ft);        /* 0 verilog 1 vhdl 2 mixed 3 systemc 4 architectural 5 software */
int vtr_writer_set_writer_name(vtr_writer *w, const char *s);
int vtr_writer_set_date(vtr_writer *w, const char *s);
int vtr_writer_set_comment(vtr_writer *w, const char *s);
int vtr_writer_set_file_attr(vtr_writer *w, const char *key, const vtr_value *v);
uint32_t vtr_writer_intern(vtr_writer *w, const char *s);       /* string id for keys/names */

/* Hierarchy. Scope/var type codes: see docs/SPEC.md (FST codes 0..29 are reused). */
uint32_t vtr_writer_begin_scope(vtr_writer *w, const char *name, uint16_t scope_type, const char *component /* nullable */);
int      vtr_writer_end_scope(vtr_writer *w);
/* kind: 0 = bits (width, states in {2,4,9}), 1 = real, 2 = variable length. direction: 0 implicit 1 in 2 out 3 inout 4 buffer 5 linkage */
int      vtr_writer_add_var(vtr_writer *w, const char *name, uint16_t var_type, uint8_t direction,
                            uint8_t kind, uint32_t width, uint8_t states, uint32_t *node_out, uint32_t *signal_out);
int      vtr_writer_add_alias(vtr_writer *w, const char *name, uint16_t var_type, uint8_t direction, uint32_t signal, uint32_t *node_out);
uint32_t vtr_writer_add_enum_table(vtr_writer *w, const char *name, size_t n, const char *const *literals, const char *const *values);
uint32_t vtr_writer_add_stream(vtr_writer *w, uint32_t parent /* or VTR_NONE */, const char *name, const char *kind);
uint32_t vtr_writer_add_generator(vtr_writer *w, uint32_t stream, const char *name);
int      vtr_writer_node_attr(vtr_writer *w, uint32_t node, const char *key, const vtr_value *v);

/* Time and values. Logic codes: 0,1,2=X,3=Z,4=U,5=W,6=L,7=H,8=- */
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

/* Transactions. phase: 0 begin 1 record 2 end. status: 0 unset 1 ok 2 error 3 aborted. kind: OpenTelemetry span kinds 0..5. */
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
 * zero-duration transaction storing only the argument values (see docs/LOGGING.md).
 * Severity: 0 trace 1 debug 2 info 3 warn 4 error 5 fatal (other values allowed).
 * arg_types: VTR_VAL_BOOL/I64/U64/F64/STR/BYTES/TIME/POINTER/TEXT. file, func, names nullable. */
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

vtr_reader *vtr_reader_open(const char *path);
void        vtr_reader_close(vtr_reader *r);

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

int         vtr_reader_meta(const vtr_reader *r, vtr_meta *out);
const char *vtr_reader_meta_string(const vtr_reader *r, int which /* 0 writer 1 date 2 comment */, size_t *len_out);
int         vtr_reader_file_attr(const vtr_reader *r, uint32_t i, uint32_t *key_out, vtr_value *v_out);
const char *vtr_reader_str(const vtr_reader *r, uint32_t id, size_t *len_out); /* not NUL-terminated */

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
size_t   vtr_reader_children(const vtr_reader *r, uint32_t id /* or VTR_NONE for roots */, uint32_t *out, size_t cap); /* returns total count */
uint32_t vtr_reader_signal_count(const vtr_reader *r);
int      vtr_reader_signal_kind(const vtr_reader *r, uint32_t sig, uint8_t *kind_out, uint32_t *width_out, uint8_t *states_out);
uint32_t vtr_reader_signal_var(const vtr_reader *r, uint32_t sig);
int      vtr_reader_find_signal(const vtr_reader *r, const char *path, char sep, uint32_t *sig_out);
int      vtr_reader_find_node(const vtr_reader *r, const char *path, char sep, uint32_t *node_out);

typedef struct vtr_signal_value {
    uint8_t  kind;    /* 0 bits 1 real 2 varlen */
    uint8_t  states;  /* packing of data: 2, 4 or 9 */
    uint32_t width;
    double   real;
    const uint8_t *data;
    size_t   len;
} vtr_signal_value;

typedef struct vtr_value_buf vtr_value_buf;
vtr_value_buf *vtr_value_buf_new(void);
void           vtr_value_buf_free(vtr_value_buf *b);
int            vtr_value_buf_get(const vtr_value_buf *b, vtr_signal_value *out);
const char    *vtr_value_buf_ascii(vtr_value_buf *b);   /* "0101xz" MSB first / real / bytes */

int vtr_reader_value_at(const vtr_reader *r, uint32_t sig, uint64_t time, vtr_value_buf *buf);

typedef int (*vtr_change_cb)(void *user, uint64_t time, uint32_t sig, const vtr_signal_value *value); /* return non-zero to stop */
int vtr_reader_changes(const vtr_reader *r, uint32_t sig, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);
int vtr_reader_for_each_change(const vtr_reader *r, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);

/* Immutable history; handles may outlive the reader and be read concurrently.
 * Borrowed time/value pointers are read-only and valid until the handle is freed. */
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

const uint64_t *vtr_reader_time_table(const vtr_reader *r, size_t *len_out);
int             vtr_reader_blackout(const vtr_reader *r, uint32_t i, uint64_t *time_out, int *active_out);

/* Transactions */
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
int vtr_reader_visit_transactions(const vtr_reader *r, uint32_t generator /* or VTR_NONE */, uint32_t stream /* or VTR_NONE */,
                                  uint64_t t0, uint64_t t1 /* 0 = no window */, vtr_tx_cb cb, void *user);
int vtr_reader_transaction(const vtr_reader *r, uint64_t id, vtr_tx_cb cb, void *user);

typedef int (*vtr_relation_cb)(void *user, uint32_t kind, uint64_t from, uint64_t to, uint32_t n_attrs, const uint32_t *keys, const vtr_value *values);
int vtr_reader_relations(const vtr_reader *r, uint64_t id, int direction /* 0 from id, 1 to id */, vtr_relation_cb cb, void *user);

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
int    vtr_reader_visit_log(const vtr_reader *r, uint32_t stream /* or VTR_NONE */, uint32_t generator /* or VTR_NONE */,
                            uint8_t min_severity, uint64_t t0, uint64_t t1 /* 0 = no window */, vtr_log_cb cb, void *user);
int    vtr_log_rec_arg(const vtr_log_rec *rec, uint32_t i, vtr_value *v_out);  /* TEXT/BYTES point into the reader */
size_t vtr_log_rec_format(const vtr_reader *r, const vtr_log_rec *rec, char *buf, size_t cap); /* snprintf-like: returns full length */

#ifdef __cplusplus
}
#endif
#endif /* VTR_H */
