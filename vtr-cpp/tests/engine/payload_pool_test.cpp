#include "stream.h"
#include <atomic>
#include <chrono>
#include <filesystem>
#include <iostream>
#include <thread>
using namespace vtr::detail::engine;
namespace fs = std::filesystem;

struct Args {
    std::uint64_t serial;
    std::array<std::uint16_t, 3> lanes;
    std::vector<std::uint32_t> words;
    std::string text;
    friend bool operator==(const Args&, const Args&) = default;
};

template <> struct vtr::detail::engine::Record<Args> {
    static constexpr std::string_view name = "ReusableArgs";

    static auto fields() {
        return std::tuple{field("serial", &Args::serial), field("lanes", &Args::lanes),
                          field("words", &Args::words), field("text", &Args::text)};
    }
};

void check(bool ok, const char* why) {
    if (!ok) {
        throw std::runtime_error(why);
    }
}

template <class F> void rejects(F&& f) {
    try {
        f();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("invalid typed final payload accepted");
}

Args value(std::uint64_t serial) {
    Args a{serial,
           {std::uint16_t(serial), std::uint16_t(serial + 1), std::uint16_t(serial + 2)},
           {},
           "tag_" + std::to_string(serial % 7)};
    for (unsigned i = 0; i < serial % 5; ++i) {
        a.words.push_back(std::uint32_t(serial + i));
    }
    if (serial % 97 == 0) {
        a.text.append(17000, 'A');
    }
    return a;
}

void run(const fs::path& path, unsigned workers, std::size_t budget, bool concurrent) {
    constexpr unsigned count = 2200;
    {
        Stream writer(path, {.codec = vtr::detail::wire::Codec::stored,
                             .page_bytes = 256,
                             .block_bytes = 4096,
                             .record_limit = 8,
                             .workers = workers,
                             .queue_jobs = 1,
                             .queue_bytes = 65536,
                             .payload_pool_bytes = budget});
        auto type = writer.control([](auto& backend) { return backend.template bind<Args>(); });
        auto gen = writer.control([&](auto& backend) {
            return backend.add_generator(none, "owner", "TRANSACTION", type);
        });
        auto note = writer.control([](auto& backend) { return backend.intern("note"); });
        auto initial = value(10001);
        const auto coordinator = writer.add_producer();
        writer.use_producer(coordinator);
        auto owner = writer.begin(gen, 0, capture(initial));
        writer.advance_time(1);
        writer.flush();
        rejects([&] {
            writer.control([&](auto& backend) { backend.end_value(owner, 1, std::string("bad")); });
        });
        rejects([&] {
            writer.control([&](auto& backend) { backend.end_value(owner, 1, initial, 9); });
        });
        writer.end(owner, 1);
        std::atomic<bool> monitor_ok = true;
        std::jthread monitor([&](std::stop_token stop) {
            while (!stop.stop_requested()) {
                if (writer.stats().payload_pool_bytes > budget) {
                    monitor_ok = false;
                }
                std::this_thread::sleep_for(std::chrono::microseconds(100));
            }
        });
        std::vector<Stream::ProducerId> producers;
        if (concurrent) {
            for (unsigned i = 0; i < 4; ++i) {
                producers.push_back(writer.add_producer(2));
            }
            writer.advance_time(count + 10);
            writer.release_producer();
        }
        auto produce = [&](unsigned first, unsigned end) {
            for (unsigned i = first; i < end; ++i) {
                writer.advance_time(i + 2);
                auto data = value(i);
                auto tx = writer.begin(gen, i);
                if (i % 2) {
                    writer.attribute(tx, note, Value(std::uint64_t(i)));
                }
                if (i % 3 == 0) {
                    writer.span_kind(tx, 5);
                }
                writer.end_value(tx, i + 1, data, i % 5);
                data.text.assign(7, 'z');
                data.words.clear();
                if (i % 17 == 0) {
                    writer.flush();
                }
            }
        };
        if (concurrent) {
            std::vector<std::jthread> threads;
            for (unsigned p = 0; p < 4; ++p) {
                threads.emplace_back([&, p] {
                    writer.use_producer(producers[p]);
                    produce(p * count / 4, (p + 1) * count / 4);
                    writer.finish_producer();
                });
            }
        } else {
            produce(0, count);
        }
        monitor.request_stop();
        monitor.join();
        if (concurrent) {
            writer.use_producer(coordinator);
        }
        writer.close();
        check(monitor_ok, "capture recycler exceeded capacity budget during use");
        auto stats = writer.stats();
        check(stats.payload_pool_bytes <= budget,
              "capture recycler exceeded final capacity budget");
        if (budget >= 8192) {
            check(stats.payload_reuses > count / 4, "completed payload buffers were not reused");
        }
        if (!budget) {
            check(stats.payload_reuses == 0 && stats.payload_pool_bytes == 0,
                  "disabled recycler retained buffers");
        }
        auto queue = writer.handoff_stats();
        check(queue.accepted == queue.written && !queue.jobs && !queue.bytes,
              "recycler returned before queue drain");
    }
    Reader reader(path, {.verify_checksums = true, .owner_cache_entries = 0});
    auto report = reader.validate();
    check(!report.incomplete_file && report.records == count + 1, "pooled capture lost records");
    std::vector<bool> seen(count + 1);
    reader.transactions({}, [&](const Transaction& tx) {
        auto data = decode<Args>(tx.payload);
        if (data.serial == 10001) {
            check(data == value(10001), "rejected final payload changed existing owner");
            seen[count] = true;
        } else {
            check(tx.status == data.serial % 5 && tx.span_kind == (data.serial % 3 == 0 ? 5 : 0) &&
                      tx.attributes.size() == data.serial % 2,
                  "reused owner slot retained classification or attributes");
            check(data.serial < count && data == value(data.serial),
                  "recycled payload changed source values");
            check(!seen[data.serial], "recycled payload repeated one record");
            seen[data.serial] = true;
        }
        return true;
    });
    check(std::all_of(seen.begin(), seen.end(), [](bool b) { return b; }),
          "pooled capture omitted a source occurrence");
}

int main(int argc, char** argv) {
    try {
        check(argc == 2, "usage: payload_pool_test DIRECTORY");
        fs::path dir = argv[1];
        fs::create_directories(dir);
        for (unsigned workers : {0, 2}) {
            for (std::size_t budget :
                 {std::size_t(0), std::size_t(128), std::size_t(8192), std::size_t(16 << 20)}) {
                for (bool concurrent : {false, true}) {
                    run(dir / ("pool-" + std::to_string(workers) + "-" + std::to_string(budget) +
                               "-" + std::to_string(concurrent) + ".vtr"),
                        workers, budget, concurrent);
                }
            }
        }
        std::cout << "Bounded recycler, disabled/tiny budgets, oversized payloads, "
                     "mutable values, rejected finalization and concurrent "
                     "producers validated\n";
    } catch (const std::exception& e) {
        std::cerr << e.what() << '\n';
        return 1;
    }
}
