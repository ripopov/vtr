#include "Vtop.h"
#include "verilated.h"
#if VM_TRACE_VTR
#include "verilated_vtr_c.h"
using Trace = VerilatedVtrC;
#else
#include "verilated_fst_c.h"
using Trace = VerilatedFstC;
#endif

int main(int argc, char** argv) {
    if (argc != 2) return 2;
    Verilated::commandArgs(argc, argv);
    Verilated::traceEverOn(true);
    Vtop model;
    Trace trace;
    model.trace(&trace, 99);
    trace.open(argv[1]);
    for (unsigned t = 0; t < 8; ++t) {
        model.clk = t & 1;
        model.d = t + 3;
        model.eval();
        trace.dump(t);
    }
    model.final();
    trace.close();
}
