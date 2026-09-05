// Compresses a whole file with zstd (default level 3) and reports the size and
// time: the "+zstd" column for the loggers that write uncompressed output.
//   log_zstd <file> [level]
#include "bench_common.hpp"

#include <zstd.h>

#include <cstdio>
#include <cstdlib>
#include <vector>

int main(int argc, char **argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <file> [level]\n", argv[0]);
        return 2;
    }
    int level = argc > 2 ? std::atoi(argv[2]) : 3;
    FILE *f = std::fopen(argv[1], "rb");
    if (!f) return 1;
    std::vector<char> in;
    static char buf[1 << 20];
    size_t k;
    while ((k = std::fread(buf, 1, sizeof buf, f)) > 0) in.insert(in.end(), buf, buf + k);
    std::fclose(f);
    // Streaming compression in 4 MiB inputs, like a logger flushing periodically.
    double t0 = benchutil::now();
    ZSTD_CStream *cs = ZSTD_createCStream();
    ZSTD_initCStream(cs, level);
    std::vector<char> ob(ZSTD_CStreamOutSize());
    uint64_t total = 0;
    size_t pos = 0;
    while (pos < in.size() || pos == 0) {
        size_t n = std::min<size_t>(in.size() - pos, 4u << 20);
        ZSTD_inBuffer zin{in.data() + pos, n, 0};
        bool end = pos + n >= in.size();
        for (;;) {
            ZSTD_outBuffer zo{ob.data(), ob.size(), 0};
            size_t rem = ZSTD_compressStream2(cs, &zo, &zin, end ? ZSTD_e_end : ZSTD_e_continue);
            total += zo.pos;
            if (end ? rem == 0 : zin.pos == zin.size) break;
        }
        pos += n;
        if (n == 0) break;
    }
    ZSTD_freeCStream(cs);
    double t1 = benchutil::now();
    std::printf("{\"bytes\":%llu,\"zstd_s\":%.6f,\"level\":%d}\n", (unsigned long long)total, t1 - t0, level);
    return 0;
}
