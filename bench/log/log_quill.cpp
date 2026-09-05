// Quill: asynchronous text logger (arguments encoded on the caller's thread,
// formatted by the backend thread into a text file). The simulation time is
// the first argument of every message; the level comes from the pattern.
//   log_quill <n> <out.log>
#include "bench_common.hpp"
#include "workload.hpp"

#include "quill/Backend.h"
#include "quill/Frontend.h"
#include "quill/LogMacros.h"
#include "quill/Logger.h"
#include "quill/sinks/FileSink.h"

#include <cstdio>
#include <string>

using namespace simlog;

int main(int argc, char **argv) {
    if (argc < 3) {
        std::fprintf(stderr, "usage: %s <n> <out.log>\n", argv[0]);
        return 2;
    }
    uint64_t n = benchutil::parse_count(argv[1]);
    quill::BackendOptions bo;
    quill::Backend::start(bo);
    auto sink = quill::Frontend::create_or_get_sink<quill::FileSink>(
        argv[2],
        []() {
            quill::FileSinkConfig cfg;
            cfg.set_open_mode('w');
            return cfg;
        }(),
        quill::FileEventNotifier{});
    quill::Logger *logger = quill::Frontend::create_or_get_logger(
        "sim", std::move(sink), quill::PatternFormatterOptions{"%(log_level:<5) %(message)", "%H:%M:%S.%Qns", quill::Timezone::GmtTime, false});
    logger->set_log_level(quill::LogLevel::Debug);
    LOG_INFO(logger, "{} start", uint64_t(0));
    logger->flush_log();
    Generator gen;
    Msg m{};
    double t0 = benchutil::now();
    for (uint64_t i = 0; i < n; ++i) {
        gen.next(m);
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                                    \
    case id:                                                                 \
        QUILL_LOG_##SEV(logger, "{} " ffmt, m.t, __VA_ARGS__);              \
        break;
#define QUILL_LOG_DEBUG_ QUILL_LOG_DEBUG
#define QUILL_LOG_WARN(...) QUILL_LOG_WARNING(__VA_ARGS__)
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
    }
    double t1 = benchutil::now();
    logger->flush_log();
    quill::Backend::stop();
    double t2 = benchutil::now();
    benchutil::Result r{"quill"};
    r.n = n;
    r.loop_s = t1 - t0;
    r.total_s = t2 - t0;
    r.cpu_s = benchutil::cpu_seconds();
    r.bytes = benchutil::file_size(argv[2]);
    benchutil::print_json(r);
    return 0;
}
