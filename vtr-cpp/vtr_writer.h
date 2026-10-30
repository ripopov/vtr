#pragma once

// Typed recording with explicit simulation-time lifetimes.
// Shared values and schema bindings are declared in vtr_common.h.
// Semantics: docs/SPEC.md and docs/TRANSACTIONS.md.

#include "vtr_common.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <memory>
#include <optional>
#include <span>
#include <string_view>
#include <typeindex>

// clang-format off
/*
    struct Instruction {
        std::uint64_t pc;
        std::uint64_t opcode;
    };

    template <> struct vtr::Schema<Instruction> {
        static constexpr std::string_view name = "Instruction";
        static constexpr auto fields = std::tuple{
            vtr::field("pc", &Instruction::pc),
            vtr::field("opcode", &Instruction::opcode),
        };
    };

    vtr::RecordId write_example() {
        namespace vt = vtr;
        vt::Writer trace("run.vtr");
        auto core = trace.scope("core0");
        auto instructions = trace.transactions<Instruction>(
            {.name = "instructions", .parent = core}, "INSTRUCTION");
        auto execute = trace.stages({.name = "execute", .parent = core});
        auto retired = trace.events({.name = "retired", .parent = core});

        auto selected = [&] {
            auto& out = trace;
            out.advance_time(10);
            auto tx = out.begin(instructions, Instruction{0x8000, 0x13});
            out.advance_time(11);
            auto visit = out.begin(execute, tx);
            out.advance_time(14);
            out.end(visit);
            out.event(retired, tx);
            out.advance_time(15);
            out.end(tx, vt::Status::ok);
            return tx.id();
        }();
        trace.close();

        return selected;
    }
*/
// clang-format on

namespace vtr {

namespace detail {
struct RecordingContext;
}

enum class SourceKind : std::uint8_t { transaction, stage, event, log };

class Writer;

class ProducerId {
  public:
    [[nodiscard]] std::uint32_t value() const noexcept {
        return value_;
    }

    friend bool operator==(ProducerId, ProducerId) = default;

  private:
    friend class Writer;

    ProducerId(std::uint32_t value, std::uint64_t origin) noexcept
        : value_(value), origin_(origin) {}

    std::uint32_t value_;
    std::uint64_t origin_;
};

// Sources and active handles belong to the writer that created them; cross-
// writer use is an error. Only ordinary transactions can supply an OwnerRef.
class OwnerRef {
  public:
    [[nodiscard]] RecordId id() const noexcept {
        return id_;
    }

  private:
    friend class Writer;
    template <class, SourceKind> friend class ActiveRecord;

    OwnerRef(RecordId id, std::uint64_t origin) noexcept : id_(id), origin_(origin) {}

    RecordId id_;
    std::uint64_t origin_;
};

template <class T, SourceKind Kind> class Source {
  public:
    using PayloadType = T;
    static constexpr SourceKind kind = Kind;
    [[nodiscard]] GeneratorId id() const noexcept;

  private:
    friend class Writer;
    Source(GeneratorId id, std::uint64_t origin) noexcept;
    GeneratorId id_;
    std::uint64_t origin_;
};

using Logger = Source<Unit, SourceKind::log>;

struct DatatypeOptions {
    std::string name;
    Attributes attributes = {};
};

// A writer-bound handle to a one-field datatype wrapping T.
template <class T> class PayloadType {
  public:
    [[nodiscard]] TypeId id() const noexcept {
        return id_;
    }

  private:
    friend class Writer;

    PayloadType(TypeId id, std::uint64_t origin) : id_(id), origin_(origin) {}

    TypeId id_;
    std::uint64_t origin_;
};

// Handles do not own a C++ lifetime and may cross producer timestamp boundaries.
// Destruction emits nothing: simulation timestamps require explicit end calls.
// Ending a handle twice is an error; its ID remains usable for lookup/relations.
template <class T, SourceKind Kind> class ActiveRecord {
    static_assert(Kind == SourceKind::transaction || Kind == SourceKind::stage);

  public:
    using PayloadType = T;
    [[nodiscard]] RecordId id() const noexcept;
    operator OwnerRef() const noexcept
        requires(Kind == SourceKind::transaction);

  private:
    friend class Writer;
    ActiveRecord(RecordId id, std::uint64_t origin) noexcept;
    RecordId id_;
    std::uint64_t origin_;
};

template <class T = Unit> using Transaction = ActiveRecord<T, SourceKind::transaction>;
template <class T = Unit> using Stage = ActiveRecord<T, SourceKind::stage>;

class ClockSource {
  public:
    [[nodiscard]] GeneratorId id() const noexcept;

  private:
    friend class Writer;
    ClockSource(GeneratorId id, std::uint64_t origin) noexcept;
    GeneratorId id_;
    std::uint64_t origin_;
};

struct SourceOptions {
    std::string_view name;
    std::optional<ScopeId> parent = {};
    Attributes metadata = {};
};

struct ScopeDeclaration {
    ScopeKind kind = ScopeKind::module;
    std::string_view component = {};
};

struct TransactionOptions {
    std::optional<OwnerRef> parent = {};
    SpanKind kind = SpanKind::unspecified;
};

enum class Codec { stored, lz4, zstd };

// Inline mode uses one caller and zero encoding workers. Threaded mode owns a backend.
enum class ExecutionMode { inline_, threaded };

// Each producer owns a fixed byte ring. Pending observations ahead of the
// serialization frontier spill to temporary storage beyond pending_bytes.
struct BufferOptions {
    std::size_t queue_bytes = 256 << 10;
    std::size_t pending_bytes = 16 << 20;
};

struct EncodingOptions {
    Codec compression = Codec::lz4;
    int compression_level = 1;
    std::size_t page_bytes = 64 << 10;
    std::size_t block_bytes = 512 << 10;
    std::uint32_t block_records = 65'536;
    std::size_t waveform_bytes = 16 << 20;
    std::uint64_t waveform_records = 1 << 24;
    std::uint32_t waveform_group_size = 256;
    bool waveform_deduplicate = true;
    std::size_t payload_pool_bytes = 16 << 20;
    bool separate_logs = true;
    bool separate_clocks = true;
    std::uint32_t workers = 0;
    std::uint32_t queue_jobs = 8;
    std::size_t queue_bytes = 256 << 20;
};

struct WriterOptions {
    ExecutionMode mode = ExecutionMode::threaded;
    BufferOptions buffer = {};
    EncodingOptions encoding = {};
    Metadata meta = {};
};

struct QueueStats {
    std::uint64_t submitted = 0;
    std::uint64_t completed = 0;
    std::uint64_t waits = 0;
    std::uint64_t wait_nanoseconds = 0;
    std::size_t peak_bytes = 0;
};

struct WriterStats {
    // Completion counts ingestion, including data waiting for the time frontier.
    // peak_bytes measures pending memory; input queues have separate storage.
    QueueStats transport;
    QueueStats encoding;
    std::size_t active_owners = 0;
    std::size_t active_stages = 0;
    std::size_t active_payload_bytes = 0;
    std::size_t peak_transaction_bytes = 0;
    std::size_t peak_record_bytes = 0;
    std::size_t peak_waveform_bytes = 0;
    std::size_t payload_pool_bytes = 0;
    std::uint64_t payload_reuses = 0;
    std::uint64_t transaction_blocks = 0;
    std::uint64_t record_blocks = 0;
    std::uint64_t spilled_bytes = 0;
    // Collector, encoders and output worker CPU since their entry points began.
    // Inline execution is zero; unavailable platform counters remain absent.
    std::optional<std::uint64_t> backend_cpu_nanoseconds = 0;
};

struct SignalUpdate {
    SignalId signal;
    std::uint64_t value;
};

// Borrowed input is copied before return, including nested text and sequences.
// IDs identify creations independently of recording time and storage order.
// Owners must be active when children are created; ownership imposes no temporal
// containment. Repeated events and overlapping stage visits remain distinct.
// In threaded mode producers may record concurrently; each attaches to one thread.
// Each signal/transaction has one tracing producer per recording timestamp.
// Advance every producer past a timestamp before the backend can complete it.
class Writer final {
  public:
    explicit Writer(const std::filesystem::path& path, WriterOptions options = {});
    ~Writer() noexcept; // Best-effort seal if close/seal was not called.
    Writer(const Writer&) = delete;
    Writer& operator=(const Writer&) = delete;
    Writer(Writer&&) = delete;
    Writer& operator=(Writer&&) = delete;

    template <class T>
    [[nodiscard]] Transaction<T> begin(Source<T, SourceKind::transaction> site, const T& payload,
                                       TransactionOptions options = {}) {
        return begin_at(site, time(), payload, options);
    }

    template <class T>
    [[nodiscard]] Transaction<T> begin(Source<T, SourceKind::transaction> site,
                                       TransactionOptions options = {}) {
        return begin_at(site, time(), options);
    }

    template <class T>
    [[nodiscard]] Stage<T> begin(Source<T, SourceKind::stage> site, OwnerRef owner,
                                 const T& payload) {
        return begin_at(site, owner, time(), payload);
    }

    template <class T>
    [[nodiscard]] Stage<T> begin(Source<T, SourceKind::stage> site, OwnerRef owner) {
        return begin_at(site, owner, time());
    }

    template <class T> void end(Transaction<T> record, Status status = Status::unset) {
        end_at(record, time(), status);
    }

    template <class T> void end(Transaction<T> record, const T& payload, Status status) {
        end_at(record, time(), payload, status);
    }

    template <class T> void end(Stage<T> record) {
        end_at(record, time());
    }

    template <class T> void end(Stage<T> record, const T& payload) {
        end_at(record, time(), payload);
    }

    template <class T>
    RecordId event(Source<T, SourceKind::event> site, OwnerRef owner, const T& payload) {
        return event_at(site, owner, time(), payload);
    }

    RecordId event(Source<Unit, SourceKind::event> site, OwnerRef owner) {
        return event_at(site, owner, time());
    }

    template <class T>
    RecordId log(Logger logger, PayloadType<T> datatype, const T& payload,
                 std::optional<OwnerRef> owner = {}) {
        return log_at(logger, datatype, time(), payload, owner);
    }

    RecordId log(Logger logger, PayloadType<Unit> datatype, std::optional<OwnerRef> owner = {}) {
        return log(logger, datatype, Unit{}, owner);
    }

    template <class T>
    [[nodiscard]] Transaction<T> begin_at(Source<T, SourceKind::transaction> site, Timestamp time,
                                          const T& payload, TransactionOptions options = {});

    // A non-unit payload may be deferred, but must be supplied before completion.
    template <class T>
    [[nodiscard]] Transaction<T> begin_at(Source<T, SourceKind::transaction> site, Timestamp time,
                                          TransactionOptions options = {});

    template <class T>
    [[nodiscard]] Stage<T> begin_at(Source<T, SourceKind::stage> site, OwnerRef owner,
                                    Timestamp time, const T& payload);
    template <class T>
    [[nodiscard]] Stage<T> begin_at(Source<T, SourceKind::stage> site, OwnerRef owner,
                                    Timestamp time);

    template <class T, SourceKind K> void payload(ActiveRecord<T, K> record, const T& payload);

    template <class T>
    void end_at(Transaction<T> record, Timestamp time, Status result = Status::unset);
    template <class T>
    void end_at(Transaction<T> record, Timestamp time, const T& payload, Status result);
    template <class T> void end_at(Stage<T> record, Timestamp time);
    template <class T> void end_at(Stage<T> record, Timestamp time, const T& payload);

    template <class T>
    RecordId event_at(Source<T, SourceKind::event> site, OwnerRef owner, Timestamp time,
                      const T& payload);
    RecordId event_at(Source<Unit, SourceKind::event> site, OwnerRef owner, Timestamp time);
    template <class T>
    RecordId log_at(Logger logger, PayloadType<T> datatype, Timestamp time, const T& payload,
                    std::optional<OwnerRef> owner = {});

    RecordId log_at(Logger logger, PayloadType<Unit> datatype, Timestamp time,
                    std::optional<OwnerRef> owner = {}) {
        return log_at(logger, datatype, time, Unit{}, owner);
    }

    RecordId log_raw(Logger logger, TypeId datatype, Timestamp time, ByteView payload,
                     std::optional<OwnerRef> owner = {});

    void attribute(OwnerRef owner, StringId key, const Value& data);

    template <class T>
        requires std::same_as<std::remove_cvref_t<T>, std::uint64_t>
    void attribute(OwnerRef owner, StringId key, T&& data) {
        attribute_u64(owner, key, data);
    }

    void classify(OwnerRef owner, SpanKind kind);

    // nullopt denotes an endpoint outside this recording, encoded as zero.
    void relate(StringId kind, std::optional<RecordId> from, std::optional<RecordId> to);

    // Period is positive. stop uses the last edge at or before max(first, time).
    RecordId run(ClockSource clock, Timestamp first_edge, Timestamp period);
    void stop(ClockSource clock, Timestamp time);

    // Advancing certifies that this producer has published all earlier recording times.
    void advance_time(Timestamp time);
    [[nodiscard]] Timestamp time();
    void emit(SignalId signal, std::uint64_t value);
    void emit(std::span<const SignalUpdate> updates);
    void emit_real(SignalId signal, double value);
    void emit_packed(SignalId signal, ByteView packed);
    // Input words are least-significant first. Width must match the declaration;
    // unused high bits are masked before the copied input is published.
    void emit_words(SignalId signal, std::span<const std::uint32_t> words, std::uint32_t width);
    void emit_bytes(SignalId signal, ByteView value);
    void dump(bool enabled);

    [[nodiscard]] RecordingId identity() const noexcept;
    [[nodiscard]] ProducerId add_producer(Timestamp initial_time = 0);
    void use_producer(ProducerId producer);
    void release_producer();
    void finish_producer();
    void publish();

    // Declarations are immutable and become visible before a referencing call returns.
    [[nodiscard]] StringId intern(std::string_view text);
    // Set metadata after interning its keys, before binding types or creating sources.
    void metadata(vtr::Metadata data);
    [[nodiscard]] ScopeId scope(std::string_view name, std::optional<ScopeId> parent = {},
                                AttributeView metadata = {}, ScopeDeclaration declaration = {});
    [[nodiscard]] NodeId enum_table(SourceOptions options, std::span<const EnumEntry> entries);
    template <class T> [[nodiscard]] TypeId bind();
    template <class T = Unit>
    [[nodiscard]] Source<T, SourceKind::transaction>
    transactions(SourceOptions options, std::string_view kind = "TRANSACTION");
    template <class T = Unit>
    [[nodiscard]] Source<T, SourceKind::stage> stages(SourceOptions options);
    template <class T = Unit>
    [[nodiscard]] Source<T, SourceKind::event> events(SourceOptions options);
    [[nodiscard]] Logger logger(SourceOptions options);

    // Descriptor addresses must identify immutable objects for this writer's lifetime.
    template <class T = Unit>
    [[nodiscard]] PayloadType<T> datatype(DatatypeOptions options,
                                          const void* descriptor = nullptr) {
        return {declare_payload_type(options, bind<T>(), descriptor), recording_origin()};
    }

    [[nodiscard]] TypeId declare_datatype(DatatypeOptions options,
                                          decltype(DataType::definition) definition);

    [[nodiscard]] ClockSource clock(SourceOptions options);
    // Set before recording or flush publishes the source.
    template <class T, SourceKind K> void associate_clock(Source<T, K> site, ClockSource clock);
    [[nodiscard]] SignalId signal(SourceOptions options, SignalKind kind = {},
                                  VariableDeclaration declaration = {});
    [[nodiscard]] NodeId alias(SignalId signal, SourceOptions options,
                               VariableDeclaration declaration = {});

    // Embedded detail has global IDs but no location generator. An absent stage
    // end stays absent on disk and resolves to max(stage.begin, owner.end).
    RecordId embedded_event(OwnerRef owner, Timestamp time, StringId name,
                            AttributeView attributes = {});
    // An absent lane selects the default stage lane, stored as the empty string.
    RecordId embedded_stage(OwnerRef owner, Timestamp begin, StringId name,
                            std::optional<StringId> lane = {},
                            std::optional<Timestamp> end = {}, AttributeView attributes = {});
    void end_embedded_stage(OwnerRef owner, RecordId stage, Timestamp time);
    void relate(StringId kind, std::optional<RecordId> from, std::optional<RecordId> to,
                AttributeView attributes);

    // flush completes inline output. In threaded mode it drains observations
    // below the shared frontier. It does not close the current timestamp or fsync.
    // close drains/joins workers and writes a complete directory and trailer.
    // seal is valid after failure and preserves explicitly incomplete output.
    void flush();
    void close();
    void seal();
    [[nodiscard]] WriterStats stats();
    // Samples owned workers without waiting for ingestion or final drain.
    [[nodiscard]] std::optional<std::uint64_t> backend_cpu_nanoseconds() const;

  private:
    void check_datatype(std::uint64_t origin) const;
    TypeId declare_payload_type(DatatypeOptions options, TypeId arguments, const void* descriptor);
    void attribute_u64(OwnerRef owner, StringId key, std::uint64_t data);
    RecordId begin_transaction(GeneratorId site, std::uint64_t origin, Timestamp time,
                               std::optional<detail::EncodedPayload> payload,
                               TransactionOptions options);
    RecordId begin_stage(GeneratorId site, std::uint64_t origin, OwnerRef owner, Timestamp time,
                         std::optional<detail::EncodedPayload> payload);
    void set_payload(RecordId record, std::uint64_t origin, SourceKind kind,
                     detail::EncodedPayload payload);
    void finish(RecordId record, std::uint64_t origin, SourceKind kind, Timestamp time,
                Status result, std::optional<detail::EncodedPayload> payload);
    RecordId instant(GeneratorId site, std::uint64_t origin, SourceKind kind, Timestamp time,
                     detail::EncodedPayload payload, std::optional<OwnerRef> owner,
                     std::optional<TypeId> datatype = {});

    std::uint64_t recording_origin() const noexcept {
        return origin_;
    }

    detail::RecordingContext* context_ = nullptr;
    template <class T, class Registry> friend TypeId detail::bind_schema(Registry&);
    TypeId bind_builtin(TypeId id);
    TypeId cached_type(std::type_index type, const std::function<TypeId()>& declare);
    TypeId declare_type(std::string name, decltype(DataType::definition) definition);
    GeneratorId source(SourceOptions options, std::string_view kind, TypeId payload);
    void associate_clock_impl(GeneratorId site, std::uint64_t origin, ClockSource clock);
    struct Impl;
    std::unique_ptr<Impl> impl_;
    std::uint64_t origin_ = 0;
};

template <class T, SourceKind K>
Source<T, K>::Source(GeneratorId id, std::uint64_t origin) noexcept : id_(id), origin_(origin) {}

template <class T, SourceKind K> GeneratorId Source<T, K>::id() const noexcept {
    return id_;
}

template <class T, SourceKind K>
ActiveRecord<T, K>::ActiveRecord(RecordId id, std::uint64_t origin) noexcept
    : id_(id), origin_(origin) {}

template <class T, SourceKind K> RecordId ActiveRecord<T, K>::id() const noexcept {
    return id_;
}

template <class T, SourceKind K>
ActiveRecord<T, K>::operator OwnerRef() const noexcept
    requires(K == SourceKind::transaction)
{
    return OwnerRef(id_, origin_);
}

template <class T> TypeId Writer::bind() {
    return detail::bind_schema<T>(*this);
}

template <class T>
Source<T, SourceKind::transaction> Writer::transactions(SourceOptions options,
                                                        std::string_view kind) {
    if (kind == "STAGE" || kind == "EVENT" || kind == "LOG" || kind == "CLOCK") {
        throw Error(ErrorCode::invalid_argument,
                    "Reserved kind requires its dedicated declaration");
    }
    return {source(std::move(options), kind, bind<T>()), recording_origin()};
}

template <class T> Source<T, SourceKind::stage> Writer::stages(SourceOptions options) {
    return {source(std::move(options), "STAGE", bind<T>()), recording_origin()};
}

template <class T> Source<T, SourceKind::event> Writer::events(SourceOptions options) {
    return {source(std::move(options), "EVENT", bind<T>()), recording_origin()};
}

template <class T, SourceKind K>
void Writer::associate_clock(Source<T, K> site, ClockSource clock) {
    associate_clock_impl(site.id_, site.origin_, clock);
}

template <class T>
Transaction<T> Writer::begin_at(Source<T, SourceKind::transaction> site, Timestamp time,
                                const T& value, TransactionOptions options) {
    detail::PayloadCapture capture(value);
    return {begin_transaction(site.id_, site.origin_, time, capture.payload(), options),
            recording_origin()};
}

template <class T>
Transaction<T> Writer::begin_at(Source<T, SourceKind::transaction> site, Timestamp time,
                                TransactionOptions options) {
    return {begin_transaction(site.id_, site.origin_, time, {}, options), recording_origin()};
}

template <class T>
Stage<T> Writer::begin_at(Source<T, SourceKind::stage> site, OwnerRef owner, Timestamp time,
                          const T& value) {
    detail::PayloadCapture capture(value);
    return {begin_stage(site.id_, site.origin_, owner, time, capture.payload()),
            recording_origin()};
}

template <class T>
Stage<T> Writer::begin_at(Source<T, SourceKind::stage> site, OwnerRef owner, Timestamp time) {
    return {begin_stage(site.id_, site.origin_, owner, time, {}), recording_origin()};
}

template <class T, SourceKind K> void Writer::payload(ActiveRecord<T, K> record, const T& value) {
    detail::PayloadCapture capture(value);
    set_payload(record.id_, record.origin_, K, capture.payload());
}

template <class T> void Writer::end_at(Transaction<T> record, Timestamp time, Status result) {
    finish(record.id_, record.origin_, SourceKind::transaction, time, result, {});
}

template <class T>
void Writer::end_at(Transaction<T> record, Timestamp time, const T& value, Status result) {
    detail::PayloadCapture capture(value);
    finish(record.id_, record.origin_, SourceKind::transaction, time, result, capture.payload());
}

template <class T> void Writer::end_at(Stage<T> record, Timestamp time) {
    finish(record.id_, record.origin_, SourceKind::stage, time, Status::unset, {});
}

template <class T> void Writer::end_at(Stage<T> record, Timestamp time, const T& value) {
    detail::PayloadCapture capture(value);
    finish(record.id_, record.origin_, SourceKind::stage, time, Status::unset, capture.payload());
}

template <class T>
RecordId Writer::event_at(Source<T, SourceKind::event> site, OwnerRef owner, Timestamp time,
                          const T& value) {
    detail::PayloadCapture capture(value);
    return instant(site.id_, site.origin_, SourceKind::event, time, capture.payload(), owner);
}

template <class T>
RecordId Writer::log_at(Logger logger, PayloadType<T> datatype, Timestamp time, const T& value,
                        std::optional<OwnerRef> owner) {
    check_datatype(datatype.origin_);
    detail::PayloadCapture capture(value);
    return instant(logger.id_, logger.origin_, SourceKind::log, time, capture.payload(), owner,
                   datatype.id_);
}

} // namespace vtr
