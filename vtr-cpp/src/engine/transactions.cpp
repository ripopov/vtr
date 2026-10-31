#include "engine/transactions.h"
#include "containers.h"
#include "format.h"
#include "scalar_page.h"
#include "transaction_memory.h"
#include <unordered_set>

namespace vtr::detail::engine {
namespace {
namespace d = vtr::detail::wire::detail;
using d::Cursor;
using d::require;

std::uint32_t u32(std::size_t n) {
    require(n <= UINT32_MAX, "transaction field exceeds u32");
    return static_cast<std::uint32_t>(n);
}

std::size_t var_size(std::uint64_t n) {
    return n ? (64 - std::countl_zero(n) + 6) / 7 : 1;
}

using detail::attrs_upper;
using detail::memory_add;
using detail::memory_mul;
using detail::transaction_upper;

std::size_t transaction_upper(const Relation& relation) {
    return memory_add(512, attrs_upper(relation.attributes));
}

void extend(RecordId& minimum, RecordId& maximum, RecordId value, bool first) {
    minimum = first ? value : std::min(minimum, value);
    maximum = first ? value : std::max(maximum, value);
}

void add_embedded(TransactionBounds& bounds, RecordId id) {
    extend(bounds.min_embedded, bounds.max_embedded, id, bounds.max_embedded == 0);
}

bool matches_owner(TransactionQuery query, const Transaction& tx) {
    return tx.begin <= query.end && tx.end >= query.begin &&
           (!query.generator || *query.generator == tx.generator) &&
           (!query.parent || *query.parent == tx.parent) && (!query.id || *query.id == tx.id);
}

bool matches_query(TransactionQuery query, const Transaction& tx) {
    return matches_owner(query, tx) &&
           (!query.embedded_id ||
            std::any_of(tx.events.begin(), tx.events.end(),
                        [&](const auto& event) { return event.id == *query.embedded_id; }) ||
            std::any_of(tx.stages.begin(), tx.stages.end(),
                        [&](const auto& stage) { return stage.id == *query.embedded_id; }));
}

void add(TransactionBounds& b, const Transaction& tx) {
    bool first = b.transactions++ == 0;
    extend(b.min_id, b.max_id, tx.id, first);
    if (first) {
        b.begin = tx.begin;
        b.end = tx.end;
    } else {
        b.begin = std::min(b.begin, tx.begin);
        b.end = std::max(b.end, tx.end);
    }
    if (tx.parent) {
        extend(b.min_parent, b.max_parent, tx.parent, b.max_parent == 0);
    }
    for (const auto& event : tx.events) {
        extend(b.min_embedded, b.max_embedded, event.id, b.max_embedded == 0);
    }
    for (const auto& stage : tx.stages) {
        extend(b.min_embedded, b.max_embedded, stage.id, b.max_embedded == 0);
    }
}

void add(TransactionBounds& b, const Relation& r) {
    bool first = b.relations++ == 0;
    extend(b.min_from, b.max_from, r.source, first);
    extend(b.min_to, b.max_to, r.destination, first);
}

void merge(TransactionBounds& result, const TransactionBounds& item) {
    if (item.transactions) {
        bool first = result.transactions == 0;
        extend(result.min_id, result.max_id, item.min_id, first);
        result.max_id = std::max(result.max_id, item.max_id);
        if (first) {
            result.begin = item.begin;
            result.end = item.end;
        } else {
            result.begin = std::min(result.begin, item.begin);
            result.end = std::max(result.end, item.end);
        }
        if (item.max_parent) {
            extend(result.min_parent, result.max_parent, item.min_parent, result.max_parent == 0);
            result.max_parent = std::max(result.max_parent, item.max_parent);
        }
        if (item.max_embedded) {
            extend(result.min_embedded, result.max_embedded, item.min_embedded,
                   result.max_embedded == 0);
            result.max_embedded = std::max(result.max_embedded, item.max_embedded);
        }
        require(item.transactions <= UINT64_MAX - result.transactions,
                "transaction count overflow");
        result.transactions += item.transactions;
    }
    if (item.relations) {
        bool first = result.relations == 0;
        extend(result.min_from, result.max_from, item.min_from, first);
        result.max_from = std::max(result.max_from, item.max_from);
        extend(result.min_to, result.max_to, item.min_to, first);
        result.max_to = std::max(result.max_to, item.max_to);
        require(item.relations <= UINT64_MAX - result.relations, "relation count overflow");
        result.relations += item.relations;
    }
}

void write_bounds(Bytes& out, const TransactionBounds& b) {
    for (auto n : {b.transactions, b.relations, b.min_id, b.max_id, b.begin, b.end, b.min_parent,
                   b.max_parent, b.min_embedded, b.max_embedded, b.min_from, b.max_from, b.min_to,
                   b.max_to}) {
        d::fixed(out, n);
    }
}

TransactionBounds read_bounds(Cursor& r) {
    TransactionBounds b;
    auto fields =
        std::array{&b.transactions, &b.relations,  &b.min_id,     &b.max_id,       &b.begin,
                   &b.end,          &b.min_parent, &b.max_parent, &b.min_embedded, &b.max_embedded,
                   &b.min_from,     &b.max_from,   &b.min_to,     &b.max_to};
    for (auto field : fields) {
        *field = r.fixed<std::uint64_t>();
    }
    require((b.transactions || b.relations) && b.min_id <= b.max_id && b.begin <= b.end &&
                b.min_parent <= b.max_parent && b.min_embedded <= b.max_embedded &&
                b.min_from <= b.max_from && b.min_to <= b.max_to,
            "invalid transaction bounds");
    require(b.transactions ? b.min_id != 0
                           : !(b.min_id || b.max_id || b.begin || b.end || b.min_parent ||
                               b.max_parent || b.min_embedded || b.max_embedded),
            "transaction bounds/count disagree");
    require(b.relations || !(b.min_from || b.max_from || b.min_to || b.max_to),
            "relation bounds/count disagree");
    require((b.min_parent != 0) == (b.max_parent != 0) &&
                (b.min_embedded != 0) == (b.max_embedded != 0),
            "invalid absent identifier bounds");
    return b;
}

bool equal(const TransactionBounds& a, const TransactionBounds& b) {
    return a == b;
}

void valid_attrs(std::span<const vtr::detail::wire::Attribute> attrs, std::uint64_t strings) {
    d::validate_attrs(attrs);
    for (const auto& attr : attrs) {
        require(attr.key < strings, "unknown transaction attribute key");
        d::validate_value(attr.value, strings);
    }
}

void valid_shape(const Transaction& tx, const Generator& generator, std::uint64_t strings) {
    require(tx.id && tx.parent < tx.id && tx.end >= tx.begin && tx.status <= 4 && tx.span_kind <= 5,
            "invalid transaction identity/time/classification");
    require(generator.kind == RecordKind::ordinary || generator.kind == RecordKind::clock,
            "invalid transaction generator kind");
    for (const auto& event : tx.events) {
        require(event.id > tx.id && event.name < strings, "invalid embedded event identity/name");
    }
    for (const auto& stage : tx.stages) {
        require(stage.id > tx.id && stage.name < strings && stage.lane < strings &&
                    (!stage.end || *stage.end >= stage.begin),
                "invalid embedded stage");
    }
    std::uint64_t previous = 0;
    for (auto block : tx.child_blocks) {
        require(block > previous, "invalid located block locator list");
        previous = block;
    }
    if (generator.kind == RecordKind::clock) {
        require(generator.payload_type == 0 && !tx.parent && !tx.span_kind &&
                    (tx.status == 0 || tx.status == 4) && tx.events.empty() && tx.stages.empty() &&
                    tx.child_blocks.empty(),
                "invalid clock transaction shape");
    }
}

void valid(const Transaction& tx, const Generator& generator, std::uint64_t strings) {
    valid_shape(tx, generator, strings);
    valid_attrs(tx.attributes, strings);
    for (const auto& event : tx.events)
        valid_attrs(event.attributes, strings);
    for (const auto& stage : tx.stages)
        valid_attrs(stage.attributes, strings);
}

void numeric_attrs(d::AttributeColumns& columns, std::span<const NumericAttribute> attrs,
                   std::uint64_t strings) {
    d::put(columns[0], attrs.size());
    if (attrs.size() <= 8 && std::all_of(attrs.begin(), attrs.end(), [](const auto& attr) {
            return attr.tag == wire::Tag::u64;
        })) {
        std::array<std::uint8_t, 8 * 5> keys;
        std::array<std::uint8_t, 8 * 10> values;
        auto* key = keys.data();
        auto* value = values.data();
        auto put = [](auto*& out, std::uint64_t number) {
            while (number >= 128) {
                *out++ = static_cast<std::uint8_t>(number) | 128;
                number >>= 7;
            }
            *out++ = static_cast<std::uint8_t>(number);
        };
        wire::Id maximum_key = 0;
        for (std::size_t i = 0; i < attrs.size(); ++i) {
            const auto& attr = attrs[i];
            require(attr.key < strings, "invalid numeric attribute key/tag");
            if (i && attr.key <= maximum_key)
                for (std::size_t j = 0; j < i; ++j)
                    require(attrs[j].key != attr.key, "duplicate numeric attribute key",
                            ErrorCode::invalid_argument);
            maximum_key = std::max(maximum_key, attr.key);
            put(key, attr.key);
            put(value, attr.number);
        }
        d::append(columns[1], {keys.data(), static_cast<std::size_t>(key - keys.data())});
        columns[2].insert(columns[2].end(), attrs.size(),
                          static_cast<std::uint8_t>(wire::Tag::u64));
        d::append(columns[3], {values.data(), static_cast<std::size_t>(value - values.data())});
        return;
    }
    wire::Id maximum_key = 0;
    for (std::size_t i = 0; i < attrs.size(); ++i) {
        const auto& attr = attrs[i];
        require(attr.key < strings && numeric_attribute(attr.tag),
                "invalid numeric attribute key/tag");
        if (i && attr.key <= maximum_key)
            for (std::size_t j = 0; j < i; ++j)
                require(attrs[j].key != attr.key, "duplicate numeric attribute key",
                        ErrorCode::invalid_argument);
        maximum_key = std::max(maximum_key, attr.key);
        d::put(columns[1], attr.key);
        columns[2].push_back(static_cast<std::uint8_t>(attr.tag));
        switch (attr.tag) {
        case wire::Tag::null:
            break;
        case wire::Tag::i64:
            d::sint(columns[3], attr.number);
            break;
        case wire::Tag::u64:
        case wire::Tag::time:
        case wire::Tag::pointer:
            d::put(columns[3], attr.number);
            break;
        case wire::Tag::f32:
            require(attr.number <= UINT32_MAX, "f32 bits out of range");
            d::fixed<std::uint32_t>(columns[4], attr.number);
            break;
        case wire::Tag::f64:
            d::fixed(columns[4], attr.number);
            break;
        default: {
            wire::Value value;
            value.tag = attr.tag;
            value.number = attr.number;
            d::encode_payload(columns[3], value);
            break;
        }
        }
    }
}

void stage_end(Bytes& out, const EmbeddedStage& stage) {
    if (!stage.end) {
        d::put(out, 0);
        return;
    }
    if (stage.begin == 0 && *stage.end == UINT64_MAX) {
        out.insert(out.end(), 9, 0x80);
        out.push_back(2);
    } else {
        d::put(out, *stage.end - stage.begin + 1);
    }
}

std::optional<Timestamp> read_stage_end(Cursor& r, Timestamp begin) {
    if (r.data.size() >= 10 &&
        std::all_of(r.data.begin(), r.data.begin() + 9, [](auto b) { return b == 0x80; }) &&
        r.data[9] == 2) {
        r.take(10);
        require(begin == 0, "full-domain stage end overflows");
        return UINT64_MAX;
    }
    auto value = r.var();
    if (!value) {
        return {};
    }
    require(value - 1 <= UINT64_MAX - begin, "embedded stage end overflows");
    return begin + value - 1;
}

constexpr std::array<unsigned, 7> time_columns{2, 3, 13, 14, 16, 19, 20};

struct TimeColumns {
    std::array<Cursor, 7> columns;
    Timestamp previous_begin = 0;

    explicit TimeColumns(const std::array<Cursor, 33>& source) {
        for (unsigned i = 0; i < columns.size(); ++i) {
            columns[i] = source[time_columns[i]];
        }
    }

    std::pair<Timestamp, Timestamp> owner() {
        auto begin = previous_begin + columns[0].signed_bits();
        auto duration = columns[1].var();
        require(duration <= UINT64_MAX - begin, "transaction end overflow");
        previous_begin = begin;
        return {begin, begin + duration};
    }

    std::uint64_t event_count() {
        auto count = columns[2].var();
        require(count <= columns[3].data.size(), "invalid embedded event count");
        return count;
    }

    Timestamp event(Timestamp owner_begin) {
        return owner_begin + columns[3].signed_bits();
    }

    std::uint64_t stage_count() {
        auto count = columns[4].var();
        require(count <= std::min(columns[5].data.size(), columns[6].data.size()),
                "invalid embedded stage count");
        return count;
    }

    std::pair<Timestamp, std::optional<Timestamp>> stage(Timestamp owner_begin) {
        auto begin = owner_begin + columns[5].signed_bits();
        return {begin, read_stage_end(columns[6], begin)};
    }

    void done() const {
        for (const auto& column : columns) {
            column.done();
        }
    }
};

std::pair<Timestamp, Timestamp> time_range_of(View raw, const TransactionBounds& expected) {
    auto columns = d::unpack_columns<33>(raw);
    require(expected.transactions <= columns[2].data.size(), "invalid transaction page count");
    TimeColumns times(columns);
    std::pair<Timestamp, Timestamp> owners{UINT64_MAX, 0}, range{expected.begin, expected.end};
    bool embedded = false;
    for (std::uint64_t i = 0; i < expected.transactions; ++i) {
        auto [begin, end] = times.owner();
        owners = {std::min(owners.first, begin), std::max(owners.second, end)};
        auto count = times.event_count();
        embedded |= count != 0;
        for (std::uint64_t j = 0; j < count; ++j) {
            auto time = times.event(begin);
            range = {std::min(range.first, time), std::max(range.second, time)};
        }
        count = times.stage_count();
        embedded |= count != 0;
        for (std::uint64_t j = 0; j < count; ++j) {
            auto [start, finish] = times.stage(begin);
            range = {std::min(range.first, start),
                     std::max(range.second, std::max(finish.value_or(end), start))};
        }
    }
    times.done();
    require(owners == std::pair{expected.begin, expected.end} && embedded,
            "transaction time bounds disagree");
    return range;
}

struct Encoder {
    std::array<Bytes, 33> columns;
    d::AttributeColumns attributes;
    ArgumentColumns arguments;
    RecordId previous_id = 0, previous_event = 0, previous_stage = 0, previous_from = 0;
    Timestamp previous_begin = 0;
    std::size_t dictionary_bytes = 0;

    void append(const Types& types, const Generator& generator, std::uint32_t local_generator,
                const Transaction& tx, std::span<const NumericAttribute> numeric = {}) {
        valid_shape(tx, generator, arguments.string_count);
        d::sint(columns[0], tx.id - previous_id);
        d::put(columns[1], local_generator);
        d::sint(columns[2], tx.begin - previous_begin);
        d::put(columns[3], tx.end - tx.begin);
        columns[4].push_back((tx.span_kind << 4) | tx.status);
        d::put(columns[5], tx.parent ? tx.id - tx.parent : 0);
        require(numeric.empty() || tx.attributes.empty(), "mixed numeric and ordinary attributes");
        if (numeric.empty())
            d::checked_column_attrs(attributes, tx.attributes, arguments.string_count);
        else
            numeric_attrs(attributes, numeric, arguments.string_count);
        d::put(columns[13], tx.events.size());
        for (const auto& event : tx.events) {
            d::sint(columns[14], event.time - tx.begin);
            d::put(columns[15], event.name);
            d::sint(columns[31], event.id - previous_event);
            previous_event = event.id;
            d::checked_column_attrs(attributes, event.attributes, arguments.string_count);
        }
        d::put(columns[16], tx.stages.size());
        for (const auto& stage : tx.stages) {
            d::put(columns[17], stage.name);
            d::put(columns[18], stage.lane);
            d::sint(columns[19], stage.begin - tx.begin);
            stage_end(columns[20], stage);
            d::sint(columns[32], stage.id - previous_stage);
            previous_stage = stage.id;
            d::checked_column_attrs(attributes, stage.attributes, arguments.string_count);
        }
        auto old_dictionary = arguments.dictionary.size();
        if (generator.payload_type == 0)
            require(tx.payload.empty(), "trailing unit payload");
        else
            arguments.append(types, generator.payload_type, tx.payload);
        for (auto i = old_dictionary; i < arguments.dictionary.size(); ++i) {
            dictionary_bytes +=
                var_size(arguments.dictionary[i].size()) + arguments.dictionary[i].size();
        }
        d::put(columns[29], tx.child_blocks.size());
        std::uint64_t previous_block = 0;
        for (auto block : tx.child_blocks) {
            d::put(columns[30], block - previous_block);
            previous_block = block;
        }
        previous_id = tx.id;
        previous_begin = tx.begin;
    }

    void append(const Relation& relation) {
        require(relation.kind < arguments.string_count, "unknown relation kind");
        d::put(columns[21], relation.kind);
        d::sint(columns[22], relation.source - previous_from);
        d::sint(columns[23], relation.destination - relation.source);
        previous_from = relation.source;
        d::checked_column_attrs(attributes, relation.attributes, arguments.string_count);
    }

    std::size_t size() const {
        std::size_t total = 1;
        for (unsigned i = 0; i < 33; ++i) {
            auto size = columns[i].size();
            if (i >= 6 && i <= 12) {
                size = attributes[i - 6].size();
            }
            if (i >= 24 && i <= 28) {
                auto a = i - 24;
                size = a == 3 ? (var_size((std::uint64_t(arguments.dictionary.size()) << 1) |
                                          (arguments.shared_text ? 1 : 0)) +
                                 dictionary_bytes)
                              : arguments.columns[a].size();
            }
            total += var_size(size) + size;
        }
        return total;
    }

    struct Checkpoint {
        std::array<std::size_t, 33> columns;
        std::array<std::size_t, 7> attributes;
        std::array<std::size_t, 5> arguments;
        std::size_t dictionary_count, dictionary_bytes;
        RecordId id, event, stage, from;
        Timestamp begin;
    };

    Checkpoint checkpoint() const {
        Checkpoint p;
        for (unsigned i = 0; i < 33; ++i) {
            p.columns[i] = columns[i].size();
        }
        for (unsigned i = 0; i < 7; ++i) {
            p.attributes[i] = attributes[i].size();
        }
        for (unsigned i = 0; i < 5; ++i) {
            p.arguments[i] = arguments.columns[i].size();
        }
        p.dictionary_count = arguments.dictionary.size();
        p.dictionary_bytes = dictionary_bytes;
        p.id = previous_id;
        p.event = previous_event;
        p.stage = previous_stage;
        p.from = previous_from;
        p.begin = previous_begin;
        return p;
    }

    void restore(const Checkpoint& p) {
        for (unsigned i = 0; i < 33; ++i) {
            columns[i].resize(p.columns[i]);
        }
        for (unsigned i = 0; i < 7; ++i) {
            attributes[i].resize(p.attributes[i]);
        }
        for (unsigned i = 0; i < 5; ++i) {
            arguments.columns[i].resize(p.arguments[i]);
        }
        while (arguments.dictionary.size() > p.dictionary_count) {
            arguments.text_ids.erase(arguments.dictionary.back());
            arguments.dictionary.pop_back();
        }
        dictionary_bytes = p.dictionary_bytes;
        previous_id = p.id;
        previous_event = p.event;
        previous_stage = p.stage;
        previous_from = p.from;
        previous_begin = p.begin;
    }

    void reset() {
        for (unsigned i = 6; i <= 12; ++i)
            attributes[i - 6].swap(columns[i]);
        for (unsigned i = 24; i <= 28; ++i)
            arguments.columns[i - 24].swap(columns[i]);
        for (auto& column : columns)
            column.clear();
        for (auto& column : attributes)
            column.clear();
        for (auto& column : arguments.columns)
            column.clear();
        arguments.dictionary.clear();
        arguments.text_ids.clear();
        arguments.text_chunks.clear();
        previous_id = previous_event = previous_stage = previous_from = 0;
        previous_begin = 0;
        dictionary_bytes = 0;
    }

    Bytes finish() {
        arguments.finish();
        for (unsigned i = 6; i <= 12; ++i) {
            columns[i] = std::move(attributes[i - 6]);
        }
        for (unsigned i = 24; i <= 28; ++i) {
            columns[i] = std::move(arguments.columns[i - 24]);
        }
        return d::pack_columns(columns);
    }
};
} // namespace

Bytes encode_transaction_block(std::uint64_t number, const Types& types,
                               const Generators& generators,
                               std::span<const Transaction> transactions,
                               std::span<const Relation> relations, RecordPolicy policy,
                               std::uint64_t string_count, NumericAttributesView numeric,
                               const TextReferences* shared_text, TransactionWorkspace* workspace) {
    require(number && (!transactions.empty() || !relations.empty()) && policy.page_bytes &&
                policy.codec <= vtr::detail::wire::Codec::zstd,
            "invalid transaction block/policy");
    numeric.validate(transactions.size());
    std::vector<const Transaction*> sorted, slots;
    if (workspace) {
        sorted.swap(workspace->sorted);
        slots.swap(workspace->slots);
    }
    sorted.clear();
    slots.clear();
    sorted.reserve(transactions.size());
    std::vector<GeneratorId> generator_ids;

    struct Binding {
        Generator generator;
        std::uint32_t local = 0;
    };

    d::HashMap<GeneratorId, Binding> local;
    RecordId first = UINT64_MAX, last = 0;
    for (const auto& tx : transactions) {
        first = std::min(first, tx.id);
        last = std::max(last, tx.id);
        auto [binding, inserted] = local.try_emplace(tx.generator);
        if (inserted) {
            binding->second.generator = generators[tx.generator];
            generator_ids.push_back(tx.generator);
        }
    }
    if (!transactions.empty()) {
        if (last - first < transactions.size() * 4) {
            slots.assign(last - first + 1, nullptr);
            for (const auto& tx : transactions) {
                require(!slots[tx.id - first], "duplicate owner identifier");
                slots[tx.id - first] = &tx;
            }
            for (auto* tx : slots) {
                if (tx) {
                    sorted.push_back(tx);
                }
            }
        } else {
            for (const auto& tx : transactions)
                sorted.push_back(&tx);
            std::sort(sorted.begin(), sorted.end(),
                      [](const auto* a, const auto* b) { return a->id < b->id; });
        }
    }
    for (std::size_t i = 1; i < sorted.size(); ++i) {
        require(sorted[i - 1]->id < sorted[i]->id, "duplicate owner identifier");
    }
    std::sort(generator_ids.begin(), generator_ids.end());
    for (std::size_t i = 0; i < generator_ids.size(); ++i) {
        local.at(generator_ids[i]).local = u32(i);
    }
    std::vector<TransactionPage> pages;
    Bytes blobs;
    TransactionBounds all;
    Encoder encoder;
    auto transfer_columns = [&] {
        std::size_t i = 0;
        for (auto& column : encoder.columns)
            column.swap(workspace->buffers[i++]);
        for (unsigned c = 6; c <= 12; ++c)
            encoder.attributes[c - 6].swap(workspace->buffers[i++]);
        for (auto& column : encoder.arguments.columns)
            column.swap(workspace->buffers[i++]);
    };
    if (workspace) {
        for (auto& column : workspace->buffers)
            column.clear();
        transfer_columns();
    }
    encoder.arguments.shared_text = shared_text;
    encoder.arguments.string_count = string_count;
    TransactionBounds bounds;
    auto flush = [&] {
        if (!bounds.transactions && !bounds.relations) {
            return;
        }
        auto raw = encoder.finish();
        auto packed = d::compress(raw, policy.codec, policy.compression_level);
        TransactionPage page;
        static_cast<TransactionBounds&>(page) = bounds;
        page.blob_offset = u32(blobs.size());
        page.blob_length = u32(packed.size());
        page.raw_length = u32(raw.size());
        pages.push_back(page);
        d::append(blobs, packed);
        merge(all, bounds);
        encoder.reset();
        encoder.arguments.string_count = string_count;
        bounds = {};
    };
    auto measured_size = encoder.size();
    std::size_t unmeasured_upper = 0;
    auto append = [&](const auto& row, std::size_t upper, const auto& encode) {
        const bool has_rows = bounds.transactions || bounds.relations;
        auto remaining = [&] {
            return policy.page_bytes - std::min(policy.page_bytes, measured_size);
        };
        if (unmeasured_upper &&
            (unmeasured_upper > remaining() || upper > remaining() - unmeasured_upper)) {
            measured_size = encoder.size();
            unmeasured_upper = 0;
        }
        std::optional<Encoder::Checkpoint> checkpoint;
        if (has_rows && upper > remaining()) {
            checkpoint = encoder.checkpoint();
        }
        encode();
        // Conservative row bounds avoid rescanning all column lengths away from a page boundary.
        if (has_rows && !checkpoint) {
            require(unmeasured_upper <= remaining() && upper <= remaining() - unmeasured_upper,
                    "transaction row bound overflow");
            unmeasured_upper += upper;
        } else {
            measured_size = encoder.size();
            unmeasured_upper = 0;
            if (has_rows && measured_size > policy.page_bytes) {
                encoder.restore(*checkpoint);
                flush();
                encode();
                measured_size = encoder.size();
            }
        }
        add(bounds, row);
        if (measured_size >= policy.page_bytes) {
            flush();
            measured_size = encoder.size();
            unmeasured_upper = 0;
        }
    };
    auto exact_append = [&](const auto& row, std::size_t upper, const auto& encode) {
        const bool has_rows = bounds.transactions || bounds.relations;
        std::optional<Encoder::Checkpoint> checkpoint;
        if (has_rows && upper > policy.page_bytes - std::min(policy.page_bytes, measured_size)) {
            checkpoint = encoder.checkpoint();
        }
        encode();
        measured_size = encoder.size();
        if (has_rows && measured_size > policy.page_bytes) {
            require(checkpoint.has_value(), "transaction row size bound violated");
            encoder.restore(*checkpoint);
            flush();
            encode();
            measured_size = encoder.size();
        }
        add(bounds, row);
        if (measured_size >= policy.page_bytes) {
            flush();
            measured_size = encoder.size();
        }
    };
    TypeId scalar_type = 0;
    if (numeric.values.empty() && relations.empty() &&
        scalar_page::eligible(types, generators, transactions, scalar_type)) {
        const auto columns = 6 + *types[scalar_type].fixed_size / 8;
        const auto overhead = 9 + columns;
        const auto per_page = std::max<std::size_t>(
            1, (policy.page_bytes - std::min<std::uint64_t>(policy.page_bytes, overhead)) /
                   (8 * columns));
        for (std::size_t first = 0; first < sorted.size(); first += per_page) {
            auto rows = std::span<const Transaction* const>(sorted).subspan(
                first, std::min(per_page, sorted.size() - first));
            TransactionBounds b;
            for (const auto* tx : rows) {
                valid(*tx, generators[tx->generator], string_count);
                add(b, *tx);
            }
            auto raw = scalar_page::encode(types, scalar_type, rows,
                                           [&](auto g) { return local.at(g).local; });
            auto packed = d::compress(raw, policy.codec, policy.compression_level);
            TransactionPage page;
            static_cast<TransactionBounds&>(page) = b;
            page.blob_offset = u32(blobs.size());
            page.blob_length = u32(packed.size());
            page.raw_length = u32(raw.size());
            pages.push_back(page);
            d::append(blobs, packed);
            merge(all, b);
        }
    } else if (policy.defer_page_size_scans) {
        for (const auto* tx : sorted) {
            const auto attributes = numeric.row(tx - transactions.data());
            const auto& binding = local.at(tx->generator);
            append(*tx, transaction_upper(*tx, attributes.size()), [&] {
                encoder.append(types, binding.generator, binding.local, *tx, attributes);
            });
        }
        for (const auto& relation : relations) {
            append(relation, transaction_upper(relation), [&] { encoder.append(relation); });
        }
    } else {
        for (const auto* tx : sorted) {
            const auto attributes = numeric.row(tx - transactions.data());
            const auto& binding = local.at(tx->generator);
            exact_append(*tx, transaction_upper(*tx, attributes.size()), [&] {
                encoder.append(types, binding.generator, binding.local, *tx, attributes);
            });
        }
        for (const auto& relation : relations) {
            exact_append(relation, transaction_upper(relation), [&] { encoder.append(relation); });
        }
    }
    flush();
    Bytes encoded;
    d::fixed(encoded, number);
    write_bounds(encoded, all);
    d::fixed(encoded, u32(generator_ids.size()));
    d::fixed(encoded, u32(pages.size()));
    for (auto generator : generator_ids) {
        d::fixed(encoded, generator);
    }
    for (const auto& page : pages) {
        write_bounds(encoded, page);
        d::fixed(encoded, page.blob_offset);
        d::fixed(encoded, page.blob_length);
        d::fixed(encoded, page.raw_length);
    }
    d::append(encoded, blobs);
    if (workspace) {
        transfer_columns();
        sorted.clear();
        slots.clear();
        sorted.swap(workspace->sorted);
        slots.swap(workspace->slots);
        constexpr std::size_t limit = 2 << 20;
        std::size_t retained = 0;
        for (auto* pointers : {&workspace->sorted, &workspace->slots}) {
            const auto bytes = pointers->capacity() * sizeof(const Transaction*);
            if (bytes > limit - retained)
                std::vector<const Transaction*>{}.swap(*pointers);
            else
                retained += bytes;
        }
        for (auto& column : workspace->buffers) {
            column.clear();
            if (column.capacity() > limit - retained)
                Bytes{}.swap(column);
            else
                retained += column.capacity();
        }
    }
    return encoded;
}

TransactionBlockView::TransactionBlockView(View encoded, const Types& types,
                                           const Generators& generators, std::uint64_t strings,
                                           std::size_t max_payload_bytes,
                                           std::span<const std::string> shared_text)
    : shared_text_(shared_text), types_(types), generators_(generators), string_count_(strings),
      max_payload_bytes_(max_payload_bytes) {
    Cursor r(encoded);
    number_ = r.fixed<std::uint64_t>();
    bounds_ = read_bounds(r);
    require(number_ != 0, "invalid transaction block number");
    auto generator_count = r.fixed<std::uint32_t>(), page_count = r.fixed<std::uint32_t>();
    require(generator_count <= r.data.size() / 4 && page_count != 0,
            "invalid transaction table count");
    for (std::uint32_t i = 0; i < generator_count; ++i) {
        auto id = r.fixed<std::uint32_t>();
        const auto& gen = generators_[id];
        types_[gen.payload_type];
        require((gen.kind == RecordKind::ordinary || gen.kind == RecordKind::clock) &&
                    (generator_ids_.empty() || generator_ids_.back() < id),
                "invalid transaction generator table");
        generator_ids_.push_back(id);
    }
    require((bounds_.transactions != 0) == !generator_ids_.empty() &&
                page_count <= r.data.size() / 124,
            "invalid transaction page count");
    std::uint64_t blob_end = 0, previous_id = 0;
    bool relation_only = false;
    TransactionBounds aggregate;
    for (std::uint32_t i = 0; i < page_count; ++i) {
        TransactionPage page;
        static_cast<TransactionBounds&>(page) = read_bounds(r);
        page.blob_offset = r.fixed<std::uint32_t>();
        page.blob_length = r.fixed<std::uint32_t>();
        page.raw_length = r.fixed<std::uint32_t>();
        require(page.blob_length && page.raw_length && page.blob_offset == blob_end,
                "invalid transaction page partition");
        if (page.transactions) {
            require(!relation_only && page.min_id > previous_id,
                    "unordered transaction page ID ranges");
            previous_id = page.max_id;
        } else {
            relation_only = true;
        }
        blob_end += page.blob_length;
        merge(aggregate, page);
        pages_.push_back(page);
    }
    require(equal(aggregate, bounds_), "transaction block/page bounds disagree");
    blobs_ = r.take(blob_end);
    r.done();
}

std::optional<std::pair<Timestamp, Timestamp>> TransactionBlockView::time_range() const {
    if (!bounds_.transactions) {
        return {};
    }
    std::pair<Timestamp, Timestamp> range{bounds_.begin, bounds_.end};
    for (const auto& page : pages_) {
        if (!page.max_embedded) {
            continue;
        }
        auto raw = d::decompress(blobs_.subspan(page.blob_offset, page.blob_length));
        require(raw.size() == page.raw_length, "transaction page raw length mismatch");
        auto [begin, end] = time_range_of(raw, page);
        range = {std::min(range.first, begin), std::max(range.second, end)};
    }
    return range;
}

void TransactionBlockView::activity(
    const std::function<void(std::size_t)>& admit,
    const std::function<void(GeneratorId, Timestamp, Timestamp)>& visit) const {
    for (const auto& page : pages_) {
        admit(page.raw_length);
        auto raw = d::decompress(blobs_.subspan(page.blob_offset, page.blob_length));
        require(raw.size() == page.raw_length, "activity page size mismatch");
        if (!raw.empty() && raw[0] == 2) {
            d::Cursor r(raw);
            r.byte();
            auto datatype = r.fixed<std::uint32_t>();
            auto count = r.fixed<std::uint32_t>();
            require(count && count == page.transactions && count <= raw.size(),
                    "invalid activity count");
            const auto& schema = types_[datatype];
            require(schema.unsigned_words && schema.fixed_size && *schema.fixed_size <= 2048,
                    "invalid activity scalar type");
            std::array<View, 4> planes;
            for (std::size_t col = 0; col < 6 + *types_[datatype].fixed_size / 8; ++col) {
                auto width = r.byte();
                require(width <= 8, "invalid activity scalar width");
                auto data = r.take(std::size_t(width) * count);
                if (col < 4) {
                    planes[col] = data;
                }
            }
            r.done();
            auto value = [&](std::size_t col, std::size_t row) {
                std::uint64_t result = 0;
                for (std::size_t byte = 0; byte < planes[col].size() / count; ++byte) {
                    result |= std::uint64_t(planes[col][byte * count + row]) << (byte * 8);
                }
                return result;
            };
            for (std::size_t row = 0; row < count; ++row) {
                auto generator = value(1, row), begin = value(2, row), length = value(3, row);
                require(generator < generator_ids_.size() && length <= UINT64_MAX - begin,
                        "invalid activity generator/time");
                visit(generator_ids_[generator], begin, begin + length);
            }
        } else {
            auto columns = d::unpack_columns<33>(raw);
            d::Cursor generators(columns[1]), begins(columns[2]), lengths(columns[3]);
            require(page.transactions <= columns[2].data.size(), "invalid activity count");
            Timestamp begin = 0;
            for (std::uint64_t row = 0; row < page.transactions; ++row) {
                auto generator = generators.var();
                begin += begins.signed_bits();
                auto length = lengths.var();
                require(generator < generator_ids_.size() && length <= UINT64_MAX - begin,
                        "invalid activity generator/time");
                visit(generator_ids_[generator], begin, begin + length);
            }
            generators.done();
            begins.done();
            lengths.done();
        }
    }
}

namespace {
bool read_relations(std::array<Cursor, 33>& c, std::array<Cursor, 7>& attributes,
                    std::uint64_t count, std::uint64_t strings, TransactionBounds& actual,
                    const std::function<bool(const Relation&)>& visit_relation) {
    RecordId previous_from = 0;
    for (std::uint64_t i = 0; i < count; ++i) {
        Relation relation;
        relation.kind = c[21].id();
        relation.source = previous_from + c[22].signed_bits();
        relation.destination = relation.source + c[23].signed_bits();
        require(relation.kind < strings, "unknown relation kind");
        previous_from = relation.source;
        relation.attributes = d::read_column_attrs(attributes);
        valid_attrs(relation.attributes, strings);
        add(actual, relation);
        if (!visit_relation(relation)) {
            return false;
        }
    }
    return true;
}
} // namespace

bool TransactionBlockView::decode_page(const TransactionPage& page,
                                       const std::function<bool(const Transaction&)>& visit_tx,
                                       const std::function<bool(const Relation&)>& visit_relation,
                                       std::optional<TransactionQuery> query) const {
    auto raw = d::decompress(blobs_.subspan(page.blob_offset, page.blob_length));
    require(raw.size() == page.raw_length, "transaction page raw length mismatch");
    if (!raw.empty() && raw[0] == 2) {
        require(!page.relations, "scalar page contains relations");
        TransactionBounds actual;
        RecordId previous = 0;
        bool complete = scalar_page::decode(
            raw, types_, generators_, generator_ids_, page.transactions, max_payload_bytes_,
            [&](const auto& tx) {
                valid(tx, generators_[tx.generator], string_count_);
                require(tx.id > previous, "unordered scalar IDs");
                previous = tx.id;
                add(actual, tx);
                return !query || matches_query(*query, tx) ? visit_tx(tx) : true;
            });
        if (!complete) {
            return false;
        }
        require(equal(actual, page), "scalar page bounds disagree");
        return true;
    }
    auto c = d::unpack_columns<33>(raw);
    std::array<Cursor, 7> attributes;
    for (unsigned i = 6; i <= 12; ++i) {
        attributes[i - 6] = c[i];
        c[i] = Cursor();
    }
    std::array<View, 5> argument_columns;
    for (unsigned i = 24; i <= 28; ++i) {
        argument_columns[i - 24] = c[i].data;
        c[i] = Cursor();
    }
    ArgumentReader arguments(argument_columns, string_count_, max_payload_bytes_, shared_text_);
    RecordId previous_id = 0, previous_event = 0, previous_stage = 0;
    Timestamp previous_begin = 0;
    TransactionBounds actual;
    require(page.transactions <= c[0].data.size() && page.relations <= c[21].data.size(),
            "invalid page record count");
    for (std::uint64_t i = 0; i < page.transactions; ++i) {
        Transaction tx;
        tx.id = previous_id + c[0].signed_bits();
        auto local = c[1].var();
        require(local < generator_ids_.size(), "invalid local generator index");
        tx.generator = generator_ids_[local];
        tx.begin = previous_begin + c[2].signed_bits();
        auto duration = c[3].var();
        require(duration <= UINT64_MAX - tx.begin, "transaction end overflow");
        tx.end = tx.begin + duration;
        auto classification = c[4].byte();
        tx.status = classification & 15;
        tx.span_kind = classification >> 4;
        auto distance = c[5].var();
        require(!distance || distance < tx.id, "invalid transaction parent distance");
        tx.parent = distance ? tx.id - distance : 0;
        const bool retain = !query || matches_owner(*query, tx);
        const auto& generator = generators_[tx.generator];
        if (retain)
            tx.attributes = d::read_column_attrs(attributes);
        else
            d::skip_column_attrs(attributes, string_count_);
        auto count = c[13].var();
        require(generator.kind != RecordKind::clock || !count, "clock has embedded events");
        require(count <= c[15].data.size() && count <= c[31].data.size(),
                "invalid embedded event count");
        for (std::uint64_t j = 0; j < count; ++j) {
            EmbeddedEvent event;
            event.time = tx.begin + c[14].signed_bits();
            event.name = c[15].id();
            event.id = previous_event + c[31].signed_bits();
            previous_event = event.id;
            require(event.id > tx.id && event.name < string_count_, "invalid embedded event");
            add_embedded(actual, event.id);
            if (retain) {
                event.attributes = d::read_column_attrs(attributes);
                tx.events.push_back(std::move(event));
            } else
                d::skip_column_attrs(attributes, string_count_);
        }
        count = c[16].var();
        require(generator.kind != RecordKind::clock || !count, "clock has embedded stages");
        require(count <= c[17].data.size() && count <= c[32].data.size(),
                "invalid embedded stage count");
        for (std::uint64_t j = 0; j < count; ++j) {
            EmbeddedStage stage;
            stage.name = c[17].id();
            stage.lane = c[18].id();
            stage.begin = tx.begin + c[19].signed_bits();
            stage.end = read_stage_end(c[20], stage.begin);
            stage.id = previous_stage + c[32].signed_bits();
            previous_stage = stage.id;
            require(stage.id > tx.id && stage.name < string_count_ && stage.lane < string_count_,
                    "invalid embedded stage");
            add_embedded(actual, stage.id);
            if (retain) {
                stage.attributes = d::read_column_attrs(attributes);
                tx.stages.push_back(std::move(stage));
            } else
                d::skip_column_attrs(attributes, string_count_);
        }
        if (retain)
            tx.payload = arguments.read(types_, generator.payload_type);
        else
            arguments.skip(types_, generator.payload_type);
        count = c[29].var();
        require(generator.kind != RecordKind::clock || !count, "clock has located children");
        require(count <= c[30].data.size(), "invalid located block count");
        std::uint64_t block = 0;
        for (std::uint64_t j = 0; j < count; ++j) {
            auto delta = c[30].var();
            require(delta && delta <= UINT64_MAX - block, "invalid located block delta");
            block += delta;
            if (retain)
                tx.child_blocks.push_back(block);
        }
        valid(tx, generator, string_count_);
        require(!i || previous_id < tx.id, "unsorted transaction IDs");
        previous_id = tx.id;
        previous_begin = tx.begin;
        add(actual, tx);
        if (retain && (!query || matches_query(*query, tx)) && !visit_tx(tx)) {
            return false;
        }
    }
    if (!read_relations(c, attributes, page.relations, string_count_, actual, visit_relation)) {
        return false;
    }
    for (const auto& column : c) {
        column.done();
    }
    for (unsigned i = 6; i <= 12; ++i) {
        attributes[i - 6].done();
    }
    arguments.done();
    require(equal(actual, page), "decoded transaction page bounds disagree");
    return true;
}

bool TransactionBlockView::decode_relations(
    const TransactionPage& page, const std::function<bool(const Relation&)>& visit) const {
    auto raw = d::decompress(blobs_.subspan(page.blob_offset, page.blob_length));
    require(raw.size() == page.raw_length, "transaction page raw length mismatch");
    auto c = d::unpack_columns<33>(raw);
    std::array<Cursor, 7> attributes;
    for (unsigned i = 0; i < 7; ++i)
        attributes[i] = c[i + 6];
    require(page.transactions <= c[13].data.size() && page.transactions <= c[16].data.size() &&
                page.relations <= c[21].data.size(),
            "invalid page record count");
    for (std::uint64_t i = 0; i < page.transactions; ++i) {
        d::skip_column_attrs(attributes, string_count_);
        auto events = c[13].var(), stages = c[16].var();
        require(events <= c[15].data.size() && stages <= c[17].data.size(),
                "invalid embedded count");
        for (std::uint64_t j = 0; j < events; ++j)
            d::skip_column_attrs(attributes, string_count_);
        for (std::uint64_t j = 0; j < stages; ++j)
            d::skip_column_attrs(attributes, string_count_);
    }
    TransactionBounds actual;
    if (!read_relations(c, attributes, page.relations, string_count_, actual, visit))
        return false;
    for (auto i : {13, 16, 21, 22, 23})
        c[i].done();
    for (const auto& column : attributes)
        column.done();
    require(actual.relations == page.relations && actual.min_from == page.min_from &&
                actual.max_from == page.max_from && actual.min_to == page.min_to &&
                actual.max_to == page.max_to,
            "decoded relation page bounds disagree");
    return true;
}

void TransactionBlockView::scan_relations(const std::function<bool(const Relation&)>& visit) const {
    for (const auto& page : pages_) {
        if (page.relations && !decode_relations(page, visit))
            return;
    }
}

bool TransactionBlockView::matches(TransactionQuery query) const {
    return bounds_.transactions &&
           (!query.generator ||
            std::binary_search(generator_ids_.begin(), generator_ids_.end(), *query.generator)) &&
           (!query.id || (*query.id >= bounds_.min_id && *query.id <= bounds_.max_id)) &&
           (!query.embedded_id || (*query.embedded_id >= bounds_.min_embedded &&
                                   *query.embedded_id <= bounds_.max_embedded));
}

void TransactionBlockView::transactions(
    TransactionQuery query, const std::function<bool(const Transaction&)>& visit) const {
    require(query.begin <= query.end, "invalid transaction window");
    if (!matches(query))
        return;
    for (const auto& page : pages_) {
        if (!page.transactions || page.begin > query.end || page.end < query.begin ||
            (query.parent && *query.parent &&
             (*query.parent < page.min_parent || *query.parent > page.max_parent)) ||
            (query.id && (*query.id < page.min_id || *query.id > page.max_id)) ||
            (query.embedded_id &&
             (*query.embedded_id < page.min_embedded || *query.embedded_id > page.max_embedded))) {
            continue;
        }
        bool keep_going = decode_page(page, visit, [](const auto&) { return true; }, query);
        if (!keep_going) {
            return;
        }
    }
}

void TransactionBlockView::relations(RecordId endpoint, bool incoming,
                                     const std::function<bool(const Relation&)>& visit) const {
    for (const auto& page : pages_) {
        auto min = incoming ? page.min_to : page.min_from,
             max = incoming ? page.max_to : page.max_from;
        if (!page.relations || endpoint < min || endpoint > max) {
            continue;
        }
        if (!decode_relations(page, [&](const auto& relation) {
                return (incoming ? relation.destination : relation.source) != endpoint ||
                       visit(relation);
            })) {
            return;
        }
    }
}

void TransactionBlockView::scan(const std::function<bool(const Transaction&)>& tx,
                                const std::function<bool(const Relation&)>& relation) const {
    std::unordered_set<GeneratorId> used;
    for (const auto& page : pages_) {
        if (!decode_page(
                page,
                [&](const auto& record) {
                    used.insert(record.generator);
                    return tx(record);
                },
                relation)) {
            return;
        }
    }
    require(used.size() == generator_ids_.size(), "unused transaction generator table entry");
}

void TransactionBlockView::validate() const {
    std::unordered_set<RecordId> ids;
    std::unordered_set<GeneratorId> used_generators;
    for (const auto& page : pages_) {
        decode_page(
            page,
            [&](const auto& tx) {
                require(ids.insert(tx.id).second, "duplicate transaction ID");
                used_generators.insert(tx.generator);
                for (const auto& e : tx.events) {
                    require(ids.insert(e.id).second, "duplicate embedded ID");
                }
                for (const auto& s : tx.stages) {
                    require(ids.insert(s.id).second, "duplicate embedded ID");
                }
                return true;
            },
            [](const auto&) { return true; });
    }
    require(used_generators.size() == generator_ids_.size(),
            "unused transaction generator table entry");
}
} // namespace vtr::detail::engine
