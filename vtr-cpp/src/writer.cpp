#include "format.hpp"
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <limits>
#include <unordered_set>

namespace vtr {
using namespace detail;
namespace {
struct WaveColumn {
  SignalKind kind;
  bool event = false, emitted = false;
  Bytes last, frame, headers, values;
  std::uint32_t last_index = 0;
  std::uint8_t last_code = 255;
  std::uint64_t last_scalar = 0;
  std::size_t count = 0, compact_count = 0;
};
struct ClockState {
  Id generator;
  std::uint64_t tx = 0;
  Time begin = 0, period = 0;
  std::optional<Time> last;
};
std::size_t attribute_bytes(std::span<const Attribute> attrs) {
  std::size_t size = attrs.size() * 16;
  for (const auto &a : attrs)
    size += a.value.payload.size();
  return size;
}
Bytes dictionary(View values, std::size_t width) {
  Bytes dict, codes;
  std::unordered_map<std::string_view, std::uint8_t> ids;
  for (std::size_t i = 0; i < values.size(); i += width) {
    std::string_view key(reinterpret_cast<const char *>(values.data() + i),
                         width);
    auto it = ids.find(key);
    if (it == ids.end()) {
      if (ids.size() == 256) {
        Bytes out{0};
        append(out, values);
        return out;
      }
      auto id = static_cast<std::uint8_t>(ids.size());
      ids.emplace(key, id);
      append(dict, values.subspan(i, width));
      codes.push_back(id);
    } else
      codes.push_back(it->second);
  }
  Bytes out;
  put(out, ids.size());
  append(out, dict);
  append(out, codes);
  return out;
}
std::size_t value_width(const WaveColumn &c) {
  if (c.kind.type == SignalKind::real)
    return 8;
  if (c.kind.type == SignalKind::bits && c.kind.width > 1) {
    if (c.compact_count == c.count)
      return (c.kind.width + 7) / 8;
    if (c.compact_count == 0)
      return c.kind.packed_size();
  }
  return 0;
}
Bytes make_column(const WaveColumn &c, unsigned xform) {
  Bytes out;
  if (!c.count)
    return out;
  if (c.kind.type == SignalKind::bits && c.kind.width == 1)
    return c.headers;
  auto width = value_width(c);
  put(out,
      (c.headers.size() << 2) | (width ? 2 : 0) |
          (width && !c.compact_count && c.kind.type == SignalKind::bits ? 1
                                                                        : 0));
  append(out, c.headers);
  if (!width || !xform)
    append(out, c.values);
  else if (xform == 4)
    append(out, dictionary(c.values, width));
  else
    append(out, transform(c.values, width, xform, false));
  return out;
}
} // namespace
struct Writer::Impl {
  WriterOptions options;
  Meta meta;
  FILE *file = nullptr;
  bool closed = false, meta_written = false, active = true;
  std::uint64_t offset = 32, next_id = 1, wave_records = 0;
  Time now = 0;
  unsigned commit_ticks = 0;
  std::chrono::steady_clock::time_point last_commit =
      std::chrono::steady_clock::now();
  std::vector<Section> directory;
  std::vector<std::string> strings{std::string{}};
  std::unordered_map<std::string, Id> interned{{std::string{}, 0}};
  std::vector<Node> nodes;
  std::vector<WaveColumn> waves;
  std::size_t written_strings = 0, written_nodes = 0, written_signals = 0;
  std::vector<Time> times;
  std::vector<std::pair<Time, bool>> blackout;
  std::vector<std::uint32_t> previous_dirty;
  std::uint32_t block_index = 0;
  std::vector<Transaction> open{1024};
  std::unordered_map<std::uint64_t, Transaction> overflow_open;
  TxColumns tx_columns, relation_columns;
  std::vector<Id> tx_generators;
  std::vector<bool> tx_generator_seen;
  std::uint64_t tx_count = 0, rel_count = 0, prev_tx_id = 0, prev_begin = 0,
                prev_from = 0;
  std::uint64_t min_id = forever, max_id = 0, min_time = forever, max_time = 0;
  std::uint64_t min_from = forever, max_from = 0, min_to = forever, max_to = 0;
  std::size_t tx_bytes = 0;
  std::vector<LogSite> sites;
  std::array<Bytes, 9> log_columns;
  std::vector<Id> log_generators;
  std::unordered_map<Id, Id> log_generator_ids;
  std::vector<std::string> log_dictionary;
  std::unordered_map<std::string, Id> log_text_ids;
  std::uint64_t log_count = 0, log_prev_id = 0, log_prev_time = 0;
  std::uint64_t log_min_id = forever, log_max_id = 0, log_min_time = forever,
                log_max_time = 0;
  std::size_t log_bytes = 0;
  std::vector<ClockState> clocks;

  Impl(const std::string &path, WriterOptions opts) : options(opts) {
    require(opts.group_size && std::has_single_bit(opts.group_size) &&
                opts.group_size <= (1u << 20),
            "invalid group size", 1);
    require(opts.block_records > 0 && opts.tx_block_bytes > 0 &&
                opts.codec <= Codec::zstd,
            "invalid writer options", 1);
    meta.group_size = opts.group_size;
    file = std::fopen(path.c_str(), "wb");
    require(file, "cannot create VTR file", 3);
    try {
      Bytes header(magic.begin(), magic.end());
      fixed<std::uint16_t>(header, 2);
      fixed<std::uint16_t>(header, 0);
      header.resize(32);
      write(header);
    } catch (...) {
      std::fclose(file);
      file = nullptr;
      throw;
    }
  }
  ~Impl() {
    if (file)
      std::fclose(file);
  }
  void alive() const { require(!closed, "writer is closed", 2); }
  void write(View data) {
    require(std::fwrite(data.data(), 1, data.size(), file) == data.size(),
            "VTR write failed", 3);
  }
  void section(std::uint32_t kind, View payload, std::uint64_t aux0 = 0,
               std::uint64_t aux1 = 0, std::uint32_t flags = 0) {
    std::array<View, 1> parts{payload};
    section_parts(kind, parts, aux0, aux1, flags);
  }
  void section_parts(std::uint32_t kind, std::span<const View> parts,
                     std::uint64_t aux0, std::uint64_t aux1,
                     std::uint32_t flags = 0) {
    alive();
    std::uint64_t length = 0;
    std::uint32_t crc = 0;
    for (auto part : parts) {
      length += part.size();
      if (options.checksums && !part.empty())
        crc = checksum(part, crc);
    }
    Bytes header;
    fixed(header, kind);
    fixed(header, flags);
    fixed(header, length);
    fixed(header, crc);
    fixed<std::uint32_t>(header, 0);
    write(header);
    for (auto part : parts)
      if (!part.empty())
        write(part);
    require(std::fflush(file) == 0, "VTR flush failed", 3);
    directory.push_back({kind, flags, offset, length, aux0, aux1});
    offset += 24 + length;
  }
  Id intern(std::string_view value) {
    alive();
    validate_utf8(view(value));
    auto [it, inserted] = interned.try_emplace(std::string(value),
                                               static_cast<Id>(strings.size()));
    if (inserted) {
      require(strings.size() < none, "string limit reached", 1);
      strings.emplace_back(value);
    }
    return it->second;
  }
  void parent(Id p) const {
    require(p == none || (p < nodes.size() && nodes[p].kind == Node::scope),
            "parent must be a scope", 1);
  }
  void key(Id id) const {
    require(id < strings.size(), "unknown string identifier", 1);
  }
  Transaction &tx(std::uint64_t id) {
    alive();
    auto &slot = open[id & (open.size() - 1)];
    if (id && slot.id == id)
      return slot;
    auto overflow = overflow_open.find(id);
    require(overflow != overflow_open.end(), "transaction is not open", 2);
    return overflow->second;
  }
  void declarations() {
    for (auto &a : meta.attrs) {
      key(a.key);
      validate_value(a.value, strings.size());
    }
    validate_attrs(meta.attrs);
    if (written_strings < strings.size()) {
      Bytes raw;
      put(raw, written_strings);
      put(raw, strings.size() - written_strings);
      for (auto i = written_strings; i < strings.size(); ++i)
        blob(raw, strings[i]);
      section(2, compress(raw, options.codec, options.compression_level),
              written_strings, strings.size() - written_strings);
      written_strings = strings.size();
    }
    if (!meta_written) {
      validate_utf8(view(meta.writer));
      validate_utf8(view(meta.date));
      validate_utf8(view(meta.comment));
      Bytes raw;
      put(raw, 1);
      sint(raw, static_cast<std::uint64_t>(meta.timescale));
      sint(raw, static_cast<std::uint64_t>(meta.time_zero));
      raw.push_back(meta.file_type);
      blob(raw, meta.writer);
      blob(raw, meta.date);
      blob(raw, meta.comment);
      put(raw, options.group_size);
      encode_attrs(raw, meta.attrs);
      section(1, raw);
      meta_written = true;
    }
    if (written_nodes < nodes.size()) {
      Bytes raw;
      put(raw, written_nodes);
      put(raw, nodes.size() - written_nodes);
      for (auto i = written_nodes; i < nodes.size(); ++i) {
        auto &n = nodes[i];
        raw.push_back(n.kind);
        put(raw, n.parent == none ? 0 : i - n.parent);
        put(raw, n.name);
        switch (n.kind) {
        case Node::scope:
          put(raw, n.type);
          put(raw, n.aux);
          break;
        case Node::generator:
          put(raw, n.aux);
          break;
        case Node::enum_table:
          put(raw, n.entries.size());
          for (auto [a, b] : n.entries) {
            put(raw, a);
            put(raw, b);
          }
          break;
        case Node::variable: {
          put(raw, n.type);
          raw.push_back(n.direction);
          put(raw, written_signals - n.signal);
          raw.push_back(!n.alias);
          if (!n.alias) {
            auto k = waves[n.signal].kind;
            raw.push_back(k.type == SignalKind::bits   ? (k.states == 2   ? 0
                                                          : k.states == 4 ? 1
                                                                          : 2)
                          : k.type == SignalKind::real ? 3
                                                       : 4);
            if (k.type == SignalKind::bits)
              put(raw, k.width);
            ++written_signals;
          }
          break;
        }
        }
        encode_attrs(raw, n.attrs);
      }
      section(3, compress(raw, options.codec, options.compression_level),
              written_nodes, nodes.size() - written_nodes);
      written_nodes = nodes.size();
    }
  }
  void add_value(Id signal, View full, View compact, bool is_compact) {
    alive();
    require(signal < waves.size(), "unknown signal", 1);
    if (!active)
      return;
    auto &c = waves[signal];
    if (options.dedup && !c.event && c.emitted &&
        std::equal(full.begin(), full.end(), c.last.begin(), c.last.end()))
      return;
    if (times.empty())
      times.push_back(now);
    auto index = static_cast<std::uint32_t>(times.size() - 1),
         dt = index - c.last_index;
    if (c.kind.type == SignalKind::bits && c.kind.width == 1) {
      auto code = full[0];
      auto entry = c.last_code < 2 && code == (c.last_code ^ 1)
                       ? std::uint64_t(dt) << 1
                       : (std::uint64_t(dt) << 5) | (code << 1) | 1;
      put(c.headers, entry);
      c.last_code = code;
    } else {
      put(c.headers, c.kind.type == SignalKind::bits
                         ? (std::uint64_t(dt) << 1) | is_compact
                         : dt);
      if (c.kind.type == SignalKind::varlen)
        blob(c.values, full);
      else
        append(c.values, is_compact ? compact : full);
    }
    c.last.assign(full.begin(), full.end());
    if (c.kind.type == SignalKind::bits && c.kind.states == 2 &&
        c.kind.width <= 64) {
      c.last_scalar = 0;
      for (std::size_t i = 0; i < full.size(); ++i)
        c.last_scalar |= std::uint64_t(full[i]) << (8 * i);
    }
    c.emitted = true;
    c.last_index = index;
    ++c.count;
    c.compact_count += is_compact;
    ++wave_records;
  }
  Bytes run(std::size_t first, std::size_t end, const std::vector<bool> &alias,
            unsigned &selected) {
    Bytes sample;
    for (auto s = first; s < end; ++s)
      if (!alias[s]) {
        auto w = value_width(waves[s]);
        if (!w)
          continue;
        auto &v = waves[s].values;
        auto n = std::min<std::size_t>(v.size() / w, 128);
        append(sample, View(v).first(n * w));
      }
    auto best =
        compress(sample, options.codec, options.compression_level).size();
    selected = 0;
    for (unsigned x = 1; x <= 4 && !sample.empty(); ++x) {
      Bytes candidate;
      for (auto s = first; s < end; ++s)
        if (!alias[s]) {
          auto w = value_width(waves[s]);
          if (!w)
            continue;
          auto &v = waves[s].values;
          auto n = std::min<std::size_t>(v.size() / w, 128);
          auto part = View(v).first(n * w);
          append(candidate,
                 x == 4 ? dictionary(part, w) : transform(part, w, x, false));
        }
      auto size =
          compress(candidate, options.codec, options.compression_level).size();
      if (size * 100 < best * (x == 4 ? 80 : 96)) {
        best = size;
        selected = x;
      }
    }
    std::vector<Bytes> columns;
    Bytes raw;
    for (auto s = first; s < end; ++s) {
      columns.push_back(alias[s] ? Bytes{} : make_column(waves[s], selected));
      put(raw, columns.back().size());
    }
    for (auto &c : columns)
      append(raw, c);
    return compress(raw, options.codec, options.compression_level);
  }
  void flush_waves() {
    if (times.empty())
      return;
    declarations();
    const auto groups = static_cast<std::uint32_t>(
        (waves.size() + options.group_size - 1) / options.group_size);
    previous_dirty.resize(groups, none);
    std::vector<Id> dirty;
    std::vector<Bytes> containers;
    std::vector<Id> alias_of(waves.size(), none);
    std::vector<bool> alias(waves.size(), false);
    std::unordered_map<std::uint64_t, std::vector<Id>> duplicates;
    for (Id s = 0; s < waves.size(); ++s)
      if (waves[s].count) {
        auto &c = waves[s];
        auto hash_bytes = [](const Bytes &b) {
          return std::hash<std::string_view>{}(
              {reinterpret_cast<const char *>(b.data()), b.size()});
        };
        auto hash = hash_bytes(c.headers) ^ std::rotl(hash_bytes(c.values), 17);
        auto &candidates = duplicates[hash];
        for (auto other : candidates)
          if (waves[other].kind == c.kind &&
              waves[other].headers == c.headers &&
              waves[other].values == c.values) {
            alias_of[s] = other;
            alias[s] = true;
            break;
          }
        if (!alias[s])
          candidates.push_back(s);
      }
    for (Id g = 0; g < groups; ++g) {
      auto first = std::size_t(g) * options.group_size,
           end = std::min(first + options.group_size, waves.size());
      if (std::none_of(waves.begin() + first, waves.begin() + end,
                       [](auto &c) { return c.count != 0; }))
        continue;
      dirty.push_back(g);
      Bytes container, frame;
      put(container, end - first);
      std::size_t count = 0;
      for (auto s = first; s < end; ++s)
        count += alias[s];
      put(container, count);
      for (auto s = first; s < end; ++s)
        if (alias[s]) {
          put(container, s - first);
          put(container, alias_of[s]);
        }
      for (auto s = first; s < end; ++s)
        if (waves[s].kind.type == SignalKind::varlen)
          blob(frame, waves[s].frame);
        else
          append(frame, waves[s].frame);
      blob(container,
           compress(frame, options.codec, options.compression_level));
      struct Run {
        std::size_t n;
        unsigned x;
        Bytes bytes;
      };
      std::vector<Run> runs;
      for (auto begin = first; begin < end;) {
        auto stop = begin;
        std::size_t size = 0;
        do {
          size += waves[stop].headers.size() + waves[stop].values.size() + 10;
          ++stop;
        } while (stop < end && stop - begin < 64 && size < 65536);
        unsigned x;
        auto data = run(begin, stop, alias, x);
        runs.push_back({stop - begin, x, std::move(data)});
        begin = stop;
      }
      put(container, runs.size());
      for (auto &r : runs) {
        put(container, r.n);
        put(container, r.x);
        put(container, r.bytes.size());
      }
      for (auto &r : runs)
        append(container, r.bytes);
      containers.push_back(std::move(container));
    }
    Bytes tt;
    Time prev = 0;
    for (auto t : times) {
      put(tt, t - prev);
      prev = t;
    }
    tt = compress(tt, options.codec, options.compression_level);
    Bytes payload;
    fixed(payload, times.front());
    fixed(payload, times.back());
    fixed<std::uint32_t>(payload, times.size());
    fixed<std::uint32_t>(payload, waves.size());
    fixed(payload, options.group_size);
    fixed<std::uint32_t>(payload, dirty.size());
    fixed(payload, groups);
    fixed<std::uint32_t>(payload, tt.size());
    append(payload, tt);
    std::uint64_t at = 0;
    for (std::size_t i = 0; i < dirty.size(); ++i) {
      fixed(payload, dirty[i]);
      fixed<std::uint32_t>(payload, containers[i].size());
      fixed(payload, at);
      at += containers[i].size();
    }
    for (auto previous : previous_dirty)
      fixed(payload, previous);
    std::vector<View> parts{payload};
    for (auto &c : containers)
      parts.emplace_back(c);
    section_parts(4, parts, times.front(), times.back());
    for (auto g : dirty)
      previous_dirty[g] = block_index;
    ++block_index;
    times.clear();
    wave_records = 0;
    for (auto &c : waves) {
      c.headers.clear();
      c.values.clear();
      if (c.kind.type == SignalKind::bits && c.kind.states == 2 &&
          c.kind.width <= 64 && c.emitted) {
        c.last.clear();
        append_scalar(c.last, c.last_scalar, (c.kind.width + 7) / 8);
      }
      c.frame = c.last;
      c.count = c.compact_count = 0;
      c.last_index = 0;
      c.last_code = 255;
    }
  }
  void finish_tx(Transaction &t) {
    for (auto &stage : t.stages)
      if (!stage.end)
        stage.end = std::max(stage.begin, t.end);
    encode_transaction(tx_columns, t, prev_tx_id, prev_begin);
    prev_tx_id = t.id;
    prev_begin = t.begin;
    min_id = std::min(min_id, t.id);
    max_id = std::max(max_id, t.id);
    min_time = std::min(min_time, t.begin);
    max_time = std::max(max_time, t.end);
    tx_generator_seen.resize(nodes.size(), false);
    if (!tx_generator_seen[t.generator]) {
      tx_generator_seen[t.generator] = true;
      tx_generators.push_back(t.generator);
    }
    ++tx_count;
    tx_bytes += 32 + attribute_bytes(t.attrs);
    for (const auto &event : t.events)
      tx_bytes += 24 + attribute_bytes(event.attrs);
    for (const auto &stage : t.stages)
      tx_bytes += 32 + attribute_bytes(stage.attrs);
    t.id = 0;
    t.attrs.clear();
    t.events.clear();
    t.stages.clear();
    if (tx_bytes >= options.tx_block_bytes)
      flush_tx();
  }
  void flush_tx() {
    if (!tx_count && !rel_count)
      return;
    declarations();
    for (unsigned i = 6; i <= 12; ++i)
      append(tx_columns[i], relation_columns[i]);
    for (unsigned i = 21; i < 24; ++i)
      append(tx_columns[i], relation_columns[i]);
    auto packed = compress(pack_columns(tx_columns), options.codec,
                           options.compression_level);
    std::sort(tx_generators.begin(), tx_generators.end());
    Bytes payload;
    for (auto n : {tx_count, rel_count, tx_count ? min_id : 0, max_id,
                   tx_count ? min_time : 0, max_time, rel_count ? min_from : 0,
                   max_from, rel_count ? min_to : 0, max_to})
      fixed(payload, n);
    fixed<std::uint32_t>(payload, tx_generators.size());
    fixed<std::uint32_t>(payload, packed.size());
    for (auto g : tx_generators)
      fixed(payload, g);
    append(payload, packed);
    section(5, payload, tx_count ? min_time : 0, max_time);
    for (auto &c : tx_columns)
      c.clear();
    for (auto &c : relation_columns)
      c.clear();
    for (auto g : tx_generators)
      tx_generator_seen[g] = false;
    tx_generators.clear();
    tx_count = rel_count = prev_tx_id = prev_begin = prev_from = max_id =
        max_time = max_from = max_to = tx_bytes = 0;
    min_id = min_time = min_from = min_to = forever;
  }
  void flush_logs() {
    if (!log_count)
      return;
    declarations();
    put(log_columns[7], log_dictionary.size());
    for (auto &text : log_dictionary)
      blob(log_columns[7], text);
    auto packed = compress(pack_columns(log_columns), options.codec,
                           options.compression_level);
    Bytes payload;
    for (auto n :
         {log_count, log_min_id, log_max_id, log_min_time, log_max_time})
      fixed(payload, n);
    fixed<std::uint32_t>(payload, log_generators.size());
    fixed<std::uint32_t>(payload, packed.size());
    for (auto g : log_generators)
      fixed(payload, g);
    append(payload, packed);
    section(8, payload, log_min_time, log_max_time, 1);
    for (auto &c : log_columns)
      c.clear();
    log_generators.clear();
    log_generator_ids.clear();
    log_dictionary.clear();
    log_text_ids.clear();
    log_count = log_prev_id = log_prev_time = log_max_id = log_max_time =
        log_bytes = 0;
    log_min_id = log_min_time = forever;
  }
};
Writer::Writer(const std::string &path, WriterOptions options)
    : impl_(std::make_unique<Impl>(path, options)) {}
Writer::~Writer() {
  try {
    if (!impl_->closed)
      close();
  } catch (...) {
  }
}
Meta &Writer::metadata() {
  impl_->alive();
  require(!impl_->meta_written, "metadata has already been written", 2);
  return impl_->meta;
}
Id Writer::intern(std::string_view value) { return impl_->intern(value); }
Id Writer::add_scope(Id parent, std::string_view name, std::uint16_t type,
                     std::string_view component) {
  auto &p = *impl_;
  p.alive();
  p.parent(parent);
  Node n;
  n.parent = parent;
  n.name = intern(name);
  n.type = type;
  n.aux = intern(component);
  p.nodes.push_back(std::move(n));
  return static_cast<Id>(p.nodes.size() - 1);
}
std::pair<Id, Id> Writer::add_var(Id parent, std::string_view name,
                                  SignalKind kind, std::uint16_t type,
                                  std::uint8_t direction) {
  auto &p = *impl_;
  p.alive();
  p.parent(parent);
  validate_kind(kind);
  p.flush_waves();
  Node n;
  n.kind = Node::variable;
  n.parent = parent;
  n.name = intern(name);
  n.type = type;
  n.direction = direction;
  n.signal = static_cast<Id>(p.waves.size());
  WaveColumn c;
  c.kind = kind;
  c.event = type == 0;
  c.last = default_value(kind);
  c.frame = c.last;
  p.waves.push_back(std::move(c));
  p.nodes.push_back(std::move(n));
  return {static_cast<Id>(p.nodes.size() - 1),
          static_cast<Id>(p.waves.size() - 1)};
}
Id Writer::add_alias(Id parent, std::string_view name, Id signal,
                     std::uint16_t type, std::uint8_t direction) {
  auto &p = *impl_;
  p.alive();
  p.parent(parent);
  require(signal < p.waves.size() && p.waves[signal].event == (type == 0),
          "invalid signal alias", 1);
  Node n;
  n.kind = Node::variable;
  n.parent = parent;
  n.name = intern(name);
  n.type = type;
  n.direction = direction;
  n.signal = signal;
  n.alias = true;
  p.nodes.push_back(std::move(n));
  return static_cast<Id>(p.nodes.size() - 1);
}
Id Writer::add_generator(Id parent, std::string_view name,
                         std::string_view kind) {
  auto &p = *impl_;
  p.alive();
  p.parent(parent);
  Node n;
  n.kind = Node::generator;
  n.parent = parent;
  n.name = intern(name);
  n.aux = intern(kind);
  p.nodes.push_back(std::move(n));
  return static_cast<Id>(p.nodes.size() - 1);
}
Id Writer::add_enum_table(Id parent, std::string_view name,
                          std::span<const std::pair<Id, Id>> entries) {
  auto &p = *impl_;
  p.alive();
  p.parent(parent);
  for (auto [a, b] : entries) {
    p.key(a);
    p.key(b);
  }
  Node n;
  n.kind = Node::enum_table;
  n.parent = parent;
  n.name = intern(name);
  n.entries.assign(entries.begin(), entries.end());
  p.nodes.push_back(std::move(n));
  return static_cast<Id>(p.nodes.size() - 1);
}
void Writer::node_attr(Id node, Id key, const Value &value) {
  auto &p = *impl_;
  p.alive();
  require(node >= p.written_nodes && node < p.nodes.size(),
          "node is absent or already written", 2);
  p.key(key);
  auto &attrs = p.nodes[node].attrs;
  unique_attr(attrs, key);
  validate_value(value, p.strings.size());
  attrs.push_back({key, value});
}
void Writer::set_time(Time time) {
  auto &p = *impl_;
  p.alive();
  require(time >= p.now, "waveform time moved backwards", 1);
  if (p.wave_records >= p.options.block_records && time != p.now)
    p.flush_waves();
  p.now = time;
  if (p.times.empty() || p.times.back() != time)
    p.times.push_back(time);
  maybe_commit();
}
Time Writer::time() const noexcept { return impl_->now; }
void Writer::dump(bool active) {
  auto &p = *impl_;
  p.alive();
  if (p.active != active) {
    p.active = active;
    p.blackout.emplace_back(p.now, active);
  }
}
void Writer::emit(Id signal, std::uint64_t value) {
  auto &p = *impl_;
  p.alive();
  require(signal < p.waves.size(), "unknown signal", 1);
  auto &c = p.waves[signal];
  auto k = c.kind;
  require(k.type == SignalKind::bits && k.width <= 64,
          "emit requires a vector of at most 64 bits", 1);
  if (k.width < 64)
    value &= (std::uint64_t{1} << k.width) - 1;
  if (k.states == 2) {
    if (!p.active ||
        (p.options.dedup && !c.event && c.emitted && value == c.last_scalar))
      return;
    if (p.times.empty())
      p.times.push_back(p.now);
    auto index = static_cast<std::uint32_t>(p.times.size() - 1);
    auto delta = std::uint64_t(index - c.last_index);
    if (k.width == 1) {
      auto code = static_cast<std::uint8_t>(value);
      put(c.headers, c.last_code < 2 && code == (c.last_code ^ 1)
                         ? delta << 1
                         : (delta << 5) | (code << 1) | 1);
      c.last_code = code;
    } else {
      put(c.headers, (delta << 1) | 1);
      append_scalar(c.values, value, c.last.size());
    }
    c.last_scalar = value;
    c.emitted = true;
    c.last_index = index;
    ++c.count;
    ++c.compact_count;
    ++p.wave_records;
    return;
  }
  Bytes compact;
  append_scalar(compact, value, (k.width + 7) / 8);
  auto full = repack(compact, k.width, 2, k.states);
  p.add_value(signal, full, compact, true);
}
void Writer::emit_bit(Id signal, std::uint8_t code) {
  if (signal < impl_->waves.size() && impl_->waves[signal].kind.states == 2) {
    require(impl_->waves[signal].kind.width == 1 && code < 2,
            "invalid bit value", 1);
    emit(signal, code);
    return;
  }
  auto &p = *impl_;
  require(signal < p.waves.size(), "unknown signal", 1);
  auto k = p.waves[signal].kind;
  require(k.type == SignalKind::bits && k.width == 1 && code < k.states,
          "invalid bit value", 1);
  p.add_value(signal, View(&code, 1), View(&code, 1), code < 2);
}
void Writer::emit_real(Id signal, double value) {
  auto &p = *impl_;
  require(signal < p.waves.size() &&
              p.waves[signal].kind.type == SignalKind::real,
          "signal is not real", 1);
  Bytes b;
  fixed(b, std::bit_cast<std::uint64_t>(value));
  p.add_value(signal, b, {}, false);
}
void Writer::emit_varlen(Id signal, std::span<const std::uint8_t> value) {
  auto &p = *impl_;
  require(signal < p.waves.size() &&
              p.waves[signal].kind.type == SignalKind::varlen,
          "signal is not variable length", 1);
  p.add_value(signal, value, {}, false);
}
void Writer::emit_packed(Id signal, std::uint8_t states,
                         std::span<const std::uint8_t> value) {
  auto &p = *impl_;
  require(signal < p.waves.size() &&
              p.waves[signal].kind.type == SignalKind::bits,
          "signal is not a vector", 1);
  auto k = p.waves[signal].kind;
  auto full = repack(value, k.width, states, k.states);
  if (k.states == 2 && k.width <= 64) {
    std::uint64_t scalar = 0;
    for (std::size_t i = 0; i < full.size(); ++i)
      scalar |= std::uint64_t(full[i]) << (8 * i);
    emit(signal, scalar);
    return;
  }

  bool compact = true;
  auto stride = k.states == 2 ? 1u : k.states == 4 ? 2u : 4u;
  for (std::size_t i = 0; i < k.width; ++i)
    if (((full[i * stride / 8] >> ((i * stride) % 8)) & ((1u << stride) - 1)) >
        1) {
      compact = false;
      break;
    }
  if (compact) {
    auto packed = repack(full, k.width, k.states, 2);
    p.add_value(signal, full, packed, true);
  } else
    p.add_value(signal, full, {}, false);
}
void Writer::emit_words(Id signal, std::span<const std::uint32_t> words) {
  auto &p = *impl_;
  require(signal < p.waves.size(), "unknown signal", 1);
  auto k = p.waves[signal].kind;
  require(k.type == SignalKind::bits &&
              words.size() >= (std::size_t(k.width) + 31) / 32,
          "insufficient vector words", 1);
  Bytes bytes((std::size_t(k.width) + 7) / 8);
  for (std::size_t i = 0; i < bytes.size(); ++i)
    bytes[i] = static_cast<std::uint8_t>(words[i / 4] >> ((i % 4) * 8));
  if (k.width % 8)
    bytes.back() &= (1u << (k.width % 8)) - 1;
  emit_packed(signal, 2, bytes);
}
void Writer::emit_logic(Id signal, std::string_view value) {
  auto &p = *impl_;
  require(signal < p.waves.size(), "unknown signal", 1);
  auto k = p.waves[signal].kind;
  require(k.type == SignalKind::bits && value.size() == k.width,
          "logic string width mismatch", 1);
  Bytes bytes((std::size_t(k.width) + 1) / 2);
  for (std::size_t i = 0; i < value.size(); ++i) {
    auto c = value[value.size() - 1 - i];
    if (c >= 'A' && c <= 'Z')
      c += 32;
    auto code = std::string_view("01xzuwlh-").find(c);
    require(code != std::string_view::npos, "invalid logic character", 1);
    bytes[i / 2] |= code << ((i % 2) * 4);
  }
  emit_packed(signal, 9, bytes);
}
std::uint64_t Writer::begin_tx(Id generator, Time begin) {
  auto &p = *impl_;
  p.alive();
  require(generator < p.nodes.size() &&
              p.nodes[generator].kind == Node::generator,
          "unknown generator", 1);
  require(p.next_id != forever, "transaction id space exhausted", 1);
  auto id = p.next_id;
  auto &slot = p.open[id & (p.open.size() - 1)];
  auto &t = slot.id ? p.overflow_open[id] : slot;
  t.id = id;
  t.generator = generator;
  t.begin = begin;
  t.end = begin;
  t.parent = 0;
  t.status = t.kind = 0;
  ++p.next_id;
  return id;
}
void Writer::tx_attr(std::uint64_t id, Id key, std::uint64_t value) {
  auto &p = *impl_;
  p.key(key);
  auto &attrs = p.tx(id).attrs;
  unique_attr(attrs, key);
  attrs.push_back({key, Value(value)});
}
void Writer::tx_attr(std::uint64_t id, Id key, const Value &value) {
  auto &p = *impl_;
  p.key(key);
  auto &attrs = p.tx(id).attrs;
  unique_attr(attrs, key);
  validate_value(value, p.strings.size());
  attrs.push_back({key, value});
}
void Writer::tx_parent(std::uint64_t id, std::uint64_t parent) {
  impl_->tx(id).parent = parent;
}
void Writer::tx_kind(std::uint64_t id, std::uint8_t kind) {
  require(kind < 16, "transaction kind exceeds four bits", 1);
  impl_->tx(id).kind = kind;
}
void Writer::tx_event(std::uint64_t id, Time time, Id name,
                      std::span<const Attribute> attrs) {
  impl_->key(name);
  validate_attrs(attrs);
  for (auto &attr : attrs) {
    impl_->key(attr.key);
    validate_value(attr.value, impl_->strings.size());
  }
  impl_->tx(id).events.push_back(
      {time, name, Attributes(attrs.begin(), attrs.end())});
}
void Writer::tx_stage(std::uint64_t id, Id name, Id lane, Time begin,
                      std::optional<Time> end,
                      std::span<const Attribute> attrs) {
  impl_->key(name);
  impl_->key(lane);
  validate_attrs(attrs);
  for (auto &attr : attrs) {
    impl_->key(attr.key);
    validate_value(attr.value, impl_->strings.size());
  }
  auto &t = impl_->tx(id);
  if (!end)
    for (auto &s : t.stages)
      if (s.lane == lane && !s.end)
        s.end = std::max(s.begin, begin);
  t.stages.push_back(
      {name, lane, begin,
       end ? std::optional<Time>(std::max(begin, *end)) : std::nullopt,
       Attributes(attrs.begin(), attrs.end())});
}
void Writer::tx_stage_end(std::uint64_t id, Id name, Id lane, Time end) {
  auto &stages = impl_->tx(id).stages;
  auto it = std::find_if(stages.rbegin(), stages.rend(), [&](auto &s) {
    return s.name == name && s.lane == lane && !s.end;
  });
  require(it != stages.rend(), "stage is not open", 2);
  it->end = std::max(it->begin, end);
}
void Writer::tx_stage_attr(std::uint64_t id, Id key, const Value &value) {
  auto &stages = impl_->tx(id).stages;
  require(!stages.empty(), "transaction has no stage", 2);
  impl_->key(key);
  auto &attrs = stages.back().attrs;
  unique_attr(attrs, key);
  validate_value(value, impl_->strings.size());
  attrs.push_back({key, value});
}
void Writer::end_tx(std::uint64_t id, Time end, std::uint8_t status) {
  require(status < 16, "transaction status exceeds four bits", 1);
  auto &t = impl_->tx(id);
  t.end = std::max(t.begin, end);
  t.status = status;
  impl_->finish_tx(t);
  impl_->overflow_open.erase(id);
  maybe_commit();
}
void Writer::relate(Id kind, std::uint64_t from, std::uint64_t to,
                    std::span<const Attribute> attrs) {
  auto &p = *impl_;
  p.alive();
  p.key(kind);
  validate_attrs(attrs);
  for (auto &attr : attrs) {
    impl_->key(attr.key);
    validate_value(attr.value, impl_->strings.size());
  }
  auto &c = p.relation_columns;
  put(c[21], kind);
  sint(c[22], from - p.prev_from);
  sint(c[23], to - from);
  column_attrs(c, attrs);
  p.prev_from = from;
  ++p.rel_count;
  p.min_from = std::min(p.min_from, from);
  p.max_from = std::max(p.max_from, from);
  p.min_to = std::min(p.min_to, to);
  p.max_to = std::max(p.max_to, to);
  p.tx_bytes += 24 + attribute_bytes(attrs);
  if (p.tx_bytes >= p.options.tx_block_bytes)
    p.flush_tx();
}
Id Writer::add_log_site(Id parent, std::uint8_t severity,
                        std::string_view format, std::span<const Tag> types,
                        std::span<const std::string_view> names,
                        std::string_view file, std::uint32_t line,
                        std::string_view function) {
  require(names.empty() || names.size() == types.size(),
          "log argument name count mismatch", 1);
  for (auto t : types)
    require(t == Tag::boolean || t == Tag::i64 || t == Tag::u64 ||
                t == Tag::f64 || t == Tag::str || t == Tag::bytes ||
                t == Tag::time || t == Tag::pointer || t == Tag::text,
            "invalid log argument type", 1);
  LogSite site;
  site.node = add_generator(parent, format, "LOG");
  site.scope = parent;
  site.format = intern(format);
  site.severity = severity;
  site.types.assign(types.begin(), types.end());
  site.file = intern(file);
  site.function = intern(function);
  site.line = line;
  std::vector<Value> tags, name_values;
  for (std::size_t i = 0; i < types.size(); ++i) {
    tags.emplace_back(std::uint64_t(types[i]));
    site.names.push_back(
        intern(names.empty() ? std::to_string(i) : std::string(names[i])));
    name_values.push_back(Value::string(site.names.back()));
  }
  node_attr(site.node, intern("log.args"), Value::list(tags));
  node_attr(site.node, intern("log.names"), Value::list(name_values));
  node_attr(site.node, intern("log.severity"), Value(std::uint64_t(severity)));
  if (!file.empty())
    node_attr(site.node, intern("log.file"), Value::string(site.file));
  if (line)
    node_attr(site.node, intern("log.line"), Value(std::uint64_t(line)));
  if (!function.empty())
    node_attr(site.node, intern("log.func"), Value::string(site.function));
  impl_->sites.push_back(std::move(site));
  return static_cast<Id>(impl_->sites.size() - 1);
}
Id Writer::log_site_node(Id site) const {
  require(site < impl_->sites.size(), "unknown log site", 1);
  return impl_->sites[site].node;
}
std::uint64_t Writer::log(Id site, Time time, std::span<const Value> args,
                          std::uint64_t parent) {
  auto &p = *impl_;
  p.alive();
  require(site < p.sites.size(), "unknown log site", 1);
  auto &s = p.sites[site];
  require(args.size() == s.types.size(), "log argument count mismatch", 1);
  for (std::size_t i = 0; i < args.size(); ++i)
    require(args[i].tag == s.types[i], "log argument type mismatch", 1);
  auto id = p.next_id++;
  auto &c = p.log_columns;
  auto [it, inserted] = p.log_generator_ids.try_emplace(
      s.node, static_cast<Id>(p.log_generators.size()));
  if (inserted)
    p.log_generators.push_back(s.node);
  put(c[0], it->second);
  sint(c[1], time - p.log_prev_time);
  sint(c[2], id - p.log_prev_id);
  put(c[3], parent ? (zig(id - parent) << 1) | 1 : 0);
  for (auto &v : args) {
    if (v.tag == Tag::text) {
      Cursor r(v.payload);
      auto text = r.string();
      r.done();
      auto [entry, new_text] = p.log_text_ids.try_emplace(
          text, static_cast<Id>(p.log_dictionary.size()));
      if (new_text)
        p.log_dictionary.push_back(std::move(text));
      put(c[6], entry->second);
    } else
      encode_payload(c[v.tag == Tag::f64                            ? 5
                       : (v.tag == Tag::str || v.tag == Tag::bytes) ? 8
                                                                    : 4],
                     v);
    p.log_bytes += 16 + v.payload.size();
  }
  p.log_prev_time = time;
  p.log_prev_id = id;
  p.log_min_time = std::min(p.log_min_time, time);
  p.log_max_time = std::max(p.log_max_time, time);
  p.log_min_id = std::min(p.log_min_id, id);
  p.log_max_id = std::max(p.log_max_id, id);
  ++p.log_count;
  p.log_bytes += 32;
  if (p.log_bytes >= p.options.tx_block_bytes)
    p.flush_logs();
  maybe_commit();
  return id;
}
std::uint64_t Writer::log_raw(Id site, Time time,
                              std::span<const std::uint8_t> args,
                              std::uint64_t parent) {
  require(site < impl_->sites.size(), "unknown log site", 1);
  Cursor r(args);
  std::vector<Value> values;
  for (auto tag : impl_->sites[site].types)
    values.push_back(decode_value(r, tag));
  r.done();
  return log(site, time, values, parent);
}
Id Writer::add_clock(Id parent, std::string_view name) {
  auto gen = add_generator(parent, name, "CLOCK");
  impl_->clocks.push_back({gen, 0, 0, 0, {}});
  return static_cast<Id>(impl_->clocks.size() - 1);
}
Id Writer::clock_generator(Id clock) const {
  require(clock < impl_->clocks.size(), "unknown clock", 1);
  return impl_->clocks[clock].generator;
}
void Writer::clock_run(Id clock, Time first, Time period) {
  require(period > 0, "clock period must be positive", 1);
  auto &p = *impl_;
  require(clock < p.clocks.size(), "unknown clock", 1);
  auto &c = p.clocks[clock];
  require(!c.last || first > *c.last, "clock stretches overlap", 1);
  require(!c.tx, "clock is already running", 2);
  c.tx = begin_tx(c.generator, first);
  c.begin = first;
  c.period = period;
  tx_attr(c.tx, intern("vtr.period"), Value::time(period));
}
void Writer::clock_stop(Id clock, Time time) {
  auto &p = *impl_;
  require(clock < p.clocks.size(), "unknown clock", 1);
  auto &c = p.clocks[clock];
  if (!c.tx)
    return;
  require(time >= c.begin, "clock stop precedes first edge", 1);
  auto last = c.begin + (time - c.begin) / c.period * c.period;
  end_tx(c.tx, last);
  c.tx = 0;
  c.last = last;
}
void Writer::maybe_commit() {
  auto &p = *impl_;
  if (!p.options.commit_interval_ms || (++p.commit_ticks & 63))
    return;
  auto now = std::chrono::steady_clock::now();
  if (now - p.last_commit >=
      std::chrono::milliseconds(p.options.commit_interval_ms)) {
    flush();
    p.last_commit = now;
  }
}
void Writer::flush() {
  auto &p = *impl_;
  p.alive();
  p.declarations();
  p.flush_waves();
  p.flush_tx();
  p.flush_logs();
  if (!p.blackout.empty()) {
    Bytes b;
    put(b, p.blackout.size());
    Time prev = 0;
    for (auto [t, active] : p.blackout) {
      b.push_back(active);
      put(b, t - prev);
      prev = t;
    }
    p.section(6, b);
    p.blackout.clear();
  }
}
void Writer::close(Ending ending) {
  auto &p = *impl_;
  if (p.closed)
    return;
  for (auto &c : p.clocks)
    if (c.tx) {
      auto last =
          c.begin + (std::max(c.begin, p.now) - c.begin) / c.period * c.period;
      end_tx(c.tx, last, 4);
      c.tx = 0;
    }
  std::vector<std::uint64_t> ids;
  for (auto &t : p.open)
    if (t.id)
      ids.push_back(t.id);
  for (auto &[id, t] : p.overflow_open)
    ids.push_back(id);
  std::sort(ids.begin(), ids.end());
  for (auto id : ids)
    end_tx(id, p.now, 4);
  if (ending.kind != Ending::closed) {
    Id scope = add_scope(none, "vtr.run", 64);
    std::string_view fmt;
    std::vector<std::string_view> names;
    std::vector<Value> args;
    switch (ending.kind) {
    case Ending::stopped:
      fmt = "run stopped by signal {}";
      names = {"signal"};
      args = {Value(std::int64_t(ending.signal))};
      break;
    case Ending::exited:
      fmt = "run exited with status {}";
      names = {"status"};
      args = {Value(std::int64_t(ending.status))};
      break;
    case Ending::crashed: {
      fmt = "run crashed by signal {} (code {}, address {}, thread {}, sealed "
            "{})";
      names = {"signal", "code", "address", "thread", "sealed"};
      Value address(ending.address);
      address.tag = Tag::pointer;
      args = {Value(std::int64_t(ending.signal)),
              Value(std::int64_t(ending.code)), address, Value(ending.thread),
              Value(ending.sealed)};
      break;
    }
    case Ending::recovered:
      fmt = "run recovered by scanning, {} bytes dropped";
      names = {"dropped"};
      args = {Value(ending.dropped)};
      break;
    case Ending::poisoned:
      fmt = "run sealed after a panic in the writer";
      break;
    default:
      break;
    }
    std::vector<Tag> types;
    for (auto &a : args)
      types.push_back(a.tag);
    auto site = add_log_site(scope, 5, fmt, types, names);
    log(site, p.now, args);
  }
  flush();
  Bytes dir;
  fixed<std::uint64_t>(dir, p.directory.size());
  for (auto &s : p.directory) {
    fixed(dir, s.kind);
    fixed(dir, s.flags);
    fixed(dir, s.offset);
    fixed(dir, s.length);
    fixed(dir, s.aux0);
    fixed(dir, s.aux1);
  }
  auto directory_offset = p.offset;
  p.section(7, dir);
  Bytes end;
  fixed(end, directory_offset);
  fixed(end, p.offset + 24);
  append(end, trailer);
  p.write(end);
  auto result = std::fclose(p.file);
  p.file = nullptr;
  p.closed = true;
  require(result == 0, "VTR close failed", 3);
}
} // namespace vtr
