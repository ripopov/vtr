#include <array>
#include <iostream>
#include <vtr_logging.h>
#include <vtr_track.h>
#include <vtr_writer.h>

int main(int argc, char** argv) {
    if (argc != 2)
        return 2;
    try {
        vtr::RecordId id(0), message(0);
        {
            vtr::Writer writer(argv[1], {.mode = vtr::ExecutionMode::inline_});
            auto source = writer.transactions<std::array<std::uint64_t, 3>>({.name = "request"});
            writer.advance_time(10);
            auto record = writer.begin(source, std::array<std::uint64_t, 3>{0, 42, UINT64_MAX});
            id = record.id();
            writer.advance_time(20);
            writer.end(record, vtr::Status::ok);
            const auto logger = writer.logger({.name = "consumer"});
            const auto datatype =
                vtr::logging::declare<std::uint64_t>(writer, {.format = "value {}"});
            message = writer.log(logger, datatype, std::uint64_t{42});
            writer.close();
        }
        std::optional<vtr::Record> retained;
        std::optional<vtr::PayloadView> arguments;
        {
            vtr::Reader reader(argv[1]);
            retained = reader.lookup(id);
            const auto log = reader.lookup(message);
            if (!log || vtr::logging::format_log(reader, *log) != "value 42")
                return 1;
            arguments = log->payload()["value"];
            if (reader.validate().records != 2)
                return 1;
        }
        return arguments && arguments->as<std::uint64_t>() == 42 && retained &&
                       retained->begin() == 10 && retained->end() == 20 &&
                       retained->payload().as<std::array<std::uint64_t, 3>>() ==
                           std::array<std::uint64_t, 3>{0, 42, UINT64_MAX}
                   ? 0
                   : 1;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
