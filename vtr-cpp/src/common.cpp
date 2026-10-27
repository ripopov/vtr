#include "bridge.h"

namespace vtr {
Error::Error(ErrorCode code, std::string message)
    : std::runtime_error(std::move(message)), code_(code) {}

ErrorCode Error::code() const noexcept {
    return code_;
}

namespace detail {
wire::Value encode_value(const Value& value, unsigned depth) {
    if (depth >= 64) {
        throw Error(ErrorCode::invalid_argument, "Attribute nesting exceeds 64");
    }
    wire::Value result;
    result.tag = static_cast<wire::Tag>(value.data.index());
    std::visit(
        [&]<class T>(const T& item) {
            if constexpr (std::same_as<T, std::monostate>) {
            } else if constexpr (std::integral<T>) {
                result.number = static_cast<std::uint64_t>(item);
            } else if constexpr (std::same_as<T, float>) {
                result.number = std::bit_cast<std::uint32_t>(item);
            } else if constexpr (std::same_as<T, double>) {
                result.number = std::bit_cast<std::uint64_t>(item);
            } else if constexpr (std::same_as<T, StringId>) {
                result.literal = item.value;
            } else if constexpr (std::same_as<T, std::string>) {
                binary::blob(result.payload, item);
            } else if constexpr (std::same_as<T, Bytes>) {
                binary::blob(result.payload, view(item));
            } else if constexpr (std::same_as<T, Bits> || std::same_as<T, Logic> ||
                                 std::same_as<T, Logic9>) {
                binary::put(result.payload, item.width);
                binary::append(result.payload, view(item.packed));
            } else if constexpr (std::same_as<T, TimeValue> || std::same_as<T, Address>) {
                result.number = item.value;
            } else if constexpr (std::same_as<T, EnumValue>) {
                result.number = static_cast<std::uint64_t>(item.value);
                result.literal = item.literal.value;
            } else if constexpr (std::same_as<T, Fixed> || std::same_as<T, UFixed>) {
                result.number = static_cast<std::uint64_t>(item.raw);
                result.scale = item.scale;
            } else {
                binary::put(result.payload, item.size());
                for (const auto& child : item) {
                    if constexpr (std::same_as<T, Value::List>) {
                        binary::encode_value(result.payload, encode_value(child, depth + 1));
                    } else {
                        binary::put(result.payload, child.key.value);
                        binary::encode_value(result.payload, encode_value(child.data, depth + 1));
                    }
                }
            }
        },
        value.data);
    return result;
}

Value decode_value(const wire::Value& value, unsigned depth) {
    if (depth >= 64) {
        throw Error(ErrorCode::corrupt_data, "Attribute nesting exceeds 64");
    }
    switch (value.tag) {
    case wire::Tag::null:
        return {};
    case wire::Tag::boolean:
        return Value(static_cast<bool>(value.number));
    case wire::Tag::i8:
        return Value(static_cast<std::int8_t>(value.number));
    case wire::Tag::i16:
        return Value(static_cast<std::int16_t>(value.number));
    case wire::Tag::i32:
        return Value(static_cast<std::int32_t>(value.number));
    case wire::Tag::i64:
        return Value(static_cast<std::int64_t>(value.number));
    case wire::Tag::u8:
        return Value(static_cast<std::uint8_t>(value.number));
    case wire::Tag::u16:
        return Value(static_cast<std::uint16_t>(value.number));
    case wire::Tag::u32:
        return Value(static_cast<std::uint32_t>(value.number));
    case wire::Tag::u64:
        return Value(static_cast<std::uint64_t>(value.number));
    case wire::Tag::f32:
        return Value(std::bit_cast<float>(static_cast<std::uint32_t>(value.number)));
    case wire::Tag::f64:
        return Value(std::bit_cast<double>(value.number));
    case wire::Tag::str:
        return Value(StringId(value.literal));
    case wire::Tag::time:
        return Value(TimeValue{value.number});
    case wire::Tag::pointer:
        return Value(Address{value.number});
    case wire::Tag::enumeration:
        return Value(EnumValue{std::bit_cast<std::int64_t>(value.number), StringId(value.literal)});
    case wire::Tag::fixed:
        return Value(Fixed{std::bit_cast<std::int64_t>(value.number), value.scale});
    case wire::Tag::ufixed:
        return Value(UFixed{value.number, value.scale});
    default:
        break;
    }
    binary::Cursor cursor(value.payload);
    switch (value.tag) {
    case wire::Tag::text:
        return Value(cursor.string());
    case wire::Tag::bytes:
        return Value(bytes(cursor.blob()));
    case wire::Tag::bits: {
        auto width = cursor.var();
        return Value(Bits{width, bytes(cursor.data)});
    }
    case wire::Tag::logic: {
        auto width = cursor.var();
        return Value(Logic{width, bytes(cursor.data)});
    }
    case wire::Tag::logic9: {
        auto width = cursor.var();
        return Value(Logic9{width, bytes(cursor.data)});
    }
    case wire::Tag::list: {
        Value::List result;
        auto size = cursor.var();
        for (std::uint64_t i = 0; i < size; ++i) {
            result.push_back(decode_value(binary::decode_value(cursor), depth + 1));
        }
        cursor.done();
        return Value(std::move(result));
    }
    case wire::Tag::map: {
        Value::Map result;
        auto size = cursor.var();
        for (std::uint64_t i = 0; i < size; ++i) {
            auto key = StringId(cursor.id());
            result.push_back({key, decode_value(binary::decode_value(cursor), depth + 1)});
        }
        cursor.done();
        return Value(std::move(result));
    }
    default:
        throw Error(ErrorCode::corrupt_data, "Unknown attribute tag");
    }
}

wire::Attributes encode_attributes(AttributeView attributes) {
    wire::Attributes result;
    result.reserve(attributes.size());
    for (const auto& item : attributes) {
        result.push_back({item.key.value, encode_value(item.data)});
    }
    return result;
}

Attributes decode_attributes(std::span<const wire::Attribute> attributes) {
    Attributes result;
    result.reserve(attributes.size());
    for (const auto& item : attributes) {
        result.push_back({StringId(item.key), decode_value(item.value)});
    }
    return result;
}

wire::Meta encode_metadata(const Metadata& metadata) {
    wire::Meta result;
    result.timescale = metadata.timescale;
    result.time_zero = metadata.time_zero;
    result.writer = metadata.writer;
    result.date = metadata.date;
    result.comment = metadata.comment;
    result.attrs = encode_attributes(metadata.extra);
    return result;
}

Metadata decode_metadata(const wire::Meta& metadata) {
    return {metadata.timescale, metadata.time_zero, metadata.writer,
            metadata.date,      metadata.comment,   decode_attributes(metadata.attrs)};
}

DataType decode_type(std::uint32_t id, const engine::Type& type,
                     const wire::Attributes& attributes) {
    DataType result{TypeId(id), type.name, static_cast<Primitive>(type.primitive),
                    decode_attributes(attributes)};
    switch (type.kind) {
    case engine::TypeKind::scalar:
        break;
    case engine::TypeKind::structure: {
        StructDefinition definition;
        for (const auto& field : type.fields) {
            definition.fields.push_back({field.name, TypeId(field.type)});
        }
        result.definition = std::move(definition);
        break;
    }
    case engine::TypeKind::array:
        result.definition = ArrayDefinition{TypeId(type.element), type.count};
        break;
    case engine::TypeKind::sequence:
        result.definition = SequenceDefinition{TypeId(type.element)};
        break;
    case engine::TypeKind::enumeration: {
        EnumDefinition definition{TypeId(type.element), {}};
        for (const auto& value : type.enumerators) {
            definition.literals.push_back({value.name, value.value});
        }
        result.definition = std::move(definition);
        break;
    }
    }
    return result;
}

} // namespace detail
} // namespace vtr
