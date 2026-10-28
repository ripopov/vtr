#include "handoff.h"
#include "thread_cpu.h"

#include <atomic>
#include <bit>
#include <chrono>
#include <thread>

namespace vtr::detail::engine::detail {
namespace {
using Clock = std::chrono::steady_clock;
constexpr std::uint64_t count_shift = 48;
constexpr std::uint64_t byte_mask = (std::uint64_t{1} << count_shift) - 1;
static_assert(std::atomic<std::uint64_t>::is_always_lock_free);
static_assert(std::atomic<std::uint32_t>::is_always_lock_free);

void require(bool condition, const char* message) {
    if (!condition) {
        throw vtr::detail::wire::Error(ErrorCode::invalid_state, message);
    }
}

void maximum(std::atomic<std::uint64_t>& target, std::uint64_t value) {
    auto old = target.load(std::memory_order_relaxed);
    while (old < value && !target.compare_exchange_weak(old, value, std::memory_order_relaxed)) {
    }
}
} // namespace

struct Handoff::Impl {
    enum class State : std::uint8_t { empty, queued, encoding, ready };

    struct Slot {
        std::atomic<State> state{State::empty};
        SectionRequest section;
        std::size_t charge = 0;
        Encode encode;
        vtr::detail::wire::Bytes bytes;
    };

    const HandoffOptions options;
    const Write write;
    const std::size_t mask;
    std::unique_ptr<Slot[]> slots;
    // One reservation updates both limits atomically, including exclusive oversized jobs.
    alignas(64) std::atomic<std::uint64_t> budget{0}, accepted{0};
    alignas(64) std::atomic<std::uint64_t> next_encode{0};
    alignas(64) std::atomic<std::uint64_t> written{0};
    std::atomic<std::uint64_t> encoding{0}, ready{0}, waiting{0};
    std::atomic<std::uint64_t> peak_jobs{0}, peak_bytes{0}, peak_encoding{0}, peak_ready{0};
    std::atomic<std::uint64_t> waits{0}, wait_nanoseconds{0}, oversized{0};
    std::atomic<bool> stopped{false}, failed{false};
    // Snapshot before testing a predicate so publication or shutdown cannot be missed.
    std::atomic<std::uint32_t> progress{0};
    std::atomic_flag failure_claim;
    std::exception_ptr error;
    std::vector<std::thread> workers;
    std::thread output;
    std::vector<std::unique_ptr<ThreadCpu>> worker_cpu;
    std::unique_ptr<ThreadCpu> output_cpu;

    static std::size_t capacity(HandoffOptions options) {
        require(options.jobs && options.jobs <= 65535 && options.bytes &&
                    options.bytes <= byte_mask,
                "invalid handoff options");
        return std::bit_ceil(options.jobs);
    }

    Impl(HandoffOptions settings, Write sink)
        : options(settings), write(std::move(sink)), mask(capacity(settings) - 1),
          slots(std::make_unique<Slot[]>(mask + 1)) {
        require(bool(write), "missing output callback");
        try {
            workers.reserve(options.workers);
            worker_cpu.reserve(options.workers);
            for (std::size_t i = 0; i < options.workers; ++i) {
                worker_cpu.push_back(std::make_unique<ThreadCpu>());
                workers.emplace_back([this, counter = worker_cpu.back().get()] {
                    const ThreadCpu::Measurement measurement{*counter};
                    encode_loop();
                });
            }
            if (options.workers) {
                output_cpu = std::make_unique<ThreadCpu>();
                output = std::thread([this] {
                    const ThreadCpu::Measurement measurement{*output_cpu};
                    output_loop();
                });
            }
        } catch (...) {
            stop();
            throw;
        }
    }

    void check() const {
        const auto stopping = stopped.load(std::memory_order_acquire);
        if (failed.load(std::memory_order_acquire)) {
            std::rethrow_exception(error);
        }
        require(!stopping, "handoff is stopped");
    }

    void fail(std::exception_ptr failure) noexcept {
        if (!failure_claim.test_and_set(std::memory_order_relaxed)) {
            error = std::move(failure);
            failed.store(true, std::memory_order_release);
            stopped.store(true, std::memory_order_release);
            notify();
        }
    }

    void notify() noexcept {
        progress.fetch_add(1, std::memory_order_release);
        progress.notify_all();
    }

    void reserve(std::size_t charge) {
        require(charge && charge <= byte_mask, "unrepresentable encoding job charge");
        const auto begin = Clock::now();
        bool blocked = false;
        try {
            for (;;) {
                const auto observed = progress.load(std::memory_order_acquire);
                check();
                auto current = budget.load(std::memory_order_acquire);
                const auto jobs = current >> count_shift;
                const auto bytes = current & byte_mask;
                const bool fits = jobs < options.jobs &&
                                  (charge > options.bytes
                                       ? jobs == 0
                                       : bytes <= options.bytes && charge <= options.bytes - bytes);
                if (fits) {
                    const auto next = current + (std::uint64_t{1} << count_shift) + charge;
                    if (budget.compare_exchange_weak(current, next, std::memory_order_acq_rel)) {
                        maximum(peak_jobs, jobs + 1);
                        maximum(peak_bytes, bytes + charge);
                        if (charge > options.bytes) {
                            ++oversized;
                        }
                        break;
                    }
                } else {
                    if (!blocked) {
                        blocked = true;
                        ++waits;
                        ++waiting;
                    }
                    progress.wait(observed, std::memory_order_acquire);
                }
            }
        } catch (...) {
            if (blocked) {
                --waiting;
            }
            throw;
        }
        if (blocked) {
            --waiting;
            wait_nanoseconds.fetch_add(
                std::chrono::duration_cast<std::chrono::nanoseconds>(Clock::now() - begin).count(),
                std::memory_order_relaxed);
        }
    }

    void submit(SectionRequest section, std::size_t charge, Encode encode) {
        require(bool(encode), "missing encoding callback");
        reserve(charge);
        const auto ticket = accepted.fetch_add(1, std::memory_order_acq_rel);
        auto& slot = slots[ticket & mask];
        for (;;) {
            const auto observed = progress.load(std::memory_order_acquire);
            check();
            if (slot.state.load(std::memory_order_acquire) == State::empty) {
                break;
            }
            progress.wait(observed, std::memory_order_acquire);
        }
        slot.section = section;
        slot.charge = charge;
        slot.encode = std::move(encode);
        if (options.workers) {
            slot.state.store(State::queued, std::memory_order_release);
            notify();
            return;
        }
        for (;;) {
            const auto observed = progress.load(std::memory_order_acquire);
            check();
            if (written.load(std::memory_order_acquire) == ticket) {
                break;
            }
            progress.wait(observed, std::memory_order_acquire);
        }
        try {
            encode_slot(slot);
            write_slot(slot);
        } catch (...) {
            fail(std::current_exception());
            throw;
        }
    }

    void encode_slot(Slot& slot) {
        slot.state.store(State::encoding, std::memory_order_relaxed);
        maximum(peak_encoding, encoding.fetch_add(1, std::memory_order_relaxed) + 1);
        try {
            slot.bytes = slot.encode();
            slot.encode = {};
        } catch (...) {
            --encoding;
            throw;
        }
        --encoding;
        maximum(peak_ready, ready.fetch_add(1, std::memory_order_relaxed) + 1);
        slot.state.store(State::ready, std::memory_order_release);
        if (options.workers) {
            notify();
        }
    }

    void write_slot(Slot& slot) {
        --ready;
        write(slot.section, slot.bytes);
        vtr::detail::wire::Bytes{}.swap(slot.bytes);
        const auto charge = slot.charge;
        slot.state.store(State::empty, std::memory_order_release);
        budget.fetch_sub((std::uint64_t{1} << count_shift) + charge, std::memory_order_release);
        written.fetch_add(1, std::memory_order_release);
        notify();
    }

    void encode_loop() noexcept {
        for (;;) {
            const auto observed = progress.load(std::memory_order_acquire);
            if (stopped.load(std::memory_order_acquire)) {
                return;
            }
            auto ticket = next_encode.load(std::memory_order_relaxed);
            if (ticket >= accepted.load(std::memory_order_acquire)) {
                progress.wait(observed, std::memory_order_acquire);
                continue;
            }
            if (!next_encode.compare_exchange_weak(ticket, ticket + 1, std::memory_order_relaxed)) {
                continue;
            }
            auto& slot = slots[ticket & mask];
            for (;;) {
                const auto queued = progress.load(std::memory_order_acquire);
                if (stopped.load(std::memory_order_acquire)) {
                    return;
                }
                if (slot.state.load(std::memory_order_acquire) == State::queued) {
                    break;
                }
                progress.wait(queued, std::memory_order_acquire);
            }
            try {
                encode_slot(slot);
            } catch (...) {
                fail(std::current_exception());
            }
        }
    }

    void output_loop() noexcept {
        for (;;) {
            const auto observed = progress.load(std::memory_order_acquire);
            if (stopped.load(std::memory_order_acquire)) {
                return;
            }
            auto& slot = slots[written.load(std::memory_order_relaxed) & mask];
            if (slot.state.load(std::memory_order_acquire) != State::ready) {
                progress.wait(observed, std::memory_order_acquire);
                continue;
            }
            try {
                write_slot(slot);
            } catch (...) {
                fail(std::current_exception());
            }
        }
    }

    void drain() {
        const auto target = accepted.load(std::memory_order_acquire);
        for (;;) {
            const auto observed = progress.load(std::memory_order_acquire);
            check();
            if (written.load(std::memory_order_acquire) >= target) {
                return;
            }
            progress.wait(observed, std::memory_order_acquire);
        }
    }

    void stop() noexcept {
        stopped.store(true, std::memory_order_release);
        notify();
        for (auto& worker : workers) {
            if (worker.joinable()) {
                worker.join();
            }
        }
        if (output.joinable()) {
            output.join();
        }
        for (std::size_t i = 0; i <= mask; ++i) {
            slots[i].encode = {};
            slots[i].bytes = {};
            slots[i].state.store(State::empty, std::memory_order_relaxed);
        }
        budget.store(0, std::memory_order_relaxed);
        encoding.store(0, std::memory_order_relaxed);
        ready.store(0, std::memory_order_relaxed);
    }

    HandoffStats stats() const {
        HandoffStats result;
        const auto allocation = budget.load(std::memory_order_acquire);
        result.jobs = allocation >> count_shift;
        result.bytes = allocation & byte_mask;
        result.written = written.load(std::memory_order_acquire);
        result.accepted = accepted.load(std::memory_order_acquire);
        result.encoding = encoding.load(std::memory_order_relaxed);
        result.ready = ready.load(std::memory_order_relaxed);
        result.waiting_submitters = waiting.load(std::memory_order_relaxed);
        result.peak_jobs = peak_jobs.load(std::memory_order_relaxed);
        result.peak_bytes = peak_bytes.load(std::memory_order_relaxed);
        result.peak_encoding = peak_encoding.load(std::memory_order_relaxed);
        result.peak_ready = peak_ready.load(std::memory_order_relaxed);
        result.wait_count = waits.load(std::memory_order_relaxed);
        result.wait_nanoseconds = wait_nanoseconds.load(std::memory_order_relaxed);
        result.oversized_jobs = oversized.load(std::memory_order_relaxed);
        const auto add_cpu = [&](const ThreadCpu& counter) {
            if (!result.worker_cpu_nanoseconds) return;
            if (const auto cpu = counter.nanoseconds()) *result.worker_cpu_nanoseconds += *cpu;
            else result.worker_cpu_nanoseconds.reset();
        };
        for (const auto& counter : worker_cpu) add_cpu(*counter);
        if (output_cpu) add_cpu(*output_cpu);
        return result;
    }
};

Handoff::Handoff(HandoffOptions options, Write write)
    : impl_(std::make_unique<Impl>(options, std::move(write))) {}

Handoff::~Handoff() {
    impl_->stop();
}

void Handoff::submit(SectionRequest section, std::size_t charge, Encode encode) {
    impl_->submit(section, charge, std::move(encode));
}

void Handoff::drain() {
    impl_->drain();
}

void Handoff::stop() noexcept {
    impl_->stop();
}

bool Handoff::failed() const noexcept {
    return impl_->failed.load(std::memory_order_acquire);
}

void Handoff::check_failure() const {
    if (failed()) {
        impl_->check();
    }
}

HandoffStats Handoff::stats() const {
    return impl_->stats();
}

} // namespace vtr::detail::engine::detail
