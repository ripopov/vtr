#include "engine/records.h"
#include "format.h"
#include <algorithm>
#include <iostream>
#include <map>
#include <random>
#include <source_location>

using namespace vtr::detail::engine;
namespace d = vtr::detail::wire::detail;

void check(bool good, const char* message) {
    if (!good) {
        throw std::runtime_error(message);
    }
}

template <class F>
void rejects(F&& action, std::source_location location = std::source_location::current()) {
    try {
        action();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("malformed record block accepted at line " +
                             std::to_string(location.line()));
}

std::vector<LocatedRecord> select_source(const std::vector<LocatedRecord>& source,
                                         const Generators& generators, RecordQuery q) {
    std::vector<LocatedRecord> rows;
    for (const auto& r : source) {
        if (q.generator && *q.generator != r.generator) {
            continue;
        }
        if (q.parent && *q.parent != r.parent) {
            continue;
        }
        if (!q.include_logs && generators[r.generator].kind == RecordKind::log) {
            continue;
        }
        if (r.begin > q.end || (r.begin == r.end ? r.begin < q.begin : r.end <= q.begin)) {
            continue;
        }
        rows.push_back(r);
    }
    return rows;
}

void same_records(std::vector<LocatedRecord> a, std::vector<LocatedRecord> b) {
    auto by_id = [](const auto& a, const auto& b) { return a.id < b.id; };
    std::sort(a.begin(), a.end(), by_id);
    std::sort(b.begin(), b.end(), by_id);
    check(a == b, "record query changed occurrences or payloads");
}

void compare_query(const RecordBlockView& block, const std::vector<LocatedRecord>& source,
                   const Generators& generators, RecordQuery query) {
    std::vector<LocatedRecord> actual;
    block.query(query, [&](const auto& row) {
        actual.push_back(row);
        return true;
    });
    same_records(std::move(actual), select_source(source, generators, query));
}

void roundtrips() {
    Types types;
    auto payload_type = types.structure("Visit", {{"serial", 8}, {"note", 13}, {"address", 9}});
    Generators generators;
    for (unsigned i = 0; i < 7; ++i) {
        generators.declare({100 + i,
                            i % 3 == 0   ? RecordKind::stage
                            : i % 3 == 1 ? RecordKind::event
                                         : RecordKind::log,
                            payload_type});
    }
    std::vector<LocatedRecord> source;
    std::mt19937_64 random(0x951a3907);
    for (unsigned i = 0; i < 12000; ++i) {
        auto generator = 100 + i % 7;
        auto begin = random() % 2000;
        auto end = begin + (generators[generator].kind == RecordKind::stage ? random() % 10 : 0);
        Payload row;
        row.scalar(std::uint32_t(i));
        row.append(i == 300 ? std::string(70000, 'x')
                   : i % 13 ? std::string("bank Δ")
                            : std::string(""));
        row.scalar(random());
        auto parent = generators[generator].kind == RecordKind::log && i % 2 ? 0 : 1 + i % 3;
        source.push_back({10 + i, generator, begin, end, parent, std::move(row).take()});
    }
    std::reverse(source.begin() + 100, source.begin() + 300);
    source.push_back({UINT64_MAX - 2, 100, 0, UINT64_MAX, 1, source[0].payload});
    source.push_back(
        {UINT64_MAX - 1, 101, UINT64_MAX, UINT64_MAX, UINT64_MAX - 2, source[1].payload});
    source.push_back({UINT64_MAX, 100, UINT64_MAX, UINT64_MAX, 2, source[2].payload});
    for (auto codec : {vtr::detail::wire::Codec::stored, vtr::detail::wire::Codec::lz4,
                       vtr::detail::wire::Codec::zstd}) {
        for (auto separate_logs : {false, true}) {
            auto encoded =
                encode_record_block(23, types, generators, source, {512, codec, 1, separate_logs});
            auto exact = encode_record_block(23, types, generators, source,
                                             {512, codec, 1, separate_logs, false});
            check(encoded == exact, "deferred activity sizing changed page boundaries or bytes");
            RecordBlockView block(encoded, types, generators);
            check(block.number() == 23 && block.count() == source.size(),
                  "block identity/count changed");
            block.validate();
            compare_query(block, source, generators, {});
            for (auto generator : {100u, 101u, 102u, 106u, 199u}) {
                compare_query(block, source, generators,
                              {.generator = generator, .parent = {}, .begin = 400, .end = 500});
            }
            for (RecordId owner : {RecordId(0), RecordId(1), RecordId(3), UINT64_MAX - 2}) {
                compare_query(block, source, generators, {.generator = {}, .parent = owner});
                compare_query(block, source, generators,
                              {.generator = {},
                               .parent = owner,
                               .begin = 0,
                               .end = UINT64_MAX,
                               .include_logs = false});
            }
            for (Timestamp time : {Timestamp(0), Timestamp(2000), UINT64_MAX}) {
                compare_query(block, source, generators,
                              {.generator = {}, .parent = {}, .begin = time, .end = time});
            }
            std::map<GeneratorId, std::vector<RecordId>> expected, actual;
            for (const auto& r : source) {
                expected[r.generator].push_back(r.id);
            }
            block.query({}, [&](const auto& r) {
                actual[r.generator].push_back(r.id);
                return true;
            });
            check(expected == actual, "page splitting changed per-generator assignment order");
            for (unsigned p = 0; p < block.pages().size(); ++p) {
                bool activity = false, logs = false;
                std::size_t rows = 0;
                for (const auto& segment : block.segments()) {
                    if (segment.page == p) {
                        (generators[segment.generator].kind == RecordKind::log ? logs : activity) =
                            true;
                        rows += segment.count;
                    }
                }
                check(!separate_logs || !(activity && logs),
                      "log-separation policy mixed page kinds");
                check(block.pages()[p].raw_length <= 512 || rows == 1,
                      "page target exceeded without an oversized record");
            }
            unsigned visited = 0;
            block.query({}, [&](const auto&) {
                ++visited;
                return false;
            });
            check(visited == 1, "query cancellation failed");
        }
    }
}

void sparse_generators() {
    Types types;
    const auto text = types.bind<std::string>();
    const std::array<Generator, 4> declarations{{
        {0, RecordKind::event, text}, {255, RecordKind::stage, text},
        {256, RecordKind::log, 9}, {UINT32_MAX - 1, RecordKind::event, 9}}};
    auto snapshot = [&] {
        Generators source;
        for (const auto& generator : declarations)
            source.declare(generator);
        const auto& retained = source[255];
        for (auto id : {0u, 255u, 256u, UINT32_MAX - 1})
            rejects([&] { source.declare({id, RecordKind::clock, 0}); });
        rejects([&] { source.declare({1, RecordKind::clock, text}); });
        rejects([&] { source.declare({1, static_cast<RecordKind>(255), text}); });
        rejects([&] { source.declare({UINT32_MAX, RecordKind::event, text}); });
        Generators copy = source;
        for (GeneratorId id = 1; id < 4096; ++id) {
            if (id != 255 && id != 256)
                source.declare({id, RecordKind::event, text});
        }
        check(retained.kind == RecordKind::stage && retained.payload_type == text,
              "generator growth or rejected duplicate changed a retained declaration");
        return copy;
    }();
    Generators assigned;
    assigned.declare({1, RecordKind::event, text});
    assigned = snapshot;
    Generators moved = std::move(assigned);
    snapshot = std::move(moved);
    check(snapshot.size() == declarations.size(), "generator snapshot changed size");
    for (auto id : {1u, 254u, 257u, 4095u, UINT32_MAX})
        rejects([&] { (void)snapshot[id]; });
    std::vector<LocatedRecord> source;
    for (const auto& generator : declarations) {
        auto payload = generator.payload_type == text ? capture(std::string("sparse Δ"))
                                                       : capture(std::uint64_t(UINT64_MAX));
        const auto end = generator.kind == RecordKind::stage ? 12u : 9u;
        source.push_back({10 + source.size(), generator.id, 9, end, 1, std::move(payload)});
    }
    auto encoded = encode_record_block(1, types, snapshot, source);
    RecordBlockView block(encoded, types, snapshot);
    block.validate();
    compare_query(block, source, snapshot, {});
    for (const auto& generator : declarations)
        compare_query(block, source, snapshot, {.generator = generator.id});
    for (const auto& expected : source) {
        unsigned found = 0;
        block.query({.id = expected.id}, [&](const auto& actual) {
            check(actual == expected, "sparse generator ID query changed its payload or kind");
            ++found;
            return true;
        });
        check(found == 1, "sparse generator ID query missed a record");
    }
}

void recovery_inputs() {
    Types types;
    Generators unit;
    unit.declare({7, RecordKind::event});
    const std::array malformed_unit{LocatedRecord{2, 7, 0, 0, 1, Bytes{0}}};
    rejects([&] { encode_record_block(1, types, unit, malformed_unit); });
    auto text = types.bind<std::string>();
    Generators generators;
    generators.declare({10, RecordKind::stage, text});
    generators.declare({11, RecordKind::event, text});
    generators.declare({12, RecordKind::log, text});
    auto payload = capture(std::string("same"));
    std::vector<LocatedRecord> source{{10, 10, 2, 4, 1, payload},
                                      {11, 10, 3, 5, 1, payload},
                                      {12, 11, 4, 4, 1, payload},
                                      {13, 11, 4, 4, 1, payload},
                                      {14, 12, 4, 4, 0, payload}};
    auto encoded = encode_record_block(4, types, generators, source,
                                       {64, vtr::detail::wire::Codec::stored, 0, false});
    for (std::size_t n = 0; n < encoded.size(); ++n) {
        rejects([&] {
            RecordBlockView truncated(View(encoded).first(n), types, generators);
            truncated.validate();
        });
    }
    auto duplicate_source = source;
    duplicate_source[1].id = duplicate_source[0].id;
    auto duplicated = encode_record_block(4, types, generators, duplicate_source);
    rejects([&] {
        RecordBlockView bad(duplicated, types, generators);
        bad.validate();
    });
    auto corrupt = encoded;
    corrupt[56 + 4] = 255;
    rejects([&] { RecordBlockView bad(corrupt, types, generators); });
    corrupt = encoded;
    corrupt[56 + 12] = 1;
    rejects([&] { RecordBlockView bad(corrupt, types, generators); });
    corrupt = encoded;
    corrupt[56 + 20] = 1;
    rejects([&] {
        RecordBlockView bad(corrupt, types, generators);
        bad.validate();
    });
    rejects([&] { encode_record_block(0, types, generators, source); });
    rejects([&] { encode_record_block(1, types, generators, {}); });
    source[0].parent = 0;
    rejects([&] { encode_record_block(1, types, generators, source); });
    source[0].parent = 10;
    rejects([&] { encode_record_block(1, types, generators, source); });
    source[0].parent = 1;
    source[0].end = 1;
    rejects([&] { encode_record_block(1, types, generators, source); });
    source[0].end = 4;
    source[2].end = 5;
    rejects([&] { encode_record_block(1, types, generators, source); });
    source[2].end = 4;
    source[0].generator = 199;
    rejects([&] { encode_record_block(1, types, generators, source); });
    rejects([&] { generators.declare({10, RecordKind::event, 0}); });
}

void fixture(unsigned codec) {
    Types types;
    auto type = types.structure("Visit", {{"serial", 8}, {"note", 13}});
    Generators generators;
    for (unsigned i = 0; i < 3; ++i) {
        generators.declare({10 + i, static_cast<RecordKind>(2 + i), type});
    }
    std::vector<LocatedRecord> source;
    for (unsigned i = 0; i < 200; ++i) {
        auto generator = 10 + i % 3;
        auto begin = (i * 17) % 400;
        auto end = begin + (generator == 11 ? i % 9 : 0);
        Payload payload;
        payload.scalar(std::uint32_t(i));
        payload.append(std::string("repeat Δ"));
        source.push_back({3 + i, generator, begin, end, generator == 12 && i % 2 ? 0u : 1u,
                          std::move(payload).take()});
    }
    std::reverse(source.begin() + 30, source.begin() + 80);
    source.push_back({UINT64_MAX, 11, UINT64_MAX, UINT64_MAX, UINT64_MAX - 1, source[0].payload});
    auto encoded =
        encode_record_block(7, types, generators, source,
                            {512, static_cast<vtr::detail::wire::Codec>(codec), 1, false});
    auto hex = [](View bytes) {
        static constexpr char digits[] = "0123456789abcdef";
        std::cout << '\"';
        for (auto b : bytes) {
            std::cout << digits[b >> 4] << digits[b & 15];
        }
        std::cout << '\"';
    };
    std::cout << "{\"declarations\":";
    hex(types.declarations());
    std::cout << ",\"block\":";
    hex(encoded);
    std::cout << ",\"generators\":[";
    for (unsigned i = 0; i < 3; ++i) {
        if (i) {
            std::cout << ',';
        }
        std::cout << '[' << 10 + i << ',' << 2 + i << ',' << type << ']';
    }
    std::cout << "],\"rows\":[";
    bool first = true;
    for (const auto& r : source) {
        if (!first) {
            std::cout << ',';
        }
        first = false;
        std::cout << '[' << r.id << ',' << r.generator << ',' << r.begin << ',' << r.end << ','
                  << r.parent << ',';
        hex(r.payload);
        std::cout << ']';
    }
    std::cout << "]}\n";
}

int main(int argc, char** argv) {
    if (argc == 3 && std::string_view(argv[1]) == "--fixture") {
        fixture(std::stoul(argv[2]));
        return 0;
    }

    try {
        roundtrips();
        recovery_inputs();
        sparse_generators();
        std::cout << "Validated 12003 disordered records with oversized text, overlaps, coincident "
                     "events, full-u64 deltas, six page policies/codecs, exact queries, and every "
                     "truncation of a small block\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
