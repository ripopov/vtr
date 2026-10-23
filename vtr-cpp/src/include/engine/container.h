#pragma once
#include "transactions.h"

namespace vtr::detail::engine {
class ContainerReader {
  public:
    explicit ContainerReader(const std::string& path, bool verify_checksums = false,
                             std::size_t max_payload_bytes = default_payload_limit);

    const vtr::detail::wire::Reader& waveform() const noexcept {
        return native_;
    }

    const Types& types() const noexcept {
        return types_;
    }

    const Generators& generators() const noexcept {
        return generators_;
    }

    std::span<const vtr::detail::wire::ContainerSection> sections() const noexcept {
        return native_.sections();
    }

    View section_data(const vtr::detail::wire::ContainerSection& section,
                      bool verify = false) const {
        return native_.section_data(section, verify);
    }

    const vtr::detail::wire::ContainerSection* block(std::uint64_t number) const;
    const RecordBlockView* record_block(std::uint64_t number) const;
    const TransactionBlockView* transaction_block(std::uint64_t number) const;

    void verify_section(const vtr::detail::wire::ContainerSection& section) const {
        native_.section_data(section, true);
    }

    void records(RecordQuery query, const std::function<bool(const LocatedRecord&)>& visit) const;
    void transactions(TransactionQuery query,
                      const std::function<bool(const Transaction&)>& visit) const;
    void relations(RecordId endpoint, bool incoming,
                   const std::function<bool(const Relation&)>& visit) const;
    void validate_record_blocks() const;
    void validate_transaction_blocks() const;

  private:
    vtr::detail::wire::Reader native_;
    Types types_;
    Generators generators_;
    std::unordered_map<std::uint64_t, std::size_t> blocks_;
    std::unordered_map<std::uint64_t, std::uint64_t> block_strings_;
    std::unordered_map<std::uint64_t, std::unique_ptr<RecordBlockView>> record_views_;
    std::unordered_map<std::uint64_t, std::unique_ptr<TransactionBlockView>> transaction_views_;
};

RecordKind record_kind(std::string_view kind);

// Recovery preserves section bytes and block numbers; the directory marks the
// result incomplete so unresolved references cannot imply a normal finalization.
void recover_container(const std::string& input, const std::string& output);
} // namespace vtr::detail::engine
