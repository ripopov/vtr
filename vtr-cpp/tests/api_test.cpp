#include "vtr_reader.h"
#include "vtr_writer.h"

#include <algorithm>
#include <atomic>
#include <bit>
#include <cmath>
#include <climits>
#include <filesystem>
#include <format>
#include <iomanip>
#include <iostream>
#include <locale>
#include <sstream>
#include <thread>

namespace vt = vtr;

enum class Operation : std::int16_t { load = -3, store = 7 };

struct Payload {
    bool valid;
    std::int8_t i8;
    std::int16_t i16;
    std::int32_t i32;
    std::int64_t i64;
    std::uint8_t u8;
    std::uint16_t u16;
    std::uint32_t u32;
    std::uint64_t u64;
    float f32;
    double f64;
    vt::StringId interned;
    std::string text;
    vt::Bytes bytes;
    vt::Bits bits;
    vt::Logic logic;
    vt::Logic9 logic9;
    vt::TimeValue time;
    vt::Address address;
    vt::Fixed fixed;
    vt::UFixed ufixed;
    Operation operation;
    std::array<std::uint32_t, 3> array;
    std::vector<std::vector<std::string>> nested;
    std::vector<vt::Unit> empty_elements;
    std::vector<bool> flags;
};

template <> struct vt::Schema<Operation> {
    static constexpr std::string_view name = "Operation";
    static constexpr auto enumerators = std::array{
        vt::Enumerator<Operation>{"load", Operation::load},
        vt::Enumerator<Operation>{"store", Operation::store},
    };
};

template <> struct vt::Schema<Payload> {
    static constexpr std::string_view name = "Payload";
    static constexpr auto fields = std::tuple{
        vt::field("valid", &Payload::valid),
        vt::field("i8", &Payload::i8),
        vt::field("i16", &Payload::i16),
        vt::field("i32", &Payload::i32),
        vt::field("i64", &Payload::i64),
        vt::field("u8", &Payload::u8),
        vt::field("u16", &Payload::u16),
        vt::field("u32", &Payload::u32),
        vt::field("u64", &Payload::u64),
        vt::field("f32", &Payload::f32),
        vt::field("f64", &Payload::f64),
        vt::field("interned", &Payload::interned),
        vt::field("text", &Payload::text),
        vt::field("bytes", &Payload::bytes),
        vt::field("bits", &Payload::bits),
        vt::field("logic", &Payload::logic),
        vt::field("logic9", &Payload::logic9),
        vt::field("time", &Payload::time),
        vt::field("address", &Payload::address),
        vt::field("fixed", &Payload::fixed),
        vt::field("ufixed", &Payload::ufixed),
        vt::field("operation", &Payload::operation),
        vt::field("array", &Payload::array),
        vt::field("nested", &Payload::nested),
        vt::field("empty_elements", &Payload::empty_elements),
        vt::field("flags", &Payload::flags),
    };
};

struct Message {
    std::uint64_t pc;
    std::string text;
};

template <> struct vt::Schema<Message> {
    static constexpr std::string_view name = "Message";
    static constexpr auto fields =
        std::tuple{vt::field("pc", &Message::pc), vt::field("text", &Message::text)};
};

struct Borrowed {
    std::string_view text;
    std::vector<std::string_view> nested;
    std::span<const std::uint32_t> words = {};
};

template <> struct vt::Schema<Borrowed> {
    static constexpr std::string_view name = "Borrowed";
    static constexpr auto fields =
        std::tuple{vt::field("text", &Borrowed::text), vt::field("nested", &Borrowed::nested),
                   vt::field("words", &Borrowed::words)};
};

void check(bool condition, std::string_view message) {
    if (!condition) {
        throw std::runtime_error(std::string(message));
    }
}

template <class F> void rejects(F&& action) {
    try {
        action();
    } catch (const vt::Error&) {
        return;
    }
    throw std::runtime_error("Expected a VTR error");
}

Payload example_payload(vt::StringId string) {
    return {true,
            -100,
            -30000,
            -2000000000,
            INT64_MIN,
            240,
            60000,
            4000000000u,
            UINT64_MAX,
            std::bit_cast<float>(0x7fc01234u),
            std::bit_cast<double>(0x8000000000000000ull),
            string,
            "hello \xce\xbb",
            {std::byte{0}, std::byte{0xff}},
            {9, {std::byte{0x81}, std::byte{1}}},
            {4, {std::byte{0xe4}}},
            {2, {std::byte{0x80}}},
            {1234},
            {0xdeadbeef},
            {-123, -2},
            {UINT64_MAX, 3},
            static_cast<Operation>(-17),
            {1, 2, 3},
            {{"one", "two"}, {}, {"three"}},
            std::vector<vt::Unit>(5),
            {true, false, true}};
}

void check_payload(const Payload& value, vt::StringId string) {
    check(value.valid && value.i8 == -100 && value.i16 == -30000 && value.i32 == -2000000000 &&
              value.i64 == INT64_MIN,
          "signed values");
    check(value.u8 == 240 && value.u16 == 60000 && value.u32 == 4000000000u &&
              value.u64 == UINT64_MAX,
          "unsigned values");
    check(std::bit_cast<std::uint32_t>(value.f32) == 0x7fc01234u &&
              std::bit_cast<std::uint64_t>(value.f64) == 0x8000000000000000ull,
          "IEEE bits");
    check(value.interned == string && value.text == "hello \xce\xbb", "strings");
    check(value.bytes == vt::Bytes{std::byte{0}, std::byte{0xff}} && value.bits.width == 9 &&
              value.bits.packed == vt::Bytes{std::byte{0x81}, std::byte{1}},
          "byte and binary vectors");
    check(value.logic.packed == vt::Bytes{std::byte{0xe4}} &&
              value.logic9.packed == vt::Bytes{std::byte{0x80}},
          "logic vectors");
    check(value.time.value == 1234 && value.address.value == 0xdeadbeef &&
              value.fixed.raw == -123 && value.fixed.scale == -2 &&
              value.ufixed.raw == UINT64_MAX && value.ufixed.scale == 3,
          "special scalars");
    check(value.operation == static_cast<Operation>(-17), "unknown enum value");
    check(value.array == std::array<std::uint32_t, 3>{1, 2, 3} && value.nested.size() == 3 &&
              value.nested[0][1] == "two" && value.nested[1].empty() &&
              value.nested[2][0] == "three" && value.empty_elements.size() == 5 &&
              value.flags == std::vector<bool>{true, false, true},
          "composed values");
}

void lifecycle(const std::filesystem::path& path, unsigned workers, vt::Codec codec,
               vt::ExecutionMode mode = vt::ExecutionMode::threaded) {
    vt::Writer out(path, {.mode = mode,
                          .buffer = {.queue_bytes = 256},
                          .encoding = {.compression = codec,
                                       .page_bytes = 128,
                                       .block_bytes = 256,
                                       .block_records = 2,
                                       .workers = workers}});
    auto identity = out.identity();
    auto key = out.intern("test");
    auto name = out.intern("phase");
    auto literal = out.intern("literal");
    out.metadata({.writer = "api_test", .extra = {{key, vt::Value(std::int16_t(-42))}}});
    auto scope = out.scope("core", {}, vt::Attributes{{key, vt::Value(true)}});
    auto owners = out.transactions<Payload>({.name = "owners", .parent = scope}, "REQUEST");
    auto children = out.transactions({.name = "children", .parent = scope});
    auto stages = out.stages<std::uint32_t>({.name = "execute", .parent = scope});
    auto events = out.events<std::string>({.name = "events", .parent = scope});
    auto messages = out.logs<Message>({.name = "{1} {0:#x} {} {1} {9} {{ok}}", .parent = scope});
    auto clock = out.clock({.name = "clock", .parent = scope});
    out.associate_clock(owners, clock);
    auto wave = out.signal({.name = "counter", .parent = scope}, {.width = 8});
    auto alias = out.alias(wave, {.name = "alias", .parent = scope});
    auto real =
        out.signal({.name = "real", .parent = scope}, {.encoding = vt::SignalEncoding::real});
    auto packed =
        out.signal({.name = "logic", .parent = scope}, {.width = 4, .alphabet = vt::States::four});
    auto bytes =
        out.signal({.name = "text", .parent = scope}, {.encoding = vt::SignalEncoding::bytes});
    auto same_type = out.bind<Payload>();
    check(same_type == out.bind<Payload>(), "schema binding is stable");
    (void)alias;

    auto owner = [&] {
        auto tx = out.begin_at(owners, 10);
        out.payload(tx, example_payload(literal));
        out.attribute(tx, key,
                      vt::Value(vt::Value::Map{
                          {key, vt::Value(vt::Value::List{vt::Value(std::string("nested")),
                                                          vt::Value(vt::Fixed{-5, -2})})}}));
        out.classify(tx, vt::SpanKind::client);
        auto stage = out.begin_at(stages, tx, 12);
        out.end_at(stage, 14, std::uint32_t(7));
        auto second = out.begin_at(stages, tx, 14, std::uint32_t(8));
        (void)second;
        auto first = out.event_at(events, tx, 13, std::string("same"));
        auto repeated = out.event_at(events, tx, 13, std::string("same"));
        check(first != repeated, "repeated events keep distinct IDs");
        out.log_at(messages, 13, Message{0x8000, "message"}, tx);
        auto child = out.begin_at(children, 30, {.parent = tx});
        out.end_at(child, 31);
        auto stretch = out.run(clock, 0, 2);
        (void)stretch;
        out.stop(clock, 20);
        out.advance_time(0);
        out.emit(wave, 1);
        out.emit_real(real, 1.25);
        out.emit_packed(packed, vt::Bytes{std::byte{0xe4}});
        out.emit_bytes(bytes, vt::Bytes{std::byte{'a'}});
        out.advance_time(5);
        const std::array updates{vt::SignalUpdate{wave, 2}};
        out.emit(updates);
        return tx;
    }();
    auto embedded_event = out.embedded_event(owner, 99, name);
    auto embedded_stage = out.embedded_stage(owner, 16, name, name);
    auto explicit_stage = out.embedded_stage(owner, 11, name, name);
    out.end_embedded_stage(owner, explicit_stage, 13);
    out.relate(key, owner.id(), embedded_event, vt::Attributes{{key, vt::Value(std::uint8_t(7))}});
    out.relate(key, std::nullopt, owner.id());
    out.end_at(owner, 15, vt::Status::ok);
    out.flush();
    out.close();
    out.close();
    auto stats = out.stats();
    if (mode == vt::ExecutionMode::inline_)
        check(stats.transport.submitted == 0 && stats.encoding.submitted == 0,
              "inline mode must bypass both queues");
    check(stats.active_owners == 0 && stats.active_stages == 0 && stats.transaction_blocks > 0 &&
              stats.record_blocks > 0,
          "writer stats");

    std::optional<vt::Record> retained;
    std::optional<vt::PayloadView> retained_view;
    {
        vt::Reader in(path, {.owner_cache_entries = 1, .owner_cache_bytes = 1 << 20});
        check(in.identity() == identity && in.ending().state == vt::Completion::complete &&
                  in.fingerprint().has_value(),
              "complete identity");
        check(in.metadata().writer == "api_test" && in.metadata().extra.size() == 1,
              "file metadata");
        check(in.find_signal("core.alias") == wave &&
                  in.find_generator("core.owners") == owners.id() && !in.find_generator("core") &&
                  !in.find("missing"),
              "hierarchy lookup");
        check(in.string(literal) == "literal" && in.nodes()[scope.value].metadata.size() == 1,
              "catalog metadata");
        auto record = in.lookup(owner.id());
        check(record && record->kind() == vt::RecordKind::transaction &&
                  record->status() == vt::Status::ok &&
                  record->span_kind() == vt::SpanKind::client && record->begin() == 10 &&
                  record->end() == 15,
              "owner fields");
        check_payload(record->payload().as<Payload>(), literal);
        check(record->payload()["nested"][2][0].as<std::string>() == "three",
              "dynamic nested access");
        rejects([&] { (void)record->payload()["unknown"]; });
        rejects([&] { (void)record->payload()["u64"].as<std::int64_t>(); });
        check(record->events().size() == 1 && record->stages().size() == 2, "embedded detail");
        check(in.lookup(embedded_event)->begin() == 99, "embedded event lookup");
        auto embedded = in.lookup(embedded_stage);
        check(embedded && embedded->name() == name && embedded->lane() == name &&
                  embedded->end() == 16 && !embedded->explicit_end(),
              "implicit embedded stage end");
        check(in.lookup(explicit_stage)->explicit_end() == 13, "explicit embedded stage end");
        auto expansion = in.expand(owner.id());
        check(expansion.complete && expansion.located.size() == 4,
              "located expansion excludes logs");
        std::size_t at_boundary = 0;
        in.records({.generator = stages.id(), .time = {14, 14}}, [&](const vt::Record& stage) {
            ++at_boundary;
            check(stage.begin() == 14 && stage.end() == 15 &&
                      stage.payload().as<std::uint32_t>() == 8,
                  "half-open stage query and automatic stage completion");
            return vt::Flow::next;
        });
        check(at_boundary == 1, "stage end is excluded");
        std::size_t ordinary_children = 0;
        in.children(owner.id(), [&](const vt::Record& child) {
            ++ordinary_children;
            check(child.begin() == 30, "independent child lifetime");
            return vt::Flow::next;
        });
        check(ordinary_children == 1, "ordinary child count");
        std::size_t logs = 0;
        in.logs(owner.id(), {}, [&](const vt::Record& log) {
            ++logs;
            check(in.format_log(log) == "message 0x8000 32768 message {?} {ok}",
                  "indexed log formatting");
            return vt::Flow::next;
        });
        check(logs == 1, "log count");
        std::size_t relations = 0;
        in.relations(owner.id(), vt::Direction::outgoing, [&](const vt::Relation& relation) {
            ++relations;
            check(relation.to == embedded_event && relation.data.size() == 1,
                  "relation attributes");
            return vt::Flow::next;
        });
        check(relations == 1, "outgoing relations");
        in.relations(owner.id(), vt::Direction::incoming, [&](const vt::Relation& relation) {
            check(!relation.from, "external relation endpoint");
            return vt::Flow::stop;
        });
        check(in.cycle_at(clock.id(), 14) == 7 && in.cycle_at(clock.id(), 21) == 10,
              "clock cycle and stopped stretch");
        check(in.value_at(wave, 4).as_u64() == 1 && in.value_at(wave, 5).as_u64() == 2,
              "wave predecessor");
        check(in.value_at(real, 1).as_real() == 1.25 &&
                  in.value_at(packed, 1).as_text() == "zx10" &&
                  in.value_at(bytes, 1).as_text() == "a",
              "wave encodings");
        std::size_t changes = 0;
        in.changes(wave, {0, 5}, [&](const vt::SignalChange&) {
            ++changes;
            return vt::Flow::stop;
        });
        check(changes == 1, "wave early stop");
        auto index = vt::ActivityIndex::build(in);
        auto loaded = vt::ActivityIndex::load(in, index.encode());
        check(loaded.fingerprint() == *in.fingerprint() &&
                  loaded.active({4, 5}) == std::vector{wave},
              "source-bound activity");
        auto validation = in.validate();
        check(validation.missing.empty() && validation.records >= 11 && validation.relations == 2,
              "full validation");
        check(in.cache_stats().owner_entries <= 1, "bounded owner cache");
        retained = record;
        retained_view = record->payload()["nested"];
        in.clear_cache();
        check(in.cache_stats().owner_entries == 0, "cache clear");
        rejects([&] { in.records({.time = {2, 1}}, [](const auto&) { return vt::Flow::next; }); });
        vt::Reader moved(std::move(in));
        check(moved.lookup(owner.id()).has_value(), "reader move");
    }
    check_payload(retained->payload().as<Payload>(), literal);
    check((*retained_view)[0][1].as<std::string>() == "two",
          "retained view after reader destruction");
}

void borrowed_and_errors(const std::filesystem::path& root) {
    auto path = root / "borrowed.vtr";
    vt::Writer out(path);
    auto messages = out.transactions<Borrowed>({.name = "borrowed"});
    std::string backing(4096, 'a');
    std::array words{1u, 2u, 3u};
    auto tx = out.begin_at(messages, 1, Borrowed{backing, {backing}, words});
    backing.assign(4096, 'b');
    words.fill(99);
    out.end_at(tx, 2);
    out.close();
    vt::Reader in(path);
    check(in.lookup(tx.id())->payload()["text"].as<std::string>() == std::string(4096, 'a') &&
              in.lookup(tx.id())->payload()["nested"][0].as<std::string>() ==
                  std::string(4096, 'a'),
          "capture copies borrowed input");
    check(in.lookup(tx.id())->payload()["words"].as<std::vector<std::uint32_t>>() ==
              std::vector<std::uint32_t>{1, 2, 3},
          "capture copies borrowed sequence input");

    vt::Writer first(root / "first.vtr", {});
    vt::Writer second(root / "second.vtr", {});
    auto producer = first.add_producer();
    rejects([&] { second.use_producer(producer); });
    first.use_producer(producer);
    auto source = first.transactions({.name = "source"});
    rejects([&] { (void)second.begin_at(source, 1); });
    auto owner = first.begin_at(source, 5);
    first.end_at(owner, 2);
    first.close();
    vt::Reader clamped(root / "first.vtr");
    check(clamped.lookup(owner.id())->end() == 5, "clamp early end");
    second.close();

    vt::Writer failed(root / "failed.vtr", {});
    auto site = failed.transactions<std::uint32_t>({.name = "site"});
    auto missing = failed.begin_at(site, 1);
    failed.end_at(missing, 2);
    failed.advance_time(1);
    rejects([&] { failed.flush(); });
    failed.seal();
    vt::Reader evidence(root / "failed.vtr");
    check(evidence.ending().state == vt::Completion::incomplete,
          "failed stream is incomplete evidence");
    auto recovered = root / "recovered.vtr";
    auto provenance = vt::recover(root / "failed.vtr", recovered);
    check(provenance.state == vt::Completion::incomplete, "recovery preserves incompleteness");
    (void)vt::Reader(recovered).validate();
    rejects([&] { (void)vt::recover(recovered, recovered); });
}

void allocation_limits(const std::filesystem::path& path) {
    vt::Writer out(path);
    auto empty = out.transactions<std::vector<vt::Unit>>({.name = "empty"});
    auto text = out.transactions<std::vector<std::string>>({.name = "text"});
    auto integer = out.transactions<long long>({.name = "integer"});
    auto tx = out.begin_at(empty, 0, std::vector<vt::Unit>(100'000));
    out.end_at(tx, 1);
    std::vector<std::string> words(10'000, "variable length text");
    words.back() = "last";
    auto large = out.begin_at(text, 0, words);
    out.end_at(large, 1);
    auto number = out.begin_at(integer, 0, -123456789012345ll);
    out.end_at(number, 1);
    out.close();
    vt::Reader bounded(path, {.max_payload_bytes = 64});
    auto retained = bounded.lookup(tx.id());
    check(retained->payload().size() == 100'000, "zero-size elements retain their count");
    try {
        (void)retained->payload().as<std::vector<vt::Unit>>();
        throw std::runtime_error("Unbounded decoded allocation accepted");
    } catch (const vt::Error& error) {
        check(error.code() == vt::ErrorCode::resource_limit, "allocation error classification");
    }
    vt::Reader input(path);
    auto decoded = input.lookup(large.id())->payload().as<std::vector<std::string>>();
    check(decoded == words, "large ragged sequence");
    check(input.lookup(number.id())->payload().as<long long>() == -123456789012345ll,
          "integer aliases use their canonical format width");
    rejects([&] { (void)input.lookup(large.id())->payload().as<std::array<std::string, 1>>(); });
}

void concurrent(const std::filesystem::path& path) {
    vt::Writer out(path, {.buffer = {.queue_bytes = 512}, .encoding = {.workers = 2}});
    auto source = out.transactions<std::uint64_t>({.name = "concurrent"});
    std::vector<vt::ProducerId> producers;
    for (unsigned i = 0; i < 4; ++i) {
        producers.push_back(out.add_producer());
    }
    std::vector<std::thread> threads;
    for (std::uint64_t thread = 0; thread < 4; ++thread) {
        threads.emplace_back([&, thread] {
            out.use_producer(producers[thread]);
            for (std::uint64_t i = 0; i < 100; ++i) {
                out.advance_time(i);
                auto tx = out.begin(source, thread * 100 + i);
                out.end_at(tx, i + 1);
            }
            out.finish_producer();
        });
    }
    for (auto& thread : threads) {
        thread.join();
    }
    out.close();
    vt::Reader input(path);
    std::vector<bool> seen(400);
    input.transactions({}, [&](const vt::Record& record) {
        auto value = record.payload().as<std::uint64_t>();
        check(value < seen.size() && !seen[value], "concurrent capture uniqueness");
        seen[value] = true;
        return vt::Flow::next;
    });
    check(std::ranges::all_of(seen, [](bool value) { return value; }),
          "concurrent capture completeness");
    (void)input.validate();
}

void scalar_attributes(const std::filesystem::path& path, vt::ExecutionMode mode) {
    const std::array values{vt::Value{},
                            vt::Value(true),
                            vt::Value(std::int8_t(-128)),
                            vt::Value(std::int16_t(-32768)),
                            vt::Value(INT32_MIN),
                            vt::Value(INT64_MIN),
                            vt::Value(std::uint8_t(UINT8_MAX)),
                            vt::Value(std::uint16_t(UINT16_MAX)),
                            vt::Value(UINT32_MAX),
                            vt::Value(UINT64_MAX),
                            vt::Value(std::bit_cast<float>(0xffc01234u)),
                            vt::Value(std::bit_cast<double>(0xfff8000000001234ull))};
    vt::Writer writer(path, {.mode = mode});
    auto source = writer.transactions({.name = "values"});
    std::vector<vt::StringId> keys;
    for (unsigned i = 0; i < values.size(); ++i) {
        keys.push_back(writer.intern("value" + std::to_string(i)));
    }
    std::vector<vt::RecordId> ids;
    for (bool direct : {false, true}) {
        auto tx = writer.begin(source);
        ids.push_back(tx.id());
        for (unsigned i = 0; i < values.size(); ++i) {
            if (direct)
                std::visit([&](const auto& value) { writer.attribute(tx, keys[i], value); },
                           values[i].data);
            else
                writer.attribute(tx, keys[i], values[i]);
        }
        writer.end(tx);
    }
    writer.close();
    vt::Reader reader(path);
    for (auto id : ids) {
        const auto record = reader.lookup(id);
        check(record && record->attributes().size() == values.size(), "scalar attributes lost");
        for (unsigned i = 0; i < values.size(); ++i) {
            const auto& actual = record->attributes()[i];
            check(actual.key == keys[i] && actual.data.data.index() == values[i].data.index(),
                  "scalar attribute tag changed");
            std::visit(
                [&]<class T>(const T& expected) {
                    if constexpr (std::same_as<T, float>) {
                        check(std::bit_cast<std::uint32_t>(std::get<T>(actual.data.data)) ==
                                  std::bit_cast<std::uint32_t>(expected),
                              "f32 attribute bits changed");
                    } else if constexpr (std::same_as<T, double>) {
                        check(std::bit_cast<std::uint64_t>(std::get<T>(actual.data.data)) ==
                                  std::bit_cast<std::uint64_t>(expected),
                              "f64 attribute bits changed");
                    } else if constexpr (std::integral<T>) {
                        check(std::get<T>(actual.data.data) == expected,
                              "scalar attribute value changed");
                    }
                },
                values[i].data);
        }
    }
    (void)reader.validate();
}

void u64_attribute_contract(const std::filesystem::path& root, vt::ExecutionMode mode) {
    const auto prefix = mode == vt::ExecutionMode::inline_ ? "inline-u64-" : "threaded-u64-";
    const auto path = root / (std::string(prefix) + "values.vtr");
    vt::Writer writer(path, {.mode = mode});
    auto unit = writer.transactions({.name = "unit"});
    auto typed = writer.transactions<Message>({.name = "typed"});
    std::vector<vt::StringId> keys;
    for (unsigned i = 0; i < 10; ++i)
        keys.push_back(writer.intern("key" + std::to_string(i)));
    const std::array<unsigned, 9> order{8, 1, 6, 0, 7, 2, 5, 3, 4};
    std::vector<vt::RecordId> ids;
    for (unsigned row = 0; row < 4; ++row) {
        auto write = [&](auto tx) {
            ids.push_back(tx.id());
            const auto count = row == 0 ? 5u : 9u;
            for (unsigned i = 0; i < count; ++i) {
                writer.attribute(tx, keys[order[i]], i == 0 ? UINT64_MAX : std::uint64_t(i - 1));
                if (row == 2 && i == 2)
                    writer.attribute(tx, keys[9], std::string("mixed"));
            }
            writer.end_at(tx, row + 1);
        };
        if (row < 2)
            write(writer.begin_at(unit, row));
        else
            write(writer.begin_at(typed, row, Message{row, "payload"}));
    }
    writer.close();
    vt::Reader reader(path);
    for (unsigned row = 0; row < ids.size(); ++row) {
        const auto record = reader.lookup(ids[row]);
        check(record.has_value(), "u64 attribute owner lost");
        const auto& attrs = record->attributes();
        const auto count = row == 0 ? 5u : 9u;
        check(attrs.size() == count + (row == 2), "u64 fallback lost attributes");
        for (unsigned i = 0; i < count; ++i) {
            const auto& attr = attrs[i + (row == 2 && i > 2)];
            check(attr.key == keys[order[i]] && std::get<std::uint64_t>(attr.data.data) ==
                                                    (i == 0 ? UINT64_MAX : std::uint64_t(i - 1)),
                  "u64 attribute order, tag or bits changed");
        }
        if (row == 2)
            check(attrs[3].key == keys[9] && std::get<std::string>(attrs[3].data.data) == "mixed",
                  "numeric to text fallback changed the text attribute");
        if (row >= 2)
            check(record->payload().as<Message>().pc == row &&
                      record->payload().as<Message>().text == "payload",
                  "u64 attributes changed the typed payload");
    }
    (void)reader.validate();

    for (unsigned failure = 0; failure < 7; ++failure) {
        const auto failed_path = root / (std::string(prefix) + std::to_string(failure) + ".vtr");
        vt::Writer out(failed_path, {.mode = mode});
        auto source = out.transactions<Message>({.name = "typed"});
        auto key = out.intern("key"), valid = out.intern("valid");
        auto forbidden = failure == 3   ? out.intern("vtr.label")
                         : failure == 4 ? out.intern("pc")
                         : failure == 5 ? vt::StringId{UINT32_MAX}
                                        : key;
        if (failure == 3 || failure == 4) {
            auto unit = out.transactions({.name = "warm"});
            auto warm = out.begin_at(unit, 0);
            if (failure == 3)
                out.attribute(warm, forbidden, std::string("label"));
            else
                out.attribute(warm, forbidden, std::uint64_t(42));
            std::vector<vt::StringId> names;
            for (unsigned i = 0; i < 512; ++i)
                names.push_back(out.intern(std::string(160, 'x') + std::to_string(i)));
            if (failure == 3)
                for (auto name : names)
                    out.attribute(warm, name, std::uint64_t(1));
            out.end_at(warm, 0);
        }
        auto tx = out.begin_at(source, 1, Message{42, "retained"});
        if (failure == 1)
            out.attribute(tx, key, std::int64_t(-1));
        else if (failure == 2)
            out.attribute(tx, key, std::string("retained"));
        else if (failure == 0)
            out.attribute(tx, key, UINT64_MAX);
        if (failure == 6)
            out.end_at(tx, 2);
        rejects([&] {
            out.attribute(tx, forbidden, UINT64_MAX);
            out.flush();
        });
        if (mode == vt::ExecutionMode::inline_) {
            if (failure != 6) {
                out.attribute(tx, valid, std::uint64_t(17));
                out.end_at(tx, 2);
            }
            out.close();
            vt::Reader input(failed_path);
            auto record = input.lookup(tx.id());
            const auto& attrs = record->attributes();
            check(attrs.size() == (failure < 3    ? 2u
                                   : failure == 6 ? 0u
                                                  : 1u),
                  "rejected u64 attribute changed owner state");
            if (failure < 3)
                check(attrs[0].key == key &&
                          (failure == 0 ? std::get<std::uint64_t>(attrs[0].data.data) == UINT64_MAX
                           : failure == 1
                               ? std::get<std::int64_t>(attrs[0].data.data) == -1
                               : std::get<std::string>(attrs[0].data.data) == "retained"),
                      "rejected u64 attribute changed the previous value");
            if (failure != 6)
                check(attrs.back().key == valid &&
                          std::get<std::uint64_t>(attrs.back().data.data) == 17,
                      "valid u64 attribute after rejection was lost");
            (void)input.validate();
        } else {
            out.seal();
            check(vt::Reader(failed_path).ending().state == vt::Completion::incomplete,
                  "rejected queued u64 attribute must leave incomplete evidence");
        }
    }

    vt::Writer first(root / (std::string(prefix) + "first.vtr"), {.mode = mode});
    vt::Writer second(root / (std::string(prefix) + "second.vtr"), {.mode = mode});
    auto first_tx = first.begin(first.transactions({.name = "first"}));
    auto second_tx = second.begin(second.transactions({.name = "second"}));
    check(first_tx.id() == second_tx.id(), "origin test requires colliding local IDs");
    auto key = second.intern("key");
    rejects([&] { second.attribute(first_tx, key, UINT64_MAX); });
    first.end(first_tx);
    second.end(second_tx);
    first.close();
    second.close();
    check(vt::Reader(root / (std::string(prefix) + "second.vtr"))
              .lookup(second_tx.id())
              ->attributes()
              .empty(),
          "foreign u64 attribute mutated a local owner");
}

void progress_without_observations(const std::filesystem::path& root) {
    for (bool parallel : {false, true}) {
        const auto path = root / (parallel ? "parallel-progress.vtr" : "progress.vtr");
        vt::Writer writer(path);
        const auto source = writer.transactions({.name = "open"});
        if (parallel) {
            const auto first = writer.add_producer(), second = writer.add_producer();
            writer.use_producer(first);
            auto tx = writer.begin(source);
            writer.advance_time(100);
            writer.finish_producer();
            writer.use_producer(second);
            writer.advance_time(50);
            writer.finish_producer();
            writer.close();
            vt::Reader reader(path);
            check(reader.lookup(tx.id())->end() == 100,
                  "terminal progress without records lost the final recording time");
            (void)reader.validate();
        } else {
            writer.advance_time(10);
            auto tx = writer.begin(source);
            writer.advance_time(20);
            writer.flush();
            writer.close();
            vt::Reader reader(path);
            check(reader.lookup(tx.id())->end() == 20,
                  "empty time advance did not extend the open transaction to recording end");
            (void)reader.validate();
        }
    }
}

void signal_free_time_range(const std::filesystem::path& root) {
    for (auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded}) {
        for (auto codec : {vt::Codec::stored, vt::Codec::lz4, vt::Codec::zstd}) {
            const auto path = root / ("signal-free-range-" + std::to_string(int(mode)) + "-" +
                                      std::to_string(int(codec)) + ".vtr");
            {
                vt::Writer writer(path, {.mode = mode, .encoding = {.compression = codec}});
                for (unsigned i = 0; i < 50; ++i) {
                    writer.advance_time(5 + 3 * i);
                    if (i % 7 == 0)
                        writer.flush();
                }
                check(writer.time() == 152, "signal-free progress lost the current time");
                writer.close();
            }
            vt::Reader reader(path);
            const auto range = reader.waveform_time_range();
            check(range && range->first <= 5 && range->last == 152,
                  "signal-free recording lost explicit time advances");
            unsigned changes = 0;
            reader.changes({}, {}, [&](const auto&) {
                ++changes;
                return vt::Flow::next;
            });
            check(changes == 0, "signal-free time generated signal observations");
            (void)reader.validate();
        }
    }
}

void inline_contract(const std::filesystem::path& root) {
    const auto path = root / "inline.vtr";
    vt::Writer writer(path, {.mode = vt::ExecutionMode::inline_,
                             .buffer = {.queue_bytes = 0, .pending_bytes = 0},
                             .encoding = {.queue_jobs = 0, .queue_bytes = 0}});
    auto source = writer.transactions<std::string_view>({.name = "request"});
    rejects([&] { (void)writer.add_producer(); });
    writer.advance_time(10);
    auto tx = writer.begin(source, std::string_view("copied"));
    rejects([&] { writer.advance_time(9); });
    check(writer.time() == 10, "rejected advance changed inline time");
    writer.advance_time(20);
    writer.finish_producer();
    rejects([&] { (void)writer.begin(source, std::string_view("late")); });
    writer.flush();
    writer.close();
    writer.close();
    vt::Reader reader(path);
    auto record = reader.lookup(tx.id());
    check(record && record->begin() == 10 && record->end() == 20 &&
              record->payload().as<std::string>() == "copied",
          "inline final progress and captured payload");
    (void)reader.validate();
    rejects([&] {
        vt::Writer invalid(root / "inline-workers.vtr",
                           {.mode = vt::ExecutionMode::inline_, .encoding = {.workers = 1}});
    });
    {
        vt::Writer unfinished(root / "inline-unfinished.vtr", {.mode = vt::ExecutionMode::inline_});
        auto site = unfinished.transactions({.name = "unfinished"});
        (void)unfinished.begin(site);
    }
    check(vt::Reader(root / "inline-unfinished.vtr").ending().state == vt::Completion::incomplete,
          "inline destruction must seal incomplete evidence");
    {
        vt::Writer failed(root / "inline-failed.vtr", {.mode = vt::ExecutionMode::inline_});
        auto site = failed.transactions<std::uint32_t>({.name = "missing"});
        auto tx = failed.begin(site);
        rejects([&] { failed.end(tx); });
        rejects([&] { failed.close(); });
        failed.seal();
    }
    check(vt::Reader(root / "inline-failed.vtr").ending().state == vt::Completion::incomplete,
          "inline failure must preserve incomplete evidence");
}

template <class T>
void fixed_array_values(const std::filesystem::path& root, std::string_view name,
                        const std::array<T, 3>& expected) {
    auto path = root / (std::string(name) + ".vtr");
    vt::RecordId id(0);
    {
        vt::Writer writer(path, {.mode = vt::ExecutionMode::inline_});
        auto source = writer.transactions<std::array<T, 3>>({.name = "values"});
        auto record = writer.begin(source, expected);
        id = record.id();
        writer.end(record);
        writer.close();
    }
    std::optional<vt::PayloadView> retained;
    {
        vt::Reader reader(path);
        retained = reader.lookup(id)->payload();
        rejects([&] { (void)retained->template as<std::array<T, 2>>(); });
        (void)reader.validate();
    }
    auto actual = retained->template as<std::array<T, 3>>();
    for (unsigned i = 0; i < actual.size(); ++i) {
        check(std::bit_cast<std::array<std::byte, sizeof(T)>>(actual[i]) ==
                  std::bit_cast<std::array<std::byte, sizeof(T)>>(expected[i]),
              "retained fixed-array bits changed");
    }
}

void fixed_arrays(const std::filesystem::path& root) {
    fixed_array_values<std::int8_t>(root, "i8", {INT8_MIN, 0, INT8_MAX});
    fixed_array_values<std::uint8_t>(root, "u8", {0, 17, UINT8_MAX});
    fixed_array_values<std::int16_t>(root, "i16", {INT16_MIN, 0, INT16_MAX});
    fixed_array_values<std::uint16_t>(root, "u16", {0, 17, UINT16_MAX});
    fixed_array_values<std::int32_t>(root, "i32", {INT32_MIN, 0, INT32_MAX});
    fixed_array_values<std::uint32_t>(root, "u32", {0, 17, UINT32_MAX});
    fixed_array_values<std::int64_t>(root, "i64", {INT64_MIN, 0, INT64_MAX});
    fixed_array_values<std::uint64_t>(root, "u64", {0, 17, UINT64_MAX});
    fixed_array_values<float>(root, "f32",
                              {-0.0f, std::bit_cast<float>(UINT32_C(0x7fc01234)), 1.0f});
    fixed_array_values<double>(root, "f64",
                               {-0.0, std::bit_cast<double>(UINT64_C(0x7ff8000000001234)), 1.0});
}

class LogPunctuation : public std::numpunct<char> {
  public:
    explicit LogPunctuation(std::string groups = "\3") : groups_(std::move(groups)) {}

  private:
    std::string groups_;

    char do_decimal_point() const override {
        return ',';
    }

    char do_thousands_sep() const override {
        return '_';
    }

    std::string do_grouping() const override {
        return groups_;
    }

    std::string do_truename() const override {
        return "yes";
    }
};

void log_locale_and_limits(const std::filesystem::path& path) {
    vt::Writer writer(path);
    auto integers = writer.logs<std::uint64_t>({.name = "{:L}"});
    auto floats = writer.logs<double>({.name = "{:.1Lf}"});
    auto booleans = writer.logs<bool>({.name = "{:L}"});
    auto wide = writer.logs<std::uint64_t>({.name = "{:65537}"});
    const std::array expected{
        std::pair{writer.log(integers, std::uint64_t{1234567}), "1_234_567"},
        std::pair{writer.log(floats, 1234.5), "1_234,5"},
        std::pair{writer.log(booleans, true), "yes"},
        std::pair{writer.log(wide, std::uint64_t{1}), "{?}"},
    };
    std::vector<std::pair<vt::RecordId, std::string>> fixed_expected;
    const std::locale punctuation(std::locale::classic(), new LogPunctuation);
    for (double value :
         {std::numeric_limits<double>::max(), -std::numeric_limits<double>::max(), -0.0, 999.5}) {
        std::ostringstream stream;
        stream.imbue(punctuation);
        stream << std::fixed << std::showpoint << std::setprecision(0) << value;
        const auto body = stream.str();
        for (const auto& [spec, text] : std::vector<std::pair<std::string, std::string>>{
                 {"#.0Lf", body},
                 {"#420.0Lf", std::format("{:>420}", body)},
                 {"*<#420.0Lf", std::format("{:*<420}", body)},
                 {"界^#420.0Lf", std::format("{:界^420}", body)},
                 {"#0420.0Lf",
                  [&] {
                      auto zero = body;
                      zero.insert(value < 0 || std::signbit(value) ? 1 : 0,
                                  body.size() < 420 ? 420 - body.size() : 0, '0');
                      return zero;
                  }()},
             }) {
            auto site = writer.logs<double>({.name = "{:" + spec + "}"});
            fixed_expected.emplace_back(writer.log(site, value), text);
        }
    }
    auto hash_fill = writer.logs<double>({.name = "{:#>8.0f}"});
    fixed_expected.emplace_back(writer.log(hash_fill, 12.0), "######12");
    auto signed_fixed = writer.logs<double>({.name = "{:+#012.0Lf}"});
    fixed_expected.emplace_back(writer.log(signed_fixed, 1234.0), "+000001_234,");
    auto stop_group = writer.logs<double>({.name = "{:*^#20.0Lf}"});
    const auto stop_group_id = writer.log(stop_group, 123456789.5);
    writer.close();
    vt::Reader reader(path);

    struct LocaleReset {
        std::locale previous = std::locale();

        ~LocaleReset() {
            std::locale::global(previous);
        }
    } reset;

    std::locale::global(std::locale(std::locale::classic(), new LogPunctuation));
    for (const auto& [id, text] : expected)
        check(reader.format_log(*reader.lookup(id)) == text,
              "log formatting ignores the global locale or exceeds limits");
    for (const auto& [id, text] : fixed_expected)
        check(reader.format_log(*reader.lookup(id)) == text,
              "alternate fixed precision-zero formatting is corrupted");
    std::locale::global(
        std::locale(std::locale::classic(), new LogPunctuation(std::string("\3\0", 2))));
    check(reader.format_log(*reader.lookup(stop_group_id)) == "****123456_790,*****",
          "alternate fixed formatting repeats a stopped digit group");
    for (const auto& [groups, text] : std::vector<std::pair<std::string, std::string>>{
             {"", "1234567"}, {"\3", "1_234_567"}, {std::string("\3\0", 2), "1234_567"},
             {std::string{3, CHAR_MAX}, "1234_567"}, {std::string(1, 0), "1234567"},
             {std::string(1, static_cast<char>(-1)), "1234567"}}) {
        std::locale::global(std::locale(std::locale::classic(), new LogPunctuation(groups)));
        check(reader.format_log(*reader.lookup(expected[0].first)) == text,
              "localized integers ignore unlimited-group boundaries");
    }
}

void waveform_visitor_kinds(const std::filesystem::path& path, vt::ExecutionMode mode) {
    vt::WriterOptions options;
    options.mode = mode;
    options.encoding.waveform_records = 2;
    vt::Writer writer(path, options);
    const auto real = writer.signal({.name = "real"}, {.encoding = vt::SignalEncoding::real});
    const auto bits = writer.signal({.name = "bits"}, {.width = 16});
    const auto bytes = writer.signal({.name = "bytes"}, {.encoding = vt::SignalEncoding::bytes});
    const auto logic = writer.signal({.name = "logic"}, {.width = 2, .alphabet = vt::States::four});
    for (std::uint64_t time : {0, 10}) {
        writer.advance_time(time);
        writer.emit_real(real, time == 0 ? -0.0 : 1.25);
        writer.emit(bits, time == 0 ? 0x1234 : 6);
        writer.emit_bytes(bytes, vt::Bytes{std::byte(time), std::byte{0xff}});
        writer.emit_packed(logic, vt::Bytes{time == 0 ? std::byte{6} : std::byte{15}});
    }
    writer.close();
    vt::Reader reader(path);
    unsigned seen = 0;
    reader.changes({}, {}, [&](const vt::SignalChange& change) {
        const auto& value = change.value;
        if (change.signal == real) {
            check(value.kind.encoding == vt::SignalEncoding::real &&
                      std::bit_cast<std::uint64_t>(value.as_real()) ==
                          std::bit_cast<std::uint64_t>(change.time == 0 ? -0.0 : 1.25),
                  "real visitor kind/value");
        } else if (change.signal == bits) {
            check(value.kind.width == 16 && value.kind.alphabet == vt::States::binary &&
                      value.as_u64() == (change.time == 0 ? 0x1234u : 6u),
                  "integer visitor kind/value");
        } else if (change.signal == bytes) {
            check(value.kind.encoding == vt::SignalEncoding::bytes && value.packed.size() == 2 &&
                      value.packed[0] == std::byte(change.time) &&
                      value.packed[1] == std::byte{0xff},
                  "byte visitor kind/value");
        } else {
            check(change.signal == logic && value.kind.width == 2 &&
                      value.kind.alphabet == vt::States::four &&
                      value.as_text() == (change.time == 0 ? "1x" : "zz"),
                  "logic visitor kind/value");
        }
        ++seen;
        return vt::Flow::next;
    });
    check(seen == 8, "mixed visitor omitted samples");
    seen = 0;
    reader.changes({}, {}, [&](const vt::SignalChange&) {
        ++seen;
        return vt::Flow::stop;
    });
    check(seen == 1, "waveform visitor ignored cancellation");
    (void)reader.validate();
}

int main(int argc, char** argv) {
    try {
        auto root = std::filesystem::path(argc > 1 ? argv[1] : "api-fixtures");
        std::filesystem::create_directories(root);
        for (auto workers : {0u, 2u}) {
            for (auto codec : {vt::Codec::stored, vt::Codec::lz4, vt::Codec::zstd}) {
                lifecycle(root / (std::to_string(workers) + "-" +
                                  std::to_string(static_cast<int>(codec)) + ".vtr"),
                          workers, codec);
            }
        }
        for (auto codec : {vt::Codec::stored, vt::Codec::lz4, vt::Codec::zstd})
            lifecycle(root / ("inline-" + std::to_string(static_cast<int>(codec)) + ".vtr"), 0,
                      codec, vt::ExecutionMode::inline_);
        fixed_arrays(root);
        log_locale_and_limits(root / "log-locale.vtr");
        borrowed_and_errors(root);
        allocation_limits(root / "allocation.vtr");
        concurrent(root / "concurrent.vtr");
        scalar_attributes(root / "attributes-inline.vtr", vt::ExecutionMode::inline_);
        scalar_attributes(root / "attributes-threaded.vtr", vt::ExecutionMode::threaded);
        for (auto mode : {vt::ExecutionMode::inline_, vt::ExecutionMode::threaded})
            u64_attribute_contract(root, mode);
        progress_without_observations(root);
        signal_free_time_range(root);
        inline_contract(root);
        waveform_visitor_kinds(root / "wave-kinds-inline.vtr", vt::ExecutionMode::inline_);
        waveform_visitor_kinds(root / "wave-kinds-threaded.vtr", vt::ExecutionMode::threaded);
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
