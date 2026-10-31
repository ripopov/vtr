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

using Primitive = vtr::Primitive;
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

class Types {
  public:
    Types();
    const Type& operator[](TypeId id) const;

    std::size_t size() const noexcept {
        return types_.size();
    }

    const vtr::detail::wire::Attributes& attributes(TypeId id) const;

    std::size_t attribute_capacity() const noexcept {
        return attributes_.capacity();
    }

    TypeId structure(std::string name, std::vector<Field> fields,
                     vtr::detail::wire::Attributes attributes = {});
    TypeId array(TypeId element, std::uint32_t count, std::string name = {},
                 vtr::detail::wire::Attributes attributes = {});
    TypeId sequence(TypeId element, std::string name = {},
                    vtr::detail::wire::Attributes attributes = {});
    TypeId enumeration(std::string name, TypeId underlying, std::vector<Enumerator> values,
                       vtr::detail::wire::Attributes attributes = {});
    Bytes declarations(TypeId first = first_declared_type) const;
    void read_declarations(View data, std::uint64_t string_count = UINT64_MAX);
    void validate(TypeId type, View payload, std::uint64_t string_count = UINT64_MAX) const;
    void visit_text(TypeId type, View payload,
                    const std::function<void(std::string_view)>& visit) const;

    TypeId declare_datatype(std::string name, decltype(vtr::DataType::definition) definition,
                            vtr::detail::wire::Attributes attributes = {});

    template <class T> TypeId bind() {
        return vtr::detail::bind_schema<T>(*this).value;
    }

  private:
    template <class T, class Registry> friend vtr::TypeId vtr::detail::bind_schema(Registry&);

    vtr::TypeId bind_builtin(vtr::TypeId id) const {
        return id;
    }

    vtr::TypeId declare_type(std::string name, decltype(vtr::DataType::definition) definition) {
        return vtr::TypeId(declare_datatype(std::move(name), std::move(definition)));
    }

    template <class F> vtr::TypeId cached_type(std::type_index key, F&& declare) {
        return bindings_.get(key, std::forward<F>(declare));
    }

    TypeId add(Type type, vtr::detail::wire::Attributes attributes = {});
    std::vector<Type> types_;
    // Cold metadata does not enlarge the descriptors used for payload decoding.
    std::vector<vtr::detail::wire::Attributes> attributes_;
    vtr::detail::SchemaBindings bindings_;
};

class PayloadBuffer {
    Bytes data_;
    std::size_t limit_;

  public:
    explicit PayloadBuffer(std::size_t reserve = 64, std::size_t limit = UINT32_MAX)
        : limit_(std::min<std::size_t>(limit, UINT32_MAX)) {
        data_.reserve(std::min(reserve, limit_));
    }

    explicit PayloadBuffer(Bytes storage) : data_(std::move(storage)), limit_(UINT32_MAX) {
        data_.clear();
        if (data_.capacity() < 64)
            data_.reserve(64);
    }

    std::byte* grow(std::size_t bytes) {
        if (bytes > limit_ - data_.size())
            throw vtr::Error(vtr::ErrorCode::resource_limit, "Typed payload byte limit exceeded");
        const auto offset = data_.size();
        data_.resize(offset + bytes);
        return reinterpret_cast<std::byte*>(data_.data() + offset);
    }

    std::size_t remaining() const noexcept {
        return limit_ - data_.size();
    }

    vtr::ByteView view() const noexcept {
        return std::as_bytes(std::span(data_));
    }

    Bytes take() && {
        return std::move(data_);
    }
};

class Payload : public vtr::detail::PayloadEncoding<PayloadBuffer> {
    using Encoding = vtr::detail::PayloadEncoding<PayloadBuffer>;

  public:
    explicit Payload(std::size_t reserve = 64, std::size_t limit = UINT32_MAX)
        : Encoding(PayloadBuffer(reserve, limit)) {}

    explicit Payload(Bytes storage) : Encoding(PayloadBuffer(std::move(storage))) {}

    std::size_t remaining() const noexcept {
        return Encoding::storage().remaining();
    }

    using Encoding::scalar;

    void scalar(float value) {
        scalar(std::bit_cast<std::uint32_t>(value));
    }

    void scalar(double value) {
        scalar(std::bit_cast<std::uint64_t>(value));
    }

    void raw(View data) {
        Encoding::raw(std::as_bytes(data));
    }

    View view() const noexcept {
        auto bytes = Encoding::view();
        return {reinterpret_cast<const std::uint8_t*>(bytes.data()), bytes.size()};
    }
};

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
    void skip(const Types& types, TypeId type);
    void done() const;

  private:
    Bytes decode(const Types& types, TypeId type, bool retain);
    std::array<View, 5> columns_;
    std::vector<View> dictionary_;
    std::span<const std::string> shared_text_;
    bool shared_ = false;
    std::uint64_t string_count_;
    std::size_t next_dictionary_ = 0, max_payload_bytes_;
};
} // namespace vtr::detail::engine
