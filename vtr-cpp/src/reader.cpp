#include "bridge.h"
#include "engine/activity.h"
#include "vtr_reader.h"

#include <algorithm>
#include <charconv>
#include <climits>
#include <cmath>
#include <format>
#include <locale>

namespace vtr {
namespace detail {

struct PayloadSchema {
    std::shared_ptr<const engine::Types> types;
    RecordingId origin;
    std::size_t allocation_limit;
};

struct PayloadStorage {
    std::shared_ptr<const PayloadSchema> schema;
    std::variant<wire::Bytes, std::shared_ptr<const void>> backing;
    engine::View bytes;
};

struct RecordStorage {
    RecordId id{0};
    RecordKind kind = RecordKind::transaction;
    std::optional<RecordId> parent;
    std::optional<GeneratorId> generator;
    Timestamp begin = 0;
    Timestamp end = 0;
    Status status = Status::unset;
    SpanKind span_kind = SpanKind::unspecified;
    TypeId type{0};
    PayloadStorage payload;
    Attributes attributes;
    std::vector<EmbeddedEvent> events;
    std::vector<EmbeddedStage> stages;
    std::vector<BlockId> child_blocks;
    std::optional<StringId> name;
    std::optional<StringId> lane;
    std::optional<Timestamp> explicit_end;
};

namespace {
void check_window(TimeWindow time) {
    if (time.first > time.last) {
        throw Error(ErrorCode::invalid_argument, "Time window begins after its end");
    }
}

void skip(binary::Cursor& cursor, const engine::Types& types, std::uint32_t id) {
    const auto& type = types[id];
    if (type.fixed_size) {
        cursor.take(*type.fixed_size);
    } else if (type.kind == engine::TypeKind::structure) {
        for (const auto& field : type.fields) {
            skip(cursor, types, field.type);
        }
    } else if (type.kind == engine::TypeKind::array || type.kind == engine::TypeKind::sequence) {
        auto count =
            type.kind == engine::TypeKind::array ? type.count : cursor.fixed<std::uint32_t>();
        const auto& element = types[type.element];
        if (element.fixed_size) {
            binary::require(!*element.fixed_size ||
                                count <= cursor.data.size() / *element.fixed_size,
                            "Truncated payload array");
            cursor.take(count * *element.fixed_size);
        } else {
            for (std::uint32_t i = 0; i < count; ++i) {
                skip(cursor, types, type.element);
            }
        }
    } else if (type.primitive == engine::Primitive::text ||
               type.primitive == engine::Primitive::bytes) {
        cursor.take(cursor.fixed<std::uint32_t>());
    } else {
        auto width = cursor.fixed<std::uint64_t>();
        auto per_byte = type.primitive == engine::Primitive::bits    ? 8u
                        : type.primitive == engine::Primitive::logic ? 4u
                                                                     : 2u;
        cursor.take(width / per_byte + (width % per_byte != 0));
    }
}

std::optional<RecordId> record_id(std::uint64_t id) {
    return id ? std::optional(RecordId(id)) : std::nullopt;
}

std::shared_ptr<RecordStorage> base_storage(
    std::uint64_t id, std::uint64_t parent, std::optional<std::uint32_t> generator,
    std::uint64_t begin, std::uint64_t end, engine::RecordKind kind, engine::TypeId datatype,
    wire::detail::View payload, std::span<const wire::Attribute> attributes,
    std::shared_ptr<const PayloadSchema> schema, std::optional<engine::RecordRef> retained = {}) {
    auto result = std::make_shared<RecordStorage>();
    result->id = RecordId(id);
    result->kind = record_kind(kind);
    result->parent = record_id(parent);
    if (generator) {
        result->generator = GeneratorId(*generator);
    }
    result->type = TypeId(datatype);
    result->begin = begin;
    result->end = end;
    result->payload.schema = std::move(schema);
    if (retained) {
        result->payload.backing = retained->retained_storage();
        result->payload.bytes = payload;
    } else {
        auto& owned = result->payload.backing.emplace<wire::Bytes>(payload.begin(), payload.end());
        result->payload.bytes = owned;
    }
    result->attributes = decode_attributes(attributes);
    return result;
}

void owner_detail(RecordStorage& result, const engine::Transaction& owner) {
    result.status = static_cast<Status>(owner.status);
    result.span_kind = static_cast<SpanKind>(owner.span_kind);
    for (const auto& event : owner.events) {
        result.events.push_back({RecordId(event.id), event.time, StringId(event.name),
                                 decode_attributes(event.attributes)});
    }
    for (const auto& stage : owner.stages) {
        result.stages.push_back({RecordId(stage.id), StringId(stage.name), StringId(stage.lane),
                                 stage.begin, stage.end, decode_attributes(stage.attributes)});
    }
    for (auto block : owner.child_blocks) {
        result.child_blocks.push_back(BlockId(block));
    }
}

std::shared_ptr<RecordStorage> record_storage(const engine::Transaction& tx,
                                              std::shared_ptr<const PayloadSchema> schema,
                                              const engine::ContainerReader& container,
                                              std::optional<engine::RecordRef> retained = {}) {
    const auto& generator = container.generators()[tx.generator];
    auto result = base_storage(tx.id, tx.parent, tx.generator, tx.begin, tx.end, generator.kind,
                               generator.payload_type, tx.payload, tx.attributes, std::move(schema),
                               std::move(retained));
    owner_detail(*result, tx);
    return result;
}

std::shared_ptr<RecordStorage> record_storage(const engine::LocatedRecord& record,
                                              std::shared_ptr<const PayloadSchema> schema,
                                              const engine::ContainerReader& container,
                                              std::optional<engine::RecordRef> retained = {}) {
    const auto& generator = container.generators()[record.generator];
    const auto datatype = record.datatype().value_or(generator.payload_type);
    auto result = base_storage(record.id, record.parent, record.generator, record.begin, record.end,
                               generator.kind, datatype, record.payload, {}, std::move(schema),
                               std::move(retained));
    return result;
}

std::shared_ptr<RecordStorage> record_storage(const engine::RecordRef& record,
                                              std::shared_ptr<const PayloadSchema> schema,
                                              const engine::ContainerReader& container) {
    if (auto owner = record.transaction()) {
        return record_storage(*owner, std::move(schema), container, record);
    }
    if (auto located = record.located()) {
        return record_storage(*located, std::move(schema), container, record);
    }
    auto result = base_storage(record.id(), record.parent(), record.generator(), record.begin(),
                               record.end(), record.kind(), 0, record.payload(),
                               record.attributes(), std::move(schema), record);
    if (auto event = record.event()) {
        result->name = StringId(event->name);
    } else if (auto stage = record.stage()) {
        result->name = StringId(stage->name);
        result->lane = StringId(stage->lane);
        result->explicit_end = stage->end;
    }
    return result;
}

} // namespace
} // namespace detail

PayloadView::PayloadView(std::shared_ptr<const detail::PayloadStorage> storage, std::size_t offset,
                         TypeId type) noexcept
    : storage_(std::move(storage)), offset_(offset), type_(type) {}

TypeId PayloadView::type() const noexcept {
    return type_;
}

PayloadView PayloadView::first_element() const {
    return detail::translate([&] {
        const auto& type = (*storage_->schema->types)[type_.value];
        auto offset = offset_ + (type.kind == detail::engine::TypeKind::sequence ? 4 : 0);
        return PayloadView(storage_, offset, TypeId(type.element));
    });
}

PayloadView PayloadView::next_element() const {
    return detail::translate([&] {
        auto data = detail::engine::View(storage_->bytes);
        detail::binary::Cursor cursor(data.subspan(offset_));
        detail::skip(cursor, *storage_->schema->types, type_.value);
        return PayloadView(storage_, data.size() - cursor.data.size(), type_);
    });
}

bool PayloadView::matches(detail::PayloadKind kind, std::string_view name) const {
    return detail::translate([&] {
        const auto& type = (*storage_->schema->types)[type_.value];
        return static_cast<detail::PayloadKind>(type.kind) == kind &&
               (name.empty() || type.name == name);
    });
}

std::optional<ByteView> PayloadView::fixed_array(TypeId element, std::size_t count) const {
    return detail::translate([&]() -> std::optional<ByteView> {
        const auto& type = (*storage_->schema->types)[type_.value];
        if (type.kind != detail::engine::TypeKind::array || type.count != count ||
            type.element != element.value || !type.fixed_size) {
            return std::nullopt;
        }
        detail::binary::Cursor cursor(detail::engine::View(storage_->bytes).subspan(offset_));
        return std::as_bytes(cursor.take(*type.fixed_size));
    });
}

std::size_t PayloadView::allocation_limit() const noexcept {
    return storage_->schema->allocation_limit;
}

std::size_t PayloadView::size() const {
    return detail::translate([&]() -> std::size_t {
        const auto& type = (*storage_->schema->types)[type_.value];
        switch (type.kind) {
        case detail::engine::TypeKind::structure:
            return type.fields.size();
        case detail::engine::TypeKind::array:
            return type.count;
        case detail::engine::TypeKind::sequence: {
            detail::binary::Cursor cursor(detail::engine::View(storage_->bytes).subspan(offset_));
            return cursor.fixed<std::uint32_t>();
        }
        default:
            return 0;
        }
    });
}

PayloadView PayloadView::operator[](std::string_view field) const {
    return detail::translate([&] {
        const auto& type = (*storage_->schema->types)[type_.value];
        if (type.kind != detail::engine::TypeKind::structure) {
            throw Error(ErrorCode::invalid_argument, "Named payload access requires a struct");
        }
        auto found = std::ranges::find(type.fields, field, &detail::engine::Field::name);
        if (found == type.fields.end()) {
            throw Error(ErrorCode::invalid_argument,
                        "Unknown payload field: " + std::string(field));
        }
        return (*this)[static_cast<std::size_t>(found - type.fields.begin())];
    });
}

PayloadView PayloadView::operator[](std::size_t index) const {
    return detail::translate([&] {
        const auto& type = (*storage_->schema->types)[type_.value];
        if (index >= size()) {
            throw Error(ErrorCode::invalid_argument, "Payload element index is out of range");
        }
        auto data = detail::engine::View(storage_->bytes);
        detail::binary::Cursor cursor(data.subspan(offset_));
        std::uint32_t element;
        if (type.kind == detail::engine::TypeKind::structure) {
            for (std::size_t i = 0; i < index; ++i) {
                detail::skip(cursor, *storage_->schema->types, type.fields[i].type);
            }
            element = type.fields[index].type;
        } else {
            element = type.element;
            if (type.kind == detail::engine::TypeKind::sequence) {
                cursor.fixed<std::uint32_t>();
            }
            const auto& item = (*storage_->schema->types)[element];
            if (item.fixed_size) {
                cursor.take(index * *item.fixed_size);
            } else {
                for (std::size_t i = 0; i < index; ++i) {
                    detail::skip(cursor, *storage_->schema->types, element);
                }
            }
        }
        return PayloadView(storage_, data.size() - cursor.data.size(), TypeId(element));
    });
}

Value PayloadView::scalar() const {
    return detail::translate([&]() -> Value {
        const auto& type = (*storage_->schema->types)[type_.value];
        if (type.kind != detail::engine::TypeKind::scalar &&
            type.kind != detail::engine::TypeKind::enumeration) {
            throw Error(ErrorCode::invalid_argument, "Aggregate payload has no scalar value");
        }
        auto primitive = type.kind == detail::engine::TypeKind::enumeration
                             ? (*storage_->schema->types)[type.element].primitive
                             : type.primitive;
        detail::binary::Cursor cursor(detail::engine::View(storage_->bytes).subspan(offset_));
        using Primitive = detail::engine::Primitive;
        switch (primitive) {
        case Primitive::unit:
            return {};
        case Primitive::boolean:
            return Value(cursor.fixed<std::uint8_t>() != 0);
        case Primitive::i8:
            return Value(std::bit_cast<std::int8_t>(cursor.fixed<std::uint8_t>()));
        case Primitive::i16:
            return Value(std::bit_cast<std::int16_t>(cursor.fixed<std::uint16_t>()));
        case Primitive::i32:
            return Value(std::bit_cast<std::int32_t>(cursor.fixed<std::uint32_t>()));
        case Primitive::i64:
            return Value(std::bit_cast<std::int64_t>(cursor.fixed<std::uint64_t>()));
        case Primitive::u8:
            return Value(cursor.fixed<std::uint8_t>());
        case Primitive::u16:
            return Value(cursor.fixed<std::uint16_t>());
        case Primitive::u32:
            return Value(cursor.fixed<std::uint32_t>());
        case Primitive::u64:
            return Value(cursor.fixed<std::uint64_t>());
        case Primitive::f32:
            return Value(std::bit_cast<float>(cursor.fixed<std::uint32_t>()));
        case Primitive::f64:
            return Value(std::bit_cast<double>(cursor.fixed<std::uint64_t>()));
        case Primitive::str:
            return Value(StringId(cursor.fixed<std::uint32_t>()));
        case Primitive::time:
            return Value(TimeValue{cursor.fixed<std::uint64_t>()});
        case Primitive::pointer:
            return Value(Address{cursor.fixed<std::uint64_t>()});
        case Primitive::text: {
            auto text = cursor.take(cursor.fixed<std::uint32_t>());
            return Value(std::string(reinterpret_cast<const char*>(text.data()), text.size()));
        }
        case Primitive::bytes:
            return Value(detail::bytes(cursor.take(cursor.fixed<std::uint32_t>())));
        case Primitive::bits:
        case Primitive::logic:
        case Primitive::logic9: {
            auto width = cursor.fixed<std::uint64_t>();
            auto per_byte = primitive == Primitive::bits    ? 8u
                            : primitive == Primitive::logic ? 4u
                                                            : 2u;
            auto packed = detail::bytes(cursor.take(width / per_byte + (width % per_byte != 0)));
            if (primitive == Primitive::bits) {
                return Value(Bits{width, std::move(packed)});
            }
            if (primitive == Primitive::logic) {
                return Value(Logic{width, std::move(packed)});
            }
            return Value(Logic9{width, std::move(packed)});
        }
        case Primitive::fixed: {
            auto raw = std::bit_cast<std::int64_t>(cursor.fixed<std::uint64_t>());
            return Value(Fixed{raw, std::bit_cast<std::int64_t>(cursor.fixed<std::uint64_t>())});
        }
        case Primitive::ufixed: {
            auto raw = cursor.fixed<std::uint64_t>();
            return Value(UFixed{raw, std::bit_cast<std::int64_t>(cursor.fixed<std::uint64_t>())});
        }
        default:
            throw Error(ErrorCode::corrupt_data, "Invalid primitive payload type");
        }
    });
}

Record::Record(std::shared_ptr<const detail::RecordStorage> storage)
    : storage_(std::move(storage)) {}

RecordId Record::id() const noexcept {
    return storage_->id;
}

RecordKind Record::kind() const noexcept {
    return storage_->kind;
}

std::optional<RecordId> Record::parent() const noexcept {
    return storage_->parent;
}

TypeId Record::datatype() const noexcept {
    return storage_->type;
}

std::optional<GeneratorId> Record::generator() const noexcept {
    return storage_->generator;
}

Timestamp Record::begin() const noexcept {
    return storage_->begin;
}

Timestamp Record::end() const noexcept {
    return storage_->end;
}

vtr::Status Record::status() const noexcept {
    return storage_->status;
}

vtr::SpanKind Record::span_kind() const noexcept {
    return storage_->span_kind;
}

PayloadView Record::payload() const {
    return PayloadView(std::shared_ptr<const detail::PayloadStorage>(storage_, &storage_->payload),
                       0, storage_->type);
}

AttributeView Record::attributes() const noexcept {
    return storage_->attributes;
}

std::span<const EmbeddedEvent> Record::events() const noexcept {
    return storage_->events;
}

std::span<const EmbeddedStage> Record::stages() const noexcept {
    return storage_->stages;
}

std::span<const BlockId> Record::child_blocks() const noexcept {
    return storage_->child_blocks;
}

std::optional<StringId> Record::name() const noexcept {
    return storage_->name;
}

std::optional<StringId> Record::lane() const noexcept {
    return storage_->lane;
}

std::optional<Timestamp> Record::explicit_end() const noexcept {
    return storage_->explicit_end;
}

struct Reader::Impl {
    detail::engine::Reader backend;
    std::shared_ptr<const detail::PayloadSchema> schema;
    vtr::Metadata metadata;
    std::vector<Node> nodes;
    std::vector<Generator> generators;
    std::vector<DataType> types;
    std::vector<SignalKind> signals;
    Provenance ending;

    Impl(const std::filesystem::path& path, ReaderOptions options)
        : backend(path.string(), {options.verify_checksums, options.owner_cache_entries,
                                  options.owner_cache_bytes, options.max_payload_bytes}),
          schema(std::make_shared<detail::PayloadSchema>(backend.container().retained_types(),
                                                         detail::identity(backend.trace_id()),
                                                         options.max_payload_bytes)),
          metadata(detail::decode_metadata(backend.metadata())) {
        auto fingerprint = backend.fingerprint();
        auto dropped = backend.container().waveform().stats().recovered_dropped;
        ending = {fingerprint
                      ? (backend.incomplete() ? Completion::incomplete : Completion::complete)
                      : Completion::unfinished,
                  dropped};
        types.reserve(schema->types->size());
        for (std::uint32_t i = 0; i < schema->types->size(); ++i) {
            if (i == static_cast<std::uint32_t>(Primitive::enumeration)) {
                types.push_back({TypeId(i), {}, Primitive::enumeration});
            } else {
                types.push_back(
                    detail::decode_type(i, (*schema->types)[i], backend.types().attributes(i)));
            }
        }
        signals.reserve(backend.signals().size());
        for (auto kind : backend.signals()) {
            signals.push_back(detail::signal_kind(kind));
        }
        auto declarations = backend.nodes();
        nodes.reserve(declarations.size());
        generators.reserve(backend.container().generators().size());
        for (std::uint32_t i = 0; i < declarations.size(); ++i) {
            const auto& node = declarations[i];
            auto kind = node.kind == detail::wire::Node::scope        ? NodeKind::scope
                        : node.kind == detail::wire::Node::generator  ? NodeKind::generator
                        : node.kind == detail::wire::Node::enum_table ? NodeKind::enum_table
                                                                      : NodeKind::signal;
            nodes.push_back(
                {NodeId(i),
                 kind,
                 StringId(node.name),
                 node.parent == detail::wire::none ? std::nullopt
                                                   : std::optional(ScopeId(node.parent)),
                 node.signal == detail::wire::none ? std::nullopt
                                                   : std::optional(SignalId(node.signal)),
                 node.alias,
                 detail::decode_attributes(node.attrs),
                 {},
                 {},
                 {},
                 {}});
            auto& declared = nodes.back();
            if (node.kind == detail::wire::Node::scope) {
                declared.scope_kind = static_cast<ScopeKind>(node.type);
                declared.component = StringId(node.aux);
            } else if (node.kind == detail::wire::Node::variable) {
                declared.variable =
                    VariableDeclaration{static_cast<VariableType>(node.type),
                                        static_cast<VariableDirection>(node.direction)};
            } else if (node.kind == detail::wire::Node::enum_table) {
                for (const auto& [name, value] : node.entries)
                    declared.enum_entries.push_back({StringId(name), StringId(value)});
            }
            if (node.kind == detail::wire::Node::generator) {
                auto id = GeneratorId(i);
                auto record_kind = detail::record_kind(backend.container().generators()[i].kind);
                auto clock = backend.clock_for(i);
                generators.push_back({id, record_kind, StringId(node.aux),
                                      TypeId(node.payload_type),
                                      clock ? std::optional(GeneratorId(*clock)) : std::nullopt});
            }
        }
    }

    template <class T> Record retain(const T& ref) const {
        return Record(detail::record_storage(ref, schema, backend.container()));
    }
};

Reader::Reader(const std::filesystem::path& path, ReaderOptions options)
    : impl_(detail::translate([&] { return std::make_unique<Impl>(path, options); })) {}

Reader::~Reader() = default;
Reader::Reader(Reader&&) noexcept = default;
Reader& Reader::operator=(Reader&&) noexcept = default;

RecordingId Reader::identity() const noexcept {
    return detail::identity(impl_->backend.trace_id());
}

std::optional<vtr::Fingerprint> Reader::fingerprint() const noexcept {
    auto value = impl_->backend.fingerprint();
    return value ? std::optional(detail::fingerprint(*value)) : std::nullopt;
}

Provenance Reader::ending() const noexcept {
    return impl_->ending;
}

const vtr::Metadata& Reader::metadata() const noexcept {
    return impl_->metadata;
}

std::span<const Node> Reader::nodes() const noexcept {
    return impl_->nodes;
}

std::span<const Generator> Reader::generators() const noexcept {
    return impl_->generators;
}

std::span<const DataType> Reader::types() const noexcept {
    return impl_->types;
}

std::span<const SignalKind> Reader::signals() const noexcept {
    return impl_->signals;
}

std::string_view Reader::string(StringId id) const {
    return detail::translate([&] { return impl_->backend.string(id.value); });
}

std::optional<NodeId> Reader::find(std::string_view path) const {
    return detail::translate([&]() -> std::optional<NodeId> {
        auto id = impl_->backend.find_node(path);
        return id == detail::wire::none ? std::nullopt : std::optional(NodeId(id));
    });
}

std::optional<SignalId> Reader::find_signal(std::string_view path) const {
    return detail::translate([&]() -> std::optional<SignalId> {
        auto id = impl_->backend.find_signal(path);
        return id == detail::wire::none ? std::nullopt : std::optional(SignalId(id));
    });
}

std::optional<GeneratorId> Reader::find_generator(std::string_view path) const {
    return detail::translate([&]() -> std::optional<GeneratorId> {
        auto id = impl_->backend.find_generator(path);
        return id == detail::wire::none ? std::nullopt : std::optional(GeneratorId(id));
    });
}

std::optional<Record> Reader::lookup(RecordId id) const {
    return detail::translate([&]() -> std::optional<Record> {
        auto ref = impl_->backend.lookup(id.value);
        return ref ? std::optional(impl_->retain(*ref)) : std::nullopt;
    });
}

void Reader::transactions(TransactionQuery query,
                          const std::function<Flow(const Record&)>& visit) const {
    detail::check_window(query.time);
    detail::translate([&] {
        detail::engine::TransactionQuery selected;
        if (query.generator) {
            selected.generator = query.generator->value;
        }
        if (query.parent) {
            selected.parent = query.parent->value;
        }
        selected.begin = query.time.first;
        selected.end = query.time.last;
        impl_->backend.transactions(
            selected, [&](const auto& tx) { return visit(impl_->retain(tx)) == Flow::next; });
    });
}

void Reader::records(RecordQuery query, const std::function<Flow(const Record&)>& visit) const {
    detail::check_window(query.time);
    detail::translate([&] {
        detail::engine::RecordQuery selected;
        if (query.generator) {
            selected.generator = query.generator->value;
        }
        if (query.datatype)
            selected.datatype = query.datatype->value;
        if (query.parent) {
            selected.parent = query.parent->value;
        }
        if (query.kind) {
            switch (*query.kind) {
            case RecordKind::stage:
                selected.kind = detail::engine::RecordKind::stage;
                break;
            case RecordKind::event:
                selected.kind = detail::engine::RecordKind::event;
                break;
            case RecordKind::log:
                selected.kind = detail::engine::RecordKind::log;
                break;
            default:
                return;
            }
        }
        selected.begin = query.time.first;
        selected.end = query.time.last;
        impl_->backend.records(selected, [&](const auto& record) {
            return visit(impl_->retain(record)) == Flow::next;
        });
    });
}

void Reader::relations(RecordId endpoint, Direction direction,
                       const std::function<Flow(const Relation&)>& visit) const {
    detail::translate([&] {
        impl_->backend.relations(
            endpoint.value, direction == Direction::incoming, [&](const auto& relation) {
                return visit({StringId(relation.kind), detail::record_id(relation.source),
                              detail::record_id(relation.destination),
                              detail::decode_attributes(relation.attributes)}) == Flow::next;
            });
    });
}

Expansion Reader::expand(RecordId owner) const {
    return detail::translate([&] {
        auto expanded = impl_->backend.expand(owner.value);
        if (!expanded.owner) {
            throw Error(ErrorCode::invalid_argument, "Expansion owner does not exist");
        }
        Expansion result{impl_->retain(*expanded.owner), {}, {}, expanded.complete};
        for (const auto& record : expanded.located) {
            result.located.push_back(impl_->retain(record));
        }
        for (auto block : expanded.missing_blocks) {
            result.missing_blocks.push_back(BlockId(block));
        }
        return result;
    });
}

void Reader::children(RecordId parent, const std::function<Flow(const Record&)>& visit) const {
    transactions({.parent = parent}, [&](const auto& record) {
        return record.kind() == RecordKind::transaction ? visit(record) : Flow::next;
    });
}

void Reader::logs(RecordId parent, TimeWindow time,
                  const std::function<Flow(const Record&)>& visit) const {
    records({.parent = parent, .kind = RecordKind::log, .time = time}, visit);
}

std::optional<std::uint64_t> Reader::cycle_at(GeneratorId clock, Timestamp time) const {
    return detail::translate([&] { return impl_->backend.cycle_at(clock.value, time); });
}

void Reader::changes(std::optional<SignalId> signal, TimeWindow time,
                     const std::function<Flow(const SignalChange&)>& visit) const {
    detail::check_window(time);
    detail::translate([&] {
        SignalChange change{0, SignalId(UINT32_MAX), {SignalKind{}, {}}};
        auto callback = [&](auto timestamp, auto id, const auto& value) {
            change.time = timestamp;
            if (change.signal.value != id) {
                change.signal = SignalId(id);
                // Avoid per-sample declaration writes before dependent value accessors.
                change.value.kind = impl_->signals[id];
            }
            change.value.packed = std::as_bytes(value.data);
            return visit(change) == Flow::next;
        };
        if (signal) {
            impl_->backend.changes(signal->value, time.first, time.last, callback);
        } else {
            impl_->backend.changes(time.first, time.last, callback);
        }
    });
}

SignalSample Reader::value_at(SignalId signal, Timestamp time) const {
    return detail::translate([&] {
        if (signal.value >= impl_->signals.size()) {
            throw Error(ErrorCode::invalid_argument, "Unknown signal");
        }
        return SignalSample{impl_->signals[signal.value],
                            detail::bytes(impl_->backend.value_at(signal.value, time))};
    });
}

std::uint64_t SignalSample::as_u64() const {
    return SignalView{kind, packed}.as_u64();
}

std::uint64_t SignalView::checked_u64() const {
    return detail::translate([&] {
        return detail::wire::SignalValue{detail::signal_kind(kind), detail::view(packed)}.as_u64();
    });
}

std::optional<TimeWindow> Reader::waveform_time_range() const noexcept {
    const auto& range = impl_->backend.container().waveform().stats().time_range;
    return range ? std::optional(TimeWindow{range->first, range->second}) : std::nullopt;
}

std::optional<TimeWindow> Reader::time_range() const {
    return detail::translate([&] {
        auto range = waveform_time_range();
        const auto& file = impl_->backend.container();
        for (const auto& section : file.sections()) {
            std::optional<std::pair<Timestamp, Timestamp>> next;
            if (section.kind == 5) {
                file.section_data(section);
                next = file.transaction_block(section.block_number)->time_range();
            } else if (section.kind == 9) {
                next = {section.aux0, section.aux1};
            }
            if (next) {
                range = range ? TimeWindow{std::min(range->first, next->first),
                                           std::max(range->last, next->second)}
                              : TimeWindow{next->first, next->second};
            }
        }
        return range;
    });
}

double SignalSample::as_real() const {
    return SignalView{kind, packed}.as_real();
}

double SignalView::as_real() const {
    return detail::translate([&] {
        return detail::wire::SignalValue{detail::signal_kind(kind), detail::view(packed)}.as_real();
    });
}

std::string SignalSample::as_text() const {
    return SignalView{kind, packed}.as_text();
}

std::string SignalView::as_text() const {
    return detail::translate([&] {
        return detail::wire::SignalValue{detail::signal_kind(kind), detail::view(packed)}.ascii();
    });
}

ValidationReport Reader::validate() const {
    return detail::translate([&] {
        auto backend = impl_->backend.validate();
        ValidationReport result{impl_->ending, backend.records, backend.relations, {}};
        for (const auto& missing : backend.missing) {
            auto kind = static_cast<MissingKind>(missing.kind);
            std::variant<RecordId, BlockId, GeneratorId> target = RecordId(missing.target);
            if (kind == MissingKind::child_block) {
                target = BlockId(missing.target);
            } else if (kind == MissingKind::clock) {
                target = GeneratorId(static_cast<std::uint32_t>(missing.target));
            }
            result.missing.push_back({kind, detail::record_id(missing.referrer), target});
        }
        return result;
    });
}

vtr::CacheStats Reader::cache_stats() const noexcept {
    auto stats = impl_->backend.cache_stats();
    return {stats.owner_entries, stats.accounted_bytes};
}

void Reader::clear_cache() const {
    impl_->backend.clear_cache();
}

Provenance recover(const std::filesystem::path& source, const std::filesystem::path& destination) {
    return detail::translate([&] {
        if (source == destination || (std::filesystem::exists(destination) &&
                                      std::filesystem::equivalent(source, destination))) {
            throw Error(ErrorCode::invalid_argument, "Recovery source and destination must differ");
        }
        detail::engine::recover_container(source.string(), destination.string());
        return Reader(destination).ending();
    });
}

namespace {
class FormatPunctuation : public std::numpunct<char> {
    std::locale source_;
    std::string groups_;

    const std::numpunct<char>& source() const {
        return std::use_facet<std::numpunct<char>>(source_);
    }

  public:
    FormatPunctuation(std::locale source, std::string groups)
        : source_(std::move(source)), groups_(std::move(groups)) {}

  private:
    std::string do_grouping() const override {
        return groups_;
    }

    char do_decimal_point() const override {
        return source().decimal_point();
    }

    char do_thousands_sep() const override {
        return source().thousands_sep();
    }

    std::string do_truename() const override {
        return source().truename();
    }

    std::string do_falsename() const override {
        return source().falsename();
    }
};

std::locale format_locale(std::string_view spec) {
    auto locale = std::locale();
    if (spec.find('L') == spec.npos)
        return locale;
    auto groups = std::use_facet<std::numpunct<char>>(locale).grouping();
    const auto stop =
        std::find_if(groups.begin(), groups.end(), [](char size) { return size <= 0; });
    if (stop == groups.end())
        return locale;
    const auto end = static_cast<std::size_t>(stop - groups.begin());
    // libc++ can loop on zero/negative groups; CHAR_MAX has the same unlimited-group meaning.
    groups.resize(end + 1);
    groups[end] = CHAR_MAX;
    return std::locale(locale, new FormatPunctuation(locale, std::move(groups)));
}

template <std::floating_point T>
std::optional<std::string> alternate_fixed_zero(T value, std::string_view spec) {
    const auto dot = spec.rfind('.');
    const auto hash = spec.rfind('#');
    if (!std::isfinite(value) || dot == spec.npos || hash == spec.npos || hash > dot ||
        (spec.back() != 'f' && spec.back() != 'F'))
        return {};
    const bool localized = spec.size() > 1 && spec[spec.size() - 2] == 'L';
    const auto precision = spec.substr(dot + 1, spec.size() - dot - (localized ? 3 : 2));
    if (precision.empty() || precision.find_first_not_of('0') != precision.npos)
        return {};
    const auto prefix = spec.substr(0, hash);
    const auto alignment = spec.substr(0, dot).find_last_of("<^>");
    if (alignment != spec.npos && hash < alignment)
        return {};

    std::formatter<T> validator;
    std::format_parse_context context(spec);
    if (validator.parse(context) != context.end())
        throw std::format_error("Invalid numeric format");

    // libstdc++ PR120967 corrupts large alternate fixed values at zero precision.
    // Format the rounded integer without alternate form, then add its radix point.
    std::string sign;
    if (!prefix.empty() && (prefix.back() == '+' || prefix.back() == '-' || prefix.back() == ' '))
        sign = prefix.back();
    const auto locale = format_locale(spec);
    auto body = std::vformat(locale, "{:" + sign + std::string(spec.substr(dot)) + "}",
                             std::make_format_args(value));
    body += localized ? std::use_facet<std::numpunct<char>>(locale).decimal_point() : '.';
    const auto width_text = spec.substr(hash + 1, dot - hash - 1);
    std::size_t width = 0;
    if (!width_text.empty())
        std::from_chars(width_text.data(), width_text.data() + width_text.size(), width);
    const auto padding = width > body.size() ? width - body.size() : 0;
    if (alignment == prefix.npos && !width_text.empty() && width_text.front() == '0') {
        const auto sign_size =
            !body.empty() && (body.front() == '+' || body.front() == '-' || body.front() == ' ');
        body.insert(sign_size, padding, '0');
        return body;
    }
    const char align = alignment == prefix.npos ? '>' : prefix[alignment];
    const auto fill = alignment == prefix.npos || alignment == 0 ? std::string_view(" ")
                                                                 : prefix.substr(0, alignment);
    const auto left = align == '<' ? 0 : align == '^' ? padding / 2 : padding;
    std::string result;
    for (std::size_t i = 0; i < left; ++i)
        result += fill;
    result += body;
    for (std::size_t i = left; i < padding; ++i)
        result += fill;
    return result;
}

template <class T> std::string format_scalar(T value, std::string_view spec) {
    // Specifications come from recordings; bound expansion before invoking std::format.
    unsigned digits = 0;
    for (unsigned char character : spec) {
        digits = character >= '0' && character <= '9' ? digits * 10 + character - '0' : 0;
        if (digits > 65536)
            return "{?}";
    }
    std::string format = "{:";
    format += spec;
    format += '}';
    try {
        if constexpr (std::floating_point<T>) {
            if (auto fixed = alternate_fixed_zero(value, spec))
                return *fixed;
        }
        return std::vformat(format_locale(spec), format, std::make_format_args(value));
    } catch (const std::format_error&) {
        return "{?}";
    }
}

std::string render_payload(const PayloadView& payload, const Reader& reader,
                           std::string_view spec) {
    const auto& type = reader.types()[payload.type().value];
    if (!std::holds_alternative<Primitive>(type.definition) &&
        !std::holds_alternative<EnumDefinition>(type.definition)) {
        return "{?}";
    }
    auto scalar = payload.scalar();
    return std::visit(
        [&]<class T>(const T& item) -> std::string {
            if constexpr (std::same_as<T, std::monostate>) {
                return format_scalar(std::string_view("null"), spec);
            } else if constexpr (std::integral<T> || std::floating_point<T> ||
                                 std::same_as<T, std::string>) {
                return format_scalar(item, spec);
            } else if constexpr (std::same_as<T, StringId>) {
                return format_scalar(reader.string(item), spec);
            } else if constexpr (std::same_as<T, TimeValue> || std::same_as<T, Address>) {
                return format_scalar(item.value, spec);
            } else if constexpr (std::same_as<T, Fixed> || std::same_as<T, UFixed>) {
                return format_scalar(std::to_string(item.raw) + "e" + std::to_string(item.scale),
                                     spec);
            } else if constexpr (std::same_as<T, Bytes>) {
                std::string text;
                for (auto byte : item) {
                    text += std::format("{:02x}", std::to_integer<unsigned>(byte));
                }
                return format_scalar(text, spec);
            } else if constexpr (std::same_as<T, Bits> || std::same_as<T, Logic> ||
                                 std::same_as<T, Logic9>) {
                if (item.width > UINT32_MAX) {
                    return "{?}";
                }
                SignalSample sample{
                    {SignalEncoding::bits, static_cast<std::uint32_t>(item.width), T::alphabet},
                    item.packed};
                return format_scalar(sample.as_text(), spec);
            } else {
                return "{?}";
            }
        },
        scalar.data);
}
} // namespace

std::string Reader::format_arguments(std::string_view format, const PayloadView& payload) const {
    if (payload.storage_->schema->origin != identity())
        throw Error(ErrorCode::wrong_recording, "Payload belongs to another recording");
    const auto& type = types()[payload.type().value];
    bool structure = std::holds_alternative<StructDefinition>(type.definition);
    auto count = structure ? payload.size() : payload.type().value == 0 ? 0u : 1u;
    std::string result;
    std::size_t automatic = 0;
    for (std::size_t i = 0; i < format.size();) {
        char current = format[i++];
        if (i < format.size() && (current == '{' || current == '}') && format[i] == current) {
            result += current;
            ++i;
        } else if (current != '{') {
            result += current;
        } else {
            auto end = format.find('}', i);
            if (end == std::string_view::npos) {
                result += format.substr(i - 1);
                break;
            }
            auto field = format.substr(i, end - i);
            auto colon = field.find(':');
            auto index_text = field.substr(0, colon);
            auto spec =
                colon == std::string_view::npos ? std::string_view{} : field.substr(colon + 1);
            std::size_t index = 0;
            bool valid = true;
            if (index_text.empty()) {
                index = automatic++;
            } else {
                auto parsed = std::from_chars(index_text.data(),
                                              index_text.data() + index_text.size(), index);
                valid =
                    parsed.ec == std::errc{} && parsed.ptr == index_text.data() + index_text.size();
            }
            result += valid && index < count
                          ? render_payload(structure ? payload[index] : payload, *this, spec)
                          : "{?}";
            i = end + 1;
        }
    }
    return result;
}

struct ActivityIndex::Impl {
    detail::engine::ActivityIndex backend;
    Bytes encoded;
};

ActivityIndex::ActivityIndex(std::shared_ptr<const Impl> storage) noexcept
    : impl_(std::move(storage)) {}

ActivityIndex ActivityIndex::build(const Reader& trace, std::size_t memory_bytes) {
    return detail::translate([&] {
        const auto& container = trace.impl_->backend.container();
        auto backend = detail::engine::ActivityIndex::build(container, memory_bytes);
        auto encoded = detail::bytes(backend.encode(container));
        return ActivityIndex(std::make_shared<Impl>(std::move(backend), std::move(encoded)));
    });
}

ActivityIndex ActivityIndex::load(const Reader& trace, ByteView sidecar, std::size_t memory_bytes) {
    return detail::translate([&] {
        auto backend = detail::engine::ActivityIndex::decode(trace.impl_->backend.container(),
                                                             detail::view(sidecar), memory_bytes);
        return ActivityIndex(
            std::make_shared<Impl>(std::move(backend), Bytes(sidecar.begin(), sidecar.end())));
    });
}

Bytes ActivityIndex::encode() const {
    return impl_->encoded;
}

vtr::Fingerprint ActivityIndex::fingerprint() const noexcept {
    return detail::fingerprint(impl_->backend.fingerprint());
}

std::vector<SignalId> ActivityIndex::active(TimeWindow time) const {
    detail::check_window(time);
    return detail::translate([&] {
        std::vector<SignalId> result;
        for (auto id : impl_->backend.active(time.first, time.last)) {
            result.push_back(SignalId(id));
        }
        return result;
    });
}

} // namespace vtr
