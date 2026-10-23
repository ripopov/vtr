#include "record_ids.h"

#include <algorithm>
#include <array>
#include <barrier>
#include <functional>
#include <iostream>
#include <thread>
#include <vector>

using vtr::detail::engine::detail::AtomicRecordIds;

void check(bool condition) {
    if (!condition)
        throw std::runtime_error("atomic record identities were reused or exhausted incorrectly");
}

void unique_ids() {
    AtomicRecordIds<std::uint64_t, 8> ids;
    std::array<std::vector<std::uint64_t>, 8> captured;
    std::barrier start(8);
    std::vector<std::thread> threads;
    for (auto& values : captured) {
        values.resize(10000);
        threads.emplace_back([&ids, &start, &values] {
            start.arrive_and_wait();
            for (auto& value : values)
                value = ids.acquire();
        });
    }
    for (auto& thread : threads)
        thread.join();
    std::vector<std::uint64_t> all;
    for (const auto& values : captured) {
        check(std::adjacent_find(values.begin(), values.end(), std::greater_equal<>()) ==
              values.end());
        all.insert(all.end(), values.begin(), values.end());
    }
    std::sort(all.begin(), all.end());
    for (std::size_t i = 0; i < all.size(); ++i)
        check(all[i] == i + 1);
}

void exhausted_ids() {
    for (std::uint8_t first : {std::uint8_t(1), std::uint8_t(247)}) {
        AtomicRecordIds<std::uint8_t, 4> ids(first);
        std::array<std::atomic<unsigned>, 256> counts{};
        std::atomic<bool> resumed{false};
        std::barrier start(4);
        std::vector<std::thread> threads;
        for (unsigned i = 0; i < 4; ++i) {
            threads.emplace_back([&] {
                bool saw_exhaustion = false;
                start.arrive_and_wait();
                for (unsigned call = 0; call < 10000; ++call) {
                    const auto id = ids.acquire();
                    if (id) {
                        counts[id].fetch_add(1, std::memory_order_relaxed);
                        if (saw_exhaustion)
                            resumed.store(true, std::memory_order_relaxed);
                    } else {
                        saw_exhaustion = true;
                    }
                }
            });
        }
        for (auto& thread : threads)
            thread.join();
        check(!resumed.load());
        for (unsigned id = 0; id < counts.size(); ++id)
            check(counts[id].load() == unsigned(id >= first && id <= 250));
        for (unsigned i = 0; i < 1000; ++i)
            check(!ids.acquire());
    }
}

int main() {
    try {
        unique_ids();
        exhausted_ids();
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
