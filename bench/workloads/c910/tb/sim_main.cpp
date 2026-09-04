// C++ harness for the openC910 CoreMark trace benchmark. Drives the core clock,
// optionally dumps every signal of the design after each clock edge to an FST or
// VTR file (whichever the model was Verilated for) and prints one JSON line:
//   {"result": "PASS", "cycles": N, "instret": N, "wall_s": s, "cpu_s": s,
//    "dump": "fst"|"vtr"|"none", "bytes": N}
// wall_s covers reset, the whole run and closing the dump; cpu_s is the process
// CPU time of the same interval (all threads).
//
//   Vtop [--dump=<file>] [--max-cycles=N]        (run in the directory holding inst.pat/data.pat)

#include <verilated.h>
#if VM_TRACE_FST
#include <verilated_fst_c.h>
using TraceFile = VerilatedFstC;
static const char* const kDumpKind = "fst";
#elif VM_TRACE_VTR
#include <verilated_vtr_c.h>
using TraceFile = VerilatedVtrC;
static const char* const kDumpKind = "vtr";
#else
using TraceFile = void;
static const char* const kDumpKind = "none";
#endif

#include <sys/resource.h>
#include <sys/stat.h>

#include <chrono>
#include <cinttypes>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>

#include "Vtop.h"

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

int main(int argc, char** argv, char**) {
    const char* dump = arg_str(argc, argv, "--dump");
    const char* mc = arg_str(argc, argv, "--max-cycles");
    const uint64_t max_cycles = mc ? std::strtoull(mc, nullptr, 0) : 5000000ULL;

    const std::unique_ptr<VerilatedContext> contextp{new VerilatedContext};
    contextp->debug(0);
    contextp->randReset(0);  // deterministic: uninitialised state reads 0
    contextp->traceEverOn(dump != nullptr);
    contextp->commandArgs(argc, argv);
    const std::unique_ptr<Vtop> top{new Vtop{contextp.get(), "TOP"}};

#if VM_TRACE_FST || VM_TRACE_VTR
    std::unique_ptr<TraceFile> tfp;
    if (dump) {
        tfp.reset(new TraceFile);
        top->trace(tfp.get(), 99);
    }
#else
    if (dump) {
        std::fprintf(stderr, "[sim] this model was built without tracing\n");
        return 2;
    }
#endif

    const auto t0 = std::chrono::steady_clock::now();
    const double cpu0 = cpu_seconds();
#if VM_TRACE_FST || VM_TRACE_VTR
    if (tfp) tfp->open(dump);
#endif
    // One time unit (100 ps, the testbench precision) per half clock period.
    auto step = [&](int clk) {
        top->clk = clk;
        top->eval();
        contextp->timeInc(1);
#if VM_TRACE_FST || VM_TRACE_VTR
        if (tfp) tfp->dump(contextp->time());
#endif
    };
    step(0);

    // Reset and program load, until the testbench releases reset.
    uint64_t boot = 0;
    while (!top->o_running && boot < 10000) {
        step(1);
        step(0);
        ++boot;
    }
    if (!top->o_running) {
        std::fprintf(stderr, "[sim] ERROR: reset never released\n");
        return 1;
    }
    uint64_t cycles = 0;
    while (!(top->o_pass || top->o_fail || contextp->gotFinish()) && cycles < max_cycles) {
        step(1);
        step(0);
        ++cycles;
    }
    const uint64_t rtl_cycles = top->o_cycles;
    const uint64_t instret = top->o_instret;
    top->final();
#if VM_TRACE_FST || VM_TRACE_VTR
    if (tfp) tfp->close();
#endif
    const double wall = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    const double cpu = cpu_seconds() - cpu0;

    const char* verdict = top->o_pass ? "PASS" : (top->o_fail ? "FAIL" : "TIMEOUT");
    std::fflush(stdout);
    std::printf("{\"result\": \"%s\", \"cycles\": %" PRIu64 ", \"instret\": %" PRIu64
                ", \"wall_s\": %.4f, \"cpu_s\": %.4f, \"dump\": \"%s\", \"bytes\": %" PRIu64 "}\n",
                verdict, rtl_cycles, instret, wall, cpu, dump ? kDumpKind : "none",
                file_size(dump));
    return top->o_pass ? 0 : 1;
}
