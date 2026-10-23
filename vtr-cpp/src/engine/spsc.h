#pragma once

#include <atomic>
#include <bit>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <memory>
#include <stdexcept>
#include <type_traits>

namespace vtr::detail::engine::detail {

inline void relax() noexcept {
#if defined(__x86_64__) || defined(__i386__)
    __builtin_ia32_pause();
#elif defined(__aarch64__)
    asm volatile("yield");
#else
    std::atomic_signal_fence(std::memory_order_seq_cst);
#endif
}

template <class Counter = std::uint64_t> class SpscBytes {
    static_assert(std::is_unsigned_v<Counter> && std::atomic<Counter>::is_always_lock_free);
    const Counter capacity_, mask_;
    const std::unique_ptr<std::uint8_t[]> storage_;
    alignas(64) Counter written_ = 0, cached_read_ = 0;
    alignas(64) std::atomic<Counter> published_write_{0};
    alignas(64) Counter read_ = 0, cached_write_ = 0;
    alignas(64) std::atomic<Counter> published_read_{0};

    static Counter validate(std::size_t capacity) {
        if (!std::has_single_bit(capacity) || capacity < 64 ||
            capacity > std::numeric_limits<Counter>::max() / 2 ||
            capacity > std::numeric_limits<std::size_t>::max() / 2) {
            throw std::invalid_argument("invalid SPSC byte capacity");
        }
        return static_cast<Counter>(capacity);
    }

  public:
    explicit SpscBytes(std::size_t capacity)
        : capacity_(validate(capacity)), mask_(capacity_ - 1),
          storage_(std::make_unique<std::uint8_t[]>(2 * capacity)) {}

    std::size_t capacity() const noexcept {
        return capacity_;
    }

    std::uint8_t* reserve(std::size_t bytes) noexcept {
        if (bytes > capacity_)
            return nullptr;
        if (bytes > capacity_ - static_cast<Counter>(written_ - cached_read_)) {
            cached_read_ = published_read_.load(std::memory_order_acquire);
            if (bytes > capacity_ - static_cast<Counter>(written_ - cached_read_))
                return nullptr;
        }
        return storage_.get() + (written_ & mask_);
    }

    void commit(std::size_t bytes) noexcept {
        written_ = static_cast<Counter>(written_ + bytes);
        published_write_.store(written_, std::memory_order_release);
    }

    const std::uint8_t* peek() noexcept {
        if (read_ == cached_write_) {
            cached_write_ = published_write_.load(std::memory_order_acquire);
            if (read_ == cached_write_)
                return nullptr;
        }
        // A crossing record uses the scratch half. Its following record starts at
        // the wrapped offset, so no live record aliases that scratch storage.
        return storage_.get() + (read_ & mask_);
    }

    void consume(std::size_t bytes) noexcept {
        read_ = static_cast<Counter>(read_ + bytes);
        if (read_ == cached_write_ ||
            static_cast<Counter>(read_ - published_read_.load(std::memory_order_relaxed)) >=
                capacity_ / 4) {
            release();
        }
    }

    void release() noexcept {
        published_read_.store(read_, std::memory_order_release);
    }

    Counter published() const noexcept {
        return published_write_.load(std::memory_order_acquire);
    }

    Counter consumed() const noexcept {
        return read_;
    }
};

} // namespace vtr::detail::engine::detail
