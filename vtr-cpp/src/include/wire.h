#pragma once

#include "vtr_common.h"

#include <array>
#include <bit>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <functional>
#include <limits>
#include <memory>
#include <optional>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <vector>

namespace vtr::detail::wire {
using Id = std::uint32_t;
using Time = std::uint64_t;
using TraceId = std::array<std::uint8_t, 16>;

struct TraceFingerprint {
    TraceId recording;
    std::uint64_t bytes;
    std::uint32_t directory_crc;
    friend bool operator==(const TraceFingerprint&, const TraceFingerprint&) = default;
};

using Bytes = std::vector<std::uint8_t>;
inline constexpr Id none = UINT32_MAX;
inline constexpr Time forever = UINT64_MAX;

using Error = vtr::Error;
using ErrorCode = vtr::ErrorCode;

enum class Codec : std::uint8_t { stored, lz4, zstd };
enum class Tag : std::uint8_t {
    null,
    boolean,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    u64,
    f32,
    f64,
    str,
    text,
    bytes,
    bits,
    logic,
    logic9,
    time,
    enumeration,
    pointer,
    fixed,
    ufixed,
    list,
    map
};

// Composite values keep their canonical payload, including nested tags, without
// a leading tag.
struct Value {
    Tag tag = Tag::null;
    std::uint64_t number = 0;
    Id literal = 0;
    std::int64_t scale = 0;
    Bytes payload;
    Value() = default;

    explicit Value(std::uint64_t v) : tag(Tag::u64), number(v) {}

    explicit Value(std::int64_t v) : tag(Tag::i64), number(static_cast<std::uint64_t>(v)) {}

    explicit Value(bool v) : tag(Tag::boolean), number(v) {}

    explicit Value(float v);
    explicit Value(double v);
    static Value string(Id id);
    static Value text(std::string_view text);
    static Value time(Time t);
    static Value bytes(std::span<const std::uint8_t> data);
    static Value list(std::span<const Value> values);
};

struct Attribute {
    Id key;
    Value value;
};

using Attributes = std::vector<Attribute>;

struct SignalKind {
    enum Type : std::uint8_t { bits, real, varlen } type = bits;

    std::uint32_t width = 1;
    std::uint8_t states = 2;
    std::size_t packed_size() const;
    friend bool operator==(const SignalKind&, const SignalKind&) = default;
};

struct SignalValue {
    SignalKind kind;
    std::span<const std::uint8_t> data;
    std::uint64_t as_u64() const;
    double as_real() const;
    std::string ascii() const;
};

inline std::uint64_t SignalValue::as_u64() const {
    if (kind.type != SignalKind::bits || kind.width > 64 || data.size() != kind.packed_size()) {
        throw Error(ErrorCode::invalid_argument, "signal is not a narrow vector");
    }
    if (kind.states == 2) {
        std::uint64_t value = 0;
        if constexpr (std::endian::native == std::endian::little) {
            switch (data.size()) {
            case 1:
                return data[0];
            case 2: {
                std::uint16_t v;
                std::memcpy(&v, data.data(), 2);
                return v;
            }
            case 4: {
                std::uint32_t v;
                std::memcpy(&v, data.data(), 4);
                return v;
            }
            case 8:
                std::memcpy(&value, data.data(), 8);
                return value;
            default:
                std::memcpy(&value, data.data(), data.size());
                return value;
            }
        }
        for (std::size_t i = 0; i < data.size(); ++i) {
            value |= std::uint64_t(data[i]) << (8 * i);
        }
        return value;
    }
    std::uint64_t value = 0;
    auto stride = kind.states == 4 ? 2u : 4u;
    for (std::size_t i = 0; i < kind.width; ++i) {
        auto code = (data[i * stride / 8] >> ((i * stride) % 8)) & ((1u << stride) - 1);
        if (code > 1) {
            throw Error(ErrorCode::invalid_argument, "signal contains nonbinary logic");
        }
        value |= std::uint64_t(code) << i;
    }
    return value;
}

struct SignalData {
    SignalKind kind;
    Bytes initial;
    std::vector<Time> times;
    std::vector<std::size_t> offsets;
    Bytes values;
    SignalValue at(std::size_t i) const;
    SignalValue value_at(Time time) const;
};

struct Node {
    enum Kind : std::uint8_t { scope = 1, variable, generator, enum_table } kind = scope;

    Id parent = none, name = 0, signal = none, aux = 0;
    Id payload_type = 0;
    std::uint16_t type = 0;
    std::uint8_t direction = 0;
    bool alias = false;
    Attributes attrs;
    std::vector<std::pair<Id, Id>> entries;
};

struct Meta {
    std::int8_t timescale = -12;
    std::int64_t time_zero = 0;
    std::uint8_t file_type = 0;
    std::string writer, date, comment;
    std::uint32_t group_size = 256;
    Attributes attrs;
};

struct HandoffStats {
    std::uint64_t accepted = 0, written = 0, wait_count = 0, wait_nanoseconds = 0,
                  oversized_jobs = 0;
    std::size_t jobs = 0, bytes = 0, encoding = 0, ready = 0, waiting_submitters = 0;
    std::size_t peak_jobs = 0, peak_bytes = 0, peak_encoding = 0, peak_ready = 0;
    std::optional<std::uint64_t> worker_cpu_nanoseconds = 0;
};

struct SignalUpdate {
    Id signal;
    std::uint64_t value;
};

struct WriterOptions {
    Codec codec = Codec::lz4;
    int compression_level = 3;
    std::uint32_t group_size = 256;
    std::uint64_t block_records = 1 << 24;
    std::size_t wave_bytes = 16 << 20;
    bool dedup = true, checksums = true;
    std::uint32_t workers = 0, queue_jobs = 8;
    std::size_t queue_bytes = 256 << 20;
};

struct ContainerSection {
    std::uint32_t kind = 0, flags = 0;
    std::uint64_t offset = 0, length = 0, aux0 = 0, aux1 = 0, block_number = 0;
};

// Writers have one producer thread. Borrowed arguments are copied before a call
// returns.
class Writer {
  public:
    const TraceId& trace_id() const noexcept;
    explicit Writer(const std::string& path, WriterOptions options = {});
    ~Writer();
    Writer(const Writer&) = delete;
    Writer& operator=(const Writer&) = delete;
    Meta& metadata();
    Id intern(std::string_view value);
    std::optional<Id> intern_with_budget(std::string_view value, std::size_t& remaining);
    std::size_t string_count() const noexcept;
    std::string_view string(Id id) const;
    Id add_scope(Id parent, std::string_view name, std::uint16_t type = 0,
                 std::string_view component = {});
    std::pair<Id, Id> add_var(Id parent, std::string_view name, SignalKind kind,
                              std::uint16_t type = 16, std::uint8_t direction = 0);
    Id add_alias(Id parent, std::string_view name, Id signal, std::uint16_t type = 16,
                 std::uint8_t direction = 0);
    Id add_generator(Id parent, std::string_view name, std::string_view kind, Id payload_type = 0);
    Id add_enum_table(Id parent, std::string_view name, std::span<const std::pair<Id, Id>> entries);
    void node_attr(Id node, Id key, const Value& value);
    void set_time(Time time);
    Time time() const noexcept;
    void dump(bool active);
    void emit(Id signal, std::uint64_t value);
    SignalKind signal_kind(Id signal) const;
    void emit(std::span<const SignalUpdate> updates);
    void emit_bit(Id signal, std::uint8_t code);
    void emit_real(Id signal, double value);
    void emit_packed(Id signal, std::uint8_t states, std::span<const std::uint8_t> value);
    void emit_words(Id signal, std::span<const std::uint32_t> words);
    void emit_logic(Id signal, std::string_view value);
    void emit_varlen(Id signal, std::span<const std::uint8_t> value);
    void append_section(std::uint32_t kind, std::span<const std::uint8_t> payload, Time begin = 0,
                        Time end = 0, std::uint64_t block_number = 0);
    void flush();
    void enqueue_section(std::uint32_t kind, Time begin, Time end, std::uint64_t block_number,
                         std::size_t memory_charge, std::function<Bytes()> encode);
    HandoffStats handoff_stats() const;
    std::pair<std::size_t, std::size_t> wave_buffer_bytes() const noexcept;
    bool failed() const noexcept;
    void check_failure() const;
    void abandon() noexcept;
    void mark_incomplete();
    void close_complete();
    void close_incomplete();
    void close();

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

struct ReaderStats {
    std::uint64_t recovered_dropped = 0;
    std::uint32_t signal_blocks = 0;
    bool recovered = false;
    std::optional<std::pair<Time, Time>> time_range;
};

struct ReaderOptions {
    bool verify_crc = false;
};

// Callback views live until the callback returns; loaded histories own shared
// immutable storage.
class Reader {
  public:
    const TraceId& trace_id() const noexcept;
    std::optional<TraceFingerprint> fingerprint() const noexcept;
    explicit Reader(const std::string& path, ReaderOptions options = {});
    ~Reader();
    Reader(const Reader&) = delete;
    Reader& operator=(const Reader&) = delete;
    const Meta& metadata() const;
    const ReaderStats& stats() const;
    std::span<const ContainerSection> sections() const;
    std::span<const std::uint8_t> section_data(const ContainerSection& section,
                                               bool verify = false) const;
    std::span<const std::string> strings() const;
    std::string_view string(Id id) const;
    std::span<const Node> nodes() const;
    std::span<const SignalKind> signals() const;
    Id signal_var(Id signal) const;
    Id find_node(std::string_view path, char separator = '.',
                 std::optional<Node::Kind> kind = {}) const;
    Id find_signal(std::string_view path, char separator = '.') const;
    bool in_scope(Id node, Id scope) const;
    void changes(Id signal, Time begin, Time end,
                 const std::function<bool(Time, Id, const SignalValue&)>& visit) const;
    void changes(Time begin, Time end,
                 const std::function<bool(Time, Id, const SignalValue&)>& visit) const;
    void validate_waveforms() const;
    Bytes value_at(Id signal, Time time) const;
    std::shared_ptr<const SignalData> load_signal(Id signal) const;
    const std::vector<Time>& time_table() const;
    std::span<const std::pair<Time, bool>> blackouts() const;
    void clear_cache() const;

  private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

void recover(const std::string& input, const std::string& output);
} // namespace vtr::detail::wire
