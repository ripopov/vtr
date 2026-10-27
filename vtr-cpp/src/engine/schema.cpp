#include "engine/schema.h"
#include "format.h"
#include <unordered_set>

namespace vtr::detail::engine {
namespace {
namespace d = vtr::detail::wire::detail;
using d::Cursor;
using d::require;
constexpr std::array<unsigned, 23> widths{0, 1, 1, 2, 4, 8, 1, 2, 4, 8,  4, 8,
                                          4, 0, 0, 0, 0, 0, 8, 0, 8, 16, 16};

bool signed_integer(Primitive p) {
    return p >= Primitive::i8 && p <= Primitive::i64;
}

bool unsigned_integer(Primitive p) {
    return p >= Primitive::u8 && p <= Primitive::u64;
}

void utf8(std::string_view s) {
    d::validate_utf8(d::view(s));
}

std::uint64_t var(Cursor& r) {
    auto before = r.data.size();
    auto value = r.var();
    auto n = before - r.data.size();
    require(n == 1 || (value >> (7 * (n - 1))) != 0, "noncanonical varint");
    return value;
}

std::uint32_t id(Cursor& r) {
    auto value = var(r);
    require(value <= UINT32_MAX, "datatype identifier overflow");
    return static_cast<std::uint32_t>(value);
}

std::string text(Cursor& r) {
    auto v = r.take(var(r));
    d::validate_utf8(v);
    return {reinterpret_cast<const char*>(v.data()), v.size()};
}

std::uint64_t row_number(Cursor& row, unsigned width) {
    auto data = row.take(width);
    switch (width) {
    case 1:
        return data[0];
    case 2:
        return d::load<std::uint16_t>(data);
    case 4:
        return d::load<std::uint32_t>(data);
    case 8:
        return d::load<std::uint64_t>(data);
    default:
        throw vtr::detail::wire::Error("invalid scalar width");
    }
}

std::uint64_t sign_extend(std::uint64_t bits, unsigned width) {
    if (width == 8) {
        return bits;
    }
    auto sign = std::uint64_t(1) << (width * 8 - 1);
    return (bits ^ sign) - sign;
}

void integer_range(Primitive p, std::uint64_t value) {
    auto width = widths[unsigned(p)];
    if (width == 8) {
        return;
    }
    if (signed_integer(p)) {
        require(sign_extend(value & ((std::uint64_t(1) << (8 * width)) - 1), width) == value,
                "signed argument out of range");
    } else {
        require(value < (std::uint64_t(1) << (8 * width)), "unsigned argument out of range");
    }
}

std::size_t packed_size(std::uint64_t width, Primitive p) {
    auto per_byte = p == Primitive::bits ? 8u : p == Primitive::logic ? 4u : 2u;
    auto size = width / per_byte + (width % per_byte != 0);
    require(size <= UINT32_MAX, "packed argument exceeds u32");
    return static_cast<std::size_t>(size);
}

void packed_valid(View data, std::uint64_t width, Primitive p) {
    auto stride = p == Primitive::bits ? 1u : p == Primitive::logic ? 2u : 4u;
    auto remainder = unsigned(width % (8 / stride)) * stride;
    require(data.empty() || !remainder || (data.back() >> remainder) == 0,
            "nonzero packed padding");
    if (p == Primitive::logic9) {
        for (std::uint64_t i = 0; i < width; ++i) {
            require(((data[i / 2] >> ((i % 2) * 4)) & 15) <= 8, "invalid logic9 state");
        }
    }
}

Primitive numeric_type(const Types& types, const Type& type) {
    return type.kind == TypeKind::enumeration ? types[type.element].primitive : type.primitive;
}

void compile_scalars(const Types& types, Type& type) {
    constexpr std::size_t limit = 256;
    if (type.kind == TypeKind::scalar || type.kind == TypeKind::enumeration) {
        type.flat_scalars = true;
        auto primitive = numeric_type(types, type);
        if (primitive != Primitive::unit) {
            type.scalar_program.push_back(primitive);
        }
    } else if (type.kind != TypeKind::sequence) {
        type.flat_scalars = true;
        auto append = [&](TypeId id, std::uint64_t count) {
            const auto& child = types[id];
            if (!child.flat_scalars ||
                (!child.scalar_program.empty() &&
                 count > (limit - type.scalar_program.size()) / child.scalar_program.size())) {
                type.flat_scalars = false;
                return;
            }
            if (child.scalar_program.empty()) {
                return;
            }
            for (std::uint64_t i = 0; i < count; ++i) {
                type.scalar_program.insert(type.scalar_program.end(), child.scalar_program.begin(),
                                           child.scalar_program.end());
            }
        };
        if (type.kind == TypeKind::structure) {
            for (const auto& field : type.fields) {
                append(field.type, 1);
                if (!type.flat_scalars) {
                    break;
                }
            }
        } else {
            append(type.element, type.count);
        }
        if (!type.flat_scalars) {
            type.scalar_program.clear();
        }
    }
    type.unsigned_words =
        type.fixed_size.has_value() && type.flat_scalars &&
        std::all_of(type.scalar_program.begin(), type.scalar_program.end(), [](auto p) {
            return p == Primitive::u64 || p == Primitive::time || p == Primitive::pointer;
        });
    type.validates_by_size =
        type.fixed_size.has_value() && type.flat_scalars &&
        std::all_of(type.scalar_program.begin(), type.scalar_program.end(), [](auto p) {
            return (p >= Primitive::i8 && p <= Primitive::f64) || p == Primitive::time ||
                   p == Primitive::pointer || p == Primitive::fixed || p == Primitive::ufixed;
        });
}

void scalar_validate(Primitive p, Cursor& row, std::uint64_t strings) {
    auto n = unsigned(p);
    if (p == Primitive::boolean) {
        require(row.byte() <= 1, "invalid bool argument");
    } else if (p == Primitive::str) {
        require(row.fixed<std::uint32_t>() < strings, "unknown argument string");
    } else if (p == Primitive::text || p == Primitive::bytes) {
        auto bytes = row.take(row.fixed<std::uint32_t>());
        if (p == Primitive::text) {
            d::validate_utf8(bytes);
        }
    } else if (p >= Primitive::bits && p <= Primitive::logic9) {
        auto width = row.fixed<std::uint64_t>();
        auto bytes = row.take(packed_size(width, p));
        packed_valid(bytes, width, p);
    } else {
        require(p != Primitive::enumeration, "enum requires a declared datatype");
        row.take(widths[n]);
    }
}

template <bool VisitNumbers = true, class Scalar, class Count>
void walk_row(const Types& types, TypeId id, Cursor& row, Scalar&& scalar, Count&& count) {
    const auto& type = types[id];
    require(type.minimum_size <= row.data.size(), "truncated typed payload");
    if (type.flat_scalars) {
        for (auto primitive : type.scalar_program) {
            scalar(primitive, row);
        }
    } else if (type.kind == TypeKind::structure) {
        for (const auto& field : type.fields) {
            walk_row<VisitNumbers>(types, field.type, row, scalar, count);
        }
    } else {
        auto n = type.count;
        if (type.kind == TypeKind::sequence) {
            n = row.fixed<std::uint32_t>();
            count(n);
        }
        const auto& element = types[type.element];
        require(!element.minimum_size || n <= row.data.size() / element.minimum_size,
                "truncated array payload");
        if (element.fixed_size == 0) {
            return;
        }
        if constexpr (!VisitNumbers) {
            if (element.validates_by_size) {
                row.take(std::uint64_t(n) * *element.fixed_size);
                return;
            }
        }
        if (element.flat_scalars) {
            if (element.scalar_program.size() == 1) {
                const auto primitive = element.scalar_program.front();
                for (std::uint32_t i = 0; i < n; ++i)
                    scalar(primitive, row);
            } else {
                for (std::uint32_t i = 0; i < n; ++i)
                    for (auto primitive : element.scalar_program)
                        scalar(primitive, row);
            }
        } else {
            for (std::uint32_t i = 0; i < n; ++i)
                walk_row<VisitNumbers>(types, type.element, row, scalar, count);
        }
    }
}

void append_scalar(ArgumentColumns& out, Primitive p, Cursor& row) {
    auto& numeric = out.columns[0];
    auto& floats = out.columns[1];
    auto& strings = out.columns[2];
    auto& misc = out.columns[4];
    auto n = unsigned(p);
    if (p == Primitive::unit) {
        return;
    }
    if (p == Primitive::boolean) {
        auto b = row.byte();
        require(b <= 1, "invalid bool argument");
        numeric.push_back(b);
    } else if (signed_integer(p)) {
        d::sint(numeric, sign_extend(row_number(row, widths[n]), widths[n]));
    } else if (unsigned_integer(p) || p == Primitive::time || p == Primitive::pointer) {
        d::put(numeric, row_number(row, widths[n]));
    } else if (p == Primitive::f32 || p == Primitive::f64) {
        d::append(floats, row.take(widths[n]));
    } else if (p == Primitive::str) {
        auto value = row.fixed<std::uint32_t>();
        require(value < out.string_count, "unknown argument string");
        d::put(misc, value);
    } else if (p == Primitive::text || p == Primitive::bytes) {
        auto data = row.take(row.fixed<std::uint32_t>());
        if (p == Primitive::bytes) {
            d::blob(misc, data);
        } else {
            const std::string_view text(reinterpret_cast<const char*>(data.data()), data.size());
            d::validate_utf8(data);
            if (out.shared_text) {
                if (auto i = out.shared_text->find(text); i != out.shared_text->end()) {
                    d::put(strings, (std::uint64_t(i->second) << 1) | 1);
                    return;
                }
            }
            const auto id = out.intern_text(text);
            d::put(strings, std::uint64_t(id) << (out.shared_text ? 1 : 0));
        }
    } else if (p >= Primitive::bits && p <= Primitive::logic9) {
        auto width = row.fixed<std::uint64_t>();
        auto data = row.take(packed_size(width, p));
        packed_valid(data, width, p);
        d::put(misc, width);
        d::append(misc, data);
    } else if (p == Primitive::fixed || p == Primitive::ufixed) {
        auto raw = row.fixed<std::uint64_t>();
        if (p == Primitive::fixed) {
            d::sint(numeric, raw);
        } else {
            d::put(numeric, raw);
        }
        d::sint(numeric, row.fixed<std::uint64_t>());
    } else {
        throw vtr::detail::wire::Error("unsupported primitive argument");
    }
}
} // namespace

Types::Types() {
    for (unsigned i = 0; i < first_declared_type; ++i) {
        Type type;
        type.primitive = static_cast<Primitive>(i);
        type.contains_text = type.primitive == Primitive::text;
        type.minimum_size = widths[i];
        if (i == 13 || i == 14) {
            type.minimum_size = 4;
        } else if (i >= 15 && i <= 17) {
            type.minimum_size = 8;
        } else {
            type.fixed_size = widths[i];
        }
        compile_scalars(*this, type);
        types_.push_back(std::move(type));
    }
    attributes_.resize(types_.size());
}

const Type& Types::operator[](TypeId id) const {
    require(id < types_.size() && id != unsigned(Primitive::enumeration),
            "unknown payload datatype");
    return types_[id];
}

const vtr::detail::wire::Attributes& Types::attributes(TypeId id) const {
    require(id < attributes_.size(), "unknown datatype attributes");
    return attributes_[id];
}

TypeId Types::add(Type type, vtr::detail::wire::Attributes attributes) {
    require(types_.size() < no_type, "datatype identifier exhaustion");
    d::validate_attrs(attributes);
    for (const auto& attribute : attributes)
        d::validate_value(attribute.value, SIZE_MAX);
    utf8(type.name);
    require(type.kind != TypeKind::scalar, "declared scalar datatype");
    auto add_child = [&](TypeId id, std::uint64_t count) {
        const auto& child = (*this)[id];
        require(child.minimum_size == 0 || count <= UINT32_MAX / child.minimum_size,
                "datatype minimum size exceeds u32");
        auto minimum = child.minimum_size * count;
        require(minimum <= UINT32_MAX - type.minimum_size, "datatype minimum size exceeds u32");
        type.minimum_size += minimum;
        if (type.fixed_size && child.fixed_size) {
            require(*child.fixed_size == 0 || count <= UINT32_MAX / *child.fixed_size,
                    "fixed datatype size exceeds u32");
            auto size = *child.fixed_size * count;
            require(size <= UINT32_MAX - *type.fixed_size, "fixed datatype size exceeds u32");
            *type.fixed_size += size;
        } else {
            type.fixed_size.reset();
        }
        type.contains_text |= child.contains_text && (count || type.kind == TypeKind::sequence);
        type.depth = std::max(type.depth, child.depth + 1);
        require(type.depth <= 64, "datatype nesting exceeds 64");
    };
    type.fixed_size = 0;
    if (type.kind == TypeKind::structure) {
        std::unordered_set<std::string> names;
        for (const auto& field : type.fields) {
            utf8(field.name);
            require(!field.name.empty() && names.insert(field.name).second,
                    "duplicate or empty datatype field");
            require(field.name != "vtr.label" || field.type == 12 || field.type == 13,
                    "invalid payload label datatype");
            add_child(field.type, 1);
        }
    } else if (type.kind == TypeKind::array) {
        add_child(type.element, type.count);
    } else if (type.kind == TypeKind::sequence) {
        add_child(type.element, 0);
        type.minimum_size = 4;
        type.fixed_size.reset();
    } else if (type.kind == TypeKind::enumeration) {
        const auto& base = (*this)[type.element];
        require(base.kind == TypeKind::scalar &&
                    (signed_integer(base.primitive) || unsigned_integer(base.primitive)),
                "enum underlying type must be an integer");
        add_child(type.element, 1);
        std::unordered_set<std::string> names;
        for (const auto& value : type.enumerators) {
            utf8(value.name);
            require(!value.name.empty() && names.insert(value.name).second,
                    "duplicate or empty enum literal");
            integer_range(base.primitive, value.value);
        }
    } else {
        throw vtr::detail::wire::Error("unknown datatype kind");
    }
    compile_scalars(*this, type);
    auto id = static_cast<TypeId>(types_.size());
    attributes_.push_back(std::move(attributes));
    try {
        types_.push_back(std::move(type));
    } catch (...) {
        attributes_.pop_back();
        throw;
    }
    return id;
}

TypeId Types::structure(std::string name, std::vector<Field> fields,
                        vtr::detail::wire::Attributes attributes) {
    Type type;
    type.kind = TypeKind::structure;
    type.name = std::move(name);
    type.fields = std::move(fields);
    return add(std::move(type), std::move(attributes));
}

TypeId Types::array(TypeId element, std::uint32_t count, std::string name,
                    vtr::detail::wire::Attributes attributes) {
    Type type;
    type.kind = TypeKind::array;
    type.name = std::move(name);
    type.element = element;
    type.count = count;
    return add(std::move(type), std::move(attributes));
}

TypeId Types::sequence(TypeId element, std::string name, vtr::detail::wire::Attributes attributes) {
    Type type;
    type.kind = TypeKind::sequence;
    type.name = std::move(name);
    type.element = element;
    return add(std::move(type), std::move(attributes));
}

TypeId Types::enumeration(std::string name, TypeId underlying, std::vector<Enumerator> values,
                          vtr::detail::wire::Attributes attributes) {
    Type type;
    type.kind = TypeKind::enumeration;
    type.name = std::move(name);
    type.element = underlying;
    type.enumerators = std::move(values);
    return add(std::move(type), std::move(attributes));
}

Bytes Types::declarations(TypeId first) const {
    require(first >= first_declared_type && first <= types_.size(), "invalid datatype chunk start");
    Bytes data;
    d::put(data, first);
    d::put(data, types_.size() - first);
    for (std::size_t i = first; i < types_.size(); ++i) {
        const auto& type = types_[i];
        const auto& attributes = attributes_[i];
        data.push_back(static_cast<std::uint8_t>(type.kind) | (attributes.empty() ? 0 : 0x80));
        d::blob(data, type.name);
        if (type.kind == TypeKind::structure) {
            d::put(data, type.fields.size());
            for (const auto& field : type.fields) {
                d::blob(data, field.name);
                d::put(data, field.type);
            }
        } else {
            d::put(data, type.element);
            if (type.kind == TypeKind::array) {
                d::put(data, type.count);
            }
            if (type.kind == TypeKind::enumeration) {
                d::put(data, type.enumerators.size());
                for (const auto& value : type.enumerators) {
                    d::fixed(data, value.value);
                    d::blob(data, value.name);
                }
            }
        }
        if (!attributes.empty())
            d::encode_attrs(data, attributes);
    }
    return data;
}

void Types::read_declarations(View data, std::uint64_t string_count) {
    Cursor r(data);
    require(id(r) == types_.size(), "noncontiguous datatype chunk");
    auto count = id(r);
    require(count <= r.data.size() / 2, "invalid datatype count");
    auto next = *this;
    for (std::uint32_t i = 0; i < count; ++i) {
        Type type;
        const auto tag = r.byte();
        type.kind = static_cast<TypeKind>(tag & 0x7f);
        type.name = text(r);
        if (type.kind == TypeKind::structure) {
            auto n = id(r);
            require(n <= r.data.size() / 2, "invalid datatype field count");
            for (std::uint32_t j = 0; j < n; ++j) {
                type.fields.push_back({text(r), id(r)});
            }
        } else if (type.kind >= TypeKind::array && type.kind <= TypeKind::enumeration) {
            type.element = id(r);
            if (type.kind == TypeKind::array) {
                type.count = id(r);
            }
            if (type.kind == TypeKind::enumeration) {
                auto n = id(r);
                require(n <= r.data.size() / 9, "invalid enum literal count");
                for (std::uint32_t j = 0; j < n; ++j) {
                    type.enumerators.push_back({r.fixed<std::uint64_t>(), text(r)});
                }
            }
        } else {
            throw vtr::detail::wire::Error("invalid declared datatype kind");
        }
        vtr::detail::wire::Attributes attributes;
        if (tag & 0x80) {
            attributes = d::decode_attrs(r);
            require(!attributes.empty(), "empty flagged datatype attributes");
            for (const auto& attribute : attributes) {
                require(attribute.key < string_count, "undeclared datatype attribute key");
                d::validate_value(attribute.value, string_count);
            }
        }
        next.add(std::move(type), std::move(attributes));
    }
    r.done();
    types_ = std::move(next.types_);
    attributes_ = std::move(next.attributes_);
}

void Types::validate(TypeId type, View payload, std::uint64_t string_count) const {
    require(payload.size() <= UINT32_MAX, "payload exceeds u32");
    const auto& schema = (*this)[type];
    if (schema.validates_by_size) {
        require(payload.size() == *schema.fixed_size, "fixed typed payload size mismatch");
        return;
    }
    Cursor r(payload);
    walk_row<false>(
        *this, type, r, [&](Primitive p, Cursor& row) { scalar_validate(p, row, string_count); },
        [](auto) {});
    r.done();
}

void Types::visit_text(TypeId type, View payload,
                       const std::function<void(std::string_view)>& visit) const {
    if (payload.empty())
        return;
    Cursor row(payload);
    walk_row<false>(
        *this, type, row,
        [&](Primitive p, Cursor& r) {
            if (p == Primitive::text) {
                auto bytes = r.take(r.fixed<std::uint32_t>());
                d::validate_utf8(bytes);
                visit({reinterpret_cast<const char*>(bytes.data()), bytes.size()});
            } else {
                scalar_validate(p, r, UINT64_MAX);
            }
        },
        [](auto) {});
    row.done();
}

std::uint32_t ArgumentColumns::intern_text(std::string_view text) {
    if (auto i = text_ids.find(text); i != text_ids.end())
        return i->second;
    require(dictionary.size() < UINT32_MAX, "argument dictionary exceeds u32");
    // Copies share immutable text; appending must not overwrite another copy's views.
    if (text_chunks.empty() || text_chunks.back().bytes.use_count() != 1 ||
        text.size() > text_chunks.back().bytes->size() - text_chunks.back().used)
        text_chunks.push_back(
            {std::make_shared<Bytes>(std::max<std::size_t>(4096, text.size())), 0});
    auto& chunk = text_chunks.back();
    auto* destination = chunk.bytes->data() + chunk.used;
    if (!text.empty())
        std::memcpy(destination, text.data(), text.size());
    chunk.used += text.size();
    const std::string_view owned(reinterpret_cast<const char*>(destination), text.size());
    const auto id = static_cast<std::uint32_t>(dictionary.size());
    dictionary.push_back(owned);
    try {
        auto [i, fresh] = text_ids.try_emplace(owned, id);
        if (!fresh)
            dictionary.pop_back();
        return i->second;
    } catch (...) {
        dictionary.pop_back();
        throw;
    }
}

void ArgumentColumns::append(const Types& types, TypeId type, View payload) {
    require(columns[3].empty(), "argument columns already finalized");
    require(payload.size() <= UINT32_MAX, "payload exceeds u32");
    if (types[type].fixed_size == 0) {
        require(payload.empty(), "trailing empty-type payload");
        return;
    }
    const auto& schema = types[type];
    if (schema.unsigned_words) {
        require(payload.size() == *schema.fixed_size, "fixed typed payload size mismatch");
        for (std::size_t offset = 0; offset < payload.size(); offset += 8) {
            d::put(columns[0], d::load<std::uint64_t>(payload.subspan(offset, 8)));
        }
        return;
    }
    Cursor row(payload);
    walk_row(
        types, type, row, [&](Primitive p, Cursor& r) { append_scalar(*this, p, r); },
        [&](auto n) { d::put(columns[0], n); });
    row.done();
}

void ArgumentColumns::finish() {
    require(columns[3].empty(), "argument columns already finalized");
    d::put(columns[3], (std::uint64_t(dictionary.size()) << 1) | (shared_text ? 1 : 0));
    for (const auto& value : dictionary) {
        d::blob(columns[3], value);
    }
}

ArgumentReader::ArgumentReader(std::array<View, 5> columns, std::uint64_t string_count,
                               std::size_t max_payload_bytes,
                               std::span<const std::string> shared_text)
    : columns_(columns), shared_text_(shared_text), string_count_(string_count),
      max_payload_bytes_(max_payload_bytes) {
    Cursor r(columns_[3]);
    const auto descriptor = var(r);
    shared_ = descriptor & 1;
    require(!shared_ || !shared_text_.empty(), "shared text table is unavailable");
    auto n = descriptor >> 1;
    require(n <= r.data.size(), "invalid argument dictionary count");
    std::unordered_set<std::string_view> seen;
    for (std::uint64_t i = 0; i < n; ++i) {
        auto text = r.take(var(r));
        d::validate_utf8(text);
        require(seen.emplace(reinterpret_cast<const char*>(text.data()), text.size()).second,
                "duplicate argument dictionary entry");
        dictionary_.push_back(text);
    }
    r.done();
    columns_[3] = {};
}

Bytes ArgumentReader::read(const Types& types, TypeId type) {
    if (types[type].fixed_size == 0) {
        return {};
    }
    std::array<Cursor, 5> cursors;
    for (unsigned i = 0; i < 5; ++i) {
        cursors[i] = Cursor(columns_[i]);
    }
    Payload row(64, max_payload_bytes_);
    auto scalar = [&](Primitive p) {
        auto& numeric = cursors[0];
        auto& floats = cursors[1];
        auto& strings = cursors[2];
        auto& misc = cursors[4];
        if (p == Primitive::unit) {
            return;
        }
        if (p == Primitive::boolean) {
            auto b = numeric.byte();
            require(b <= 1, "invalid bool argument");
            row.scalar(b);
        } else if (signed_integer(p) || unsigned_integer(p) || p == Primitive::time ||
                   p == Primitive::pointer) {
            auto bits = var(numeric);
            if (signed_integer(p)) {
                bits = d::unzig(bits);
            }
            integer_range(p, bits);
            auto width = widths[unsigned(p)];
            switch (width) {
            case 1:
                row.scalar(static_cast<std::uint8_t>(bits));
                break;
            case 2:
                row.scalar(static_cast<std::uint16_t>(bits));
                break;
            case 4:
                row.scalar(static_cast<std::uint32_t>(bits));
                break;
            case 8:
                row.scalar(bits);
                break;
            default:
                throw vtr::detail::wire::Error("invalid scalar width");
            }
        } else if (p == Primitive::f32 || p == Primitive::f64) {
            row.raw(floats.take(widths[unsigned(p)]));
        } else if (p == Primitive::str) {
            auto value = id(misc);
            require(value < string_count_, "unknown argument string");
            row.scalar(value);
        } else if (p == Primitive::text) {
            auto index = var(strings);
            if (shared_ && (index & 1)) {
                index >>= 1;
                require(index < shared_text_.size() && index < string_count_,
                        "unknown shared text identifier");
                const auto& text = shared_text_[index];
                d::validate_utf8({reinterpret_cast<const std::uint8_t*>(text.data()), text.size()});
                row.length(text.size());
                row.raw({reinterpret_cast<const std::uint8_t*>(text.data()), text.size()});
                return;
            }
            if (shared_)
                index >>= 1;
            require(index < dictionary_.size() && index <= next_dictionary_,
                    "invalid argument dictionary index/order");
            if (index == next_dictionary_) {
                ++next_dictionary_;
            }
            row.length(dictionary_[index].size());
            row.raw(dictionary_[index]);
        } else if (p == Primitive::bytes) {
            auto bytes = misc.take(var(misc));
            row.length(bytes.size());
            row.raw(bytes);
        } else if (p >= Primitive::bits && p <= Primitive::logic9) {
            auto width = var(misc);
            auto bytes = misc.take(packed_size(width, p));
            packed_valid(bytes, width, p);
            row.scalar(width);
            row.raw(bytes);
        } else if (p == Primitive::fixed || p == Primitive::ufixed) {
            auto raw = var(numeric);
            row.scalar(p == Primitive::fixed ? d::unzig(raw) : raw);
            row.scalar(d::unzig(var(numeric)));
        } else {
            throw vtr::detail::wire::Error("unsupported primitive argument");
        }
    };
    auto visit = [&](auto&& self, TypeId id) -> void {
        const auto& item = types[id];
        if (item.flat_scalars) {
            for (auto primitive : item.scalar_program) {
                scalar(primitive);
            }
        } else if (item.kind == TypeKind::structure) {
            for (const auto& field : item.fields) {
                self(self, field.type);
            }
        } else {
            auto n = item.count;
            if (item.kind == TypeKind::sequence) {
                n = ::vtr::detail::engine::id(cursors[0]);
                row.length(n);
            }
            const auto& element = types[item.element];
            if (element.fixed_size == 0) {
                return;
            }
            require(!element.minimum_size || n <= row.remaining() / element.minimum_size,
                    "typed payload byte limit exceeded");
            if (element.fixed_size) {
                require(n <= row.remaining() / *element.fixed_size,
                        "typed payload byte limit exceeded");
            }
            // Every nonempty decoded element consumes at least one byte from a stream.
            std::size_t remaining = 0;
            for (const auto& c : cursors) {
                remaining += c.data.size();
            }
            require(n <= remaining, "invalid encoded array count");
            for (std::uint32_t i = 0; i < n; ++i) {
                self(self, item.element);
            }
        }
    };
    visit(visit, type);
    for (unsigned i = 0; i < 5; ++i) {
        columns_[i] = cursors[i].data;
    }
    auto result = std::move(row).take();
    require(result.size() <= UINT32_MAX, "decoded payload exceeds u32");
    return result;
}

void ArgumentReader::done() const {
    for (auto c : columns_) {
        require(c.empty(), "trailing argument bytes");
    }
    require(next_dictionary_ == dictionary_.size(), "unused argument dictionary entries");
}
} // namespace vtr::detail::engine
