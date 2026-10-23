#pragma once
#include "numeric_attributes.h"
#include "records.h"

namespace vtr::detail::engine {
using Attributes = vtr::detail::wire::Attributes;

struct EmbeddedEvent {
    RecordId id;
    Timestamp time;
    vtr::detail::wire::Id name;
    Attributes attributes;
};

struct EmbeddedStage {
    RecordId id;
    vtr::detail::wire::Id name, lane;
    Timestamp begin;
    std::optional<Timestamp> end;
    Attributes attributes;
};

struct Transaction {
    RecordId id = 0, parent = 0;
    GeneratorId generator = UINT32_MAX;
    Timestamp begin = 0, end = 0;
    std::uint8_t status = 0, span_kind = 0;
    Bytes payload;
    Attributes attributes;
    std::vector<EmbeddedEvent> events;
    std::vector<EmbeddedStage> stages;
    std::vector<std::uint64_t> child_blocks;
};

struct Relation {
    vtr::detail::wire::Id kind;
    RecordId source, destination;
    Attributes attributes;
};

struct TransactionBounds {
    std::uint64_t transactions = 0, relations = 0;
    RecordId min_id = 0, max_id = 0;
    Timestamp begin = 0, end = 0;
    RecordId min_parent = 0, max_parent = 0, min_embedded = 0, max_embedded = 0;
    RecordId min_from = 0, max_from = 0, min_to = 0, max_to = 0;
    friend bool operator==(const TransactionBounds&, const TransactionBounds&) = default;
};

struct TransactionPage : TransactionBounds {
    std::uint32_t blob_offset, blob_length, raw_length;
};

struct TransactionQuery {
    std::optional<GeneratorId> generator = {};
    std::optional<RecordId> parent = {}, id = {}, embedded_id = {};
    Timestamp begin = 0, end = UINT64_MAX;
};

// A workspace belongs to one assembly thread and retains no borrowed rows between blocks.
struct TransactionWorkspace {
    std::array<Bytes, 33 + 7 + 5> buffers;
    std::vector<const Transaction*> sorted, slots;
};

Bytes encode_transaction_block(std::uint64_t number, const Types& types,
                               const Generators& generators,
                               std::span<const Transaction> transactions,
                               std::span<const Relation> relations, RecordPolicy policy = {},
                               std::uint64_t string_count = UINT64_MAX,
                               NumericAttributesView numeric = {},
                               const TextReferences* shared_text = nullptr,
                               TransactionWorkspace* workspace = nullptr);

class TransactionBlockView {
  public:
    TransactionBlockView(View encoded, const Types& types, const Generators& generators,
                         std::uint64_t string_count = UINT64_MAX,
                         std::size_t max_payload_bytes = default_payload_limit,
                         std::span<const std::string> shared_text = {});

    std::uint64_t number() const noexcept {
        return number_;
    }

    const TransactionBounds& bounds() const noexcept {
        return bounds_;
    }

    std::span<const TransactionPage> pages() const noexcept {
        return pages_;
    }

    void transactions(TransactionQuery query,
                      const std::function<bool(const Transaction&)>& visit) const;
    void relations(RecordId endpoint, bool incoming,
                   const std::function<bool(const Relation&)>& visit) const;
    void scan(const std::function<bool(const Transaction&)>& tx,
              const std::function<bool(const Relation&)>& relation) const;
    void validate() const;
    std::optional<std::pair<Timestamp, Timestamp>> time_range() const;

  private:
    bool decode_page(const TransactionPage& page, const std::function<bool(const Transaction&)>& tx,
                     const std::function<bool(const Relation&)>& relation) const;
    View blobs_;
    std::span<const std::string> shared_text_;
    const Types& types_;
    const Generators& generators_;
    std::uint64_t number_, string_count_;
    std::size_t max_payload_bytes_;
    TransactionBounds bounds_;
    std::vector<GeneratorId> generator_ids_;
    std::vector<TransactionPage> pages_;
};
} // namespace vtr::detail::engine
