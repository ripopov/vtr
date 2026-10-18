#include "c_api_internal.hpp"
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <sstream>
#include <unistd.h>
using namespace vtr;
using namespace vtr::detail;
using namespace vtr::capi;
struct vtr_activity_control {
  mutable std::atomic<unsigned> state{0};
  mutable std::atomic<size_t> completed{0}, total{0};
};
struct ActivityStretch {
  Time begin, end, gap;
};
struct vtr_activity_index {
  std::vector<std::vector<ActivityStretch>> signals;
  std::vector<std::pair<Time, Time>> cells;
  std::uint64_t source_size = 0;
  std::uint32_t source_crc = 0;
};
struct vtr_activity_classes {
  std::vector<Id> active, undecided;
};
namespace {
void cancelled(const vtr_activity_control *c) {
  require(!c || c->state.load() != 1, "activity build cancelled", 2);
}
Bytes read_bytes(const std::string &path) {
  std::ifstream f(path, std::ios::binary | std::ios::ate);
  require(bool(f), "cannot read activity file", 3);
  auto len = f.tellg();
  require(len >= 0 && std::uint64_t(len) < (1ull << 32),
          "activity file too large");
  Bytes data(static_cast<size_t>(len));
  f.seekg(0);
  require(bool(f.read(reinterpret_cast<char *>(data.data()),
                      static_cast<std::streamsize>(data.size()))),
          "activity read failed", 3);
  return data;
}
struct Source {
  std::uint64_t size;
  std::uint32_t crc;
  std::vector<Section> blocks;
};
Source source(const std::string &path) {
  std::ifstream f(path, std::ios::binary | std::ios::ate);
  require(bool(f), "cannot read source identity", 3);
  auto length = f.tellg();
  require(length >= 56, "source has no directory");
  Source s{static_cast<std::uint64_t>(length), 0, {}};
  std::array<std::uint8_t, 24> end{};
  f.seekg(length - std::streamoff(24));
  f.read(reinterpret_cast<char *>(end.data()), 24);
  require(std::equal(trailer.begin(), trailer.end(), end.begin() + 16),
          "source has no valid trailer");
  auto off = load<std::uint64_t>(end);
  require(off >= 32 && off <= s.size - 48, "invalid source directory");
  f.seekg(static_cast<std::streamoff>(off));
  std::array<std::uint8_t, 24> header{};
  f.read(reinterpret_cast<char *>(header.data()), 24);
  require(load<Id>(header) == 7, "source directory missing");
  auto bytes = load<std::uint64_t>(View(header).subspan(8));
  require(bytes == s.size - off - 48 && bytes < (1u << 30),
          "invalid source directory length");
  Bytes raw(bytes);
  require(bool(f.read(reinterpret_cast<char *>(raw.data()), raw.size())),
          "cannot read source directory", 3);
  s.crc = checksum(raw);
  Cursor r(raw);
  auto n = r.fixed<std::uint64_t>();
  require(n <= r.data.size() / 40, "invalid source directory count");
  for (std::uint64_t i = 0; i < n; ++i) {
    Section e;
    e.kind = r.fixed<Id>();
    e.flags = r.fixed<Id>();
    e.offset = r.fixed<std::uint64_t>();
    e.length = r.fixed<std::uint64_t>();
    e.aux0 = r.fixed<std::uint64_t>();
    e.aux1 = r.fixed<std::uint64_t>();
    if (e.kind == 4)
      s.blocks.push_back(e);
  }
  r.done();
  return s;
}
std::vector<std::string> paths(const char *path, const char *cache,
                               const Source &s) {
  std::vector<std::string> p{std::string(text(path)) + ".index"};
  if (cache) {
    std::ostringstream name;
    name << "vtr-" << s.size << "-" << std::hex << std::setw(8)
         << std::setfill('0') << s.crc << ".index";
    p.push_back((std::filesystem::path(cache) / name.str()).string());
  }
  return p;
}
template <size_t N> Bytes pack(const std::array<Bytes, N> &c) {
  Bytes out;
  for (auto &b : c)
    put(out, b.size());
  for (auto &b : c)
    append(out, b);
  return compress(out, Codec::zstd, 3);
}
template <size_t N> std::array<Cursor, N> unpack(View bytes, Bytes &storage) {
  storage = decompress(bytes);
  Cursor r(storage);
  std::array<size_t, N> lengths{};
  for (auto &n : lengths)
    n = r.var();
  std::array<Cursor, N> c;
  for (size_t i = 0; i < N; ++i)
    c[i] = Cursor(r.take(lengths[i]));
  r.done();
  return c;
}
void add_section(Bytes &file, std::vector<Section> &directory, Id kind,
                 View payload, Time begin = 0, Time end = 0) {
  directory.push_back({kind, 0, file.size(), payload.size(), begin, end});
  fixed(file, kind);
  fixed<Id>(file, 0);
  fixed<std::uint64_t>(file, payload.size());
  fixed(file, checksum(payload));
  fixed<Id>(file, 0);
  append(file, payload);
}
std::unique_ptr<vtr_activity_index>
decode(const vtr_reader &r, View bytes, std::uint64_t memory,
       const vtr_activity_control *control) {
  cancelled(control);
  require(!memory || bytes.size() <= memory,
          "activity index exceeds memory limit", 2);
  require(
      bytes.size() >= 56 && bytes[0] == 0x89 &&
          std::equal(bytes.begin() + 1, bytes.begin() + 4, view("VTI").begin()),
      "invalid activity magic");
  require(load<std::uint16_t>(bytes.subspan(8)) == 2,
          "unsupported activity version", 5);
  require(std::equal(trailer.begin(), trailer.end(), bytes.end() - 8),
          "missing activity trailer");
  auto end = bytes.size() - 24, dir = load<std::uint64_t>(bytes.subspan(end));
  require(load<std::uint64_t>(bytes.subspan(end + 8)) == bytes.size() &&
              dir < end,
          "invalid activity trailer");
  auto index = std::make_unique<vtr_activity_index>();
  index->signals.resize(r.reader.signals().size());
  std::vector<std::optional<Time>> start(index->signals.size());
  bool header = false, tail = false;
  std::uint64_t offset = 32, used = bytes.size();
  while (offset < end) {
    require(end - offset >= 24, "truncated activity section");
    Cursor h(bytes.subspan(offset, 24));
    auto kind = h.fixed<Id>();
    h.fixed<Id>();
    auto len = h.fixed<std::uint64_t>();
    auto crc = h.fixed<Id>();
    require(h.fixed<Id>() == 0 && len <= end - offset - 24,
            "invalid activity section");
    auto body = bytes.subspan(offset + 24, len);
    require(checksum(body) == crc, "activity checksum mismatch", 7);
    Cursor b(body);
    if (kind == 1) {
      require(!header && len == 40, "invalid activity header");
      index->source_size = b.fixed<std::uint64_t>();
      index->source_crc = b.fixed<Id>();
      require(b.byte() == 1, "unsupported activity source");
      b.take(3);
      require(b.fixed<Id>() == index->signals.size(),
              "activity signal count mismatch");
      header = true;
    } else if (kind == 2 || kind == 3) {
      require(header && !tail, "invalid activity section order");
      Time begin = 0, finish = 0;
      Id rows;
      unsigned exponent = 0;
      if (kind == 2) {
        begin = b.fixed<std::uint64_t>();
        finish = b.fixed<std::uint64_t>();
        b.fixed<std::uint64_t>();
        rows = b.fixed<Id>();
        b.fixed<Id>();
        exponent = b.byte();
        b.take(7);
        require(exponent <= 63 && begin <= finish, "invalid activity block");
        index->cells.emplace_back(finish, Time{1} << exponent);
      } else {
        rows = b.fixed<Id>();
        require(b.fixed<Id>() == 0, "invalid tail flags");
        tail = true;
      }
      require(rows <= index->signals.size(), "invalid activity row count");
      Bytes raw;
      Id signal = 0;
      auto add = [&](Time length, Time gap) {
        require(start[signal].has_value() &&
                    length <= forever - *start[signal] && gap <= length,
                "invalid activity stretch");
        index->signals[signal].push_back(
            {*start[signal], *start[signal] + length, gap});
        used += sizeof(ActivityStretch);
        require(!memory || used <= memory,
                "activity index exceeds memory limit", 2);
        return *start[signal] + length;
      };
      if (kind == 2) {
        auto c = unpack<6>(b.data, raw);
        for (Id i = 0; i < rows; ++i) {
          auto delta = c[0].var();
          require(delta <= none - signal, "activity signal overflow");
          signal += static_cast<Id>(delta);
          require(signal < start.size() && (i == 0 || delta > 0),
                  "invalid activity signal");
          auto head = c[1].var();
          if (head & 1) {
            require(!start[signal], "duplicate first activity");
            auto dt = c[2].var();
            require(dt <= finish - begin, "invalid first activity time");
            start[signal] = begin + dt;
          }
          for (std::uint64_t j = 0; j < (head >> 1); ++j) {
            auto length = c[3].var(), silence = c[4].var(), gap = c[5].var();
            auto last = add(length, gap);
            require(silence && silence <= forever - last,
                    "invalid activity silence");
            start[signal] = last + silence;
          }
        }
        for (auto &col : c)
          col.done();
      } else {
        auto c = unpack<3>(b.data, raw);
        for (Id i = 0; i < rows; ++i) {
          auto delta = c[0].var();
          require(delta <= none - signal, "activity signal overflow");
          signal += static_cast<Id>(delta);
          require(signal < start.size() && (i == 0 || delta > 0),
                  "invalid activity tail signal");
          add(c[1].var(), c[2].var());
          start[signal].reset();
        }
        for (auto &col : c)
          col.done();
      }
    } else if (kind == 7) {
      require(offset == dir, "invalid activity directory offset");
    } else
      throw Error("unknown activity section", 4);
    offset += 24 + len;
    cancelled(control);
  }
  require(offset == end && header && tail, "incomplete activity index");
  for (auto s : start)
    require(!s, "missing activity tail row");
  return index;
}
} // namespace
extern "C" {
vtr_activity_control *vtr_activity_control_new(void) {
  return result<vtr_activity_control *>(
      nullptr, [] { return new vtr_activity_control; });
}
void vtr_activity_control_free(vtr_activity_control *c) { delete c; }
int vtr_activity_control_cancel(const vtr_activity_control *c) {
  if (!c)
    return 0;
  unsigned expected = 0;
  return c->state.compare_exchange_strong(expected, 1);
}
int vtr_activity_control_is_cancelled(const vtr_activity_control *c) {
  return c && c->state.load() == 1;
}
int vtr_activity_control_complete(const vtr_activity_control *c) {
  if (!c)
    return 0;
  unsigned expected = 0;
  return c->state.compare_exchange_strong(expected, 2);
}
void vtr_activity_control_progress(const vtr_activity_control *c, size_t *done,
                                   size_t *total) {
  if (done)
    *done = c ? c->completed.load() : 0;
  if (total)
    *total = c ? c->total.load() : 0;
}
int vtr_activity_write(const vtr_reader *r, const char *path, const char *cache,
                       uint32_t threads, uint64_t memory,
                       vtr_activity_summary *out) {
  return vtr_activity_write_controlled(r, path, cache, threads, memory, nullptr,
                                       out);
}
int vtr_activity_write_controlled(const vtr_reader *r, const char *path,
                                  const char *cache, uint32_t, uint64_t memory,
                                  const vtr_activity_control *control,
                                  vtr_activity_summary *out) {
  return status([&] {
    auto &rd = need(r).reader;
    cancelled(control);
    auto src = source(std::string(text(path)));
    if (control)
      control->total = src.blocks.size();
    std::vector<std::vector<ActivityStretch>> stretches(rd.signals().size());
    auto minimum = rd.stats().time_range ? rd.stats().time_range->first : 0;
    std::uint64_t used =
                      stretches.size() * sizeof(std::vector<ActivityStretch>),
                  changes = 0;
    rd.changes(0, forever, [&](Time t, Id signal, SignalValue) {
      cancelled(control);
      if (t <= minimum)
        return true;
      auto &list = stretches[signal];
      if (!list.empty() && list.back().end == t)
        return true;
      ++changes;
      if (!list.empty() && t - list.back().end <= 1) {
        list.back().gap = std::max(list.back().gap, t - list.back().end);
        list.back().end = t;
      } else {
        list.push_back({t, t, 0});
        used += sizeof(ActivityStretch);
        require(!memory || used <= memory,
                "activity index exceeds memory limit", 2);
      }
      return true;
    });
    Bytes file(magic.begin(), magic.end());
    file[3] = 'I';
    fixed<std::uint16_t>(file, 2);
    fixed<std::uint16_t>(file, 0);
    file.resize(32);
    std::vector<Section> dir;
    Bytes h;
    fixed(h, src.size);
    fixed(h, src.crc);
    h.push_back(1);
    h.resize(16);
    fixed<Id>(h, stretches.size());
    fixed<Id>(h, 0);
    fixed<Id>(h, 0);
    fixed<Id>(h, 0);
    fixed(h, minimum);
    add_section(file, dir, 1, h);
    std::vector<size_t> closed(stretches.size());
    std::vector<bool> first(stretches.size(), false);
    std::uint64_t total_stretches = 0;
    for (auto &s : stretches)
      total_stretches += s.size();
    for (auto &block : src.blocks) {
      cancelled(control);
      std::array<Bytes, 6> cols;
      Id previous = 0, rows = 0, nclosed = 0;
      for (Id signal = 0; signal < stretches.size(); ++signal) {
        auto &list = stretches[signal];
        if (list.empty())
          continue;
        bool initial = !first[signal] && list[0].begin <= block.aux1;
        auto count = closed[signal];
        while (count + 1 < list.size() && list[count + 1].begin <= block.aux1)
          ++count;
        auto number = count - closed[signal];
        if (!initial && !number)
          continue;
        put(cols[0], signal - previous);
        previous = signal;
        put(cols[1], (number << 1) | initial);
        if (initial) {
          require(list[0].begin >= block.aux0, "activity first outside block");
          put(cols[2], list[0].begin - block.aux0);
          first[signal] = true;
        }
        for (auto i = closed[signal]; i < count; ++i) {
          put(cols[3], list[i].end - list[i].begin);
          put(cols[4], list[i + 1].begin - list[i].end);
          put(cols[5], list[i].gap);
        }
        closed[signal] = count;
        ++rows;
        nclosed += number;
      }
      Bytes body;
      fixed(body, block.aux0);
      fixed(body, block.aux1);
      fixed(body, block.length + 24);
      fixed(body, rows);
      fixed(body, nclosed);
      body.resize(40);
      append(body, pack(cols));
      add_section(file, dir, 2, body, block.aux0, block.aux1);
      if (control)
        ++control->completed;
    }
    std::array<Bytes, 3> tails;
    Id previous = 0, rows = 0;
    for (Id signal = 0; signal < stretches.size(); ++signal)
      if (!stretches[signal].empty()) {
        auto &s = stretches[signal].back();
        put(tails[0], signal - previous);
        previous = signal;
        put(tails[1], s.end - s.begin);
        put(tails[2], s.gap);
        ++rows;
      }
    Bytes tail;
    fixed(tail, rows);
    fixed<Id>(tail, 0);
    append(tail, pack(tails));
    add_section(file, dir, 3, tail);
    Bytes directory;
    fixed<std::uint64_t>(directory, dir.size());
    for (auto &s : dir) {
      fixed(directory, s.kind);
      fixed(directory, s.flags);
      fixed(directory, s.offset);
      fixed(directory, s.length);
      fixed(directory, s.aux0);
      fixed(directory, s.aux1);
    }
    auto offset = file.size();
    add_section(file, dir, 7, directory);
    fixed<std::uint64_t>(file, offset);
    fixed<std::uint64_t>(file, file.size() + 16);
    append(file, trailer);
    auto choices = paths(path, cache, src);
    bool written = false;
    for (auto &destination : choices) {
      auto temporary = destination + ".tmp." + std::to_string(getpid());
      try {
        if (destination != choices.front())
          std::filesystem::create_directories(
              std::filesystem::path(destination).parent_path());
        std::ofstream f(temporary, std::ios::binary | std::ios::trunc);
        if (!f)
          continue;
        f.write(reinterpret_cast<const char *>(file.data()), file.size());
        f.close();
        require(bool(f), "activity write failed", 3);
        if (control) {
          unsigned expected = 0;
          require(control->state.compare_exchange_strong(expected, 3) ||
                      expected == 3,
                  "activity build cancelled", 2);
        }
        std::filesystem::rename(temporary, destination);
        written = true;
        break;
      } catch (...) {
        if (control) {
          unsigned expected = 3;
          control->state.compare_exchange_strong(expected, 0);
        }
        std::error_code ec;
        std::filesystem::remove(temporary, ec);
        if (destination == choices.back())
          throw;
      }
    }
    require(written, "cannot publish activity index", 3);
    if (control)
      control->state.store(2);
    if (out)
      *out = {static_cast<Id>(src.blocks.size()),
              static_cast<Id>(stretches.size()),
              changes,
              total_stretches,
              1,
              1,
              file.size(),
              src.size};
  });
}
vtr_activity_index *vtr_activity_decode(const vtr_reader *r,
                                        const uint8_t *data, size_t len,
                                        uint64_t memory,
                                        const vtr_activity_control *control) {
  return result<vtr_activity_index *>(nullptr, [&] {
    return decode(need(r), span(data, len), memory, control).release();
  });
}
vtr_activity_index *vtr_activity_load(const vtr_reader *r, const char *path,
                                      const char *cache) {
  return result<vtr_activity_index *>(nullptr, [&] {
    need(r);
    auto src = source(std::string(text(path)));
    for (auto &p : paths(path, cache, src))
      try {
        auto bytes = read_bytes(p);
        auto index = decode(*r, bytes, 0, nullptr);
        if (index->source_size == src.size && index->source_crc == src.crc)
          return index.release();
      } catch (const Error &) {
      }
    throw Error("activity index not found", 9);
  });
}
void vtr_activity_free(vtr_activity_index *x) { delete x; }
uint64_t vtr_activity_exact_width(const vtr_activity_index *x, uint64_t begin,
                                  uint64_t end) {
  return result<Time>(0, [&] {
    auto &index = need(x);
    Time width = forever;
    for (size_t i = 0; i < index.cells.size(); ++i) {
      if (i && index.cells[i - 1].first >= end)
        break;
      if (i + 1 < index.cells.size() && index.cells[i].first < begin)
        continue;
      width = std::min(width, index.cells[i].second);
    }
    return width == forever ? Time{1} : width;
  });
}
vtr_activity_classes *vtr_activity_classify(const vtr_activity_index *x,
                                            uint64_t begin, uint64_t end) {
  return result<vtr_activity_classes *>(nullptr, [&] {
    auto &index = need(x);
    require(begin <= end, "invalid activity window", 1);
    auto c = std::make_unique<vtr_activity_classes>();
    auto width = end - begin == forever ? forever : end - begin + 1;
    auto exact = vtr_activity_exact_width(x, begin, end);
    for (Id s = 0; s < index.signals.size(); ++s) {
      auto &list = index.signals[s];
      auto it = std::lower_bound(list.begin(), list.end(), begin,
                                 [](auto &st, auto t) { return st.end < t; });
      if (it == list.end() || it->begin > end)
        continue;
      if (it->begin >= begin || it->end <= end ||
          (it->gap != UINT32_MAX && it->gap <= width) || width >= exact)
        c->active.push_back(s);
      else
        c->undecided.push_back(s);
    }
    return c.release();
  });
}
const uint32_t *vtr_activity_classes_active(const vtr_activity_classes *c,
                                            size_t *len) {
  if (len)
    *len = c ? c->active.size() : 0;
  return c ? c->active.data() : nullptr;
}
const uint32_t *vtr_activity_classes_undecided(const vtr_activity_classes *c,
                                               size_t *len) {
  if (len)
    *len = c ? c->undecided.size() : 0;
  return c ? c->undecided.data() : nullptr;
}
void vtr_activity_classes_free(vtr_activity_classes *c) { delete c; }
int vtr_activity_resolve(const vtr_reader *r, const uint32_t *signals, size_t n,
                         uint64_t begin, uint64_t end, uint32_t *out,
                         size_t *len) {
  return status([&] {
    auto &rd = need(r).reader;
    span(signals, n);
    require(out || !n, "null activity output", 8);
    need(len) = 0;
    auto first = rd.stats().time_range ? rd.stats().time_range->first : 0;
    for (auto s : span(signals, n)) {
      bool found = false;
      rd.changes(s, begin, end, [&](Time t, Id, SignalValue) {
        if (t > first)
          found = true;
        return !found;
      });
      if (found)
        out[(*len)++] = s;
    }
  });
}
}
