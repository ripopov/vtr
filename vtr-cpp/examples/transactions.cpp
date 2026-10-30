#include "instruction.h"
#include "vtr_reader.h"
#include "vtr_writer.h"

#include <algorithm>
#include <iostream>

namespace vt = vtr;

void write_transactions(const char* path) {
    vt::Writer trace(path);
    auto core = trace.scope("core0");
    auto instructions = trace.transactions<Instruction>(
        {.name = "instructions", .parent = core}, "INSTRUCTION");
    auto fetch = trace.stages({.name = "fetch", .parent = core});
    auto execute = trace.stages({.name = "execute", .parent = core});
    auto issued = trace.events({.name = "issued", .parent = core});
    auto retired = trace.events({.name = "retired", .parent = core});
    auto decode = trace.intern("decode");
    auto decoded = trace.intern("decoded");
    auto result_ready = trace.intern("result_ready");

    auto tx = trace.begin_at(instructions, 10, Instruction{0x8000, 0x13});
    auto fetch_visit = trace.begin_at(fetch, tx, 10);
    trace.end_at(fetch_visit, 11);
    auto decode_visit = trace.embedded_stage(tx, 11, decode);
    trace.end_embedded_stage(tx, decode_visit, 12);
    trace.embedded_event(tx, 12, decoded);
    trace.event_at(issued, tx, 12);
    auto execute_visit = trace.begin_at(execute, tx, 12);
    trace.end_at(execute_visit, 14);
    trace.embedded_event(tx, 14, result_ready);
    trace.event_at(retired, tx, 14);
    trace.end_at(tx, 15, vt::Status::ok);
    trace.close();
    std::cout << "Wrote instruction " << tx.id().value << " to " << path << '\n';
}

int read_transactions(const char* path) {
    vt::Reader input(path);
    auto instructions = input.find_generator("core0.instructions");
    if (!instructions) {
        std::cerr << "No core0.instructions generator in this file\n";
        return 1;
    }
    std::optional<vt::Record> selected;
    input.transactions({.generator = *instructions, .time = {10, 20}},
                       [&](const vt::Record& transaction) {
                           selected = transaction;
                           return vt::Flow::stop;
                       });
    if (!selected) {
        std::cout << "No instructions in [10, 20]\n";
        return 0;
    }
    auto instruction = selected->payload().as<Instruction>();
    std::cout << "Instruction " << selected->id().value << " pc=" << instruction.pc
              << " from " << selected->begin() << " to " << selected->end() << '\n';
    std::cout << "  child_blocks: [";
    auto separator = "";
    for (auto block : selected->child_blocks()) {
        std::cout << separator << block.value;
        separator = ", ";
    }
    std::cout << "]\n";
    auto activity = input.expand(selected->id());
    for (const auto& record : activity.located) {
        auto name = input.string(input.nodes()[record.generator()->value].name);
        if (record.kind() == vt::RecordKind::stage) {
            std::cout << "  Located stage " << name << " from " << record.begin() << " to "
                      << record.end() << '\n';
        } else if (record.kind() == vt::RecordKind::event) {
            std::cout << "  Located event " << name << " at " << record.begin() << '\n';
        }
    }
    for (const auto& stage : selected->stages()) {
        auto end = stage.end.value_or(std::max(stage.begin, selected->end()));
        auto lane = input.string(stage.lane);
        std::cout << "  Unlocated stage " << input.string(stage.name) << " in lane "
                  << (lane.empty() ? "(default)" : lane) << " from " << stage.begin << " to "
                  << end << '\n';
    }
    for (const auto& event : selected->events()) {
        std::cout << "  Unlocated event " << input.string(event.name) << " at " << event.time
                  << '\n';
    }
    std::cout << activity.located.size() << " located records, "
              << selected->stages().size() + selected->events().size() << " unlocated records, "
              << activity.missing_blocks.size() << " missing blocks\n";
    auto validation = input.validate();
    std::cout << validation.records << " validated records\n";
    return 0;
}

int main(int argc, char** argv) {
    try {
        auto path = argc > 1 ? argv[1] : "run.vtr";
        write_transactions(path);
        return read_transactions(path);
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
