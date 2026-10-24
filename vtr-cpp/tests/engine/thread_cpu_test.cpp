#include "handoff.h"
#include "thread_cpu.h"

#include <atomic>
#include <iostream>
#include <stdexcept>
#include <time.h>

using namespace vtr::detail::engine::detail;

std::uint64_t thread_cpu() {
    timespec value{};
    if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &value)) throw std::runtime_error("thread clock unavailable");
    return std::uint64_t(value.tv_sec) * 1'000'000'000 + value.tv_nsec;
}

std::uint64_t work(std::uint64_t value) {
    for (unsigned i = 0; i < 100'000; ++i) value = value * 6364136223846793005ULL + i;
    return value;
}

int main() {
    try {
        std::atomic<std::uint64_t> callback_cpu{0}, output{0};
        Handoff queue({.workers = 2, .jobs = 4, .bytes = 4096},
                      [&](const auto&, auto bytes) { output.fetch_add(bytes.front()); });
        for (unsigned i = 0; i < 32; ++i) {
            queue.submit({}, 128, [&, i] {
                const auto start = thread_cpu();
                const auto value = work(i);
                callback_cpu.fetch_add(thread_cpu() - start);
                return vtr::detail::wire::Bytes{static_cast<std::uint8_t>(value)};
            });
        }
        queue.drain();
        const auto running = queue.stats();
        queue.stop();
        const auto final = queue.stats();
        if (final.written != 32) throw std::runtime_error("worker lost submitted jobs");
#if defined(__linux__)
        if (!running.worker_cpu_nanoseconds || !final.worker_cpu_nanoseconds ||
            !*running.worker_cpu_nanoseconds || *running.worker_cpu_nanoseconds > *final.worker_cpu_nanoseconds ||
            *final.worker_cpu_nanoseconds < callback_cpu.load())
            throw std::runtime_error("owned worker CPU does not include callback CPU and shutdown");
#else
        if (running.worker_cpu_nanoseconds || final.worker_cpu_nanoseconds)
            throw std::runtime_error("unsupported worker clock reported CPU time");
#endif
        const auto checksum = work(output.load());
        if (queue.stats().worker_cpu_nanoseconds != final.worker_cpu_nanoseconds)
            throw std::runtime_error("joined worker CPU changes with coordinator computation");
        std::cout << checksum << ' ' << final.worker_cpu_nanoseconds.value_or(0) << '\n';
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
