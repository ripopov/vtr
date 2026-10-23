#include "engine/vtr.h"

#include <array>
#include <filesystem>
#include <iostream>
#include <stdexcept>

using namespace vtr::detail::engine;

void check(bool condition) {
    if (!condition) {
        throw std::runtime_error("supplied identities changed storage or lookup semantics");
    }
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: assigned_ids_test DIRECTORY");
        }
        std::filesystem::create_directories(argv[1]);
        const auto path = std::filesystem::path(argv[1]) / "assigned.vtr";
        Writer writer(path, {.page_bytes = 64, .record_limit = 2});
        const auto source = writer.add_generator(none, "requests", "TRANSACTION");
        const auto events = writer.add_generator(none, "events", "EVENT");
        const std::array<RecordId, 4> identities{40, 7, 41, 2};
        for (std::size_t i = 0; i < identities.size(); ++i) {
            writer.assign_record_id(identities[i]);
            const auto id = writer.begin(source, i * 10);
            check(id == identities[i]);
            writer.end(id, i * 10 + 1);
        }
        writer.assign_record_id(50);
        const auto parent = writer.begin(source, 50);
        writer.assign_record_id(60);
        const auto child = writer.event(events, parent, 51);
        writer.assign_record_id(55);
        const auto independent = writer.begin(source, 52);
        writer.end(independent, 53);
        writer.end(parent, 54);
        const auto automatic = writer.begin(source, 55);
        check(automatic == 61);
        writer.end(automatic, 56);
        writer.close();

        Reader reader(path);
        check(reader.validate().records == 8);
        for (std::size_t i = 0; i < identities.size(); ++i) {
            const auto found = reader.transaction(identities[i]);
            check(found && found->id == identities[i] && found->begin == i * 10);
        }
        check(!reader.lookup(8));
        const auto found = reader.lookup(child);
        check(found && found->parent() == parent);
        check(reader.expand(parent).complete && reader.expand(parent).located.size() == 1);
        check(reader.transaction(automatic)->begin == 55);
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
