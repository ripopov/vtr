/* fstapi.c reader benchmark: open, hierarchy walk, value-at queries, streaming all changes.
 * Usage: fst_read <in.fst> <plan.txt>
 * plan.txt: line 1 = signal indices (0-based, space separated); line 2 = times. */
#include "replay.h"
#include "fstapi.h"

static uint64_t g_count = 0;
static void cb(void *user, uint64_t time, fstHandle h, const unsigned char *value) { (void)user; (void)time; (void)h; (void)value; g_count++; }
static void cbv(void *user, uint64_t time, fstHandle h, const unsigned char *value, uint32_t len) { (void)user; (void)time; (void)h; (void)value; (void)len; g_count++; }

static int read_list(FILE *f, uint64_t *out, int cap) {
    char line[1 << 16];
    if (!fgets(line, sizeof line, f)) return 0;
    int n = 0; char *p = line;
    while (n < cap) { char *e; unsigned long long v = strtoull(p, &e, 10); if (e == p) break; out[n++] = v; p = e; }
    return n;
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s <in.fst> <plan.txt>\n", argv[0]); return 2; }
    uint64_t sigs[1024], times[1024];
    FILE *pf = fopen(argv[2], "r"); if (!pf) { perror(argv[2]); return 1; }
    int ns = read_list(pf, sigs, 1024), nt = read_list(pf, times, 1024);
    fclose(pf);
    /* open */
    double best_open = 1e9;
    for (int i = 0; i < 3; i++) { double t = rp_wall(); fstReaderContext *c = fstReaderOpen(argv[1]); double d = rp_wall() - t; if (d < best_open) best_open = d; fstReaderClose(c); }
    fstReaderContext *ctx = fstReaderOpen(argv[1]);
    /* hierarchy walk */
    double t = rp_wall();
    uint64_t nvars = 0; size_t namebytes = 0;
    struct fstHier *h;
    while ((h = fstReaderIterateHier(ctx))) { if (h->htyp == FST_HT_VAR) { nvars++; namebytes += h->u.var.name_length; } }
    double t_hier = rp_wall() - t;
    /* value at time: ns signals x nt times, using the rvat API (fresh open, like a one-shot tool) */
    fstReaderClose(ctx);
    t = rp_wall();
    ctx = fstReaderOpen(argv[1]);
    char buf[1 << 16];
    uint64_t q = 0;
    for (int i = 0; i < nt; i++) for (int j = 0; j < (ns < 10 ? ns : 10); j++) { fstReaderGetValueFromHandleAtTime(ctx, times[i], (fstHandle)(sigs[j] + 1), buf); q++; }
    double t_value = rp_wall() - t;
    fstReaderClose(ctx);
    /* load N signals (mask + iterate) */
    double t_load[4]; int ks[4] = {1, 10, 100, 1000};
    for (int k = 0; k < 4; k++) {
        t = rp_wall();
        ctx = fstReaderOpen(argv[1]);
        fstReaderClrFacProcessMaskAll(ctx);
        for (int j = 0; j < ks[k] && j < ns; j++) fstReaderSetFacProcessMask(ctx, (fstHandle)(sigs[j] + 1));
        g_count = 0;
        fstReaderIterBlocks2(ctx, cb, cbv, NULL, NULL);
        t_load[k] = rp_wall() - t;
        fstReaderClose(ctx);
    }
    /* stream all */
    t = rp_wall();
    ctx = fstReaderOpen(argv[1]);
    fstReaderSetFacProcessMaskAll(ctx);
    g_count = 0;
    fstReaderIterBlocks2(ctx, cb, cbv, NULL, NULL);
    double t_all = rp_wall() - t;
    uint64_t all = g_count;
    fstReaderClose(ctx);
    printf("{\"reader\": \"fstapi\", \"open_s\": %.6f, \"hierarchy_s\": %.6f, \"vars\": %llu, \"value_at_s\": %.6f, \"queries\": %llu, "
           "\"load_1_s\": %.6f, \"load_10_s\": %.6f, \"load_100_s\": %.6f, \"load_1000_s\": %.6f, \"stream_all_s\": %.6f, \"changes\": %llu}\n",
           best_open, t_hier, (unsigned long long)nvars, t_value, (unsigned long long)q, t_load[0], t_load[1], t_load[2], t_load[3], t_all, (unsigned long long)all);
    (void)namebytes;
    return 0;
}
