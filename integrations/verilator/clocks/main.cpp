// Runs tb with a VTR trace. +late_open opens the file at 5 ns, after clocks were
// declared and started; +nosignals opens it without registering the model's signals.
#include "Vtb.h"
#include "verilated.h"
#include "verilated_vtr_c.h"

int main(int argc, char** argv) {
    VerilatedContext context;
    context.commandArgs(argc, argv);
    context.traceEverOn(true);
    const bool lateOpen = context.commandArgsPlusMatch("late_open")[0];
    const bool noSignals = context.commandArgsPlusMatch("nosignals")[0];
    Vtb model{&context};
    VerilatedVtrC trace;
    if (!noSignals) model.trace(&trace, 99);
    if (!lateOpen) trace.open("clocks.vtr");
    while (!context.gotFinish()) {
        if (lateOpen && !trace.isOpen() && context.time() >= 5000) trace.open("clocks.vtr");
        model.eval();
        if (trace.isOpen()) trace.dump(context.time());
        if (!model.eventsPending()) break;
        context.time(model.nextTimeSlot());
    }
    model.final();
    trace.close();
}
