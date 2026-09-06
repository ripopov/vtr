// Real simulator snapshots for the RTL VDB integration tests.
#include "Vtop.h"
#include "verilated.h"
#include "verilated_vtr_c.h"
#include <cstdlib>

int main(int argc, char** argv) {
    VerilatedContext context;
    context.traceEverOn(true);
    Vtop top{&context, argc > 2 ? argv[2] : "TOP"};
    VerilatedVtrC trace;
    top.trace(&trace, 99);
    trace.open(argv[1]);
    for (unsigned t = 0; t <= 30; ++t) {
        context.time(t);
        top.clk = (t % 10) >= 5;
#if defined(VDB_PIPELINE)
        top.rst_n = t < 2 || t >= 4;
        top.en = t < 12 || t >= 22;
        top.sel = t < 12;
        top.a = 3;
        top.b = 9;
#elif defined(VDB_SEMANTICS)
        top.rst = t < 4;
        top.en = t < 12 || t >= 22;
        top.sel = t >= 12;
        top.a = 3;
        top.b = 9;
#elif defined(VDB_NETLIST)
        top.select = t < 12;
        top.a = 3;
        top.b = 9;
        top.wide[0] = 0x12345678;
        top.wide[1] = 0xabcdef01;
        top.wide[2] = 0xdeadbeef;
#elif defined(VDB_OPERATORS)
        top.a = t < 12 ? 0x79 : 3;
        top.b = 3;
        top.index = t < 12 ? 2 : 1;
#else
        top.a = 3;
        top.sel = 0;
#endif
        top.eval();
        trace.dump(t);
    }
    top.final();
    trace.close();
}
