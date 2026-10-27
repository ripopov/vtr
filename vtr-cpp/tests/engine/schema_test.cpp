#include "engine/schema.h"
#include "format.h"
#include <iostream>
#include <random>
#include <source_location>

using namespace vtr::detail::engine;
namespace d = vtr::detail::wire::detail;

enum class Operation : std::int8_t { load = -7, store = 9 };

struct Operand {
    std::uint32_t index;
    std::int16_t offset;
    float weight;
};

struct Request {
    std::uint64_t address;
    Operation operation;
    std::array<Operand, 3> operands;
    std::vector<Operand> extra;
    std::string description;
    Bytes data;
    double estimate;
    std::vector<bool> flags;
};

struct Recursive {
    std::vector<Recursive> children;
};

template <> struct vtr::detail::engine::Enum<Operation> {
    static constexpr std::string_view name = "Operation";

    static auto values() {
        return std::array{std::pair{Operation::load, "load"}, std::pair{Operation::store, "store"}};
    }
};

template <> struct vtr::detail::engine::Record<Operand> {
    static constexpr std::string_view name = "Operand";

    static auto fields() {
        return std::tuple{field("index", &Operand::index), field("offset", &Operand::offset),
                          field("weight", &Operand::weight)};
    }
};

template <> struct vtr::detail::engine::Record<Request> {
    static constexpr std::string_view name = "Request";

    static auto fields() {
        return std::tuple{
            field("address", &Request::address),         field("operation", &Request::operation),
            field("operands", &Request::operands),       field("extra", &Request::extra),
            field("description", &Request::description), field("data", &Request::data),
            field("estimate", &Request::estimate),       field("flags", &Request::flags)};
    }
};

template <> struct vtr::detail::engine::Record<Recursive> {
    static constexpr std::string_view name = "Recursive";

    static auto fields() {
        return std::tuple{field("children", &Recursive::children)};
    }
};

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
    throw std::runtime_error("malformed input accepted at line " + std::to_string(location.line()));
}

std::array<View, 5> views(const ArgumentColumns& columns) {
    std::array<View, 5> result;
    for (unsigned i = 0; i < 5; ++i) {
        result[i] = columns.columns[i];
    }
    return result;
}

Bytes roundtrip(const Types& types, TypeId id, View row, std::uint64_t string_count = UINT64_MAX) {
    types.validate(id, row, string_count);
    ArgumentColumns columns;
    columns.string_count = string_count;
    columns.append(types, id, row);
    columns.finish();
    ArgumentReader reader(views(columns), string_count);
    auto decoded = reader.read(types, id);
    reader.done();
    types.validate(id, decoded, string_count);
    check(std::equal(row.begin(), row.end(), decoded.begin(), decoded.end()),
          "typed payload changed");
    return decoded;
}

void aggregates() {
    Types types;
    auto id = types.bind<Request>();
    auto count = types.size();
    check(id == types.bind<Request>() && count == types.size(), "shared datatype was redeclared");
    auto wire = types.declarations();
    Types decoder;
    decoder.read_declarations(wire);
    check(decoder.declarations() == wire, "datatype definitions changed");
    auto last = types.size();
    auto late = types.structure("Late", {{"request", id}, {"name", 13}});
    decoder.read_declarations(types.declarations(last));
    check(decoder.declarations() == types.declarations() && late == last,
          "late declaration failed");
    std::mt19937_64 random(0x63ac529e);
    std::vector<Bytes> rows;
    ArgumentColumns columns;
    for (unsigned i = 0; i < 1500; ++i) {
        Request request{};
        request.address = random();
        request.operation = static_cast<Operation>(i % 3 == 0 ? -7 : i % 3 == 1 ? 9 : 22);
        for (auto& operand : request.operands) {
            operand = {std::uint32_t(random()), std::int16_t(random()),
                       std::bit_cast<float>(std::uint32_t(random()))};
        }
        request.extra.assign(i % 11, request.operands[i % 3]);
        request.description = i % 4 ? "bank conflict Δ" : "";
        request.data.assign(i % 17, std::uint8_t(i));
        request.estimate = std::bit_cast<double>(random());
        request.flags.assign(i % 13, i % 2);
        auto row = capture(request);
        types.validate(id, row);
        auto decoded = decode<Request>(row);
        check(capture(decoded) == row, "C++20 typed decode changed payload bits");
        columns.append(types, id, row);
        rows.push_back(std::move(row));
    }
    columns.finish();
    ArgumentReader reader(views(columns));
    for (const auto& row : rows) {
        check(reader.read(decoder, id) == row, "aggregate round trip changed bits");
    }
    reader.done();
    check(columns.dictionary.size() == 2, "text dictionary failed to deduplicate");
    for (std::size_t n = 0; n < rows[0].size(); ++n) {
        rejects([&] { types.validate(id, View(rows[0]).first(n)); });
    }
    for (std::size_t n = 0; n < rows[0].size(); ++n) {
        rejects([&] { decode<Request>(View(rows[0]).first(n)); });
    }
    rejects([&] { decode<Request>(rows[1], 1); });
    auto trailing = rows[0];
    trailing.push_back(0);
    rejects([&] { types.validate(id, trailing); });
    auto scalar = capture(std::uint64_t(9));
    rejects([&] { roundtrip(types, id, scalar); });
    for (unsigned i = 0; i < 5; ++i) {
        if (columns.columns[i].empty()) {
            continue;
        }
        auto truncated = views(columns);
        truncated[i] = truncated[i].first(truncated[i].size() - 1);
        rejects([&] {
            ArgumentReader bad(truncated);
            for (unsigned j = 0; j < rows.size(); ++j) {
                bad.read(types, id);
            }
            bad.done();
        });
    }
}

void primitives() {
    Types types;
    for (auto value : {INT64_MIN, std::int64_t(-1), std::int64_t(0), INT64_MAX}) {
        roundtrip(types, 5, capture(value));
    }
    for (auto value : {std::uint64_t(0), std::uint64_t(127), std::uint64_t(128), UINT64_MAX}) {
        roundtrip(types, 9, capture(value));
        roundtrip(types, 18, capture(value));
        roundtrip(types, 20, capture(value));
    }
    for (auto bits :
         {std::uint32_t(0), std::uint32_t(0x80000000), std::uint32_t(0x7fc0deed), UINT32_MAX}) {
        roundtrip(types, 10, capture(std::bit_cast<float>(bits)));
    }
    for (auto bits : {std::uint64_t(0), std::uint64_t(0x8000000000000000),
                      std::uint64_t(0x7ff800000000beef), UINT64_MAX}) {
        roundtrip(types, 11, capture(std::bit_cast<double>(bits)));
    }
    roundtrip(types, 0, {});
    roundtrip(types, 1, capture(true));
    roundtrip(types, 1, capture(false));
    roundtrip(types, 2, capture(std::int8_t(INT8_MIN)));
    roundtrip(types, 3, capture(std::int16_t(INT16_MIN)));
    roundtrip(types, 4, capture(std::int32_t(INT32_MIN)));
    roundtrip(types, 6, capture(std::uint8_t(UINT8_MAX)));
    roundtrip(types, 7, capture(std::uint16_t(UINT16_MAX)));
    roundtrip(types, 8, capture(std::uint32_t(UINT32_MAX)));
    roundtrip(types, 12, capture(std::uint32_t(11)), 12);
    rejects([&] { types.validate(12, capture(std::uint32_t(12)), 12); });
    rejects([&] { types.validate(1, Bytes{2}); });
    rejects([&] { types.validate(13, capture(std::string("\xc0\x80"))); });
    for (unsigned p : {21u, 22u}) {
        Payload row;
        row.scalar(UINT64_MAX);
        row.scalar(INT64_MIN);
        roundtrip(types, p, row.view());
    }
    for (unsigned p : {15u, 16u, 17u}) {
        Payload row;
        row.scalar(std::uint64_t(3));
        if (p == 15) {
            row.raw(Bytes{7});
        }
        if (p == 16) {
            row.raw(Bytes{0x39});
        }
        if (p == 17) {
            row.raw(Bytes{0x81, 0x03});
        }
        roundtrip(types, p, row.view());
        auto bad = Bytes(row.view().begin(), row.view().end());
        bad.back() |= 0x80;
        rejects([&] { types.validate(p, bad); });
    }
    Payload invalid;
    invalid.scalar(std::uint64_t(2));
    invalid.raw(Bytes{0x91});
    rejects([&] { types.validate(17, invalid.view()); });
    ArgumentColumns invalid_integer;
    d::put(invalid_integer.columns[0], 256);
    invalid_integer.finish();
    rejects([&] {
        ArgumentReader reader(views(invalid_integer));
        reader.read(types, 6);
    });
    invalid_integer.columns[0] = Bytes{0x80, 2};
    rejects([&] {
        ArgumentReader reader(views(invalid_integer));
        reader.read(types, 2);
    });
    invalid_integer.columns[0] = Bytes{0x80, 0};
    rejects([&] {
        ArgumentReader reader(views(invalid_integer));
        reader.read(types, 9);
    });
}

void declarations_and_limits() {
    Types types;
    rejects([&] { types.structure("bad", {{"x", 8}, {"x", 9}}); });
    rejects([&] { types.array(999, 2); });
    rejects([&] { types.enumeration("bad", 10, {}); });
    rejects([&] { types.enumeration("bad", 2, {{128, "overflow"}}); });
    rejects([&] { types.enumeration("bad", 6, {{UINT64_MAX, "negative"}}); });
    rejects([&] { types.enumeration("bad", 2, {{0, "a"}, {1, "a"}}); });
    rejects([&] { types.structure(std::string("\xff"), {}); });
    rejects([&] { types.array(9, UINT32_MAX); });
    rejects([&] { types.bind<Recursive>(); });
    rejects([&] { types.bind<Recursive>(); });
    auto unit = types.structure("Empty", {});
    auto zero_array = types.array(unit, UINT32_MAX);
    roundtrip(types, zero_array, {});
    auto sequence = types.sequence(zero_array);
    auto row = capture(std::uint32_t(UINT32_MAX));
    roundtrip(types, sequence, row);
    auto current = unit;
    while (types[current].depth < 64) {
        current = types.array(current, 1);
    }
    rejects([&] { types.sequence(current); });
    auto wire = types.declarations();
    for (std::size_t n = 0; n < wire.size(); ++n) {
        Types decoder;
        rejects([&] { decoder.read_declarations(View(wire).first(n)); });
        check(decoder.size() == first_declared_type, "failed datatype chunk was partly applied");
    }
    Types decoder;
    decoder.read_declarations(wire);
    rejects([&] { decoder.read_declarations(wire); });
    rejects([&] { types.validate(19, {}); });
    rejects([&] { types.validate(9999, {}); });
}

void bounded_programs() {
    Types types;
    auto enum_type = types.enumeration("State", 2, {{UINT64_MAX, "minus_one"}});
    auto fields = types.structure("Scalars", {{"integer", 9}, {"float", 10}, {"enum", enum_type}});
    auto short_array = types.array(fields, 16), long_array = types.array(fields, 300);
    auto dynamic_array = types.sequence(fields);
    for (auto type : {short_array, long_array, dynamic_array}) {
        Payload row;
        if (type == dynamic_array) {
            row.length(321);
        }
        auto count = type == short_array ? 16 : type == long_array ? 300 : 321;
        for (int i = 0; i < count; ++i) {
            row.scalar(std::uint64_t(UINT64_MAX - i));
            row.scalar(std::bit_cast<float>(std::uint32_t(0x7fc00001 + i)));
            row.scalar(std::int8_t(i % 128));
        }
        auto encoded = Bytes(row.view().begin(), row.view().end());
        check(roundtrip(types, type, encoded) == encoded, "bounded scalar program changed payload");
        rejects([&] { types.validate(type, View(encoded).first(encoded.size() - 1)); });
        encoded.push_back(0);
        rejects([&] { types.validate(type, encoded); });
    }
    auto checked = types.structure("Checked", {{"bool", 1}, {"string", 12}, {"text", 13}});
    auto array = types.array(checked, 2);
    Payload row;
    for (unsigned i = 0; i < 2; ++i) {
        row.scalar(true);
        row.scalar(std::uint32_t(i));
        row.append(std::string("label"));
    }
    auto data = Bytes(row.view().begin(), row.view().end());
    roundtrip(types, array, data, 2);
    auto boolean = data;
    boolean[0] = 2;
    rejects([&] { types.validate(array, boolean, 2); });
    auto string = data;
    string[1] = 2;
    rejects([&] { types.validate(array, string, 2); });
    auto text = data;
    text[9] = 0xff;
    rejects([&] { types.validate(array, text, 2); });
}

void decoded_byte_limits() {
    Types types;
    auto numeric = types.sequence(9), text = types.sequence(13);
    auto check_limit = [&](TypeId type, const Bytes& payload, std::size_t small) {
        ArgumentColumns columns;
        columns.append(types, type, payload);
        columns.finish();
        rejects([&] {
            ArgumentReader reader(views(columns), UINT64_MAX, small);
            reader.read(types, type);
        });
        ArgumentReader exact(views(columns), UINT64_MAX, payload.size());
        check(exact.read(types, type) == payload, "exact decoded payload limit changed values");
        exact.done();
    };
    check_limit(numeric, capture(std::vector<std::uint64_t>(512, 0)), 4096);
    check_limit(text, capture(std::vector<std::string>(64, std::string(1024, 'x'))), 1024);
    auto empty = types.structure("EmptyLimit", {});
    auto many_empty = types.sequence(empty);
    auto payload = capture(std::uint32_t(UINT32_MAX));
    ArgumentColumns columns;
    columns.append(types, many_empty, payload);
    columns.finish();
    ArgumentReader unit_sequence(views(columns), UINT64_MAX, 4);
    check(unit_sequence.read(types, many_empty) == payload,
          "zero-size sequence elements consumed a decoded byte budget");
    unit_sequence.done();
}

void hybrid_text_validation() {
    Types types;
    const std::vector<std::string> global{"", std::string(1024, 'x')};
    TextReferences references;
    references.emplace(global[1], 1);
    ArgumentColumns columns;
    columns.shared_text = &references;
    const std::vector<std::string> rows{global[1], "short", global[1], ""};
    for (const auto& value : rows)
        columns.append(types, 13, capture(value));
    columns.finish();
    ArgumentReader reader(views(columns), global.size(), default_payload_limit, global);
    for (const auto& value : rows)
        check(reader.read(types, 13) == capture(value), "hybrid text changed canonical bytes");
    reader.done();
    auto invalid = [&](ArgumentColumns bad, std::span<const std::string> strings,
                       std::size_t limit = default_payload_limit) {
        rejects([&] {
            ArgumentReader decoded(views(bad), global.size(), limit, strings);
            for (unsigned i = 0; i < rows.size(); ++i)
                decoded.read(types, 13);
            decoded.done();
        });
    };
    invalid(columns, {});
    invalid(columns, global, 100);
    const std::vector<std::string> corrupt{"", std::string(1024, char(0xff))};
    invalid(columns, corrupt);
    rejects([&] {
        ArgumentReader limited(views(columns), 1, default_payload_limit, global);
        limited.read(types, 13);
    });
    auto bad = columns;
    bad.columns[2][0] = 5;
    invalid(bad, global);
    bad = columns;
    bad.columns[2][0] = 2;
    invalid(bad, global);
    bad = columns;
    bad.columns[2][1] = 4;
    invalid(bad, global);
    bad = columns;
    bad.columns[3][0] = 0;
    invalid(bad, global);
}

void dictionary_validation() {
    Types types;
    ArgumentColumns columns;
    columns.append(types, 13, capture(std::string("a")));
    columns.append(types, 13, capture(std::string("b")));
    columns.finish();
    auto skipped = columns;
    skipped.columns[2][0] = 1;
    rejects([&] {
        ArgumentReader reader(views(skipped));
        reader.read(types, 13);
    });
    auto duplicate = columns;
    duplicate.columns[3].back() = 'a';
    rejects([&] { ArgumentReader reader(views(duplicate)); });
    rejects([&] {
        ArgumentReader reader(views(columns));
        reader.read(types, 13);
        reader.done();
    });
    auto unknown = columns;
    unknown.columns[2][0] = 2;
    rejects([&] {
        ArgumentReader reader(views(unknown));
        reader.read(types, 13);
    });
}

void text_copy_isolation() {
    Types types;
    const auto first = capture(std::string("shared short text"));
    const auto original_tail = capture(std::string(3000, 'a'));
    const auto copied_tail = capture(std::string(3000, 'b'));
    const auto after_destroy = capture(std::string(5000, 'c'));
    auto verify = [&](ArgumentColumns columns, std::span<const Bytes> expected) {
        columns.finish();
        ArgumentReader reader(views(columns));
        for (const auto& row : expected)
            check(reader.read(types, 13) == row, "copied dictionary text was overwritten");
        reader.done();
    };
    auto detached = [&] {
        ArgumentColumns original;
        original.append(types, 13, first);
        auto copied = original;
        original.append(types, 13, original_tail);
        copied.append(types, 13, copied_tail);
        verify(original, std::array{first, original_tail});
        verify(copied, std::array{first, copied_tail});
        return copied;
    }();
    auto moved = std::move(detached);
    moved.append(types, 13, after_destroy);
    moved.append(types, 13, first);
    verify(moved, std::array{first, copied_tail, after_destroy, first});
}

void text_lookup_and_utf8() {
    Types types;
    ArgumentColumns columns;
    std::vector<Bytes> rows;
    for (unsigned i = 0; i < 512; ++i) {
        auto text = std::to_string(i) + std::string(i % 3 ? 4 : 4096, 'a');
        text.push_back('\0');
        text += "UTF-8 Δ";
        rows.push_back(capture(text));
        columns.append(types, 13, rows.back());
        std::fill(text.begin(), text.end(), 'x');
    }
    auto copied = columns;
    auto moved = std::move(copied);
    for (auto i = rows.size(); i-- > 0;)
        moved.append(types, 13, rows[i]);
    check(moved.dictionary.size() == rows.size(), "text lookup lost ownership or deduplication");
    moved.finish();
    ArgumentReader reader(views(moved));
    for (const auto& row : rows)
        check(reader.read(types, 13) == row, "text dictionary changed first-use content");
    for (auto i = rows.size(); i-- > 0;)
        check(reader.read(types, 13) == rows[i], "text dictionary changed repeated content");
    reader.done();

    const std::array valid{std::string("\xc2\x80"),         std::string("\xe0\xa0\x80"),
                           std::string("\xed\x9f\xbf"),     std::string("\xf0\x90\x80\x80"),
                           std::string("\xf4\x8f\xbf\xbf"), std::string(1, '\0')};
    const std::array invalid{std::string("\x80"),
                             std::string("\xff"),
                             std::string("\xc0\x80"),
                             std::string("\xe0\x80\x80"),
                             std::string("\xed\xa0\x80"),
                             std::string("\xf0\x80\x80\x80"),
                             std::string("\xf4\x90\x80\x80"),
                             std::string("\xe2\x28\xa1"),
                             std::string("\xc2"),
                             std::string("\xe2\x82"),
                             std::string("\xf0\x90\x80")};
    for (unsigned prefix = 0; prefix < 24; ++prefix) {
        for (const auto& text : valid) {
            const auto row = capture(std::string(prefix, 'a') + text + std::string(32, 'b'));
            types.validate(13, row);
            columns.append(types, 13, row);
        }
        for (const auto& text : invalid) {
            for (unsigned suffix : {0u, 32u}) {
                const auto row =
                    capture(std::string(prefix, 'a') + text + std::string(suffix, 'b'));
                rejects([&] { types.validate(13, row); });
                const auto entries = columns.dictionary.size();
                rejects([&] { columns.append(types, 13, row); });
                check(columns.dictionary.size() == entries,
                      "invalid UTF-8 entered the argument dictionary");
            }
        }
    }
}

void fixture(bool shared = false) {
    Types types;
    auto request_type = types.bind<Request>();
    std::vector<std::pair<TypeId, Bytes>> rows;
    for (unsigned i = 0; i < 3; ++i) {
        Request request{};
        request.address = UINT64_MAX - i;
        request.operation = static_cast<Operation>(i == 0 ? -7 : i == 1 ? 9 : 22);
        request.operands = {
            {{0, INT16_MIN, std::bit_cast<float>(std::uint32_t(0x80000000))},
             {UINT32_MAX, INT16_MAX, std::bit_cast<float>(std::uint32_t(0x7fc0beef))},
             {1234, -42, 1.5f}}};
        request.extra.assign(i, request.operands[2]);
        request.description = i < 2 ? "retry Δ" : "";
        request.data = Bytes{0, 255, static_cast<std::uint8_t>(i)};
        request.estimate = std::bit_cast<double>(std::uint64_t(0x7ff800000000babe));
        request.flags.assign(i + 1, i % 2);
        rows.emplace_back(request_type, capture(request));
    }
    for (TypeId id = 0; id < first_declared_type; ++id) {
        if (id == 19) {
            continue;
        }
        Payload payload;
        if (id == 1) {
            payload.scalar(true);
        } else if (id >= 2 && id <= 9) {
            auto width = std::array<unsigned, 8>{1, 2, 4, 8, 1, 2, 4, 8}[id - 2];
            for (unsigned i = 0; i < width; ++i) {
                payload.scalar(std::uint8_t(255));
            }
        } else if (id == 10) {
            payload.scalar(std::bit_cast<float>(std::uint32_t(0x7fc0ffff)));
        } else if (id == 11) {
            payload.scalar(std::bit_cast<double>(std::uint64_t(0xfff0000000000000)));
        } else if (id == 12) {
            payload.scalar(std::uint32_t(2));
        } else if (id == 13) {
            payload.append(std::string("retry Δ"));
        } else if (id == 14) {
            payload.append(Bytes{0, 255, 128});
        } else if (id >= 15 && id <= 17) {
            payload.scalar(std::uint64_t(3));
            if (id == 15) {
                payload.raw(Bytes{5});
            }
            if (id == 16) {
                payload.raw(Bytes{0x39});
            }
            if (id == 17) {
                payload.raw(Bytes{0x81, 0x03});
            }
        } else if (id >= 18 && id <= 20) {
            payload.scalar(UINT64_MAX);
        } else if (id >= 21) {
            payload.scalar(UINT64_MAX);
            payload.scalar(INT64_MIN);
        }
        rows.emplace_back(id, std::move(payload).take());
    }
    const std::vector<std::string> shared_strings{"", "retry Δ"};
    TextReferences references;
    references.emplace("", 0);
    references.emplace("retry Δ", 1);
    ArgumentColumns columns;
    columns.shared_text = shared ? &references : nullptr;
    for (const auto& [id, row] : rows) {
        types.validate(id, row);
        columns.append(types, id, row);
    }
    columns.finish();
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
    std::cout << ",\"columns\":[";
    for (unsigned i = 0; i < 5; ++i) {
        if (i) {
            std::cout << ',';
        }
        hex(columns.columns[i]);
    }
    std::cout << "],\"global_text\":[";
    if (shared)
        std::cout << "\"\",\"retry Δ\"";
    std::cout << "],\"rows\":[";
    bool first = true;
    for (const auto& [id, row] : rows) {
        if (!first) {
            std::cout << ',';
        }
        first = false;
        std::cout << "[" << id << ',';
        hex(row);
        std::cout << ']';
    }
    std::cout << "]}\n";
}

void datatype_metadata() {
    using vtr::detail::wire::Attributes;
    using vtr::detail::wire::Value;
    Types types;
    const auto arguments = types.structure("Arguments", {{"request", 9}});
    auto metadata = [](std::string_view text) {
        const std::array nested{Value(std::int64_t{-7}), Value::bytes(Bytes{0, 255})};
        return Attributes{{0, Value::string(1)}, {2, Value::text(text)}, {3, Value::list(nested)}};
    };
    const auto first = types.structure("Message", {{"args", arguments}}, metadata("started {}"));
    const auto second = types.structure("Message", {{"args", arguments}}, metadata("failed {}"));
    check(first != second && types[first].fixed_size == 8 && types[second].fixed_size == 8,
          "metadata changed type identity or canonical layout");
    const auto bytes = capture(std::uint64_t{42});
    for (auto id : {arguments, first, second})
        types.validate(id, bytes, 4);
    const Attributes generic{{0, Value(true)}};
    types.array(arguments, 2, "custom", generic);
    types.sequence(arguments, "custom", generic);
    types.enumeration("custom", 4, {}, generic);
    const auto declarations = types.declarations();
    Types decoded;
    decoded.read_declarations(declarations, 4);
    check(decoded.declarations() == declarations, "metadata declaration roundtrip changed bytes");
    Types invalid;
    rejects([&] { invalid.read_declarations(declarations, 1); });
    check(invalid.size() == first_declared_type, "invalid metadata published a partial chunk");
    for (std::size_t end = 0; end < declarations.size(); ++end) {
        rejects([&] { invalid.read_declarations(View(declarations).first(end), 4); });
        check(invalid.size() == first_declared_type, "truncated metadata changed declarations");
    }
    rejects([&] { types.structure("bad", {}, {{0, Value(true)}, {0, Value(false)}}); });
    Types plain, explicit_empty;
    plain.structure("A", {{"x", 9}});
    explicit_empty.structure("A", {{"x", 9}}, {});
    check(plain.declarations() == explicit_empty.declarations(),
          "empty metadata changes fixed-source declaration bytes");
    auto flagged_empty = plain.declarations();
    flagged_empty[2] |= 0x80;
    flagged_empty.push_back(0);
    rejects([&] { Types{}.read_declarations(flagged_empty); });
}

int main(int argc, char** argv) {
    if (argc == 2 && (std::string_view(argv[1]) == "--fixture" ||
                      std::string_view(argv[1]) == "--shared-fixture")) {
        fixture(std::string_view(argv[1]) == "--shared-fixture");
        return 0;
    }

    try {
        datatype_metadata();
        aggregates();
        primitives();
        declarations_and_limits();
        dictionary_validation();
        text_copy_isolation();
        hybrid_text_validation();
        text_lookup_and_utf8();
        bounded_programs();
        decoded_byte_limits();
        std::cout << "Validated shared aggregate schemas, 1500 exact mixed payloads, all primitive "
                     "domains, truncation, malformed columns, dictionaries, recursive and "
                     "oversized types\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
