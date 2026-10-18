#pragma once

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

namespace vtr {
using Id = std::uint32_t;
using Time = std::uint64_t;
using Bytes = std::vector<std::uint8_t>;
inline constexpr Id none = UINT32_MAX;
inline constexpr Time forever = UINT64_MAX;

class Error : public std::runtime_error {
public:
  int code;
  explicit Error(std::string message, int code = 1)
      : std::runtime_error(std::move(message)), code(code) {}
};

enum class Codec : std::uint8_t { stored, lz4, zstd };
enum class Tag : std::uint8_t {
  null,
  boolean,
  i64,
  u64,
  f64,
  str,
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
  map,
  text
};

// Composite values keep their canonical payload, including nested tags, without
// a leading tag.
struct Value {
  Tag tag = Tag::null;
  std::uint64_t number = 0;
  Id literal = 0;
  std::int32_t scale = 0;
  Bytes payload;
  Value() = default;
  explicit Value(std::uint64_t v) : tag(Tag::u64), number(v) {}
  explicit Value(std::int64_t v)
      : tag(Tag::i64), number(static_cast<std::uint64_t>(v)) {}
  explicit Value(bool v) : tag(Tag::boolean), number(v) {}
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
  friend bool operator==(const SignalKind &, const SignalKind &) = default;
};
struct SignalValue {
  SignalKind kind;
  std::span<const std::uint8_t> data;
  std::uint64_t as_u64() const;
  double as_real() const;
  std::string ascii() const;
};
inline std::uint64_t SignalValue::as_u64() const {
  if (kind.type != SignalKind::bits || kind.width > 64 ||
      data.size() != kind.packed_size())
    throw Error("signal is not a narrow vector");
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
    for (std::size_t i = 0; i < data.size(); ++i)
      value |= std::uint64_t(data[i]) << (8 * i);
    return value;
  }
  std::uint64_t value = 0;
  auto stride = kind.states == 4 ? 2u : 4u;
  for (std::size_t i = 0; i < kind.width; ++i) {
    auto code =
        (data[i * stride / 8] >> ((i * stride) % 8)) & ((1u << stride) - 1);
    if (code > 1)
      throw Error("signal contains nonbinary logic");
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
  enum Kind : std::uint8_t {
    scope = 1,
    variable,
    generator,
    enum_table
  } kind = scope;
  Id parent = none, name = 0, signal = none, aux = 0;
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
struct Event {
  Time time;
  Id name;
  Attributes attrs;
};
struct Stage {
  Id name, lane;
  Time begin;
  std::optional<Time> end;
  Attributes attrs;
};
struct Transaction {
  std::uint64_t id = 0, parent = 0;
  Id generator = none;
  Time begin = 0, end = 0;
  std::uint8_t status = 0, kind = 0;
  Attributes attrs;
  std::vector<Event> events;
  std::vector<Stage> stages;
};
struct Relation {
  Id kind;
  std::uint64_t from, to;
  Attributes attrs;
};
struct LogSite {
  Id node = none, scope = none, format = 0, file = 0, function = 0;
  std::uint32_t line = 0;
  std::uint8_t severity = 2;
  std::vector<Tag> types;
  std::vector<Id> names;
};
struct LogRecord {
  std::uint64_t id = 0, parent = 0;
  Time time = 0;
  Id site = none;
  std::vector<Value> args;
};
struct Ending {
  enum Kind : std::uint8_t {
    closed,
    stopped,
    exited,
    crashed,
    recovered,
    poisoned
  } kind = closed;
  std::int32_t signal = 0, code = 0, status = 0;
  std::uint64_t address = 0, thread = 0, dropped = 0;
  bool sealed = false;
  std::optional<Time> time;
};
struct WriterOptions {
  Codec codec = Codec::lz4;
  int compression_level = 3;
  std::uint32_t group_size = 256;
  std::uint64_t block_records = 1 << 24;
  std::size_t tx_block_bytes = 4 << 20;
  bool dedup = true, checksums = true;
  std::uint32_t commit_interval_ms = 0;
};

// Writers have one producer thread. Borrowed arguments are copied before a call
// returns.
class Writer {
public:
  explicit Writer(const std::string &path, WriterOptions options = {});
  ~Writer();
  Writer(const Writer &) = delete;
  Writer &operator=(const Writer &) = delete;
  Meta &metadata();
  Id intern(std::string_view value);
  Id add_scope(Id parent, std::string_view name, std::uint16_t type = 0,
               std::string_view component = {});
  std::pair<Id, Id> add_var(Id parent, std::string_view name, SignalKind kind,
                            std::uint16_t type = 16,
                            std::uint8_t direction = 0);
  Id add_alias(Id parent, std::string_view name, Id signal,
               std::uint16_t type = 16, std::uint8_t direction = 0);
  Id add_generator(Id parent, std::string_view name, std::string_view kind);
  Id add_enum_table(Id parent, std::string_view name,
                    std::span<const std::pair<Id, Id>> entries);
  void node_attr(Id node, Id key, const Value &value);
  void set_time(Time time);
  Time time() const noexcept;
  void dump(bool active);
  void emit(Id signal, std::uint64_t value);
  void emit_bit(Id signal, std::uint8_t code);
  void emit_real(Id signal, double value);
  void emit_packed(Id signal, std::uint8_t states,
                   std::span<const std::uint8_t> value);
  void emit_words(Id signal, std::span<const std::uint32_t> words);
  void emit_logic(Id signal, std::string_view value);
  void emit_varlen(Id signal, std::span<const std::uint8_t> value);
  std::uint64_t begin_tx(Id generator, Time begin);
  void tx_attr(std::uint64_t tx, Id key, std::uint64_t value);
  void tx_attr(std::uint64_t tx, Id key, const Value &value);
  void tx_parent(std::uint64_t tx, std::uint64_t parent);
  void tx_kind(std::uint64_t tx, std::uint8_t kind);
  void tx_event(std::uint64_t tx, Time time, Id name,
                std::span<const Attribute> attrs = {});
  void tx_stage(std::uint64_t tx, Id name, Id lane, Time begin,
                std::optional<Time> end, std::span<const Attribute> attrs = {});
  void tx_stage_end(std::uint64_t tx, Id name, Id lane, Time end);
  void tx_stage_attr(std::uint64_t tx, Id key, const Value &value);
  void end_tx(std::uint64_t tx, Time end, std::uint8_t status = 0);
  void relate(Id kind, std::uint64_t from, std::uint64_t to,
              std::span<const Attribute> attrs = {});
  Id add_log_site(Id parent, std::uint8_t severity, std::string_view format,
                  std::span<const Tag> types,
                  std::span<const std::string_view> names = {},
                  std::string_view file = {}, std::uint32_t line = 0,
                  std::string_view function = {});
  Id log_site_node(Id site) const;
  std::uint64_t log(Id site, Time time, std::span<const Value> args,
                    std::uint64_t parent = 0);
  std::uint64_t log_raw(Id site, Time time, std::span<const std::uint8_t> args,
                        std::uint64_t parent = 0);
  Id add_clock(Id parent, std::string_view name);
  Id clock_generator(Id clock) const;
  void clock_run(Id clock, Time first, Time period);
  void clock_stop(Id clock, Time time);
  void flush();
  void close(Ending ending = {});

private:
  void maybe_commit();
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

struct Query {
  Id generator = none, scope = none;
  Time begin = 0, end = forever;
};
struct ReaderStats {
  std::uint64_t transactions = 0, relations = 0, logs = 0,
                recovered_dropped = 0;
  std::uint32_t signal_blocks = 0, tx_blocks = 0, log_blocks = 0;
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
  explicit Reader(const std::string &path, ReaderOptions options = {});
  ~Reader();
  Reader(const Reader &) = delete;
  Reader &operator=(const Reader &) = delete;
  const Meta &metadata() const;
  const ReaderStats &stats() const;
  std::span<const std::string> strings() const;
  std::string_view string(Id id) const;
  std::span<const Node> nodes() const;
  std::span<const SignalKind> signals() const;
  Id signal_var(Id signal) const;
  Id find_node(std::string_view path, char separator = '.') const;
  Id find_signal(std::string_view path, char separator = '.') const;
  bool in_scope(Id node, Id scope) const;
  void changes(
      Id signal, Time begin, Time end,
      const std::function<bool(Time, Id, const SignalValue &)> &visit) const;
  void changes(
      Time begin, Time end,
      const std::function<bool(Time, Id, const SignalValue &)> &visit) const;
  Bytes value_at(Id signal, Time time) const;
  std::shared_ptr<const SignalData> load_signal(Id signal) const;
  const std::vector<Time> &time_table() const;
  std::span<const std::pair<Time, bool>> blackouts() const;
  void
  transactions(Query query,
               const std::function<bool(const Transaction &)> &visit) const;
  bool transaction(std::uint64_t id,
                   const std::function<void(const Transaction &)> &visit) const;
  void relations(std::uint64_t id, bool incoming,
                 const std::function<bool(const Relation &)> &visit) const;
  std::span<const LogSite> log_sites() const;
  void logs(Query query, std::uint8_t severity,
            const std::function<bool(const LogRecord &)> &visit) const;
  std::string format_log(const LogRecord &record) const;
  Ending ending() const;
  void clear_cache() const;

private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};
void recover(const std::string &input, const std::string &output);
} // namespace vtr
