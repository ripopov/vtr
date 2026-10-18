#include "c_api_internal.hpp"
#include <unordered_set>
using namespace vtr;
using namespace vtr::detail;
using namespace vtr::capi;
struct vtr_scope_sizes {
  std::vector<vtr_scope_size> sizes;
};
struct vtr_contributions {
  std::vector<std::set<Id>> signals;
};
struct vtr_census {
  std::vector<vtr_scope_size> scopes;
  std::vector<std::set<Id>> signals;
  std::vector<Id> stack;
};
struct vtr_clock_timeline {
  std::vector<vtr_stretch> stretches;
  bool open = false;
  std::uint64_t count = 0;
  static std::uint64_t edges(const vtr_stretch &s) {
    return s.period ? (s.end - s.begin) / s.period + 1 : 1;
  }
  std::optional<Time> edge(std::uint64_t cycle) const {
    auto it = std::upper_bound(
        stretches.begin(), stretches.end(), cycle,
        [](auto c, const auto &s) { return c < s.first_cycle; });
    if (it == stretches.begin())
      return {};
    --it;
    auto offset = cycle - it->first_cycle;
    if (offset >= edges(*it))
      return {};
    return it->begin + offset * it->period;
  }
  std::optional<vtr_cycle_at> at(Time time) const {
    auto it =
        std::upper_bound(stretches.begin(), stretches.end(), time,
                         [](auto t, const auto &s) { return t < s.begin; });
    if (it == stretches.begin())
      return {};
    --it;
    auto k = it->period
                 ? std::min((time - it->begin) / it->period, edges(*it) - 1)
                 : 0;
    vtr_cycle_at c{};
    c.cycle = it->first_cycle + k;
    c.edge = it->begin + k * it->period;
    auto next = edge(c.cycle + 1);
    c.has_next_edge = next.has_value();
    c.next_edge = next.value_or(0);
    c.fraction = next ? double(time - c.edge) / double(*next - c.edge) : 0;
    if (k + 1 == edges(*it)) {
      auto following = it + 1;
      if (following == stretches.end())
        c.stopped = !open;
      else {
        auto gap = following->begin - it->end;
        c.stopped = (!it->period || gap / 2 > it->period) &&
                    (!following->period || gap / 2 > following->period);
      }
    }
    return c;
  }
};
namespace {
std::vector<Id> clocks(const Reader &reader) {
  std::vector<Id> ids;
  auto nodes = reader.nodes();
  for (Id i = 0; i < nodes.size(); ++i)
    if (nodes[i].kind == Node::generator &&
        reader.string(nodes[i].aux) == "CLOCK")
      ids.push_back(i);
  return ids;
}
std::unique_ptr<vtr_scope_sizes> finish(vtr_census &c) {
  auto out = std::make_unique<vtr_scope_sizes>();
  for (size_t i = 0; i < c.scopes.size(); ++i)
    c.scopes[i].signals = static_cast<Id>(c.signals[i].size());
  out->sizes = std::move(c.scopes);
  return out;
}
} // namespace
extern "C" {
vtr_census *vtr_census_new(void) {
  return result<vtr_census *>(nullptr, [] { return new vtr_census; });
}
vtr_census *vtr_census_new_recording(void) { return vtr_census_new(); }
uint32_t vtr_census_enter(vtr_census *c) {
  return result<Id>(none, [&] {
    auto &v = need(c);
    auto id = static_cast<Id>(v.scopes.size());
    v.scopes.push_back(
        {none, v.stack.empty() ? none : v.stack.back(), 0, 0, 1});
    v.signals.emplace_back();
    for (auto parent : v.stack)
      ++v.scopes[parent].scopes;
    v.stack.push_back(id);
    return id;
  });
}
int vtr_census_var(vtr_census *c, uint32_t signal) {
  return status([&] {
    auto &v = need(c);
    require(!v.stack.empty(), "no open scope", 1);
    for (auto parent : v.stack) {
      ++v.scopes[parent].variables;
      v.signals[parent].insert(signal);
    }
  });
}
int vtr_census_leave(vtr_census *c) {
  return status([&] {
    auto &v = need(c);
    require(!v.stack.empty(), "no open scope", 1);
    v.stack.pop_back();
  });
}
vtr_scope_sizes *vtr_census_finish(vtr_census *c) {
  return result<vtr_scope_sizes *>(nullptr, [&] {
    need(c);
    std::unique_ptr<vtr_census> owner(c);
    return finish(*c).release();
  });
}
vtr_scope_sizes *vtr_census_finish_recorded(vtr_census *c,
                                            vtr_contributions **out) {
  return result<vtr_scope_sizes *>(nullptr, [&] {
    need(c);
    need(out);
    std::unique_ptr<vtr_census> owner(c);
    auto sizes = finish(*c);
    auto contribution = std::make_unique<vtr_contributions>();
    contribution->signals = std::move(c->signals);
    *out = contribution.release();
    return sizes.release();
  });
}
void vtr_census_free(vtr_census *c) { delete c; }
size_t vtr_contributions_count(const vtr_contributions *x,
                               const uint32_t *signals, size_t n, uint32_t *out,
                               size_t cap) {
  return result<size_t>(0, [&] {
    auto &c = need(x);
    span(signals, n);
    require(out || !cap, "null output array", 8);
    std::set<Id> selected(signals, signals + n);
    for (size_t i = 0; i < std::min(cap, c.signals.size()); ++i) {
      out[i] = 0;
      for (auto s : selected)
        out[i] += c.signals[i].contains(s);
    }
    return c.signals.size();
  });
}
void vtr_contributions_free(vtr_contributions *c) { delete c; }
vtr_scope_sizes *vtr_reader_scope_sizes(const vtr_reader *r) {
  return result<vtr_scope_sizes *>(nullptr, [&] {
    auto &reader = need(r).reader;
    auto nodes = reader.nodes();
    vtr_census census;
    std::function<void(Id)> visit = [&](Id parent) {
      for (Id id = 0; id < nodes.size(); ++id)
        if (nodes[id].parent == parent) {
          if (nodes[id].kind == Node::scope) {
            auto i = vtr_census_enter(&census);
            census.scopes[i].node = id;
            visit(id);
            vtr_census_leave(&census);
          } else if (nodes[id].kind == Node::variable && !census.stack.empty())
            vtr_census_var(&census, nodes[id].signal);
        }
    };
    visit(none);
    return finish(census).release();
  });
}
size_t vtr_scope_sizes_len(const vtr_scope_sizes *s) {
  return s ? s->sizes.size() : 0;
}
int vtr_scope_sizes_get(const vtr_scope_sizes *s, size_t i,
                        vtr_scope_size *out) {
  return status([&] { need(out) = at(need(s).sizes, i); });
}
void vtr_scope_sizes_free(vtr_scope_sizes *s) { delete s; }
uint32_t vtr_reader_clock_count(const vtr_reader *r) {
  return result<Id>(
      0, [&] { return static_cast<Id>(clocks(need(r).reader).size()); });
}
int vtr_reader_clock(const vtr_reader *r, uint32_t clock, vtr_clock_info *out) {
  return status(
      [&] { need(out).generator = at(clocks(need(r).reader), clock); });
}
uint32_t vtr_reader_generator_clock(const vtr_reader *r, uint32_t generator) {
  return result<Id>(none, [&] {
    auto &rd = need(r).reader;
    auto &n = at(rd.nodes(), generator);
    Id target = none;
    for (auto &a : n.attrs)
      if (rd.string(a.key) == "vtr.clock" && a.value.tag == Tag::str)
        target = rd.find_node(rd.string(a.value.literal));
    auto list = clocks(rd);
    for (Id i = 0; i < list.size(); ++i)
      if (list[i] == target)
        return i;
    return none;
  });
}
vtr_clock_timeline *vtr_reader_load_clock(const vtr_reader *r, uint32_t clock) {
  return result<vtr_clock_timeline *>(nullptr, [&] {
    auto &rd = need(r).reader;
    auto gen = at(clocks(rd), clock);
    auto tl = std::make_unique<vtr_clock_timeline>();
    rd.transactions({gen}, [&](const Transaction &t) {
      Time period = 0;
      for (auto &a : t.attrs)
        if (rd.string(a.key) == "vtr.period") {
          require(a.value.tag == Tag::time, "invalid clock period");
          period = a.value.number;
        }
      require(t.end >= t.begin &&
                  ((period && ((t.end - t.begin) % period == 0)) ||
                   (!period && t.begin == t.end)),
              "invalid clock stretch");
      tl->stretches.push_back({t.begin, t.end, period, 0});
      tl->open |= t.status == 4;
      return true;
    });
    std::sort(tl->stretches.begin(), tl->stretches.end(),
              [](auto &a, auto &b) { return a.begin < b.begin; });
    std::optional<Time> end;
    for (auto &s : tl->stretches) {
      require(!end || s.begin > *end, "overlapping clock stretches");
      s.first_cycle = tl->count;
      auto count = vtr_clock_timeline::edges(s);
      require(count <= forever - tl->count, "clock edge count overflow");
      tl->count += count;
      end = s.end;
    }
    tl->open &= !tl->stretches.empty();
    return tl.release();
  });
}
void vtr_clock_timeline_free(vtr_clock_timeline *t) { delete t; }
size_t vtr_clock_timeline_stretch_count(const vtr_clock_timeline *t) {
  return t ? t->stretches.size() : 0;
}
uint64_t vtr_clock_timeline_edge_count(const vtr_clock_timeline *t) {
  return t ? t->count : 0;
}
int vtr_clock_timeline_is_open(const vtr_clock_timeline *t) {
  return t && t->open;
}
int vtr_clock_timeline_stretch(const vtr_clock_timeline *t, size_t i,
                               vtr_stretch *out) {
  return status([&] { need(out) = at(need(t).stretches, i); });
}
int vtr_clock_timeline_cycle_at(const vtr_clock_timeline *t, uint64_t time,
                                vtr_cycle_at *out) {
  return status([&] {
    auto cycle = need(t).at(time);
    require(cycle.has_value(), "no clock edge before time", 9);
    need(out) = *cycle;
  });
}
int vtr_clock_timeline_edge(const vtr_clock_timeline *t, uint64_t cycle,
                            uint64_t *out) {
  return status([&] {
    auto edge = need(t).edge(cycle);
    require(edge.has_value(), "clock cycle not found", 9);
    need(out) = *edge;
  });
}
int vtr_clock_timeline_next_edge(const vtr_clock_timeline *t, uint64_t time,
                                 uint64_t *out) {
  return status([&] {
    auto &tl = need(t);
    auto cycle = tl.at(time);
    auto edge = tl.edge(cycle ? cycle->cycle + 1 : 0);
    require(edge.has_value(), "next clock edge not found", 9);
    need(out) = *edge;
  });
}
int vtr_clock_timeline_prev_edge(const vtr_clock_timeline *t, uint64_t time,
                                 uint64_t *out) {
  return status([&] {
    auto &tl = need(t);
    require(time > 0, "previous clock edge not found", 9);
    auto cycle = tl.at(time - 1);
    require(cycle.has_value(), "previous clock edge not found", 9);
    need(out) = cycle->edge;
  });
}
}
