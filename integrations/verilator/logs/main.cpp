#include "Vtop.h"
#include "verilated.h"
#include "verilated_vtr_c.h"
int main(int argc, char** argv) {
    VerilatedContext context;
    context.commandArgs(argc, argv);
    context.traceEverOn(true);
    context.fatalOnError(true);
    Vtop model{&context};
    VerilatedVtrC trace;
    model.trace(&trace, 99);
    std::unique_ptr<VerilatedContext> other;
    if (context.commandArgsPlusMatch("context")[0]) other.reset(new VerilatedContext);
    trace.open("logs.vtr");
    trace.open("logs.vtr");  // An already-open trace must not add a second log sink.
    Verilated::threadContextp(&context);
    for (uint64_t t = 0; t < 8 && !context.gotFinish(); ++t) {
        context.time(t);
        model.clk = t & 1;
        model.eval();
        trace.dump(t);
        if (t == 3 && context.commandArgsPlusMatch("repeat")[0]) trace.dump(t);
    }
    model.final();
    trace.close();
}
