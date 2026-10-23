#include <array>
#include <iostream>
#include <vtr_reader.h>
#include <vtr_track.h>
#include <vtr_writer.h>

int main(int argc, char** argv) {
    if (argc != 2)
        return 2;
    try {
        vtr::RecordId id(0);
        {
            vtr::Writer writer(argv[1], {.mode = vtr::ExecutionMode::inline_});
            auto source = writer.transactions<std::array<std::uint64_t, 3>>({.name = "request"});
            writer.advance_time(10);
            auto record = writer.begin(source, std::array<std::uint64_t, 3>{0, 42, UINT64_MAX});
            id = record.id();
            writer.advance_time(20);
            writer.end(record, vtr::Status::ok);
            writer.close();
        }
        std::optional<vtr::Record> retained;
        {
            vtr::Reader reader(argv[1]);
            retained = reader.lookup(id);
            if (reader.validate().records != 1)
                return 1;
        }
        return retained && retained->begin() == 10 && retained->end() == 20 &&
                       retained->payload().as<std::array<std::uint64_t, 3>>() ==
                           std::array<std::uint64_t, 3>{0, 42, UINT64_MAX}
                   ? 0
                   : 1;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
