#pragma once

#include "containers.h"
#include <algorithm>
#include <array>
#include <bit>
#include <concepts>
#include <cstring>
#include <limits>
#include <memory>
#include <string>
#include <tuple>
#include <typeindex>
#include <unordered_map>
#include <utility>
#include <wire.h>

namespace vtr::detail::engine {
using Bytes = vtr::detail::wire::Bytes;
using View = std::span<const std::uint8_t>;
using TypeId = std::uint32_t;
inline constexpr TypeId no_type = UINT32_MAX;
inline constexpr std::size_t default_payload_limit = 64 << 20;
inline constexpr TypeId first_declared_type = 23;

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
    ufixed
};
enum class TypeKind : std::uint8_t { scalar, structure, array, sequence, enumeration };

struct Field {
    std::string name;
    TypeId type;
};

struct Enumerator {
    std::uint64_t value;
    std::string name;
};

struct Type {
    TypeKind kind = TypeKind::scalar;
    Primitive primitive = Primitive::unit;
    std::string name;
    TypeId element = no_type;
    std::uint32_t count = 0;
    std::vector<Field> fields;
    std::vector<Enumerator> enumerators;
    std::uint64_t minimum_size = 0;
    std::optional<std::uint64_t> fixed_size;
    unsigned depth = 1;
    bool flat_scalars = false, validates_by_size = false, unsigned_words = false;
    bool contains_text = false;
    std::vector<Primitive> scalar_program;
};

// C++20 bindings declare member names once, without serializing object padding.
template <class T> struct Record;
template <class T> struct Enum;

template <class Owner, class Member> struct MemberField {
    using member_type = Member;
    std::string_view name;
    Member Owner::* member;
};

template <class Owner, class Member>
constexpr auto field(std::string_view name, Member Owner::* member) {
    return MemberField<Owner, Member>{name, member};
}
template <class T>
concept BoundRecord = requires {
    Record<T>::fields();
    Record<T>::name;
};
template <class T>
concept BoundEnum = std::is_enum_v<T> && requires {
    Enum<T>::values();
    Enum<T>::name;
};

template <class T> struct IsVector : std::false_type {};

template <class T, class A> struct IsVector<std::vector<T, A>> : std::true_type {
    using element = T;
};

template <class T> struct IsArray : std::false_type {};

template <class T, std::size_t N> struct IsArray<std::array<T, N>> : std::true_type {
    using element = T;
    static constexpr std::size_t size = N;
};

class Types {
  public:
    Types();
    const Type& operator[](TypeId id) const;

    std::size_t size() const noexcept {
        return types_.size();
    }

    TypeId structure(std::string name, std::vector<Field> fields);
    TypeId array(TypeId element, std::uint32_t count, std::string name = {});
    TypeId sequence(TypeId element, std::string name = {});
    TypeId enumeration(std::string name, TypeId underlying, std::vector<Enumerator> values);
    Bytes declarations(TypeId first = first_declared_type) const;
    void read_declarations(View data);
    void validate(TypeId type, View payload, std::uint64_t string_count = UINT64_MAX) const;
    void visit_text(TypeId type, View payload,
                    const std::function<void(std::string_view)>& visit) const;

    template <class T> TypeId bind() {
        using U = std::remove_cvref_t<T>;
        const auto key = std::type_index(typeid(U));
        if (auto i = bindings_.find(key); i != bindings_.end()) {
            if (i->second == no_type) {
                throw vtr::detail::wire::Error("recursive payload datatype");
            }
            return i->second;
        }
        bindings_.emplace(key, no_type);
        try {
            TypeId id;
            if constexpr (std::same_as<U, bool>) {
                id = 1;
            } else if constexpr (std::integral<U>) {
                static_assert(sizeof(U) <= 8);
                constexpr auto width = std::countr_zero(sizeof(U));
                id = (std::is_signed_v<U> ? 2 : 6) + width;
            } else if constexpr (std::same_as<U, float>) {
                id = 10;
            } else if constexpr (std::same_as<U, double>) {
                id = 11;
            } else if constexpr (std::same_as<U, std::string> ||
                                 std::same_as<U, std::string_view>) {
                id = 13;
            } else if constexpr (std::same_as<U, Bytes>) {
                id = 14;
            } else if constexpr (IsVector<U>::value) {
                id = sequence(bind<typename IsVector<U>::element>());
            } else if constexpr (IsArray<U>::value) {
                static_assert(IsArray<U>::size <= UINT32_MAX);
                id = array(bind<typename IsArray<U>::element>(), IsArray<U>::size);
            } else if constexpr (BoundRecord<U>) {
                std::vector<Field> fields;
                std::apply(
                    [&](auto... item) {
                        (fields.push_back({std::string(item.name),
                                           bind<std::remove_cvref_t<decltype(std::declval<U>().*
                                                                             (item.member))>>()}),
                         ...);
                    },
                    Record<U>::fields());
                id = structure(std::string(Record<U>::name), std::move(fields));
            } else if constexpr (BoundEnum<U>) {
                std::vector<Enumerator> values;
                for (auto [value, name] : Enum<U>::values()) {
                    values.push_back(
                        {static_cast<std::uint64_t>(static_cast<std::underlying_type_t<U>>(value)),
                         std::string(name)});
                }
                id = enumeration(std::string(Enum<U>::name), bind<std::underlying_type_t<U>>(),
                                 std::move(values));
            } else {
                static_assert(sizeof(U) == 0, "type needs a Record or Enum binding");
            }
            bindings_.at(key) = id;
            return id;
        } catch (...) {
            bindings_.erase(key);
            throw;
        }
    }

  private:
    TypeId add(Type type);
    std::vector<Type> types_;
    std::unordered_map<std::type_index, TypeId> bindings_;
};

template <class T> consteval std::size_t capture_size() {
    using U = std::remove_cvref_t<T>;
    if constexpr (std::integral<U> || std::floating_point<U> || std::is_enum_v<U>) {
        return sizeof(U);
    } else if constexpr (IsArray<U>::value) {
        constexpr auto size = capture_size<typename IsArray<U>::element>();
        return size == SIZE_MAX ? SIZE_MAX : size * IsArray<U>::size;
    } else if constexpr (BoundRecord<U>) {
        using Fields = decltype(Record<U>::fields());
        return []<std::size_t... I>(std::index_sequence<I...>) {
            constexpr bool fixed =
                ((capture_size<typename std::tuple_element_t<I, Fields>::member_type>() !=
                  SIZE_MAX) &&
                 ...);
            if constexpr (fixed) {
                return (std::size_t(0) + ... +
                        capture_size<typename std::tuple_element_t<I, Fields>::member_type>());
            } else {
                return SIZE_MAX;
            }
        }(std::make_index_sequence<std::tuple_size_v<Fields>>{});
    } else {
        return SIZE_MAX;
    }
}

template <class T> void capture_fixed(std::uint8_t*& out, const T& value) {
    using U = std::remove_cvref_t<T>;
    if constexpr (std::is_enum_v<U>) {
        capture_fixed(out, static_cast<std::underlying_type_t<U>>(value));
    } else if constexpr (std::same_as<U, bool>) {
        *out++ = value;
    } else if constexpr (std::integral<U> || std::floating_point<U>) {
        if constexpr (std::endian::native == std::endian::little) {
            std::memcpy(out, &value, sizeof(U));
        } else {
            std::array<std::uint8_t, sizeof(U)> data;
            std::memcpy(data.data(), &value, sizeof(U));
            std::reverse_copy(data.begin(), data.end(), out);
        }
        out += sizeof(U);
    } else if constexpr (IsArray<U>::value) {
        for (const auto& item : value) {
            capture_fixed(out, item);
        }
    } else {
        std::apply([&](auto... f) { (capture_fixed(out, value.*(f.member)), ...); },
                   Record<U>::fields());
    }
}

class Payload {
  public:
    explicit Payload(std::size_t reserve = 64, std::size_t limit = UINT32_MAX)
        : limit_(std::min<std::size_t>(limit, UINT32_MAX)) {
        data_.reserve(std::min(reserve, limit_));
    }

    explicit Payload(Bytes storage, std::size_t reserve = 64)
        : data_(std::move(storage)), limit_(UINT32_MAX) {
        data_.clear();
        if (data_.capacity() < reserve) {
            data_.reserve(reserve);
        }
    }

    std::size_t remaining() const noexcept {
        return limit_ - data_.size();
    }

    void raw(View data) {
        if (data.size() > remaining()) {
            throw vtr::detail::wire::Error("typed payload byte limit exceeded");
        }
        data_.insert(data_.end(), data.begin(), data.end());
    }

    void length(std::size_t size) {
        if (size > UINT32_MAX) {
            throw vtr::detail::wire::Error("payload collection exceeds u32");
        }
        scalar(std::uint32_t(size));
    }

    template <std::integral T> void scalar(T value) {
        if (sizeof(T) > remaining()) {
            throw vtr::detail::wire::Error("typed payload byte limit exceeded");
        }
        if constexpr (std::same_as<T, bool>) {
            data_.push_back(value);
        } else {
            using U = std::make_unsigned_t<T>;
            auto bits = static_cast<U>(value);
            if constexpr (sizeof(T) == 1) {
                data_.push_back(static_cast<std::uint8_t>(bits));
            } else {
                auto offset = data_.size();
                data_.resize(offset + sizeof(T));
                if constexpr (std::endian::native == std::endian::little) {
                    std::memcpy(data_.data() + offset, &bits, sizeof(T));
                } else {
                    for (unsigned i = 0; i < sizeof(T); ++i) {
                        data_[offset + i] = static_cast<std::uint8_t>(bits >> (8 * i));
                    }
                }
            }
        }
    }

    void scalar(float value) {
        scalar(std::bit_cast<std::uint32_t>(value));
    }

    void scalar(double value) {
        scalar(std::bit_cast<std::uint64_t>(value));
    }

    template <class T> void append(const T& value) {
        using U = std::remove_cvref_t<T>;
        if constexpr (std::integral<U> || std::floating_point<U>) {
            scalar(value);
        } else if constexpr (std::is_enum_v<U>) {
            scalar(static_cast<std::underlying_type_t<U>>(value));
        } else if constexpr (std::same_as<U, std::string> || std::same_as<U, std::string_view>) {
            length(value.size());
            raw({reinterpret_cast<const std::uint8_t*>(value.data()), value.size()});
        } else if constexpr (std::same_as<U, Bytes>) {
            length(value.size());
            raw(value);
        } else if constexpr (IsVector<U>::value) {
            length(value.size());
            if constexpr (std::same_as<typename U::value_type, bool>) {
                for (bool item : value) {
                    scalar(item);
                }
            } else {
                for (const auto& item : value) {
                    append(item);
                }
            }
        } else if constexpr (IsArray<U>::value) {
            for (const auto& item : value) {
                append(item);
            }
        } else if constexpr (BoundRecord<U>) {
            std::apply([&](auto... item) { (append(value.*(item.member)), ...); },
                       Record<U>::fields());
        } else {
            static_assert(sizeof(U) == 0, "unsupported payload value");
        }
    }

    template <class T> static void append_to(Bytes& out, const T& value) {
        if constexpr (capture_size<T>() != SIZE_MAX) {
            constexpr auto size = capture_size<T>();
            if (size > UINT32_MAX - out.size()) {
                throw vtr::detail::wire::Error("typed capture arena exceeds u32");
            }
            auto offset = out.size();
            out.resize(offset + size);
            auto* cursor = out.data() + offset;
            capture_fixed(cursor, value);
        } else {
            Payload p(0);
            p.data_.swap(out);
            auto previous = p.data_.size();
            try {
                p.append(value);
            } catch (...) {
                p.data_.resize(previous);
                p.data_.swap(out);
                throw;
            }
            p.data_.swap(out);
        }
    }

    View view() const noexcept {
        return data_;
    }

    Bytes take() && {
        return std::move(data_);
    }

  private:
    Bytes data_;
    std::size_t limit_;
};

template <class T> Bytes capture(const T& value) {
    Payload payload;
    payload.append(value);
    return std::move(payload).take();
}

class PayloadReader {
  public:
    explicit PayloadReader(View data, std::size_t allocation_budget = 64 << 20)
        : data_(data), budget_(allocation_budget) {}

    View take(std::size_t n) {
        if (n > data_.size()) {
            throw vtr::detail::wire::Error("truncated captured payload");
        }
        auto result = data_.first(n);
        data_ = data_.subspan(n);
        return result;
    }

    template <std::integral T> T scalar() {
        if constexpr (std::same_as<T, bool>) {
            auto value = take(1)[0];
            if (value > 1) {
                throw vtr::detail::wire::Error("invalid captured bool");
            }
            return value;
        } else {
            auto bytes = take(sizeof(T));
            std::make_unsigned_t<T> bits = 0;
            for (unsigned i = 0; i < sizeof(T); ++i) {
                bits |= std::make_unsigned_t<T>(bytes[i]) << (8 * i);
            }
            return std::bit_cast<T>(bits);
        }
    }

    template <class T> T read() {
        using U = std::remove_cvref_t<T>;
        if constexpr (std::integral<U>) {
            return scalar<U>();
        } else if constexpr (std::same_as<U, float>) {
            return std::bit_cast<float>(scalar<std::uint32_t>());
        } else if constexpr (std::same_as<U, double>) {
            return std::bit_cast<double>(scalar<std::uint64_t>());
        } else if constexpr (std::is_enum_v<U>) {
            return static_cast<U>(scalar<std::underlying_type_t<U>>());
        } else if constexpr (std::same_as<U, std::string>) {
            auto count = scalar<std::uint32_t>();
            spend(count);
            auto bytes = take(count);
            return {reinterpret_cast<const char*>(bytes.data()), bytes.size()};
        } else if constexpr (std::same_as<U, Bytes>) {
            auto count = scalar<std::uint32_t>();
            spend(count);
            auto bytes = take(count);
            return {bytes.begin(), bytes.end()};
        } else if constexpr (IsVector<U>::value) {
            using Element = typename U::value_type;
            auto count = scalar<std::uint32_t>();
            if (count > budget_ / sizeof(Element)) {
                throw vtr::detail::wire::Error("captured payload allocation budget exceeded");
            }
            spend(count * sizeof(Element));
            U result;
            result.reserve(count);
            for (std::uint32_t i = 0; i < count; ++i) {
                result.push_back(read<Element>());
            }
            return result;
        } else if constexpr (IsArray<U>::value) {
            U result;
            for (auto& item : result) {
                item = read<typename U::value_type>();
            }
            return result;
        } else if constexpr (BoundRecord<U>) {
            U result{};
            std::apply(
                [&](auto... item) {
                    ((result.*(item.member) =
                          read<std::remove_cvref_t<decltype(result.*(item.member))>>()),
                     ...);
                },
                Record<U>::fields());
            return result;
        } else {
            static_assert(sizeof(U) == 0, "unsupported captured payload type");
        }
    }

    void done() const {
        if (!data_.empty()) {
            throw vtr::detail::wire::Error("trailing captured payload bytes");
        }
    }

  private:
    void spend(std::size_t bytes) {
        if (bytes > budget_) {
            throw vtr::detail::wire::Error("captured payload allocation budget exceeded");
        }
        budget_ -= bytes;
    }

    View data_;
    std::size_t budget_;
};

template <class T> T decode(View payload, std::size_t allocation_budget = 64 << 20) {
    PayloadReader reader(payload, allocation_budget);
    auto value = reader.read<T>();
    reader.done();
    return value;
}

inline constexpr std::size_t shared_text_minimum = 256;
// Views borrow submitted payloads; retain their storage until encoding finishes.
using TextReferences = wire::detail::HashMap<std::string_view, wire::Id>;

// One set belongs to one independently compressed segment or transaction page.
struct ArgumentColumns {
    std::array<Bytes, 5> columns;
    struct TextChunk {
        std::shared_ptr<Bytes> bytes;
        std::size_t used = 0;
    };
    std::vector<TextChunk> text_chunks;
    wire::detail::HashMap<std::string_view, std::uint32_t> text_ids;
    std::vector<std::string_view> dictionary;
    std::uint64_t string_count = UINT64_MAX;
    const TextReferences* shared_text = nullptr;
    void append(const Types& types, TypeId type, View payload);
    void finish();
    std::uint32_t intern_text(std::string_view text);
};

class ArgumentReader {
  public:
    explicit ArgumentReader(std::array<View, 5> columns, std::uint64_t string_count = UINT64_MAX,
                            std::size_t max_payload_bytes = default_payload_limit,
                            std::span<const std::string> shared_text = {});
    Bytes read(const Types& types, TypeId type);
    void done() const;

  private:
    std::array<View, 5> columns_;
    std::vector<View> dictionary_;
    std::span<const std::string> shared_text_;
    bool shared_ = false;
    std::uint64_t string_count_;
    std::size_t next_dictionary_ = 0, max_payload_bytes_;
};
} // namespace vtr::detail::engine
