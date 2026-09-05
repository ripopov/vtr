// binlog: structured binary log; call sites are registered lazily as event
// sources, events carry the source id, a clock value (here: simulation time)
// and the raw arguments. A consumer thread moves the queue to the file.
//   log_binlog <n> <out.blog>
#include "bench_common.hpp"
#include "workload.hpp"

#include <binlog/binlog.hpp>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <fstream>
#include <thread>

using namespace simlog;

static binlog::Severity sev_of(uint8_t s) {
    switch (s) {
    case DEBUG: return binlog::Severity::debug;
    case INFO: return binlog::Severity::info;
    case WARN: return binlog::Severity::warning;
    default: return binlog::Severity::error;
    }
}

int main(int argc, char **argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage: %s <n> <out.blog>\n", argv[0]);
        return 2;
    }
    uint64_t n = benchutil::parse_count(argv[1]);
    std::ofstream out(argv[2], std::ofstream::out | std::ofstream::binary);
    binlog::Session session;
    // Clock values are simulation nanoseconds; tell bread they are ns since 0.
    session.setClockSync(binlog::ClockSync{0, 1'000'000'000, 0, 0, "UTC"});
    binlog::SessionWriter writer(session, 1 << 20);
    std::atomic<bool> stop{false};
    std::thread consumer([&] {
        while (!stop.load(std::memory_order_relaxed)) {
            session.consume(out);
            std::this_thread::sleep_for(std::chrono::microseconds(100));
        }
        session.consume(out);
    });
    Generator gen;
    Msg m{};
    double t0 = benchutil::now();
    for (uint64_t i = 0; i < n; ++i) {
        gen.next(m);
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                                                        \
    case id:                                                                                     \
        BINLOG_CREATE_SOURCE_AND_EVENT(writer, sev_of(SEV), sim, m.t, bfmt, __VA_ARGS__);        \
        break;
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
    }
    double t1 = benchutil::now();
    stop.store(true);
    consumer.join();
    out.close();
    double t2 = benchutil::now();
    benchutil::Result r{"binlog"};
    r.n = n;
    r.loop_s = t1 - t0;
    r.total_s = t2 - t0;
    r.cpu_s = benchutil::cpu_seconds();
    r.bytes = benchutil::file_size(argv[2]);
    benchutil::print_json(r);
    return 0;
}
