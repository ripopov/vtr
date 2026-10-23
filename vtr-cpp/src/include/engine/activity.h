#pragma once
#include "container.h"

namespace vtr::detail::engine {
struct ActivityStretch {
    Timestamp begin, end, gap;
};

class ActivityIndex {
  public:
    static ActivityIndex build(const ContainerReader& trace, std::size_t memory_limit = 256 << 20);
    static ActivityIndex decode(const ContainerReader& trace, View bytes,
                                std::size_t memory_limit = 256 << 20);
    Bytes encode(const ContainerReader& trace) const;
    std::vector<vtr::detail::wire::Id> active(Timestamp begin, Timestamp end) const;

    const vtr::detail::wire::TraceFingerprint& fingerprint() const noexcept {
        return fingerprint_;
    }

  private:
    vtr::detail::wire::TraceFingerprint fingerprint_{};
    Timestamp minimum_ = 0;
    std::vector<std::vector<ActivityStretch>> signals_;
};
} // namespace vtr::detail::engine
