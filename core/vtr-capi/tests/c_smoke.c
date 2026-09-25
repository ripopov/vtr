/* C smoke test for the VTR C API: writes a file, reads it back, checks values. */
#include "vtr.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(x) do { int rc_ = (x); if (rc_ != VTR_OK) { fprintf(stderr, "%s:%d: %s -> %d (%s)\n", __FILE__, __LINE__, #x, rc_, vtr_last_error()); return 1; } } while (0)
#define ASSERT(x) do { if (!(x)) { fprintf(stderr, "%s:%d: assertion failed: %s\n", __FILE__, __LINE__, #x); return 1; } } while (0)

static int count_changes(void *user, uint64_t time, uint32_t sig, const vtr_signal_value *v) {
    (void)time; (void)sig; (void)v;
    (*(int *)user)++;
    return 0;
}

static int tx_cb(void *user, const vtr_tx *tx) {
    vtr_tx_info info;
    const vtr_reader *r = (const vtr_reader *)user;
    if (vtr_tx_get(r, tx, &info) != VTR_OK) return 1;
    if (info.id == 1) {
        uint32_t key; vtr_value v;
        if (info.attr_count != 1 || vtr_tx_attr(tx, 0, &key, &v) != VTR_OK) return 1;
        if (v.tag != VTR_VAL_U64 || v.u != 0x1000) return 1;
        if (info.stage_count != 2) return 1;
    }
    return 0;
}

/* A clock that changes speed, written and read back through the C API. */
static int clock_smoke(const char *path) {
    vtr_writer *w = vtr_writer_create(path, NULL);
    ASSERT(w != NULL);
    uint32_t top = vtr_writer_add_scope(w, VTR_NONE, "top", VTR_SCOPE_MODULE, "top");
    uint32_t clk = vtr_writer_add_clock(w, top, "clk");
    ASSERT(clk == 0);
    uint32_t cs = vtr_writer_clock_stream(w, clk);
    uint32_t pipe = vtr_writer_add_stream(w, top, "pipe", "PIPELINE");
    vtr_value link; memset(&link, 0, sizeof link); link.tag = VTR_VAL_STR; link.str_id = vtr_writer_intern(w, "top.clk");
    CHECK(vtr_writer_node_attr(w, pipe, "vtr.clock", &link));
    ASSERT(vtr_writer_clock_run(w, clk, 0, 0) == VTR_ERR_INVALID);
    CHECK(vtr_writer_clock_run(w, clk, 10, 4));
    ASSERT(vtr_writer_clock_run(w, clk, 20, 4) == VTR_ERR_STATE);
    CHECK(vtr_writer_clock_stop(w, clk, 29));      /* edges 10..26 */
    CHECK(vtr_writer_clock_run(w, clk, 30, 10));   /* edges 30..90, running at close */
    CHECK(vtr_writer_set_time(w, 95));
    CHECK(vtr_writer_close(w));

    vtr_reader *rd = vtr_reader_open(path);
    ASSERT(rd != NULL);
    ASSERT(vtr_reader_clock_count(rd) == 1);
    vtr_clock_info ci;
    CHECK(vtr_reader_clock(rd, 0, &ci));
    ASSERT(ci.stream == cs);
    ASSERT(vtr_reader_clock(rd, 1, &ci) == VTR_ERR_NOT_FOUND);
    ASSERT(vtr_reader_stream_clock(rd, pipe) == 0);
    ASSERT(vtr_reader_stream_clock(rd, top) == VTR_NONE);
    vtr_clock_timeline *tl = vtr_reader_load_clock(rd, 0);
    ASSERT(tl != NULL);
    vtr_reader_close(rd);   /* the timeline outlives the reader */
    ASSERT(vtr_clock_timeline_stretch_count(tl) == 2);
    ASSERT(vtr_clock_timeline_edge_count(tl) == 5 + 7);
    ASSERT(vtr_clock_timeline_is_open(tl));
    vtr_stretch st;
    CHECK(vtr_clock_timeline_stretch(tl, 1, &st));
    ASSERT(st.begin == 30 && st.end == 90 && st.period == 10 && st.first_cycle == 5);
    vtr_cycle_at c;
    ASSERT(vtr_clock_timeline_cycle_at(tl, 9, &c) == VTR_ERR_NOT_FOUND);
    CHECK(vtr_clock_timeline_cycle_at(tl, 35, &c));
    ASSERT(c.cycle == 5 && c.edge == 30 && c.has_next_edge && c.next_edge == 40 && c.fraction == 0.5 && !c.stopped);
    uint64_t t;
    CHECK(vtr_clock_timeline_edge(tl, 4, &t));
    ASSERT(t == 26);
    CHECK(vtr_clock_timeline_next_edge(tl, 26, &t));
    ASSERT(t == 30);
    CHECK(vtr_clock_timeline_prev_edge(tl, 30, &t));
    ASSERT(t == 26);
    ASSERT(vtr_clock_timeline_edge(tl, 12, &t) == VTR_ERR_NOT_FOUND);
    vtr_clock_timeline_free(tl);
    return 0;
}

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "c_smoke.vtr";
    vtr_writer_options o;
    vtr_writer_options_default(&o);
    o.block_records = 64;
    vtr_writer *w = vtr_writer_create(path, &o);
    ASSERT(w != NULL);
    CHECK(vtr_writer_set_timescale(w, -12));
    uint32_t top = vtr_writer_add_scope(w, VTR_NONE, "top", VTR_SCOPE_MODULE, "top");
    ASSERT(top != VTR_NONE);
    uint32_t clk, clk_n, bus, bus_n, r, r_n, s, s_n, wide, wide_n, event, event_n;
    CHECK(vtr_writer_add_var(w, top, "clk", VTR_VAR_WIRE, VTR_DIR_INPUT, VTR_SIGNAL_BITS, 1, 4, &clk_n, &clk));
    CHECK(vtr_writer_add_var(w, top, "bus", VTR_VAR_REG, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 16, 4, &bus_n, &bus));
    CHECK(vtr_writer_add_var(w, top, "r", VTR_VAR_REAL, VTR_DIR_IMPLICIT, VTR_SIGNAL_REAL, 0, 0, &r_n, &r));
    CHECK(vtr_writer_add_var(w, top, "s", VTR_VAR_STRING, VTR_DIR_IMPLICIT, VTR_SIGNAL_VARLEN, 0, 0, &s_n, &s));
    CHECK(vtr_writer_add_var(w, top, "wide", VTR_VAR_LOGIC, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 96, 4, &wide_n, &wide));
    CHECK(vtr_writer_add_var(w, top, "event", VTR_VAR_EVENT, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 1, 2, &event_n, &event));
    vtr_value av; memset(&av, 0, sizeof av); av.tag = VTR_VAL_I64; av.i = -42;
    CHECK(vtr_writer_node_attr(w, bus_n, "msb", &av));
    uint32_t stream = vtr_writer_add_stream(w, VTR_NONE, "pipe", "PIPELINE");
    uint32_t gen = vtr_writer_add_generator(w, stream, "instruction");
    ASSERT(stream != VTR_NONE && gen != VTR_NONE);
    /* Bad parents are rejected and add nothing. */
    uint32_t bad_n = 0, bad_s = 0;
    ASSERT(vtr_writer_add_scope(w, 12345, "bad", VTR_SCOPE_MODULE, NULL) == VTR_NONE);
    ASSERT(strlen(vtr_last_error()) > 0);
    ASSERT(vtr_writer_add_scope(w, clk_n, "bad", VTR_SCOPE_MODULE, NULL) == VTR_NONE);
    ASSERT(vtr_writer_add_var(w, gen, "bad", VTR_VAR_WIRE, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 1, 2, &bad_n, &bad_s) == VTR_ERR_INVALID);
    ASSERT(vtr_writer_add_alias(w, top, "bad", VTR_VAR_WIRE, VTR_DIR_IMPLICIT, 999) == VTR_NONE);
    ASSERT(vtr_writer_add_stream(w, bus_n, "bad", "PIPELINE") == VTR_NONE);
    ASSERT(vtr_writer_add_generator(w, top, "bad") == VTR_NONE);
    ASSERT(vtr_writer_add_scope(NULL, VTR_NONE, "bad", VTR_SCOPE_MODULE, NULL) == VTR_NONE);
    uint32_t late = VTR_NONE, late_v = VTR_NONE, late_v_n = VTR_NONE, late_gen = VTR_NONE, late_alias = VTR_NONE;
    uint32_t k_pc = vtr_writer_intern(w, "pc");
    uint32_t lane = vtr_writer_intern(w, "0");
    uint32_t st_f = vtr_writer_intern(w, "F");
    uint32_t st_x = vtr_writer_intern(w, "X");
    uint32_t dep = vtr_writer_intern(w, "dep");
    uint64_t prev = 0;
    for (uint64_t i = 0; i < 1000; i++) {
        CHECK(vtr_writer_set_time(w, i * 10));
        CHECK(vtr_writer_emit_bit(w, clk, (uint8_t)(i & 1)));
        CHECK(vtr_writer_emit_bit(w, event, 1));
        CHECK(vtr_writer_emit_bit(w, event, 1));
        if (i % 4 == 0) CHECK(vtr_writer_emit_u64(w, bus, i * 3));
        if (i % 10 == 0) CHECK(vtr_writer_emit_real(w, r, (double)i / 4.0));
        if (i % 50 == 0) { char buf[32]; snprintf(buf, sizeof buf, "s%llu", (unsigned long long)i); CHECK(vtr_writer_emit_varlen(w, s, (const uint8_t *)buf, strlen(buf))); }
        if (i % 7 == 0) { uint32_t words[3] = { (uint32_t)i, 0xdeadbeef, 0xff }; CHECK(vtr_writer_emit_words(w, wide, words, 3)); }
        if (i == 500) CHECK(vtr_writer_emit_logic_str(w, bus, "xxxx000011110000", SIZE_MAX));
        if (i == 500) {
            /* Hierarchy added after values were written. */
            late = vtr_writer_add_scope(w, top, "late", VTR_SCOPE_MODULE, "late_m");
            ASSERT(late != VTR_NONE);
            CHECK(vtr_writer_add_var(w, late, "v", VTR_VAR_BIT, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 8, 2, &late_v_n, &late_v));
            late_alias = vtr_writer_add_alias(w, late, "clk", VTR_VAR_WIRE, VTR_DIR_INPUT, clk);
            uint32_t late_stream = vtr_writer_add_stream(w, late, "events", "MONITOR");
            late_gen = vtr_writer_add_generator(w, late_stream, "hit");
            ASSERT(late_alias != VTR_NONE && late_gen != VTR_NONE);
        }
        if (i >= 500) CHECK(vtr_writer_emit_u64(w, late_v, i & 0xff));
        if (i == 600) {
            uint64_t tx;
            CHECK(vtr_writer_begin_tx(w, late_gen, i * 10, &tx));
            CHECK(vtr_writer_end_tx(w, tx, i * 10 + 5, VTR_TX_STATUS_UNSET));
        }
        if (i % 100 == 0) {
            uint64_t tx;
            CHECK(vtr_writer_begin_tx(w, gen, i * 10, &tx));
            vtr_value pc; memset(&pc, 0, sizeof pc); pc.tag = VTR_VAL_U64; pc.u = 0x1000 + i;
            CHECK(vtr_writer_tx_attr(w, tx, k_pc, &pc));
            CHECK(vtr_writer_tx_stage_begin(w, tx, st_f, lane, i * 10));
            CHECK(vtr_writer_tx_stage_begin(w, tx, st_x, lane, i * 10 + 5));
            if (prev) CHECK(vtr_writer_relate(w, dep, prev, tx, 0, NULL, NULL));
            CHECK(vtr_writer_end_tx(w, tx, i * 10 + 20, VTR_TX_STATUS_UNSET));
            prev = tx;
        }
    }
    CHECK(vtr_writer_close(w));

    vtr_reader *rd = vtr_reader_open(path);
    ASSERT(rd != NULL);
    vtr_meta m;
    CHECK(vtr_reader_meta(rd, &m));
    ASSERT(m.timescale == -12);
    ASSERT(m.signal_count == 7);
    ASSERT(m.tx_count == 11);
    ASSERT(m.relation_count == 9);
    ASSERT(m.has_time_range && m.time_end == 9990);
    uint32_t sig;
    CHECK(vtr_reader_find_signal(rd, "top.bus", '.', &sig));
    ASSERT(sig == bus);
    vtr_node_info ni;
    CHECK(vtr_reader_node(rd, bus_n, &ni));
    ASSERT(ni.kind == VTR_NODE_VAR && ni.type_code == VTR_VAR_REG && ni.signal == bus && ni.attr_count == 1);
    uint32_t key; vtr_value v;
    CHECK(vtr_reader_node_attr(rd, bus_n, 0, &key, &v));
    size_t klen; const char *ks = vtr_reader_str(rd, key, &klen);
    ASSERT(klen == 3 && memcmp(ks, "msb", 3) == 0 && v.tag == VTR_VAL_I64 && v.i == -42);
    uint32_t kids[16], owner_late = VTR_NONE;
    ASSERT(vtr_reader_children(rd, VTR_NONE, kids, 16) == 2);
    ASSERT(vtr_reader_children(rd, top, kids, 16) == 7);
    ASSERT(vtr_reader_children(rd, late, kids, 16) == 3);
    CHECK(vtr_reader_find_signal(rd, "top.late.v", '.', &sig));
    ASSERT(sig == late_v);
    CHECK(vtr_reader_find_signal(rd, "top.late.clk", '.', &sig));
    ASSERT(sig == clk);
    CHECK(vtr_reader_node(rd, late_v_n, &ni));
    ASSERT(ni.parent == late);
    CHECK(vtr_reader_transaction_generator(rd, 7, &owner_late));
    ASSERT(owner_late == late_gen);
    vtr_value_buf *b = vtr_value_buf_new();
    int n;
    CHECK(vtr_reader_value_at(rd, bus, 45, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b), "0000000000001100") == 0); /* i=4 -> 12 */
    CHECK(vtr_reader_value_at(rd, bus, 5000, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b), "xxxx000011110000") == 0);
    CHECK(vtr_reader_value_at(rd, bus, 5040, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b), "0000010111101000") == 0); /* i=504 -> 1512 */
    CHECK(vtr_reader_value_at(rd, r, 100, b));
    vtr_signal_value sv; CHECK(vtr_value_buf_get(b, &sv));
    ASSERT(sv.kind == 1 && sv.real == 2.5);
    CHECK(vtr_reader_value_at(rd, s, 999, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b), "s50") == 0);
    CHECK(vtr_reader_value_at(rd, late_v, 4990, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b), "00000000") == 0); /* before its declaration */
    CHECK(vtr_reader_value_at(rd, late_v, 5010, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b), "11110101") == 0); /* i=501 -> 0xf5 */
    n = 0;
    CHECK(vtr_reader_changes(rd, late_v, 0, 9990, count_changes, &n));
    ASSERT(n == 500);
    CHECK(vtr_reader_value_at(rd, wide, 70, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b) + 96 - 32, "00000000000000000000000000000111") == 0);
    n = 0;
    CHECK(vtr_reader_changes(rd, clk, 0, 95, count_changes, &n));
    ASSERT(n == 10);
    uint16_t declared_type = 0xffff;
    CHECK(vtr_reader_signal_var_type(rd, event, &declared_type));
    ASSERT(declared_type == 0);
    CHECK(vtr_reader_signal_var_type(rd, clk, &declared_type));
    ASSERT(declared_type == 16);
    ASSERT(vtr_reader_signal_var_type(rd, VTR_NONE, &declared_type) == VTR_ERR_NOT_FOUND);
    ASSERT(vtr_reader_signal_var_type(rd, event, NULL) == VTR_ERR_NULL);
    ASSERT(vtr_reader_signal_var_type(NULL, event, &declared_type) == VTR_ERR_NULL);
    vtr_signal_data *events = vtr_reader_load_signal(rd, event);
    ASSERT(events != NULL && vtr_signal_data_len(events) == 2000);
    for (size_t i = 0; i < 2000; ++i) ASSERT(vtr_signal_data_times(events)[i] == (i / 2) * 10);
    vtr_signal_data_free(events);
    n = 0;
    CHECK(vtr_reader_changes(rd, event, 0, 0, count_changes, &n));
    ASSERT(n == 2);
    vtr_signal_data *d = vtr_reader_load_signal(rd, bus);
    ASSERT(d != NULL && vtr_signal_data_len(d) == 251);
    ASSERT(vtr_signal_data_times(d)[1] == 40);
    ASSERT(vtr_signal_data_index_at(d, 39) == 0);
    uint32_t requests[] = {s, bus, clk, bus, s};
    vtr_signal_data *loaded[5] = {NULL};
    CHECK(vtr_reader_load_signals(rd, requests, 5, loaded));
    ASSERT(loaded[1] != loaded[3]); /* independent handles, shared storage */
    ASSERT(vtr_signal_data_times(loaded[1]) == vtr_signal_data_times(loaded[3]));
    ASSERT(vtr_signal_data_times(loaded[0]) == vtr_signal_data_times(loaded[4]));
    vtr_signal_value va, vb;
    CHECK(vtr_signal_data_get(loaded[0], 1, &va));
    CHECK(vtr_signal_data_get(loaded[4], 1, &vb));
    ASSERT(va.data == vb.data && va.len == vb.len);
    CHECK(vtr_signal_data_get(loaded[1], 1, &va));
    CHECK(vtr_signal_data_get(loaded[3], 1, &vb));
    ASSERT(va.data == vb.data && va.len == vb.len);
    vtr_signal_data *survivor = vtr_signal_data_clone(loaded[1]);
    ASSERT(survivor != NULL);
    ASSERT(vtr_signal_data_times(survivor) == vtr_signal_data_times(loaded[1]));
    for (size_t i = 0; i < 5; i++) vtr_signal_data_free(loaded[i]);
    CHECK(vtr_reader_load_signals(rd, NULL, 0, NULL));
    ASSERT(vtr_reader_load_signals(rd, NULL, 1, &d) == VTR_ERR_NULL);
    ASSERT(vtr_reader_load_signals(rd, requests, 1, NULL) == VTR_ERR_NULL);
    ASSERT(vtr_reader_load_signals(NULL, NULL, 0, NULL) == VTR_ERR_NULL);
    uint32_t invalid[] = {bus, VTR_NONE};
    vtr_signal_data *unchanged[] = {d, d};
    ASSERT(vtr_reader_load_signals(rd, invalid, 2, unchanged) == VTR_ERR_INVALID);
    ASSERT(unchanged[0] == d && unchanged[1] == d);
    ASSERT(vtr_signal_data_clone(NULL) == NULL);
    vtr_signal_data_free(d);
    size_t tl; const uint64_t *tt = vtr_reader_time_table(rd, &tl);
    ASSERT(tt != NULL && tl == 1000 && tt[999] == 9990);
    CHECK(vtr_reader_visit_transactions(rd, VTR_NONE, VTR_NONE, 0, 0, tx_cb, rd));
    CHECK(vtr_reader_transaction(rd, 1, tx_cb, rd));
    ASSERT(vtr_reader_transaction(rd, 99, tx_cb, rd) == VTR_ERR_NOT_FOUND);
    uint32_t owner = VTR_NONE;
    CHECK(vtr_reader_transaction_generator(rd, 1, &owner));
    ASSERT(owner == gen);
    ASSERT(vtr_reader_transaction_generator(rd, 99, &owner) == VTR_ERR_NOT_FOUND);
    ASSERT(owner == gen);
    ASSERT(vtr_reader_transaction_generator(rd, 1, NULL) == VTR_ERR_NULL);
    ASSERT(vtr_reader_transaction_generator(NULL, 1, &owner) == VTR_ERR_NULL);
    uint64_t owner_ids[] = {99, 1, 1};
    uint32_t owners[3];
    CHECK(vtr_reader_transaction_generators(rd, owner_ids, 3, owners));
    ASSERT(owners[0] == VTR_NONE && owners[1] == gen && owners[2] == gen);
    CHECK(vtr_reader_transaction_generators(rd, NULL, 0, NULL));
    ASSERT(vtr_reader_transaction_generators(rd, NULL, 1, owners) == VTR_ERR_NULL);
    ASSERT(vtr_reader_transaction_generators(rd, owner_ids, 1, NULL) == VTR_ERR_NULL);
    ASSERT(vtr_reader_transaction_generators(NULL, NULL, 0, NULL) == VTR_ERR_NULL);
    vtr_reader_clear_cache(NULL);
    vtr_reader_clear_cache(rd);
    ASSERT(vtr_signal_data_times(survivor)[1] == 40);
    CHECK(vtr_signal_data_get(survivor, 1, &sv));
    tt = vtr_reader_time_table(rd, &tl);
    ASSERT(tt != NULL && tl == 1000 && tt[999] == 9990);
    CHECK(vtr_reader_transaction(rd, 1, tx_cb, rd));
    vtr_value_buf_free(b);
    vtr_reader_close(rd);
    ASSERT(vtr_signal_data_len(survivor) == 251);
    ASSERT(vtr_signal_data_times(survivor)[1] == 40);
    CHECK(vtr_signal_data_get(survivor, 1, &sv));
    ASSERT(sv.kind == 0 && sv.width == 16);
    vtr_signal_data_free(survivor);
    char clock_path[4096];
    snprintf(clock_path, sizeof clock_path, "%s.clock.vtr", path);
    if (clock_smoke(clock_path)) return 1;
    /* error paths */
    ASSERT(vtr_reader_open("/nonexistent/file.vtr") == NULL);
    ASSERT(strlen(vtr_last_error()) > 0);
    printf("c_smoke: ok\n");
    return 0;
}
