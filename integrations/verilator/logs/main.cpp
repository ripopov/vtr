#include "Vtop.h"
#include "verilated.h"
#include "verilated_vtr_c.h"
#include <cstdlib>
#include <memory>
#include <string_view>

int main(int argc, char** argv) {
    if (argc != 4) return 2;
    const auto mode = std::strtoul(argv[2], nullptr, 10);
    auto context = std::make_unique<VerilatedContext>();
    context->commandArgs(argc, argv);
    context->traceEverOn(true);
    context->fatalOnError(mode == 2);
    Vtop model{context.get()};
    vtr::WriterOptions options;
    if (std::string_view{argv[3]} == "inline") options.mode = vtr::ExecutionMode::inline_;
    VerilatedVtrC trace{options};
    model.trace(&trace, 99);
    trace.open(argv[1]);
    for (unsigned time = 0; time < 8 && !context->gotFinish(); ++time) {
        context->time(time);
        model.mode = mode;
        model.clk = time & 1;
        model.eval();
        trace.dump(time);
        if (mode == 6 && time == 3) trace.dump(time);
    }
    model.final();
    trace.close();
}
