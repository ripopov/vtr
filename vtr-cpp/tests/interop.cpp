#include "format.hpp"
#include "vtr.h"
#include "vtr.hpp"
#include <iomanip>
#include <iostream>
using namespace vtr;
using namespace vtr::detail;
namespace {
void hash(std::uint64_t &h, View b) {
  for (auto v : b)
    h = (h ^ v) * 1099511628211ull;
}
void hash_number(std::uint64_t &h, std::uint64_t n) {
  Bytes b;
  fixed(b, n);
  hash(h, b);
}
void activity(const std::string &path, bool write_index) {
  std::unique_ptr<vtr_reader, decltype(&vtr_reader_close)> r(
      vtr_reader_open(path.c_str()), vtr_reader_close);
  require(bool(r), vtr_last_error());
  if (write_index)
    require(vtr_activity_write(r.get(), path.c_str(), nullptr, 1, 0, nullptr) ==
                VTR_OK,
            vtr_last_error());
  std::unique_ptr<vtr_activity_index, decltype(&vtr_activity_free)> index(
      vtr_activity_load(r.get(), path.c_str(), nullptr), vtr_activity_free);
  require(bool(index), vtr_last_error());
  for (Time begin :
       std::array<Time, 8>{0, 1, 7, 33, 500, 1999, 5000, (Time{1} << 34) + 1}) {
    for (Time width : std::array<Time, 5>{0, 1, 10, 1000, Time{1} << 32}) {
      std::unique_ptr<vtr_activity_classes,
                      decltype(&vtr_activity_classes_free)>
          c(vtr_activity_classify(index.get(), begin, begin + width),
            vtr_activity_classes_free);
      require(bool(c), vtr_last_error());
      std::cout << begin << ':' << width << ':';
      size_t n = 0;
      auto ids = vtr_activity_classes_active(c.get(), &n);
      for (size_t i = 0; i < n; ++i)
        std::cout << ids[i] << ',';
      std::cout << ':';
      ids = vtr_activity_classes_undecided(c.get(), &n);
      for (size_t i = 0; i < n; ++i)
        std::cout << ids[i] << ',';
      std::cout << '\n';
    }
  }
}
void digest(const std::string &path) {
  Reader r(path, {.verify_crc = true});
  std::uint64_t h = 1469598103934665603ull;
  std::vector<std::shared_ptr<const SignalData>> histories;
  for (Id s = 0; s < r.signals().size(); ++s) {
    hash_number(h, s);
    auto data = r.load_signal(s);
    histories.push_back(data);
    hash(h, view(SignalValue{data->kind, data->initial}.ascii()));
    for (size_t i = 0; i < data->times.size(); ++i) {
      hash_number(h, data->times[i]);
      hash(h, view(data->at(i).ascii()));
    }
    for (Time t : {0, 1, 7, 33, 500, 1999, 5000}) {
      auto b = r.value_at(s, t);
      hash(h, view(SignalValue{data->kind, b}.ascii()));
    }
  }
  std::vector<std::size_t> positions(histories.size());
  r.changes(0, forever, [&](Time time, Id signal, const SignalValue &value) {
    auto &history = *histories.at(signal);
    auto i = positions[signal]++;
    require(i < history.times.size() && history.times[i] == time &&
                history.at(i).ascii() == value.ascii(),
            "bulk scan differs from selective history");
    return true;
  });
  for (std::size_t s = 0; s < histories.size(); ++s)
    require(positions[s] == histories[s]->times.size(),
            "bulk scan lost changes");
  std::vector<Transaction> txs;
  r.transactions({}, [&](auto &t) {
    txs.push_back(t);
    return true;
  });
  std::sort(txs.begin(), txs.end(),
            [](auto &a, auto &b) { return a.id < b.id; });
  for (auto &t : txs) {
    Bytes row;
    put(row, t.id);
    put(row, t.generator);
    put(row, t.begin);
    put(row, t.end - t.begin);
    row.push_back(t.status);
    row.push_back(t.kind);
    put(row, t.parent ? t.parent + 1 : 0);
    encode_attrs(row, t.attrs);
    put(row, t.events.size());
    for (auto &e : t.events) {
      put(row, e.time);
      put(row, e.name);
      encode_attrs(row, e.attrs);
    }
    put(row, t.stages.size());
    for (auto &s : t.stages) {
      put(row, s.name);
      put(row, s.lane);
      put(row, s.begin);
      put(row, s.end ? *s.end - s.begin + 1 : 0);
      encode_attrs(row, s.attrs);
    }
    hash(h, row);
    r.relations(t.id, false, [&](auto &rel) {
      hash_number(h, rel.kind);
      hash_number(h, rel.from);
      hash_number(h, rel.to);
      Bytes attributes;
      encode_attrs(attributes, rel.attrs);
      hash(h, attributes);
      return true;
    });
  }
  std::cout << std::hex << std::setw(16) << std::setfill('0') << h << '\n';
}
void write(const std::string &path, Codec codec) {
  WriterOptions options;
  options.codec = codec;
  options.block_records = 2048;
  options.tx_block_bytes = 4096;
  options.dedup = false;
  Writer w(path, options);
  auto top = w.add_scope(none, "top", 0, "oracle");
  std::vector<std::pair<Id, SignalKind>> signals;
  for (std::uint8_t states : {2, 4, 9})
    for (unsigned width : {1, 13, 96}) {
      SignalKind k{SignalKind::bits, width, states};
      signals.emplace_back(
          w.add_var(top, "s" + std::to_string(signals.size()), k).second, k);
    }
  auto real = w.add_var(top, "real", {SignalKind::real, 0, 0}, 3).second;
  auto bytes = w.add_var(top, "bytes", {SignalKind::varlen, 0, 0}, 21).second;
  auto event = w.add_var(top, "event", {SignalKind::bits, 1, 2}, 0).second;
  w.add_alias(top, "alias", signals[0].first);
  auto g0 = w.add_generator(top, "requests", "REQUEST"),
       g1 = w.add_generator(top, "responses", "RESPONSE");
  auto label = w.intern("label"), literal = w.intern("literal");
  std::vector<Value> values{Value{},
                            Value(true),
                            Value(std::int64_t(-123)),
                            Value(UINT64_MAX),
                            Value(1.25),
                            Value::string(literal),
                            Value::bytes(Bytes{0, 255, 1})};
  for (auto [tag, width, data] : std::vector<std::tuple<Tag, unsigned, Bytes>>{
           {Tag::bits, 13, {0x34, 0x12}},
           {Tag::logic, 3, {0x39}},
           {Tag::logic9, 3, {0x87, 1}}}) {
    Value v;
    v.tag = tag;
    put(v.payload, width);
    append(v.payload, data);
    values.push_back(v);
  }
  values.push_back(Value::time(999));
  Value e(std::int64_t(-2));
  e.tag = Tag::enumeration;
  e.literal = literal;
  values.push_back(e);
  Value pointer(std::uint64_t(0x1234));
  pointer.tag = Tag::pointer;
  values.push_back(pointer);
  Value fixed_value(std::int64_t(-42));
  fixed_value.tag = Tag::fixed;
  fixed_value.scale = 3;
  values.push_back(fixed_value);
  Value ufixed(std::uint64_t(42));
  ufixed.tag = Tag::ufixed;
  ufixed.scale = -3;
  values.push_back(ufixed);
  std::array<Value, 2> list{Value(false), Value::text("nested")};
  values.push_back(Value::list(list));
  Value map;
  map.tag = Tag::map;
  put(map.payload, 1);
  put(map.payload, label);
  encode_value(map.payload, Value(std::uint64_t(9)));
  values.push_back(map);
  values.push_back(Value::text("inline text"));
  std::vector<Id> keys;
  for (size_t i = 0; i < values.size(); ++i)
    keys.push_back(w.intern("key" + std::to_string(i)));
  auto stage = w.intern("execute"), lane = w.intern("lane"),
       relation = w.intern("depends");
  std::array<Tag, 3> types{Tag::text, Tag::i64, Tag::f64};
  std::array<std::string_view, 3> names{"text", "number", "fraction"};
  auto site = w.add_log_site(top, 2, "{} {} {}", types, names);
  for (Time t = 0; t < 2000; ++t) {
    w.set_time(t);
    for (auto [s, k] : signals) {
      if (k.width == 1)
        w.emit_bit(s, t % 2);
      else if (k.width == 96) {
        std::array<std::uint32_t, 3> words{static_cast<Id>(t), 0x1234,
                                           static_cast<Id>(t / 17)};
        w.emit_words(s, words);
      } else if (k.states != 2 && t % 17 == 0)
        w.emit_logic(s, "x0011z0011001");
      else
        w.emit(s, (t * 17) & 8191);
    }
    w.emit_real(real, double(t % 4) + 0.25);
    w.emit_varlen(bytes, view("value-" + std::to_string(t % 13)));
    w.emit_bit(event, 1);
    w.emit_bit(event, 1);
    if (t % 10 == 0) {
      auto a = w.begin_tx(g0, t + 50), b = w.begin_tx(g1, t);
      w.tx_parent(b, a);
      for (size_t i = 0; i < values.size(); ++i)
        w.tx_attr(a, keys[i], values[i]);
      std::array<Attribute, 1> event_attrs{{{label, Value::text("event")}}},
          stage_attrs{{{label, Value(t)}}},
          relation_attrs{{{label, Value(true)}}};
      w.tx_event(a, t, label, event_attrs);
      w.tx_stage(a, stage, lane, t + 1, t + 5, stage_attrs);
      w.relate(relation, a, b, relation_attrs);
      w.end_tx(b, t + 2, 2);
      w.end_tx(a, t + 60, 1);
      std::array<Value, 3> args{Value::text("message"), Value(std::int64_t(-5)),
                                Value(2.5)};
      w.log(site, t, args);
    }
  }
  w.close();
}
} // namespace
int main(int argc, char **argv) {
  try {
    if (argc < 3)
      return 2;
    if (std::string_view(argv[1]) == "activity" ||
        std::string_view(argv[1]) == "index") {
      activity(argv[2], std::string_view(argv[1]) == "index");
      return 0;
    }
    if (std::string_view(argv[1]) == "write")
      write(argv[2], static_cast<Codec>(std::stoi(argv[3])));
    digest(argv[2]);
    return 0;
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
