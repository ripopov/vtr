#include "engine/container.h"
#include "format.h"
#include "payload.h"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <source_location>

namespace fs = std::filesystem;
namespace d = vtr::detail::wire::detail;
using namespace vtr::detail::engine;

void check(bool good, const char* why) {
    if (!good) {
        throw std::runtime_error(why);
    }
}

template <class F>
void rejects(F&& action, std::source_location at = std::source_location::current()) {
    try {
        action();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("malformed input accepted at line " + std::to_string(at.line()));
}

Bytes read_file(const fs::path& path) {
    std::ifstream in(path, std::ios::binary);
    return Bytes(std::istreambuf_iterator<char>(in), {});
}

void write_file(const fs::path& path, View bytes) {
    std::ofstream out(path, std::ios::binary);
    out.write(reinterpret_cast<const char*>(bytes.data()), bytes.size());
    check(bool(out), "cannot write fixture");
}

void attributes() {
    for (unsigned tag = 0; tag <= 24; ++tag) {
        vtr::detail::wire::Value value;
        value.tag = static_cast<vtr::detail::wire::Tag>(tag);
        if (tag >= 2 && tag <= 5) {
            value.number = UINT64_MAX;
        }
        if (tag >= 6 && tag <= 9) {
            value.number = (std::uint64_t(1) << (8u << (tag - 6)) / 2) - 1;
        }
        if (tag == 10) {
            value.number = 0x7fc0beef;
        }
        if (tag == 11) {
            value.number = 0x7ff80000beef1234;
        }
        if (tag == 12 || tag == 19) {
            value.literal = 4;
        }
        if (tag == 13) {
            value = vtr::detail::wire::Value::text("typed Δ");
        }
        if (tag == 14) {
            value = vtr::detail::wire::Value::bytes(Bytes{0, 255});
        }
        if (tag >= 15 && tag <= 17) {
            d::put(value.payload, 3);
            value.payload.push_back(0);
            if (tag == 17) {
                value.payload.push_back(0);
            }
        }
        if (tag == 21 || tag == 22) {
            value.number = UINT64_MAX;
            value.scale = INT64_MIN;
        }
        if (tag == 23) {
            std::array values{vtr::detail::wire::Value(std::int64_t(-7)),
                              vtr::detail::wire::Value::text("x")};
            value = vtr::detail::wire::Value::list(values);
        }
        if (tag == 24) {
            d::put(value.payload, 1);
            d::put(value.payload, 1);
            d::encode_value(value.payload, vtr::detail::wire::Value::string(2));
        }
        d::validate_value(value, 5);
        Bytes encoded;
        d::encode_value(encoded, value);
        d::Cursor r(encoded);
        auto actual = d::decode_value(r);
        r.done();
        d::validate_value(actual, 5);
        check(actual.tag == value.tag && actual.number == value.number &&
                  actual.literal == value.literal && actual.scale == value.scale &&
                  actual.payload == value.payload,
              "attribute type or bits changed");
    }
    vtr::detail::wire::Value overflow(std::uint64_t(256));
    overflow.tag = vtr::detail::wire::Tag::u8;
    rejects([&] { d::validate_value(overflow, 1); });
    overflow.tag = vtr::detail::wire::Tag::i8;
    overflow.number = 128;
    rejects([&] { d::validate_value(overflow, 1); });
    vtr::detail::wire::Value map;
    map.tag = vtr::detail::wire::Tag::map;
    d::put(map.payload, 2);
    for (unsigned i = 0; i < 2; ++i) {
        d::put(map.payload, 1);
        d::encode_value(map.payload, vtr::detail::wire::Value());
    }
    rejects([&] { d::validate_value(map, 2); });
}

void container(const fs::path& dir) {
    auto path = dir / "container.vtr";
    Types types;
    auto schema = types.structure("Request", {{"address", 9}, {"name", 13}});
    Generators generators;
    GeneratorId gen;
    {
        vtr::detail::wire::Writer writer(path, {.codec = vtr::detail::wire::Codec::zstd,
                                                .compression_level = 1,
                                                .group_size = 8,
                                                .block_records = 32});
        writer.metadata().timescale = -9;
        writer.append_section(10, types.declarations());
        auto top = writer.add_scope(vtr::detail::wire::none, "top");
        auto wave = writer.add_var(top, "wave", {vtr::detail::wire::SignalKind::bits, 8, 2}).second;
        auto event =
            writer.add_var(top, "event", {vtr::detail::wire::SignalKind::bits, 1, 2}, 0).second;
        writer.add_alias(top, "wave_alias", wave);
        gen = writer.add_generator(top, "stage", "STAGE", schema);
        generators.declare({gen, RecordKind::stage, schema});
        for (unsigned t = 0; t < 80; ++t) {
            writer.set_time(t);
            writer.emit(wave, t / 4);
            writer.emit(event, 1);
            if (t == 40) {
                writer.add_generator(top, "late", "EVENT", 0);
            }
        }
        Payload payload;
        payload.scalar(std::uint64_t(0x1234));
        payload.append(std::string("same Δ"));
        std::vector<LocatedRecord> rows{
            {2, gen, 10, 20, 1, Bytes(payload.view().begin(), payload.view().end())},
            {3, gen, 12, 22, 1, Bytes(payload.view().begin(), payload.view().end())}};
        auto block = encode_record_block(14, types, generators, rows);
        writer.append_section(9, block, 10, 22, 14);
        writer.close();
    }
    ContainerReader reader(path, true);
    check(reader.types().declarations() == types.declarations(), "file datatypes changed");
    check(reader.block(14) && reader.block(14)->kind == 9 && !reader.block(13),
          "block map changed");
    check(!reader.waveform().stats().recovered, "normal file reported as recovered");
    reader.validate_record_blocks();
    std::size_t stages = 0, changes = 0, events = 0;
    reader.records({}, [&](const auto& r) {
        ++stages;
        check(r.parent == 1 && r.generator == gen, "record owner/location changed");
        return true;
    });
    auto wave = reader.waveform().find_signal("top.wave");
    auto event = reader.waveform().find_signal("top.event");
    check(reader.waveform().find_signal("top.wave_alias") == wave, "alias identity changed");
    reader.waveform().changes(0, UINT64_MAX, [&](auto time, auto signal, const auto& value) {
        if (signal == wave) {
            ++changes;
            check(value.as_u64() == time / 4, "signal transition changed");
        }
        if (signal == event) {
            ++events;
            check(value.as_u64() == 1, "event signal changed");
        }
        return true;
    });
    check(stages == 2 && changes == 20 && events == 80, "file lost records or event occurrences");
    auto value = reader.waveform().value_at(wave, 39);
    check(vtr::detail::wire::SignalValue{reader.waveform().signals()[wave], value}.as_u64() == 9,
          "point lookup lost preceding state");
    auto bytes = read_file(path);
    auto growing = dir / "growing.vtr";
    auto recovered = dir / "recovered.vtr";
    bool have_meta = false;
    for (const auto& section : reader.sections()) {
        if (section.kind == 1) {
            have_meta = true;
        }
        auto cut = section.offset + 24 + section.length;
        write_file(growing, View(bytes).first(cut));
        if (!have_meta) {
            rejects([&] { ContainerReader incomplete(growing, true); });
            rejects([&] { recover_container(growing, recovered); });
            continue;
        }
        ContainerReader partial(growing, true);
        check(partial.waveform().stats().recovered, "growing file reported complete");
        recover_container(growing, recovered);
        ContainerReader sealed(recovered, true);
        check(sealed.waveform().stats().recovered, "recovered directory hid incompleteness");
        check(sealed.waveform().stats().recovered_dropped == 0,
              "exact-boundary recovery invented dropped bytes");
        sealed.validate_record_blocks();
        check(partial.sections().size() == sealed.sections().size(),
              "recovery changed complete section count");
        for (std::size_t i = 0; i < partial.sections().size(); ++i) {
            check(partial.sections()[i].block_number == sealed.sections()[i].block_number,
                  "recovery renumbered blocks");
        }
    }
    const auto& last = reader.sections().back();
    auto corrupt = bytes;
    corrupt[last.offset + 24 + last.length - 1] ^= 0x80;
    write_file(growing, corrupt);
    recover_container(growing, recovered);
    ContainerReader recovered_corrupt(recovered, true);
    check(recovered_corrupt.sections().size() + 1 == reader.sections().size(),
          "CRC recovery retained damaged section");
    check(recovered_corrupt.waveform().stats().recovered_dropped == bytes.size() - last.offset,
          "recovery lost dropped-byte provenance");
    auto incomplete = last.offset + 24 + last.length - 1;
    write_file(growing, View(bytes).first(incomplete));
    recover_container(growing, recovered);
    ContainerReader truncated(recovered, true);
    check(truncated.sections().size() + 1 == reader.sections().size(),
          "recovery retained a partial section");
    rejects([&] { recover_container(path, path); });
}

void mixed_transactions(const fs::path& dir) {
    for (bool owner_first : {false, true}) {
        auto path = dir / (owner_first ? "owner-first.vtr" : "child-first.vtr");
        vtr::detail::wire::Writer writer(path);
        Types types;
        auto schema = types.structure("Diagnostic", {{"address", 9}, {"message", 13}});
        writer.append_section(10, types.declarations());
        auto scope = writer.add_scope(vtr::detail::wire::none, "top");
        auto ordinary = writer.add_generator(scope, "instruction", "INSTRUCTION");
        auto stage = writer.add_generator(scope, "execute", "STAGE");
        auto log = writer.add_generator(scope, "diagnostics", "LOG");
        auto callsite = schema;
        auto clock = writer.add_generator(scope, "clock", "CLOCK");
        auto event_name = writer.intern("local_note"), stage_name = writer.intern("local_wait"),
             lane = writer.intern("main");
        auto period_key = writer.intern("vtr.period"), dependency = writer.intern("depends_on");
        Generators generators;
        generators.declare({ordinary, RecordKind::ordinary, 0});
        generators.declare({stage, RecordKind::stage, 0});
        generators.declare({log, RecordKind::log, 0});

        generators.declare({clock, RecordKind::clock, 0});
        Transaction owner;
        owner.id = 1;
        owner.generator = ordinary;
        owner.begin = 0;
        owner.end = 20;
        owner.events.push_back({2, 5, event_name, {}});
        owner.stages.push_back({3, stage_name, lane, 3, 10, {}});
        owner.child_blocks = {50};
        Transaction stretch;
        stretch.id = 6;
        stretch.generator = clock;
        stretch.begin = 0;
        stretch.end = 10;
        stretch.attributes = {{period_key, vtr::detail::wire::Value::time(5)}};
        std::array txs{owner, stretch};
        std::array relations{Relation{dependency, 2, 4, {}}};
        Payload diagnostic;
        diagnostic.scalar(std::uint64_t(42));
        diagnostic.append(std::string("retry"));
        std::array children{LocatedRecord{4, stage, 6, 8, 1, {}},
                            LocatedRecord{5, log, 7, 7, 1, std::move(diagnostic).take(), callsite}};
        auto tx_block = encode_transaction_block(51, types, generators, txs, relations, {},
                                                 writer.string_count());
        auto record_block =
            encode_record_block(50, types, generators, children, {}, writer.string_count());
        auto append_owner = [&] { writer.append_section(5, tx_block, 0, 20, 51); };
        auto append_child = [&] { writer.append_section(9, record_block, 6, 8, 50); };
        if (owner_first) {
            append_owner();
            append_child();
        } else {
            append_child();
            append_owner();
        }
        writer.close();
        ContainerReader reader(path, true);
        reader.validate_record_blocks();
        reader.validate_transaction_blocks();
        unsigned owners = 0, records = 0, edges = 0;
        reader.transactions({}, [&](const auto& tx) {
            ++owners;
            if (tx.id == 1) {
                check(tx.events[0].id == 2 && tx.stages[0].id == 3 &&
                          tx.child_blocks == std::vector<std::uint64_t>{50},
                      "embedded identity or child locator changed");
            }
            return true;
        });
        reader.records({}, [&](const auto&) {
            ++records;
            return true;
        });
        reader.relations(2, false, [&](const auto& r) {
            ++edges;
            check(r.destination == 4, "embedded relation endpoint changed");
            return true;
        });
        check(owners == 2 && records == 2 && edges == 1, "mixed file lost observations");
        auto first_block =
            *std::find_if(reader.sections().begin(), reader.sections().end(),
                          [](const auto& section) { return section.block_number != 0; });
        auto bytes = read_file(path);
        auto partial_path = dir / "mixed-partial.vtr", sealed_path = dir / "mixed-sealed.vtr";
        write_file(partial_path, View(bytes).first(first_block.offset + 24 + first_block.length));
        recover_container(partial_path, sealed_path);
        ContainerReader sealed(sealed_path, true);
        check(sealed.waveform().stats().recovered && sealed.block(first_block.block_number),
              "mixed recovery hid incompleteness");
        check(!sealed.block(owner_first ? 50 : 51), "mixed recovery invented missing block");
        owners = records = edges = 0;
        sealed.transactions({}, [&](const auto&) {
            ++owners;
            return true;
        });
        sealed.records({}, [&](const auto&) {
            ++records;
            return true;
        });
        sealed.relations(2, false, [&](const auto&) {
            ++edges;
            return true;
        });
        check(owner_first ? owners == 2 && records == 0 && edges == 1
                          : owners == 0 && records == 2 && edges == 0,
              "mixed recovery discarded surviving references or records");
    }
}

void time_before_signals(const fs::path& dir) {
    namespace w = vtr::detail::wire;
    for (unsigned workers : {0u, 2u}) {
        const auto empty = dir / ("time-without-signals-" + std::to_string(workers) + ".vtr");
        {
            w::Writer writer(empty, {.workers = workers});
            for (unsigned time = 0; time < 4096; ++time) {
                writer.set_time(time);
                if (time % 257 == 0)
                    writer.flush();
            }
            check(writer.time() == 4095 && writer.wave_buffer_bytes().first <= 16 &&
                      writer.wave_buffer_bytes().second <= 16,
                  "signal-free time exceeded its bounded range storage");
            rejects([&] { writer.set_time(4094); });
            writer.close();
        }
        w::Reader no_signals(empty, {.verify_crc = true});
        check(no_signals.signals().empty() &&
                  no_signals.stats().time_range == std::pair<w::Time, w::Time>{0, 4095},
              "signal-free recording lost its captured time range");
        const auto late = dir / ("late-first-signal-" + std::to_string(workers) + ".vtr");
        w::Id signal;
        {
            w::Writer writer(late, {.workers = workers});
            writer.set_time(10);
            writer.dump(false);
            writer.set_time(20);
            writer.flush();
            rejects([&] { writer.set_time(19); });
            writer.set_time(30);
            signal = writer.add_var(w::none, "late", {w::SignalKind::bits, 8, 2}).second;
            writer.dump(true);
            writer.emit(signal, 5);
            writer.set_time(40);
            writer.emit(signal, 7);
            writer.set_time(50);
            writer.close();
        }
        w::Reader reader(late, {.verify_crc = true});
        check(reader.stats().time_range == std::pair<w::Time, w::Time>{10, 50},
              "late signal declaration lost the recording time range");
        std::vector<std::pair<w::Time, std::uint64_t>> changes;
        reader.changes(0, UINT64_MAX, [&](auto time, auto id, const auto& value) {
            check(id == signal, "late signal changed its identity");
            changes.emplace_back(time, value.as_u64());
            return true;
        });
        check(changes == std::vector<std::pair<w::Time, std::uint64_t>>{{30, 5}, {40, 7}},
              "time-only advances changed the first signal's waveform");
        auto value = reader.value_at(signal, 35);
        check(w::SignalValue{reader.signals()[signal], value}.as_u64() == 5,
              "late signal point query lost its first value");
    }
}

void declaration_order(const fs::path& dir) {
    auto future_schema = dir / "future-schema.vtr";
    {
        vtr::detail::wire::Writer writer(future_schema);
        Types types;
        auto type = types.structure("Late", {});
        writer.add_generator(vtr::detail::wire::none, "source", "EVENT", type);
        writer.flush();
        writer.append_section(10, types.declarations());
        writer.close();
    }
    rejects([&] { ContainerReader invalid(future_schema, true); });
    auto future_string = dir / "future-string.vtr";
    {
        vtr::detail::wire::Writer writer(future_string);
        Types types;
        Generators generators;
        auto gen = writer.add_generator(vtr::detail::wire::none, "source", "EVENT", 12);
        generators.declare({gen, RecordKind::event, 12});
        std::array rows{LocatedRecord{2, gen, 0, 0, 1, capture(std::uint32_t(50))}};
        auto block = encode_record_block(1, types, generators, rows);
        writer.append_section(9, block, 0, 0, 1);
        for (unsigned i = 0; i < 60; ++i) {
            writer.intern("late-" + std::to_string(i));
        }
        writer.close();
    }
    ContainerReader invalid(future_string, true);
    rejects([&] { invalid.records({}, [](const auto&) { return true; }); });
}

void budgeted_text(const std::filesystem::path& directory) {
    const auto path = directory / "budgeted-text.vtr";
    vtr::detail::wire::Writer writer(path);
    std::size_t remaining = 6;
    const auto first = writer.intern_with_budget("abcd", remaining);
    check(first && remaining == 2, "shared text admission lost its byte charge");
    check(writer.intern_with_budget("abcd", remaining) == first && remaining == 2,
          "shared text repetition consumed its budget twice");
    check(!writer.intern_with_budget("oversized", remaining) && remaining == 2,
          "shared text exceeded its byte budget");
    check(writer.intern_with_budget("xy", remaining).has_value() && !remaining,
          "shared text rejected the exact budget");
    check(!writer.intern_with_budget("z", remaining), "shared text admitted beyond its budget");
    const auto declared = writer.intern("existing");
    check(writer.intern_with_budget("existing", remaining) == declared &&
              writer.intern_with_budget("abcd", remaining) == first && !remaining,
          "shared text stopped reusing existing IDs when its budget was exhausted");
    writer.close();
    vtr::detail::wire::Reader reader(path);
    check(reader.strings().size() == 4, "denied shared text entered the string table");
}

void interned_text_lifetime(const std::filesystem::path& directory) {
    const auto path = directory / "interned-text.vtr";
    vtr::detail::wire::Writer writer(path);
    std::vector<std::pair<vtr::detail::wire::Id, std::string>> expected;
    std::vector<std::string_view> retained;
    auto add = [&](std::string_view value) {
        const auto copy = std::string(value);
        const auto id = writer.intern(value);
        expected.emplace_back(id, copy);
        retained.push_back(writer.string(id));
        return id;
    };
    check(add("") == 0, "empty text lost its reserved ID");
    add(std::string("before\0after", 12));
    for (unsigned i = 0; i < 4096; ++i) {
        auto value = std::to_string(i) + ":" +
                     std::string(i % 31 == 0 ? 10000 + i : i % 63, char('a' + i % 26));
        if (i % 7 == 0)
            value += " \xce\xbb";
        const auto id = add(value);
        value.assign(value.size(), 'x');
        check(writer.string(id) == expected.back().second, "interned text borrowed caller storage");
    }
    const auto alias = writer.string(expected[33].first).substr(2, 257);
    add(alias);
    add(writer.string(expected[2].first).substr(0, 4097));
    const auto count = writer.string_count();
    rejects([&] { writer.intern(std::string_view("\xc0\xaf", 2)); });
    check(writer.string_count() == count, "rejected text changed the ID index");
    for (std::size_t i = 0; i < expected.size(); ++i) {
        const auto& [id, text] = expected[i];
        check(writer.string(id) == text && retained[i] == text,
              "interned text changed after index or chunk growth");
        check(writer.intern(text) == id, "dictionary growth lost an existing text ID");
    }
    check(writer.string_count() == count, "reinterning duplicated existing text");
    writer.close();
    vtr::detail::wire::Reader reader(path);
    check(reader.strings().size() == count, "persisted text count changed");
    for (const auto& [id, text] : expected)
        check(reader.strings()[id] == text, "persisted text bytes changed");
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: container_test DIRECTORY");
        }
        fs::create_directories(argv[1]);
        budgeted_text(argv[1]);
        interned_text_lifetime(argv[1]);
        attributes();
        container(argv[1]);
        mixed_transactions(argv[1]);
        time_before_signals(argv[1]);
        declaration_order(argv[1]);
        std::cout << "Validated version-3 container, shared datatypes, attribute widths, "
                     "waveform/event occurrence history, mixed sections, late generators, "
                     "directory/block maps, and truncation/CRC recovery\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
