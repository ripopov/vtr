#pragma once

#include "engine/transactions.h"

namespace vtr::detail::engine::detail {

inline std::size_t memory_add(std::size_t a, std::size_t b) {
    return b > SIZE_MAX - a ? SIZE_MAX : a + b;
}

inline std::size_t memory_mul(std::size_t a, std::size_t b) {
    return a && b > SIZE_MAX / a ? SIZE_MAX : a * b;
}

inline std::size_t owned_attrs(const Attributes& attrs) {
    auto bytes = memory_mul(attrs.capacity(), sizeof(vtr::detail::wire::Attribute));
    for (const auto& a : attrs) {
        bytes = memory_add(bytes, a.value.payload.capacity());
    }
    return bytes;
}

inline std::size_t attrs_upper(std::span<const vtr::detail::wire::Attribute> attrs) {
    auto bytes = std::size_t(10);
    for (const auto& a : attrs) {
        bytes = memory_add(bytes, memory_add(32, a.value.payload.size()));
    }
    return bytes;
}

inline std::size_t owned_tx(const Transaction& tx) {
    auto bytes = memory_add(sizeof(tx), tx.payload.capacity());
    for (auto n : {owned_attrs(tx.attributes), memory_mul(tx.child_blocks.capacity(), 8),
                   memory_mul(tx.events.capacity(), sizeof(EmbeddedEvent)),
                   memory_mul(tx.stages.capacity(), sizeof(EmbeddedStage))}) {
        bytes = memory_add(bytes, n);
    }
    for (const auto& e : tx.events) {
        bytes = memory_add(bytes, owned_attrs(e.attributes));
    }
    for (const auto& stage : tx.stages) {
        bytes = memory_add(bytes, owned_attrs(stage.attributes));
    }
    return bytes;
}

inline std::size_t transaction_upper(const Transaction& tx, std::size_t numeric = 0,
                                     std::size_t framing = 512) {
    // Framing can grow by nine bytes per column. Canonical payload bytes expand
    // by at most ten when transposed to varints and first-use dictionary entries.
    auto bytes = memory_add(framing, memory_mul(tx.payload.size(), 10));
    bytes = memory_add(bytes, attrs_upper(tx.attributes));
    bytes = memory_add(bytes, memory_mul(numeric, 32));
    for (const auto& event : tx.events) {
        bytes = memory_add(bytes, memory_add(64, attrs_upper(event.attributes)));
    }
    for (const auto& stage : tx.stages) {
        bytes = memory_add(bytes, memory_add(96, attrs_upper(stage.attributes)));
    }
    return memory_add(bytes, memory_mul(tx.child_blocks.size(), 10));
}

} // namespace vtr::detail::engine::detail
