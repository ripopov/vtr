#include "instruction.h"
#include "vtr_logging.h"

#include <algorithm>
#include <iostream>
#include <optional>
#include <variant>

namespace vt = vtr;

void write_relationships(const char* path) {
    vt::Writer trace(path);
    auto core = trace.scope("core0");
    auto instructions = trace.transactions<Instruction>({.name = "instructions", .parent = core});
    auto logger = trace.logger({.name = "diagnostics"});
    auto messages =
        vtr::logging::declare<Instruction>(trace, {.format = "pc={0:#x} opcode={1:#x}"});
    auto clock = trace.clock({.name = "clock", .parent = core});
    trace.associate_clock(instructions, clock);
    auto label = trace.intern("vtr.label");
    auto phase = trace.intern("decode");
    auto lane = trace.intern("pipeline");
    auto dependency = trace.intern("depends_on");

    auto stretch = trace.run(clock, 0, 2);
    Instruction first{0x8000, 0x00100093};
    Instruction second{0x8004, 0x00108133};
    auto parent = trace.begin_at(instructions, 10, first);
    trace.attribute(parent, label, trace.intern("addi x1, x0, 1"));
    auto child = trace.begin_at(instructions, 12, second, {.parent = parent});
    trace.attribute(child, label, trace.intern("add x2, x1, x1"));
    auto detail = trace.embedded_stage(parent, 10, phase, lane);
    trace.end_embedded_stage(parent, detail, 12);
    trace.log_at(logger, messages, 13, first, parent);
    trace.relate(dependency, child.id(), parent.id());
    trace.end_at(child, 17, vt::Status::ok);
    trace.end_at(parent, 15, vt::Status::ok);
    trace.stop(clock, 20);
    trace.close();
    (void)stretch;
}

void print_instruction(const vt::Reader& input, const vt::Record& record) {
    auto instruction = record.payload().as<Instruction>();
    std::cout << "Instruction " << record.id().value;
    for (const auto& attribute : record.attributes()) {
        if (input.string(attribute.key) == "vtr.label") {
            if (const auto* label = std::get_if<vt::StringId>(&attribute.data.data)) {
                std::cout << " " << input.string(*label);
            }
        }
    }
    std::cout << " pc=0x" << std::hex << instruction.pc << " opcode=0x" << instruction.opcode
              << std::dec << " ticks=" << record.begin() << "-" << record.end() << '\n';
}

int read_relationships(const char* path) {
    vt::Reader input(path);
    auto instructions = input.find_generator("core0.instructions");
    if (!instructions) {
        std::cerr << "No core0.instructions generator in this file\n";
        return 1;
    }
    auto clock = input.find_generator("core0.clock");
    if (!clock) {
        std::cerr << "No core0.clock generator in this file\n";
        return 1;
    }
    std::optional<vt::Record> parent;
    input.transactions({.generator = *instructions}, [&](const vt::Record& transaction) {
        if (transaction.parent()) {
            return vt::Flow::next;
        }
        parent = transaction;
        return vt::Flow::stop;
    });
    if (!parent) {
        std::cout << "No parent instruction in this file\n";
        return 0;
    }

    auto cycle = input.cycle_at(*clock, 14);
    if (!cycle) {
        std::cerr << "No clock cycle at tick 14\n";
        return 1;
    }
    std::cout << "Cycle at tick 14: " << *cycle << '\n';
    print_instruction(input, *parent);
    for (const auto& stage : parent->stages()) {
        std::cout << "  Stage " << input.string(stage.name) << " on " << input.string(stage.lane)
                  << " at ticks " << stage.begin << "-"
                  << stage.end.value_or(std::max(stage.begin, parent->end()))
                  << '\n';
    }
    input.children(parent->id(), [&](const vt::Record& record) {
        std::cout << "  Child: ";
        print_instruction(input, record);
        return vt::Flow::next;
    });
    input.logs(parent->id(), {}, [&](const vt::Record& record) {
        std::cout << "  Log: " << vtr::logging::format_log(input, record) << '\n';
        return vt::Flow::next;
    });
    input.relations(parent->id(), vt::Direction::incoming, [&](const vt::Relation& relation) {
        std::cout << "  Relation: " << relation.from->value << " " << input.string(relation.kind)
                  << " " << relation.to->value << '\n';
        return vt::Flow::next;
    });
    (void)input.validate();
    return 0;
}

int main(int argc, char** argv) {
    try {
        auto path = argc > 1 ? argv[1] : "relationships.vtr";
        write_relationships(path);
        return read_relationships(path);
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
