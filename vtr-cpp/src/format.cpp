#include "format.hpp"
#include <iomanip>
#include <lz4.h>
#include <sstream>
#include <zlib.h>
#include <zstd.h>

namespace vtr::detail {
[[noreturn]] void fail(const char *message, int code) {
  throw Error(message, code);
}
Bytes compress(View raw, Codec codec, int level) {
  require(raw.size() < (1u << 30), "compression block exceeds format limit", 1);
  Bytes packed;
  if (codec == Codec::lz4 && !raw.empty()) {
    packed.resize(LZ4_compressBound(static_cast<int>(raw.size())));
    auto size = LZ4_compress_default(reinterpret_cast<const char *>(raw.data()),
                                     reinterpret_cast<char *>(packed.data()),
                                     static_cast<int>(raw.size()),
                                     static_cast<int>(packed.size()));
    require(size > 0, "LZ4 compression failed", 6);
    packed.resize(size);
  } else if (codec == Codec::zstd && !raw.empty()) {
    packed.resize(ZSTD_compressBound(raw.size()));
    auto size = ZSTD_compress(packed.data(), packed.size(), raw.data(),
                              raw.size(), level);
    require(!ZSTD_isError(size), "Zstandard compression failed", 6);
    packed.resize(size);
  }
  if (codec == Codec::stored || raw.empty() || packed.size() >= raw.size())
    codec = Codec::stored;
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
  require(codec <= 2, "unknown compression codec", 6);
  Bytes raw(size);
  if (codec == 1) {
    require(r.data.size() <= INT32_MAX, "LZ4 input too large");
    auto n = LZ4_decompress_safe(reinterpret_cast<const char *>(r.data.data()),
                                 reinterpret_cast<char *>(raw.data()),
                                 static_cast<int>(r.data.size()),
                                 static_cast<int>(size));
    require(n >= 0 && static_cast<std::size_t>(n) == size, "invalid LZ4 block");
  } else {
    auto n =
        ZSTD_decompress(raw.data(), raw.size(), r.data.data(), r.data.size());
    require(!ZSTD_isError(n) && n == size, "invalid Zstandard block");
  }
  return raw;
}
std::uint32_t checksum(View data, std::uint32_t seed) {
  return static_cast<std::uint32_t>(crc32_z(seed, data.data(), data.size()));
}
void validate_utf8(View text) {
  for (std::size_t i = 0; i < text.size();) {
    auto first = text[i++];
    if (first < 128)
      continue;
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
    } else
      throw Error("invalid UTF-8", 1);
    require(extra <= text.size() - i, "truncated UTF-8", 1);
    for (unsigned j = 0; j < extra; ++j) {
      auto byte = text[i++];
      require((byte & 0xc0) == 0x80, "invalid UTF-8 continuation", 1);
      code = (code << 6) | (byte & 63);
    }
    require((extra != 2 || code >= 0x800) && (extra != 3 || code >= 0x10000) &&
                code <= 0x10ffff && (code < 0xd800 || code > 0xdfff),
            "invalid UTF-8 code point", 1);
  }
}
void validate_value(const Value &v, std::size_t string_count) {
  require(v.tag <= Tag::text, "unknown value tag", 1);
  if (v.tag == Tag::str || v.tag == Tag::enumeration)
    require(v.literal < string_count, "unknown value string identifier", 1);
  if (v.tag == Tag::boolean)
    require(v.number <= 1, "invalid boolean", 1);
  if ((v.tag >= Tag::bytes && v.tag <= Tag::logic9) || v.tag >= Tag::list) {
    Cursor payload(v.payload);
    decode_value(payload, v.tag);
    payload.done();
    if (v.tag == Tag::list || v.tag == Tag::map) {
      Cursor children(v.payload);
      auto count = children.var();
      for (std::uint64_t i = 0; i < count; ++i) {
        if (v.tag == Tag::map)
          require(children.id() < string_count, "unknown map key", 1);
        validate_value(decode_value(children), string_count);
      }
    }
  }
}
void encode_payload(Bytes &out, const Value &v) {
  switch (v.tag) {
  case Tag::null:
    break;
  case Tag::boolean:
    require(v.number <= 1, "invalid boolean", 1);
    out.push_back(v.number);
    break;
  case Tag::i64:
    sint(out, v.number);
    break;
  case Tag::u64:
  case Tag::time:
  case Tag::pointer:
    put(out, v.number);
    break;
  case Tag::str:
    put(out, v.literal);
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
    require(v.tag <= Tag::text, "unknown value tag", 1);
    append(out, v.payload);
    break;
  }
}
void encode_value(Bytes &out, const Value &value) {
  out.push_back(static_cast<std::uint8_t>(value.tag));
  encode_payload(out, value);
}
Value decode_value(Cursor &r, std::optional<Tag> tag, unsigned depth) {
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
  case Tag::i64:
    v.number = r.signed_bits();
    break;
  case Tag::u64:
  case Tag::time:
  case Tag::pointer:
    v.number = r.var();
    break;
  case Tag::f64:
    v.number = r.fixed<std::uint64_t>();
    break;
  case Tag::str:
    v.literal = r.id();
    break;
  case Tag::enumeration:
    v.number = r.signed_bits();
    v.literal = r.id();
    break;
  case Tag::fixed:
  case Tag::ufixed: {
    v.number = v.tag == Tag::fixed ? r.signed_bits() : r.var();
    auto scale = r.signed_var();
    require(scale >= INT32_MIN && scale <= INT32_MAX, "fixed scale overflow");
    v.scale = static_cast<std::int32_t>(scale);
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
    auto width = r.id();
    require(width && width < (1u << 31), "invalid bit width");
    auto per_byte = v.tag == Tag::bits ? 8u : v.tag == Tag::logic ? 4u : 2u;
    auto data = r.take((std::size_t(width) + per_byte - 1) / per_byte);
    auto stride = 8u / per_byte;
    for (std::size_t bit = 0; bit < width; ++bit)
      require(((data[bit / per_byte] >> ((bit % per_byte) * stride)) &
               ((1u << stride) - 1)) < (v.tag == Tag::bits    ? 2u
                                        : v.tag == Tag::logic ? 4u
                                                              : 9u),
              "invalid packed logic code");
    if (width % per_byte)
      require((data.back() >> ((width % per_byte) * stride)) == 0,
              "nonzero vector padding");
    break;
  }
  case Tag::list:
  case Tag::map: {
    auto n = r.var();
    require(n <= r.data.size(), "invalid composite count");
    for (std::uint64_t i = 0; i < n; ++i) {
      if (v.tag == Tag::map)
        r.id();
      decode_value(r, {}, depth + 1);
    }
    break;
  }
  default:
    throw Error("unknown value tag", 4);
  }
  if ((v.tag >= Tag::bytes && v.tag <= Tag::logic9) || v.tag >= Tag::list)
    v.payload.assign(start.begin(),
                     start.begin() + (start.size() - r.data.size()));
  return v;
}
void unique_attr(std::span<const Attribute> attrs, Id key) {
  require(std::none_of(attrs.begin(), attrs.end(),
                       [key](auto &a) { return a.key == key; }),
          "duplicate attribute key", 1);
}
void validate_attrs(std::span<const Attribute> attrs) {
  for (std::size_t i = 0; i < attrs.size(); ++i)
    unique_attr(attrs.first(i), attrs[i].key);
}
void encode_attrs(Bytes &out, std::span<const Attribute> attrs) {
  put(out, attrs.size());
  for (auto &a : attrs) {
    put(out, a.key);
    encode_value(out, a.value);
  }
}
Attributes decode_attrs(Cursor &r) {
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
  require(k.type <= SignalKind::varlen, "unknown signal kind", 1);
  if (k.type == SignalKind::bits)
    require(k.width > 0 && k.width < (1u << 31) &&
                (k.states == 2 || k.states == 4 || k.states == 9),
            "invalid signal width or states", 1);
}
Bytes default_value(SignalKind k) {
  Bytes b(k.packed_size(), k.type == SignalKind::bits && k.states != 2
                               ? (k.states == 4 ? 0xaa : 0x22)
                               : 0);
  if (k.type == SignalKind::bits && !b.empty()) {
    auto bits = k.width * (k.states == 2 ? 1u : k.states == 4 ? 2u : 4u);
    if (bits % 8)
      b.back() &= (1u << (bits % 8)) - 1;
  }
  return b;
}
Bytes repack(View b, std::uint32_t width, std::uint8_t from, std::uint8_t to) {
  require(from == 2 || from == 4 || from == 9, "invalid logic packing", 1);
  auto f = from == 2 ? 1u : from == 4 ? 2u : 4u;
  auto t = to == 2 ? 1u : to == 4 ? 2u : 4u;
  require(b.size() == (std::size_t(width) * f + 7) / 8,
          "packed signal size mismatch", 1);
  Bytes out((std::size_t(width) * t + 7) / 8, 0);
  for (std::size_t i = 0; i < width; ++i) {
    auto c = (b[i * f / 8] >> ((i * f) % 8)) & ((1u << f) - 1);
    require(c < from && c < to, "logic code exceeds declared states", 1);
    out[i * t / 8] |= c << ((i * t) % 8);
  }
  return out;
}
void column_attrs(TxColumns &c, std::span<const Attribute> attrs) {
  put(c[6], attrs.size());
  for (auto &a : attrs) {
    put(c[7], a.key);
    c[8].push_back(static_cast<std::uint8_t>(a.value.tag));
    auto &v = a.value;
    switch (v.tag) {
    case Tag::null:
      break;
    case Tag::f64:
      fixed(c[10], v.number);
      break;
    case Tag::str:
      put(c[11], v.literal);
      break;
    case Tag::enumeration:
      sint(c[9], v.number);
      put(c[11], v.literal);
      break;
    case Tag::bytes:
    case Tag::bits:
    case Tag::logic:
    case Tag::logic9:
    case Tag::list:
    case Tag::map:
    case Tag::text:
      append(c[12], v.payload);
      break;
    default:
      encode_payload(c[9], v);
      break;
    }
  }
}
Attributes read_column_attrs(std::array<Cursor, 24> &c) {
  auto n = c[6].var();
  require(n <= c[8].data.size(), "invalid transaction attribute count");
  Attributes attrs;
  attrs.reserve(n);
  for (std::uint64_t i = 0; i < n; ++i) {
    auto key = c[7].id();
    auto tag = static_cast<Tag>(c[8].byte());
    Value v;
    if (tag == Tag::enumeration) {
      v.tag = tag;
      v.number = c[9].signed_bits();
      v.literal = c[11].id();
    } else {
      unsigned col =
          tag == Tag::f64                                                   ? 10
          : tag == Tag::str                                                 ? 11
          : ((tag >= Tag::bytes && tag <= Tag::logic9) || tag >= Tag::list) ? 12
                                                                            : 9;
      v = decode_value(c[col], tag);
    }
    unique_attr(attrs, key);
    attrs.push_back({key, std::move(v)});
  }
  return attrs;
}
void encode_transaction(TxColumns &c, const Transaction &t, std::uint64_t id,
                        Time begin) {
  sint(c[0], t.id - id);
  put(c[1], t.generator);
  sint(c[2], t.begin - begin);
  put(c[3], t.end - t.begin);
  c[4].push_back((t.kind << 4) | t.status);
  put(c[5], t.parent ? (zig(t.id - t.parent) << 1) | 1 : 0);
  column_attrs(c, t.attrs);
  put(c[13], t.events.size());
  for (auto &e : t.events) {
    sint(c[14], e.time - t.begin);
    put(c[15], e.name);
    column_attrs(c, e.attrs);
  }
  put(c[16], t.stages.size());
  for (auto &s : t.stages) {
    put(c[17], s.name);
    put(c[18], s.lane);
    sint(c[19], s.begin - t.begin);
    put(c[20], s.end ? *s.end - s.begin + 1 : 0);
    column_attrs(c, s.attrs);
  }
}
Transaction decode_transaction(std::array<Cursor, 24> &c, std::uint64_t &id,
                               Time &begin) {
  Transaction t;
  id += c[0].signed_bits();
  t.id = id;
  require(id != 0, "zero transaction identifier");
  t.generator = c[1].id();
  begin += c[2].signed_bits();
  t.begin = begin;
  auto duration = c[3].var();
  require(duration <= forever - begin, "transaction time overflow");
  t.end = begin + duration;
  auto flags = c[4].byte();
  t.status = flags & 15;
  t.kind = flags >> 4;
  auto parent = c[5].var();
  t.parent = parent ? t.id - unzig(parent >> 1) : 0;
  t.attrs = read_column_attrs(c);
  auto n = c[13].var();
  require(n <= c[15].data.size(), "invalid event count");
  t.events.reserve(n);
  for (std::uint64_t i = 0; i < n; ++i) {
    auto time = begin + c[14].signed_bits();
    auto name = c[15].id();
    t.events.push_back({time, name, read_column_attrs(c)});
  }
  n = c[16].var();
  require(n <= c[17].data.size(), "invalid stage count");
  t.stages.reserve(n);
  for (std::uint64_t i = 0; i < n; ++i) {
    auto name = c[17].id(), lane = c[18].id();
    auto b = begin + c[19].signed_bits(), end = c[20].var();
    require(!end || end - 1 <= forever - b, "stage time overflow");
    t.stages.push_back({name, lane, b,
                        end ? std::optional<Time>(b + end - 1) : std::nullopt,
                        read_column_attrs(c)});
  }
  return t;
}
Bytes transform(View data, std::size_t width, unsigned x, bool inverse) {
  require(width && data.size() % width == 0 && x <= 3,
          "invalid value transform");
  Bytes out(data.begin(), data.end());
  auto n = data.size() / width;
  auto shuffle = [&] {
    Bytes scratch(out.size());
    auto fixed_width = [&]<std::size_t W>() {
      if (inverse) {
        for (std::size_t i = 0; i < n; ++i)
          for (std::size_t b = 0; b < W; ++b)
            scratch[i * W + b] = out[b * n + i];
      } else {
        for (std::size_t b = 0; b < W; ++b)
          for (std::size_t i = 0; i < n; ++i)
            scratch[b * n + i] = out[i * W + b];
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
      for (std::size_t b = 0; b < width; ++b)
        for (std::size_t i = 0; i < n; ++i)
          if (inverse)
            scratch[i * width + b] = out[b * n + i];
          else
            scratch[b * n + i] = out[i * width + b];
    }
    out.swap(scratch);
  };
  if (inverse && (x & 1))
    shuffle();
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
          auto sum =
              unsigned(out[i * width + b]) + out[(i - 1) * width + b] + carry;
          out[i * width + b] = sum;
          carry = sum >> 8;
        }
      }
    } else if (!decoded) {
      for (std::size_t i = n; i-- > 1;) {
        unsigned borrow = 0;
        for (std::size_t b = 0; b < width; ++b) {
          auto diff =
              unsigned(out[i * width + b]) - out[(i - 1) * width + b] - borrow;
          out[i * width + b] = diff;
          borrow = (diff >> 8) & 1;
        }
      }
    }
  }
  if (!inverse && (x & 1))
    shuffle();
  return out;
}
} // namespace vtr::detail

namespace vtr {
Value::Value(double v)
    : tag(Tag::f64), number(std::bit_cast<std::uint64_t>(v)) {}
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
  for (auto &x : values)
    detail::encode_value(v.payload, x);
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
  detail::require(kind.type == SignalKind::real, "signal is not real", 1);
  return std::bit_cast<double>(detail::load<std::uint64_t>(data));
}
std::string SignalValue::ascii() const {
  if (kind.type == SignalKind::varlen)
    return {reinterpret_cast<const char *>(data.data()), data.size()};
  if (kind.type == SignalKind::real) {
    std::ostringstream s;
    s << std::setprecision(17) << as_real();
    return s.str();
  }
  auto stride = kind.states == 2 ? 1u : kind.states == 4 ? 2u : 4u;
  std::string out(kind.width, '0');
  for (std::size_t i = 0; i < kind.width; ++i) {
    auto code =
        (data[i * stride / 8] >> ((i * stride) % 8)) & ((1u << stride) - 1);
    detail::require(code < kind.states, "invalid logic code");
    out[kind.width - i - 1] = "01xzuwlh-"[code];
  }
  return out;
}
SignalValue SignalData::at(std::size_t i) const {
  detail::require(i < times.size(), "signal history index out of range", 9);
  return {kind,
          std::span(values).subspan(offsets[i], offsets[i + 1] - offsets[i])};
}
SignalValue SignalData::value_at(Time t) const {
  auto it = std::upper_bound(times.begin(), times.end(), t);
  return it == times.begin() ? SignalValue{kind, initial}
                             : at(std::size_t(it - times.begin() - 1));
}
} // namespace vtr
