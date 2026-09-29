// Stimulus for docs/vdb-fable/soc.sv: reset, program the DMA for a burst of
// three while the CPU holds the bus, let it run, then a second burst of one.
#include "Vtop.h"
#include "verilated.h"
#include "verilated_vtr_c.h"

int main(int argc, char** argv) {
    VerilatedContext context;
    context.traceEverOn(true);
    Vtop top{&context, "TOP"};
    VerilatedVtrC trace;
    top.trace(&trace, 99);
    trace.open(argc > 1 ? argv[1] : "trace.vtr");
    for (unsigned t = 0; t <= 400; ++t) {
        context.time(t);
        const unsigned cycle = t / 10;
        top.clk = (t % 10) >= 5;
        top.rst_n = t >= 12;
        // cycle 3: write CTRL = start | irq_en | burst 3; cycle 20: start | burst 1
        top.wen = cycle == 3 || cycle == 20;
        top.addr = (cycle == 7) ? 4 : (cycle == 12 ? 8 : 0);
        top.wdata = cycle == 3 ? 0xC3 : (cycle == 20 ? 0x81 : 0);
        top.cpu_req = cycle >= 4 && cycle <= 6;
        top.a = 0x10 + cycle;
        top.w = 0x1000 + cycle * 3;
        top.eval();
        trace.dump(t);
    }
    top.final();
    trace.close();
}
