#include "vtr_reader.h"
#include "vtr_writer.h"

#include <filesystem>
#include <iostream>

namespace vt = vtr;

void check(bool value, const char* message) {
    if (!value)
        throw std::runtime_error(message);
}

void declarations(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode, .encoding = {.waveform_deduplicate = false}});
    const auto packed = writer.intern("packed"), enum_key = writer.intern("enum_table");
    const auto root = writer.scope("TOP", {}, {}, {vt::ScopeKind::interface, "core"});
    const vt::Attributes attributes{{packed, vt::Value(true)}};
    const auto scope = writer.scope("packet", root, attributes, {vt::ScopeKind::struct_, ""});
    const auto other = writer.scope("vendor", root, {}, {static_cast<vt::ScopeKind>(4095), "vendor"});
    const std::array entries{vt::EnumEntry{writer.intern("IDLE"), writer.intern("0000")},
                             vt::EnumEntry{writer.intern("BUSY"), writer.intern("1010")}};
    const auto table = writer.enum_table({.name = "state_t", .parent = scope}, entries);
    const vt::Attributes enumeration{{enum_key, vt::Value(std::uint64_t(table.value))}};
    const auto state = writer.signal({.name = "state", .parent = scope, .metadata = enumeration},
                                     {.width = 4}, {vt::VariableType::enum_, vt::VariableDirection::output});
    const auto alias = writer.alias(state, {.name = "input_state", .parent = root, .metadata = enumeration},
                                    {vt::VariableType::logic, vt::VariableDirection::input});
    const auto event = writer.signal({.name = "event", .parent = root}, {},
                                     {vt::VariableType::event, vt::VariableDirection::implicit});
    const auto event_alias = writer.alias(event, {.name = "event_alias", .parent = root},
                                          {vt::VariableType::event});
    const auto repeat = writer.signal({.name = "repeat", .parent = root});
    writer.advance_time(0);
    writer.emit(state, 10);
    writer.emit(event, 1);
    writer.emit(repeat, 1);
    writer.advance_time(1);
    writer.emit(state, 5);
    writer.emit(event, 1);
    writer.emit(repeat, 1);
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "declaration file validation");
    const auto nodes = reader.nodes();
    const auto& top = nodes[root.value];
    check(top.kind == vt::NodeKind::scope && top.scope_kind == vt::ScopeKind::interface &&
              top.component && reader.string(*top.component) == "core", "scope identity and component");
    check(nodes[scope.value].scope_kind == vt::ScopeKind::struct_ &&
              std::get<bool>(nodes[scope.value].metadata[0].data.data), "packed scope metadata");
    check(nodes[other.value].scope_kind == static_cast<vt::ScopeKind>(4095), "vendor scope kind preserved");
    const auto& enumeration_node = nodes[table.value];
    check(enumeration_node.kind == vt::NodeKind::enum_table && !enumeration_node.signal &&
              !enumeration_node.variable && !enumeration_node.scope_kind &&
              enumeration_node.enum_entries.size() == 2, "enum table remains distinct from a signal");
    for (std::size_t i = 0; i < entries.size(); ++i)
        check(enumeration_node.enum_entries[i].name == entries[i].name &&
                  enumeration_node.enum_entries[i].value == entries[i].value,
              "enum names and exact bit strings preserved");
    check(reader.find_signal("TOP.packet.state") == state &&
              reader.find_signal("TOP.input_state") == state, "alias shares signal identity");
    const auto& input = nodes[alias.value];
    check(input.alias && input.variable && input.variable->type == vt::VariableType::logic &&
              input.variable->direction == vt::VariableDirection::input && input.metadata.size() == 1 &&
              std::get<std::uint64_t>(input.metadata[0].data.data) == table.value,
          "alias declaration properties and enum association preserved");
    bool output = false;
    for (const auto& node : nodes)
        if (node.signal == state && !node.alias)
            output = node.variable && node.variable->type == vt::VariableType::enum_ &&
                     node.variable->direction == vt::VariableDirection::output;
    check(output && nodes[event_alias.value].variable->type == vt::VariableType::event,
          "signal declaration properties preserved");
    unsigned events = 0;
    reader.changes(event, {}, [&](const auto& change) {
        check(change.time == events && change.value.as_u64() == 1, "same-valued event pulses preserved");
        ++events;
        return vt::Flow::next;
    });
    check(events == 2 && reader.value_at(state, 0).as_u64() == 10 &&
              reader.value_at(state, 1).as_u64() == 5, "declared signal histories");
    unsigned repeated = 0;
    reader.changes(repeat, {}, [&](const auto& change) {
        check(change.time == repeated && change.value.as_u64() == 1, "disabled value deduplication");
        ++repeated;
        return vt::Flow::next;
    });
    check(repeated == 2, "same-valued observations retained when deduplication disabled");
}

void binary_packed(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode});
    const std::array<unsigned, 8> widths{1, 8, 63, 64, 65, 95, 256, 257};
    std::vector<vt::SignalId> signals;
    for (auto width : widths)
        signals.push_back(writer.signal({.name = "bits" + std::to_string(width)}, {.width = width}));
    for (unsigned time = 0; time != 3; ++time) {
        writer.advance_time(time);
        for (std::size_t i = 0; i < signals.size(); ++i) {
            vt::Bytes value((widths[i] + 7) / 8, std::byte{0xff});
            if (time == 1 && widths[i] % 8)
                value.back() &= std::byte((1U << (widths[i] % 8)) - 1);
            if (time == 2) value.front() &= std::byte{0xfe};
            writer.emit_packed(signals[i], value);
            std::fill(value.begin(), value.end(), std::byte{0});
        }
    }
    writer.close();
    vt::Reader reader(path);
    check(reader.validate().missing.empty(), "binary packed recordings validate");
    for (std::size_t i = 0; i < signals.size(); ++i) {
        unsigned changes = 0;
        reader.changes(signals[i], {}, [&](const auto& change) {
            check(change.time == (changes ? 2 : 0), "binary padding does not create a logical change");
            for (unsigned bit = 0; bit < widths[i]; ++bit) {
                const auto byte = std::to_integer<unsigned>(change.value.packed[bit / 8]);
                check(((byte >> (bit % 8)) & 1) == (change.time != 2 || bit != 0),
                      "packed binary input copied with exact bit significance");
            }
            if (widths[i] % 8)
                check((std::to_integer<unsigned>(change.value.packed.back()) >> (widths[i] % 8)) == 0,
                      "dirty high padding cleared");
            ++changes;
            return vt::Flow::next;
        });
        check(changes == 2, "binary fast path preserves deduplication and final changes");
    }
}

void word_failures(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::Writer writer(path, {.mode = mode});
    const auto signal = writer.signal({.name = "wide"}, {.width = 95});
    const std::array<std::uint32_t, 3> words{0x01234567, 0x89abcdef, UINT32_MAX};
    for (const auto width : {0U, 1000U}) {
        bool rejected = false;
        try { writer.emit_words(signal, words, width); }
        catch (const vt::Error&) { rejected = true; }
        check(rejected, "invalid word input rejected before publication");
    }
    writer.emit_words(signal, words, 95);
    bool rejected = false;
    try {
        writer.emit_words(signal, words, 94);
        if (mode == vt::ExecutionMode::threaded) writer.close();
    } catch (const vt::Error&) { rejected = true; }
    check(rejected, "word width mismatch rejected even for the same packed byte count");
    if (mode == vt::ExecutionMode::inline_) {
        writer.advance_time(1);
        writer.emit_words(signal, words, 95);
        writer.close();
        const auto sample = vt::Reader(path).value_at(signal, 1);
        check(sample.packed.size() == 12 && sample.packed[0] == std::byte{0x67} &&
                  sample.packed[4] == std::byte{0xef} && sample.packed[11] == std::byte{0x7f},
              "word order, padding and retryable inline argument error");
    } else {
        writer.seal();
        check(vt::Reader(path).ending().state == vt::Completion::incomplete,
              "asynchronous word input failure retains incomplete evidence");
    }
}

void named_lookups(const std::filesystem::path& path, vt::ExecutionMode mode, bool clock_first) {
    vt::Writer writer(path, {.mode = mode});
    const auto root = writer.scope("top");
    const vt::SourceOptions options{.name = "clk", .parent = root};
    const auto [signal, clock] = [&] {
        if (clock_first) {
            const auto clock = writer.clock(options);
            return std::pair{writer.signal(options), clock};
        }
        const auto signal = writer.signal(options);
        return std::pair{signal, writer.clock(options)};
    }();
    const auto grouped_signal = writer.signal({.name = "group", .parent = root},
                                             {.encoding = vt::SignalEncoding::real});
    const auto group = writer.scope("group", root);
    const auto source = writer.transactions<std::uint64_t>({.name = "requests", .parent = group});
    writer.associate_clock(source, clock);
    writer.run(clock, 0, 2);
    writer.emit(signal, 1);
    writer.emit_real(grouped_signal, 3.5);
    const auto owner = writer.begin(source, std::uint64_t(42));
    writer.advance_time(8);
    writer.end(owner, vt::Status::ok);
    writer.close();
    vt::Reader reader(path);
    check(reader.find_generator("top.clk") == clock.id(),
          "find generator despite a same-named waveform");
    check(reader.find_signal("top.clk") == signal,
          "find waveform despite a same-named clock");
    check(reader.find_signal("top.group") == grouped_signal,
          "find a same-named group waveform");
    check(reader.find_generator("top.group.requests") == source.id(),
          "traverse scopes despite a same-named waveform");
    check(!reader.find_generator("top.group") && !reader.find_signal("top.group.requests"),
          "reject the wrong declaration category");
    check(reader.value_at(signal, 8).as_u64() == 1, "named waveform remains readable");
    check(reader.lookup(owner.id())->payload().as<std::uint64_t>() == 42,
          "named owner payload remains readable");
    check(reader.cycle_at(clock.id(), 8) == 4, "named clock remains queryable");
    check(reader.validate().missing.empty(),
          "ambiguous paths do not alter declaration identities");
}

void time_ranges(const std::filesystem::path& path, vt::ExecutionMode mode, vt::Codec codec,
                 bool full_domain) {
    vt::Writer writer(path, {.mode = mode,
                            .encoding = {.compression = codec, .page_bytes = 1,
                                         .block_records = 1}});
    const auto source = writer.transactions<std::string>({.name = "owners"});
    const auto name = writer.intern("execute"), key = writer.intern("detail");
    const auto owner = writer.begin_at(source, 100, std::string(28'000, 'p'));
    writer.attribute(owner, key, vt::Value(vt::Bytes(32'000, std::byte{7})));
    writer.embedded_stage(owner, 50, name, name, 160);
    const vt::Attributes event_attributes{{key, vt::Value(std::string(20'000, 'e'))}};
    writer.embedded_event(owner, 180, name, event_attributes);
    if (full_domain)
        writer.embedded_stage(owner, 0, name, name, UINT64_MAX);
    writer.end_at(owner, 110, vt::Status::ok);
    writer.close();
    check(vt::Reader(path).validate().missing.empty(), "range fixture validates");
    vt::Reader reader(path, {.max_payload_bytes = 1});
    const auto range = reader.time_range();
    check(range && range->first == (full_domain ? 0 : 50) &&
              range->last == (full_domain ? UINT64_MAX : 180),
          "range includes embedded detail outside the owner");
    check(reader.cache_stats().owner_entries == 0, "time summary does not fill the owner cache");
    bool rejected = false;
    try { (void)reader.lookup(owner.id()); }
    catch (const vt::Error&) { rejected = true; }
    check(rejected, "record reads retain payload allocation admission");
    check(reader.time_range()->last == range->last && reader.cache_stats().owner_entries == 0,
          "summary remains independent of failed payload reads");
}

int main(int argc, char** argv) {
    try {
        const auto root = std::filesystem::path(argc > 1 ? argv[1] : "declaration-fixtures");
        std::filesystem::create_directories(root);
        for (auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded}) {
            for (auto codec : {vt::Codec::stored, vt::Codec::lz4, vt::Codec::zstd}) {
                for (bool full : {false, true}) {
                    const auto name = std::to_string(static_cast<int>(mode)) + "-range-" +
                                      std::to_string(static_cast<int>(codec)) + "-" +
                                      std::to_string(full) + ".vtr";
                    time_ranges(root / name, mode, codec, full);
                }
            }
            for (bool clock_first : {false, true}) {
                const auto name =
                    std::string(mode == vt::ExecutionMode::inline_ ? "inline-" : "threaded-") +
                    (clock_first ? "clock-first.vtr" : "wave-first.vtr");
                named_lookups(root / name, mode, clock_first);
            }
            declarations(root / (mode == vt::ExecutionMode::inline_ ? "inline.vtr" : "threaded.vtr"), mode);
            word_failures(root / (mode == vt::ExecutionMode::inline_ ? "inline-words.vtr" : "threaded-words.vtr"), mode);
            binary_packed(root / (mode == vt::ExecutionMode::inline_ ? "inline-binary.vtr" : "threaded-binary.vtr"), mode);
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
