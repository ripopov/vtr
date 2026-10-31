#include "format.h"
#include <iomanip>
#include <lz4.h>
#include <sstream>
#include <unordered_set>
#include <zlib.h>
#include <zstd.h>

namespace vtr::detail::wire::detail {
[[noreturn]] void fail(const char* message, ErrorCode code) {
    throw Error(code, message);
}

Bytes compress(View raw, Codec codec, int level) {
    require(raw.size() < (1u << 30), "compression block exceeds format limit",
            ErrorCode::invalid_argument);
    Bytes packed;
    if (codec == Codec::lz4 && !raw.empty()) {
        packed.resize(LZ4_compressBound(static_cast<int>(raw.size())));
        auto size = LZ4_compress_default(
            reinterpret_cast<const char*>(raw.data()), reinterpret_cast<char*>(packed.data()),
            static_cast<int>(raw.size()), static_cast<int>(packed.size()));
        require(size > 0, "LZ4 compression failed", ErrorCode::encoding);
        packed.resize(size);
    } else if (codec == Codec::zstd && !raw.empty()) {
        packed.resize(ZSTD_compressBound(raw.size()));
        auto size = ZSTD_compress(packed.data(), packed.size(), raw.data(), raw.size(), level);
        require(!ZSTD_isError(size), "Zstandard compression failed", ErrorCode::encoding);
        packed.resize(size);
    }
    if (codec == Codec::stored || raw.empty() || packed.size() >= raw.size()) {
        codec = Codec::stored;
    }
    Bytes out;
    out.reserve(11 + (codec == Codec::stored ? raw.size() : packed.size()));
    out.push_back(static_cast<std::uint8_t>(codec));
    put(out, raw.size());
    append(out, codec == Codec::stored ? raw : View(packed));
    return out;
}

Bytes decompress(View packed) {
    Cursor r(packed);
    auto codec = r.byte();
    auto size = r.var();
    require(size < (1u << 30), "decompressed block exceeds format limit");
    if (codec == 0) {
        require(r.data.size() == size, "stored blob size mismatch");
        return Bytes(r.data.begin(), r.data.end());
    }
    require(codec <= 2, "unknown compression codec", ErrorCode::corrupt_data);
    Bytes raw(size);
    if (codec == 1) {
        require(r.data.size() <= INT32_MAX, "LZ4 input too large");
        auto n = LZ4_decompress_safe(reinterpret_cast<const char*>(r.data.data()),
                                     reinterpret_cast<char*>(raw.data()),
                                     static_cast<int>(r.data.size()), static_cast<int>(size));
        require(n >= 0 && static_cast<std::size_t>(n) == size, "invalid LZ4 block");
    } else {
        auto n = ZSTD_decompress(raw.data(), raw.size(), r.data.data(), r.data.size());
        require(!ZSTD_isError(n) && n == size, "invalid Zstandard block");
    }
    return raw;
}

std::uint32_t checksum(View data, std::uint32_t seed) {
    return static_cast<std::uint32_t>(crc32_z(seed, data.data(), data.size()));
}

void validate_utf8(View text) {
    for (std::size_t i = 0; i < text.size();) {
        if (text.size() - i >= sizeof(std::uint64_t)) {
            std::uint64_t word;
            std::memcpy(&word, text.data() + i, sizeof(word));
            if (!(word & 0x8080808080808080)) {
                i += sizeof(word);
                continue;
            }
        }
        auto first = text[i++];
        if (first < 128) {
            continue;
        }
        unsigned extra;
        std::uint32_t code;
        if (first >= 0xc2 && first <= 0xdf) {
            extra = 1;
            code = first & 31;
        } else if (first >= 0xe0 && first <= 0xef) {
            extra = 2;
            code = first & 15;
        } else if (first >= 0xf0 && first <= 0xf4) {
            extra = 3;
            code = first & 7;
        } else {
            throw Error(ErrorCode::corrupt_data, "invalid UTF-8");
        }
        require(extra <= text.size() - i, "truncated UTF-8", ErrorCode::corrupt_data);
        for (unsigned j = 0; j < extra; ++j) {
            auto byte = text[i++];
            require((byte & 0xc0) == 0x80, "invalid UTF-8 continuation", ErrorCode::corrupt_data);
            code = (code << 6) | (byte & 63);
        }
        require((extra != 2 || code >= 0x800) && (extra != 3 || code >= 0x10000) &&
                    code <= 0x10ffff && (code < 0xd800 || code > 0xdfff),
                "invalid UTF-8 code point", ErrorCode::corrupt_data);
    }
}

bool payload_value(Tag tag) {
    return (tag >= Tag::text && tag <= Tag::logic9) || tag == Tag::list || tag == Tag::map;
}

void scalar_range(const Value& value) {
    auto tag = value.tag;
    if ((tag >= Tag::i8 && tag < Tag::i64) || (tag >= Tag::u8 && tag < Tag::u64)) {
        auto offset = unsigned(tag) - unsigned(tag >= Tag::u8 ? Tag::u8 : Tag::i8);
        auto bits = 8u << offset;
        auto mask = (std::uint64_t(1) << bits) - 1;
        if (tag >= Tag::u8) {
            require((value.number & ~mask) == 0, "unsigned value out of range");
        } else {
            auto sign = std::uint64_t(1) << (bits - 1);
            require(((value.number & mask) ^ sign) - sign == value.number,
                    "signed value out of range");
        }
    }
    if (tag == Tag::f32) {
        require(value.number <= UINT32_MAX, "f32 bits out of range");
    }
}

void validate_value(const Value& v, std::size_t string_count) {
    require(v.tag <= Tag::map, "unknown value tag", ErrorCode::corrupt_data);
    if (v.tag == Tag::str || v.tag == Tag::enumeration) {
        require(v.literal < string_count, "unknown value string identifier",
                ErrorCode::corrupt_data);
    }
    scalar_range(v);
    if (v.tag == Tag::boolean) {
        require(v.number <= 1, "invalid boolean", ErrorCode::corrupt_data);
    }
    if (payload_value(v.tag)) {
        Cursor payload(v.payload);
        decode_value(payload, v.tag);
        payload.done();
        if (v.tag == Tag::list || v.tag == Tag::map) {
            Cursor children(v.payload);
            auto count = children.var();
            for (std::uint64_t i = 0; i < count; ++i) {
                if (v.tag == Tag::map) {
                    require(children.id() < string_count, "unknown map key",
                            ErrorCode::corrupt_data);
                }
                validate_value(decode_value(children), string_count);
            }
        }
    }
}

void encode_payload(Bytes& out, const Value& v) {
    scalar_range(v);
    switch (v.tag) {
    case Tag::null:
        break;
    case Tag::boolean:
        require(v.number <= 1, "invalid boolean", ErrorCode::invalid_argument);
        out.push_back(v.number);
        break;
    case Tag::i8:
    case Tag::i16:
    case Tag::i32:
    case Tag::i64:
        sint(out, v.number);
        break;
    case Tag::u8:
    case Tag::u16:
    case Tag::u32:
    case Tag::u64:
    case Tag::time:
    case Tag::pointer:
        put(out, v.number);
        break;
    case Tag::str:
        put(out, v.literal);
        break;
    case Tag::f32:
        fixed<std::uint32_t>(out, v.number);
        break;
    case Tag::f64:
        fixed(out, v.number);
        break;
    case Tag::enumeration:
        sint(out, v.number);
        put(out, v.literal);
        break;
    case Tag::fixed:
        sint(out, v.number);
        sint(out, static_cast<std::uint64_t>(v.scale));
        break;
    case Tag::ufixed:
        put(out, v.number);
        sint(out, static_cast<std::uint64_t>(v.scale));
        break;
    default:
        require(v.tag <= Tag::map, "unknown value tag", ErrorCode::invalid_argument);
        append(out, v.payload);
        break;
    }
}

void encode_value(Bytes& out, const Value& value) {
    out.push_back(static_cast<std::uint8_t>(value.tag));
    encode_payload(out, value);
}

Value decode_value(Cursor& r, std::optional<Tag> tag, unsigned depth, bool retain,
                   std::uint64_t strings) {
    require(depth < 64, "value nesting exceeds limit");
    Value v;
    v.tag = tag ? *tag : static_cast<Tag>(r.byte());
    auto start = r.data;
    switch (v.tag) {
    case Tag::null:
        break;
    case Tag::boolean:
        v.number = r.byte();
        require(v.number <= 1, "invalid boolean");
        break;
    case Tag::i8:
    case Tag::i16:
    case Tag::i32:
    case Tag::i64:
        v.number = r.signed_bits();
        break;
    case Tag::u8:
    case Tag::u16:
    case Tag::u32:
    case Tag::u64:
    case Tag::time:
    case Tag::pointer:
        v.number = r.var();
        break;
    case Tag::f32:
        v.number = r.fixed<std::uint32_t>();
        break;
    case Tag::f64:
        v.number = r.fixed<std::uint64_t>();
        break;
    case Tag::str:
        v.literal = r.id();
        require(v.literal < strings, "unknown attribute string");
        break;
    case Tag::enumeration:
        v.number = r.signed_bits();
        v.literal = r.id();
        require(v.literal < strings, "unknown attribute string");
        break;
    case Tag::fixed:
    case Tag::ufixed: {
        v.number = v.tag == Tag::fixed ? r.signed_bits() : r.var();
        v.scale = r.signed_var();
        break;
    }
    case Tag::bytes:
        r.blob();
        break;
    case Tag::text:
        validate_utf8(r.blob());
        break;
    case Tag::bits:
    case Tag::logic:
    case Tag::logic9: {
        auto width = r.var();
        auto per_byte = v.tag == Tag::bits ? 8u : v.tag == Tag::logic ? 4u : 2u;
        auto data = r.take(width / per_byte + (width % per_byte != 0));
        auto stride = 8u / per_byte;
        for (std::size_t bit = 0; bit < width; ++bit) {
            require(((data[bit / per_byte] >> ((bit % per_byte) * stride)) & ((1u << stride) - 1)) <
                        (v.tag == Tag::bits    ? 2u
                         : v.tag == Tag::logic ? 4u
                                               : 9u),
                    "invalid packed logic code");
        }
        if (width % per_byte) {
            require((data.back() >> ((width % per_byte) * stride)) == 0, "nonzero vector padding");
        }
        break;
    }
    case Tag::list:
    case Tag::map: {
        auto n = r.var();
        require(n <= r.data.size(), "invalid composite count");
        std::unordered_set<Id> keys;
        for (std::uint64_t i = 0; i < n; ++i) {
            if (v.tag == Tag::map) {
                auto key = r.id();
                require(key < strings, "unknown attribute key");
                require(keys.insert(key).second, "duplicate map key");
            }
            decode_value(r, {}, depth + 1, false, strings);
        }
        break;
    }
    default:
        throw Error(ErrorCode::corrupt_data, "unknown value tag");
    }
    scalar_range(v);
    if (retain && payload_value(v.tag)) {
        v.payload.assign(start.begin(), start.begin() + (start.size() - r.data.size()));
    }
    return v;
}

void unique_attr(std::span<const Attribute> attrs, Id key) {
    require(std::none_of(attrs.begin(), attrs.end(), [key](auto& a) { return a.key == key; }),
            "duplicate attribute key", ErrorCode::invalid_argument);
}

void validate_attrs(std::span<const Attribute> attrs) {
    for (std::size_t i = 0; i < attrs.size(); ++i) {
        unique_attr(attrs.first(i), attrs[i].key);
    }
}

void encode_attrs(Bytes& out, std::span<const Attribute> attrs) {
    put(out, attrs.size());
    for (auto& a : attrs) {
        put(out, a.key);
        encode_value(out, a.value);
    }
}

Attributes decode_attrs(Cursor& r) {
    auto n = r.var();
    require(n <= r.data.size() / 2, "invalid attribute count");
    Attributes attrs;
    attrs.reserve(n);
    for (std::uint64_t i = 0; i < n; ++i) {
        auto key = r.id();
        unique_attr(attrs, key);
        attrs.push_back({key, decode_value(r)});
    }
    return attrs;
}

void validate_kind(SignalKind k) {
    require(k.type <= SignalKind::varlen, "unknown signal kind", ErrorCode::invalid_argument);
    if (k.type == SignalKind::bits) {
        require(k.width > 0 && k.width < (1u << 31) &&
                    (k.states == 2 || k.states == 4 || k.states == 9),
                "invalid signal width or states", ErrorCode::invalid_argument);
    }
}

Bytes default_value(SignalKind k) {
    Bytes b(k.packed_size(),
            k.type == SignalKind::bits && k.states != 2 ? (k.states == 4 ? 0xaa : 0x22) : 0);
    if (k.type == SignalKind::bits && !b.empty()) {
        auto bits = k.width * (k.states == 2 ? 1u : k.states == 4 ? 2u : 4u);
        if (bits % 8) {
            b.back() &= (1u << (bits % 8)) - 1;
        }
    }
    return b;
}

Bytes repack(View b, std::uint32_t width, std::uint8_t from, std::uint8_t to) {
    require(from == 2 || from == 4 || from == 9, "invalid logic packing",
            ErrorCode::invalid_argument);
    auto f = from == 2 ? 1u : from == 4 ? 2u : 4u;
    auto t = to == 2 ? 1u : to == 4 ? 2u : 4u;
    require(b.size() == (std::size_t(width) * f + 7) / 8, "packed signal size mismatch",
            ErrorCode::invalid_argument);
    Bytes out((std::size_t(width) * t + 7) / 8, 0);
    for (std::size_t i = 0; i < width; ++i) {
        auto c = (b[i * f / 8] >> ((i * f) % 8)) & ((1u << f) - 1);
        require(c < from && c < to, "logic code exceeds declared states",
                ErrorCode::invalid_argument);
        out[i * t / 8] |= c << ((i * t) % 8);
    }
    return out;
}

void checked_column_attrs(AttributeColumns& c, std::span<const Attribute> attrs,
                          std::uint64_t strings) {
    put(c[0], attrs.size());
    Id maximum_key = 0;
    for (std::size_t i = 0; i < attrs.size(); ++i) {
        const auto& a = attrs[i];
        if (i && a.key <= maximum_key)
            unique_attr(attrs.first(i), a.key);
        maximum_key = std::max(maximum_key, a.key);
        require(a.key < strings, "unknown transaction attribute key");
        put(c[1], a.key);
        c[2].push_back(static_cast<std::uint8_t>(a.value.tag));
        auto& v = a.value;
        switch (v.tag) {
        case Tag::null:
            break;
        case Tag::i64:
            sint(c[3], v.number);
            break;
        case Tag::u64:
        case Tag::time:
        case Tag::pointer:
            put(c[3], v.number);
            break;
        case Tag::f32:
            require(v.number <= UINT32_MAX, "f32 bits out of range");
            fixed<std::uint32_t>(c[4], v.number);
            break;
        case Tag::f64:
            fixed(c[4], v.number);
            break;
        case Tag::str:
            require(v.literal < strings, "unknown value string identifier",
                    ErrorCode::invalid_argument);
            put(c[5], v.literal);
            break;
        case Tag::enumeration:
            require(v.literal < strings, "unknown value string identifier",
                    ErrorCode::invalid_argument);
            sint(c[3], v.number);
            put(c[5], v.literal);
            break;
        case Tag::bytes:
        case Tag::bits:
        case Tag::logic:
        case Tag::logic9:
        case Tag::list:
        case Tag::map:
        case Tag::text:
            validate_value(v, strings);
            append(c[6], v.payload);
            break;
        default:
            encode_payload(c[3], v);
            break;
        }
    }
}

namespace {
template <bool Retain> Attributes column_attrs(std::array<Cursor, 7>& c, std::uint64_t strings) {
    auto n = c[0].var();
    require(n <= c[2].data.size(), "invalid transaction attribute count");
    Attributes attrs;
    std::unordered_set<Id> keys;
    if constexpr (Retain)
        attrs.reserve(n);
    for (std::uint64_t i = 0; i < n; ++i) {
        auto key = c[1].id();
        require(key < strings, "unknown attribute key");
        auto tag = static_cast<Tag>(c[2].byte());
        Value v;
        if (tag == Tag::enumeration) {
            v.tag = tag;
            v.number = c[3].signed_bits();
            v.literal = c[5].id();
            require(v.literal < strings, "unknown attribute string");
        } else {
            unsigned col = (tag == Tag::f32 || tag == Tag::f64) ? 4
                           : tag == Tag::str                    ? 5
                           : (payload_value(tag))               ? 6
                                                                : 3;
            v = decode_value(c[col], tag, 0, Retain, strings);
        }
        if constexpr (Retain) {
            unique_attr(attrs, key);
            attrs.push_back({key, std::move(v)});
        } else {
            require(keys.insert(key).second, "duplicate attribute key");
        }
    }
    return attrs;
}
} // namespace

Attributes read_column_attrs(std::array<Cursor, 7>& c) {
    return column_attrs<true>(c, UINT64_MAX);
}

void skip_column_attrs(std::array<Cursor, 7>& c, std::uint64_t strings) {
    column_attrs<false>(c, strings);
}

Bytes transform(View data, std::size_t width, unsigned x, bool inverse) {
    require(width && data.size() % width == 0 && x <= 3, "invalid value transform");
    Bytes out(data.begin(), data.end());
    auto n = data.size() / width;
    auto shuffle = [&] {
        Bytes scratch(out.size());
        auto fixed_width = [&]<std::size_t W>() {
            if (inverse) {
                for (std::size_t i = 0; i < n; ++i) {
                    for (std::size_t b = 0; b < W; ++b) {
                        scratch[i * W + b] = out[b * n + i];
                    }
                }
            } else {
                for (std::size_t b = 0; b < W; ++b) {
                    for (std::size_t i = 0; i < n; ++i) {
                        scratch[b * n + i] = out[i * W + b];
                    }
                }
            }
        };
        switch (width) {
        case 1:
            return;
        case 2:
            fixed_width.template operator()<2>();
            break;
        case 4:
            fixed_width.template operator()<4>();
            break;
        case 8:
            fixed_width.template operator()<8>();
            break;
        case 16:
            fixed_width.template operator()<16>();
            break;
        default:
            for (std::size_t b = 0; b < width; ++b) {
                for (std::size_t i = 0; i < n; ++i) {
                    if (inverse) {
                        scratch[i * width + b] = out[b * n + i];
                    } else {
                        scratch[b * n + i] = out[i * width + b];
                    }
                }
            }
        }
        out.swap(scratch);
    };
    if (inverse && (x & 1)) {
        shuffle();
    }
    if (x & 2) {
        auto words = [&]<class T>() {
            T previous = 0;
            for (std::size_t i = 0; i < n; ++i) {
                T value;
                std::memcpy(&value, out.data() + i * sizeof(T), sizeof(T));
                T next = inverse ? T(value + previous) : T(value - previous);
                previous = inverse ? next : value;
                std::memcpy(out.data() + i * sizeof(T), &next, sizeof(T));
            }
        };
        bool decoded = false;
        if constexpr (std::endian::native == std::endian::little) {
            decoded = true;
            switch (width) {
            case 1:
                words.template operator()<std::uint8_t>();
                break;
            case 2:
                words.template operator()<std::uint16_t>();
                break;
            case 4:
                words.template operator()<std::uint32_t>();
                break;
            case 8:
                words.template operator()<std::uint64_t>();
                break;
            default:
                decoded = false;
            }
        }
        if (!decoded && inverse) {
            for (std::size_t i = 1; i < n; ++i) {
                unsigned carry = 0;
                for (std::size_t b = 0; b < width; ++b) {
                    auto sum = unsigned(out[i * width + b]) + out[(i - 1) * width + b] + carry;
                    out[i * width + b] = sum;
                    carry = sum >> 8;
                }
            }
        } else if (!decoded) {
            for (std::size_t i = n; i-- > 1;) {
                unsigned borrow = 0;
                for (std::size_t b = 0; b < width; ++b) {
                    auto diff = unsigned(out[i * width + b]) - out[(i - 1) * width + b] - borrow;
                    out[i * width + b] = diff;
                    borrow = (diff >> 8) & 1;
                }
            }
        }
    }
    if (!inverse && (x & 1)) {
        shuffle();
    }
    return out;
}
} // namespace vtr::detail::wire::detail

namespace vtr::detail::wire {
Value::Value(float v) : tag(Tag::f32), number(std::bit_cast<std::uint32_t>(v)) {}

Value::Value(double v) : tag(Tag::f64), number(std::bit_cast<std::uint64_t>(v)) {}

Value Value::string(Id id) {
    Value v;
    v.tag = Tag::str;
    v.literal = id;
    return v;
}

Value Value::text(std::string_view text) {
    Value v;
    v.tag = Tag::text;
    detail::blob(v.payload, text);
    return v;
}

Value Value::time(Time t) {
    Value v(t);
    v.tag = Tag::time;
    return v;
}

Value Value::bytes(std::span<const std::uint8_t> data) {
    Value v;
    v.tag = Tag::bytes;
    detail::blob(v.payload, data);
    return v;
}

Value Value::list(std::span<const Value> values) {
    Value v;
    v.tag = Tag::list;
    detail::put(v.payload, values.size());
    for (auto& x : values) {
        detail::encode_value(v.payload, x);
    }
    return v;
}

std::size_t SignalKind::packed_size() const {
    return type == varlen ? 0
           : type == real ? 8
                          : (std::size_t(width) * (states == 2   ? 1
                                                   : states == 4 ? 2
                                                                 : 4) +
                             7) /
                                8;
}

double SignalValue::as_real() const {
    detail::require(kind.type == SignalKind::real, "signal is not real",
                    ErrorCode::invalid_argument);
    return std::bit_cast<double>(detail::load<std::uint64_t>(data));
}

std::string SignalValue::ascii() const {
    if (kind.type == SignalKind::varlen) {
        return {reinterpret_cast<const char*>(data.data()), data.size()};
    }
    if (kind.type == SignalKind::real) {
        std::ostringstream s;
        s << std::setprecision(17) << as_real();
        return s.str();
    }
    auto stride = kind.states == 2 ? 1u : kind.states == 4 ? 2u : 4u;
    std::string out(kind.width, '0');
    for (std::size_t i = 0; i < kind.width; ++i) {
        auto code = (data[i * stride / 8] >> ((i * stride) % 8)) & ((1u << stride) - 1);
        detail::require(code < kind.states, "invalid logic code");
        out[kind.width - i - 1] = "01xzuwlh-"[code];
    }
    return out;
}

SignalValue SignalData::at(std::size_t i) const {
    detail::require(i < times.size(), "signal history index out of range",
                    ErrorCode::invalid_argument);
    return {kind, std::span(values).subspan(offsets[i], offsets[i + 1] - offsets[i])};
}

SignalValue SignalData::value_at(Time t) const {
    auto it = std::upper_bound(times.begin(), times.end(), t);
    return it == times.begin() ? SignalValue{kind, initial}
                               : at(std::size_t(it - times.begin() - 1));
}
} // namespace vtr::detail::wire
