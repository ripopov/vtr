#include "vtr_reader.h"
#include "vtr_writer.h"
#include <bit>
#include <fstream>
#include <iostream>

namespace vt = vtr;

struct Packet {
    std::uint64_t pc;
    std::string description;
    std::vector<std::int16_t> data;
    friend bool operator==(const Packet&, const Packet&) = default;
};

template <> struct vt::Schema<Packet> {
    static constexpr std::string_view name = "Packet";
    static constexpr auto fields =
        std::tuple{vt::field("pc", &Packet::pc), vt::field("description", &Packet::description),
                   vt::field("data", &Packet::data)};
};

Packet packet() {
    return {0x8000, "packet Δ", {-32768, 0, 32767}};
}

void check(bool condition) {
    if (!condition)
        throw std::runtime_error("cross-language trace mismatch");
}

void write(const char* path, unsigned codec, unsigned workers, std::string_view mode) {
    vt::WriterOptions options;
    options.mode = mode == "inline" ? vt::ExecutionMode::inline_ : vt::ExecutionMode::threaded;
    options.encoding.compression = static_cast<vt::Codec>(codec);
    options.encoding.workers = workers;
    options.encoding.block_bytes = 512;
    options.encoding.page_bytes = 128;
    options.encoding.waveform_records = 4;
    options.meta.timescale = -9;
    vt::Writer writer(path, options);
    const auto label = writer.intern("vtr.label"), name = writer.intern("embedded");
    const auto lane = writer.intern("lane"), relation = writer.intern("causes");
    const auto core = writer.scope("core");
    auto requests = writer.transactions<Packet>({.name = "requests", .parent = core}, "REQUEST");
    auto scalar = writer.transactions<std::array<std::uint64_t, 3>>(
        {.name = "scalar", .parent = core}, "SCALAR");
    auto execute = writer.stages<std::string>({.name = "execute", .parent = core});
    auto events = writer.events({.name = "retired", .parent = core});
    auto logger = writer.logger({.name = "diagnostics"});
    auto logs = writer.callsite<Packet>({.format = "packet {}"});
    auto retry = writer.callsite<std::string>(
        {.format = "retry {}", .severity = 3, .file = "shared.cpp", .line = 17});
    auto memory_logger = writer.logger({.name = "memory"});
    auto clock = writer.clock({.name = "clock", .parent = core});
    writer.associate_clock(requests, clock);
    auto counter = writer.signal({.name = "counter", .parent = core}, {.width = 32});
    (void)writer.alias(counter, {.name = "alias", .parent = core});
    auto logic = writer.signal({.name = "logic", .parent = core},
                               {.width = 4, .alphabet = vt::States::four});
    auto real =
        writer.signal({.name = "real", .parent = core}, {.encoding = vt::SignalEncoding::real});
    auto wide = writer.signal({.name = "wide", .parent = core}, {.width = 129});
    writer.advance_time(0);
    writer.run(clock, 0, 2);
    writer.emit(std::array{vt::SignalUpdate{counter, 0}, vt::SignalUpdate{counter, 1}});
    const vt::Bytes states{std::byte{0xe4}};
    writer.emit_packed(logic, states);
    writer.emit_real(real, std::bit_cast<double>(std::uint64_t{0x7ff800000000beef}));
    const std::array<std::uint32_t, 5> words{0x12345678, 0x90abcdef, 0x76543210, 0xfedcba98,
                                             UINT32_MAX};
    writer.emit_words(wide, words, 129);
    writer.advance_time(10);
    writer.emit(counter, 2);
    writer.emit_real(real, -0.0);
    auto owner = writer.begin(requests);
    writer.payload(owner, packet());
    writer.attribute(owner, label, vt::Value(std::string("instruction Δ")));
    auto visit = writer.begin_at(execute, owner, 11);
    writer.payload(visit, std::string("replaced stage"));
    (void)writer.embedded_event(owner, 12, name);
    (void)writer.embedded_stage(owner, 100, name, lane);
    auto child = writer.begin_at(requests, 12, packet(), {.parent = owner});
    writer.payload(child, Packet{1, "replaced owner", {}});
    writer.advance_time(12);
    writer.dump(false);
    writer.dump(false);
    writer.emit(counter, 999);
    writer.emit_packed(logic, vt::Bytes{std::byte{0}});
    writer.emit_real(real, 1.0);
    writer.emit_words(wide, std::array<std::uint32_t, 5>{}, 129);
    writer.advance_time(14);
    writer.emit(counter, 3);
    (void)writer.event(events, owner);
    (void)writer.event(events, owner);
    (void)writer.log(logger, logs, packet(), owner);
    writer.stop(clock, 10);
    writer.end(visit, std::string("execute Δ"));
    writer.advance_time(15);
    writer.dump(true);
    writer.dump(true);
    writer.emit(counter, 2);
    writer.end(owner, vt::Status::ok);
    writer.relate(relation, owner.id(), child.id(), {});
    writer.advance_time(20);
    writer.emit(counter, 4);
    writer.end(child, packet(), vt::Status::ok);
    writer.flush();
    for (std::uint64_t time = 21; time < 37; ++time) {
        writer.advance_time(time);
        writer.emit(counter, time);
        auto tx = writer.begin(scalar);
        if (time % 2 == 0) {
            writer.payload(tx, std::array<std::uint64_t, 3>{time, time * 2, UINT64_MAX});
            writer.end(tx, vt::Status::ok);
        } else {
            writer.end(tx, std::array<std::uint64_t, 3>{time, time * 2, UINT64_MAX},
                       vt::Status::ok);
        }
    }
    writer.advance_time(40);
    writer.log(logger, retry, std::string("bank 3"));
    writer.log(logger, retry, std::string("bank 3"));
    writer.log(memory_logger, logs, packet());
    writer.log(logger, logs, packet());
    writer.close();
}

void read(const char* path) {
    std::optional<vt::Record> retained;
    std::optional<vt::Record> retained_log;
    std::optional<vt::PayloadView> retained_arguments;
    {
        vt::Reader reader(path);
        const auto report = reader.validate();
        check(report.records == 29 && report.relations == 1 && report.missing.empty());
        auto owner = reader.lookup(vt::RecordId(2));
        check(owner.has_value());
        retained = owner;
        check(owner->payload().as<Packet>() == packet() && owner->begin() == 10 &&
              owner->end() == 15);
        check(reader.expand(owner->id()).located.size() == 3);
        check(reader.lookup(vt::RecordId(3))->payload().as<std::string>() == "execute Δ");
        check(reader.lookup(vt::RecordId(5))->end() == 100);
        check(reader.lookup(vt::RecordId(6))->parent() == owner->id());
        check(reader.lookup(vt::RecordId(9))->payload().as<Packet>() == packet());
        check(reader.format_log(*reader.lookup(vt::RecordId(9))) == "packet 32768");
        check(reader.callsites().size() == 2);
        auto retry = reader.lookup(vt::RecordId(26));
        auto duplicate = reader.lookup(vt::RecordId(27));
        retained_log = retry;
        retained_arguments = reader.lookup(vt::RecordId(9))->payload()["data"];
        check(retry->generator() == reader.find_generator("diagnostics") &&
              retry->payload().as<std::string>() == "bank 3");
        check(retry->callsite() == duplicate->callsite() && retry->id() != duplicate->id());
        check(reader.lookup(vt::RecordId(28))->generator() == reader.find_generator("memory"));
        check(reader.lookup(vt::RecordId(28))->callsite() ==
              reader.lookup(vt::RecordId(9))->callsite());
        unsigned selected = 0;
        reader.records({.generator = retry->generator(), .callsite = retry->callsite()},
                       [&](const vt::Record& record) {
                           check(reader.format_log(record) == "retry bank 3");
                           ++selected;
                           return vt::Flow::next;
                       });
        check(selected == 2);
        unsigned count = 0;
        reader.transactions(
            {.generator = reader.find_generator("core.scalar")}, [&](const vt::Record& record) {
                check(record.payload().as<std::array<std::uint64_t, 3>>() ==
                      std::array<std::uint64_t, 3>{record.begin(), record.begin() * 2, UINT64_MAX});
                ++count;
                return vt::Flow::next;
            });
        check(count == 16);
        auto counter = *reader.find_signal("core.counter");
        check(reader.find_signal("core.alias") == counter);
        for (auto [time, expected] : std::array<std::pair<std::uint64_t, std::uint64_t>, 7>{
                 {{0, 1}, {9, 1}, {10, 2}, {14, 2}, {20, 4}, {30, 30}, {40, 36}}})
            check(reader.value_at(counter, time).as_u64() == expected);
        std::vector<std::pair<std::uint64_t, std::uint64_t>> counter_changes;
        reader.changes(counter, {}, [&](const vt::SignalChange& change) {
            counter_changes.emplace_back(change.time, change.value.as_u64());
            return vt::Flow::next;
        });
        std::vector<std::pair<std::uint64_t, std::uint64_t>> expected_changes{
            {0, 0}, {0, 1}, {10, 2}, {20, 4}};
        for (std::uint64_t time = 21; time < 37; ++time)
            expected_changes.emplace_back(time, time);
        check(counter_changes == expected_changes);
        check(reader.value_at(*reader.find_signal("core.logic"), 40).as_text() == "zx10");
        check(std::bit_cast<std::uint64_t>(
                  reader.value_at(*reader.find_signal("core.real"), 0).as_real()) ==
              0x7ff800000000beef);
        check(std::bit_cast<std::uint64_t>(
                  reader.value_at(*reader.find_signal("core.real"), 10).as_real()) ==
              std::bit_cast<std::uint64_t>(-0.0));
        auto wide = reader.value_at(*reader.find_signal("core.wide"), 40);
        check(wide.packed.size() == 17 && wide.packed[0] == std::byte{0x78} &&
              wide.packed[16] == std::byte{1});
    }
    check(retained->payload().as<Packet>() == packet());
    check(retained_log->callsite().has_value() &&
          retained_log->payload().as<std::string>() == "bank 3");
    check(retained_arguments->as<std::vector<std::int16_t>>() == packet().data);
    std::cout << "29 records, typed payloads, ownership, clocks, relations, waveforms and retained "
                 "schemas verified\n";
}

void index(const char* command, const char* trace_path, const char* index_path) {
    vt::Reader reader(trace_path);
    if (std::string_view(command) == "index-write") {
        auto bytes = vt::ActivityIndex::build(reader).encode();
        std::ofstream output(index_path, std::ios::binary);
        output.write(reinterpret_cast<const char*>(bytes.data()), bytes.size());
        check(bool(output));
    } else {
        std::ifstream input(index_path, std::ios::binary);
        std::vector<char> raw{std::istreambuf_iterator<char>(input), {}};
        auto loaded = vt::ActivityIndex::load(
            reader, {reinterpret_cast<const std::byte*>(raw.data()), raw.size()});
        auto expected = vt::ActivityIndex::build(reader);
        for (std::uint64_t first = 0; first <= 42; ++first)
            for (std::uint64_t last = first; last <= 42; ++last)
                check(loaded.active({first, last}) == expected.active({first, last}));
    }
}

int main(int argc, char** argv) {
    try {
        if (argc == 4 && std::string_view(argv[1]) == "recover")
            (void)vt::recover(argv[2], argv[3]);
        else if (argc == 4 && (std::string_view(argv[1]) == "index-read" ||
                               std::string_view(argv[1]) == "index-write"))
            index(argv[1], argv[2], argv[3]);
        else if (argc == 3 && std::string_view(argv[1]) == "read")
            read(argv[2]);
        else if (argc == 6 && std::string_view(argv[1]) == "write")
            write(argv[2], std::stoul(argv[3]), std::stoul(argv[4]), argv[5]);
        else
            throw std::runtime_error(
                "usage: typed_interop read PATH | write PATH CODEC WORKERS MODE");
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
