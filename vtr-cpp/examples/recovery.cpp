#include "vtr_reader.h"
#include "vtr_writer.h"

#include <iostream>

int main(int argc, char** argv) {
    namespace vt = vtr;
    try {
        auto source = argc > 1 ? argv[1] : "incomplete.vtr";
        auto destination = argc > 2 ? argv[2] : "recovered.vtr";
        {
            vt::Writer trace(source);
            auto operations = trace.transactions({.name = "operations"});
            auto tx = trace.begin_at(operations, 10);
            trace.end_at(tx, 15, vt::Status::ok);
            trace.seal();
        }
        auto ending = vt::recover(source, destination);
        vt::Reader input(destination);
        auto report = input.validate();
        std::cout << report.records << " recovered records, " << report.missing.size()
                  << " missing references, " << ending.discarded_bytes << " discarded bytes\n";
        return ending.state == vt::Completion::incomplete ? 0 : 1;
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
