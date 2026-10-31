#pragma once
#include "container.h"
#include <variant>

namespace vtr::detail::engine {
using Id = vtr::detail::wire::Id;
using TraceId = vtr::detail::wire::TraceId;
using TraceFingerprint = vtr::detail::wire::TraceFingerprint;
using HandoffStats = vtr::detail::wire::HandoffStats;
using Value = vtr::detail::wire::Value;
using Tag = vtr::detail::wire::Tag;
using SignalKind = vtr::detail::wire::SignalKind;
using SignalValue = vtr::detail::wire::SignalValue;
using SignalUpdate = vtr::detail::wire::SignalUpdate;
inline constexpr Id none = vtr::detail::wire::none;

struct WriterOptions {
    vtr::detail::wire::Codec codec = vtr::detail::wire::Codec::zstd;
    int compression_level = 1;
    std::size_t page_bytes = 64 << 10, block_bytes = 512 << 10;
    std::uint32_t record_limit = 65536;
    std::uint64_t wave_records = 1 << 24;
    std::size_t wave_bytes = 16 << 20;
    std::uint32_t group_size = 256;
    bool checksums = true, separate_logs = true, separate_clocks = true, dedup = true;
    std::uint32_t workers = 0, queue_jobs = 8;
    std::size_t queue_bytes = 256 << 20;
    std::optional<std::size_t> payload_pool_bytes = {};
};

struct WriterStats {
    std::size_t active_owners = 0, active_stages = 0, active_payload_bytes = 0;
    std::uint64_t transaction_blocks = 0, record_blocks = 0, max_created_id = 0;
    std::size_t peak_record_buffer_bytes = 0, peak_transaction_buffer_bytes = 0;
    std::size_t wave_buffer_bytes = 0, peak_wave_buffer_bytes = 0;
    std::size_t payload_pool_bytes = 0;
    std::uint64_t payload_reuses = 0;
};

class Writer {
  public:
    explicit Writer(const std::string& path, WriterOptions options = {});
    ~Writer();
    Writer(const Writer&) = delete;
    Writer& operator=(const Writer&) = delete;
    const TraceId& trace_id() const noexcept;
    vtr::detail::wire::Meta& metadata();
    Id intern(std::string_view value);
    Id add_scope(Id parent, std::string_view name, std::uint16_t type = 0,
                 std::string_view component = {});
    std::pair<Id, Id> add_var(Id parent, std::string_view name, SignalKind kind,
                              std::uint16_t type = 16, std::uint8_t direction = 0);
    Id add_alias(Id parent, std::string_view name, Id signal, std::uint16_t type = 16,
                 std::uint8_t direction = 0);
    Id add_enum_table(Id parent, std::string_view name, std::span<const std::pair<Id, Id>> entries);
    TypeId structure(std::string name, std::vector<Field> fields);
    void declare_types(View declarations);
    TypeId declare_datatype(std::string name, decltype(vtr::DataType::definition) definition,
                            vtr::detail::wire::Attributes attributes = {});
    const Types& types() const;

    template <class T> TypeId bind() {
        return bind_type([](Types& types) { return types.bind<T>(); });
    }

    GeneratorId add_generator(Id parent, std::string_view name, std::string_view kind,
                              TypeId payload_type = 0);
    void node_attribute(Id node, Id key, const Value& value);
    void generator_attribute(GeneratorId generator, Id key, const Value& value);
    void set_time(Timestamp time);
    void emit(Id signal, std::uint64_t value);
    SignalKind signal_kind(Id signal) const;
    void emit(std::span<const SignalUpdate> updates);
    void emit_packed(Id signal, std::uint8_t states, View value);
    void emit_real(Id signal, double value);
    void emit_varlen(Id signal, View value);
    void dump(bool active);
    void assign_record_id(RecordId id);
    RecordId begin(GeneratorId generator, Timestamp time, RecordId parent = 0);
    RecordId begin(GeneratorId generator, Timestamp time, View payload, RecordId parent = 0);
    void payload(RecordId owner, View payload);
    void attribute(RecordId owner, Id key, const Value& value);
    void attribute(RecordId owner, Id key, std::uint64_t value);
    void span_kind(RecordId owner, std::uint8_t kind);
    void end(RecordId owner, Timestamp time, std::uint8_t status = 0);
    void end(RecordId owner, Timestamp time, Bytes payload, std::uint8_t status = 0);
    void end_captured(RecordId owner, Timestamp time, View payload, std::uint8_t status = 0);

    // Keep typed serialization inline so fixed-size captures can be optimized.
    template <class T>
    void end_value(RecordId owner, Timestamp time, const T& value, std::uint8_t status = 0) {
        try {
            Payload payload(prepare_value_end(owner, status));
            payload.append(value);
            commit_value_end(owner, time, std::move(payload).take(), status);
        } catch (const vtr::detail::wire::Error& error) {
            capture_failure(error.code());
            throw;
        } catch (...) {
            capture_failure(ErrorCode::resource_limit);
            throw;
        }
    }

    RecordId event(GeneratorId generator, RecordId owner, Timestamp time, View payload = {});
    RecordId log(GeneratorId generator, std::uint32_t datatype, Timestamp time, View payload = {},
                 RecordId owner = 0);
    RecordId begin_stage(GeneratorId generator, RecordId owner, Timestamp time);
    RecordId begin_stage(GeneratorId generator, RecordId owner, Timestamp time, View payload);
    void stage_payload(RecordId stage, View payload);
    void end_stage(RecordId stage, Timestamp time);
    RecordId embedded_event(RecordId owner, Timestamp time, Id name,
                            std::span<const vtr::detail::wire::Attribute> attrs = {});
    RecordId embedded_stage(RecordId owner, Id name, Id lane, Timestamp begin,
                            std::optional<Timestamp> end = {},
                            std::span<const vtr::detail::wire::Attribute> attrs = {});
    void embedded_stage_end(RecordId owner, RecordId stage, Timestamp time);
    void relate(Id kind, RecordId source, RecordId destination,
                std::span<const vtr::detail::wire::Attribute> attrs = {});
    GeneratorId add_clock(Id parent, std::string_view name);
    void associate_clock(GeneratorId source, GeneratorId clock);
    RecordId clock_run(GeneratorId clock, Timestamp first, Timestamp period);
    void clock_stop(GeneratorId clock, Timestamp time);
    void flush();
    void close();
    void seal();
    WriterStats stats() const;
    HandoffStats handoff_stats() const;

  private:
    Bytes prepare_value_end(RecordId owner, std::uint8_t status);
    void commit_value_end(RecordId owner, Timestamp time, Bytes payload, std::uint8_t status);
    void capture_failure(ErrorCode code) noexcept;
    TypeId bind_type(const std::function<TypeId(Types&)>& bind);
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

struct MissingReference {
    enum class Kind {
        parent,
        relation_source,
        relation_destination,
        child_block,
        clock_generator
    } kind;
    RecordId referrer, target;
    std::uint64_t block_number;
};

struct ValidationReport {
    bool incomplete_file = false;
    std::uint64_t records = 0, relations = 0;
    std::vector<MissingReference> missing;
};

using TransactionRef = std::shared_ptr<const Transaction>;

class RecordRef {
  public:
    static RecordRef retain(const Transaction& tx, RecordKind kind) {
        return RecordRef(std::make_shared<const Transaction>(tx), kind);
    }

    static RecordRef retain(const LocatedRecord& record, RecordKind kind) {
        return RecordRef(std::make_shared<const LocatedRecord>(record), kind);
    }

    RecordId id() const;
    RecordId parent() const;

    RecordKind kind() const noexcept {
        return kind_;
    }

    std::optional<GeneratorId> generator() const;
    Timestamp begin() const;
    Timestamp end() const;
    View payload() const;
    std::shared_ptr<const void> retained_storage() const;
    std::span<const vtr::detail::wire::Attribute> attributes() const;
    TransactionRef transaction() const;
    const LocatedRecord* located() const;
    const EmbeddedEvent* event() const;
    const EmbeddedStage* stage() const;

  private:
    friend class Reader;

    struct Embedded {
        TransactionRef owner;
        std::size_t index;
    };

    RecordRef(TransactionRef tx, RecordKind kind) : data_(std::move(tx)), kind_(kind) {}

    RecordRef(std::shared_ptr<const LocatedRecord> record, RecordKind kind)
        : data_(std::move(record)), kind_(kind) {}

    RecordRef(Embedded record, RecordKind kind) : data_(std::move(record)), kind_(kind) {}

    std::variant<TransactionRef, std::shared_ptr<const LocatedRecord>, Embedded> data_;
    RecordKind kind_;
};

struct Expansion {
    TransactionRef owner;
    std::vector<LocatedRecord> located;
    std::vector<std::uint64_t> missing_blocks;
    bool complete = false;
};

struct CacheStats {
    std::size_t owner_entries = 0, accounted_bytes = 0;
    std::size_t located_pages = 0, located_page_bytes = 0;
};

struct ReaderOptions {
    bool verify_checksums = false;
    std::size_t owner_cache_entries = 4096, owner_cache_bytes = 32 << 20,
                max_payload_bytes = default_payload_limit, located_page_cache_bytes = 8 << 20;
};

class Reader {
  public:
    explicit Reader(const std::string& path, ReaderOptions options = {});
    ~Reader();
    Reader(const Reader&) = delete;
    Reader& operator=(const Reader&) = delete;
    const TraceId& trace_id() const noexcept;
    std::optional<TraceFingerprint> fingerprint() const noexcept;
    const Types& types() const;
    const vtr::detail::wire::Meta& metadata() const;
    std::span<const vtr::detail::wire::Node> nodes() const;
    std::span<const std::string> strings() const;
    std::string_view string(Id id) const;
    std::span<const SignalKind> signals() const;
    Id find_node(std::string_view path) const;
    Id find_generator(std::string_view path) const;
    Id find_signal(std::string_view path) const;
    const ContainerReader& container() const noexcept;
    bool incomplete() const;
    void changes(Timestamp begin, Timestamp end,
                 const std::function<bool(Timestamp, Id, const SignalValue&)>& visit) const;
    void changes(Id signal, Timestamp begin, Timestamp end,
                 const std::function<bool(Timestamp, Id, const SignalValue&)>& visit) const;
    Bytes value_at(Id signal, Timestamp time) const;
    std::shared_ptr<const vtr::detail::wire::SignalData> load_signal(Id signal) const;
    void transactions(TransactionQuery query,
                      const std::function<bool(const Transaction&)>& visit) const;
    void records(RecordQuery query, const std::function<bool(const LocatedRecord&)>& visit) const;
    void relations(RecordId endpoint, bool incoming,
                   const std::function<bool(const Relation&)>& visit) const;
    TransactionRef transaction(RecordId id) const;
    std::optional<RecordRef> lookup(RecordId id) const;
    Expansion expand(RecordId owner) const;
    std::vector<Transaction> children(RecordId parent) const;
    std::vector<LocatedRecord> logs(RecordId parent, Timestamp begin = 0,
                                    Timestamp end = UINT64_MAX) const;
    std::optional<std::uint64_t> cycle_at(GeneratorId clock, Timestamp time) const;
    std::optional<GeneratorId> clock_for(GeneratorId source) const;
    ValidationReport validate() const;
    CacheStats cache_stats() const;
    void clear_cache() const;

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
} // namespace vtr::detail::engine
