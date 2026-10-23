#include "instruction.h"
#include "vtr_writer.h"

#include <iostream>

int main(int argc, char** argv) {
    namespace vt = vtr;
    try {
        auto path = argc > 1 ? argv[1] : "run.vtr";
        vt::Writer trace(path);
        auto core = trace.scope("core0");
        auto instructions = trace.transactions<Instruction>(
            {.name = "instructions", .parent = core}, "INSTRUCTION");
        auto execute = trace.stages({.name = "execute", .parent = core});
        auto retired = trace.events({.name = "retired", .parent = core});

        trace.advance_time(10);
        auto tx = trace.begin(instructions, Instruction{0x8000, 0x13});
        trace.advance_time(11);
        auto visit = trace.begin(execute, tx);
        trace.advance_time(14);
        trace.end(visit);
        trace.event(retired, tx);
        trace.advance_time(15);
        trace.end(tx, vt::Status::ok);
        trace.close();
        std::cout << "Wrote instruction " << tx.id().value << " to " << path << '\n';
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
