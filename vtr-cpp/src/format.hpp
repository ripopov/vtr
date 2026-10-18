#pragma once
#include "vtr.hpp"
#include <algorithm>
#include <array>
#include <bit>
#include <cstring>

namespace vtr::detail {
using View = std::span<const std::uint8_t>;
[[noreturn]] void fail(const char *message, int code);
inline void require(bool ok, const char *message, int code = 4) {
  if (!ok) [[unlikely]]
    fail(message, code);
}
[[gnu::always_inline]] inline void
append_scalar(Bytes &out, std::uint64_t value, std::size_t size) {
  const auto offset = out.size();
  out.resize(offset + size);
  if constexpr (std::endian::native == std::endian::little) {
    switch (size) {
    case 1:
      out[offset] = static_cast<std::uint8_t>(value);
      break;
    case 2: {
      auto v = static_cast<std::uint16_t>(value);
      std::memcpy(out.data() + offset, &v, 2);
      break;
    }
    case 4: {
      auto v = static_cast<std::uint32_t>(value);
      std::memcpy(out.data() + offset, &v, 4);
      break;
    }
    case 8:
      std::memcpy(out.data() + offset, &value, 8);
      break;
    default:
      std::memcpy(out.data() + offset, &value, size);
      break;
    }
  } else
    for (std::size_t i = 0; i < size; ++i)
      out[offset + i] = static_cast<std::uint8_t>(value >> (8 * i));
}
inline std::uint64_t zig(std::uint64_t v) { return (v << 1) ^ (0 - (v >> 63)); }
inline std::uint64_t unzig(std::uint64_t v) { return (v >> 1) ^ (0 - (v & 1)); }
[[gnu::always_inline]] inline void put(Bytes &out, std::uint64_t v) {
  while (v >= 128) {
    out.push_back(static_cast<std::uint8_t>(v) | 128);
    v >>= 7;
  }
  out.push_back(static_cast<std::uint8_t>(v));
}
inline void sint(Bytes &out, std::uint64_t v) { put(out, zig(v)); }
[[gnu::always_inline]] inline void append(Bytes &out, View v) {
  out.insert(out.end(), v.begin(), v.end());
}
inline View view(std::string_view s) {
  return {reinterpret_cast<const std::uint8_t *>(s.data()), s.size()};
}
inline void blob(Bytes &out, View v) {
  put(out, v.size());
  append(out, v);
}
inline void blob(Bytes &out, std::string_view s) { blob(out, view(s)); }
template <class T> void fixed(Bytes &out, T value) {
  for (unsigned i = 0; i < sizeof(T); ++i)
    out.push_back(static_cast<std::uint8_t>(value >> (i * 8)));
}
template <class T> T load(View p) {
  require(p.size() >= sizeof(T), "truncated fixed integer");
  T result = 0;
  for (unsigned i = 0; i < sizeof(T); ++i)
    result |= static_cast<T>(p[i]) << (8 * i);
  return result;
}
void validate_utf8(View text);
struct Cursor {
  View data;
  explicit Cursor(View d = {}) : data(d) {}
  bool empty() const { return data.empty(); }
  View take(std::size_t n) {
    require(n <= data.size(), "truncated payload");
    auto v = data.first(n);
    data = data.subspan(n);
    return v;
  }
  std::uint8_t byte() { return take(1)[0]; }
  std::uint64_t var() {
    std::uint64_t v = 0;
    for (unsigned shift = 0; shift < 64; shift += 7) {
      auto b = byte();
      require(shift != 63 || b <= 1, "varint overflow");
      v |= std::uint64_t(b & 127) << shift;
      if (!(b & 128))
        return v;
    }
    throw Error("varint overflow", 4);
  }
  std::uint32_t id() {
    auto v = var();
    require(v <= UINT32_MAX, "identifier overflow");
    return static_cast<Id>(v);
  }
  std::uint64_t signed_bits() { return unzig(var()); }
  std::int64_t signed_var() {
    return std::bit_cast<std::int64_t>(signed_bits());
  }
  View blob() {
    auto n = var();
    return take(n);
  }
  std::string string() {
    auto b = blob();
    validate_utf8(b);
    return {reinterpret_cast<const char *>(b.data()), b.size()};
  }
  template <class T> T fixed() { return load<T>(take(sizeof(T))); }
  void done() const { require(empty(), "trailing payload bytes"); }
};
Bytes compress(View raw, Codec codec, int level);
Bytes decompress(View packed);
std::uint32_t checksum(View data, std::uint32_t seed = 0);
void seal_file(const std::string &path, Ending ending);
void encode_payload(Bytes &out, const Value &value);
void validate_value(const Value &value, std::size_t string_count);
Value decode_value(Cursor &r, std::optional<Tag> tag = {}, unsigned depth = 0);
void encode_value(Bytes &out, const Value &value);
void encode_attrs(Bytes &out, std::span<const Attribute> attrs);
Attributes decode_attrs(Cursor &r);
void unique_attr(std::span<const Attribute> attrs, Id key);
void validate_attrs(std::span<const Attribute> attrs);
Bytes default_value(SignalKind kind);
Bytes repack(View bytes, std::uint32_t width, std::uint8_t from,
             std::uint8_t to);
void validate_kind(SignalKind kind);
struct Section {
  std::uint32_t kind = 0, flags = 0;
  std::uint64_t offset = 0, length = 0, aux0 = 0, aux1 = 0;
};
inline constexpr std::array<std::uint8_t, 8> magic{0x89, 'V', 'T', 'R',
                                                   13,   10,  26,  10};
inline constexpr std::array<std::uint8_t, 8> trailer{'V', 'T', 'R', '_',
                                                     'E', 'N', 'D', 0};
using TxColumns = std::array<Bytes, 24>;
void column_attrs(TxColumns &columns, std::span<const Attribute> attrs);
Attributes read_column_attrs(std::array<Cursor, 24> &columns);
void encode_transaction(TxColumns &columns, const Transaction &tx,
                        std::uint64_t previous_id, Time previous_begin);
Transaction decode_transaction(std::array<Cursor, 24> &columns,
                               std::uint64_t &previous_id,
                               Time &previous_begin);
template <std::size_t N>
Bytes pack_columns(const std::array<Bytes, N> &columns) {
  Bytes raw;
  std::size_t total = N * 10 + 10;
  for (auto &c : columns) {
    require(c.size() <= (1u << 30) - total,
            "column block exceeds format limit");
    total += c.size();
  }
  put(raw, N);
  raw.reserve(total);
  for (auto &c : columns)
    put(raw, c.size());
  for (auto &c : columns)
    append(raw, c);
  return raw;
}
template <std::size_t N> std::array<Cursor, N> unpack_columns(View raw) {
  Cursor r(raw);
  require(r.var() == N, "invalid column count");
  std::array<std::size_t, N> sizes{};
  for (auto &n : sizes)
    n = r.var();
  std::array<Cursor, N> columns;
  for (std::size_t i = 0; i < N; ++i)
    columns[i] = Cursor(r.take(sizes[i]));
  r.done();
  return columns;
}
Bytes transform(View data, std::size_t width, unsigned transform, bool inverse);
} // namespace vtr::detail
