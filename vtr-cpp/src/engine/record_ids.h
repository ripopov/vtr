#pragma once

#include <atomic>
#include <cstdint>
#include <limits>
#include <stdexcept>
#include <type_traits>

namespace vtr::detail::engine::detail {

template <class Counter = std::uint64_t,
          Counter MaxConcurrent = std::numeric_limits<std::uint32_t>::max()>
class alignas(64) AtomicRecordIds {
    static_assert(std::is_unsigned_v<Counter> && std::atomic<Counter>::is_always_lock_free);
    static_assert(MaxConcurrent && MaxConcurrent < std::numeric_limits<Counter>::max() / 2);
    static constexpr Counter exhausted = std::numeric_limits<Counter>::max() - MaxConcurrent;
    std::atomic<Counter> next_;

  public:
    explicit AtomicRecordIds(Counter first = 1) : next_(first) {
        if (!first || first > exhausted)
            throw std::invalid_argument("invalid initial record identifier");
    }

    // At most MaxConcurrent calls may be in flight. The guard range prevents
    // wrap before an exhausted caller resets the counter; zero is never an ID.
    Counter acquire() noexcept {
        const auto id = next_.fetch_add(1, std::memory_order_relaxed);
        if (id < exhausted)
            return id;
        next_.store(exhausted, std::memory_order_relaxed);
        return 0;
    }
};

} // namespace vtr::detail::engine::detail
