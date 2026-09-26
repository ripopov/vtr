// Runs tb with a VTR trace in pipeline.vtr. +nosignals opens the file without
// registering the model's signals: the recording holds clocks, transactions and log.
#include "Vtb.h"
#include "verilated.h"
#include "verilated_vtr_c.h"

int main(int argc, char** argv) {
    VerilatedContext context;
    context.commandArgs(argc, argv);
    context.traceEverOn(true);
    const bool noSignals = context.commandArgsPlusMatch("nosignals")[0];
    Vtb model{&context};
    VerilatedVtrC trace;
    if (!noSignals) model.trace(&trace, 99);
    trace.open("pipeline.vtr");
    while (!context.gotFinish()) {
        model.eval();
        trace.dump(context.time());
        if (!model.eventsPending()) break;
        context.time(model.nextTimeSlot());
    }
    model.final();
    trace.close();
}
