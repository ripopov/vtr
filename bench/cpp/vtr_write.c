/* Replays a workload into VTR through the C API. Usage: vtr_write <in.rpl> <out.vtr> [zstd|lz4|none] [background|inline] */
#include "replay.h"
#include "vtr.h"

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s <in.rpl> <out.vtr> [zstd|lz4|none] [background|inline]\n", argv[0]); return 2; }
    rp_replay rp;
    if (rp_load(argv[1], &rp)) return 1;
    vtr_writer_options o; vtr_writer_options_default(&o);
    if (argc > 3) o.codec = strcmp(argv[3], "lz4") == 0 ? 1 : strcmp(argv[3], "none") == 0 ? 0 : 2;
    if (argc > 4 && strcmp(argv[4], "inline") == 0) o.background = 0;
    uint32_t *sig = (uint32_t *)calloc(rp.n_sig, 4);
    uint8_t *declared = (uint8_t *)calloc(rp.n_sig, 1);
    double w0 = rp_wall(), c0 = rp_cpu();
    vtr_writer *w = vtr_writer_create(argv[2], &o);
    if (!w) { fprintf(stderr, "%s\n", vtr_last_error()); return 1; }
    vtr_writer_set_timescale(w, rp.timescale);
    /* Open scopes, innermost last; the replay's scopes are balanced. */
    uint32_t *path = (uint32_t *)calloc(rp.n_hier + 1, 4), depth = 0;
    for (uint32_t i = 0; i < rp.n_hier; i++) {
        rp_hier *h = &rp.hier[i];
        uint32_t parent = depth ? path[depth - 1] : VTR_NONE;
        if (h->op == 1) path[depth++] = vtr_writer_add_scope(w, parent, h->name, 0, NULL);
        else if (h->op == 2) depth--;
        else if (h->op == 3) { rp_sig *s = &rp.sigs[h->sig]; uint32_t node; vtr_writer_add_var(w, parent, h->name, 16, 0, s->kind, s->width, s->states, &node, &sig[h->sig]); declared[h->sig] = 1; }
        else vtr_writer_add_alias(w, parent, h->name, 16, 0, sig[h->sig]);
    }
    free(path);
    for (uint32_t s = 0; s < rp.n_sig; s++) if (!declared[s]) { char nm[32]; snprintf(nm, sizeof nm, "s%u", s); uint32_t node; vtr_writer_add_var(w, VTR_NONE, nm, 16, 0, rp.sigs[s].kind, rp.sigs[s].width, rp.sigs[s].states, &node, &sig[s]); }
    uint64_t last = UINT64_MAX, n = 0;
    int rc = 0;
    for (uint64_t i = 0; i < rp.n_rec; i++) {
        rp_rec *r = &rp.recs[i];
        if (r->time != last) { rc |= vtr_writer_set_time(w, r->time); last = r->time; }
        const uint8_t *p = rp.heap + r->off;
        uint32_t wd = rp.sigs[r->sig].width;
        switch (r->form) {
        case 0:
            if (wd <= 64) { uint64_t v = 0; memcpy(&v, p, r->len); rc |= vtr_writer_emit_u64(w, sig[r->sig], v); }
            else rc |= vtr_writer_emit_packed(w, sig[r->sig], 2, p, r->len);
            break;
        case 1: rc |= vtr_writer_emit_logic_str(w, sig[r->sig], (const char *)p, r->len); break;
        case 2: { double d; memcpy(&d, p, 8); rc |= vtr_writer_emit_real(w, sig[r->sig], d); break; }
        default: rc |= vtr_writer_emit_varlen(w, sig[r->sig], p, r->len); break;
        }
        n++;
        if (rc) { fprintf(stderr, "error: %s\n", vtr_last_error()); return 1; }
    }
    if (rp.end_time != last) vtr_writer_set_time(w, rp.end_time);
    rc = vtr_writer_close(w);
    if (rc) { fprintf(stderr, "close: %s\n", vtr_last_error()); return 1; }
    double wall = rp_wall() - w0, cpu = rp_cpu() - c0;
    printf("{\"writer\": \"vtr-c%s\", \"records\": %llu, \"wall_s\": %.6f, \"cpu_s\": %.6f, \"bytes\": %ld, \"changes_per_s\": %.1f}\n",
           o.background ? "" : "-inline", (unsigned long long)n, wall, cpu, rp_file_size(argv[2]), n / wall);
    return 0;
}
