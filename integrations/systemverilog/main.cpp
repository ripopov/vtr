// Runs tb under upstream Verilator with --vpi: the standalone sink writes the file
// named by +vtr_trace=. A VPI simulator reports the start and end of simulation
// itself; a Verilator harness forwards them.
#include "Vtb.h"
#include "verilated.h"
#include "verilated_vpi.h"

int main(int argc, char** argv) {
    VerilatedContext context;
    context.commandArgs(argc, argv);
    Vtb model{&context};
    VerilatedVpi::callCbs(cbStartOfSimulation);
    while (!context.gotFinish()) {
        model.eval();
        VerilatedVpi::callValueCbs();
        if (!model.eventsPending()) break;
        context.time(model.nextTimeSlot());
    }
    model.final();
    VerilatedVpi::callCbs(cbEndOfSimulation);
}
