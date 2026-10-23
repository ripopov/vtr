#pragma once

// Selective queries with immutable retained records and payloads.
// Semantics: vtr/docs/SPEC.md and vtr/docs/TRANSACTIONS.md.

#include "vtr_common.h"

#include <bit>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <functional>
#include <limits>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <variant>
#include <vector>

// clang-format off
/*
    #include "vtr_reader.h"
    #include <iostream>

    int main(int argc, char** argv) {
        namespace vt = vtr;
        vt::Reader input(argc > 1 ? argv[1] : "run.vtr");
        auto execute = input.find_generator("core0.execute");
        if (!execute) {
            std::cerr << "No core0.execute generator in this file\n";
            return 1;
        }

        std::optional<vt::Record> selected;
        input.records({.generator = *execute, .kind = vt::RecordKind::stage, .time = {10, 20}},
                      [&](const vt::Record& visit) {
                          selected = visit;
                          return vt::Flow::stop;
                      });
        if (!selected) {
            std::cout << "No execute visits in [10, 20]\n";
            return 0;
        }

        auto owner = input.lookup(*selected->parent());
        if (!owner) {
            std::cerr << "Visit owner is missing from this incomplete file\n";
            return 1;
        }
        auto pc = owner->payload()["pc"].scalar();
        auto activity = input.expand(owner->id());
        std::cout << "Instruction " << owner->id().value
                  << " pc=" << std::get<std::uint64_t>(pc.data) << " executed from "
                  << selected->begin() << " to " << selected->end() << "; "
                  << activity.located.size() << " located records, "
                  << activity.missing_blocks.size() << " missing blocks\n";
    }
*/
// clang-format on

namespace vtr {

class PayloadView;

namespace detail {
enum class PayloadKind { scalar, structure, array, sequence, enumeration };
template <class T> T decode_owned(const PayloadView& view, std::size_t& budget);
struct RecordStorage;
struct PayloadStorage;
} // namespace detail

// Copies retain immutable storage. Views obtained from this object remain valid
// while it is held. as<T>() returns owning C++ values; view members (including
// string_view) are rejected there, but remain valid inputs to typed capture.
class PayloadView {
  public:
    [[nodiscard]] TypeId type() const noexcept;
    [[nodiscard]] std::size_t size() const; // Number of aggregate elements; zero for scalars.
    [[nodiscard]] PayloadView operator[](std::string_view field) const;
    [[nodiscard]] PayloadView operator[](std::size_t index) const;
    [[nodiscard]] Value scalar() const;
    template <class T> [[nodiscard]] T as() const;

  private:
    friend class Record;
    template <class T> friend T detail::decode_owned(const PayloadView&, std::size_t&);
    std::optional<ByteView> fixed_array(TypeId element, std::size_t count) const;
    PayloadView first_element() const;
    PayloadView next_element() const;
    bool matches(detail::PayloadKind kind, std::string_view name = {}) const;
    std::size_t allocation_limit() const noexcept;
    PayloadView(std::shared_ptr<const detail::PayloadStorage> storage, std::size_t offset,
                TypeId type) noexcept;
    std::shared_ptr<const detail::PayloadStorage> storage_;
    std::size_t offset_ = 0;
    TypeId type_{0};
};

struct EmbeddedEvent {
    RecordId id;
    Timestamp time;
    StringId name;
    Attributes data;
};

struct EmbeddedStage {
    RecordId id;
    StringId name;
    StringId lane;
    Timestamp begin;
    std::optional<Timestamp> end;
    Attributes data;
};

// An immutable retained record, valid after cache eviction or reader destruction.
// Located events/logs have begin=end; embedded records have no generator.
// Non-ordinary records expose unset/unspecified classification, except an open
// clock stretch. Embedded detail/child_blocks are empty on non-owner records.
// name/lane/explicit_end describe embedded detail; end() always gives the
// effective end, including an embedded stage with no explicit end.
class Record {
  public:
    [[nodiscard]] RecordId id() const noexcept;
    [[nodiscard]] RecordKind kind() const noexcept;
    [[nodiscard]] std::optional<RecordId> parent() const noexcept;
    [[nodiscard]] std::optional<GeneratorId> generator() const noexcept;
    [[nodiscard]] Timestamp begin() const noexcept;
    [[nodiscard]] Timestamp end() const noexcept;
    [[nodiscard]] vtr::Status status() const noexcept;
    [[nodiscard]] vtr::SpanKind span_kind() const noexcept;
    [[nodiscard]] PayloadView payload() const;
    [[nodiscard]] AttributeView attributes() const noexcept;
    [[nodiscard]] std::span<const EmbeddedEvent> events() const noexcept;
    [[nodiscard]] std::span<const EmbeddedStage> stages() const noexcept;
    [[nodiscard]] std::span<const BlockId> child_blocks() const noexcept;
    [[nodiscard]] std::optional<StringId> name() const noexcept;
    [[nodiscard]] std::optional<StringId> lane() const noexcept;
    [[nodiscard]] std::optional<Timestamp> explicit_end() const noexcept;

  private:
    friend class Reader;
    explicit Record(std::shared_ptr<const detail::RecordStorage> storage);
    std::shared_ptr<const detail::RecordStorage> storage_;
};

// Closed query windows include points at either endpoint. Positive-duration
// stages match begin<=last && end>first; other intervals match inclusively.
// Embedded timestamps do not cause an owner to match an owner-interval query.
// first > last is an argument error.
struct TimeWindow {
    Timestamp first = 0;
    Timestamp last = std::numeric_limits<Timestamp>::max();
};

struct TransactionQuery {
    std::optional<GeneratorId> generator = {};
    std::optional<RecordId> parent = {};
    TimeWindow time = {};
};

struct RecordQuery {
    std::optional<GeneratorId> generator = {};
    std::optional<RecordId> parent = {};
    std::optional<RecordKind> kind = {};
    TimeWindow time = {};
};

enum class Flow { next, stop };
enum class Direction { outgoing, incoming };

struct Relation {
    StringId kind;
    std::optional<RecordId> from;
    std::optional<RecordId> to;
    Attributes data;
};

struct Expansion {
    Record owner;
    std::vector<Record> located;
    std::vector<BlockId> missing_blocks;
    bool complete;
};

enum class NodeKind { scope, signal, generator, enum_table };

struct Node {
    NodeId id;
    NodeKind kind;
    StringId name;
    std::optional<ScopeId> parent;
    std::optional<SignalId> signal;
    bool alias = false;
    Attributes metadata;
    std::optional<ScopeKind> scope_kind;
    std::optional<StringId> component;
    std::optional<VariableDeclaration> variable;
    std::vector<EnumEntry> enum_entries;
};

struct Generator {
    GeneratorId id;
    RecordKind kind;
    StringId kind_name;
    TypeId payload;
    std::optional<GeneratorId> clock;
};

struct SignalSample {
    SignalKind kind;
    Bytes packed;
    [[nodiscard]] std::uint64_t as_u64() const;
    [[nodiscard]] double as_real() const;
    [[nodiscard]] std::string as_text() const;
};

// Change visitors borrow decoded page storage; copy packed bytes to retain a sample.
struct SignalView {
    SignalKind kind;
    ByteView packed;

    [[nodiscard]] std::uint64_t as_u64() const {
        if constexpr (std::endian::native == std::endian::little) {
            if (kind.encoding == SignalEncoding::bits && kind.alphabet == States::binary &&
                kind.width && kind.width <= 64 && packed.size() == (kind.width + 7) / 8) {
                std::uint64_t value = 0;
                switch (packed.size()) {
                case 1:
                    return std::to_integer<std::uint8_t>(packed[0]);
                case 2: {
                    std::uint16_t word;
                    std::memcpy(&word, packed.data(), 2);
                    return word;
                }
                case 4: {
                    std::uint32_t word;
                    std::memcpy(&word, packed.data(), 4);
                    return word;
                }
                case 8:
                    std::memcpy(&value, packed.data(), 8);
                    return value;
                default:
                    std::memcpy(&value, packed.data(), packed.size());
                    return value;
                }
            }
        }
        return checked_u64();
    }

    [[nodiscard]] double as_real() const;
    [[nodiscard]] std::string as_text() const;

  private:
    [[nodiscard]] std::uint64_t checked_u64() const;
};

// A change visitor borrows both this object and its packed bytes.
struct SignalChange {
    Timestamp time;
    SignalId signal;
    SignalView value;
};

struct ReaderOptions {
    bool verify_checksums = true;
    std::size_t max_payload_bytes = 64 << 20;
    std::size_t owner_cache_entries = 4'096;
    std::size_t owner_cache_bytes = 32 << 20;
};

struct CacheStats {
    std::size_t owner_entries = 0;
    std::size_t accounted_bytes = 0;
};

enum class Completion { complete, incomplete, unfinished };

struct Provenance {
    Completion state;
    std::uint64_t discarded_bytes = 0;
};
enum class MissingKind { parent, relation_source, relation_destination, child_block, clock };

struct MissingReference {
    MissingKind kind;
    std::optional<RecordId> referrer;
    std::variant<RecordId, BlockId, GeneratorId> target;
};

struct ValidationReport {
    Provenance ending;
    std::uint64_t records = 0;
    std::uint64_t relations = 0;
    std::vector<MissingReference> missing;
};

// Queries decode selectively, preserve repeated occurrences and stop when the
// visitor returns Flow::stop. Callback references last through that invocation;
// copying a record retains it. Query order is storage order, not timestamp order.
// Catalog/string views last until reader destruction. Serialize reader calls
// externally; checksum validation on access is distinct from validate().
class Reader {
  public:
    explicit Reader(const std::filesystem::path& path, ReaderOptions options = {});
    ~Reader();
    Reader(const Reader&) = delete;
    Reader& operator=(const Reader&) = delete;
    Reader(Reader&&) noexcept;
    Reader& operator=(Reader&&) noexcept;

    [[nodiscard]] RecordingId identity() const noexcept;
    [[nodiscard]] std::optional<vtr::Fingerprint> fingerprint() const noexcept;
    [[nodiscard]] Provenance ending() const noexcept;
    [[nodiscard]] const vtr::Metadata& metadata() const noexcept;
    [[nodiscard]] std::span<const Node> nodes() const noexcept;
    [[nodiscard]] std::span<const Generator> generators() const noexcept;
    [[nodiscard]] std::span<const DataType> types() const noexcept;
    [[nodiscard]] std::span<const SignalKind> signals() const noexcept;
    [[nodiscard]] std::string_view string(StringId id) const;
    [[nodiscard]] std::optional<NodeId> find(std::string_view path) const;
    [[nodiscard]] std::optional<SignalId> find_signal(std::string_view path) const;
    [[nodiscard]] std::optional<GeneratorId> find_generator(std::string_view path) const;

    [[nodiscard]] std::optional<Record> lookup(RecordId id) const;
    // transactions visits ordinary owners and clock stretches; records visits
    // located stages, events and logs. Embedded detail is reached through owners
    // or global lookup, never by a location query.
    void transactions(TransactionQuery query,
                      const std::function<Flow(const Record&)>& visit) const;
    void records(RecordQuery query, const std::function<Flow(const Record&)>& visit) const;
    void relations(RecordId endpoint, Direction direction,
                   const std::function<Flow(const Relation&)>& visit) const;

    // Expansion finds located EVENT/STAGE children via the owner's block list.
    // Ordinary children and logs use separate searches; they are not in that list.
    [[nodiscard]] Expansion expand(RecordId owner) const;
    void children(RecordId parent, const std::function<Flow(const Record&)>& visit) const;
    void logs(RecordId parent, TimeWindow time,
              const std::function<Flow(const Record&)>& visit) const;
    [[nodiscard]] std::string format_log(const Record& log) const;
    [[nodiscard]] std::optional<std::uint64_t> cycle_at(GeneratorId clock, Timestamp time) const;

    void changes(std::optional<SignalId> signal, TimeWindow time,
                 const std::function<Flow(const SignalChange&)>& visit) const;
    [[nodiscard]] std::optional<TimeWindow> waveform_time_range() const noexcept;
    // Includes embedded detail outside owners; reads time columns without materializing payloads.
    [[nodiscard]] std::optional<TimeWindow> time_range() const;
    [[nodiscard]] SignalSample value_at(SignalId signal, Timestamp time) const;

    // Full validation checks unloaded pages and complete-file invariants.
    // Corruption throws; unresolved references in incomplete evidence are reported.
    [[nodiscard]] ValidationReport validate() const;
    [[nodiscard]] vtr::CacheStats cache_stats() const noexcept;
    void clear_cache() const;

  private:
    friend class ActivityIndex;
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

// Source and destination must differ. Recovery retains only complete sections
// with valid integrity checks, preserving recording/record/block identities.
// The rebuilt directory remains incomplete; no observations are synthesized.
[[nodiscard]] Provenance recover(const std::filesystem::path& source,
                                 const std::filesystem::path& destination);

// Source-bound VTI 3.0 sidecars are derived data, separate from the 3.2 trace.
// Build/load require a finalized fingerprint; recovery invalidates old sidecars.
class ActivityIndex {
  public:
    [[nodiscard]] static ActivityIndex build(const Reader& trace,
                                             std::size_t memory_bytes = 256 << 20);
    [[nodiscard]] static ActivityIndex load(const Reader& trace, ByteView sidecar,
                                            std::size_t memory_bytes = 256 << 20);
    [[nodiscard]] Bytes encode() const;
    [[nodiscard]] vtr::Fingerprint fingerprint() const noexcept;
    [[nodiscard]] std::vector<SignalId> active(TimeWindow time) const;

  private:
    struct Impl;
    explicit ActivityIndex(std::shared_ptr<const Impl> storage) noexcept;
    std::shared_ptr<const Impl> impl_;
};

namespace detail {

inline void consume_allocation(std::size_t count, std::size_t size, std::size_t& budget) {
    if (size && count > budget / size) {
        throw Error(ErrorCode::resource_limit, "Decoded C++ value exceeds the allocation budget");
    }
    budget -= count * size;
}

template <class T> T decode_owned(const PayloadView& view, std::size_t& budget) {
    static_assert(!std::same_as<T, std::string_view> && !SequenceTraits<T>::borrowed,
                  "PayloadView::as<T>() requires owning members");
    if constexpr (std::same_as<T, Unit>) {
        if (!std::holds_alternative<std::monostate>(view.scalar().data)) {
            throw Error(ErrorCode::invalid_argument, "Payload is not unit");
        }
        return {};
    } else if constexpr (std::integral<T> && !std::same_as<T, bool>) {
        using Integers = std::conditional_t<
            std::is_signed_v<T>, std::tuple<std::int8_t, std::int16_t, std::int32_t, std::int64_t>,
            std::tuple<std::uint8_t, std::uint16_t, std::uint32_t, std::uint64_t>>;
        using Canonical = std::tuple_element_t<std::countr_zero(sizeof(T)), Integers>;
        auto scalar = view.scalar();
        auto value = std::get_if<Canonical>(&scalar.data);
        if (!value) {
            throw Error(ErrorCode::invalid_argument, "Payload integer type does not match T");
        }
        return static_cast<T>(*value);
    } else if constexpr (builtin_type<T>() != UINT32_MAX) {
        auto scalar = view.scalar();
        auto value = std::get_if<T>(&scalar.data);
        if (!value) {
            throw Error(ErrorCode::invalid_argument, "Payload scalar type does not match T");
        }
        if constexpr (std::same_as<T, std::string> || std::same_as<T, Bytes>) {
            consume_allocation(value->size(), sizeof(typename T::value_type), budget);
        } else if constexpr (std::same_as<T, Bits> || std::same_as<T, Logic> ||
                             std::same_as<T, Logic9>) {
            consume_allocation(value->packed.size(), 1, budget);
        }
        return std::move(*value);
    } else if constexpr (std::is_enum_v<T>) {
        if (!view.matches(PayloadKind::enumeration, Schema<T>::name)) {
            throw Error(ErrorCode::invalid_argument, "Payload enum schema does not match T");
        }
        return static_cast<T>(decode_owned<std::underlying_type_t<T>>(view, budget));
    } else if constexpr (ArrayTraits<T>::value) {
        if (!view.matches(PayloadKind::array) || view.size() != ArrayTraits<T>::size) {
            throw Error(ErrorCode::invalid_argument, "Payload array size does not match T");
        }
        using Element = typename ArrayTraits<T>::Element;
        if constexpr (std::endian::native == std::endian::little &&
                      ((std::integral<Element> && !std::same_as<Element, bool>) ||
                       std::same_as<Element, float> || std::same_as<Element, double>)) {
            if (auto bytes =
                    view.fixed_array(TypeId(builtin_type<Element>()), ArrayTraits<T>::size)) {
                T result{};
                if (!bytes->empty()) {
                    std::memcpy(result.data(), bytes->data(), bytes->size());
                }
                return result;
            }
        }
        return [&]<std::size_t... I>(std::index_sequence<I...>) {
            return T{decode_owned<typename ArrayTraits<T>::Element>(view[I], budget)...};
        }(std::make_index_sequence<ArrayTraits<T>::size>{});
    } else if constexpr (SequenceTraits<T>::value) {
        if (!view.matches(PayloadKind::sequence)) {
            throw Error(ErrorCode::invalid_argument, "Payload is not a sequence");
        }
        auto size = view.size();
        consume_allocation(size, sizeof(typename T::value_type), budget);
        T result;
        result.reserve(size);
        auto element = view.first_element();
        for (std::size_t i = 0; i < size; ++i) {
            result.push_back(decode_owned<typename T::value_type>(element, budget));
            if (i + 1 < size) {
                element = element.next_element();
            }
        }
        return result;
    } else {
        static_assert(requires { Schema<T>::fields; }, "Declare struct Schema<T>::fields");
        if (!view.matches(PayloadKind::structure, Schema<T>::name) ||
            view.size() != std::tuple_size_v<decltype(Schema<T>::fields)>) {
            throw Error(ErrorCode::invalid_argument, "Payload field count does not match T");
        }
        if constexpr (std::is_aggregate_v<T>) {
            return std::apply(
                [&](auto... item) {
                    return T{decode_owned<
                        std::remove_cvref_t<decltype(std::declval<T>().*(item.member))>>(
                        view[item.name], budget)...};
                },
                Schema<T>::fields);
        } else {
            T result{};
            std::apply(
                [&](auto... item) {
                    ((result.*(item.member) =
                          decode_owned<std::remove_cvref_t<decltype(result.*(item.member))>>(
                              view[item.name], budget)),
                     ...);
                },
                Schema<T>::fields);
            return result;
        }
    }
}

} // namespace detail

template <class T> T PayloadView::as() const {
    auto budget = allocation_limit();
    return detail::decode_owned<T>(*this, budget);
}

} // namespace vtr
