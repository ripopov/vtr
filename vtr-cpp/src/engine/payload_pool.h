#pragma once

#include "engine/transactions.h"
#include <atomic>
#include <memory>
#include <type_traits>
#include <utility>

namespace vtr::detail::engine::detail {

// Encoders return owned storage; only the assembly thread takes it back.
class PayloadPool {
    struct Returned {
        Returned* next = nullptr;
        std::vector<Bytes> payloads;
        std::vector<Transaction> transactions;
        std::vector<LocatedRecord> records;
        std::vector<Relation> relations;
    };

    const std::size_t limit_;
    std::atomic<std::size_t> charge_{0};
    std::atomic<Returned*> returned_{nullptr};
    Returned* available_ = nullptr;

    template <class Row> static std::vector<Row>& cache(Returned& node) {
        if constexpr (std::is_same_v<Row, Transaction>) {
            return node.transactions;
        } else if constexpr (std::is_same_v<Row, LocatedRecord>) {
            return node.records;
        } else {
            return node.relations;
        }
    }

    bool reserve(std::size_t bytes) {
        auto current = charge_.load(std::memory_order_relaxed);
        do {
            if (bytes > limit_ - current) {
                return false;
            }
        } while (
            !charge_.compare_exchange_weak(current, current + bytes, std::memory_order_relaxed));
        return true;
    }

    void collect() {
        auto* node = returned_.exchange(nullptr, std::memory_order_acquire);
        while (node) {
            auto* next = node->next;
            node->next = available_;
            available_ = node;
            node = next;
        }
    }

    void remove_empty() {
        auto** link = &available_;
        while (*link) {
            auto* node = *link;
            if (!node->payloads.capacity() && !node->transactions.capacity() &&
                !node->records.capacity() && !node->relations.capacity()) {
                *link = node->next;
                charge_.fetch_sub(sizeof(Returned), std::memory_order_relaxed);
                delete node;
            } else {
                link = &node->next;
            }
        }
    }

  public:
    explicit PayloadPool(std::size_t limit) : limit_(limit) {}

    ~PayloadPool() {
        if (!limit_)
            return;
        collect();
        while (available_) {
            auto* next = available_->next;
            delete available_;
            available_ = next;
        }
    }

    std::size_t take(std::vector<Bytes>& local) {
        if (!limit_) {
            return 0;
        }
        collect();
        for (auto* node = available_; node; node = node->next) {
            if (node->payloads.empty()) {
                continue;
            }
            std::size_t payload_bytes = 0;
            for (const auto& bytes : node->payloads) {
                payload_bytes += bytes.capacity();
            }
            charge_.fetch_sub(payload_bytes + node->payloads.capacity() * sizeof(Bytes),
                              std::memory_order_relaxed);
            local = std::move(node->payloads);
            remove_empty();
            return payload_bytes;
        }
        return 0;
    }

    template <class Row> void borrow(std::vector<Row>& rows) {
        if (!limit_ || rows.capacity()) {
            return;
        }
        collect();
        for (auto* node = available_; node; node = node->next) {
            auto& stored = cache<Row>(*node);
            if (!stored.capacity()) {
                continue;
            }
            charge_.fetch_sub(stored.capacity() * sizeof(Row), std::memory_order_relaxed);
            rows = std::move(stored);
            remove_empty();
            return;
        }
    }

    template <class Row> void recycle(std::vector<Row>& rows, bool retain_rows) {
        if (!limit_) {
            return;
        }
        const auto available = limit_ - charge_.load(std::memory_order_relaxed);
        if (available <= sizeof(Returned)) {
            return;
        }
        auto remaining = available - sizeof(Returned);
        const auto row_bytes = rows.capacity() * sizeof(Row);
        const bool retain = retain_rows && row_bytes <= remaining;
        if (retain) {
            remaining -= row_bytes;
        }
        const auto payload_budget = remaining;
        std::size_t count = 0, payload_bytes = 0;
        if constexpr (!std::is_same_v<Row, Relation>) {
            for (const auto& row : rows) {
                const auto bytes = row.payload.capacity();
                if (bytes && remaining >= sizeof(Bytes) && bytes <= remaining - sizeof(Bytes)) {
                    remaining -= bytes + sizeof(Bytes);
                    payload_bytes += bytes;
                    ++count;
                }
            }
        }
        if (!retain && !count) {
            return;
        }
        auto node = std::make_unique<Returned>();
        node->payloads.reserve(count);
        const auto total = sizeof(Returned) + node->payloads.capacity() * sizeof(Bytes) +
                           payload_bytes + (retain ? row_bytes : 0);
        if (!reserve(total)) {
            return;
        }
        remaining = payload_budget;
        if constexpr (!std::is_same_v<Row, Relation>) {
            for (auto& row : rows) {
                const auto bytes = row.payload.capacity();
                if (bytes && remaining >= sizeof(Bytes) && bytes <= remaining - sizeof(Bytes)) {
                    remaining -= bytes + sizeof(Bytes);
                    row.payload.clear();
                    node->payloads.push_back(std::move(row.payload));
                }
            }
        }
        rows.clear();
        if (retain) {
            cache<Row>(*node) = std::move(rows);
        }
        auto* head = returned_.load(std::memory_order_relaxed);
        do {
            node->next = head;
        } while (!returned_.compare_exchange_weak(head, node.get(), std::memory_order_release,
                                                  std::memory_order_relaxed));
        node.release();
    }

    std::size_t charge() const {
        return charge_.load(std::memory_order_relaxed);
    }
};
} // namespace vtr::detail::engine::detail
