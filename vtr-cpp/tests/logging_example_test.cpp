#include "../examples/logging.h"
#include "vtr_reader.h"

#include <barrier>
#include <exception>
#include <filesystem>
#include <iostream>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <thread>

namespace vt = vtr;

void check(bool value, const char* message) {
    if (!value) {
        throw std::runtime_error(message);
    }
}

void emit(logging::Logger& logger, std::uint32_t value, std::size_t text_size) {
    LOG_INFO(logger, "value={}", value);
    LOG_WARNING(logger, "value={}", value + std::uint32_t{1});
    std::string text(text_size, 'x');
    LOG_INFO(logger, "text={}", text);
    text.assign("overwritten");
    LOG_INFO(logger, "ready");
    LOG_INFO(logger, "many={} {} {} {} {} {} {} {} {} {} {}", 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10);
}

void verify_capture(const std::filesystem::path& path) {
    vt::Reader reader(path);
    check(reader.generators().size() == 10, "one source per callsite and logger");
    std::map<vt::GeneratorId, unsigned> counts;
    std::map<std::string, unsigned> messages;
    std::set<vt::TypeId> value_types;
    std::set<std::uint64_t> lines;
    unsigned info = 0, warning = 0;
    for (const auto& node : reader.nodes()) {
        if (node.kind != vt::NodeKind::generator) {
            continue;
        }
        check(node.metadata.size() == 4, "complete static source metadata");
        for (const auto& attribute : node.metadata) {
            const auto key = reader.string(attribute.key);
            if (key == "log.severity") {
                const auto level = std::get<std::uint64_t>(attribute.data.data);
                info += level == 2;
                warning += level == 3;
            } else if (key == "log.line") {
                lines.insert(std::get<std::uint64_t>(attribute.data.data));
            } else if (key == "log.file") {
                const auto file = reader.string(std::get<vt::StringId>(attribute.data.data));
                check(file.ends_with("logging_example_test.cpp"), "caller file metadata");
            } else if (key == "log.func") {
                const auto function = reader.string(std::get<vt::StringId>(attribute.data.data));
                check(function.find("emit(") != std::string_view::npos, "caller function metadata");
            }
        }
    }
    for (const auto& generator : reader.generators()) {
        if (reader.string(reader.nodes()[generator.id.value].name) == "value={}") {
            value_types.insert(generator.payload);
        }
    }
    check(info == 8 && warning == 2 && lines.size() == 5, "distinct callsite provenance");
    check(value_types.size() == 1, "argument schema shared across callsites and loggers");
    reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
        ++counts[*record.generator()];
        const auto message = reader.format_log(record);
        ++messages[message];
        check(record.attributes().empty(), "messages contain no repeated source metadata");
        if (message.starts_with("many=")) {
            check(record.payload().size() == 11 &&
                      record.payload()["arg10"].as<std::int32_t>() == 10,
                  "argument pack order and multi-digit field names");
        } else if (message.starts_with("text=")) {
            const auto text = record.payload()["arg0"].as<std::string>();
            check((text.size() == 5 || text.size() == 4096) &&
                      text.find_first_not_of('x') == std::string::npos,
                  "small and large borrowed strings captured before mutation");
        } else if (message == "ready") {
            check(record.payload().size() == 0, "zero argument capture");
        }
        return vt::Flow::next;
    });
    check(counts.size() == 10, "each logger uses its own sources");
    for (const auto& [source, count] : counts) {
        (void)source;
        check(count == 2, "repeated interleaved calls reuse sources");
    }
    check(messages["value=10"] == 2 && messages["value=11"] == 2 &&
              messages["value=20"] == 2 && messages["value=21"] == 2 &&
              messages["ready"] == 4 && messages["many=0 1 2 3 4 5 6 7 8 9 10"] == 4,
          "deferred formatting preserves values");
    check(reader.validate().records == 20, "complete capture validation");
}

void capture(const std::filesystem::path& directory) {
    // Reusing exactly the same writer storage catches caches keyed only by its address.
    std::optional<vt::Writer> writer;
    for (const auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded}) {
        const auto path =
            directory / (mode == vt::ExecutionMode::inline_ ? "inline.vtr" : "threaded.vtr");
        writer.emplace(path, vt::WriterOptions{.mode = mode});
        {
            logging::Logger first(*writer, "first"), second(*writer, "second");
            for (vt::Timestamp time = 1; time <= 2; ++time) {
                writer->advance_time(time);
                emit(first, 10, time == 1 ? 5 : 4096);
                emit(second, 20, time == 1 ? 4096 : 5);
            }
            writer->close();
        }
        writer.reset();
        verify_capture(path);
    }
}

void filtering(const std::filesystem::path& path) {
    vt::Writer writer(path);
    logging::Logger logger(writer);
    int arguments = 0, selections = 0;
    auto select_logger = [&]() -> logging::Logger& {
        ++selections;
        return logger;
    };
    logger.set_level(logging::Severity::warning);
    LOG_INFO(select_logger(), "filtered={}", ++arguments);
    check(arguments == 0 && selections == 1, "disabled arguments skipped; logger evaluated once");
    LOG_WARNING(select_logger(), "enabled={}", ++arguments);
    check(arguments == 1 && selections == 2, "enabled expressions evaluated once");
    logger.set_level(logging::Severity::off);
    LOG_WARNING(logger, "filtered warning={}", ++arguments);
    LOG_INFO(logger, "filtered info={}", ++arguments);
    check(arguments == 1, "off level skips every argument expression");
    writer.close();
    vt::Reader reader(path);
    check(reader.generators().size() == 1, "disabled calls declare no sources");
    reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
        check(reader.format_log(record) == "enabled=1", "filtered output");
        return vt::Flow::next;
    });
    check(reader.validate().records == 1, "only enabled calls recorded");
}

void preparation(const std::filesystem::path& path) {
    vt::Writer writer(path);
    logging::Logger logger(writer);
    using Payload = decltype(logging::arguments(std::uint64_t{0}));
    static constexpr logging::Metadata metadata{
        "prepared={:#x}", logging::Severity::info, std::source_location::current()};
    static_assert(Payload::valid_format(metadata.format));
    static constinit const logging::Callsite<Payload> site(metadata);
    const auto source = logger.prepare(site).id();
    check(logger.prepare(site).id() == source, "preparation reuses source without emitting");
    logger.write(site, logging::arguments(std::uint64_t{0x1234}));
    writer.close();
    vt::Reader reader(path);
    check(reader.generators().size() == 1, "preparation declares exactly one source");
    reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
        check(record.generator() == source && reader.format_log(record) == "prepared=0x1234",
              "prepared source used for typed capture");
        return vt::Flow::next;
    });
    check(reader.validate().records == 1, "preparation emits no warmup record");
}

void emit_parallel(logging::Logger& logger, unsigned value) {
    LOG_INFO(logger, "parallel={} word={}", value, "ready");
    LOG_WARNING(logger, "counter={}", std::uint64_t{value});
}

void concurrent_registration(const std::filesystem::path& directory) {
    constexpr unsigned workers = 4;
    std::barrier start(workers);
    std::array<std::exception_ptr, workers> errors{};
    std::vector<std::jthread> threads;
    for (unsigned worker = 0; worker < workers; ++worker) {
        threads.emplace_back([&, worker] {
            bool synchronized = false;
            try {
                const auto path = directory / ("parallel-" + std::to_string(worker) + ".vtr");
                vt::Writer writer(path, {.mode = vt::ExecutionMode::inline_});
                logging::Logger logger(writer);
                start.arrive_and_wait();
                synchronized = true;
                for (unsigned value = 0; value < 20; ++value) {
                    emit_parallel(logger, value);
                }
                writer.close();
                vt::Reader reader(path);
                check(reader.generators().size() == 2, "concurrent registration reuses typed slots");
                reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
                    const auto text = reader.format_log(record);
                    check(text.starts_with("parallel=") || text.starts_with("counter="),
                          "concurrent capture uses the correct schema");
                    return vt::Flow::next;
                });
                check(reader.validate().records == 40, "independent concurrent logger output");
            } catch (...) {
                if (!synchronized) {
                    start.arrive_and_drop();
                }
                errors[worker] = std::current_exception();
            }
        });
    }
    threads.clear();
    for (const auto& error : errors) {
        if (error) {
            std::rethrow_exception(error);
        }
    }
}

int main(int argc, char** argv) {
    try {
        const auto directory = argc > 1 ? argv[1] : "logging-fixtures";
        std::filesystem::create_directories(directory);
        capture(directory);
        filtering(std::filesystem::path(directory) / "filtered.vtr");
        preparation(std::filesystem::path(directory) / "prepared.vtr");
        concurrent_registration(directory);
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
