/* Generate the local large_fst.fst stress trace for manual performance testing.
 *
 * One sample per 1 ns tick for every signal. Three 32-bit signed sine waves
 * with 1M, 10M and 100M samples (periods of 1,000, 20,000 and 1,000,000
 * samples, amplitude 2^30 - 1; each holds its last value once its samples
 * end) and a 1-bit clock toggling on each of 100M ticks.
 *
 * From the repository root:
 * cc -O2 volna/volna/examples/generate_large_fst.c \
 *   ext/libfstwriter/integration_test/verilator_share/gtkwave/fstapi.c \
 *   -Iext/libfstwriter/integration_test/verilator_share/gtkwave \
 *   $(pkg-config --cflags --libs liblz4 zlib) -lm -o /tmp/generate-large-fst
 * /tmp/generate-large-fst volna/volna/examples/large_fst.fst
 */
#include "fstapi.h"
#include <assert.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>

#define TICKS 100000000ULL

struct sine {
    const char *name;
    uint64_t samples;
    double period;
    fstHandle handle;
};

int main(int argc, char **argv) {
    assert(argc == 2);
    fstWriterContext *w = fstWriterCreate(argv[1], 1);
    assert(w);
    fstWriterSetPackType(w, FST_WR_PT_LZ4);
    fstWriterSetTimescale(w, -9);
    fstWriterSetScope(w, FST_ST_VCD_MODULE, "top", "top");
    fstHandle clk = fstWriterCreateVar(w, FST_VT_VCD_WIRE, FST_VD_OUTPUT, 1, "clk", 0);
    struct sine sines[] = {
        {"sine_1m", 1000000ULL, 1000.0, 0},
        {"sine_10m", 10000000ULL, 20000.0, 0},
        {"sine_100m", 100000000ULL, 1000000.0, 0},
    };
    for (int i = 0; i < 3; i++)
        sines[i].handle = fstWriterCreateVar(w, FST_VT_VCD_INTEGER, FST_VD_OUTPUT, 32,
                                             sines[i].name, 0);
    fstWriterSetUpscope(w);
    const double amplitude = 1073741823.0;
    const double tau = 6.283185307179586;
    for (uint64_t t = 0; t < TICKS; t++) {
        fstWriterEmitTimeChange(w, t);
        fstWriterEmitValueChange(w, clk, (t & 1) ? "0" : "1");
        for (int i = 0; i < 3; i++) {
            if (t >= sines[i].samples) continue;
            int32_t v = (int32_t)lrint(amplitude * sin(tau * (double)t / sines[i].period));
            fstWriterEmitValueChange32(w, sines[i].handle, 32, (uint32_t)v);
        }
        if (t % 10000000ULL == 0) {
            fprintf(stderr, "\r%3llu%%", (unsigned long long)(t * 100 / TICKS));
        }
    }
    fstWriterEmitTimeChange(w, TICKS);
    fstWriterClose(w);
    fprintf(stderr, "\rdone\n");
    return 0;
}
