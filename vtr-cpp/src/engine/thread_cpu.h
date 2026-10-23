#pragma once

// CPU snapshots for owned workers; no timing calls run on the producer path.

#include <cstdint>
#include <mutex>
#include <optional>
#include <pthread.h>
#include <time.h>

namespace vtr::detail::engine::detail {

class alignas(64) ThreadCpu {
  public:
    class Measurement {
      public:
        explicit Measurement(ThreadCpu& counter) : counter_(counter) { counter_.start(); }
        ~Measurement() { counter_.finish(); }
        Measurement(const Measurement&) = delete;
        Measurement& operator=(const Measurement&) = delete;
      private:
        ThreadCpu& counter_;
    };

    std::optional<std::uint64_t> nanoseconds() const {
        const std::lock_guard lock(mutex_);
        if (!valid_) return {};
        if (!running_) return total_;
        const auto now = read(clock_);
        return now ? std::optional(*now - start_) : std::nullopt;
    }

  private:
    static std::optional<std::uint64_t> read(clockid_t clock) noexcept {
        timespec value{};
        if (clock_gettime(clock, &value)) return {};
        return std::uint64_t(value.tv_sec) * 1'000'000'000 + value.tv_nsec;
    }

    void start() {
        const std::lock_guard lock(mutex_);
        if (pthread_getcpuclockid(pthread_self(), &clock_)) { valid_ = false; return; }
        const auto now = read(clock_);
        if (!now) { valid_ = false; return; }
        start_ = *now;
        running_ = true;
    }

    void finish() noexcept {
        const std::lock_guard lock(mutex_);
        if (!running_) return;
        const auto now = read(clock_);
        valid_ = now.has_value();
        if (now) total_ = *now - start_;
        running_ = false;
    }

    // Worker exit cannot invalidate its clock while a snapshot is reading it.
    mutable std::mutex mutex_;
    clockid_t clock_ = 0;
    std::uint64_t start_ = 0, total_ = 0;
    bool running_ = false, valid_ = true;
};
} // namespace vtr::detail::engine::detail
