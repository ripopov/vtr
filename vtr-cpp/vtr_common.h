#pragma once

// Shared values, identities and schema bindings for the VTR library.
// Semantics: docs/SPEC.md and docs/TRANSACTIONS.md.

#include <algorithm>
#include <array>
#include <bit>
#include <compare>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <optional>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>

namespace vtr {

inline constexpr unsigned format_major = 3;
inline constexpr unsigned format_minor = 4;
using Timestamp = std::uint64_t;
using Bytes = std::vector<std::byte>;
using ByteView = std::span<const std::byte>;

namespace detail {
struct ScopeTag;
struct NodeTag;
struct SignalTag;
struct GeneratorTag;
struct StringTag;
struct TypeTag;
struct RecordTag;
struct BlockTag;
} // namespace detail

// IDs are file-local. Zero is a valid hierarchy/string/type index, but never a
// record or block ID. Absence is represented by optional, not an index sentinel.
template <class Tag, class Rep> struct Identifier {
    Rep value;

    explicit constexpr Identifier(Rep value) noexcept : value(value) {}

    friend constexpr auto operator<=>(Identifier, Identifier) = default;
};

using ScopeId = Identifier<detail::ScopeTag, std::uint32_t>;
using NodeId = Identifier<detail::NodeTag, std::uint32_t>;
using SignalId = Identifier<detail::SignalTag, std::uint32_t>;
using GeneratorId = Identifier<detail::GeneratorTag, std::uint32_t>;
using StringId = Identifier<detail::StringTag, std::uint32_t>;
using TypeId = Identifier<detail::TypeTag, std::uint32_t>;
using RecordId = Identifier<detail::RecordTag, std::uint64_t>;
using BlockId = Identifier<detail::BlockTag, std::uint64_t>;

struct RecordingId {
    std::array<std::byte, 16> bytes;
    friend bool operator==(const RecordingId&, const RecordingId&) = default;
};

struct Fingerprint {
    RecordingId recording;
    std::uint64_t file_bytes;
    std::uint32_t directory_crc;
    friend bool operator==(const Fingerprint&, const Fingerprint&) = default;
};

enum class ErrorCode {
    invalid_argument,
    invalid_state,
    wrong_recording,
    id_exhausted,
    unsupported_version,
    corrupt_data,
    resource_limit,
    io,
};

class Error : public std::runtime_error {
  public:
    Error(ErrorCode code, std::string message);
    [[nodiscard]] ErrorCode code() const noexcept;

  private:
    ErrorCode code_;
};

enum class Status : std::uint8_t { unset, ok, error, aborted, open };
enum class SpanKind : std::uint8_t { unspecified, internal, server, client, producer, consumer };
enum class RecordKind : std::uint8_t {
    transaction,
    stage,
    event,
    log,
    clock,
    embedded_event,
    embedded_stage,
};

struct Unit {};

struct TimeValue {
    Timestamp value;
};

struct Address {
    std::uint64_t value;
};

struct Fixed {
    std::int64_t raw;
    std::int64_t scale;
};

struct UFixed {
    std::uint64_t raw;
    std::int64_t scale;
};

struct EnumValue {
    std::int64_t value;
    StringId literal;
};
enum class States : std::uint8_t { binary = 2, four = 4, nine = 9 };

// Least-significant state first: 1, 2 or 4 bits per state. Unused high bits are
// zero; four-state codes are 0, 1, X, Z; nine-state codes follow SPEC.md.
template <States Alphabet> struct PackedVector {
    static constexpr States alphabet = Alphabet;
    std::uint64_t width;
    Bytes packed;
};

using Bits = PackedVector<States::binary>;
using Logic = PackedVector<States::four>;
using Logic9 = PackedVector<States::nine>;

struct Attribute;

struct Value {
    using List = std::vector<Value>;
    using Map = std::vector<Attribute>;
    using Storage =
        std::variant<std::monostate, bool, std::int8_t, std::int16_t, std::int32_t, std::int64_t,
                     std::uint8_t, std::uint16_t, std::uint32_t, std::uint64_t, float, double,
                     StringId, std::string, Bytes, Bits, Logic, Logic9, TimeValue, EnumValue,
                     Address, Fixed, UFixed, List, Map>;

    Storage data;
    Value() = default;

    template <class T>
        requires std::constructible_from<Storage, T>
    Value(T&& item) : data(std::forward<T>(item)) {}
};

struct Attribute {
    StringId key;
    Value data;
};

using Attributes = std::vector<Attribute>;
using AttributeView = std::span<const Attribute>;

// Bind structs with name + fields, enums with name + enumerators. Builtins are
// Unit, bool, fixed-width integers, float/double, string/string_view, Bytes,
// StringId, Bits, Logic, Logic9, TimeValue, Address, Fixed and UFixed.
// std::array and std::vector compose recursively. Struct fields use declaration
// order; padding is never captured. Enums preserve unknown numeric values.
template <class T> struct Schema;

template <class Owner, class Member> struct MemberField {
    std::string_view name;
    Member Owner::*member;
};

template <class Owner, class Member>
[[nodiscard]] constexpr auto field(std::string_view name, Member Owner::*member) noexcept {
    return MemberField<Owner, Member>{name, member};
}

template <class E>
    requires std::is_enum_v<E>
struct Enumerator {
    std::string_view name;
    E value;
};

enum class Primitive : std::uint8_t {
    unit,
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
};

struct FieldDefinition {
    std::string name;
    TypeId type;
};

struct EnumLiteral {
    std::string name;
    std::uint64_t value_bits;
};

struct StructDefinition {
    std::vector<FieldDefinition> fields;
};

struct ArrayDefinition {
    TypeId element;
    std::uint32_t count;
};

struct SequenceDefinition {
    TypeId element;
};

struct EnumDefinition {
    TypeId underlying;
    std::vector<EnumLiteral> literals;
};

struct DataType {
    TypeId id;
    std::string name;
    std::variant<Primitive, StructDefinition, ArrayDefinition, SequenceDefinition, EnumDefinition>
        definition;
    Attributes attributes = {};
};

struct Metadata {
    std::int8_t timescale = -12; // One tick = 10^timescale seconds.
    std::int64_t time_zero = 0;
    std::string writer;
    std::string date;
    std::string comment;
    Attributes extra;
};

enum class ScopeKind : std::uint16_t {
    module = 0,
    task = 1,
    function = 2,
    begin_ = 3,
    fork = 4,
    generate = 5,
    struct_ = 6,
    union_ = 7,
    class_ = 8,
    interface = 9,
    package = 10,
    program = 11,
    vhdl_architecture = 12,
    vhdl_procedure = 13,
    vhdl_function = 14,
    vhdl_record = 15,
    vhdl_process = 16,
    vhdl_block = 17,
    vhdl_for_generate = 18,
    vhdl_if_generate = 19,
    vhdl_generate = 20,
    vhdl_package = 21,
    sv_array = 22,
    generic = 64,
    sc_module = 65,
    resource = 66,
    instrumentation_scope = 67,
    core = 68
};

enum class VariableType : std::uint16_t {
    event = 0,
    integer = 1,
    parameter = 2,
    real = 3,
    real_parameter = 4,
    reg = 5,
    supply0 = 6,
    supply1 = 7,
    time = 8,
    tri = 9,
    tri_and = 10,
    tri_or = 11,
    tri_reg = 12,
    tri0 = 13,
    tri1 = 14,
    wand = 15,
    wire = 16,
    wor = 17,
    port = 18,
    sparse_array = 19,
    realtime = 20,
    string = 21,
    bit = 22,
    logic = 23,
    int_ = 24,
    shortint = 25,
    longint = 26,
    byte = 27,
    enum_ = 28,
    shortreal = 29,
    bits = 64,
    bytes = 65
};

enum class VariableDirection : std::uint8_t { implicit, input, output, inout, buffer, linkage };

struct VariableDeclaration {
    VariableType type = VariableType::wire;
    VariableDirection direction = VariableDirection::implicit;
};

struct EnumEntry {
    StringId name;
    StringId value;
};

enum class SignalEncoding { bits, real, bytes };

struct SignalKind {
    SignalEncoding encoding = SignalEncoding::bits;
    std::uint32_t width = 1;
    States alphabet = States::binary;
};

namespace detail {

template <class T> struct SequenceTraits : std::false_type {
    static constexpr bool borrowed = false;
};

template <class T, class Allocator>
struct SequenceTraits<std::vector<T, Allocator>> : std::true_type {
    using Element = T;
    static constexpr bool borrowed = false;
};

template <class T, std::size_t N> struct SequenceTraits<std::span<T, N>> : std::true_type {
    using Element = std::remove_cv_t<T>;
    static constexpr bool borrowed = true;
};

template <class T> struct ArrayTraits : std::false_type {};

template <class T, std::size_t N> struct ArrayTraits<std::array<T, N>> : std::true_type {
    using Element = T;
    static constexpr std::size_t size = N;
};

template <class T> constexpr std::uint32_t builtin_type() {
    if constexpr (std::same_as<T, Unit>) {
        return 0;
    } else if constexpr (std::same_as<T, bool>) {
        return 1;
    } else if constexpr (std::integral<T>) {
        static_assert(sizeof(T) <= 8);
        return (std::is_signed_v<T> ? 2 : 6) + std::countr_zero(sizeof(T));
    } else if constexpr (std::same_as<T, float>) {
        return 10;
    } else if constexpr (std::same_as<T, double>) {
        return 11;
    } else if constexpr (std::same_as<T, StringId>) {
        return 12;
    } else if constexpr (std::same_as<T, std::string> || std::same_as<T, std::string_view>) {
        return 13;
    } else if constexpr (std::same_as<T, Bytes>) {
        return 14;
    } else if constexpr (std::same_as<T, Bits>) {
        return 15;
    } else if constexpr (std::same_as<T, Logic>) {
        return 16;
    } else if constexpr (std::same_as<T, Logic9>) {
        return 17;
    } else if constexpr (std::same_as<T, TimeValue>) {
        return 18;
    } else if constexpr (std::same_as<T, Address>) {
        return 20;
    } else if constexpr (std::same_as<T, Fixed>) {
        return 21;
    } else if constexpr (std::same_as<T, UFixed>) {
        return 22;
    } else {
        return UINT32_MAX;
    }
}

class GrowablePayloadBuffer {
  public:
    static constexpr std::size_t inline_capacity = 256;

    std::byte* grow(std::size_t bytes) {
        if (bytes > UINT32_MAX - size_)
            throw Error(ErrorCode::resource_limit, "Payload exceeds u32");
        const auto previous = size_;
        size_ += bytes;
        if (!large_ && size_ <= inline_.size())
            return inline_.data() + previous;
        if (!large_) {
            overflow_.reserve(std::max(size_, inline_.size() * 2));
            overflow_.insert(overflow_.end(), inline_.begin(), inline_.begin() + previous);
            large_ = true;
        }
        overflow_.resize(size_);
        return overflow_.data() + previous;
    }

    [[nodiscard]] ByteView view() const noexcept {
        return {large_ ? overflow_.data() : inline_.data(), size_};
    }

    [[nodiscard]] Bytes take() && {
        if (large_)
            return std::move(overflow_);
        auto data = view();
        return Bytes(data.begin(), data.end());
    }

  private:
    std::array<std::byte, inline_capacity> inline_;
    Bytes overflow_;
    std::size_t size_ = 0;
    bool large_ = false;
};

class BoundedPayloadBuffer {
  public:
    explicit BoundedPayloadBuffer(std::span<std::byte> destination) : destination_(destination) {
        if (destination.size() > UINT32_MAX)
            throw Error(ErrorCode::resource_limit, "Payload exceeds u32");
    }

    std::byte* grow(std::size_t bytes) {
        if (bytes > destination_.size() - size_)
            size_changed();
        const auto previous = size_;
        size_ += bytes;
        return destination_.data() + previous;
    }

    [[nodiscard]] ByteView view() const noexcept {
        return destination_.first(size_);
    }

  private:
    // Exception construction must stay outside the per-field bounds check.
    [[noreturn]] static void size_changed();

    std::span<std::byte> destination_;
    std::size_t size_ = 0;
};

template <class Storage> class PayloadEncoding {
  public:
    PayloadEncoding() = default;

    explicit PayloadEncoding(std::span<std::byte> destination) : storage_(destination) {}

    template <std::integral T> void scalar(T value) {
        if constexpr (std::same_as<T, bool>) {
            *storage_.grow(1) = static_cast<std::byte>(value);
        } else {
            auto bits = static_cast<std::make_unsigned_t<T>>(value);
            auto* output = storage_.grow(sizeof(T));
            for (std::size_t i = 0; i < sizeof(T); ++i) {
                output[i] = static_cast<std::byte>(bits >> (8 * i));
            }
        }
    }

    void raw(ByteView bytes) {
        if (!bytes.empty())
            std::memcpy(storage_.grow(bytes.size()), bytes.data(), bytes.size());
    }

    void length(std::size_t size) {
        if (size > UINT32_MAX) {
            throw Error(ErrorCode::resource_limit, "Payload collection exceeds u32");
        }
        scalar(static_cast<std::uint32_t>(size));
    }

    template <class T> void append(const T& value) {
        using U = std::remove_cvref_t<T>;
        if constexpr (std::same_as<U, Unit>) {
        } else if constexpr (std::integral<U>) {
            scalar(value);
        } else if constexpr (std::same_as<U, float>) {
            scalar(std::bit_cast<std::uint32_t>(value));
        } else if constexpr (std::same_as<U, double>) {
            scalar(std::bit_cast<std::uint64_t>(value));
        } else if constexpr (std::same_as<U, std::string> || std::same_as<U, std::string_view>) {
            length(value.size());
            raw(std::as_bytes(std::span(value.data(), value.size())));
        } else if constexpr (std::same_as<U, Bytes>) {
            length(value.size());
            raw(value);
        } else if constexpr (std::same_as<U, StringId> || std::same_as<U, TimeValue> ||
                             std::same_as<U, Address>) {
            scalar(value.value);
        } else if constexpr (std::same_as<U, Fixed> || std::same_as<U, UFixed>) {
            scalar(value.raw);
            scalar(value.scale);
        } else if constexpr (std::same_as<U, Bits> || std::same_as<U, Logic> ||
                             std::same_as<U, Logic9>) {
            scalar(value.width);
            raw(value.packed);
        } else if constexpr (std::is_enum_v<U>) {
            scalar(static_cast<std::underlying_type_t<U>>(value));
        } else if constexpr (SequenceTraits<U>::value) {
            length(value.size());
            for (const auto& item : value) {
                append(static_cast<const typename U::value_type&>(item));
            }
        } else if constexpr (ArrayTraits<U>::value) {
            for (const auto& item : value) {
                append(item);
            }
        } else {
            static_assert(requires { Schema<U>::fields; }, "Declare a Schema<T> binding");
            std::apply([&](auto... field) { (append(value.*(field.member)), ...); },
                       Schema<U>::fields);
        }
    }

    [[nodiscard]] ByteView view() const noexcept {
        return storage_.view();
    }

    [[nodiscard]] Bytes take() && {
        return std::move(storage_).take();
    }

  private:
    Storage storage_;
};

using PayloadEncoder = PayloadEncoding<GrowablePayloadBuffer>;
using CaptureEncoder = PayloadEncoding<BoundedPayloadBuffer>;

inline std::size_t payload_add(std::size_t size, std::size_t bytes) {
    if (bytes > UINT32_MAX - size)
        throw Error(ErrorCode::resource_limit, "Payload exceeds u32");
    return size + bytes;
}

template <class T> constexpr std::optional<std::size_t> fixed_payload_size() {
    using U = std::remove_cvref_t<T>;
    if constexpr (std::same_as<U, Unit>)
        return 0;
    else if constexpr (std::integral<U> || std::floating_point<U>)
        return sizeof(U);
    else if constexpr (std::is_enum_v<U>)
        return sizeof(std::underlying_type_t<U>);
    else if constexpr (std::same_as<U, StringId> || std::same_as<U, TimeValue> ||
                       std::same_as<U, Address>)
        return sizeof(decltype(std::declval<U>().value));
    else if constexpr (std::same_as<U, Fixed> || std::same_as<U, UFixed>)
        return sizeof(decltype(std::declval<U>().raw)) + sizeof(decltype(std::declval<U>().scale));
    else if constexpr (std::same_as<U, std::string> || std::same_as<U, std::string_view> ||
                       std::same_as<U, Bytes> || std::same_as<U, Bits> || std::same_as<U, Logic> ||
                       std::same_as<U, Logic9> || SequenceTraits<U>::value)
        return {};
    else if constexpr (ArrayTraits<U>::value) {
        constexpr auto element = fixed_payload_size<typename ArrayTraits<U>::Element>();
        if constexpr (element.has_value())
            return *element * ArrayTraits<U>::size;
        return {};
    } else {
        std::size_t total = 0;
        bool all_fixed = true;
        std::apply(
            [&](auto... field) {
                (
                    [&] {
                        constexpr auto size =
                            fixed_payload_size<decltype(std::declval<U>().*(field.member))>();
                        if constexpr (size.has_value())
                            total += *size;
                        else
                            all_fixed = false;
                    }(),
                    ...);
            },
            Schema<U>::fields);
        return all_fixed ? std::optional<std::size_t>(total) : std::nullopt;
    }
}

template <class T> std::size_t payload_size(const T& value) {
    using U = std::remove_cvref_t<T>;
    constexpr auto fixed = fixed_payload_size<U>();
    if constexpr (fixed.has_value()) {
        return payload_add(0, *fixed);
    } else if constexpr (std::same_as<U, std::string> || std::same_as<U, std::string_view> ||
                         std::same_as<U, Bytes>) {
        return payload_add(4, value.size());
    } else if constexpr (std::same_as<U, Bits> || std::same_as<U, Logic> ||
                         std::same_as<U, Logic9>) {
        return payload_add(sizeof(value.width), value.packed.size());
    } else if constexpr (SequenceTraits<U>::value || ArrayTraits<U>::value) {
        std::size_t size = ArrayTraits<U>::value ? 0 : 4;
        if (value.size() > UINT32_MAX)
            throw Error(ErrorCode::resource_limit, "Payload collection exceeds u32");
        constexpr auto element = fixed_payload_size<typename U::value_type>();
        if constexpr (element.has_value()) {
            if constexpr (*element != 0) {
                if (value.size() > (UINT32_MAX - size) / *element)
                    throw Error(ErrorCode::resource_limit, "Payload exceeds u32");
            }
            return size + value.size() * *element;
        } else {
            for (const auto& item : value)
                size = payload_add(size, payload_size(item));
            return size;
        }
    } else {
        std::size_t size = 0;
        std::apply(
            [&](auto... field) {
                ((size = payload_add(size, payload_size(value.*(field.member)))), ...);
            },
            Schema<U>::fields);
        return size;
    }
}

// Encoding must finish before return: inputs can borrow mutable simulator memory.
struct EncodedPayload {
    std::size_t size;
    const void* source;
    void (*encode)(std::span<std::byte>, const void*) = nullptr;

    void copy_to(std::span<std::byte> destination) const {
        if (encode)
            encode(destination, source);
        else if (size)
            std::memcpy(destination.data(), source, size);
    }
};

class PayloadCapture {
    std::array<std::byte, GrowablePayloadBuffer::inline_capacity> small_;
    EncodedPayload payload_;

  public:
    template <class T> explicit PayloadCapture(const T& value) {
        const auto size = payload_size(value);
        if (size <= small_.size()) {
            CaptureEncoder encoder{std::span(small_).first(size)};
            encoder.append(value);
            if (encoder.view().size() != size)
                throw Error(ErrorCode::invalid_argument, "Payload changed size during capture");
            payload_ = {size, small_.data()};
        } else {
            payload_ = {size, &value, [](std::span<std::byte> output, const void* source) {
                            CaptureEncoder encoder(output);
                            encoder.append(*static_cast<const T*>(source));
                            if (encoder.view().size() != output.size())
                                throw Error(ErrorCode::invalid_argument,
                                            "Payload changed size during capture");
                        }};
        }
    }

    PayloadCapture(const PayloadCapture&) = delete;
    PayloadCapture& operator=(const PayloadCapture&) = delete;

    EncodedPayload payload() const noexcept {
        return payload_;
    }
};

template <class T> Bytes encode_payload(const T& value) {
    PayloadEncoder encoder;
    encoder.append(value);
    return std::move(encoder).take();
}

} // namespace detail

} // namespace vtr
