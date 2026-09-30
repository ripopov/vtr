// The simulation of integrations/verilator/crash/run.py: a standard main loop
// with a VTR trace, which dies at +crash=<cycle> as +mode=<how> says:
// segv, abort, stack (unbounded recursion), exit (exit(3)) or term (SIGTERM to
// itself while it runs; the loop ends on gotFinish, which the crash guard sets).
// The trace opts into the crash guard unless +noguard is given.
#include "Vtop.h"
#include "verilated.h"
#include "verilated_vtr_c.h"

#include <csignal>
#include <cstdlib>
#include <cstring>
#include <string>
#include <sys/resource.h>
#include <unistd.h>

namespace {
std::string g_mode;
uint32_t g_crash = 0xffffffffu;

#pragma GCC diagnostic ignored "-Winfinite-recursion"
int recurse(int n) {
    volatile char pad[4096];
    pad[0] = char(n);
    return recurse(n + 1) + pad[0];
}
}  // namespace

extern "C" void maybe_crash(int cycle) {
    if (static_cast<uint32_t>(cycle) != g_crash) return;
    if (g_mode == "segv") *static_cast<volatile int*>(nullptr) = 1;
    if (g_mode == "abort") abort();
    if (g_mode == "stack") recurse(0);
    if (g_mode == "exit") exit(3);
    if (g_mode == "term") kill(getpid(), SIGTERM);
}

int main(int argc, char** argv) {
    const rlimit noCore{0, 0};
    setrlimit(RLIMIT_CORE, &noCore);
    VerilatedContext context;
    context.commandArgs(argc, argv);
    context.traceEverOn(true);
    const char* arg = context.commandArgsPlusMatch("mode=");
    if (arg[0]) g_mode = arg + std::strlen("+mode=");
    arg = context.commandArgsPlusMatch("crash=");
    if (arg[0]) g_crash = std::strtoul(arg + std::strlen("+crash="), nullptr, 10);
    uint64_t cycles = 20000;
    arg = context.commandArgsPlusMatch("cycles=");
    if (arg[0]) cycles = std::strtoull(arg + std::strlen("+cycles="), nullptr, 10);
    Vtop model{&context};
    VerilatedVtrC trace;
    trace.guard(!context.commandArgsPlusMatch("noguard")[0]);
    model.trace(&trace, 99);
    trace.open("crash.vtr");
    for (uint64_t t = 0; t < 2 * cycles && !context.gotFinish(); ++t) {
        context.time(t);
        model.clk = t & 1;
        model.eval();
        trace.dump(t);
    }
    model.final();
    trace.close();
    return 0;
}
