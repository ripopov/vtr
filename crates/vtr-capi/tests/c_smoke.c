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
        uint32_t key; uint8_t phase; vtr_value v;
        if (info.attr_count != 1 || vtr_tx_attr(tx, 0, &key, &phase, &v) != VTR_OK) return 1;
        if (v.tag != VTR_VAL_U64 || v.u != 0x1000) return 1;
        if (info.stage_count != 2) return 1;
    }
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
    uint32_t top = vtr_writer_begin_scope(w, "top", 0, "top");
    ASSERT(top != VTR_NONE);
    uint32_t clk, clk_n, bus, bus_n, r, r_n, s, s_n, wide, wide_n;
    CHECK(vtr_writer_add_var(w, "clk", 16, 1, 0, 1, 4, &clk_n, &clk));
    CHECK(vtr_writer_add_var(w, "bus", 5, 0, 0, 16, 4, &bus_n, &bus));
    CHECK(vtr_writer_add_var(w, "r", 3, 0, 1, 0, 0, &r_n, &r));
    CHECK(vtr_writer_add_var(w, "s", 21, 0, 2, 0, 0, &s_n, &s));
    CHECK(vtr_writer_add_var(w, "wide", 23, 0, 0, 96, 4, &wide_n, &wide));
    vtr_value av; memset(&av, 0, sizeof av); av.tag = VTR_VAL_I64; av.i = -42;
    CHECK(vtr_writer_node_attr(w, bus_n, "msb", &av));
    CHECK(vtr_writer_end_scope(w));
    uint32_t stream = vtr_writer_add_stream(w, VTR_NONE, "pipe", "PIPELINE");
    uint32_t gen = vtr_writer_add_generator(w, stream, "instruction");
    uint32_t k_pc = vtr_writer_intern(w, "pc");
    uint32_t lane = vtr_writer_intern(w, "0");
    uint32_t st_f = vtr_writer_intern(w, "F");
    uint32_t st_x = vtr_writer_intern(w, "X");
    uint32_t dep = vtr_writer_intern(w, "dep");
    uint64_t prev = 0;
    for (uint64_t i = 0; i < 1000; i++) {
        CHECK(vtr_writer_set_time(w, i * 10));
        CHECK(vtr_writer_emit_bit(w, clk, (uint8_t)(i & 1)));
        if (i % 4 == 0) CHECK(vtr_writer_emit_u64(w, bus, i * 3));
        if (i % 10 == 0) CHECK(vtr_writer_emit_real(w, r, (double)i / 4.0));
        if (i % 50 == 0) { char buf[32]; snprintf(buf, sizeof buf, "s%llu", (unsigned long long)i); CHECK(vtr_writer_emit_varlen(w, s, (const uint8_t *)buf, strlen(buf))); }
        if (i % 7 == 0) { uint32_t words[3] = { (uint32_t)i, 0xdeadbeef, 0xff }; CHECK(vtr_writer_emit_words(w, wide, words, 3)); }
        if (i == 500) CHECK(vtr_writer_emit_logic_str(w, bus, "xxxx000011110000", SIZE_MAX));
        if (i % 100 == 0) {
            uint64_t tx;
            CHECK(vtr_writer_begin_tx(w, gen, i * 10, &tx));
            vtr_value pc; memset(&pc, 0, sizeof pc); pc.tag = VTR_VAL_U64; pc.u = 0x1000 + i;
            CHECK(vtr_writer_tx_attr(w, tx, k_pc, 0, &pc));
            CHECK(vtr_writer_tx_stage_begin(w, tx, st_f, lane, i * 10));
            CHECK(vtr_writer_tx_stage_begin(w, tx, st_x, lane, i * 10 + 5));
            if (prev) CHECK(vtr_writer_relate(w, dep, prev, tx, 0, NULL, NULL));
            CHECK(vtr_writer_end_tx(w, tx, i * 10 + 20, 0));
            prev = tx;
        }
    }
    CHECK(vtr_writer_close(w));

    vtr_reader *rd = vtr_reader_open(path);
    ASSERT(rd != NULL);
    vtr_meta m;
    CHECK(vtr_reader_meta(rd, &m));
    ASSERT(m.timescale == -12);
    ASSERT(m.signal_count == 5);
    ASSERT(m.tx_count == 10);
    ASSERT(m.relation_count == 9);
    ASSERT(m.has_time_range && m.time_end == 9990);
    uint32_t sig;
    CHECK(vtr_reader_find_signal(rd, "top.bus", '.', &sig));
    ASSERT(sig == bus);
    vtr_node_info ni;
    CHECK(vtr_reader_node(rd, bus_n, &ni));
    ASSERT(ni.kind == 2 && ni.type_code == 5 && ni.signal == bus && ni.attr_count == 1);
    uint32_t key; vtr_value v;
    CHECK(vtr_reader_node_attr(rd, bus_n, 0, &key, &v));
    size_t klen; const char *ks = vtr_reader_str(rd, key, &klen);
    ASSERT(klen == 3 && memcmp(ks, "msb", 3) == 0 && v.tag == VTR_VAL_I64 && v.i == -42);
    uint32_t kids[16];
    ASSERT(vtr_reader_children(rd, VTR_NONE, kids, 16) == 2);
    ASSERT(vtr_reader_children(rd, top, kids, 16) == 5);
    vtr_value_buf *b = vtr_value_buf_new();
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
    CHECK(vtr_reader_value_at(rd, wide, 70, b));
    ASSERT(strcmp(vtr_value_buf_ascii(b) + 96 - 32, "00000000000000000000000000000111") == 0);
    int n = 0;
    CHECK(vtr_reader_changes(rd, clk, 0, 95, count_changes, &n));
    ASSERT(n == 10);
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
    vtr_value_buf_free(b);
    vtr_reader_close(rd);
    ASSERT(vtr_signal_data_len(survivor) == 251);
    ASSERT(vtr_signal_data_times(survivor)[1] == 40);
    CHECK(vtr_signal_data_get(survivor, 1, &sv));
    ASSERT(sv.kind == 0 && sv.width == 16);
    vtr_signal_data_free(survivor);
    /* error paths */
    ASSERT(vtr_reader_open("/nonexistent/file.vtr") == NULL);
    ASSERT(strlen(vtr_last_error()) > 0);
    printf("c_smoke: ok\n");
    return 0;
}
