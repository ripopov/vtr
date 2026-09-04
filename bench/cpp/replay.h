/* Loader for VTR benchmark replay files (see crates/vtr-bench/src/replay.rs). C99. */
#ifndef VTR_BENCH_REPLAY_H
#define VTR_BENCH_REPLAY_H
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <time.h>

typedef struct { uint8_t kind, states; uint32_t width; } rp_sig;
typedef struct { uint8_t op; uint32_t sig; char *name; } rp_hier;
typedef struct { uint64_t time; uint32_t sig; uint8_t form; uint32_t off, len; } rp_rec;
typedef struct {
    int8_t timescale;
    uint32_t n_sig, n_hier;
    uint64_t n_rec, end_time;
    rp_sig *sigs;
    rp_hier *hier;
    rp_rec *recs;
    uint8_t *heap;
    size_t heap_len;
} rp_replay;

static uint64_t rp_varint(const uint8_t *b, size_t *p) {
    uint64_t r = 0; int s = 0;
    for (;;) { uint8_t x = b[(*p)++]; r |= (uint64_t)(x & 0x7f) << s; if (x < 0x80) return r; s += 7; }
}

static int rp_load(const char *path, rp_replay *rp) {
    FILE *f = fopen(path, "rb");
    if (!f) { perror(path); return -1; }
    fseek(f, 0, SEEK_END); long len = ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *d = (uint8_t *)malloc(len);
    if (fread(d, 1, len, f) != (size_t)len) { fclose(f); return -1; }
    fclose(f);
    if (memcmp(d, "VTRRPL01", 8) != 0) { fprintf(stderr, "not a replay file\n"); return -1; }
    memset(rp, 0, sizeof *rp);
    rp->timescale = (int8_t)d[8];
    memcpy(&rp->n_sig, d + 9, 4); memcpy(&rp->n_hier, d + 13, 4);
    memcpy(&rp->n_rec, d + 17, 8); memcpy(&rp->end_time, d + 25, 8);
    size_t p = 33;
    rp->sigs = (rp_sig *)malloc(sizeof(rp_sig) * rp->n_sig);
    for (uint32_t i = 0; i < rp->n_sig; i++) { rp->sigs[i].kind = d[p]; rp->sigs[i].states = d[p + 1]; memcpy(&rp->sigs[i].width, d + p + 2, 4); p += 6; }
    rp->hier = (rp_hier *)malloc(sizeof(rp_hier) * rp->n_hier);
    for (uint32_t i = 0; i < rp->n_hier; i++) {
        uint8_t op = d[p++]; rp->hier[i].op = op; rp->hier[i].sig = 0; rp->hier[i].name = NULL;
        if (op == 1 || op == 3 || op == 4) {
            if (op != 1) rp->hier[i].sig = (uint32_t)rp_varint(d, &p);
            size_t l = rp_varint(d, &p);
            rp->hier[i].name = (char *)malloc(l + 1); memcpy(rp->hier[i].name, d + p, l); rp->hier[i].name[l] = 0; p += l;
        }
    }
    rp->recs = (rp_rec *)malloc(sizeof(rp_rec) * rp->n_rec);
    rp->heap = (uint8_t *)malloc(len - p + 16);
    uint64_t t = 0; size_t hp = 0;
    for (uint64_t i = 0; i < rp->n_rec; i++) {
        t += rp_varint(d, &p);
        uint32_t sig = (uint32_t)rp_varint(d, &p);
        uint8_t form = d[p++];
        size_t l;
        if (form == 0) l = (rp->sigs[sig].width + 7) / 8; else if (form == 2) l = 8; else l = rp_varint(d, &p);
        rp->recs[i].time = t; rp->recs[i].sig = sig; rp->recs[i].form = form; rp->recs[i].off = (uint32_t)hp; rp->recs[i].len = (uint32_t)l;
        memcpy(rp->heap + hp, d + p, l); hp += l; p += l;
    }
    rp->heap_len = hp;
    free(d);
    return 0;
}

static double rp_wall(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); return ts.tv_sec + ts.tv_nsec * 1e-9; }
static double rp_cpu(void) { struct rusage u; getrusage(RUSAGE_SELF, &u); return u.ru_utime.tv_sec + u.ru_utime.tv_usec * 1e-6 + u.ru_stime.tv_sec + u.ru_stime.tv_usec * 1e-6; }
static long rp_file_size(const char *path) { FILE *f = fopen(path, "rb"); if (!f) return 0; fseek(f, 0, SEEK_END); long l = ftell(f); fclose(f); return l; }

/* Expands packed 2-state LE bytes to an ASCII string (MSB first). */
static void rp_to_ascii(const uint8_t *packed, uint32_t width, char *out) {
    for (uint32_t i = 0; i < width; i++) out[i] = '0' + ((packed[(width - 1 - i) >> 3] >> ((width - 1 - i) & 7)) & 1);
    out[width] = 0;
}
#endif
