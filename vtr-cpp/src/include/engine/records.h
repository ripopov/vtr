#pragma once
#include "schema.h"
#include <functional>

namespace vtr::detail::engine {
using RecordId = std::uint64_t;
using Timestamp = std::uint64_t;
using GeneratorId = std::uint32_t;
enum class RecordKind : std::uint8_t {
    ordinary,
    clock,
    event,
    stage,
    log,
    embedded_event,
    embedded_stage
};

struct Generator {
    GeneratorId id;
    RecordKind kind;
    TypeId payload_type = 0;
};

class Generators {
  public:
    void declare(Generator generator);
    void declare_callsite(std::uint32_t id, TypeId datatype);
    TypeId callsite_type(std::uint32_t id) const;
    TypeId payload_type(GeneratorId generator, std::optional<std::uint32_t> callsite) const;
    const Generator& operator[](GeneratorId id) const {
        if (id < prefix_.size() && prefix_[id])
            return *prefix_[id];
        return lookup(id);
    }

    std::size_t size() const noexcept {
        return generators_.size();
    }

    std::size_t callsite_count() const noexcept {
        return callsites_.size();
    }

  private:
    std::unordered_map<GeneratorId, Generator> generators_;
    std::unordered_map<std::uint32_t, TypeId> callsites_;
    std::array<std::optional<Generator>, 256> prefix_;
    const Generator& lookup(GeneratorId id) const;
};

struct LocatedRecord {
    RecordId id = 0;
    GeneratorId generator = UINT32_MAX;
    // The reserved absent ID fits the alignment gap and preserves fixed-source buffer budgets.
    std::uint32_t callsite_id = UINT32_MAX;
    Timestamp begin = 0, end = 0;
    RecordId parent = 0;
    Bytes payload;

    LocatedRecord() = default;

    LocatedRecord(RecordId id, GeneratorId generator, Timestamp begin, Timestamp end,
                  RecordId parent, Bytes payload, std::optional<std::uint32_t> callsite = {})
        : id(id), generator(generator), callsite_id(callsite.value_or(UINT32_MAX)), begin(begin),
          end(end), parent(parent), payload(std::move(payload)) {}

    std::optional<std::uint32_t> callsite() const noexcept {
        return callsite_id == UINT32_MAX ? std::nullopt : std::optional(callsite_id);
    }

    friend bool operator==(const LocatedRecord&, const LocatedRecord&) = default;
};

struct RecordPolicy {
    std::size_t page_bytes = 64 << 10;
    vtr::detail::wire::Codec codec = vtr::detail::wire::Codec::zstd;
    int compression_level = 1;
    bool separate_logs = true, defer_page_size_scans = true;
};

struct SegmentHeader {
    GeneratorId generator;
    std::uint32_t page, count, raw_offset, raw_length;
    RecordId min_id, max_id, min_parent, max_parent;
    Timestamp begin, end;
    std::optional<std::uint32_t> callsite = {};
};

struct PageHeader {
    std::uint32_t blob_offset, blob_length, raw_length;
};

struct RecordQuery {
    std::optional<GeneratorId> generator;
    std::optional<RecordId> parent;
    std::optional<RecordId> id = {};
    std::optional<RecordKind> kind = {};
    Timestamp begin = 0, end = UINT64_MAX;
    bool include_logs = true;
    std::optional<std::uint32_t> callsite = {};
};

Bytes encode_record_block(std::uint64_t number, const Types& types, const Generators& generators,
                          std::span<const LocatedRecord> records, RecordPolicy policy = {},
                          std::uint64_t string_count = UINT64_MAX);

// The caller keeps the encoded section alive for this view's lifetime.
class RecordBlockView {
  public:
    RecordBlockView(View encoded, const Types& types, const Generators& generators,
                    std::uint64_t string_count = UINT64_MAX,
                    std::size_t max_payload_bytes = default_payload_limit);

    std::uint64_t number() const noexcept {
        return number_;
    }

    std::uint64_t count() const noexcept {
        return count_;
    }

    Timestamp begin() const noexcept {
        return begin_;
    }

    Timestamp end() const noexcept {
        return end_;
    }

    RecordId min_id() const noexcept {
        return min_id_;
    }

    RecordId max_id() const noexcept {
        return max_id_;
    }

    std::span<const SegmentHeader> segments() const noexcept {
        return segments_;
    }

    std::span<const PageHeader> pages() const noexcept {
        return pages_;
    }

    void query(RecordQuery query, const std::function<bool(const LocatedRecord&)>& visit) const;
    void validate() const;

  private:
    View encoded_, blobs_;
    const Types& types_;
    const Generators& generators_;
    std::uint64_t number_, count_, string_count_;
    std::size_t max_payload_bytes_;
    RecordId min_id_, max_id_;
    Timestamp begin_, end_;
    std::vector<SegmentHeader> segments_;
    std::vector<PageHeader> pages_;
};
} // namespace vtr::detail::engine
