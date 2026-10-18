#include "format.hpp"
#include "vtr.hpp"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <random>
using namespace vtr;
using namespace vtr::detail;
void check(bool ok, const char *why) {
  if (!ok)
    throw std::runtime_error(why);
}
template <class F> void rejects(F &&f) {
  bool rejected = false;
  try {
    f();
  } catch (const Error &) {
    rejected = true;
  }
  check(rejected, "invalid operation was accepted");
}
Bytes read(const std::string &path) {
  std::ifstream f(path, std::ios::binary | std::ios::ate);
  auto size = f.tellg();
  check(size >= 0, "missing test file");
  Bytes bytes(size);
  f.seekg(0);
  f.read(reinterpret_cast<char *>(bytes.data()), bytes.size());
  return bytes;
}
void write_bytes(const std::string &path, View bytes) {
  std::ofstream f(path, std::ios::binary);
  f.write(reinterpret_cast<const char *>(bytes.data()), bytes.size());
  check(bool(f), "test file write failed");
}
int main(int argc, char **argv) {
  try {
    check(argc == 2, "expected temporary directory");
    std::mt19937 values(42);
    for (std::size_t width : {1, 2, 3, 4, 8, 12, 16, 32}) {
      Bytes original(width * 257);
      for (auto &b : original)
        b = static_cast<std::uint8_t>(values());
      for (unsigned x = 0; x < 4; ++x)
        check(transform(transform(original, width, x, false), width, x, true) ==
                  original,
              "value transform round trip");
    }
    auto path = (std::filesystem::path(argv[1]) / "native.vtr").string();
    WriterOptions options;
    options.block_records = 3;
    options.tx_block_bytes = 256;
    {
      Writer w(path, options);
      auto top = w.add_scope(none, "top");
      auto s = w.add_var(top, "s", {SignalKind::bits, 8, 2}).second;
      auto event = w.add_var(top, "event", {SignalKind::bits, 1, 2}, 0).second;
      auto gen = w.add_generator(top, "request", "tx");
      auto key = w.intern("key"), stage = w.intern("stage");
      auto old = w.begin_tx(gen, 9000);
      w.tx_attr(old, key, std::uint64_t(1));
      rejects([&] { w.tx_attr(old, key, std::uint64_t(2)); });
      for (std::uint64_t i = 0; i < 5000; ++i) {
        auto id = w.begin_tx(gen, i);
        w.end_tx(id, i + 1);
      }
      w.tx_stage(old, stage, 0, 9000, {});
      w.end_tx(old, 9001);
      rejects([&] { w.end_tx(old, 9002); });
      w.set_time(10);
      w.emit(s, 0);
      w.emit(s, 0);
      w.emit_bit(event, 1);
      w.emit_bit(event, 1);
      w.set_time(11);
      std::array<std::uint8_t, 1> b{7};
      w.emit_packed(s, 2, b);
      w.emit(s, 7);
      w.emit(s, 8);
      rejects([&] { w.set_time(9); });
      w.flush();
      auto late = w.add_var(top, "late", {SignalKind::bits, 13, 4}).second;
      w.set_time(20);
      w.emit_logic(late, "x0011z0011001");
      w.close();
    }
    std::shared_ptr<const SignalData> surviving;
    {
      Reader r(path);
      auto s = r.find_signal("top.s"), event = r.find_signal("top.event"),
           late = r.find_signal("top.late");
      surviving = r.load_signal(s);
      check(surviving->times == std::vector<Time>({10, 11, 11}),
            "level dedup or same-time order");
      check(r.load_signal(event)->times == std::vector<Time>({10, 10}),
            "event occurrences lost");
      check(r.load_signal(s).get() == surviving.get(),
            "history storage not shared");
      auto before = r.value_at(late, 19);
      check(SignalValue{r.signals()[late], before}.ascii() == "xxxxxxxxxxxxx",
            "late signal default");
      auto after = r.value_at(late, 20);
      check(SignalValue{r.signals()[late], after}.ascii() == "x0011z0011001",
            "late signal frame");
      std::size_t count = 0;
      r.transactions({}, [&](auto &t) {
        ++count;
        if (t.id == 1) {
          check(t.attrs.size() == 1 && t.attrs[0].value.number == 1,
                "duplicate attr changed value");
          check(t.stages[0].end == 9001, "open stage not ended");
        }
        return true;
      });
      check(count == 5001, "long-lived open transaction collision");
      r.clear_cache();
      check(surviving->at(2).as_u64() == 8, "history lifetime");
    }
    check(surviving->at(2).as_u64() == 8, "history did not survive reader");
    auto bytes = read(path);
    auto corrupt = bytes;
    corrupt[0] ^= 1;
    write_bytes(path + ".bad", corrupt);
    rejects([&] { Reader r(path + ".bad"); });
    corrupt = bytes;
    corrupt[8] = 3;
    write_bytes(path + ".bad", corrupt);
    rejects([&] { Reader r(path + ".bad"); });
    auto dir = load<std::uint64_t>(View(bytes).subspan(bytes.size() - 24));
    std::size_t block = 32;
    while (load<Id>(View(bytes).subspan(block)) != 4)
      block += 24 + load<std::uint64_t>(View(bytes).subspan(block + 8));
    corrupt = bytes;
    corrupt[block + 16] ^= 1;
    write_bytes(path + ".bad", corrupt);
    {
      Reader fast(path + ".bad");
      check(!fast.load_signal(0)->times.empty(),
            "default reader scanned block CRC");
      Reader checked(path + ".bad", {.verify_crc = true});
      rejects([&] { checked.load_signal(0); });
    }
    write_bytes(path + ".bad", View(corrupt).first(dir));
    {
      Reader recovered(path + ".bad");
      check(recovered.stats().recovered &&
                recovered.stats().recovered_dropped > 0,
            "recovery accepted damaged section CRC");
      check(recovered.stats().signal_blocks == 0,
            "recovery passed a corrupt block");
    }
    write_bytes(path + ".partial", View(bytes).first(dir));
    {
      Reader r(path + ".partial");
      check(r.stats().recovered, "unsealed file not marked recovered");
      check(r.load_signal(0)->times.size() == 3,
            "recovery dropped completed block");
    }
    recover(path + ".partial", path + ".recovered");
    {
      Reader r(path + ".recovered");
      check(!r.stats().recovered && r.ending().kind == Ending::recovered,
            "recovery not sealed");
      check(r.load_signal(0)->times.size() == 3, "recovery changed signals");
      check(r.transaction(1,
                          [](auto &t) {
                            check(t.id == 1 && t.end == 9001,
                                  "recovery changed transaction id");
                          }),
            "recovery lost transaction");
    }
    {
      Writer invalid(path + ".invalid");
      invalid.metadata().comment = std::string(1, char(0xff));
      rejects([&] { invalid.flush(); });
      invalid.metadata().comment = "valid UTF-8";
      invalid.close();
      Reader valid(path + ".invalid");
      check(valid.metadata().comment == "valid UTF-8",
            "metadata UTF-8 validation");
    }
    auto logged = path + ".log";
    {
      Writer w(logged);
      auto scope = w.add_scope(none, "log");
      std::array<Tag, 3> types{Tag::u64, Tag::f64, Tag::text};
      auto site =
          w.add_log_site(scope, 2, "hex={:#06x} real={:.2f} {{ {} }}", types);
      std::array<Value, 3> args{Value(std::uint64_t(42)), Value(1.25),
                                Value::text("ok")};
      w.log(site, 4, args);
      w.close();
    }
    {
      Reader r(logged);
      r.logs({}, 0, [&](auto &record) {
        check(r.format_log(record) == "hex=0x002a real=1.25 { ok }",
              "log formatting");
        return true;
      });
    }
    {
      auto crowded = path + ".crowded";
      Writer w(crowded);
      auto scope = w.add_scope(none, "crowded");
      auto gen = w.add_generator(scope, "requests", "request");
      std::vector<Id> keys;
      std::vector<std::uint64_t> ids;
      for (unsigned i = 0; i < 4096; ++i) {
        auto name = "attribute-with-a-long-name-" + std::to_string(i);
        keys.push_back(w.intern(name));
        ids.push_back(w.begin_tx(gen, i));
        w.tx_attr(ids.back(), keys.back(), std::uint64_t(i));
      }
      for (unsigned i = 0; i < keys.size(); ++i)
        check(w.intern("attribute-with-a-long-name-" + std::to_string(i)) ==
                  keys[i],
              "interned key changed after growth");
      std::shuffle(ids.begin(), ids.end(), values);
      for (auto id : ids) {
        w.tx_attr(id, w.intern("completion"), std::uint64_t(id));
        w.end_tx(id, 10000);
      }
      w.close();
      Reader r(crowded);
      std::vector<bool> seen(ids.size());
      r.transactions({}, [&](auto &t) {
        check(t.id > 0 && t.id <= seen.size() && !seen[t.id - 1],
              "transaction identity lost during erase");
        seen[t.id - 1] = true;
        check(t.attrs.size() == 2 && t.attrs[0].key == keys[t.id - 1] &&
                  t.attrs[0].value.number == t.id - 1 &&
                  t.attrs[1].value.number == t.id && t.end == 10000,
              "transaction attributes lost during growth or erase");
        return true;
      });
      check(std::all_of(seen.begin(), seen.end(), [](bool v) { return v; }),
            "open transaction lost");
    }
    {
      auto aliased = path + ".aliases";
      WriterOptions opts;
      opts.group_size = 16;
      opts.block_records = 4096;
      Writer w(aliased, opts);
      auto scope = w.add_scope(none, "aliases");
      for (unsigned i = 0; i < 257; ++i)
        w.add_var(scope, "s" + std::to_string(i), {SignalKind::bits, 8, 2});
      for (Time t = 0; t < 300; ++t) {
        w.set_time(t);
        for (Id signal = 0; signal < 257; ++signal)
          w.emit(signal, (t + signal % 3) % 256);
      }
      w.close();
      Reader r(aliased);
      std::vector<unsigned> positions(257);
      r.changes(0, forever, [&](Time t, Id signal, SignalValue value) {
        check(t == positions.at(signal)++ &&
                  value.as_u64() == (t + signal % 3) % 256,
              "cross-group alias lost or reordered changes");
        return true;
      });
      check(std::all_of(positions.begin(), positions.end(),
                        [](auto n) { return n == 300; }),
            "alias fan-out lost a signal");
      auto retained = r.load_signal(0);
      for (Id signal = 0; signal < 257; ++signal) {
        auto history = r.load_signal(signal);
        check(history->times.size() == 300 &&
                  history->at(299).as_u64() == (299 + signal % 3) % 256 &&
                  SignalValue{history->kind, r.value_at(signal, 255)}.as_u64() ==
                      (255 + signal % 3) % 256,
              "selective alias history changed");
      }
      r.clear_cache();
      check(retained->at(256).as_u64() == 0,
            "cache growth or clear invalidated retained history");
      unsigned callbacks = 0;
      r.changes(0, forever, [&](Time, Id, SignalValue) { return ++callbacks < 3; });
      check(callbacks == 3, "alias scan ignored callback cancellation");
    }
    // Mutations exercise checked lengths, identifiers, checksums, and recovery
    // under sanitizers.
    std::mt19937 random(1729);
    for (unsigned i = 0; i < 128; ++i) {
      corrupt = bytes;
      auto at = random() % corrupt.size();
      corrupt[at] ^= 1u << (random() % 8);
      write_bytes(path + ".bad", corrupt);
      try {
        Reader r(path + ".bad");
        r.changes(0, forever, [](Time, Id, SignalValue) { return true; });
        r.transactions({}, [](auto &) { return true; });
      } catch (const Error &) {
      }
    }
    std::cout << "native: ok\n";
    return 0;
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
