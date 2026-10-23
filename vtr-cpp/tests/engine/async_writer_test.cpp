#include "stream.h"
#include <algorithm>
#include <chrono>
#include <filesystem>
#include <future>
#include <iostream>
#include <set>
#include <thread>

using namespace vtr::detail::engine;
namespace fs = std::filesystem;

struct Args {
    std::uint64_t producer, sequence;
    std::string text;
};

template <> struct vtr::detail::engine::Record<Args> {
    static constexpr std::string_view name = "ConcurrentArgs";

    static auto fields() {
        return std::tuple{field("producer", &Args::producer), field("sequence", &Args::sequence),
                          field("text", &Args::text)};
    }
};

void check(bool good, const char* why) {
    if (!good) {
        throw std::runtime_error(why);
    }
}

struct Family {
    RecordId owner, stage, event, log, local_event, local_stage;
    Timestamp time;
    Bytes payload, attribute;
    std::vector<std::pair<RecordId, Bytes>> late;
};

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: async_writer_test DIRECTORY");
        }
        fs::create_directories(argv[1]);
        auto path = fs::path(argv[1]) / "concurrent-writer.vtr";
        constexpr unsigned producers = 4, count = 200;
        constexpr std::size_t queue_bytes = 2 << 20;
        std::array<std::vector<Family>, producers> families;
        GeneratorId ordinary, stage, event, log, clock;
        RecordId root, clock_span, late_child, late_event;
        {
            Stream writer(path, {.codec = vtr::detail::wire::Codec::lz4,
                                 .block_bytes = 4096,
                                 .record_limit = 17,
                                 .workers = 3,
                                 .queue_jobs = 3,
                                 .queue_bytes = queue_bytes});
            auto type = writer.control([](auto& backend) { return backend.template bind<Args>(); });
            clock = writer.control([](auto& backend) { return backend.add_clock(none, "clock"); });
            check(clock == 0, "root clock fixture changed");
            ordinary = writer.control([&](auto& backend) {
                return backend.add_generator(none, "instruction", "INSTRUCTION", type);
            });
            auto root_gen = writer.control(
                [](auto& backend) { return backend.add_generator(none, "grid", "GRID"); });
            stage = writer.control(
                [&](auto& backend) { return backend.add_generator(none, "slot", "STAGE", type); });
            event = writer.control([&](auto& backend) {
                return backend.add_generator(none, "conflict", "EVENT", type);
            });
            log = writer.control([&](auto& backend) {
                return backend.add_generator(none, "p={} i={} text={}", "LOG", type);
            });
            writer.control([&](auto& backend) {
                backend.generator_attribute(log, backend.intern("log.severity"),
                                            Value(std::uint64_t(3)));
            });
            writer.control([&](auto& backend) { backend.associate_clock(ordinary, clock); });
            auto extra = writer.control([](auto& backend) { return backend.intern("extra"); }),
                 note = writer.control([](auto& backend) { return backend.intern("note"); }),
                 wait = writer.control([](auto& backend) { return backend.intern("wait"); }),
                 lane = writer.control([](auto& backend) { return backend.intern("main"); }),
                 cause = writer.control([](auto& backend) { return backend.intern("depends"); });
            const auto coordinator = writer.add_producer();
            writer.use_producer(coordinator);
            root = writer.begin(root_gen, 0);
            std::array<RecordId, producers> roots;
            std::array<Stream::ProducerId, producers> producer_ids;
            for (unsigned i = 0; i < producers; ++i) {
                roots[i] = writer.begin(root_gen, 0, root);
                producer_ids[i] = writer.add_producer();
            }
            clock_span = writer.clock_run(clock, 0, 2);
            writer.advance_time(10000);
            writer.release_producer();
            std::vector<std::future<void>> produced;
            std::exception_ptr monitor_error;
            std::jthread monitor([&](std::stop_token stop) {
                try {
                    while (!stop.stop_requested()) {
                        auto transport = writer.stream_stats();
                        check(transport.completed <= transport.submitted,
                              "ingestion completed an uncounted publication");
                        auto stats = writer.handoff_stats();
                        check(stats.jobs <= 3 && stats.written <= stats.accepted,
                              "live queue job counters escaped bounds");
                        check(stats.bytes <= queue_bytes || stats.jobs == 1,
                              "oversized work shared the queue");
                        std::this_thread::sleep_for(std::chrono::microseconds(100));
                    }
                } catch (...) {
                    monitor_error = std::current_exception();
                }
            });
            for (unsigned producer = 0; producer < producers; ++producer) {
                produced.push_back(std::async(std::launch::async, [&, producer] {
                    writer.use_producer(producer_ids[producer]);
                    for (unsigned i = 0; i < count; ++i) {
                        writer.advance_time(i + 1);
                        auto text = i % 31 == 0 ? std::string(65536, char('a' + producer % 2))
                                                : "p" + std::to_string(producer % 2) + "-" +
                                                      std::to_string(i);
                        auto payload = capture(Args{producer % 2, i, text});
                        Timestamp time = (i * 17) % 64;
                        auto owner = writer.begin(ordinary, time, payload, roots[producer]);
                        auto visit = writer.begin_stage(stage, owner, time + 10);
                        auto point = writer.event(event, owner, time + 11, payload);
                        auto message = writer.log(log, time + 3, payload, owner);
                        auto local = writer.embedded_event(owner, time + 90, note);
                        auto local_stage = writer.embedded_stage(owner, wait, lane, time + 100);
                        std::array values{Value(std::uint64_t(i)), Value::text("nested")};
                        auto attribute = Value::list(values);
                        auto canonical = attribute.payload;
                        writer.attribute(owner, extra, attribute);
                        std::fill(attribute.payload.begin(), attribute.payload.end(), 255);
                        Family family{owner,       visit, point,   message,   local,
                                      local_stage, time,  payload, canonical, {}};
                        if (producer == 0 && i == 100) {
                            auto late_type = writer.control([](auto& backend) {
                                return backend.structure("Late", {{"value", 9}, {"text", 13}});
                            });
                            auto late_generator = writer.control([&](auto& backend) {
                                return backend.add_generator(none, "late", "EVENT", late_type);
                            });
                            Payload row;
                            row.scalar(std::uint64_t(0xfedcba9876543210));
                            row.append(std::string_view("late"));
                            auto data = std::move(row).take();
                            family.late.emplace_back(writer.event(late_generator, owner, 999, data),
                                                     data);
                            auto array = writer.control([](auto& backend) {
                                return backend.template bind<std::array<std::uint16_t, 3>>();
                            });
                            auto array_gen = writer.control([&](auto& backend) {
                                return backend.add_generator(none, "array", "EVENT", array);
                            });
                            auto elements = capture(std::array<std::uint16_t, 3>{65535, 0, 12345});
                            family.late.emplace_back(writer.event(array_gen, owner, 1000, elements),
                                                     elements);
                        }
                        writer.stage_payload(visit, payload);
                        writer.end_stage(visit, time + 12);
                        writer.relate(cause, owner, point);
                        writer.end(owner, time + 20);
                        std::fill(payload.begin(), payload.end(), 255);
                        families[producer].push_back(std::move(family));
                    }
                    writer.advance_time(count + 1);
                    writer.end(roots[producer], count + 100);
                    writer.finish_producer();
                }));
            }
            for (auto& producer : produced) {
                producer.get();
            }
            writer.use_producer(coordinator);
            auto data = capture(Args{9, 999, "independent child"});
            late_child = writer.begin(ordinary, 9000, data, root);
            writer.end(root, 1);
            late_event = writer.event(event, late_child, 9001, data);
            writer.end(late_child, 9002);
            writer.advance_time(10000);
            writer.clock_stop(clock, 10000);
            writer.relate(cause, clock_span, late_child);
            monitor.request_stop();
            monitor.join();
            if (monitor_error) {
                std::rethrow_exception(monitor_error);
            }
            writer.close();
            auto stats = writer.handoff_stats();
            check(stats.accepted == stats.written && stats.jobs == 0 && stats.bytes == 0 &&
                      stats.oversized_jobs > 0,
                  "worker close did not drain oversized traffic");
        }
        Reader reader(
            path,
            {.verify_checksums = true, .owner_cache_entries = 64, .owner_cache_bytes = 256 << 10});
        auto report = reader.validate();
        check(!report.incomplete_file && report.records == 6 * producers * count + 6 + producers &&
                  report.relations == producers * count + 1,
              "concurrent writer lost identities or causal edges");
        check(reader.children(root).size() == producers + 1 &&
                  reader.transaction(late_child)->parent == root,
              "ordinary children were tied to parent finalization");
        check(reader.expand(root).complete && reader.expand(root).located.empty(),
              "ordinary children leaked into located locators");
        auto expanded_child = reader.expand(late_child);
        check(expanded_child.complete && expanded_child.located.size() == 1 &&
                  expanded_child.located[0].id == late_event,
              "closed ancestor hid a located child");
        check(reader.clock_for(ordinary) == std::optional<GeneratorId>(clock) &&
                  reader.cycle_at(clock, 9999) == std::optional<std::uint64_t>(4999),
              "concurrent trace lost root clock association");
        std::size_t stage_count = 0;
        reader.records({.generator = stage, .parent = {}}, [&](const auto&) {
            ++stage_count;
            return true;
        });
        check(stage_count == producers * count, "overlapping located stages were coalesced");
        for (const auto& producer : families) {
            for (const auto& family : producer) {
                auto owner = reader.transaction(family.owner);
                check(owner && reader.transaction(owner->parent)->parent == root &&
                          owner->payload == family.payload,
                      "caller payload mutation reached owner");
                check(owner->attributes.size() == 1 &&
                          owner->attributes[0].value.tag == Tag::list &&
                          owner->attributes[0].value.payload == family.attribute,
                      "caller attribute mutation reached worker");
                std::set<RecordId> expected{family.stage, family.event};
                for (const auto& [id, data] : family.late) {
                    expected.insert(id);
                    auto ref = reader.lookup(id);
                    check(ref && Bytes(ref->payload().begin(), ref->payload().end()) == data,
                          "late schema or generator was not snapshotted");
                }
                auto expanded = reader.expand(family.owner);
                std::set<RecordId> actual;
                check(expanded.complete, "worker owner expansion is incomplete");
                for (const auto& record : expanded.located) {
                    actual.insert(record.id);
                    if (record.id == family.stage || record.id == family.event) {
                        check(record.payload == family.payload,
                              "located payload ownership changed");
                    }
                }
                check(actual == expected, "worker locators lost or mixed records");
                auto logs = reader.logs(family.owner);
                check(logs.size() == 1 && logs[0].id == family.log &&
                          logs[0].payload == family.payload,
                      "worker log association changed");
                auto local = reader.lookup(family.local_event),
                     local_stage = reader.lookup(family.local_stage);
                check(local && local->parent() == family.owner &&
                          local->begin() == family.time + 90 && local_stage &&
                          local_stage->parent() == family.owner && !local_stage->stage()->end,
                      "embedded identities, open ends or independent time changed");
                std::vector<Relation> relations;
                reader.relations(family.owner, false, [&](const auto& edge) {
                    relations.push_back(edge);
                    return true;
                });
                check(relations.size() == 1 && relations[0].destination == family.event,
                      "concurrent causal endpoints changed");
            }
        }
        std::cout << "concurrent capture, immutable late declarations, oversized "
                     "payloads, identity/navigation/clock and held-parent "
                     "lifetimes validated\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
