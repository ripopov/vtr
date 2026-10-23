#include "engine/transactions.h"
#include "format.h"
#include <iostream>
#include <map>
#include <source_location>

using namespace vtr::detail::engine;
namespace d = vtr::detail::wire::detail;

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
    throw std::runtime_error("malformed transaction accepted at line " + std::to_string(at.line()));
}

Bytes canonical(const Transaction& tx) {
    Bytes out;
    for (auto n : {tx.id, tx.parent, tx.begin, tx.end}) {
        d::fixed(out, n);
    }
    d::fixed(out, tx.generator);
    out.push_back(tx.status);
    out.push_back(tx.span_kind);
    d::blob(out, tx.payload);
    d::encode_attrs(out, tx.attributes);
    d::put(out, tx.events.size());
    for (const auto& e : tx.events) {
        d::fixed(out, e.id);
        d::fixed(out, e.time);
        d::fixed(out, e.name);
        d::encode_attrs(out, e.attributes);
    }
    d::put(out, tx.stages.size());
    for (const auto& s : tx.stages) {
        d::fixed(out, s.id);
        d::fixed(out, s.name);
        d::fixed(out, s.lane);
        d::fixed(out, s.begin);
        out.push_back(s.end.has_value());
        if (s.end) {
            d::fixed(out, *s.end);
        }
        d::encode_attrs(out, s.attributes);
    }
    d::put(out, tx.child_blocks.size());
    for (auto b : tx.child_blocks) {
        d::fixed(out, b);
    }
    return out;
}

Bytes canonical(const Relation& relation) {
    Bytes out;
    d::fixed(out, relation.kind);
    d::fixed(out, relation.source);
    d::fixed(out, relation.destination);
    d::encode_attrs(out, relation.attributes);
    return out;
}

bool selected(const Transaction& tx, TransactionQuery q) {
    if ((q.generator && *q.generator != tx.generator) || (q.parent && *q.parent != tx.parent) ||
        (q.id && *q.id != tx.id) || tx.begin > q.end || tx.end < q.begin) {
        return false;
    }
    return !q.embedded_id || std::any_of(tx.events.begin(), tx.events.end(), [&](const auto& e) {
        return e.id == *q.embedded_id;
    }) || std::any_of(tx.stages.begin(), tx.stages.end(), [&](const auto& e) {
        return e.id == *q.embedded_id;
    });
}

void compare_query(const TransactionBlockView& view, const std::vector<Transaction>& txs,
                   TransactionQuery query) {
    std::map<RecordId, Bytes> expected, actual;
    for (const auto& tx : txs) {
        if (selected(tx, query)) {
            expected.emplace(tx.id, canonical(tx));
        }
    }
    view.transactions(query, [&](const auto& tx) {
        check(actual.emplace(tx.id, canonical(tx)).second, "query duplicated owner");
        return true;
    });
    check(actual == expected, "transaction query changed payload, identity or embedded detail");
}

void roundtrips() {
    Types types;
    auto schema = types.structure("Request", {{"serial", 9}, {"opcode", 8}, {"label", 13}});
    Generators generators;
    generators.declare({100, RecordKind::ordinary, schema});
    generators.declare({101, RecordKind::ordinary, 0});
    generators.declare({102, RecordKind::clock, 0});
    std::vector<Transaction> txs;
    for (unsigned i = 0; i < 2000; ++i) {
        Transaction tx;
        tx.id = 10 * i + 1;
        tx.parent = i % 2 ? 10 * (i / 2) + 1 : 0;
        tx.generator = 100 + i % 2;
        tx.begin = (i * 17) % 12000;
        tx.end = tx.begin + i % 99;
        tx.status = i % 5;
        tx.span_kind = i % 6;
        if (i % 2 == 0) {
            Payload row;
            row.scalar(std::uint64_t(i));
            row.scalar(std::uint32_t(i % 12));
            row.append(i == 700 ? std::string(128000, 'x') : std::string("label Δ"));
            tx.payload = std::move(row).take();
        }
        vtr::detail::wire::Value narrow(std::int64_t(-1));
        narrow.tag = vtr::detail::wire::Tag::i8;
        tx.attributes = {
            {0, narrow},
            {1, vtr::detail::wire::Value(std::bit_cast<float>(std::uint32_t(0x7fc00000 + i)))}};
        tx.events.push_back(
            {tx.id + 1, tx.begin - 1, 2, {{3, vtr::detail::wire::Value::text("event")}}});
        tx.stages.push_back({tx.id + 2,
                             4,
                             5,
                             0,
                             i == 0 ? UINT64_MAX : tx.end,
                             {{6, vtr::detail::wire::Value::string(7)}}});
        tx.stages.push_back({tx.id + 3, 8, 5, tx.end + 10, {}, {}});
        tx.child_blocks = {1, 3, 9 + i % 100};
        txs.push_back(std::move(tx));
    }
    Transaction clock;
    clock.id = 90001;
    clock.generator = 102;
    clock.begin = 100;
    clock.end = 120;
    clock.attributes = {{10, vtr::detail::wire::Value::time(5)}};
    txs.push_back(clock);
    std::reverse(txs.begin(), txs.end());
    std::vector<Relation> relations;
    for (unsigned i = 0; i < 40; ++i) {
        relations.push_back({11,
                             std::uint64_t(i % 3 ? i * 10 + 2 : 0),
                             std::uint64_t((i + 3) * 10 + 3),
                             {{12, vtr::detail::wire::Value::text("dependency")}}});
    }
    relations.push_back({11, UINT64_MAX, 0, {}});
    TransactionWorkspace workspace;
    for (auto codec : {vtr::detail::wire::Codec::stored, vtr::detail::wire::Codec::lz4,
                       vtr::detail::wire::Codec::zstd}) {
        for (auto page_bytes : {512u, 65536u}) {
            auto encoded = encode_transaction_block(17, types, generators, txs, relations,
                                                    {page_bytes, codec, 1, true}, 64);
            auto cached = encode_transaction_block(17, types, generators, txs, relations,
                                                   {page_bytes, codec, 1, true}, 64, {}, nullptr,
                                                   &workspace);
            check(cached == encoded, "column workspace changed transaction bytes");
            TransactionBlockView view(encoded, types, generators, 64);
            view.validate();
            compare_query(view, txs, {});
            compare_query(view, txs,
                          {.generator = 101,
                           .parent = {},
                           .id = {},
                           .embedded_id = {},
                           .begin = 300,
                           .end = 600});
            for (auto id : {RecordId(1), RecordId(501), RecordId(90001), UINT64_MAX}) {
                compare_query(view, txs, {.generator = {}, .parent = {}, .id = id});
            }
            for (auto parent : {RecordId(0), RecordId(1), RecordId(501), UINT64_MAX}) {
                compare_query(view, txs, {.generator = {}, .parent = parent});
            }
            for (auto id : {RecordId(2), RecordId(503), RecordId(504), UINT64_MAX}) {
                compare_query(view, txs,
                              {.generator = {}, .parent = {}, .id = {}, .embedded_id = id});
            }
            for (bool incoming : {false, true}) {
                for (RecordId id : {RecordId(0), RecordId(2), RecordId(33), UINT64_MAX}) {
                    std::vector<Bytes> expected, actual;
                    for (const auto& relation : relations) {
                        if ((incoming ? relation.destination : relation.source) == id) {
                            expected.push_back(canonical(relation));
                        }
                    }
                    view.relations(id, incoming, [&](const auto& relation) {
                        actual.push_back(canonical(relation));
                        return true;
                    });
                    check(actual == expected, "relation query changed endpoint or attributes");
                }
            }
            for (const auto& page : view.pages()) {
                check(page.raw_length <= page_bytes || page.transactions + page.relations == 1,
                      "transaction page target exceeded");
            }
            unsigned count = 0;
            view.transactions({}, [&](const auto&) {
                ++count;
                return false;
            });
            check(count == 1, "transaction query cancellation failed");
        }
    }
    auto relation_only = encode_transaction_block(
        8, types, generators, {}, relations, {64, vtr::detail::wire::Codec::zstd, 1, true}, 64);
    TransactionBlockView relation_view(relation_only, types, generators, 64);
    relation_view.validate();
    check(relation_view.bounds().transactions == 0 &&
              relation_view.bounds().relations == relations.size(),
          "relation-only block bounds changed");
}

void malformed() {
    Types types;
    Generators generators;
    generators.declare({10, RecordKind::ordinary, 0});
    generators.declare({11, RecordKind::event, 0});
    Transaction tx;
    tx.id = 1;
    tx.generator = 10;
    tx.begin = 20;
    tx.end = 40;
    tx.events.push_back({2, 19, 3, {}});
    tx.stages.push_back({3, 4, 5, 100, {}, {}});
    std::vector<Transaction> source{tx};
    auto encoded = encode_transaction_block(1, types, generators, source, {},
                                            {64, vtr::detail::wire::Codec::stored, 0, true}, 16);
    for (std::size_t n = 0; n < encoded.size(); ++n) {
        rejects([&] {
            TransactionBlockView partial(View(encoded).first(n), types, generators, 16);
            partial.validate();
        });
    }
    auto duplicate = source;
    duplicate[0].stages[0].id = 2;
    auto duplicate_wire = encode_transaction_block(1, types, generators, duplicate, {});
    rejects([&] {
        TransactionBlockView invalid(duplicate_wire, types, generators);
        invalid.validate();
    });
    auto wrong = source;
    wrong[0].status = 5;
    rejects([&] { encode_transaction_block(1, types, generators, wrong, {}); });
    wrong = source;
    wrong[0].generator = 11;
    rejects([&] { encode_transaction_block(1, types, generators, wrong, {}); });
    wrong = source;
    wrong[0].parent = 1;
    rejects([&] { encode_transaction_block(1, types, generators, wrong, {}); });
    wrong = source;
    wrong[0].child_blocks = {2, 2};
    rejects([&] { encode_transaction_block(1, types, generators, wrong, {}); });
    wrong = source;
    wrong[0].events[0].id = 0;
    rejects([&] { encode_transaction_block(1, types, generators, wrong, {}); });
    wrong = source;
    wrong[0].events[0].name = 16;
    rejects([&] { encode_transaction_block(1, types, generators, wrong, {}, {}, 16); });
    rejects([&] { encode_transaction_block(1, types, generators, {}, {}); });
    for (bool deferred : {false, true}) {
        wrong = source;
        wrong[0].payload = {1};
        rejects([&] {
            encode_transaction_block(1, types, generators, wrong, {},
                                     {64, vtr::detail::wire::Codec::stored, 0, true, deferred});
        });
    }
    auto corrupt = encoded;
    corrupt[128] = 255;
    rejects([&] {
        TransactionBlockView invalid(corrupt, types, generators, 16);
        invalid.validate();
    });
}

void attribute_validation() {
    namespace w = vtr::detail::wire;
    Types types;
    Generators generators;
    generators.declare({10, RecordKind::ordinary, 0});
    Transaction tx;
    tx.id = 1;
    tx.generator = 10;
    tx.end = 10;
    tx.events.push_back({2, 1, 1, {}});
    tx.stages.push_back({3, 2, 3, 0, 5, {}});
    const auto number = [](w::Tag tag, std::uint64_t bits) {
        w::Value value;
        value.tag = tag;
        value.number = bits;
        return value;
    };
    std::vector<Attributes> invalid{{{64, w::Value()}},
                                    {{1, w::Value()}, {1, w::Value()}},
                                    {{1, number(w::Tag::boolean, 2)}},
                                    {{1, number(w::Tag::i8, 128)}},
                                    {{1, number(w::Tag::u8, 256)}},
                                    {{1, number(w::Tag::f32, std::uint64_t(1) << 32)}},
                                    {{1, w::Value::string(64)}},
                                    {{1, number(static_cast<w::Tag>(255), 0)}}};
    auto enumeration = number(w::Tag::enumeration, UINT64_MAX);
    enumeration.literal = 64;
    invalid.push_back({{1, enumeration}});
    w::Value text;
    text.tag = w::Tag::text;
    d::blob(text.payload, Bytes{0xc0, 0x80});
    invalid.push_back({{1, text}});
    w::Value bits;
    bits.tag = w::Tag::bits;
    d::put(bits.payload, 3);
    bits.payload.push_back(255);
    invalid.push_back({{1, bits}});
    const std::array child{w::Value::string(64)};
    invalid.push_back({{1, w::Value::list(child)}});
    w::Value map;
    map.tag = w::Tag::map;
    d::put(map.payload, 1);
    d::put(map.payload, 64);
    d::encode_value(map.payload, w::Value());
    invalid.push_back({{1, map}});
    for (const auto& attrs : invalid) {
        for (unsigned location = 0; location < 4; ++location) {
            std::vector<Transaction> rows{tx};
            std::array relations{Relation{4, 1, 1, {}}};
            if (location == 0)
                rows[0].attributes = attrs;
            if (location == 1)
                rows[0].events[0].attributes = attrs;
            if (location == 2)
                rows[0].stages[0].attributes = attrs;
            if (location == 3)
                relations[0].attributes = attrs;
            rejects([&] {
                encode_transaction_block(1, types, generators, rows, relations,
                                         {128, w::Codec::stored, 0, true}, 64);
            });
        }
    }
    tx.attributes = {
        {20, w::Value(UINT64_MAX)}, {3, w::Value(INT64_MIN)}, {12, w::Value::time(UINT64_MAX)}};
    tx.events[0].attributes = tx.stages[0].attributes = tx.attributes;
    const std::vector<Transaction> rows{tx};
    const std::array relations{Relation{4, 1, 1, tx.attributes}};
    auto data = encode_transaction_block(1, types, generators, rows, relations,
                                         {128, w::Codec::stored, 0, true}, 64);
    TransactionBlockView view(data, types, generators, 64);
    view.validate();
    compare_query(view, rows, {});
    view.relations(1, false, [&](const auto& relation) {
        check(canonical(relation) == canonical(relations[0]),
              "unordered relation attributes changed");
        return true;
    });
}

Attributes fixture_attributes() {
    Attributes result;
    for (unsigned tag = 0; tag <= 24; ++tag) {
        vtr::detail::wire::Value value;
        value.tag = static_cast<vtr::detail::wire::Tag>(tag);
        if (tag >= 2 && tag <= 5) {
            value.number = UINT64_MAX;
        }
        if (tag >= 6 && tag <= 9) {
            value.number = 127;
        }
        if (tag == 5) {
            value.number = std::uint64_t(1) << 63;
        }
        if (tag == 9 || tag == 18 || tag == 20) {
            value.number = UINT64_MAX;
        }
        if (tag == 10) {
            value.number = 0x7fc0beef;
        }
        if (tag == 11) {
            value.number = 0x7ff80000beef1234;
        }
        if (tag == 12 || tag == 19) {
            value.literal = 42;
        }
        if (tag == 13) {
            value = vtr::detail::wire::Value::text("attribute Δ");
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
                              vtr::detail::wire::Value::text("nested")};
            value = vtr::detail::wire::Value::list(values);
        }
        if (tag == 24) {
            d::put(value.payload, 1);
            d::put(value.payload, 31);
            d::encode_value(value.payload, vtr::detail::wire::Value::string(32));
        }
        result.push_back({tag, std::move(value)});
    }
    return result;
}

void numeric_attributes() {
    namespace w = vtr::detail::wire;
    NumericAttributeColumns columns;
    for (unsigned i = 0; i < 20; ++i)
        columns.append({});
    check(!columns.owned_bytes(), "empty numeric rows allocated storage");
    columns.view().validate(20);
    columns.clear();
    Types types;
    Generators generators;
    generators.declare({100, RecordKind::ordinary, 9});
    generators.declare({101, RecordKind::ordinary, 9});
    std::vector<Transaction> expected;
    for (unsigned i = 0; i < 81; ++i) {
        Transaction tx;
        tx.id = 81 - i;
        tx.generator = 100 + i % 2;
        tx.begin = i;
        tx.end = i + 5;
        tx.payload = capture(std::uint64_t(i));
        if (i % 4) {
            for (const auto& attribute : fixture_attributes()) {
                if (numeric_attribute(attribute.value.tag))
                    tx.attributes.push_back(attribute);
            }
            if (i % 2) {
                tx.attributes[1].value.number = 1;
                for (unsigned tag = 2; tag <= 5; ++tag) {
                    auto bits = 8u << (tag - 2);
                    tx.attributes[tag].value.number = UINT64_MAX << (bits - 1);
                }
                for (unsigned tag = 6; tag <= 9; ++tag) {
                    auto bits = 8u << (tag - 6);
                    tx.attributes[tag].value.number = UINT64_MAX >> (64 - bits);
                }
                tx.attributes[10].value.number = 0x80000000;
                tx.attributes[11].value.number = 0x8000000000000000;
                tx.attributes[12].value.number = UINT64_MAX;
                tx.attributes[13].value.number = UINT64_MAX;
            }
        }
        std::vector<NumericAttribute> row;
        for (const auto& attribute : tx.attributes)
            row.push_back({attribute.key, attribute.value.tag, attribute.value.number});
        columns.append(row);
        expected.push_back(std::move(tx));
    }
    auto compact = expected;
    for (auto& tx : compact)
        tx.attributes.clear();
    columns.view().validate(compact.size());
    TransactionWorkspace workspace;
    for (auto codec : {w::Codec::stored, w::Codec::lz4, w::Codec::zstd}) {
        for (auto bytes : {256u, 65536u}) {
            for (bool deferred : {false, true}) {
                RecordPolicy policy{bytes, codec, 1, true, deferred};
                auto rich =
                    encode_transaction_block(1, types, generators, expected, {}, policy, 64);
                auto packed = encode_transaction_block(1, types, generators, compact, {}, policy,
                                                       64, columns.view());
                auto cached = encode_transaction_block(1, types, generators, compact, {}, policy,
                                                       64, columns.view(), nullptr, &workspace);
                check(cached == packed, "reused workspace changed dense numeric row sorting");
                check(rich == packed, "numeric columns changed encoded bytes");
                TransactionBlockView view(packed, types, generators, 64);
                view.validate();
                compare_query(view, expected, {});
                compare_query(view, expected, {.generator = {}, .parent = {}, .id = 40});
            }
        }
    }
    auto invalid = [&](NumericAttributesView view) {
        rejects([&] { encode_transaction_block(1, types, generators, compact, {}, {}, 64, view); });
    };
    auto view = columns.view();
    invalid({view.values, {}});
    invalid({view.values, view.offsets.first(2)});
    auto offsets = std::vector<std::size_t>(view.offsets.begin(), view.offsets.end());
    offsets[3] = SIZE_MAX;
    invalid({view.values, offsets});
    auto values = std::vector<NumericAttribute>(view.values.begin(), view.values.end());
    values[0].key = 64;
    invalid({values, view.offsets});
    values[0] = view.values[0];
    values[1].key = values[0].key;
    invalid({values, view.offsets});
    values[1] = view.values[1];
    values[0].tag = w::Tag::text;
    invalid({values, view.offsets});
    values[0] = view.values[0];
    values[1].number = 2;
    invalid({values, view.offsets});
    for (const auto [tag, number] : std::array{
             std::pair{w::Tag::boolean, std::uint64_t(2)},
             std::pair{w::Tag::i8, std::uint64_t(128)},
             std::pair{w::Tag::i16, std::uint64_t(32768)},
             std::pair{w::Tag::i32, std::uint64_t(0x80000000)},
             std::pair{w::Tag::u8, std::uint64_t(256)},
             std::pair{w::Tag::u16, std::uint64_t(65536)},
             std::pair{w::Tag::u32, std::uint64_t(1) << 32}, std::pair{w::Tag::f32, UINT64_MAX},
             std::pair{static_cast<w::Tag>(255), std::uint64_t(0)}}) {
        values.assign(view.values.begin(), view.values.end());
        values[0].tag = tag;
        values[0].number = number;
        invalid({values, view.offsets});
    }
    offsets.assign(view.offsets.begin(), view.offsets.end());
    offsets.front() = 1;
    invalid({view.values, offsets});
    offsets.front() = 0;
    --offsets.back();
    invalid({view.values, offsets});
    compact[1].attributes = {{31, w::Value(std::uint64_t(1))}};
    invalid(view);
    NumericAttributeColumns moved(std::move(columns));
    moved.view().validate(expected.size());
    columns.clear();
    columns.append({});
    columns.view().validate(1);
    moved.clear();
    std::array row{NumericAttribute{1, w::Tag::u64, 123}};
    moved.append(row);
    moved.view().validate(1);
    check(moved.view().row(0)[0].number == 123, "numeric column reuse retained stale rows");
}

void workspace_recovery() {
    Types types;
    Generators generators;
    generators.declare({100, RecordKind::ordinary, 13});
    generators.declare({101, RecordKind::ordinary});
    TransactionWorkspace workspace;
    Transaction large;
    large.id = UINT64_MAX;
    large.generator = 100;
    large.end = 5;
    large.payload = capture(std::string(3 << 20, 'x'));
    std::array rows{large};
    auto encode = [&](const auto& input, TransactionWorkspace* reuse) {
        return encode_transaction_block(1, types, generators, input, {},
                                         {512, vtr::detail::wire::Codec::stored}, 64,
                                         {}, nullptr, reuse);
    };
    check(encode(rows, &workspace) == encode(rows, nullptr),
          "oversized workspace row changed encoded bytes");
    std::size_t retained = (workspace.sorted.capacity() + workspace.slots.capacity()) *
                           sizeof(const Transaction*);
    for (const auto& column : workspace.buffers) {
        check(column.empty(), "workspace retained encoded content");
        retained += column.capacity();
    }
    check(workspace.sorted.empty() && workspace.slots.empty() && retained <= (2 << 20),
          "workspace retained borrowed rows or oversized storage");
    rows[0].payload = Bytes{0};
    rejects([&] { encode(rows, &workspace); });
    rows[0].payload = capture(std::string("after rejection"));
    check(encode(rows, &workspace) == encode(rows, nullptr),
          "failed encoding left stale workspace state");
    rows[0].id = 1;
    rows[0].generator = 101;
    rows[0].payload.clear();
    check(encode(rows, &workspace) == encode(rows, nullptr),
          "workspace reuse changed scalar-page encoding");
}

void unsigned_attribute_rows() {
    namespace w = vtr::detail::wire;
    Types types;
    Generators generators;
    generators.declare({100, RecordKind::ordinary});
    const std::array<w::Id, 9> keys{UINT32_MAX - 1, 1, 127, 128, 16383, 16384,
                                      2097151, 2097152, 268435456};
    const std::array<std::uint64_t, 9> values{UINT64_MAX, 0, 127, 128, 16383, 16384,
                                               std::uint64_t(1) << 35,
                                               std::uint64_t(1) << 63, 42};
    for (auto count : {1u, 2u, 7u, 8u, 9u}) {
        std::vector<Transaction> expected(2);
        NumericAttributeColumns columns;
        for (unsigned row = 0; row < expected.size(); ++row) {
            auto& tx = expected[row];
            tx.id = 2 - row;
            tx.generator = 100;
            tx.end = row + 1;
            std::vector<NumericAttribute> numeric;
            for (unsigned i = 0; i < count; ++i) {
                const auto value = values[(i + row) % values.size()];
                tx.attributes.push_back({keys[i], w::Value(value)});
                numeric.push_back({keys[i], w::Tag::u64, value});
            }
            columns.append(numeric);
        }
        auto compact = expected;
        for (auto& tx : compact)
            tx.attributes.clear();
        for (auto codec : {w::Codec::stored, w::Codec::lz4, w::Codec::zstd}) {
            for (auto bytes : {256u, 65536u}) {
                RecordPolicy policy{bytes, codec};
                const auto rich = encode_transaction_block(1, types, generators, expected, {},
                                                           policy, UINT32_MAX);
                const auto packed = encode_transaction_block(1, types, generators, compact, {},
                                                             policy, UINT32_MAX, columns.view());
                check(rich == packed, "unsigned row batching changed canonical bytes");
                TransactionBlockView view(packed, types, generators, UINT32_MAX);
                view.validate();
                compare_query(view, expected, {});
                compare_query(view, expected, {.id = 1});
            }
        }
        const auto source = columns.view();
        auto invalid = std::vector<NumericAttribute>(source.values.begin(), source.values.end());
        invalid.back().key = UINT32_MAX;
        rejects([&] {
            encode_transaction_block(1, types, generators, compact, {}, {}, UINT32_MAX,
                                     {invalid, source.offsets});
        });
        if (count > 1) {
            invalid.assign(source.values.begin(), source.values.end());
            invalid.back().key = invalid[invalid.size() - count].key;
            rejects([&] {
                encode_transaction_block(1, types, generators, compact, {}, {}, UINT32_MAX,
                                         {invalid, source.offsets});
            });
        }
    }
}

void fixture(unsigned codec) {
    Types types;
    auto schema = types.structure("Request", {{"serial", 9}, {"label", 13}});
    Generators generators;
    generators.declare({100, RecordKind::ordinary, schema});
    generators.declare({101, RecordKind::ordinary, 0});
    generators.declare({102, RecordKind::clock, 0});
    std::vector<Transaction> source;
    for (unsigned i = 0; i < 8; ++i) {
        Transaction tx;
        tx.id = i * 10 + 1;
        tx.parent = i % 2 ? 1 : 0;
        tx.generator = 100 + i % 2;
        tx.begin = i * 3;
        tx.end = tx.begin + 5;
        tx.status = i % 5;
        tx.span_kind = i % 6;
        tx.attributes = fixture_attributes();
        if (tx.generator == 100) {
            Payload payload;
            payload.scalar(std::uint64_t(UINT64_MAX - i));
            payload.append(std::string(i % 4 == 0 ? "payload Δ" : "local"));
            tx.payload = std::move(payload).take();
        }
        tx.events.push_back(
            {tx.id + 1, UINT64_MAX - i, 40, {{41, vtr::detail::wire::Value::text("event")}}});
        tx.stages.push_back({tx.id + 2, 42, 43, 0, i == 0 ? UINT64_MAX : tx.end, {}});
        tx.stages.push_back({tx.id + 3,
                             44,
                             43,
                             tx.end + 5,
                             {},
                             {{45, vtr::detail::wire::Value(std::int64_t(-1))}}});
        tx.child_blocks = {1, 4, 100};
        source.push_back(std::move(tx));
    }
    Transaction clock;
    clock.id = 99;
    clock.generator = 102;
    clock.begin = 0;
    clock.end = 100;
    clock.attributes = {{46, vtr::detail::wire::Value::time(10)}};
    source.push_back(clock);
    std::reverse(source.begin(), source.end());
    std::vector<Relation> relations{
        {47, 2, 13, {{48, vtr::detail::wire::Value::text("dependency")}}},
        {47, 0, UINT64_MAX, {}},
        {47, 99, 0, {}}};
    TextReferences shared;
    shared.emplace("payload Δ", 1);
    auto encoded = encode_transaction_block(
        7, types, generators, source, relations,
        {1024, static_cast<vtr::detail::wire::Codec>(codec), 1, true}, 64, {}, &shared);
    auto hex = [](View data) {
        static constexpr char digits[] = "0123456789abcdef";
        std::cout << '\"';
        for (auto b : data) {
            std::cout << digits[b >> 4] << digits[b & 15];
        }
        std::cout << '\"';
    };
    std::cout << "{\"declarations\":";
    hex(types.declarations());
    std::cout << ",\"generators\":[[100,0," << schema << "],[101,0,0],[102,1,0]],\"block\":";
    hex(encoded);
    std::cout << ",\"global_text\":[\"\",\"payload Δ\"],\"transactions\":[";
    bool first = true;
    for (const auto& tx : source) {
        if (!first) {
            std::cout << ',';
        }
        first = false;
        hex(canonical(tx));
    }
    std::cout << "],\"relations\":[";
    first = true;
    for (const auto& relation : relations) {
        if (!first) {
            std::cout << ',';
        }
        first = false;
        hex(canonical(relation));
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
        malformed();
        attribute_validation();
        numeric_attributes();
        unsigned_attribute_rows();
        workspace_recovery();
        std::cout << "Validated 2001 ordinary/clock transactions, typed and optional attributes, "
                     "global embedded IDs, full-domain stage ends, open stages, locators, "
                     "relation-only pages, exact ownership/ID/endpoint queries, six page/codec "
                     "variants and all small-block truncations\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
