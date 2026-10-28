#include "engine/vtr.h"
#include "format.h"
#include "payload.h"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <source_location>

using namespace vtr::detail::engine;
namespace fs = std::filesystem;

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
    throw std::runtime_error("invalid full-file invariant accepted at line " +
                             std::to_string(at.line()));
}

void write_case(const fs::path& path, const std::string& name) {
    vtr::detail::wire::Writer core(path);
    Types types;
    TypeId payload_type = 0;
    if (name == "attribute-collision") {
        std::vector<Field> fields;
        fields.push_back({"address", 9});
        payload_type = types.structure("Request", std::move(fields));
    }
    if (name == "datatype-metadata") {
        const auto kind = core.intern("kind"), severity = core.intern("log.severity");
        payload_type = types.structure("Opaque", {},
                                       {{kind, Value::text("callsite")}, {severity, Value(true)}});
    }
    if (types.size() > first_declared_type) {
        core.append_section(10, types.declarations());
    }
    auto scope = core.add_scope(none, "top");
    auto ordinary = core.add_generator(scope, "owner", "TRANSACTION", payload_type);
    auto event = core.add_generator(scope, "event", "EVENT");
    auto log = core.add_generator(scope, "log", "LOG");
    auto datatype = TypeId{0};
    auto clock = core.add_generator(scope, "clock", "CLOCK", name == "clock-schema" ? 9 : 0);
    auto period = core.intern("vtr.period"), edge = core.intern("dependency"),
         label = core.intern("vtr.label"), address = core.intern("address");
    if (name == "clock-schema") {
        core.close();
        return;
    }
    if (name == "clock-association-missing") {
        Value value(std::uint64_t(999));
        value.tag = Tag::u32;
        core.node_attr(ordinary, core.intern("vtr.clock"), value);
    }
    if (name == "clock-association-wrong-kind") {
        Value value{std::uint64_t(scope)};
        value.tag = Tag::u32;
        core.node_attr(ordinary, core.intern("vtr.clock"), value);
    }
    Generators generators;
    generators.declare({ordinary, RecordKind::ordinary, payload_type});
    generators.declare({event, RecordKind::event, 0});
    generators.declare({log, RecordKind::log, 0});

    generators.declare({clock, RecordKind::clock, 0});
    Transaction root;
    root.id = 1;
    root.generator = ordinary;
    root.begin = 0;
    root.end = 10;
    std::vector<Transaction> txs;
    std::vector<LocatedRecord> rows;
    std::vector<Relation> relations;
    if (name == "attribute-collision") {
        root.payload = capture(std::uint64_t(42));
        root.attributes = {{address, Value(std::uint64_t(42))}};
    }
    if (name == "label-type") {
        root.attributes = {{label, Value(std::uint64_t(42))}};
    }
    if (name == "duplicate-id") {
        root.events.push_back({2, 5, label, {}});
        root.child_blocks = {2};
        rows.push_back({2, event, 5, 5, 1, {}});
    }
    if (name == "locator-missing") {
        rows.push_back({2, event, 5, 5, 1, {}});
    }
    if (name == "locator-extra-log") {
        root.child_blocks = {2};
        rows.push_back({2, log, 5, 5, 1, {}, datatype});
    }
    if (name == "locator-wrong-kind") {
        root.child_blocks = {1};
    }
    if (name == "locator-unresolved") {
        root.child_blocks = {999};
    }
    if (name == "parent-wrong-kind") {
        root.generator = clock;
        root.end = 0;
        rows.push_back({2, event, 0, 0, 1, {}});
    }
    if (name == "clock-period") {
        root.generator = clock;
        root.attributes = {{period, Value::time(4)}};
    }
    if (name == "clock-overlap") {
        root.generator = clock;
        root.attributes = {{period, Value::time(5)}};
        auto second = root;
        second.id = 2;
        second.begin = 10;
        second.end = 20;
        txs.push_back(second);
    }
    if (name == "parent-missing") {
        auto child = root;
        child.id = 3;
        child.parent = 2;
        txs.push_back(child);
    }
    if (name == "relation-unresolved") {
        relations.push_back({edge, 1, 999, {}});
    }
    if (name == "outside-endpoints") {
        relations.push_back({edge, 0, 0, {}});
    }
    txs.push_back(root);
    auto encoded =
        encode_transaction_block(1, types, generators, txs, relations, {}, core.string_count());
    auto end = name == "clock-overlap" ? 20 : root.end;
    core.append_section(5, encoded, 0, end, 1);
    if (!rows.empty()) {
        core.append_section(
            9, encode_record_block(2, types, generators, rows, {}, core.string_count()),
            rows[0].begin, rows[0].end, 2);
    }
    core.close();
}

void wave_case(const fs::path& path, const std::string& name) {
    namespace d = vtr::detail::wire::detail;
    vtr::detail::wire::Writer writer(path, {.codec = vtr::detail::wire::Codec::stored});
    writer.add_var(none, "signal", {SignalKind::bits, 3, 9});
    Bytes frame{0x22, 0x02}, column{7, 0, 0x10, 0x02};
    if (name == "frame-disagrees") {
        frame[0] = 0x21;
    }
    if (name == "frame-code") {
        frame[0] = 0x2f;
    }
    if (name == "frame-padding") {
        frame[1] = 0xf2;
    }
    if (name == "column-code") {
        column[2] = 0x1f;
    }
    if (name == "column-padding") {
        column[3] = 0xf2;
    }
    if (name == "compact-padding") {
        column = {6, 1, 0x81};
    }
    Bytes run;
    d::put(run, column.size());
    d::append(run, column);
    auto packed = d::compress(run, vtr::detail::wire::Codec::stored, 0);
    Bytes container;
    d::put(container, 1);
    d::put(container, 0);
    d::blob(container, d::compress(frame, vtr::detail::wire::Codec::stored, 0));
    d::put(container, 1);
    d::put(container, 1);
    d::put(container, 0);
    d::put(container, packed.size());
    d::append(container, packed);
    auto tt = d::compress(Bytes{1}, vtr::detail::wire::Codec::stored, 0);
    Bytes section;
    d::fixed<std::uint64_t>(section, 1);
    d::fixed<std::uint64_t>(section, 1);
    for (auto value : {1u, 1u, 256u, 1u, 1u, static_cast<unsigned>(tt.size())}) {
        d::fixed(section, value);
    }
    d::append(section, tt);
    d::fixed<std::uint32_t>(section, 0);
    d::fixed<std::uint32_t>(section, container.size());
    d::fixed<std::uint64_t>(section, name == "container-partition" ? 1 : 0);
    d::fixed<std::uint32_t>(section, name == "predecessor" ? 0 : none);
    d::append(section, container);
    writer.close();
    vtr::detail::wire::Reader skeleton(path);
    std::ifstream input(path, std::ios::binary);
    Bytes file(std::istreambuf_iterator<char>(input), {});
    auto directory_offset = d::load<std::uint64_t>(View(file).subspan(file.size() - 24, 8));
    file.resize(directory_offset);
    auto append_section = [&](std::uint32_t kind, View data) {
        d::fixed(file, kind);
        d::fixed<std::uint32_t>(file, 0);
        d::fixed<std::uint64_t>(file, data.size());
        d::fixed(file, d::checksum(data));
        d::fixed<std::uint32_t>(file, 0);
        d::append(file, data);
    };
    append_section(4, section);
    auto final_directory = file.size();
    Bytes directory;
    d::fixed<std::uint64_t>(directory, skeleton.sections().size() + 1);
    d::fixed<std::uint64_t>(directory, 0);
    for (const auto& entry : skeleton.sections()) {
        d::fixed(directory, entry.kind);
        d::fixed(directory, entry.flags);
        d::fixed(directory, entry.offset);
        d::fixed(directory, entry.length);
        d::fixed(directory, entry.aux0);
        d::fixed(directory, entry.aux1);
        d::fixed(directory, entry.block_number);
    }
    d::fixed<std::uint32_t>(directory, 4);
    d::fixed<std::uint32_t>(directory, 0);
    d::fixed<std::uint64_t>(directory, directory_offset);
    d::fixed<std::uint64_t>(directory, section.size());
    d::fixed<std::uint64_t>(directory, name == "directory-bounds" ? 0 : 1);
    d::fixed<std::uint64_t>(directory, 1);
    d::fixed<std::uint64_t>(directory, 0);
    append_section(7, directory);
    auto final_size = file.size() + 24;
    d::fixed<std::uint64_t>(file, final_directory);
    d::fixed<std::uint64_t>(file, final_size);
    d::append(file, d::trailer);
    std::ofstream output(path, std::ios::binary);
    output.write(reinterpret_cast<const char*>(file.data()), file.size());
    check(bool(output), "cannot write malformed waveform fixture");
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: full_validation_test DIRECTORY");
        }
        fs::create_directories(argv[1]);
        auto dir = fs::path(argv[1]);
        const auto opaque = dir / "datatype-metadata.vtr";
        write_case(opaque, "datatype-metadata");
        Reader(opaque).validate();
        for (std::string name :
             {"duplicate-id", "locator-missing", "locator-extra-log", "locator-wrong-kind",
              "parent-wrong-kind", "clock-period", "clock-overlap", "attribute-collision",
              "label-type", "clock-schema", "clock-association-wrong-kind"}) {
            auto path = dir / (name + ".vtr"), recovered = dir / (name + ".recovered.vtr");
            write_case(path, name);
            rejects([&] {
                Reader reader(path, {.verify_checksums = true});
                reader.validate();
            });
            recover_container(path, recovered);
            rejects([&] {
                Reader reader(recovered, {.verify_checksums = true});
                reader.validate();
            });
        }
        for (std::string name : {"parent-missing", "relation-unresolved", "locator-unresolved",
                                 "clock-association-missing"}) {
            auto path = dir / (name + ".vtr"), recovered = dir / (name + ".recovered.vtr");
            write_case(path, name);
            rejects([&] {
                Reader reader(path);
                reader.validate();
            });
            recover_container(path, recovered);
            Reader reader(recovered, {.verify_checksums = true});
            auto report = reader.validate();
            check(report.incomplete_file && report.missing.size() == 1,
                  "recovery hid or invented missing references");
            if (name == "locator-unresolved") {
                auto expansion = reader.expand(1);
                check(expansion.owner && !expansion.complete && expansion.located.empty() &&
                          expansion.missing_blocks == std::vector<std::uint64_t>{999},
                      "owner expansion invented missing child IDs/counts");
            }
        }
        for (const std::string name :
             {"frame-disagrees", "frame-code", "frame-padding", "column-code", "column-padding",
              "compact-padding", "container-partition", "predecessor", "directory-bounds"}) {
            auto path = dir / ("wave-" + name + ".vtr");
            wave_case(path, name);
            rejects([&] {
                Reader reader(path, {.verify_checksums = true});
                reader.validate();
            });
        }
        auto wave_valid = dir / "wave-valid.vtr";
        wave_case(wave_valid, "valid");
        Reader(wave_valid, {.verify_checksums = true}).validate();
        auto valid = dir / "outside-endpoints.vtr",
             recovered = dir / "outside-endpoints.recovered.vtr";
        write_case(valid, "outside-endpoints");
        Reader normal(valid);
        check(!normal.validate().incomplete_file && normal.validate().missing.empty() &&
                  !normal.lookup(0),
              "outside-file endpoints became record IDs");
        recover_container(valid, recovered);
        Reader incomplete(recovered);
        check(incomplete.validate().incomplete_file && incomplete.validate().missing.empty(),
              "a rebuilt directory hid recovery provenance");
        {
            namespace wire = vtr::detail::wire;
            wire::Writer closed((dir / "closed-packed.vtr").string());
            std::vector<std::pair<wire::Id, Bytes>> signals;
            for (const auto width : {1U, 64U, 65U, 257U}) {
                const auto [node, signal] = closed.add_var(wire::none, std::to_string(width),
                                                           {wire::SignalKind::bits, width, 2});
                (void)node;
                signals.emplace_back(signal, Bytes((width + 7) / 8, 0xff));
            }
            closed.close();
            for (const auto& [signal, value] : signals)
                rejects([&] { closed.emit_packed(signal, 2, value); });
        }
        auto zero_clock_path = dir / "zero-clock-id.vtr";
        {
            Writer writer(zero_clock_path);
            auto clock = writer.add_clock(none, "clock"),
                 source = writer.add_generator(none, "source", "TRANSACTION");
            check(clock == 0, "fixture clock is not node zero");
            writer.associate_clock(source, clock);
            auto owner = writer.begin(source, 0);
            writer.end(owner, 1);
            writer.clock_run(clock, 0, 2);
            writer.set_time(6);
            writer.close();
        }
        Reader zero(zero_clock_path);
        zero.validate();
        check(zero.clock_for(1).has_value() && zero.clock_for(1).value() == 0 &&
                  zero.cycle_at(0, 6) == 3,
              "node-zero clock association became absent");
        std::cout
            << "Validated global duplicate IDs, ownership kinds, exact EVENT/STAGE locator sets, "
               "excluded LOG locators, relation endpoints, clock periods/overlaps/associations, "
               "labels, payload/attribute collisions and recoverable missing references\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
