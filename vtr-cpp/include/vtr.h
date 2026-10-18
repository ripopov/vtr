/* vtr.h - C API for VTR (Versatile Trace Record) trace files. */

#ifndef VTR_H
#define VTR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define VTR_OK            0
#define VTR_ERR_INVALID   1
#define VTR_ERR_STATE     2
#define VTR_ERR_IO        3
#define VTR_ERR_CORRUPT   4
#define VTR_ERR_VERSION   5
#define VTR_ERR_CODEC     6
#define VTR_ERR_CHECKSUM  7
#define VTR_ERR_NULL      8
#define VTR_ERR_NOT_FOUND 9

#define VTR_NONE 0xFFFFFFFFu

const char *vtr_last_error(void);
const char *vtr_version(void);

#define VTR_CODEC_NONE 0
#define VTR_CODEC_LZ4  1
#define VTR_CODEC_ZSTD 2

#define VTR_FILE_VERILOG       0
#define VTR_FILE_VHDL          1
#define VTR_FILE_MIXED         2
#define VTR_FILE_SYSTEMC       3
#define VTR_FILE_ARCHITECTURAL 4
#define VTR_FILE_SOFTWARE      5
#define VTR_FILE_OTHER         255

#define VTR_API_VERSION 3

#define VTR_NODE_SCOPE      1
#define VTR_NODE_VAR        2
#define VTR_NODE_GENERATOR  3
#define VTR_NODE_ENUM_TABLE 4

#define VTR_SIGNAL_BITS   0
#define VTR_SIGNAL_REAL   1
#define VTR_SIGNAL_VARLEN 2

#define VTR_DIR_IMPLICIT 0
#define VTR_DIR_INPUT    1
#define VTR_DIR_OUTPUT   2
#define VTR_DIR_INOUT    3
#define VTR_DIR_BUFFER   4
#define VTR_DIR_LINKAGE  5

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

#define VTR_LOGIC_0 0
#define VTR_LOGIC_1 1
#define VTR_LOGIC_X 2
#define VTR_LOGIC_Z 3
#define VTR_LOGIC_U 4
#define VTR_LOGIC_W 5
#define VTR_LOGIC_L 6
#define VTR_LOGIC_H 7
#define VTR_LOGIC_DONT_CARE 8

#define VTR_TX_STATUS_UNSET   0
#define VTR_TX_STATUS_OK      1
#define VTR_TX_STATUS_ERROR   2
#define VTR_TX_STATUS_ABORTED 3
#define VTR_TX_STATUS_OPEN    4
#define VTR_TX_KIND_UNSPECIFIED 0
#define VTR_TX_KIND_INTERNAL    1
#define VTR_TX_KIND_SERVER      2
#define VTR_TX_KIND_CLIENT      3
#define VTR_TX_KIND_PRODUCER    4
#define VTR_TX_KIND_CONSUMER    5

#define VTR_META_WRITER  0
#define VTR_META_DATE    1
#define VTR_META_COMMENT 2
#define VTR_RELATIONS_FROM 0
#define VTR_RELATIONS_TO   1

#define VTR_SEVERITY_TRACE 0
#define VTR_SEVERITY_DEBUG 1
#define VTR_SEVERITY_INFO  2
#define VTR_SEVERITY_WARN  3
#define VTR_SEVERITY_ERROR 4
#define VTR_SEVERITY_FATAL 5

#define VTR_VAL_NULL     0
#define VTR_VAL_BOOL     1
#define VTR_VAL_I64      2
#define VTR_VAL_U64      3
#define VTR_VAL_F64      4
#define VTR_VAL_STR      5
#define VTR_VAL_BYTES    6
#define VTR_VAL_BITS     7
#define VTR_VAL_LOGIC    8
#define VTR_VAL_LOGIC9   9
#define VTR_VAL_TIME     10
#define VTR_VAL_ENUM     11
#define VTR_VAL_POINTER  12
#define VTR_VAL_FIXED    13
#define VTR_VAL_UFIXED   14
#define VTR_VAL_LIST     15
#define VTR_VAL_MAP      16
#define VTR_VAL_TEXT     17

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

typedef struct vtr_writer vtr_writer;

typedef struct vtr_writer_options {
    uint8_t  codec;
    int      level;
    uint32_t group_size;
    uint64_t block_records;
    uint64_t tx_block_bytes;
    int      dedup;
    int      checksums;
    uint32_t commit_interval_ms;
} vtr_writer_options;

void        vtr_writer_options_default(vtr_writer_options *o);
vtr_writer *vtr_writer_create(const char *path, const vtr_writer_options *opts );
int         vtr_writer_close(vtr_writer *w);

enum {
    VTR_ENDING_CLOSED = 0,
    VTR_ENDING_STOPPED = 1,
    VTR_ENDING_EXITED = 2,
    VTR_ENDING_CRASHED = 3,
    VTR_ENDING_RECOVERED = 4,
    VTR_ENDING_POISONED = 5,
};
typedef struct vtr_ending {
    uint8_t  kind;
    int32_t  signal, code, status;
    uint64_t address, thread, dropped;
    int      sealed;
    int      has_time;
    uint64_t time;
} vtr_ending;
int    vtr_writer_close_ending(vtr_writer *w, const vtr_ending *end);

uint32_t vtr_writer_enter(vtr_writer *w);
void     vtr_writer_leave(vtr_writer *w, uint32_t prev);

typedef struct vtr_guard_options {
    uint32_t deadline_ms;
    int      crashes;
    int      stops;
    uint32_t stop_grace_ms;
    int      exit;
} vtr_guard_options;
void vtr_guard_options_default(vtr_guard_options *o);
int  vtr_guard_install(const vtr_guard_options *opts , int *active_out );
int  vtr_guard_watch(vtr_writer *w);
void vtr_guard_unwatch(vtr_writer *w);
void vtr_guard_thread_init(void);
int  vtr_guard_stop_requested(void);
uint64_t vtr_guard_add_stop_callback(void (*cb)(void *user, int signal), void *user);
void     vtr_guard_remove_stop_callback(uint64_t id);
void vtr_guard_ending(vtr_ending *out);
size_t vtr_ending_format(const vtr_ending *end, char *buf, size_t cap);

int vtr_recover(const char *in_path, const char *out_path, int64_t *dropped_out);

int vtr_writer_set_timescale(vtr_writer *w, int8_t exp);
int vtr_writer_set_time_zero(vtr_writer *w, int64_t t);
int vtr_writer_set_file_type(vtr_writer *w, uint8_t ft);
int vtr_writer_set_writer_name(vtr_writer *w, const char *s);
int vtr_writer_set_date(vtr_writer *w, const char *s);
int vtr_writer_set_comment(vtr_writer *w, const char *s);
int vtr_writer_set_file_attr(vtr_writer *w, const char *key, const vtr_value *v);
uint32_t vtr_writer_intern(vtr_writer *w, const char *s);

uint32_t vtr_writer_add_scope(vtr_writer *w, uint32_t parent, const char *name, uint16_t scope_type, const char *component );
int      vtr_writer_add_var(vtr_writer *w, uint32_t parent, const char *name, uint16_t var_type, uint8_t direction,
                            uint8_t kind, uint32_t width, uint8_t states, uint32_t *node_out, uint32_t *signal_out);
uint32_t vtr_writer_add_alias(vtr_writer *w, uint32_t parent, const char *name, uint16_t var_type, uint8_t direction, uint32_t signal);
uint32_t vtr_writer_add_enum_table(vtr_writer *w, uint32_t parent, const char *name, size_t n, const char *const *literals, const char *const *values);
uint32_t vtr_writer_add_generator(vtr_writer *w, uint32_t parent, const char *name, const char *kind);
int      vtr_writer_node_attr(vtr_writer *w, uint32_t node, const char *key, const vtr_value *v);

int vtr_writer_set_time(vtr_writer *w, uint64_t t);
int vtr_writer_dump_off(vtr_writer *w);
int vtr_writer_dump_on(vtr_writer *w);
int vtr_writer_emit_bit(vtr_writer *w, uint32_t sig, uint8_t code);
int vtr_writer_emit_u64(vtr_writer *w, uint32_t sig, uint64_t value);
int vtr_writer_emit_words(vtr_writer *w, uint32_t sig, const uint32_t *words, size_t n);
int vtr_writer_emit_logic_str(vtr_writer *w, uint32_t sig, const char *s, size_t len );
int vtr_writer_emit_packed(vtr_writer *w, uint32_t sig, uint8_t states, const uint8_t *data, size_t len);
int vtr_writer_emit_real(vtr_writer *w, uint32_t sig, double v);
int vtr_writer_emit_varlen(vtr_writer *w, uint32_t sig, const uint8_t *data, size_t len);
int vtr_writer_flush(vtr_writer *w);

int vtr_writer_begin_tx(vtr_writer *w, uint32_t generator, uint64_t time, uint64_t *tx_out);
int vtr_writer_set_tx_parent(vtr_writer *w, uint64_t tx, uint64_t parent);
int vtr_writer_set_tx_kind(vtr_writer *w, uint64_t tx, uint8_t kind);
int vtr_writer_tx_attr(vtr_writer *w, uint64_t tx, uint32_t key, const vtr_value *v);
int vtr_writer_tx_event(vtr_writer *w, uint64_t tx, uint64_t time, uint32_t name, size_t n, const uint32_t *keys, const vtr_value *values);
int vtr_writer_tx_stage_begin(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t time);
int vtr_writer_tx_stage_end(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t time);
int vtr_writer_tx_stage(vtr_writer *w, uint64_t tx, uint32_t name, uint32_t lane, uint64_t begin, uint64_t end, size_t n, const uint32_t *keys, const vtr_value *values);
int vtr_writer_tx_stage_attr(vtr_writer *w, uint64_t tx, uint32_t key, const vtr_value *v);
int vtr_writer_end_tx(vtr_writer *w, uint64_t tx, uint64_t time, uint8_t status);
int vtr_writer_relate(vtr_writer *w, uint32_t kind, uint64_t from, uint64_t to, size_t n, const uint32_t *keys, const vtr_value *values);

#define VTR_LOG_GENERATOR_KIND "LOG"
uint32_t vtr_writer_add_log_site(vtr_writer *w, uint32_t scope, uint8_t severity, const char *fmt, const char *file, uint32_t line,
                                 const char *func, size_t n_args, const uint8_t *arg_types, const char *const *names);
uint32_t vtr_writer_log_site_node(const vtr_writer *w, uint32_t site);
int      vtr_writer_log(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent , size_t n, const vtr_value *args, uint64_t *id_out );
int      vtr_writer_log_raw(vtr_writer *w, uint32_t site, uint64_t time, uint64_t parent, const uint8_t *args, size_t len, uint64_t *id_out);

#define VTR_CLOCK_GENERATOR_KIND "CLOCK"
uint32_t vtr_writer_add_clock(vtr_writer *w, uint32_t scope, const char *name);
uint32_t vtr_writer_clock_generator(const vtr_writer *w, uint32_t clock);
int      vtr_writer_clock_run(vtr_writer *w, uint32_t clock, uint64_t first, uint64_t period);
int      vtr_writer_clock_stop(vtr_writer *w, uint32_t clock, uint64_t t);

typedef struct vtr_reader vtr_reader;

typedef struct vtr_reader_options {
    int verify_crc;
} vtr_reader_options;
vtr_reader *vtr_reader_open(const char *path);
vtr_reader *vtr_reader_open_with_options(const char *path, const vtr_reader_options *options);
void        vtr_reader_close(vtr_reader *r);
void        vtr_reader_clear_cache(vtr_reader *r);

typedef struct vtr_meta {
    int8_t   timescale;
    int64_t  time_zero;
    uint8_t  file_type;
    uint32_t group_size;
    int      has_time_range;
    uint64_t time_start, time_end;
    uint16_t version_major, version_minor;
    int      recovered;
    uint64_t recovered_dropped;
    uint32_t signal_count, node_count, string_count, signal_block_count, tx_block_count;
    uint64_t tx_count, relation_count;
    uint32_t blackout_count, file_attr_count;
    uint32_t log_block_count;
    uint64_t log_count;
    uint32_t log_site_count;
} vtr_meta;

int         vtr_reader_meta(const vtr_reader *r, vtr_meta *out);
int         vtr_reader_ending(const vtr_reader *r, vtr_ending *out);
const char *vtr_reader_meta_string(const vtr_reader *r, int which , size_t *len_out);
int         vtr_reader_file_attr(const vtr_reader *r, uint32_t i, uint32_t *key_out, vtr_value *v_out);
const char *vtr_reader_str(const vtr_reader *r, uint32_t id, size_t *len_out);

typedef struct vtr_node_info {
    uint8_t  kind;
    uint32_t parent;
    uint32_t name;
    uint16_t type_code;
    uint8_t  direction;
    uint32_t signal;
    int      is_alias;
    uint32_t aux_str;
    uint32_t attr_count, entry_count, child_count;
} vtr_node_info;

uint32_t vtr_reader_node_count(const vtr_reader *r);
int      vtr_reader_node(const vtr_reader *r, uint32_t id, vtr_node_info *out);
int      vtr_reader_node_attr(const vtr_reader *r, uint32_t id, uint32_t i, uint32_t *key_out, vtr_value *v_out);
int      vtr_reader_enum_entry(const vtr_reader *r, uint32_t id, uint32_t i, uint32_t *literal_out, uint32_t *value_out);
size_t   vtr_reader_children(const vtr_reader *r, uint32_t id , uint32_t *out, size_t cap);
uint32_t vtr_reader_signal_count(const vtr_reader *r);
int      vtr_reader_signal_kind(const vtr_reader *r, uint32_t sig, uint8_t *kind_out, uint32_t *width_out, uint8_t *states_out);
int vtr_reader_signal_var_type(const vtr_reader *r, uint32_t sig, uint16_t *type_out);
uint32_t vtr_reader_signal_var(const vtr_reader *r, uint32_t sig);
int      vtr_reader_find_signal(const vtr_reader *r, const char *path, char sep, uint32_t *sig_out);
int      vtr_reader_find_node(const vtr_reader *r, const char *path, char sep, uint32_t *node_out);

typedef struct vtr_census vtr_census;
typedef struct vtr_contributions vtr_contributions;
typedef struct vtr_scope_sizes vtr_scope_sizes;
typedef struct vtr_scope_size {
    uint32_t node;
    uint32_t parent;
    uint32_t signals, variables, scopes;
} vtr_scope_size;
vtr_census      *vtr_census_new(void);
uint32_t         vtr_census_enter(vtr_census *c);
int              vtr_census_var(vtr_census *c, uint32_t signal);
int              vtr_census_leave(vtr_census *c);
vtr_scope_sizes *vtr_census_finish(vtr_census *c);
void             vtr_census_free(vtr_census *c);
vtr_census      *vtr_census_new_recording(void);
vtr_scope_sizes *vtr_census_finish_recorded(vtr_census *c, vtr_contributions **out);
size_t           vtr_contributions_count(const vtr_contributions *x, const uint32_t *signals, size_t n,
                                         uint32_t *out, size_t cap);
void             vtr_contributions_free(vtr_contributions *x);
vtr_scope_sizes *vtr_reader_scope_sizes(const vtr_reader *r);
size_t           vtr_scope_sizes_len(const vtr_scope_sizes *s);
int              vtr_scope_sizes_get(const vtr_scope_sizes *s, size_t i, vtr_scope_size *out);
void             vtr_scope_sizes_free(vtr_scope_sizes *s);

typedef struct vtr_activity_index vtr_activity_index;
typedef struct vtr_activity_classes vtr_activity_classes;
typedef struct vtr_activity_summary {
    uint32_t blocks, signals;
    uint64_t changes, stretches;
    uint64_t delta_min, delta_max;
    uint64_t bytes, source_bytes;
} vtr_activity_summary;
int                   vtr_activity_write(const vtr_reader *r, const char *trace_path, const char *cache_dir,
                                         uint32_t threads, uint64_t memory, vtr_activity_summary *out);
typedef struct vtr_activity_control vtr_activity_control;
vtr_activity_control *vtr_activity_control_new(void);
void                  vtr_activity_control_free(vtr_activity_control *c);
int                   vtr_activity_control_cancel(const vtr_activity_control *c);
int                   vtr_activity_control_is_cancelled(const vtr_activity_control *c);
int                   vtr_activity_control_complete(const vtr_activity_control *c);
void                  vtr_activity_control_progress(const vtr_activity_control *c, size_t *completed, size_t *total);
int                   vtr_activity_write_controlled(const vtr_reader *r, const char *trace_path, const char *cache_dir,
                                         uint32_t threads, uint64_t memory, const vtr_activity_control *control, vtr_activity_summary *out);
vtr_activity_index   *vtr_activity_load(const vtr_reader *r, const char *trace_path, const char *cache_dir);
vtr_activity_index   *vtr_activity_decode(const vtr_reader *r, const uint8_t *data, size_t len,
                                         uint64_t memory, const vtr_activity_control *control);
void                  vtr_activity_free(vtr_activity_index *x);
uint64_t              vtr_activity_exact_width(const vtr_activity_index *x, uint64_t t0, uint64_t t1);
vtr_activity_classes *vtr_activity_classify(const vtr_activity_index *x, uint64_t t0, uint64_t t1);
const uint32_t       *vtr_activity_classes_active(const vtr_activity_classes *c, size_t *len);
const uint32_t       *vtr_activity_classes_undecided(const vtr_activity_classes *c, size_t *len);
void                  vtr_activity_classes_free(vtr_activity_classes *c);
int                   vtr_activity_resolve(const vtr_reader *r, const uint32_t *signals, size_t n, uint64_t t0, uint64_t t1,
                                           uint32_t *out, size_t *out_len);

typedef struct vtr_signal_value {
    uint8_t  kind;
    uint8_t  states;
    uint32_t width;
    double   real;
    const uint8_t *data;
    size_t   len;
} vtr_signal_value;

typedef struct vtr_value_buf vtr_value_buf;
vtr_value_buf *vtr_value_buf_new(void);
void           vtr_value_buf_free(vtr_value_buf *b);
int            vtr_value_buf_get(const vtr_value_buf *b, vtr_signal_value *out);
const char    *vtr_value_buf_ascii(vtr_value_buf *b);

int vtr_reader_value_at(const vtr_reader *r, uint32_t sig, uint64_t time, vtr_value_buf *buf);

typedef int (*vtr_change_cb)(void *user, uint64_t time, uint32_t sig, const vtr_signal_value *value);
int vtr_reader_changes(const vtr_reader *r, uint32_t sig, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);
int vtr_reader_for_each_change(const vtr_reader *r, uint64_t t0, uint64_t t1, vtr_change_cb cb, void *user);

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

const uint64_t *vtr_reader_time_table(const vtr_reader *r, size_t *len_out);
int             vtr_reader_blackout(const vtr_reader *r, uint32_t i, uint64_t *time_out, int *active_out);

typedef struct vtr_tx vtr_tx;
typedef struct vtr_tx_info {
    uint64_t id;
    uint32_t generator, scope;
    uint64_t begin, end;
    uint8_t  status, kind;
    int      has_parent;
    uint64_t parent;
    uint32_t attr_count, event_count, stage_count;
} vtr_tx_info;

int vtr_tx_get(const vtr_reader *r, const vtr_tx *tx, vtr_tx_info *out);
int vtr_tx_attr(const vtr_tx *tx, uint32_t i, uint32_t *key_out, vtr_value *v_out);
int vtr_tx_event(const vtr_tx *tx, uint32_t i, uint64_t *time_out, uint32_t *name_out, uint32_t *attr_count_out);
int vtr_tx_event_attr(const vtr_tx *tx, uint32_t i, uint32_t j, uint32_t *key_out, vtr_value *v_out);
int vtr_tx_stage(const vtr_tx *tx, uint32_t i, uint32_t *name_out, uint32_t *lane_out, uint64_t *begin_out, uint64_t *end_out, int *has_end_out, uint32_t *attr_count_out);
int vtr_tx_stage_attr(const vtr_tx *tx, uint32_t i, uint32_t j, uint32_t *key_out, vtr_value *v_out);

typedef int (*vtr_tx_cb)(void *user, const vtr_tx *tx);
int vtr_reader_visit_transactions(const vtr_reader *r, uint32_t generator , uint32_t scope ,
                                  uint64_t t0, uint64_t t1 , vtr_tx_cb cb, void *user);
int vtr_reader_transaction(const vtr_reader *r, uint64_t id, vtr_tx_cb cb, void *user);
int vtr_reader_transaction_generator(const vtr_reader *r, uint64_t id, uint32_t *out_generator);
int vtr_reader_transaction_generators(const vtr_reader *r, const uint64_t *ids, size_t len, uint32_t *out_generators);

typedef int (*vtr_relation_cb)(void *user, uint32_t kind, uint64_t from, uint64_t to, uint32_t n_attrs, const uint32_t *keys, const vtr_value *values);
int vtr_reader_relations(const vtr_reader *r, uint64_t id, int direction , vtr_relation_cb cb, void *user);

typedef struct vtr_log_site_info {
    uint32_t node, scope;
    uint8_t  severity;
    uint32_t fmt;
    uint32_t file, func;
    uint32_t line;
    uint32_t arg_count;
} vtr_log_site_info;

uint32_t vtr_reader_log_site_count(const vtr_reader *r);
uint64_t vtr_reader_log_count(const vtr_reader *r);
int      vtr_reader_log_site(const vtr_reader *r, uint32_t i, vtr_log_site_info *out);
int      vtr_reader_log_site_arg(const vtr_reader *r, uint32_t i, uint32_t j, uint8_t *type_out, uint32_t *name_out);
uint32_t vtr_reader_log_site_of(const vtr_reader *r, uint32_t generator);

typedef struct vtr_log_rec {
    uint64_t id, time;
    uint64_t parent;
    uint32_t site, generator, scope;
    uint8_t  severity;
    uint32_t arg_count;
    const void *inner_;
} vtr_log_rec;

typedef int (*vtr_log_cb)(void *user, const vtr_log_rec *rec);
int    vtr_reader_visit_log(const vtr_reader *r, uint32_t scope , uint32_t generator ,
                            uint8_t min_severity, uint64_t t0, uint64_t t1 , vtr_log_cb cb, void *user);
int    vtr_log_rec_arg(const vtr_log_rec *rec, uint32_t i, vtr_value *v_out);
size_t vtr_log_rec_format(const vtr_reader *r, const vtr_log_rec *rec, char *buf, size_t cap);

typedef struct vtr_clock_info {
    uint32_t generator;
} vtr_clock_info;

typedef struct vtr_stretch {
    uint64_t begin, end;
    uint64_t period;
    uint64_t first_cycle;
} vtr_stretch;

typedef struct vtr_cycle_at {
    uint64_t cycle, edge;
    int      has_next_edge;
    uint64_t next_edge;
    double   fraction;
    int      stopped;
} vtr_cycle_at;

typedef struct vtr_clock_timeline vtr_clock_timeline;
uint32_t vtr_reader_clock_count(const vtr_reader *r);
int      vtr_reader_clock(const vtr_reader *r, uint32_t clock, vtr_clock_info *out);
uint32_t vtr_reader_generator_clock(const vtr_reader *r, uint32_t generator);
vtr_clock_timeline *vtr_reader_load_clock(const vtr_reader *r, uint32_t clock);
void     vtr_clock_timeline_free(vtr_clock_timeline *t);
size_t   vtr_clock_timeline_stretch_count(const vtr_clock_timeline *t);
uint64_t vtr_clock_timeline_edge_count(const vtr_clock_timeline *t);
int      vtr_clock_timeline_is_open(const vtr_clock_timeline *t);
int      vtr_clock_timeline_stretch(const vtr_clock_timeline *t, size_t i, vtr_stretch *out);
int      vtr_clock_timeline_cycle_at(const vtr_clock_timeline *t, uint64_t time, vtr_cycle_at *out);
int      vtr_clock_timeline_edge(const vtr_clock_timeline *t, uint64_t cycle, uint64_t *time_out);
int      vtr_clock_timeline_next_edge(const vtr_clock_timeline *t, uint64_t time, uint64_t *time_out);
int      vtr_clock_timeline_prev_edge(const vtr_clock_timeline *t, uint64_t time, uint64_t *time_out);

#ifdef __cplusplus
}
#endif
#endif
