#include "instruction.h"
#include "vtr_reader.h"

#include <iostream>

int main(int argc, char** argv) {
    namespace vt = vtr;
    try {
        vt::Reader input(argc > 1 ? argv[1] : "run.vtr");
        auto execute = input.find_generator("core0.execute");
        if (!execute) {
            std::cerr << "No core0.execute generator in this file\n";
            return 1;
        }
        std::optional<vt::Record> selected;
        input.records({.generator = *execute, .kind = vt::RecordKind::stage, .time = {10, 20}},
                      [&](const vt::Record& visit) {
                          selected = visit;
                          return vt::Flow::stop;
                      });
        if (!selected) {
            std::cout << "No execute visits in [10, 20]\n";
            return 0;
        }
        auto owner = input.lookup(*selected->parent());
        if (!owner) {
            std::cerr << "Visit owner is missing from this incomplete file\n";
            return 1;
        }
        auto instruction = owner->payload().as<Instruction>();
        auto activity = input.expand(owner->id());
        std::cout << "Instruction " << owner->id().value << " pc=" << instruction.pc
                  << " executed from " << selected->begin() << " to " << selected->end() << "; "
                  << activity.located.size() << " located records, "
                  << activity.missing_blocks.size() << " missing blocks\n";
        auto validation = input.validate();
        std::cout << validation.records << " validated records\n";
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
