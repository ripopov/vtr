#include "handoff.h"
#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <future>
#include <iostream>
#include <mutex>

using namespace vtr::detail::engine::detail;
using namespace std::chrono_literals;

void check(bool value, const char* message) {
    if (!value) {
        throw std::runtime_error(message);
    }
}

struct Gate {
    std::mutex mutex;
    std::condition_variable changed;
    bool opened = false;

    void open() {
        {
            std::lock_guard lock(mutex);
            opened = true;
        }
        changed.notify_all();
    }

    void wait() {
        std::unique_lock lock(mutex);
        check(changed.wait_for(lock, 5s, [&] { return opened; }), "gate timed out");
    }
};

template <class F> void until(F&& predicate) {
    auto limit = std::chrono::steady_clock::now() + 5s;
    while (!predicate()) {
        check(std::chrono::steady_clock::now() < limit, "handoff condition timed out");
        std::this_thread::sleep_for(1ms);
    }
}

template <class F> void failed(F&& action, bool allocation = false) {
    try {
        action();
    } catch (const std::bad_alloc&) {
        check(allocation, "unexpected allocation failure");
        return;
    } catch (const std::runtime_error& e) {
        check(!allocation && std::string_view(e.what()) == "injected failure",
              "wrong latched error");
        return;
    }
    throw std::runtime_error("failed queue accepted operation");
}

SectionRequest section(unsigned i) {
    return {9, 0, i, i, i + 1};
}

void reorder_and_backpressure() {
    std::array<Gate, 3> encoded;
    Gate writing, release_write;
    std::mutex output_mutex;
    std::vector<unsigned> output;
    std::vector<vtr::detail::wire::Bytes> payloads;
    Handoff queue({3, 3, 30}, [&](const auto& request, auto bytes) {
        if (request.begin == 0) {
            writing.open();
            release_write.wait();
        }
        std::lock_guard lock(output_mutex);
        output.push_back(request.begin);
        payloads.emplace_back(bytes.begin(), bytes.end());
    });
    vtr::detail::wire::Bytes source{1, 2, 3};
    for (unsigned i = 0; i < 3; ++i) {
        queue.submit(section(i), 10, [&, i, copy = source] {
            encoded[i].wait();
            return copy;
        });
    }
    source.assign(3, 99);
    until([&] { return queue.stats().encoding == 3; });
    encoded[2].open();
    until([&] { return queue.stats().ready == 1; });
    encoded[1].open();
    until([&] { return queue.stats().ready == 2; });
    auto producer = std::async(std::launch::async, [&] {
        queue.submit(section(3), 10, [] { return vtr::detail::wire::Bytes{4}; });
    });
    until([&] { return queue.stats().waiting_submitters == 1; });
    {
        std::lock_guard lock(output_mutex);
        check(output.empty(), "out-of-order completion reached file sink");
    }
    encoded[0].open();
    writing.wait();
    auto held = queue.stats();
    check(held.jobs == 3 && held.bytes == 30 && held.ready == 2 && held.waiting_submitters == 1,
          "ready or writing work escaped queue accounting");
    release_write.open();
    producer.get();
    queue.drain();
    check(output == std::vector<unsigned>{0, 1, 2, 3}, "sink order differs from submission order");
    check(payloads[0] == vtr::detail::wire::Bytes({1, 2, 3}) && payloads[1] == payloads[0] &&
              payloads[2] == payloads[0] && payloads[3] == vtr::detail::wire::Bytes({4}),
          "owned handoff payload changed");
    auto stats = queue.stats();
    check(stats.accepted == 4 && stats.written == 4 && stats.jobs == 0 && stats.bytes == 0 &&
              stats.wait_count == 1 && stats.wait_nanoseconds && stats.peak_jobs == 3 &&
              stats.peak_bytes == 30 && stats.peak_encoding == 3,
          "handoff saturation counters disagree");
}

void oversized_exclusive() {
    Gate first, large;
    std::vector<unsigned> output;
    Handoff queue({2, 4, 16}, [&](const auto& request, auto) { output.push_back(request.begin); });
    queue.submit(section(0), 8, [&] {
        first.wait();
        return vtr::detail::wire::Bytes{0};
    });
    auto producer = std::async(std::launch::async, [&] {
        queue.submit(section(1), 32, [&] {
            large.wait();
            return vtr::detail::wire::Bytes{1};
        });
    });
    until([&] { return queue.stats().waiting_submitters == 1; });
    first.open();
    producer.get();
    auto follower = std::async(std::launch::async, [&] {
        queue.submit(section(2), 8, [] { return vtr::detail::wire::Bytes{2}; });
    });
    until([&] { return queue.stats().waiting_submitters == 1; });
    auto current = queue.stats();
    check(current.jobs == 1 && current.bytes == 32 && current.oversized_jobs == 1,
          "oversized job was not exclusive");
    large.open();
    follower.get();
    queue.drain();
    check(output == std::vector<unsigned>({0, 1, 2}) && queue.stats().peak_bytes == 32,
          "oversized queue ordering or accounting changed");
}

void encoder_failure(bool allocation) {
    Gate first, fail;
    std::vector<unsigned> output;
    Handoff queue({2, 2, 20}, [&](const auto& request, auto) { output.push_back(request.begin); });
    queue.submit(section(0), 10, [&] {
        first.wait();
        return vtr::detail::wire::Bytes{0};
    });
    queue.submit(section(1), 10, [&]() -> vtr::detail::wire::Bytes {
        fail.wait();
        if (allocation) {
            throw std::bad_alloc();
        }
        throw std::runtime_error("injected failure");
    });
    auto producer = std::async(std::launch::async, [&] {
        failed([&] { queue.submit(section(2), 10, [] { return vtr::detail::wire::Bytes{2}; }); },
               allocation);
    });
    until([&] { return queue.stats().waiting_submitters == 1; });
    fail.open();
    producer.get();
    check(queue.failed(), "encoder failure not latched");
    failed([&] { queue.check_failure(); }, allocation);
    failed([&] { queue.drain(); }, allocation);
    first.open();
    queue.stop();
    check(output.empty() && queue.stats().jobs == 0 && queue.stats().bytes == 0,
          "failed queue wrote beyond missing prefix or retained jobs");
}

void sink_failure() {
    Gate fail;
    std::vector<unsigned> output;
    Handoff queue({2, 4, 40}, [&](const auto& request, auto) {
        if (request.begin == 1) {
            fail.wait();
            throw std::runtime_error("injected failure");
        }
        output.push_back(request.begin);
    });
    for (unsigned i = 0; i < 4; ++i) {
        queue.submit(section(i), 10, [i] { return vtr::detail::wire::Bytes{std::uint8_t(i)}; });
    }
    fail.open();
    failed([&] { queue.drain(); });
    failed([&] { queue.submit(section(4), 10, [] { return vtr::detail::wire::Bytes{4}; }); });
    queue.stop();
    check(output == std::vector<unsigned>{0} && queue.stats().written == 1,
          "sink failure did not retain exactly the written prefix");
}

void synchronous() {
    std::vector<unsigned> output;
    Handoff queue({0, 1, 16}, [&](const auto& request, auto) { output.push_back(request.begin); });
    queue.submit(section(0), 8, [] { return vtr::detail::wire::Bytes{0}; });
    queue.submit(section(1), 32, [] { return vtr::detail::wire::Bytes{1}; });
    queue.drain();
    check(output == std::vector<unsigned>({0, 1}) && queue.stats().oversized_jobs == 1 &&
              queue.stats().peak_bytes == 32 && queue.stats().jobs == 0,
          "synchronous handoff differs");
    failed([&] {
        queue.submit(section(2), 8, []() -> vtr::detail::wire::Bytes {
            throw std::runtime_error("injected failure");
        });
    });
    failed([&] { queue.drain(); });
    queue.stop();
    check(queue.stats().jobs == 0, "synchronous failure leaked charge");
}

void concurrent_producers() {
    for (unsigned seed = 0; seed < 3; ++seed) {
        constexpr unsigned producers = 4, records = 300;

        struct Written {
            unsigned producer, sequence;
        };

        std::vector<Written> written;
        Handoff queue({4, 7, 256}, [&](const auto& request, auto bytes) {
            auto producer = unsigned(request.block_number), sequence = unsigned(request.begin);
            check(bytes.size() == 16 + (sequence * 13 + producer * 5) % 100,
                  "concurrent payload size changed");
            for (auto byte : bytes) {
                check(byte == std::uint8_t(sequence + producer),
                      "concurrent payload ownership changed");
            }
            written.push_back({producer, sequence});
        });
        std::vector<std::future<void>> submitted;
        for (unsigned producer = 0; producer < producers; ++producer) {
            submitted.push_back(std::async(std::launch::async, [&, producer] {
                for (unsigned i = 0; i < records; ++i) {
                    vtr::detail::wire::Bytes payload(16 + (i * 13 + producer * 5) % 100,
                                                     std::uint8_t(i + producer));
                    queue.submit({9, 0, i, i, producer}, 16 + ((i * 7 + producer) % 64),
                                 [copy = payload, i, producer, seed] {
                                     std::this_thread::sleep_for(std::chrono::microseconds(
                                         ((i * 37 + producer + seed) % 5) * 30));
                                     return copy;
                                 });
                    payload.assign(payload.size(), 255);
                }
            }));
        }
        for (auto& producer : submitted) {
            producer.get();
        }
        queue.drain();
        std::array<unsigned, producers> next{};
        for (const auto& record : written) {
            check(record.producer < producers && record.sequence == next[record.producer]++,
                  "concurrent producer submission order changed");
        }
        check(written.size() == producers * records &&
                  std::all_of(next.begin(), next.end(), [](auto n) { return n == records; }),
              "concurrent handoff lost or duplicated records");
        auto stats = queue.stats();
        check(stats.written == producers * records && stats.accepted == stats.written &&
                  !stats.jobs && !stats.bytes && stats.peak_jobs <= 7 && stats.peak_bytes <= 256 &&
                  stats.wait_count,
              "concurrent handoff exceeded bounds");
    }
}

int main() {
    try {
        reorder_and_backpressure();
        oversized_exclusive();
        encoder_failure(false);
        encoder_failure(true);
        sink_failure();
        synchronous();
        concurrent_producers();
        std::cout << "ordered bounded handoff, ownership, saturation, oversized "
                     "work and worker/sink failures validated\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
