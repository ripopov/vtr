// Baseline: a plain text log written with fprintf through a 1 MiB stdio buffer.
// This is what most simulators do today and the input CLP compresses.
//   log_text <n> <out.log>
#include "bench_common.hpp"
#include "workload.hpp"

#include <cstdio>

using namespace simlog;

int main(int argc, char **argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage: %s <n> <out.log>\n", argv[0]);
        return 2;
    }
    uint64_t n = benchutil::parse_count(argv[1]);
    FILE *f = std::fopen(argv[2], "wb");
    if (!f) return 1;
    static char buf[1 << 20];
    std::setvbuf(f, buf, _IOFBF, sizeof buf);
    Generator gen;
    Msg m{};
    double t0 = benchutil::now();
    for (uint64_t i = 0; i < n; ++i) {
        gen.next(m);
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                                                        \
    case id:                                                                                     \
        std::fprintf(f, "%" PRIu64 " %-5s " pfmt "\n", m.t, sev_name(SEV), __VA_ARGS__);         \
        break;
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
    }
    double t1 = benchutil::now();
    std::fclose(f);
    double t2 = benchutil::now();
    benchutil::Result r{"text"};
    r.n = n;
    r.loop_s = t1 - t0;
    r.total_s = t2 - t0;
    r.cpu_s = benchutil::cpu_seconds();
    r.bytes = benchutil::file_size(argv[2]);
    benchutil::print_json(r);
    return 0;
}
