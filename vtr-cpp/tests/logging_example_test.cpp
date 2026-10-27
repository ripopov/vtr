#include "../examples/logging.h"
#include "vtr_logging.h"

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

namespace vtr {
void emit_namespaced(::logging::Logger& logger) {
    LOG_INFO(logger, "namespace={}", 42);
}
} // namespace vtr

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
    check(reader.generators().size() == 2, "one generator per logger");
    check(vtr::logging::callsites(reader).size() == 5, "callsites shared by both loggers");
    std::map<vt::GeneratorId, unsigned> counts;
    std::map<std::string, unsigned> messages;
    std::set<vt::TypeId> value_types;
    std::set<std::uint64_t> lines;
    unsigned info = 0, warning = 0;
    for (const auto& metadata : vtr::logging::callsites(reader)) {
        info += metadata.severity == 2;
        warning += metadata.severity == 3;
        check(metadata.line.has_value() && metadata.file.has_value() &&
                  metadata.function.has_value(),
              "complete static logging metadata");
        lines.insert(*metadata.line);
        check(metadata.file->ends_with("logging_example_test.cpp"), "caller file metadata");
        check(metadata.function->find("emit(") != std::string::npos, "caller function metadata");
    }
    check(info == 4 && warning == 1 && lines.size() == 5, "distinct callsite provenance");

    reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
        ++counts[*record.generator()];
        if (vtr::logging::metadata(reader, record.datatype())->format == "value={}")
            value_types.insert(record.payload()["value"].type());
        const auto message = vtr::logging::format_log(reader, record);
        ++messages[message];
        check(record.attributes().empty(), "messages contain no repeated source metadata");
        if (message.starts_with("many=")) {
            check(record.payload()["value"].size() == 11 &&
                      record.payload()["value"]["arg10"].as<std::int32_t>() == 10,
                  "argument pack order and multi-digit field names");
        } else if (message.starts_with("text=")) {
            const auto text = record.payload()["value"]["arg0"].as<std::string>();
            check((text.size() == 5 || text.size() == 4096) &&
                      text.find_first_not_of('x') == std::string::npos,
                  "small and large borrowed strings captured before mutation");
        } else if (message == "ready") {
            check(record.payload()["value"].size() == 0, "zero argument capture");
        }
        return vt::Flow::next;
    });
    check(counts.size() == 2, "messages retain their logger");
    for (const auto& [source, count] : counts) {
        (void)source;
        check(count == 10, "each logger contains all callsites");
    }
    check(messages["value=10"] == 2 && messages["value=11"] == 2 && messages["value=20"] == 2 &&
              messages["value=21"] == 2 && messages["ready"] == 4 &&
              messages["many=0 1 2 3 4 5 6 7 8 9 10"] == 4,
          "deferred formatting preserves values");
    check(value_types.size() == 1, "argument schema shared across callsites and loggers");
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

void recording_lifetimes(const std::filesystem::path& directory) {
    std::optional<vt::Writer> writer;
    std::optional<vt::Logger> previous_logger;
    std::optional<vt::PayloadType<std::uint64_t>> previous_callsite;
    auto rejects_stale = [](auto&& action) {
        try {
            action();
        } catch (const vt::Error& error) {
            check(error.code() == vt::ErrorCode::wrong_recording, "stale handle error code");
            return;
        }
        throw std::runtime_error("stale recording handle accepted");
    };
    for (unsigned generation = 0; generation < 32; ++generation) {
        const auto path = directory / "lifetime.vtr";
        writer.emplace(path, vt::WriterOptions{.mode = vt::ExecutionMode::inline_});
        const auto logger = writer->logger({.name = "logger"});
        const auto callsite = vtr::logging::declare<std::uint64_t>(*writer, {.format = "{}"});
        if (previous_logger) {
            check(previous_logger->id() == logger.id() && previous_callsite->id() == callsite.id(),
                  "sequential recordings require colliding local IDs");
            rejects_stale([&] { writer->log(*previous_logger, callsite, std::uint64_t{1}); });
            rejects_stale([&] { writer->log(logger, *previous_callsite, std::uint64_t{1}); });
        }
        writer->log(logger, callsite, std::uint64_t{generation});
        writer->close();
        previous_logger = logger;
        previous_callsite = callsite;
        writer.reset();
        vt::Reader reader(path);
        check(reader.validate().records == 1, "rejected stale handles emitted messages");
        reader.records({}, [&](const vt::Record& record) {
            check(record.payload()["value"].as<std::uint64_t>() == generation,
                  "new recording payload changed");
            return vt::Flow::next;
        });
    }
}

void raw_schema_validation(const std::filesystem::path& directory) {
    for (const auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded}) {
        for (unsigned scenario = 0; scenario < 5; ++scenario) {
            const auto path = directory / "raw.vtr";
            vt::Writer writer(path, {.mode = mode});
            const auto logger = writer.logger({.name = "raw"});
            auto callsite =
                vtr::logging::declare_raw(writer, {.format = "{}"}, writer.bind<std::uint64_t>());
            std::array<std::byte, 9> bytes{std::byte{42}};
            std::size_t length = 8;
            if (scenario == 1)
                length = 7;
            if (scenario == 2)
                length = 9;
            if (scenario == 3)
                callsite = vt::TypeId(UINT32_MAX);
            if (scenario == 4)
                callsite = vt::TypeId(logger.id().value);
            bool failed = false;
            try {
                writer.log_raw(logger, callsite, 0, vt::ByteView(bytes).first(length));
                writer.close();
            } catch (const vt::Error&) {
                failed = true;
            }
            check(failed == (scenario != 0), "raw schema validation accepted malformed capture");
            if (!failed) {
                vt::Reader reader(path);
                check(reader.validate().records == 1, "raw capture lost its record");
                check(reader.lookup(vt::RecordId(1))->payload()["value"].as<std::uint64_t>() == 42,
                      "raw capture changed canonical payload");
            }
        }
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
    check(reader.generators().size() == 1 && vtr::logging::callsites(reader).size() == 1,
          "disabled calls declare no callsites");
    reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
        check(vtr::logging::format_log(reader, record) == "enabled=1", "filtered output");
        return vt::Flow::next;
    });
    check(reader.validate().records == 1, "only enabled calls recorded");
}

void preparation(const std::filesystem::path& path) {
    vt::Writer writer(path);
    logging::Logger logger(writer);
    using Payload = decltype(logging::arguments(std::uint64_t{0}));
    static constexpr logging::Metadata metadata{"prepared={:#x}", logging::Severity::info,
                                                std::source_location::current()};
    static_assert(Payload::valid_format(metadata.format));
    static constinit const logging::Callsite<Payload> site(metadata);
    const auto source = logger.prepare(site).id();
    check(logger.prepare(site).id() == source, "preparation reuses source without emitting");
    logger.write(site, logging::arguments(std::uint64_t{0x1234}));
    writer.close();
    vt::Reader reader(path);
    check(reader.generators().size() == 1, "preparation declares exactly one source");
    reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
        check(record.datatype() == source &&
                  vtr::logging::format_log(reader, record) == "prepared=0x1234",
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
                check(reader.generators().size() == 1,
                      "concurrent registration retains one logger");
                reader.records({.kind = vt::RecordKind::log}, [&](const vt::Record& record) {
                    const auto text = vtr::logging::format_log(reader, record);
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

void shared_logger_concurrent_first_use(const std::filesystem::path& path) {
    vt::Writer writer(path, {.encoding = {.workers = 2}});
    const auto shared = writer.logger({.name = "shared"});
    constexpr unsigned workers = 4;
    std::vector<vt::ProducerId> producers;
    for (unsigned worker = 0; worker < workers; ++worker)
        producers.push_back(writer.add_producer());
    std::barrier start(workers);
    std::array<std::exception_ptr, workers> errors{};
    std::vector<std::jthread> threads;
    for (unsigned worker = 0; worker < workers; ++worker) {
        threads.emplace_back([&, worker] {
            bool synchronized = false;
            try {
                writer.use_producer(producers[worker]);
                logging::Logger logger(writer, shared);
                start.arrive_and_wait();
                synchronized = true;
                for (unsigned value = 0; value < 20; ++value)
                    emit_parallel(logger, value);
                writer.finish_producer();
            } catch (...) {
                if (!synchronized)
                    start.arrive_and_drop();
                errors[worker] = std::current_exception();
                writer.release_producer();
            }
        });
    }
    threads.clear();
    for (const auto& error : errors)
        if (error)
            std::rethrow_exception(error);
    writer.close();
    vt::Reader reader(path);
    check(reader.generators().size() == 1 && vtr::logging::callsites(reader).size() == 2,
          "shared logger registration duplicated a generator or callsite");
    reader.records({.generator = shared.id()}, [&](const vt::Record& record) {
        const auto text = vtr::logging::format_log(reader, record);
        check(text.starts_with("parallel=") || text.starts_with("counter="),
              "concurrent callsite binding changed schema");
        return vt::Flow::next;
    });
    check(reader.validate().records == workers * 40, "shared logger lost concurrent captures");
}

void metadata_registration(const std::filesystem::path& directory) {
    for (unsigned change = 0; change != 3; ++change) {
        const auto path = directory / ("metadata-cache-" + std::to_string(change) + ".vtr");
        vt::Writer writer(path, {.mode = vt::ExecutionMode::inline_});
        const int descriptor = 0;
        const auto key = writer.intern("custom");
        vt::DatatypeOptions options{.name = "annotated",
                                    .attributes = {{key, vt::Value(std::string("original"))}}};
        const auto original = writer.datatype<std::uint64_t>(options, &descriptor);
        check(writer.datatype<std::uint64_t>(options, &descriptor).id() == original.id(),
              "immutable metadata registration must reuse its declaration");
        const auto distinct = writer.datatype<std::uint64_t>(options);
        check(distinct.id() != original.id(), "independent metadata declarations were merged");
        if (change == 0)
            options.name = "changed";
        if (change == 1)
            options.attributes[0].data = vt::Value(std::string("changed"));
        bool rejected = false;
        try {
            if (change == 2)
                (void)writer.datatype<std::int64_t>(options, &descriptor);
            else
                (void)writer.datatype<std::uint64_t>(options, &descriptor);
        } catch (const vt::Error& error) {
            check(error.code() == vt::ErrorCode::invalid_argument, "metadata cache error code");
            rejected = true;
        }
        check(rejected, "metadata cache accepted a changed declaration");
        writer.close();
        vt::Reader reader(path);
        const auto& type = reader.types()[original.id().value];
        check(type.name == "annotated" &&
                  std::get<std::string>(type.attributes[0].data.data) == "original",
              "failed registration mutated a published declaration");
    }
}

int main(int argc, char** argv) {
    try {
        const auto directory = argc > 1 ? argv[1] : "logging-fixtures";
        std::filesystem::create_directories(directory);
        metadata_registration(directory);
        {
            const auto path = std::filesystem::path(directory) / "namespaced.vtr";
            vt::Writer writer(path, {.mode = vt::ExecutionMode::inline_});
            ::logging::Logger logger(writer);
            vt::emit_namespaced(logger);
            writer.close();
            vt::Reader reader(path);
            unsigned records = 0;
            reader.records({}, [&](const vt::Record& record) {
                check(vtr::logging::format_log(reader, record) == "namespace=42",
                      "logging macros resolve their frontend inside namespace vtr");
                ++records;
                return vt::Flow::next;
            });
            check(records == 1, "namespaced logging lost its message");
        }
        capture(directory);
        recording_lifetimes(directory);
        raw_schema_validation(directory);
        filtering(std::filesystem::path(directory) / "filtered.vtr");
        preparation(std::filesystem::path(directory) / "prepared.vtr");
        concurrent_registration(directory);
        shared_logger_concurrent_first_use(std::filesystem::path(directory) / "shared.vtr");
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
