// Verilator harness for the RSA256 design from ext/libfstwriter (MIT). Runs the
// benchmark driver (RSA_bench_tb.sv) for a number of cycles, optionally dumping
// every signal after each clock edge to the trace format the model was built
// for (--trace-fst or --trace-vtr), and prints one JSON line with the timing:
//   {"cycles": N, "wall_s": s, "cpu_s": s, "dump": "fst"|"vtr"|"none", "bytes": N}
//
//   rsa_tb [--dump=<file>] [--cycles=N]
#include "VRSA_tb.h"
#include <verilated.h>
#if VM_TRACE_FST
#include <verilated_fst_c.h>
using TraceFile = VerilatedFstC;
static const char* const kDumpKind = "fst";
#elif VM_TRACE_VTR
#include <verilated_vtr_c.h>
using TraceFile = VerilatedVtrC;
static const char* const kDumpKind = "vtr";
#endif

#include <sys/resource.h>
#include <sys/stat.h>

#include <chrono>
#include <cinttypes>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>

namespace {

const char* arg_str(int argc, char** argv, const char* key) {
    const size_t klen = std::strlen(key);
    for (int i = 1; i < argc; ++i) {
        if (!std::strncmp(argv[i], key, klen) && argv[i][klen] == '=') return argv[i] + klen + 1;
    }
    return nullptr;
}

double cpu_seconds() {
    struct rusage ru;
    getrusage(RUSAGE_SELF, &ru);
    return ru.ru_utime.tv_sec + ru.ru_utime.tv_usec * 1e-6 + ru.ru_stime.tv_sec
           + ru.ru_stime.tv_usec * 1e-6;
}

uint64_t file_size(const char* path) {
    struct stat st;
    return (path && stat(path, &st) == 0) ? static_cast<uint64_t>(st.st_size) : 0;
}

}  // namespace

int main(int argc, char** argv) {
    Verilated::commandArgs(argc, argv);
    const char* dump = arg_str(argc, argv, "--dump");
    const char* cy = arg_str(argc, argv, "--cycles");
    const long cycles = cy ? std::atol(cy) : 200000;
    std::unique_ptr<VRSA_tb> tb(new VRSA_tb);
#if VM_TRACE_FST || VM_TRACE_VTR
    std::unique_ptr<TraceFile> tfp;
    if (dump) {
        Verilated::traceEverOn(true);
        tfp.reset(new TraceFile);
        tb->trace(tfp.get(), 99);
    }
#else
    if (dump) {
        std::fprintf(stderr, "this model was built without tracing\n");
        return 2;
    }
#endif
    const auto t0 = std::chrono::steady_clock::now();
    const double cpu0 = cpu_seconds();
#if VM_TRACE_FST || VM_TRACE_VTR
    if (tfp) tfp->open(dump);
#endif
    vluint64_t t = 0;
    auto step = [&]() {
        tb->clk = !tb->clk;
        tb->eval();
#if VM_TRACE_FST || VM_TRACE_VTR
        if (tfp) tfp->dump(t);
#endif
        t++;
    };
    tb->clk = 0;
    tb->rst_n = 0;
    while (t < 20) step();
    tb->rst_n = 1;
    for (long i = 0; i < cycles * 2; i++) step();
    tb->final();
#if VM_TRACE_FST || VM_TRACE_VTR
    if (tfp) tfp->close();
#endif
    const double wall = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    const double cpu = cpu_seconds() - cpu0;
    std::printf("{\"cycles\": %ld, \"wall_s\": %.4f, \"cpu_s\": %.4f, \"dump\": \"%s\", \"bytes\": %" PRIu64 "}\n",
                cycles, wall, cpu,
#if VM_TRACE_FST || VM_TRACE_VTR
                dump ? kDumpKind : "none",
#else
                "none",
#endif
                file_size(dump));
    return 0;
}
