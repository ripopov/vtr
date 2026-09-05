// NanoLog (C++17 version): static format information extracted at compile
// time, arguments packed to their minimal byte width by a background thread.
// NanoLog stamps its own TSC time, so the simulation time is the first
// argument of every message.
//   log_nanolog <n> <out.nlog>
#include "bench_common.hpp"
#include "workload.hpp"

#include "NanoLogCpp17.h"

#include <cstdio>

using namespace simlog;

#define NL_DEBUG DEBUG
#define NL_INFO NOTICE
#define NL_WARN WARNING
#define NL_ERROR ERROR

int main(int argc, char **argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage: %s <n> <out.nlog>\n", argv[0]);
        return 2;
    }
    uint64_t n = benchutil::parse_count(argv[1]);
    std::remove(argv[2]);
    NanoLog::setLogFile(argv[2]);
    NanoLog::setLogLevel(NanoLog::LogLevels::DEBUG);
    NanoLog::preallocate();
    Generator gen;
    Msg m{};
    double t0 = benchutil::now();
    for (uint64_t i = 0; i < n; ++i) {
        gen.next(m);
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                                    \
    case id:                                                                 \
        NANO_LOG(NL_##SEV, "%" PRIu64 " " pfmt, m.t, __VA_ARGS__);          \
        break;
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
    }
    double t1 = benchutil::now();
    NanoLog::sync();
    double t2 = benchutil::now();
    benchutil::Result r{"nanolog"};
    r.n = n;
    r.loop_s = t1 - t0;
    r.total_s = t2 - t0;
    r.cpu_s = benchutil::cpu_seconds();
    r.bytes = benchutil::file_size(argv[2]);
    benchutil::print_json(r);
    return 0;
}
