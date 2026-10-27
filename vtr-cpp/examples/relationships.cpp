#include "instruction.h"
#include "vtr_logging.h"

#include <iostream>

int main(int argc, char** argv) {
    namespace vt = vtr;
    try {
        auto path = argc > 1 ? argv[1] : "relationships.vtr";
        vt::Writer trace(path);
        auto core = trace.scope("core0");
        auto instructions =
            trace.transactions<Instruction>({.name = "instructions", .parent = core});
        auto logger = trace.logger({.name = "diagnostics"});
        auto messages =
            vtr::logging::declare<Instruction>(trace, {.format = "pc={0:#x} opcode={1}"});
        auto clock = trace.clock({.name = "clock", .parent = core});
        trace.associate_clock(instructions, clock);
        auto label = trace.intern("vtr.label");
        auto phase = trace.intern("decode");
        auto lane = trace.intern("pipeline");
        auto dependency = trace.intern("depends_on");

        auto stretch = trace.run(clock, 0, 2);
        auto parent = trace.begin_at(instructions, 10, Instruction{0x8000, 0x13});
        trace.attribute(parent, label, vt::Value(std::string("first instruction")));
        auto child =
            trace.begin_at(instructions, 12, Instruction{0x8004, 0x33}, {.parent = parent});
        auto detail = trace.embedded_stage(parent, 10, phase, lane);
        trace.end_embedded_stage(parent, detail, 12);
        trace.log_at(logger, messages, 13, Instruction{0x8000, 0x13}, parent);
        trace.relate(dependency, parent.id(), child.id());
        trace.end_at(child, 17, vt::Status::ok);
        trace.end_at(parent, 15, vt::Status::ok);
        trace.stop(clock, 20);
        trace.close();
        (void)stretch;

        vt::Reader input(path);
        std::cout << "Cycle at tick 14: " << *input.cycle_at(clock.id(), 14) << '\n';
        input.children(parent.id(), [](const vt::Record& record) {
            std::cout << "Child " << record.id().value << '\n';
            return vt::Flow::next;
        });
        input.logs(parent.id(), {}, [&](const vt::Record& record) {
            std::cout << vtr::logging::format_log(input, record) << '\n';
            return vt::Flow::next;
        });
        input.relations(parent.id(), vt::Direction::outgoing, [](const vt::Relation& relation) {
            std::cout << relation.from->value << " -> " << relation.to->value << '\n';
            return vt::Flow::next;
        });
        (void)input.validate();
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
