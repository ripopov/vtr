#include "containers.h"
#include "engine/vtr.h"
#include "format.h"
#include "payload_pool.h"
#include "transaction_memory.h"

namespace vtr::detail::engine {
namespace {
namespace d = vtr::detail::wire::detail;
using d::require;

std::size_t attr_bytes(std::span<const vtr::detail::wire::Attribute> attrs) {
    auto n = attrs.size() * sizeof(vtr::detail::wire::Attribute);
    for (const auto& a : attrs) {
        n += a.value.payload.size();
    }
    return n;
}

std::size_t tx_bytes(const Transaction& tx) {
    auto n =
        sizeof(tx) + tx.payload.size() + attr_bytes(tx.attributes) + tx.child_blocks.size() * 8;
    for (const auto& e : tx.events) {
        n += sizeof(e) + attr_bytes(e.attributes);
    }
    for (const auto& s : tx.stages) {
        n += sizeof(s) + attr_bytes(s.attributes);
    }
    return n;
}

using detail::attrs_upper;
using detail::memory_add;
using detail::memory_mul;
using detail::owned_attrs;
using detail::owned_tx;
using detail::transaction_upper;

std::size_t schema_memory(const Types& types, const Generators& generators) {
    auto bytes =
        memory_add(memory_mul(types.size(), 2 * sizeof(Type)), memory_mul(types.size() + 64, 128));
    bytes = memory_add(
        bytes, memory_mul(types.attribute_capacity(), 2 * sizeof(vtr::detail::wire::Attributes)));
    bytes = memory_add(bytes, sizeof(Generators));
    bytes = memory_add(bytes, memory_mul(generators.size() + 1, 128));
    for (TypeId id = 0; id < types.size(); ++id) {
        if (id == unsigned(Primitive::enumeration)) {
            continue;
        }
        const auto& type = types[id];
        const auto& attributes = types.attributes(id);
        for (auto n : {type.name.capacity(), memory_mul(type.fields.capacity(), sizeof(Field)),
                       memory_mul(type.enumerators.capacity(), sizeof(Enumerator)),
                       type.scalar_program.capacity(),
                       memory_mul(attributes.capacity(), sizeof(vtr::detail::wire::Attribute))}) {
            bytes = memory_add(bytes, n);
        }
        for (const auto& attribute : attributes)
            bytes = memory_add(bytes, attribute.value.payload.capacity());
        for (const auto& field : type.fields) {
            bytes = memory_add(bytes, field.name.capacity());
        }
        for (const auto& value : type.enumerators) {
            bytes = memory_add(bytes, value.name.capacity());
        }
    }
    return bytes;
}

std::size_t record_charge(const std::vector<LocatedRecord>& rows) {
    auto owned = memory_mul(rows.capacity(), sizeof(LocatedRecord));
    auto encoded = std::size_t(4096);
    for (const auto& row : rows) {
        owned = memory_add(owned, row.payload.capacity());
        encoded = memory_add(encoded, memory_add(256, memory_mul(row.payload.size(), 10)));
    }
    return memory_add(owned, memory_mul(encoded, 12));
}

std::size_t transaction_charge(const std::vector<Transaction>& rows,
                               const std::vector<Relation>& relations,
                               const NumericAttributeColumns& numeric) {
    auto owned = memory_mul(rows.capacity() - rows.size(), sizeof(Transaction));
    owned = memory_add(owned, memory_mul(relations.capacity(), sizeof(Relation)));
    owned = memory_add(owned, numeric.owned_bytes());
    auto encoded = std::size_t(4096);
    for (const auto& row : rows) {
        owned = memory_add(owned, owned_tx(row));
        encoded = memory_add(
            encoded, transaction_upper(row, numeric.view().row(&row - rows.data()).size(), 640));
    }
    for (const auto& relation : relations) {
        owned = memory_add(owned, owned_attrs(relation.attributes));
        encoded = memory_add(encoded, memory_add(640, attrs_upper(relation.attributes)));
    }
    return memory_add(owned, memory_mul(encoded, 12));
}

vtr::detail::wire::WriterOptions native_options(const WriterOptions& o) {
    require(o.page_bytes && o.block_bytes && o.record_limit && o.wave_records &&
                o.codec <= vtr::detail::wire::Codec::zstd,
            "invalid writer options", ErrorCode::invalid_argument);
    vtr::detail::wire::WriterOptions result;
    result.codec = o.codec;
    result.compression_level = o.compression_level;
    result.group_size = o.group_size;
    result.block_records = o.wave_records;
    result.wave_bytes = o.wave_bytes;
    result.checksums = o.checksums;
    result.dedup = o.dedup;
    result.workers = o.workers;
    result.queue_jobs = o.queue_jobs;
    result.queue_bytes = o.queue_bytes;
    return result;
}
} // namespace

struct Writer::Impl {
    struct Snapshot {
        Types types;
        Generators generators;
        std::size_t charge;

        Snapshot(const Types& t, const Generators& g)
            : types(t), generators(g), charge(schema_memory(types, generators)) {}
    };

    std::shared_ptr<const Snapshot> snapshot;

    std::shared_ptr<const Snapshot> schemas() {
        if (!snapshot) {
            snapshot = std::make_shared<const Snapshot>(types, generators);
        }
        return snapshot;
    }

    struct Owner {
        Transaction value;
        bool payload_ready = false;
        std::uint8_t numeric_count = 0;
        TypeId payload_type = 0;
        Id maximum_attribute_key = 0;
        std::uint32_t numeric_slot = UINT32_MAX;
        std::vector<RecordId> stages;
    };

    struct Stage {
        LocatedRecord record;
        bool payload_ready;
    };

    struct Clock {
        RecordId id = 0;
        Timestamp begin = 0, period = 0;
        std::optional<Timestamp> previous_edge;
    };

    struct AttributeName {
        std::uint64_t id_plus_one = 0;
        std::string_view name;
    };

    WriterOptions options;
    // Native interned characters stay immutable through index growth and rehash.
    std::array<AttributeName, 64> attribute_names;
    Types types;
    Generators generators;
    std::size_t written_types = first_declared_type;
    RecordId next_id = 1, assigned_id = 0, max_reference = 0;
    std::uint64_t next_block = 1, record_number = 0, transaction_number = 0;
    std::vector<Owner> owners{1024};
    d::HashMap<RecordId, Owner> overflow;
    d::HashMap<RecordId, Stage> stages;
    d::HashMap<GeneratorId, Clock> clocks;
    std::size_t owner_count = 0;
    std::vector<LocatedRecord> records;
    std::vector<Transaction> transactions;
    NumericAttributeColumns numeric_attributes;
    TransactionWorkspace transaction_columns;
    std::size_t shared_text_remaining = 4 << 20;
    std::vector<std::array<NumericAttribute, 8>> numeric_owners;
    std::vector<std::uint32_t> free_numeric_owners;

    std::span<const NumericAttribute> numeric_row(const Owner& owner) const {
        if (!owner.numeric_count)
            return {};
        return std::span(numeric_owners[owner.numeric_slot]).first(owner.numeric_count);
    }

    std::array<NumericAttribute, 8>& numeric_storage(Owner& owner) {
        if (owner.numeric_slot == UINT32_MAX) {
            if (free_numeric_owners.empty()) {
                require(numeric_owners.size() < UINT32_MAX, "too many live numeric attribute rows");
                owner.numeric_slot = static_cast<std::uint32_t>(numeric_owners.size());
                numeric_owners.emplace_back();
            } else {
                owner.numeric_slot = free_numeric_owners.back();
                free_numeric_owners.pop_back();
            }
        }
        return numeric_owners[owner.numeric_slot];
    }

    void release_numeric(Owner& owner) {
        if (owner.numeric_slot != UINT32_MAX) {
            free_numeric_owners.push_back(owner.numeric_slot);
            owner.numeric_slot = UINT32_MAX;
        }
        owner.numeric_count = 0;
    }

    std::string_view attribute_name(Id key) {
        auto& cached = attribute_names[key % attribute_names.size()];
        const auto token = std::uint64_t(key) + 1;
        if (cached.id_plus_one != token) {
            const auto name = native.string(key);
            cached.name = name;
            cached.id_plus_one = token;
        }
        return cached.name;
    }

    void validate_attribute(const Owner& owner, Id key, std::string_view name, Tag tag) const {
        if (name == "vtr.label")
            require(tag == Tag::str || tag == Tag::text, "invalid label type");
        if ((!owner.value.attributes.empty() || owner.numeric_count) &&
            key <= owner.maximum_attribute_key) {
            d::unique_attr(owner.value.attributes, key);
            for (std::size_t i = 0; i < owner.numeric_count; ++i)
                require(numeric_owners[owner.numeric_slot][i].key != key, "duplicate attribute key",
                        ErrorCode::invalid_argument);
        }
        if (owner.payload_type)
            for (const auto& field : types[owner.payload_type].fields)
                require(field.name != name, "attribute duplicates payload field");
    }

    bool store_numeric_attribute(Owner& owner, Id key, Tag tag, std::uint64_t value) {
        if (!owner.value.attributes.empty() || owner.numeric_count == 8)
            return false;
        numeric_storage(owner)[owner.numeric_count++] = {key, tag, value};
        owner.maximum_attribute_key = std::max(owner.maximum_attribute_key, key);
        return true;
    }

    Attributes& full_attributes(Owner& owner) {
        auto& attrs = owner.value.attributes;
        if (attrs.empty()) {
            attrs.reserve(std::max<std::size_t>(4, owner.numeric_count + 1));
            for (const auto& attribute : numeric_row(owner)) {
                Value scalar;
                scalar.tag = attribute.tag;
                scalar.number = attribute.number;
                attrs.push_back({attribute.key, std::move(scalar)});
            }
            release_numeric(owner);
        }
        return attrs;
    }

    std::vector<Relation> relations;
    std::size_t record_bytes = 0, transaction_bytes = 0;
    WriterStats counters;
    bool closed = false, poisoned = false;
    std::shared_ptr<detail::PayloadPool> payload_pool;
    std::vector<Bytes> spare_payloads;
    std::size_t spare_payload_bytes = 0;

    Bytes take_payload() {
        if (spare_payloads.empty() && payload_pool) {
            spare_payload_bytes = payload_pool->take(spare_payloads);
        }
        if (spare_payloads.empty()) {
            return {};
        }
        auto result = std::move(spare_payloads.back());
        spare_payloads.pop_back();
        spare_payload_bytes -= result.capacity();
        ++counters.payload_reuses;
        result.clear();
        return result;
    }

    Bytes copy_payload(View data) {
        if (data.empty()) {
            return {};
        }
        auto result = take_payload();
        result.assign(data.begin(), data.end());
        return result;
    }

    // Keep output last so earlier allocation failures cannot finalize a trace
    // before the incomplete marker is installed.
    vtr::detail::wire::Writer native;

    Impl(const std::string& path, WriterOptions opts)
        : options(opts), payload_pool(opts.workers || opts.payload_pool_bytes.value_or(0)
                                          ? std::make_shared<detail::PayloadPool>(
                                                opts.payload_pool_bytes.value_or(16 << 20) / 2)
                                          : nullptr),
          native(path, native_options(opts)) {
        native.mark_incomplete();
    }

    void alive() const {
        require(!closed && !poisoned, "writer is closed or poisoned", ErrorCode::invalid_state);
        if (options.workers) {
            native.check_failure();
        }
    }

    template <class F> decltype(auto) call(F&& action) {
        try {
            return action();
        } catch (const vtr::detail::wire::Error& error) {
            if (native.failed() || (error.code() != ErrorCode::invalid_argument &&
                                    error.code() != ErrorCode::invalid_state &&
                                    error.code() != ErrorCode::corrupt_data)) {
                poisoned = true;
            }
            if (!native.failed() && error.code() == ErrorCode::corrupt_data)
                throw Error(ErrorCode::invalid_argument, error.what());
            throw;
        } catch (...) {
            poisoned = true;
            throw;
        }
    }

    template <class F> decltype(auto) critical(F&& action) {
        try {
            return action();
        } catch (...) {
            poisoned = true;
            throw;
        }
    }

    RecordId allocate() {
        auto id = std::exchange(assigned_id, 0);
        if (!id) {
            require(next_id, "record identifier exhaustion", ErrorCode::id_exhausted);
            id = next_id++;
        } else if (next_id && id >= next_id) {
            next_id = id + 1;
        }
        counters.max_created_id = std::max(counters.max_created_id, id);
        return id;
    }

    std::uint64_t allocate_block() {
        require(next_block, "block number exhaustion", ErrorCode::id_exhausted);
        return next_block++;
    }

    RecordPolicy policy() const {
        // Deferred scans regress high-generator workloads under concurrent encoding.
        return {options.page_bytes, options.codec, options.compression_level, options.separate_logs,
                options.workers == 0};
    }

    Owner& owner(RecordId id) {
        alive();
        auto& slot = owners[id & (owners.size() - 1)];
        if (id && slot.value.id == id) {
            return slot;
        }
        auto i = overflow.find(id);
        require(i != overflow.end(), "owner is not active");
        return i->second;
    }

    void key(Id id) const {
        native.string(id);
    }

    void attrs(std::span<const vtr::detail::wire::Attribute> list) const {
        d::validate_attrs(list);
        for (const auto& a : list) {
            key(a.key);
            d::validate_value(a.value, native.string_count());
            if (native.string(a.key) == "vtr.label") {
                require(a.value.tag == Tag::str || a.value.tag == Tag::text, "invalid label type");
            }
        }
    }

    void write_types() {
        critical([&] {
            if (written_types == types.size()) {
                return;
            }
            native.append_section(10, types.declarations(written_types));
            written_types = types.size();
            snapshot.reset();
        });
    }

    RecordId begin(GeneratorId gen, Timestamp time, std::optional<View> payload, RecordId parent) {
        alive();
        const auto& generator = generators[gen];
        require(generator.kind == RecordKind::ordinary,
                "ordinary transaction requires an ordinary generator");
        if (parent) {
            owner(parent);
        }
        if (payload) {
            types.validate(generator.payload_type, *payload, native.string_count());
        }
        auto id = allocate();
        auto& slot = owners[id & (owners.size() - 1)];
        if (slot.value.id) {
            auto displaced = slot.value.id;
            overflow.emplace(displaced, std::move(slot));
            slot = Owner{};
        }
        slot.value.id = id;
        slot.value.parent = parent;
        slot.value.generator = gen;
        slot.value.begin = time;
        slot.value.end = 0;
        slot.value.status = 0;
        slot.value.span_kind = 0;
        slot.maximum_attribute_key = 0;
        slot.numeric_count = 0;
        slot.payload_type = generator.payload_type;
        slot.payload_ready = payload.has_value() || generator.payload_type == 0 ||
                             types[generator.payload_type].fixed_size == 0;
        if (payload) {
            slot.value.payload = copy_payload(*payload);
        }
        ++owner_count;
        return id;
    }

    void flush_records() {
        critical([&] {
            if (records.empty()) {
                return;
            }
            auto begin = UINT64_MAX, end = std::uint64_t(0);
            for (const auto& record : records) {
                begin = std::min(begin, record.begin);
                end = std::max(end, record.end);
            }
            if (options.workers) {
                auto schema = schemas();
                const auto charge = memory_add(schema->charge, record_charge(records));
                auto batch = std::make_shared<std::vector<LocatedRecord>>(std::move(records));
                auto number = record_number;
                auto format = policy();
                auto strings = native.string_count();
                auto pool = payload_pool;
                native.enqueue_section(
                    9, begin, end, number, charge, [schema, batch, number, format, strings, pool] {
                        auto encoded = encode_record_block(
                            number, schema->types, schema->generators, *batch, format, strings);
                        pool->recycle(*batch, true);
                        return encoded;
                    });
            } else {
                auto encoded = encode_record_block(record_number, types, generators, records,
                                                   policy(), native.string_count());
                native.append_section(9, encoded, begin, end, record_number);
                if (payload_pool)
                    payload_pool->recycle(records, false);
            }
            ++counters.record_blocks;
            records.clear();
            record_number = 0;
            record_bytes = 0;
        });
    }

    void flush_transactions() {
        critical([&] {
            if (transactions.empty() && relations.empty()) {
                return;
            }
            auto begin = UINT64_MAX, end = std::uint64_t(0);
            std::vector<const Transaction*> sorted_text;
            for (const auto& tx : transactions) {
                begin = std::min(begin, tx.begin);
                end = std::max(end, tx.end);
                if (tx.payload.size() >= shared_text_minimum &&
                    types[generators[tx.generator].payload_type].contains_text)
                    sorted_text.push_back(&tx);
            }
            if (transactions.empty())
                begin = 0;
            std::shared_ptr<TextReferences> text;
            std::sort(sorted_text.begin(), sorted_text.end(),
                      [](auto a, auto b) { return a->id < b->id; });
            for (const auto* tx : sorted_text) {
                types.visit_text(
                    generators[tx->generator].payload_type, tx->payload,
                    [&](std::string_view value) {
                        if (value.size() < shared_text_minimum)
                            return;
                        if (text && text->contains(value))
                            return;
                        if (auto id = native.intern_with_budget(value, shared_text_remaining)) {
                            if (!text)
                                text = std::make_shared<TextReferences>();
                            text->emplace(value, *id);
                        }
                    });
            }
            if (options.workers) {
                auto schema = schemas();
                const auto charge = memory_add(
                    memory_add(schema->charge,
                               transaction_charge(transactions, relations, numeric_attributes)),
                    text ? memory_add(memory_mul(text->values().capacity(),
                                                 sizeof(TextReferences::value_type)),
                                      text->index_bytes())
                         : 0);
                struct Batch {
                    std::vector<Transaction> rows;
                    std::vector<Relation> relations;
                    NumericAttributeColumns numeric;
                };
                auto batch = std::make_shared<Batch>(Batch{
                    std::move(transactions), std::move(relations), std::move(numeric_attributes)});
                auto number = transaction_number;
                auto format = policy();
                auto strings = native.string_count();
                auto pool = payload_pool;
                native.enqueue_section(5, begin, end, number, charge,
                                       [schema, batch, number, format, strings, pool, text] {
                                           auto encoded = encode_transaction_block(
                                               number, schema->types, schema->generators,
                                               batch->rows, batch->relations, format, strings,
                                               batch->numeric.view(), text.get());
                                           pool->recycle(batch->rows, true);
                                           pool->recycle(batch->relations, true);
                                           return encoded;
                                       });
            } else {
                auto encoded = encode_transaction_block(
                    transaction_number, types, generators, transactions, relations, policy(),
                    native.string_count(), numeric_attributes.view(), text.get(),
                    &transaction_columns);
                native.append_section(5, encoded, begin, end, transaction_number);
                if (payload_pool)
                    payload_pool->recycle(transactions, false);
            }
            ++counters.transaction_blocks;
            transactions.clear();
            numeric_attributes.clear();
            relations.clear();
            transaction_number = 0;
            transaction_bytes = 0;
        });
    }

    void submit(LocatedRecord record) {
        const auto bytes = sizeof(record) + record.payload.size();
        if (bytes > options.block_bytes) {
            flush_records();
        }
        if (!record_number) {
            record_number = allocate_block();
        }
        if (generators[record.generator].kind != RecordKind::log) {
            auto& list = owner(record.parent).value.child_blocks;
            if (list.empty() || list.back() != record_number) {
                list.push_back(record_number);
            }
        }
        record_bytes += bytes;
        counters.peak_record_buffer_bytes =
            std::max(counters.peak_record_buffer_bytes, record_bytes);
        if (payload_pool)
            payload_pool->borrow(records);
        records.push_back(std::move(record));
        if (record_bytes >= options.block_bytes || records.size() >= options.record_limit) {
            flush_records();
        }
    }

    void submit(Transaction&& transaction, std::span<const NumericAttribute> numeric = {}) {
        // Logical attribute widths keep batching independent of storage representation.
        const auto bytes = tx_bytes(transaction) + numeric.size() * sizeof(wire::Attribute);
        if (bytes > options.block_bytes) {
            flush_transactions();
        }
        if (!transaction_number) {
            transaction_number = allocate_block();
        }
        transaction_bytes += bytes;
        counters.peak_transaction_buffer_bytes =
            std::max(counters.peak_transaction_buffer_bytes, transaction_bytes);
        if (payload_pool)
            payload_pool->borrow(transactions);
        transactions.push_back(std::move(transaction));
        numeric_attributes.append(numeric);
        if (transaction_bytes >= options.block_bytes ||
            transactions.size() + relations.size() >= options.record_limit) {
            flush_transactions();
        }
    }

    RecordId begin_stage(GeneratorId gen, RecordId parent, Timestamp time,
                         std::optional<View> payload) {
        auto& active = owner(parent);
        const auto& generator = generators[gen];
        require(generator.kind == RecordKind::stage, "stage requires STAGE generator");
        const auto type = generator.payload_type;
        if (payload) {
            types.validate(type, *payload, native.string_count());
        }
        auto id = allocate();
        Stage stage{};
        stage.record.id = id;
        stage.record.generator = gen;
        stage.record.begin = stage.record.end = time;
        stage.record.parent = parent;
        stage.payload_ready = payload.has_value() || type == 0 || types[type].fixed_size == 0;
        if (payload) {
            stage.record.payload = copy_payload(*payload);
        }
        stages.emplace(id, std::move(stage));
        active.stages.push_back(id);
        return id;
    }

    void end_stage(RecordId id, Timestamp time) {
        alive();
        auto i = stages.find(id);
        require(i != stages.end(), "stage is not active");
        require(i->second.payload_ready, "missing mandatory stage payload");
        auto record = std::move(i->second.record);
        auto& active = owner(record.parent).stages;
        auto found = std::find(active.begin(), active.end(), id);
        require(found != active.end(), "active stage owner mismatch");
        *found = active.back();
        active.pop_back();
        stages.erase(i);
        record.end = std::max(record.begin, time);
        submit(std::move(record));
    }

    void end(RecordId id, Timestamp time, std::uint8_t status) {
        auto& active = owner(id);
        require(status <= 4 && active.payload_ready, "invalid status or missing mandatory payload");
        for (auto stage : active.stages) {
            require(stages.at(stage).payload_ready, "missing mandatory stage payload");
        }
        active.value.end = std::max(active.value.begin, time);
        active.value.status = status;
        while (!active.stages.empty()) {
            end_stage(active.stages.back(), active.value.end);
        }
        if (active.stages.capacity() > 8)
            std::vector<RecordId>{}.swap(active.stages);
        submit(std::move(active.value), numeric_row(active));
        release_numeric(active);
        active.value.id = 0;
        active.payload_ready = false;
        if (&active != &owners[id & (owners.size() - 1)]) {
            overflow.erase(id);
        }
        --owner_count;
    }

    template <class Captured>
    void end_with_payload(RecordId id, Timestamp time, Captured&& payload, std::uint8_t status) {
        auto& active = owner(id);
        require(status <= 4, "invalid status");
        types.validate(generators[active.value.generator].payload_type, payload,
                       native.string_count());
        for (auto stage : active.stages)
            require(stages.at(stage).payload_ready, "missing mandatory stage payload");
        if constexpr (std::same_as<std::remove_cvref_t<Captured>, Bytes>)
            active.value.payload = std::move(payload);
        else
            active.value.payload = copy_payload(payload);
        active.payload_ready = true;
        end(id, time, status);
    }

    void stop_clock(GeneratorId gen, Timestamp time, std::uint8_t status) {
        auto i = clocks.find(gen);
        require(i != clocks.end() && i->second.id, "clock is not running");
        auto& clock = i->second;
        auto last =
            clock.begin + (std::max(clock.begin, time) - clock.begin) / clock.period * clock.period;
        Transaction tx;
        tx.id = clock.id;
        tx.generator = gen;
        tx.begin = clock.begin;
        tx.end = last;
        tx.status = status;
        if (last != clock.begin) {
            tx.attributes.push_back({native.intern("vtr.period"), Value::time(clock.period)});
        }
        if (options.separate_clocks) {
            flush_transactions();
        }
        submit(std::move(tx));
        if (options.separate_clocks) {
            flush_transactions();
        }
        clock.previous_edge = last;
        clock.id = 0;
    }

    void flush() {
        alive();
        flush_records();
        flush_transactions();
        native.flush();
    }
};

Writer::Writer(const std::string& path, WriterOptions options)
    : impl_(std::make_unique<Impl>(path, options)) {}

Writer::~Writer() {
    try {
        close();
    } catch (...) {
        try {
            seal();
        } catch (...) {
        }
    }
}

const TraceId& Writer::trace_id() const noexcept {
    return impl_->native.trace_id();
}

vtr::detail::wire::Meta& Writer::metadata() {
    auto& p = *impl_;
    return p.call([&]() -> vtr::detail::wire::Meta& {
        p.alive();
        return p.native.metadata();
    });
}

Id Writer::intern(std::string_view value) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        return p.native.intern(value);
    });
}

Id Writer::add_scope(Id parent, std::string_view name, std::uint16_t type,
                     std::string_view component) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        return p.native.add_scope(parent, name, type, component);
    });
}

std::pair<Id, Id> Writer::add_var(Id parent, std::string_view name, SignalKind kind,
                                  std::uint16_t type, std::uint8_t direction) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        return p.native.add_var(parent, name, kind, type, direction);
    });
}

Id Writer::add_alias(Id parent, std::string_view name, Id signal, std::uint16_t type,
                     std::uint8_t direction) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        return p.native.add_alias(parent, name, signal, type, direction);
    });
}

Id Writer::add_enum_table(Id parent, std::string_view name,
                          std::span<const std::pair<Id, Id>> entries) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        return p.native.add_enum_table(parent, name, entries);
    });
}

TypeId Writer::structure(std::string name, std::vector<Field> fields) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        auto id = p.types.structure(std::move(name), std::move(fields));
        p.write_types();
        return id;
    });
}

TypeId Writer::declare_datatype(std::string name, decltype(vtr::DataType::definition) definition,
                                vtr::detail::wire::Attributes attributes) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        for (const auto& attribute : attributes) {
            require(attribute.key < p.native.string_count(), "Undeclared datatype attribute key",
                    ErrorCode::invalid_argument);
            d::validate_value(attribute.value, p.native.string_count());
        }
        const auto id =
            p.types.declare_datatype(std::move(name), std::move(definition), std::move(attributes));
        p.write_types();
        return id;
    });
}

void Writer::declare_types(View data) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.types.read_declarations(data, p.native.string_count());
        p.write_types();
    });
}

const Types& Writer::types() const {
    impl_->alive();
    return impl_->types;
}

TypeId Writer::bind_type(const std::function<TypeId(Types&)>& bind) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        auto id = bind(p.types);
        p.write_types();
        return id;
    });
}

GeneratorId Writer::add_generator(Id parent, std::string_view name, std::string_view kind,
                                  TypeId type) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        const auto& schema = p.types[type];
        for (const auto& field : schema.fields) {
            if (field.name == "vtr.label") {
                require(field.type == 12 || field.type == 13, "invalid payload label type");
            }
        }
        auto category = record_kind(kind);
        require((category != RecordKind::clock && category != RecordKind::log) || type == 0,
                "clock requires unit payload");
        auto id = p.native.add_generator(parent, name, kind, type);
        p.generators.declare({id, category, type});
        p.snapshot.reset();
        if (category == RecordKind::clock) {
            p.clocks.emplace(id, Impl::Clock{});
        }
        return id;
    });
}

void Writer::node_attribute(Id node, Id key, const Value& value) {
    auto& p = *impl_;
    p.call([&] {
        p.alive();
        p.native.node_attr(node, key, value);
    });
}

void Writer::generator_attribute(GeneratorId generator, Id key, const Value& value) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        auto kind = p.generators[generator].kind;
        auto name = p.native.string(key);
        if (name == "vtr.label") {
            require(value.tag == Tag::str || value.tag == Tag::text, "invalid label metadata type");
        }
        if (name == "vtr.clock") {
            require(value.tag == Tag::u32 && value.number <= UINT32_MAX,
                    "clock association must be u32");
            require(p.generators[static_cast<GeneratorId>(value.number)].kind == RecordKind::clock,
                    "invalid associated clock generator");
        }
        if (kind == RecordKind::log) {
            if (name == "log.severity") {
                require(value.tag == Tag::enumeration, "invalid log severity metadata type");
            }
            if (name == "log.line") {
                require(value.tag == Tag::u64, "invalid numeric log metadata type");
            }
            if (name == "log.file" || name == "log.func") {
                require(value.tag == Tag::str, "invalid log provenance type");
            }
        }
        p.native.node_attr(generator, key, value);
    });
}

void Writer::set_time(Timestamp time) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.set_time(time);
    });
}

void Writer::emit(Id signal, std::uint64_t value) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.emit(signal, value);
    });
}

SignalKind Writer::signal_kind(Id signal) const {
    return impl_->native.signal_kind(signal);
}

void Writer::emit(std::span<const SignalUpdate> updates) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.emit(updates);
    });
}

void Writer::emit_packed(Id signal, std::uint8_t states, View value) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.emit_packed(signal, states, value);
    });
}

void Writer::emit_real(Id signal, double value) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.emit_real(signal, value);
    });
}

void Writer::emit_varlen(Id signal, View value) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.emit_varlen(signal, value);
    });
}

void Writer::dump(bool active) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.native.dump(active);
    });
}

void Writer::assign_record_id(RecordId id) {
    auto& p = *impl_;
    p.call([&] {
        p.alive();
        require(id && !p.assigned_id, "invalid supplied record identifier");
        p.assigned_id = id;
    });
}

RecordId Writer::begin(GeneratorId gen, Timestamp time, RecordId parent) {
    auto& p = *impl_;
    return p.call([&] { return p.begin(gen, time, {}, parent); });
}

RecordId Writer::begin(GeneratorId gen, Timestamp time, View payload, RecordId parent) {
    auto& p = *impl_;
    return p.call([&] { return p.begin(gen, time, payload, parent); });
}

void Writer::payload(RecordId id, View payload) {
    auto& p = *impl_;
    return p.call([&] {
        auto& owner = p.owner(id);
        p.types.validate(p.generators[owner.value.generator].payload_type, payload,
                         p.native.string_count());
        owner.value.payload.assign(payload.begin(), payload.end());
        owner.payload_ready = true;
    });
}

void Writer::attribute(RecordId id, Id key, const Value& value) {
    auto& p = *impl_;
    p.call([&] {
        auto& owner = p.owner(id);
        const auto name = p.attribute_name(key);
        d::validate_value(value, p.native.string_count());
        p.validate_attribute(owner, key, name, value.tag);
        if (numeric_attribute(value.tag) && value.payload.empty() &&
            p.store_numeric_attribute(owner, key, value.tag, value.number))
            return;
        p.full_attributes(owner).push_back({key, value});
        owner.maximum_attribute_key = std::max(owner.maximum_attribute_key, key);
    });
}

void Writer::attribute(RecordId id, Id key, std::uint64_t value) {
    auto& p = *impl_;
    p.call([&] {
        auto& owner = p.owner(id);
        const auto name = p.attribute_name(key);
        p.validate_attribute(owner, key, name, Tag::u64);
        if (p.store_numeric_attribute(owner, key, Tag::u64, value))
            return;
        p.full_attributes(owner).push_back({key, Value(value)});
        owner.maximum_attribute_key = std::max(owner.maximum_attribute_key, key);
    });
}

void Writer::span_kind(RecordId id, std::uint8_t kind) {
    auto& p = *impl_;
    return p.call([&] {
        require(kind <= 5, "invalid span kind");
        p.owner(id).value.span_kind = kind;
    });
}

void Writer::end(RecordId id, Timestamp time, std::uint8_t status) {
    auto& p = *impl_;
    return p.call([&] { p.end(id, time, status); });
}

void Writer::end(RecordId id, Timestamp time, Bytes payload, std::uint8_t status) {
    auto& p = *impl_;
    p.call([&] { p.end_with_payload(id, time, std::move(payload), status); });
}

void Writer::end_captured(RecordId id, Timestamp time, View payload, std::uint8_t status) {
    auto& p = *impl_;
    p.call([&] { p.end_with_payload(id, time, payload, status); });
}

Bytes Writer::prepare_value_end(RecordId id, std::uint8_t status) {
    auto& p = *impl_;
    const auto& active = p.owner(id);
    require(status <= 4, "invalid status");
    for (auto stage : active.stages) {
        require(p.stages.at(stage).payload_ready, "missing mandatory stage payload");
    }
    return p.take_payload();
}

void Writer::capture_failure(ErrorCode code) noexcept {
    if (impl_->native.failed() ||
        (code != ErrorCode::invalid_argument && code != ErrorCode::invalid_state &&
         code != ErrorCode::corrupt_data)) {
        impl_->poisoned = true;
    }
}

void Writer::commit_value_end(RecordId id, Timestamp time, Bytes payload, std::uint8_t status) {
    auto& p = *impl_;
    auto& active = p.owner(id);
    p.types.validate(p.generators[active.value.generator].payload_type, payload,
                     p.native.string_count());
    active.value.payload = std::move(payload);
    active.payload_ready = true;
    p.end(id, time, status);
}

RecordId Writer::event(GeneratorId gen, RecordId owner, Timestamp time, View payload) {
    auto& p = *impl_;
    return p.call([&] {
        p.owner(owner);
        require(p.generators[gen].kind == RecordKind::event, "event requires EVENT generator");
        p.types.validate(p.generators[gen].payload_type, payload, p.native.string_count());
        auto id = p.allocate();
        p.submit(LocatedRecord{id, gen, time, time, owner, p.copy_payload(payload)});
        return id;
    });
}

RecordId Writer::log(GeneratorId gen, std::uint32_t datatype, Timestamp time, View payload,
                     RecordId owner) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        if (owner) {
            p.owner(owner);
        }
        require(p.generators[gen].kind == RecordKind::log, "log requires LOG generator");
        p.types.validate(p.generators.payload_type(gen, datatype), payload,
                         p.native.string_count());
        auto id = p.allocate();
        p.submit(LocatedRecord{id, gen, time, time, owner, p.copy_payload(payload), datatype});
        return id;
    });
}

RecordId Writer::begin_stage(GeneratorId gen, RecordId owner, Timestamp time) {
    auto& p = *impl_;
    return p.call([&] { return p.begin_stage(gen, owner, time, std::nullopt); });
}

RecordId Writer::begin_stage(GeneratorId gen, RecordId owner, Timestamp time, View payload) {
    auto& p = *impl_;
    return p.call([&] { return p.begin_stage(gen, owner, time, payload); });
}

void Writer::stage_payload(RecordId id, View payload) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        auto i = p.stages.find(id);
        require(i != p.stages.end(), "stage is not active");
        p.types.validate(p.generators[i->second.record.generator].payload_type, payload,
                         p.native.string_count());
        i->second.record.payload.assign(payload.begin(), payload.end());
        i->second.payload_ready = true;
    });
}

void Writer::end_stage(RecordId id, Timestamp time) {
    auto& p = *impl_;
    return p.call([&] { p.end_stage(id, time); });
}

RecordId Writer::embedded_event(RecordId id, Timestamp time, Id name,
                                std::span<const vtr::detail::wire::Attribute> attrs) {
    auto& p = *impl_;
    return p.call([&] {
        auto& owner = p.owner(id);
        p.key(name);
        p.attrs(attrs);
        auto event = p.allocate();
        owner.value.events.push_back({event, time, name, Attributes(attrs.begin(), attrs.end())});
        return event;
    });
}

RecordId Writer::embedded_stage(RecordId id, Id name, Id lane, Timestamp begin,
                                std::optional<Timestamp> end,
                                std::span<const vtr::detail::wire::Attribute> attrs) {
    auto& p = *impl_;
    return p.call([&] {
        auto& owner = p.owner(id);
        p.key(name);
        p.key(lane);
        p.attrs(attrs);
        require(!end || *end >= begin, "negative embedded stage duration");
        auto stage = p.allocate();
        owner.value.stages.push_back(
            {stage, name, lane, begin, end, Attributes(attrs.begin(), attrs.end())});
        return stage;
    });
}

void Writer::embedded_stage_end(RecordId id, RecordId stage, Timestamp time) {
    auto& p = *impl_;
    return p.call([&] {
        auto& owner = p.owner(id);
        auto i = std::find_if(owner.value.stages.begin(), owner.value.stages.end(),
                              [&](const auto& s) { return s.id == stage; });
        require(i != owner.value.stages.end(), "unknown embedded stage");
        i->end = std::max(i->begin, time);
    });
}

void Writer::relate(Id kind, RecordId source, RecordId destination,
                    std::span<const vtr::detail::wire::Attribute> attrs) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.key(kind);
        p.attrs(attrs);
        p.max_reference = std::max({p.max_reference, source, destination});
        const auto bytes = sizeof(Relation) + attr_bytes(attrs);
        if (bytes > p.options.block_bytes) {
            p.flush_transactions();
        }
        if (!p.transaction_number) {
            p.transaction_number = p.allocate_block();
        }
        p.transaction_bytes += bytes;
        p.counters.peak_transaction_buffer_bytes =
            std::max(p.counters.peak_transaction_buffer_bytes, p.transaction_bytes);
        p.relations.push_back({kind, source, destination, Attributes(attrs.begin(), attrs.end())});
        if (p.transaction_bytes >= p.options.block_bytes ||
            p.transactions.size() + p.relations.size() >= p.options.record_limit) {
            p.flush_transactions();
        }
    });
}

GeneratorId Writer::add_clock(Id parent, std::string_view name) {
    return add_generator(parent, name, "CLOCK", 0);
}

void Writer::associate_clock(GeneratorId source, GeneratorId clock) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.generators[source];
        require(p.generators[clock].kind == RecordKind::clock,
                "clock association requires a CLOCK generator");
        Value value{std::uint64_t(clock)};
        value.tag = Tag::u32;
        p.native.node_attr(source, p.native.intern("vtr.clock"), value);
    });
}

RecordId Writer::clock_run(GeneratorId gen, Timestamp first, Timestamp period) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        require(period != 0, "clock period is zero");
        auto i = p.clocks.find(gen);
        require(i != p.clocks.end(), "not a clock generator");
        if (i->second.id) {
            require(first > i->second.begin, "clock stretches share an edge");
            p.stop_clock(gen, first - 1, 0);
        }
        auto& clock = i->second;
        require(!clock.previous_edge || first > *clock.previous_edge,
                "clock stretches share an edge");
        clock.id = p.allocate();
        clock.begin = first;
        clock.period = period;
        return clock.id;
    });
}

void Writer::clock_stop(GeneratorId gen, Timestamp time) {
    auto& p = *impl_;
    return p.call([&] {
        p.alive();
        p.stop_clock(gen, time, 0);
    });
}

void Writer::flush() {
    auto& p = *impl_;
    return p.call([&] { p.flush(); });
}

void Writer::close() {
    auto& p = *impl_;
    return p.call([&] {
        if (p.closed) {
            return;
        }
        p.alive();
        require(p.max_reference <= p.counters.max_created_id,
                "unresolved relation endpoint at normal close");
        for (const auto& owner : p.owners) {
            if (owner.value.id) {
                require(owner.payload_ready, "missing mandatory payload at close");
            }
        }
        for (const auto& [id, owner] : p.overflow) {
            (void)id;
            require(owner.payload_ready, "missing mandatory payload at close");
        }
        for (const auto& [id, stage] : p.stages) {
            (void)id;
            require(stage.payload_ready, "missing mandatory stage payload at close");
        }
        std::vector<RecordId> ids;
        for (const auto& owner : p.owners) {
            if (owner.value.id) {
                ids.push_back(owner.value.id);
            }
        }
        for (const auto& [id, owner] : p.overflow) {
            (void)owner;
            ids.push_back(id);
        }
        std::sort(ids.begin(), ids.end());
        for (auto id : ids) {
            p.end(id, p.native.time(), 4);
        }
        std::vector<GeneratorId> clocks;
        for (const auto& [gen, clock] : p.clocks) {
            if (clock.id) {
                clocks.push_back(gen);
            }
        }
        for (auto gen : clocks) {
            p.stop_clock(gen, p.native.time(), 4);
        }
        p.critical([&] {
            p.flush();
            p.native.close_complete();
        });
        p.closed = true;
    });
}

void Writer::seal() {
    auto& p = *impl_;
    return p.call([&] {
        if (p.closed) {
            return;
        }
        if (p.poisoned) {
            p.native.abandon();
        } else {
            p.critical([&] {
                p.flush();
                p.native.close_incomplete();
            });
        }
        p.closed = true;
        p.owners.clear();
        p.overflow.clear();
        p.stages.clear();
        p.clocks.clear();
        p.owner_count = 0;
    });
}

HandoffStats Writer::handoff_stats() const {
    return impl_->native.handoff_stats();
}

WriterStats Writer::stats() const {
    const auto& p = *impl_;
    auto result = p.counters;
    const auto wave = p.native.wave_buffer_bytes();
    result.wave_buffer_bytes = wave.first;
    result.peak_wave_buffer_bytes = wave.second;
    result.payload_pool_bytes = (p.payload_pool ? p.payload_pool->charge() : 0) +
                                p.spare_payload_bytes + p.spare_payloads.capacity() * sizeof(Bytes);
    result.active_owners = p.owner_count;
    result.active_stages = p.stages.size();
    for (const auto& owner : p.owners) {
        if (owner.value.id) {
            result.active_payload_bytes += tx_bytes(owner.value) - sizeof(Transaction) +
                                           owner.numeric_count * sizeof(NumericAttribute);
        }
    }
    for (const auto& [id, owner] : p.overflow) {
        (void)id;
        result.active_payload_bytes += tx_bytes(owner.value) - sizeof(Transaction) +
                                       owner.numeric_count * sizeof(NumericAttribute);
    }
    for (const auto& [id, stage] : p.stages) {
        (void)id;
        result.active_payload_bytes += stage.record.payload.size();
    }
    return result;
}
} // namespace vtr::detail::engine
