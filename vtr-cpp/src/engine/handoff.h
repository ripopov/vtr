#pragma once

#include <memory>
#include <wire.h>

namespace vtr::detail::engine::detail {
struct SectionRequest {
    std::uint32_t kind = 0, flags = 0;
    std::uint64_t begin = 0, end = 0, block_number = 0;
};

struct HandoffOptions {
    std::size_t workers = 0, jobs = 8, bytes = 64 << 20;
};

using HandoffStats = vtr::detail::wire::HandoffStats;

class Handoff {
  public:
    using Encode = std::function<vtr::detail::wire::Bytes()>;
    using Write = std::function<void(const SectionRequest&, std::span<const std::uint8_t>)>;
    Handoff(HandoffOptions options, Write write);
    ~Handoff();
    Handoff(const Handoff&) = delete;
    Handoff& operator=(const Handoff&) = delete;
    // Callbacks cannot wait for their own output or submit into this bounded queue.
    void submit(SectionRequest section, std::size_t memory_charge, Encode encode);
    void drain();
    void stop() noexcept;
    bool failed() const noexcept;
    void check_failure() const;
    HandoffStats stats() const;

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
} // namespace vtr::detail::engine::detail
