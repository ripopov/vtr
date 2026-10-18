#include "format.hpp"
#include <fcntl.h>
#include <filesystem>
#include <format>
#include <fstream>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

namespace vtr {
using namespace detail;
namespace {
struct Mapping {
  const std::uint8_t *address = nullptr;
  std::size_t size = 0;
  explicit Mapping(const std::string &path) {
    int fd = ::open(path.c_str(), O_RDONLY);
    require(fd >= 0, "cannot open VTR file", 3);
    struct stat st{};
    if (fstat(fd, &st) != 0 || st.st_size < 32) {
      ::close(fd);
      throw Error("invalid VTR file size", 4);
    }
    size = static_cast<std::size_t>(st.st_size);
    auto p = mmap(nullptr, size, PROT_READ, MAP_PRIVATE, fd, 0);
    ::close(fd);
    require(p != MAP_FAILED, "cannot map VTR file", 3);
    address = static_cast<const std::uint8_t *>(p);
  }
  ~Mapping() {
    if (address)
      munmap(const_cast<std::uint8_t *>(address), size);
  }
  View slice(std::uint64_t offset, std::uint64_t len) const {
    require(offset <= size && len <= size - offset, "section exceeds file");
    return {address + offset, static_cast<std::size_t>(len)};
  }
};
struct SignalBlock {
  Section section;
  Time begin, end;
  std::uint32_t n_times, n_signals, group_size, n_dirty, n_groups, tt_len;
};
struct RecordBlock {
  Section section;
  std::uint64_t count, relations, min_id, max_id, begin, end;
  std::vector<Id> generators;
  std::size_t blob_offset, blob_len;
};
struct Group {
  std::size_t first = 0, count = 0;
  std::unordered_map<Id, Id> aliases;
  Bytes frame;
  struct Run {
    std::size_t first, count;
    unsigned transform;
    View bytes;
  };
  std::vector<Run> runs;
};
std::vector<Value> list_values(const Value &v) {
  require(v.tag == Tag::list, "expected list value");
  Cursor r(v.payload);
  auto count = r.var();
  require(count <= r.data.size(), "invalid list count");
  std::vector<Value> result;
  for (std::uint64_t i = 0; i < count; ++i)
    result.push_back(decode_value(r));
  r.done();
  return result;
}
} // namespace
struct Reader::Impl {
  Mapping file;
  bool verify_crc;
  Meta meta;
  ReaderStats stats;
  std::vector<Section> sections;
  std::vector<std::string> strings;
  std::vector<Node> nodes;
  std::vector<SignalKind> signals;
  std::vector<Id> signal_nodes;
  std::vector<SignalBlock> waves;
  std::vector<RecordBlock> tx_blocks, log_blocks;
  std::vector<std::pair<Time, bool>> blackout;
  std::vector<LogSite> sites;
  std::unordered_map<Id, Id> site_by_node;
  mutable std::mutex cache_mutex;
  mutable std::unordered_map<Id, std::shared_ptr<const SignalData>>
      history_cache;
  mutable std::vector<Time> all_times;
  mutable bool times_loaded = false;
  explicit Impl(const std::string &path, ReaderOptions options)
      : file(path), verify_crc(options.verify_crc) {
    require(std::equal(magic.begin(), magic.end(), file.address),
            "invalid VTR magic");
    Cursor header(file.slice(8, 24));
    require(header.fixed<std::uint16_t>() == 2 &&
                header.fixed<std::uint16_t>() == 0,
            "unsupported VTR version", 5);
    for (auto b : header.data)
      require(b == 0, "unsupported VTR header flags");
    bool directory = false;
    if (file.size >= 56 && std::equal(trailer.begin(), trailer.end(),
                                      file.address + file.size - 8)) {
      auto off = load<std::uint64_t>(file.slice(file.size - 24, 8));
      auto len = load<std::uint64_t>(file.slice(file.size - 16, 8));
      if (len == file.size && off >= 32 && off <= file.size - 48) {
        auto d = section_at(off);
        require(d.kind == 7 && d.length == file.size - off - 48,
                "invalid directory bounds");
        Cursor r(payload(d, true));
        auto n = r.fixed<std::uint64_t>();
        require(n <= r.data.size() / 40, "invalid directory entry count");
        std::uint64_t previous = 32;
        for (std::uint64_t i = 0; i < n; ++i) {
          Section s;
          s.kind = r.fixed<std::uint32_t>();
          s.flags = r.fixed<std::uint32_t>();
          s.offset = r.fixed<std::uint64_t>();
          s.length = r.fixed<std::uint64_t>();
          s.aux0 = r.fixed<std::uint64_t>();
          s.aux1 = r.fixed<std::uint64_t>();
          require(s.offset == previous && s.offset < off &&
                      off - s.offset >= 24 && s.length <= off - s.offset - 24,
                  "invalid directory entry");
          auto actual = section_at(s.offset);
          require(s.kind == actual.kind && s.flags == actual.flags &&
                      s.length == actual.length,
                  "directory/header mismatch");
          previous = s.offset + 24 + s.length;
          sections.push_back(s);
        }
        r.done();
        require(previous == off, "directory omits sections");
        directory = true;
      }
    }
    if (!directory) {
      stats.recovered = true;
      std::uint64_t off = 32;
      while (off <= file.size && file.size - off >= 24) {
        try {
          auto s = section_at(off);
          if (!s.kind || s.kind == 7)
            break;
          payload(s, true);
          sections.push_back(s);
          off += 24 + s.length;
        } catch (const Error &) {
          break;
        }
      }
      stats.recovered_dropped = file.size - off;
    }
    bool have_meta = false;
    for (auto &s : sections) {
      switch (s.kind) {
      case 1: {
        require(!have_meta, "duplicate META section");
        Cursor r(payload(s));
        require(r.var() == 1, "unknown META layout");
        auto exp = r.signed_var();
        require(exp >= -128 && exp <= 127, "invalid timescale");
        meta.timescale = static_cast<std::int8_t>(exp);
        meta.time_zero = r.signed_var();
        meta.file_type = r.byte();
        meta.writer = r.string();
        meta.date = r.string();
        meta.comment = r.string();
        meta.group_size = r.id();
        require(meta.group_size && std::has_single_bit(meta.group_size) &&
                    meta.group_size <= (1u << 20),
                "invalid signal group size");
        meta.attrs = decode_attrs(r);
        r.done();
        have_meta = true;
        break;
      }
      case 2: {
        auto raw = decompress(payload(s));
        Cursor r(raw);
        require(r.id() == strings.size(), "noncontiguous string identifiers");
        auto n = r.var();
        require(n <= r.data.size(), "invalid string count");
        if (strings.size() + n > strings.capacity())
          strings.reserve(std::max(strings.size() + n, strings.capacity() * 2));
        for (std::uint64_t i = 0; i < n; ++i) {
          auto text = r.blob();
          validate_utf8(text);
          strings.emplace_back(reinterpret_cast<const char *>(text.data()),
                               text.size());
        }
        r.done();
        require(!strings.empty() && strings[0].empty(),
                "string zero is not empty");
        break;
      }
      case 3:
        require(have_meta, "hierarchy precedes metadata");
        read_nodes(s);
        break;
      case 4: {
        require(have_meta, "signal block precedes metadata");
        Cursor r(unchecked_payload(s));
        SignalBlock b{s,
                      r.fixed<std::uint64_t>(),
                      r.fixed<std::uint64_t>(),
                      r.fixed<std::uint32_t>(),
                      r.fixed<std::uint32_t>(),
                      r.fixed<std::uint32_t>(),
                      r.fixed<std::uint32_t>(),
                      r.fixed<std::uint32_t>(),
                      r.fixed<std::uint32_t>()};
        require(b.begin <= b.end && b.n_times > 0 &&
                    b.n_signals <= signals.size() &&
                    b.group_size == meta.group_size &&
                    b.n_groups ==
                        (std::uint64_t(b.n_signals) + b.group_size - 1) /
                            b.group_size &&
                    b.n_dirty <= b.n_groups,
                "invalid signal block header");
        require(waves.empty() || waves.back().end <= b.begin,
                "overlapping signal blocks");
        require(b.tt_len <= r.data.size(), "invalid time table length");
        waves.push_back(b);
        range(b.begin, b.end);
        break;
      }
      case 5:
      case 8: {
        require(have_meta, "record block precedes metadata");
        auto b = record_block(s);
        range(b.begin, b.end);
        if (s.kind == 5) {
          stats.transactions += b.count;
          stats.relations += b.relations;
          tx_blocks.push_back(std::move(b));
        } else {
          stats.logs += b.count;
          log_blocks.push_back(std::move(b));
        }
        break;
      }
      case 6: {
        Cursor r(payload(s));
        auto n = r.var();
        require(n <= r.data.size() / 2, "invalid blackout count");
        Time t = 0;
        for (std::uint64_t i = 0; i < n; ++i) {
          auto active = r.byte();
          require(active <= 1, "invalid blackout state");
          auto dt = r.var();
          require(dt <= forever - t, "blackout time overflow");
          t += dt;
          blackout.emplace_back(t, active != 0);
        }
        r.done();
        break;
      }
      default:
        require(s.flags & 1, "unknown required section");
        break;
      }
    }
    require(have_meta && !strings.empty(), "missing metadata or strings");
    stats.signal_blocks = static_cast<std::uint32_t>(waves.size());
    stats.tx_blocks = static_cast<std::uint32_t>(tx_blocks.size());
    stats.log_blocks = static_cast<std::uint32_t>(log_blocks.size());
    read_sites();
  }
  Section section_at(std::uint64_t offset) const {
    Cursor r(file.slice(offset, 24));
    Section s;
    s.offset = offset;
    s.kind = r.fixed<std::uint32_t>();
    s.flags = r.fixed<std::uint32_t>();
    s.length = r.fixed<std::uint64_t>();
    r.fixed<std::uint32_t>();
    require(r.fixed<std::uint32_t>() == 0, "nonzero section reserved field");
    file.slice(offset + 24, s.length);
    return s;
  }
  View unchecked_payload(const Section &s) const {
    return file.slice(s.offset + 24, s.length);
  }
  View payload(const Section &s, bool force_crc = false) const {
    auto p = unchecked_payload(s);
    auto crc = load<std::uint32_t>(file.slice(s.offset + 16, 4));
    require(!(verify_crc || force_crc) || !crc || checksum(p) == crc,
            "section checksum mismatch", 7);
    return p;
  }
  void range(Time begin, Time end) {
    if (!stats.time_range)
      stats.time_range = {{begin, end}};
    else {
      stats.time_range->first = std::min(stats.time_range->first, begin);
      stats.time_range->second = std::max(stats.time_range->second, end);
    }
  }
  void string_id(Id id) const {
    require(id < strings.size(), "unknown string identifier");
  }
  void attrs_valid(std::span<const Attribute> attrs) const {
    for (auto &a : attrs) {
      string_id(a.key);
      validate_value(a.value, strings.size());
    }
  }
  void read_nodes(const Section &section) {
    auto raw = decompress(payload(section));
    Cursor r(raw);
    require(r.id() == nodes.size(), "noncontiguous node identifiers");
    auto count = r.var();
    require(count <= r.data.size() / 4, "invalid node count");
    if (nodes.size() + count > nodes.capacity())
      nodes.reserve(std::max(nodes.size() + count, nodes.capacity() * 2));
    for (std::uint64_t i = 0; i < count; ++i) {
      Node n;
      n.kind = static_cast<Node::Kind>(r.byte());
      auto parent = r.id();
      require(parent <= nodes.size(), "invalid parent distance");
      n.parent = parent ? static_cast<Id>(nodes.size() - parent) : none;
      require(n.parent == none || nodes[n.parent].kind == Node::scope,
              "parent is not a scope");
      n.name = r.id();
      string_id(n.name);
      switch (n.kind) {
      case Node::scope:
        n.type = static_cast<std::uint16_t>(r.id());
        n.aux = r.id();
        string_id(n.aux);
        break;
      case Node::generator:
        n.aux = r.id();
        string_id(n.aux);
        break;
      case Node::variable: {
        n.type = static_cast<std::uint16_t>(r.id());
        n.direction = r.byte();
        auto back = r.id();
        auto declares = r.byte();
        require(declares <= 1 && back <= signals.size(),
                "invalid signal declaration");
        n.alias = !declares;
        n.signal = static_cast<Id>(signals.size() - back);
        if (declares) {
          require(!back, "new signal has nonzero back reference");
          auto code = r.byte();
          require(code <= 4, "unknown signal kind");
          SignalKind k;
          if (code <= 2) {
            k.type = SignalKind::bits;
            k.states = code == 0 ? 2 : code == 1 ? 4 : 9;
            k.width = r.id();
          } else {
            k.type = code == 3 ? SignalKind::real : SignalKind::varlen;
            k.width = 0;
            k.states = 0;
          }
          validate_kind(k);
          signals.push_back(k);
          signal_nodes.push_back(static_cast<Id>(nodes.size()));
        } else {
          require(back > 0, "alias points to undeclared signal");
          require((nodes[signal_nodes[n.signal]].type == 0) == (n.type == 0),
                  "conflicting event alias");
        }
        break;
      }
      case Node::enum_table: {
        auto n_entries = r.var();
        require(n_entries <= r.data.size() / 2, "invalid enumeration count");
        for (std::uint64_t j = 0; j < n_entries; ++j) {
          auto a = r.id(), b = r.id();
          string_id(a);
          string_id(b);
          n.entries.emplace_back(a, b);
        }
        break;
      }
      default:
        throw Error("unknown node kind", 4);
      }
      n.attrs = decode_attrs(r);
      attrs_valid(n.attrs);
      nodes.push_back(std::move(n));
    }
    r.done();
  }
  RecordBlock record_block(const Section &s) const {
    Cursor r(unchecked_payload(s));
    RecordBlock b{};
    b.section = s;
    b.count = r.fixed<std::uint64_t>();
    if (s.kind == 5)
      b.relations = r.fixed<std::uint64_t>();
    b.min_id = r.fixed<std::uint64_t>();
    b.max_id = r.fixed<std::uint64_t>();
    b.begin = r.fixed<std::uint64_t>();
    b.end = r.fixed<std::uint64_t>();
    if (s.kind == 5)
      r.take(32);
    auto n = r.fixed<std::uint32_t>();
    b.blob_len = r.fixed<std::uint32_t>();
    require(n <= r.data.size() / 4, "invalid generator count");
    for (Id i = 0; i < n; ++i) {
      auto g = r.fixed<Id>();
      require(g < nodes.size() && nodes[g].kind == Node::generator,
              "unknown block generator");
      b.generators.push_back(g);
    }
    require(b.blob_len == r.data.size() && b.begin <= b.end,
            "invalid record block bounds");
    b.blob_offset = s.length - r.data.size();
    return b;
  }
  void read_sites() {
    for (Id id = 0; id < nodes.size(); ++id) {
      auto &n = nodes[id];
      if (n.kind != Node::generator || strings[n.aux] != "LOG")
        continue;
      LogSite s;
      s.node = id;
      s.scope = n.parent;
      s.format = n.name;
      bool args = false;
      for (auto &a : n.attrs) {
        auto &key = strings[a.key];
        auto &v = a.value;
        if (key == "log.args") {
          args = true;
          for (auto &x : list_values(v)) {
            require(x.tag == Tag::u64, "invalid log argument type");
            auto tag = static_cast<Tag>(x.number);
            require(tag == Tag::boolean || tag == Tag::i64 || tag == Tag::u64 ||
                        tag == Tag::f64 || tag == Tag::str ||
                        tag == Tag::bytes || tag == Tag::time ||
                        tag == Tag::pointer || tag == Tag::text,
                    "unsupported log argument type");
            s.types.push_back(tag);
          }
        } else if (key == "log.names") {
          for (auto &x : list_values(v)) {
            require(x.tag == Tag::str, "invalid log argument name");
            string_id(x.literal);
            s.names.push_back(x.literal);
          }
        } else if (key == "log.severity") {
          require(v.tag == Tag::u64 && v.number <= 255, "invalid log severity");
          s.severity = static_cast<std::uint8_t>(v.number);
        } else if (key == "log.file" || key == "log.func") {
          require(v.tag == Tag::str, "invalid log source string");
          (key == "log.file" ? s.file : s.function) = v.literal;
        } else if (key == "log.line") {
          require(v.tag == Tag::u64 && v.number <= UINT32_MAX,
                  "invalid source line");
          s.line = static_cast<Id>(v.number);
        }
      }
      if (!args)
        continue;
      require(s.names.size() == s.types.size(), "log argument names mismatch");
      site_by_node[id] = static_cast<Id>(sites.size());
      sites.push_back(std::move(s));
    }
    for (auto &b : log_blocks)
      for (auto g : b.generators)
        require(site_by_node.contains(g), "log block references invalid site");
  }
  bool in_scope(Id node, Id scope) const {
    if (scope == none)
      return true;
    while (node != none) {
      require(node < nodes.size(), "unknown node", 1);
      if (node == scope)
        return true;
      node = nodes[node].parent;
    }
    return false;
  }
  bool selected(Id node, const Query &q) const {
    return (q.generator == none || q.generator == node) &&
           in_scope(node, q.scope);
  }
  bool selected(const RecordBlock &b, const Query &q) const {
    return b.begin <= q.end && b.end >= q.begin &&
           std::any_of(b.generators.begin(), b.generators.end(),
                       [&](auto g) { return selected(g, q); });
  }
  std::vector<Time> block_times(const SignalBlock &b) const {
    auto raw = decompress(unchecked_payload(b.section).subspan(40, b.tt_len));
    Cursor r(raw);
    require(b.n_times <= raw.size(), "invalid time table count");
    std::vector<Time> result;
    result.reserve(b.n_times);
    Time t = 0;
    for (Id i = 0; i < b.n_times; ++i) {
      auto delta = r.var();
      require(delta <= forever - t && (i == 0 || delta > 0),
              "invalid time table ordering");
      t += delta;
      result.push_back(t);
    }
    r.done();
    require(result.front() == b.begin && result.back() == b.end,
            "time table bounds mismatch");
    return result;
  }
  std::optional<Group> group(const SignalBlock &b, Id group_id) const {
    auto data = unchecked_payload(b.section);
    Cursor r(data.subspan(40 + b.tt_len));
    std::uint32_t length = 0;
    std::uint64_t offset = 0;
    bool found = false;
    Id prev = none;
    for (Id i = 0; i < b.n_dirty; ++i) {
      auto g = r.fixed<Id>(), len = r.fixed<std::uint32_t>();
      auto off = r.fixed<std::uint64_t>();
      require(g < b.n_groups && (i == 0 || g > prev),
              "invalid dirty group index");
      prev = g;
      if (g == group_id) {
        found = true;
        length = len;
        offset = off;
      }
    }
    r.take(std::size_t(b.n_groups) * 4);
    if (!found)
      return {};
    require(offset <= r.data.size() && length <= r.data.size() - offset,
            "invalid group range");
    Cursor c(r.data.subspan(offset, length));
    Group result;
    result.first = std::size_t(group_id) * b.group_size;
    result.count = c.var();
    require(result.count ==
                std::min<std::size_t>(b.group_size, b.n_signals - result.first),
            "invalid group signal count");
    auto aliases = c.var();
    require(aliases <= result.count, "invalid alias count");
    for (std::uint64_t i = 0; i < aliases; ++i) {
      auto local = c.id(), target = c.id();
      require(local < result.count && target < b.n_signals &&
                  target < result.first + local &&
                  signals[target] == signals[result.first + local],
              "invalid dynamic alias");
      require(result.aliases.emplace(local, target).second, "duplicate alias");
    }
    result.frame = decompress(c.blob());
    auto n = c.var();
    require(n <= result.count, "invalid run count");
    struct Descriptor {
      std::size_t count;
      unsigned transform;
      std::size_t size;
    };
    std::vector<Descriptor> descriptors;
    std::size_t total = 0;
    for (std::uint64_t i = 0; i < n; ++i) {
      auto count = c.var(), x = c.var(), size = c.var();
      require(count > 0 && count <= result.count - total && x <= 4,
              "invalid run descriptor");
      descriptors.push_back({static_cast<std::size_t>(count),
                             static_cast<unsigned>(x),
                             static_cast<std::size_t>(size)});
      total += count;
    }
    require(total == result.count, "runs do not cover group");
    total = result.first;
    for (auto d : descriptors) {
      result.runs.push_back({total, d.count, d.transform, c.take(d.size)});
      total += d.count;
    }
    c.done();
    return result;
  }
  Bytes frame_value(const Group &g, Id signal) const {
    Cursor r(g.frame);
    Bytes result;
    for (std::size_t s = g.first; s < g.first + g.count; ++s) {
      auto k = signals[s];
      auto value =
          k.type == SignalKind::varlen ? r.blob() : r.take(k.packed_size());
      if (s == signal)
        result.assign(value.begin(), value.end());
    }
    r.done();
    return result;
  }
  bool column(
      Id signal, View column, unsigned x, const std::vector<Time> &tt,
      Time begin, Time end,
      const std::function<bool(Time, Id, const SignalValue &)> &visit) const {
    if (column.empty())
      return true;
    auto kind = signals[signal];
    Cursor r(column);
    std::uint64_t index = 0;
    if (kind.type == SignalKind::bits && kind.width == 1) {
      std::uint8_t code = 255;
      while (!r.empty()) {
        auto entry = r.var();
        auto delta = entry >> (entry & 1 ? 5 : 1);
        require(delta <= forever - index, "signal time overflow");
        index += delta;
        if (entry & 1)
          code = (entry >> 1) & 15;
        else {
          require(code < 2, "invalid bit toggle");
          code ^= 1;
        }
        require(index < tt.size() && code < kind.states, "invalid bit column");
        auto time = tt[index];
        if (time > end)
          break;
        if (time >= begin && !visit(time, signal, {kind, View(&code, 1)}))
          return false;
      }
      return true;
    }
    auto word = r.var();
    auto header_len = word >> 2;
    Cursor headers(r.take(header_len));
    auto values = r.data;
    bool eligible = word & 2, full = word & 1;
    Bytes transformed;
    if (eligible) {
      require(kind.type != SignalKind::varlen,
              "variable length column cannot be transformed");
      auto width = kind.type == SignalKind::real ? 8
                   : full                        ? kind.packed_size()
                          : (std::size_t(kind.width) + 7) / 8;
      std::size_t count = 0;
      Cursor h = headers;
      while (!h.empty()) {
        h.var();
        ++count;
      }
      require(width && count < (1u << 30) / width,
              "column exceeds format limit");
      if (x == 4) {
        Cursor dict(values);
        auto n = dict.var();
        require(n <= 256, "invalid dictionary size");
        if (!n)
          transformed =
              Bytes(dict.take(count * width).begin(), dict.data.end());
        else {
          auto entries = dict.take(n * width);
          transformed.reserve(count * width);
          for (std::size_t i = 0; i < count; ++i) {
            auto code = dict.byte();
            require(code < n, "invalid dictionary code");
            append(transformed, entries.subspan(code * width, width));
          }
        }
        dict.done();
        values = transformed;
      } else if (x) {
        transformed = transform(values, width, x, true);
        values = transformed;
      }
      require(values.size() == count * width,
              "fixed column value length mismatch");
    }
    Cursor v(values);
    Bytes expanded;
    while (!headers.empty()) {
      auto h = headers.var();
      auto delta = kind.type == SignalKind::bits ? h >> 1 : h;
      require(delta <= forever - index, "signal time overflow");
      index += delta;
      require(index < tt.size(), "signal time index exceeds table");
      View value;
      if (kind.type == SignalKind::varlen)
        value = v.blob();
      else if (kind.type == SignalKind::real)
        value = v.take(8);
      else {
        bool compact = h & 1;
        require(kind.states != 2 || compact, "noncompact two-state value");
        value = v.take(compact ? (std::size_t(kind.width) + 7) / 8
                               : kind.packed_size());
        if (compact && kind.states != 2) {
          expanded = repack(value, kind.width, 2, kind.states);
          value = expanded;
        }
      }
      auto time = tt[index];
      if (time > end)
        break;
      if (time >= begin && !visit(time, signal, {kind, value}))
        return false;
    }
    if (headers.empty())
      v.done();
    return true;
  }
  bool one_signal(
      const SignalBlock &b, const Group &g, Id signal,
      const std::vector<Time> &times, Time begin, Time end,
      const std::function<bool(Time, Id, const SignalValue &)> &visit) const {
    auto alias = g.aliases.find(static_cast<Id>(signal - g.first));
    if (alias != g.aliases.end()) {
      auto target = alias->second;
      auto target_group = group(b, target / b.group_size);
      require(target_group.has_value() &&
                  !target_group->aliases.contains(
                      static_cast<Id>(target - target_group->first)),
              "alias target is absent or aliased");
      return one_signal(
          b, *target_group, target, times, begin, end,
          [&](Time t, Id, SignalValue v) { return visit(t, signal, v); });
    }
    for (auto &run : g.runs)
      if (signal >= run.first && signal < run.first + run.count) {
        auto raw = decompress(run.bytes);
        Cursor r(raw);
        std::vector<std::size_t> sizes;
        for (std::size_t i = 0; i < run.count; ++i)
          sizes.push_back(r.var());
        for (std::size_t i = 0; i < run.count; ++i) {
          auto col = r.take(sizes[i]);
          if (run.first + i == signal)
            return column(signal, col, run.transform, times, begin, end, visit);
        }
      }
    throw Error("missing signal run", 4);
  }
  bool signal_changes(
      Id signal, Time begin, Time end,
      const std::function<bool(Time, Id, const SignalValue &)> &visit) const {
    require(signal < signals.size(), "unknown signal", 1);
    for (auto &b : waves) {
      if (b.begin > end)
        break;
      if (b.end < begin || signal >= b.n_signals)
        continue;
      payload(b.section);
      auto g = group(b, signal / b.group_size);
      if (!g)
        continue;
      auto tt = block_times(b);
      if (!one_signal(b, *g, signal, tt, begin, end, visit))
        return false;
    }
    return true;
  }
  bool
  tx_block(const RecordBlock &b,
           const std::function<bool(const Transaction &)> &visit,
           const std::function<bool(const Relation &)> &relation = {}) const {
    auto raw =
        decompress(payload(b.section).subspan(b.blob_offset, b.blob_len));
    auto c = unpack_columns<24>(raw);
    std::uint64_t id = 0;
    Time begin = 0;
    for (std::uint64_t i = 0; i < b.count; ++i) {
      auto t = decode_transaction(c, id, begin);
      require(t.generator < nodes.size() &&
                  nodes[t.generator].kind == Node::generator,
              "invalid transaction generator");
      require(t.id >= b.min_id && t.id <= b.max_id && t.begin >= b.begin &&
                  t.end <= b.end,
              "transaction exceeds block bounds");
      if (!visit(t))
        return false;
    }
    std::uint64_t from = 0;
    for (std::uint64_t i = 0; i < b.relations; ++i) {
      auto kind = c[21].id();
      from += c[22].signed_bits();
      auto to = from + c[23].signed_bits();
      Relation rel{kind, from, to, read_column_attrs(c)};
      if (relation && !relation(rel))
        return false;
    }
    for (auto &stream : c)
      stream.done();
    return true;
  }
  bool log_block(const RecordBlock &b,
                 const std::function<bool(const LogRecord &)> &visit) const {
    auto raw =
        decompress(payload(b.section).subspan(b.blob_offset, b.blob_len));
    auto c = unpack_columns<9>(raw);
    auto n = c[7].var();
    require(n <= c[7].data.size(), "invalid log dictionary count");
    std::vector<std::string> dict;
    for (std::uint64_t i = 0; i < n; ++i)
      dict.push_back(c[7].string());
    c[7].done();
    std::uint64_t id = 0;
    Time time = 0;
    for (std::uint64_t i = 0; i < b.count; ++i) {
      auto g = c[0].var();
      require(g < b.generators.size(), "invalid log generator index");
      auto site = site_by_node.at(b.generators[g]);
      time += c[1].signed_bits();
      id += c[2].signed_bits();
      auto parent = c[3].var();
      LogRecord record{
          id, parent ? id - unzig(parent >> 1) : 0, time, site, {}};
      require(id >= b.min_id && id <= b.max_id && time >= b.begin &&
                  time <= b.end,
              "log exceeds block bounds");
      record.args.reserve(sites[site].types.size());
      for (auto tag : sites[site].types) {
        if (tag == Tag::text) {
          auto index = c[6].var();
          require(index < dict.size(), "invalid log text index");
          record.args.push_back(Value::text(dict[index]));
        } else
          record.args.push_back(
              decode_value(c[tag == Tag::f64                          ? 5
                             : (tag == Tag::str || tag == Tag::bytes) ? 8
                                                                      : 4],
                           tag));
      }
      if (!visit(record))
        return false;
    }
    for (auto &stream : c)
      stream.done();
    return true;
  }
  Transaction log_tx(const LogRecord &r) const {
    auto &site = sites[r.site];
    Transaction t;
    t.id = r.id;
    t.parent = r.parent;
    t.generator = site.node;
    t.begin = t.end = r.time;
    for (std::size_t i = 0; i < r.args.size(); ++i)
      t.attrs.push_back({site.names[i], r.args[i]});
    return t;
  }
};
Reader::Reader(const std::string &path, ReaderOptions options)
    : impl_(std::make_unique<Impl>(path, options)) {}
Reader::~Reader() = default;
const Meta &Reader::metadata() const { return impl_->meta; }
const ReaderStats &Reader::stats() const { return impl_->stats; }
std::span<const std::string> Reader::strings() const { return impl_->strings; }
std::string_view Reader::string(Id id) const {
  impl_->string_id(id);
  return impl_->strings[id];
}
std::span<const Node> Reader::nodes() const { return impl_->nodes; }
std::span<const SignalKind> Reader::signals() const { return impl_->signals; }
Id Reader::signal_var(Id id) const {
  require(id < impl_->signal_nodes.size(), "unknown signal", 9);
  return impl_->signal_nodes[id];
}
Id Reader::find_node(std::string_view path, char separator) const {
  auto &p = *impl_;
  Id parent = none;
  std::size_t start = 0;
  while (start <= path.size()) {
    auto end = path.find(separator, start);
    auto part =
        path.substr(start, end == std::string_view::npos ? path.size() - start
                                                         : end - start);
    Id found = none;
    for (Id i = 0; i < p.nodes.size(); ++i)
      if (p.nodes[i].parent == parent && p.strings[p.nodes[i].name] == part) {
        found = i;
        break;
      }
    if (found == none)
      return none;
    parent = found;
    if (end == std::string_view::npos)
      return parent;
    start = end + 1;
  }
  return none;
}
Id Reader::find_signal(std::string_view path, char separator) const {
  auto node = find_node(path, separator);
  return node == none || impl_->nodes[node].kind != Node::variable
             ? none
             : impl_->nodes[node].signal;
}
bool Reader::in_scope(Id node, Id scope) const {
  return impl_->in_scope(node, scope);
}
void Reader::changes(
    Id signal, Time begin, Time end,
    const std::function<bool(Time, Id, const SignalValue &)> &visit) const {
  require(bool(visit), "missing change callback", 1);
  impl_->signal_changes(signal, begin, end, visit);
}
void Reader::changes(
    Time begin, Time end,
    const std::function<bool(Time, Id, const SignalValue &)> &visit) const {
  auto &p = *impl_;
  require(bool(visit), "missing change callback", 1);
  for (auto &b : p.waves) {
    if (b.begin > end)
      break;
    if (b.end < begin)
      continue;
    p.payload(b.section);
    auto times = p.block_times(b);
    std::vector<std::optional<Group>> groups;
    groups.reserve(b.n_groups);
    std::unordered_map<Id, std::vector<Id>> aliases;
    for (Id g = 0; g < b.n_groups; ++g) {
      groups.push_back(p.group(b, g));
      if (groups.back())
        for (auto [local, target] : groups.back()->aliases)
          aliases[target].push_back(
              static_cast<Id>(groups.back()->first + local));
    }
    for (auto &[target, ids] : aliases) {
      auto &owner = groups[target / b.group_size];
      require(owner && !owner->aliases.contains(
                           static_cast<Id>(target - owner->first)),
              "alias target is absent or aliased");
    }
    for (auto &group : groups) {
      if (!group)
        continue;
      for (auto &run : group->runs) {
        auto raw = decompress(run.bytes);
        Cursor r(raw);
        std::vector<std::size_t> sizes;
        for (std::size_t i = 0; i < run.count; ++i)
          sizes.push_back(r.var());
        for (std::size_t i = 0; i < run.count; ++i) {
          auto col = r.take(sizes[i]);
          auto id = static_cast<Id>(run.first + i);
          if (group->aliases.contains(static_cast<Id>(id - group->first)))
            continue;
          auto copies = aliases.find(id);
          if (copies == aliases.end()) {
            if (!p.column(id, col, run.transform, times, begin, end, visit))
              return;
          } else if (!p.column(
                         id, col, run.transform, times, begin, end,
                         [&](Time t, Id signal, const SignalValue &value) {
                           if (!visit(t, signal, value))
                             return false;
                           for (auto alias : copies->second)
                             if (!visit(t, alias, value))
                               return false;
                           return true;
                         }))
            return;
        }
        r.done();
      }
    }
  }
}
Bytes Reader::value_at(Id signal, Time time) const {
  auto &p = *impl_;
  require(signal < p.signals.size(), "unknown signal", 1);
  Bytes result = default_value(p.signals[signal]);
  auto it = std::upper_bound(p.waves.begin(), p.waves.end(), time,
                             [](Time t, const auto &b) { return t < b.begin; });
  while (it != p.waves.begin()) {
    --it;
    auto &b = *it;
    if (signal >= b.n_signals)
      return result;
    p.payload(b.section);
    auto g = p.group(b, signal / b.group_size);
    if (!g)
      continue;
    result = p.frame_value(*g, signal);
    auto times = p.block_times(b);
    p.one_signal(b, *g, signal, times, 0, time, [&](Time, Id, SignalValue v) {
      result.assign(v.data.begin(), v.data.end());
      return true;
    });
    break;
  }
  return result;
}
std::shared_ptr<const SignalData> Reader::load_signal(Id signal) const {
  auto &p = *impl_;
  {
    std::lock_guard lock(p.cache_mutex);
    auto found = p.history_cache.find(signal);
    if (found != p.history_cache.end())
      return found->second;
  }
  require(signal < p.signals.size(), "unknown signal", 1);
  auto data = std::make_shared<SignalData>();
  data->kind = p.signals[signal];
  data->initial = default_value(data->kind);
  data->offsets.push_back(0);
  p.signal_changes(signal, 0, forever, [&](Time time, Id, SignalValue value) {
    data->times.push_back(time);
    append(data->values, value.data);
    data->offsets.push_back(data->values.size());
    return true;
  });
  std::lock_guard lock(p.cache_mutex);
  return p.history_cache.emplace(signal, data).first->second;
}
const std::vector<Time> &Reader::time_table() const {
  auto &p = *impl_;
  std::lock_guard lock(p.cache_mutex);
  if (!p.times_loaded) {
    for (auto &b : p.waves) {
      p.payload(b.section);
      for (auto time : p.block_times(b))
        if (p.all_times.empty() || p.all_times.back() != time)
          p.all_times.push_back(time);
    }
    p.times_loaded = true;
  }
  return p.all_times;
}
std::span<const std::pair<Time, bool>> Reader::blackouts() const {
  return impl_->blackout;
}
void Reader::transactions(
    Query q, const std::function<bool(const Transaction &)> &visit) const {
  auto &p = *impl_;
  require(bool(visit), "missing transaction callback", 1);
  for (auto &b : p.tx_blocks)
    if (p.selected(b, q))
      if (!p.tx_block(b, [&](auto &t) {
            return !p.selected(t.generator, q) || t.begin > q.end ||
                   t.end < q.begin || visit(t);
          }))
        return;
  for (auto &b : p.log_blocks)
    if (p.selected(b, q))
      if (!p.log_block(b, [&](auto &r) {
            return !p.selected(p.sites[r.site].node, q) || r.time < q.begin ||
                   r.time > q.end || visit(p.log_tx(r));
          }))
        return;
}
bool Reader::transaction(
    std::uint64_t id,
    const std::function<void(const Transaction &)> &visit) const {
  auto &p = *impl_;
  bool found = false;
  for (auto &b : p.tx_blocks)
    if (id >= b.min_id && id <= b.max_id) {
      p.tx_block(b, [&](auto &t) {
        if (t.id != id)
          return true;
        visit(t);
        found = true;
        return false;
      });
      if (found)
        return true;
    }
  for (auto &b : p.log_blocks)
    if (id >= b.min_id && id <= b.max_id) {
      p.log_block(b, [&](auto &r) {
        if (r.id != id)
          return true;
        visit(p.log_tx(r));
        found = true;
        return false;
      });
      if (found)
        return true;
    }
  return false;
}
void Reader::relations(
    std::uint64_t id, bool incoming,
    const std::function<bool(const Relation &)> &visit) const {
  auto &p = *impl_;
  for (auto &b : p.tx_blocks)
    if (b.relations)
      if (!p.tx_block(
              b, [](auto &) { return true; },
              [&](auto &r) {
                return (incoming ? r.to : r.from) != id || visit(r);
              }))
        return;
}
std::span<const LogSite> Reader::log_sites() const { return impl_->sites; }
void Reader::logs(Query q, std::uint8_t severity,
                  const std::function<bool(const LogRecord &)> &visit) const {
  auto &p = *impl_;
  for (auto &b : p.log_blocks)
    if (p.selected(b, q))
      if (!p.log_block(b, [&](auto &r) {
            auto &s = p.sites[r.site];
            return s.severity < severity || !p.selected(s.node, q) ||
                   r.time < q.begin || r.time > q.end || visit(r);
          }))
        return;
}
std::string Reader::format_log(const LogRecord &record) const {
  auto &p = *impl_;
  require(record.site < p.sites.size(), "unknown log site", 1);
  auto format = string(p.sites[record.site].format);
  std::string out;
  std::size_t arg = 0;
  for (std::size_t i = 0; i < format.size(); ++i) {
    if (i + 1 < format.size() && ((format[i] == '{' && format[i + 1] == '{') ||
                                  (format[i] == '}' && format[i + 1] == '}'))) {
      out += format[i++];
      continue;
    }
    if (format[i] != '{') {
      out += format[i];
      continue;
    }
    auto end = format.find('}', i + 1);
    require(end != std::string_view::npos && arg < record.args.size(),
            "invalid log format", 1);
    auto spec = format.substr(i + 1, end - i - 1);
    auto &v = record.args[arg++];
    const std::string pattern = "{" + std::string(spec) + "}";
    auto format_value = [&](auto &value) {
      return std::vformat(pattern, std::make_format_args(value));
    };
    if (v.tag == Tag::text) {
      Cursor payload(v.payload);
      auto text = payload.string();
      out += format_value(text);
    } else if (v.tag == Tag::str) {
      auto text = string(v.literal);
      out += format_value(text);
    } else if (v.tag == Tag::boolean) {
      auto value = v.number != 0;
      out += format_value(value);
    } else if (v.tag == Tag::i64) {
      auto value = std::bit_cast<std::int64_t>(v.number);
      out += format_value(value);
    } else if (v.tag == Tag::f64) {
      auto value = std::bit_cast<double>(v.number);
      out += format_value(value);
    } else if (v.tag == Tag::pointer) {
      auto value = reinterpret_cast<const void *>(v.number);
      out += format_value(value);
    } else if (v.tag == Tag::bytes) {
      Cursor payload(v.payload);
      std::ostringstream hex;
      for (auto byte : payload.blob())
        hex << std::hex << std::setw(2) << std::setfill('0') << unsigned(byte);
      auto text = hex.str();
      out += format_value(text);
    } else
      out += format_value(v.number);
    i = end;
  }
  return out;
}
Ending Reader::ending() const {
  Ending ending;
  if (stats().recovered) {
    ending.kind = Ending::recovered;
    ending.dropped = stats().recovered_dropped;
  }
  logs({}, 0, [&](const LogRecord &r) {
    auto &s = impl_->sites[r.site];
    if (s.scope == none || string(impl_->nodes[s.scope].name) != "vtr.run" ||
        impl_->nodes[s.scope].parent != none)
      return true;
    auto fmt = string(s.format);
    auto &a = r.args;
    Ending e;
    e.time = r.time;
    if (fmt == "run stopped by signal {}" && a.size() == 1) {
      e.kind = Ending::stopped;
      e.signal = static_cast<std::int32_t>(a[0].number);
    } else if (fmt == "run exited with status {}" && a.size() == 1) {
      e.kind = Ending::exited;
      e.status = static_cast<std::int32_t>(a[0].number);
    } else if (fmt == "run crashed by signal {} (code {}, address {}, thread "
                      "{}, sealed {})" &&
               a.size() == 5) {
      e.kind = Ending::crashed;
      e.signal = static_cast<std::int32_t>(a[0].number);
      e.code = static_cast<std::int32_t>(a[1].number);
      e.address = a[2].number;
      e.thread = a[3].number;
      e.sealed = a[4].number != 0;
    } else if (fmt == "run recovered by scanning, {} bytes dropped" &&
               a.size() == 1) {
      e.kind = Ending::recovered;
      e.dropped = a[0].number;
    } else if (fmt == "run sealed after a panic in the writer" && a.empty())
      e.kind = Ending::poisoned;
    else
      return true;
    ending = e;
    return true;
  });
  return ending;
}
void Reader::clear_cache() const {
  std::lock_guard lock(impl_->cache_mutex);
  impl_->history_cache.clear();
  impl_->all_times.clear();
  impl_->times_loaded = false;
}
void recover(const std::string &input, const std::string &output) {
  require(input != output, "recovery input and output must differ", 1);
  std::error_code error;
  require(!std::filesystem::equivalent(input, output, error),
          "recovery paths refer to the same file", 1);
  Reader reader(input);
  Ending ending;
  ending.kind = Ending::recovered;
  ending.dropped = reader.stats().recovered_dropped;
  require(std::filesystem::copy_file(
              input, output, std::filesystem::copy_options::overwrite_existing,
              error),
          "cannot copy recovery input", 3);
  seal_file(output, ending);
}
} // namespace vtr
