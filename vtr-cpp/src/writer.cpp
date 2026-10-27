#include "bridge.h"
#include "stream.h"
#include "vtr_writer.h"

#include <atomic>
#include <limits>
#include <unordered_map>

namespace vtr {
namespace detail {

void BoundedPayloadBuffer::size_changed() {
    throw Error(ErrorCode::invalid_argument, "Payload changed size during capture");
}

namespace {
std::uint64_t next_recording_origin() {
    static std::atomic<std::uint64_t> next{1};
    auto value = next.load(std::memory_order_relaxed);
    for (;;) {
        if (value == std::numeric_limits<std::uint64_t>::max())
            throw Error(ErrorCode::resource_limit, "Recording identities exhausted");
        if (next.compare_exchange_weak(value, value + 1, std::memory_order_relaxed))
            return value;
    }
}
} // namespace

struct RecordingContext {
    std::unique_ptr<engine::Writer> direct;
    std::unique_ptr<engine::Stream> stream;
    Timestamp current_time = 0;
    bool finished = false, closed = false;
    // Allocator address reuse must not make handles from a previous recording valid.
    const std::uint64_t origin = next_recording_origin();

    ~RecordingContext() {
        if (direct) {
            try {
                direct->seal();
            } catch (...) {
            }
        }
    }

    void check_origin(std::uint64_t candidate) const {
        if (candidate != origin) {
            throw Error(ErrorCode::wrong_recording, "Handle belongs to another recording");
        }
    }

    template <class F> decltype(auto) call(F&& action) {
        return translate([&]() -> decltype(auto) {
            if (stream)
                return action(*stream);
            if (finished)
                throw Error(ErrorCode::invalid_argument, "Producer has finished");
            return action(*direct);
        });
    }

    template <class F> decltype(auto) control(F&& action) {
        return translate([&]() -> decltype(auto) {
            if (stream)
                return stream->control(std::forward<F>(action));
            return action(*direct);
        });
    }

    template <class F> decltype(auto) call_payload(const EncodedPayload& payload, F&& action) {
        return translate([&]() -> decltype(auto) {
            if (stream)
                return action(*stream, payload);
            if (finished)
                throw Error(ErrorCode::invalid_argument, "Producer has finished");
            if (!payload.encode)
                return action(
                    *direct,
                    engine::View(static_cast<const std::uint8_t*>(payload.source), payload.size));
            engine::Bytes storage(payload.size);
            payload.copy_to(std::as_writable_bytes(std::span(storage)));
            return action(*direct, engine::View(storage));
        });
    }
};

namespace {
engine::WriterOptions encoding_options(const EncodingOptions& options) {
    engine::WriterOptions result;
    result.codec = static_cast<wire::Codec>(options.compression);
    result.compression_level = options.compression_level;
    result.page_bytes = options.page_bytes;
    result.block_bytes = options.block_bytes;
    result.record_limit = options.block_records;
    result.wave_records = options.waveform_records;
    result.wave_bytes = options.waveform_bytes;
    result.group_size = options.waveform_group_size;
    result.dedup = options.waveform_deduplicate;
    result.separate_logs = options.separate_logs;
    result.separate_clocks = options.separate_clocks;
    result.workers = options.workers;
    result.queue_jobs = options.queue_jobs;
    result.queue_bytes = options.queue_bytes;
    result.payload_pool_bytes = options.payload_pool_bytes;
    return result;
}

std::uint32_t parent_id(std::optional<ScopeId> parent) {
    return parent ? parent->value : wire::none;
}
} // namespace
} // namespace detail

struct Writer::Impl {
    detail::RecordingContext context;
    std::unordered_map<std::type_index, TypeId> types;
    std::unordered_map<const void*, std::pair<TypeId, TypeId>> datatypes;

    Impl(const std::filesystem::path& path, const WriterOptions& options) {
        auto encoding = detail::encoding_options(options.encoding);
        if (options.mode == ExecutionMode::inline_) {
            if (encoding.workers)
                throw Error(ErrorCode::invalid_argument,
                            "Inline mode requires zero encoding workers");
            encoding.payload_pool_bytes = 0;
            context.direct = std::make_unique<detail::engine::Writer>(path.string(), encoding);
        } else if (options.mode == ExecutionMode::threaded) {
            context.stream = std::make_unique<detail::engine::Stream>(
                path.string(), encoding,
                detail::engine::StreamOptions{options.buffer.queue_bytes,
                                              options.buffer.pending_bytes});
        } else {
            throw Error(ErrorCode::invalid_argument, "Invalid execution mode");
        }
        context.control(
            [&](auto& writer) { writer.metadata() = detail::encode_metadata(options.meta); });
    }
};

ClockSource::ClockSource(GeneratorId id, std::uint64_t origin) noexcept
    : id_(id), origin_(origin) {}

GeneratorId ClockSource::id() const noexcept {
    return id_;
}

Writer::Writer(const std::filesystem::path& path, WriterOptions options)
    : impl_(detail::translate([&] { return std::make_unique<Impl>(path, options); })) {
    context_ = &impl_->context;
    origin_ = context_->origin;
}

Writer::~Writer() noexcept = default;

RecordingId Writer::identity() const noexcept {
    return detail::identity(context_->stream ? context_->stream->trace_id()
                                             : context_->direct->trace_id());
}

ProducerId Writer::add_producer(Timestamp time) {
    if (!context_->stream)
        throw Error(ErrorCode::invalid_argument, "Inline mode has one implicit producer");
    return detail::translate(
        [&] { return ProducerId(context_->stream->add_producer(time), recording_origin()); });
}

void Writer::use_producer(ProducerId producer) {
    context_->check_origin(producer.origin_);
    if (!context_->stream)
        throw Error(ErrorCode::invalid_argument, "Inline mode has one implicit producer");
    detail::translate([&] { context_->stream->use_producer(producer.value_); });
}

void Writer::release_producer() {
    if (context_->stream)
        detail::translate([&] { context_->stream->release_producer(); });
}

void Writer::finish_producer() {
    if (context_->stream)
        detail::translate([&] { context_->stream->finish_producer(); });
    else
        context_->finished = true;
}

void Writer::publish() {
    if (context_->stream)
        detail::translate([&] { context_->stream->publish(); });
    else
        (void)time();
}

Timestamp Writer::time() {
    if (context_->stream)
        return detail::translate([&] { return context_->stream->time(); });
    if (context_->finished || context_->closed)
        throw Error(ErrorCode::invalid_argument, "Producer has finished or recording is closed");
    return context_->current_time;
}

StringId Writer::intern(std::string_view text) {
    return context_->control([&](auto& out) { return StringId(out.intern(text)); });
}

void Writer::metadata(vtr::Metadata data) {
    context_->control([&](auto& out) { out.metadata() = detail::encode_metadata(data); });
}

ScopeId Writer::scope(std::string_view name, std::optional<ScopeId> parent, AttributeView metadata,
                      ScopeDeclaration declaration) {
    return context_->control([&](auto& out) {
        auto id =
            out.add_scope(detail::parent_id(parent), name,
                          static_cast<std::uint16_t>(declaration.kind), declaration.component);
        for (const auto& item : metadata) {
            out.node_attribute(id, item.key.value, detail::encode_value(item.data));
        }
        return ScopeId(id);
    });
}

NodeId Writer::enum_table(SourceOptions options, std::span<const EnumEntry> entries) {
    return context_->control([&](auto& out) {
        std::vector<std::pair<detail::wire::Id, detail::wire::Id>> literals;
        literals.reserve(entries.size());
        for (const auto& entry : entries)
            literals.emplace_back(entry.name.value, entry.value.value);
        const auto node =
            out.add_enum_table(detail::parent_id(options.parent), options.name, literals);
        for (const auto& item : options.metadata)
            out.node_attribute(node, item.key.value, detail::encode_value(item.data));
        return NodeId(node);
    });
}

TypeId Writer::bind_builtin(TypeId id) {
    return context_->control([&](auto& out) {
        (void)out.types();
        return id;
    });
}

TypeId Writer::cached_type(std::type_index type, const std::function<TypeId()>& declare) {
    return context_->control([&](auto&) -> TypeId {
        context_->control([](auto& out) { (void)out.types(); });
        auto [entry, inserted] = impl_->types.emplace(type, TypeId(UINT32_MAX));
        if (!inserted) {
            if (entry->second.value == UINT32_MAX) {
                throw Error(ErrorCode::invalid_argument, "Recursive payload schema");
            }
            return entry->second;
        }
        try {
            auto id = declare();
            impl_->types.at(type) = id;
            return id;
        } catch (...) {
            impl_->types.erase(type);
            throw;
        }
    });
}

TypeId Writer::declare_type(std::string name, decltype(DataType::definition) definition) {
    return declare_datatype({.name = std::move(name)}, std::move(definition));
}

TypeId Writer::declare_datatype(DatatypeOptions options,
                                decltype(DataType::definition) definition) {
    const auto& name = options.name;
    auto attributes = detail::encode_attributes(options.attributes);
    return context_->control([&](auto& out) {
        auto types = out.types();
        auto first = static_cast<std::uint32_t>(types.size());
        auto id = std::visit(
            [&]<class T>(const T& value) -> std::uint32_t {
                if constexpr (std::same_as<T, StructDefinition>) {
                    std::vector<detail::engine::Field> fields;
                    for (const auto& item : value.fields) {
                        fields.push_back({item.name, item.type.value});
                    }
                    return types.structure(name, std::move(fields), attributes);
                } else if constexpr (std::same_as<T, ArrayDefinition>) {
                    return types.array(value.element.value, value.count, name, attributes);
                } else if constexpr (std::same_as<T, SequenceDefinition>) {
                    return types.sequence(value.element.value, name, attributes);
                } else if constexpr (std::same_as<T, EnumDefinition>) {
                    std::vector<detail::engine::Enumerator> literals;
                    for (const auto& item : value.literals) {
                        literals.push_back({item.value_bits, item.name});
                    }
                    return types.enumeration(name, value.underlying.value, std::move(literals),
                                             attributes);
                } else {
                    throw Error(ErrorCode::invalid_argument, "Cannot declare a primitive type");
                }
            },
            definition);
        out.declare_types(types.declarations(first));
        return TypeId(id);
    });
}

GeneratorId Writer::source(SourceOptions options, std::string_view kind, TypeId payload) {
    return context_->control([&](auto& out) {
        auto id =
            out.add_generator(detail::parent_id(options.parent), options.name, kind, payload.value);
        for (const auto& item : options.metadata) {
            out.generator_attribute(id, item.key.value, detail::encode_value(item.data));
        }
        return GeneratorId(id);
    });
}

Logger Writer::logger(SourceOptions options) {
    return {source(std::move(options), "LOG", TypeId(0)), recording_origin()};
}

TypeId Writer::declare_payload_type(DatatypeOptions options, TypeId arguments,
                                    const void* descriptor) {
    return context_->control([&](auto& out) {
        if (descriptor) {
            const auto found = impl_->datatypes.find(descriptor);
            if (found != impl_->datatypes.end()) {
                const auto& declared = out.types()[found->second.first.value];
                detail::wire::Bytes previous, current;
                detail::wire::detail::encode_attrs(
                    previous, out.types().attributes(found->second.first.value));
                detail::wire::detail::encode_attrs(current,
                                                   detail::encode_attributes(options.attributes));
                if (found->second.second != arguments || declared.name != options.name ||
                    previous != current)
                    throw Error(ErrorCode::invalid_argument,
                                "Datatype descriptor changed declaration");
                return found->second.first;
            }
        }
        auto types = out.types();
        const auto first = static_cast<std::uint32_t>(types.size());
        const auto id = types.structure(std::move(options.name), {{"value", arguments.value}},
                                        detail::encode_attributes(options.attributes));
        out.declare_types(types.declarations(first));
        const auto result = TypeId(id);
        if (descriptor)
            impl_->datatypes.emplace(descriptor, std::pair{result, arguments});
        return result;
    });
}

void Writer::check_datatype(std::uint64_t origin) const {
    context_->check_origin(origin);
}

RecordId Writer::log_raw(Logger logger, TypeId datatype, Timestamp time, ByteView payload,
                         std::optional<OwnerRef> owner) {
    return instant(logger.id_, logger.origin_, SourceKind::log, time,
                   {payload.size(), payload.data()}, owner, datatype);
}

ClockSource Writer::clock(SourceOptions options) {
    return {source(std::move(options), "CLOCK", TypeId(0)), recording_origin()};
}

void Writer::associate_clock_impl(GeneratorId site, std::uint64_t origin, ClockSource clock) {
    context_->check_origin(origin);
    context_->check_origin(clock.origin_);
    context_->control([&](auto& out) { out.associate_clock(site.value, clock.id_.value); });
}

SignalId Writer::signal(SourceOptions options, SignalKind kind, VariableDeclaration declaration) {
    return context_->control([&](auto& out) {
        auto [node, id] =
            out.add_var(detail::parent_id(options.parent), options.name, detail::signal_kind(kind),
                        static_cast<std::uint16_t>(declaration.type),
                        static_cast<std::uint8_t>(declaration.direction));
        for (const auto& item : options.metadata) {
            out.node_attribute(node, item.key.value, detail::encode_value(item.data));
        }
        return SignalId(id);
    });
}

NodeId Writer::alias(SignalId signal, SourceOptions options, VariableDeclaration declaration) {
    return context_->control([&](auto& out) {
        auto id = out.add_alias(detail::parent_id(options.parent), options.name, signal.value,
                                static_cast<std::uint16_t>(declaration.type),
                                static_cast<std::uint8_t>(declaration.direction));
        for (const auto& item : options.metadata) {
            out.node_attribute(id, item.key.value, detail::encode_value(item.data));
        }
        return NodeId(id);
    });
}

RecordId Writer::begin_transaction(GeneratorId site, std::uint64_t origin, Timestamp time,
                                   std::optional<detail::EncodedPayload> payload,
                                   TransactionOptions options) {
    context_->check_origin(origin);
    if (options.parent)
        context_->check_origin(options.parent->origin_);
    const auto parent = options.parent ? options.parent->id_.value : 0;
    const auto id =
        payload ? context_->call_payload(*payload,
                                         [&](auto& out, const auto& data) {
                                             return out.begin(site.value, time, data, parent);
                                         })
                : context_->call([&](auto& out) { return out.begin(site.value, time, parent); });
    if (options.kind != SpanKind::unspecified)
        context_->call(
            [&](auto& out) { out.span_kind(id, static_cast<std::uint8_t>(options.kind)); });
    return RecordId(id);
}

RecordId Writer::begin_stage(GeneratorId site, std::uint64_t origin, OwnerRef owner, Timestamp time,
                             std::optional<detail::EncodedPayload> payload) {
    context_->check_origin(origin);
    context_->check_origin(owner.origin_);
    if (payload)
        return RecordId(context_->call_payload(*payload, [&](auto& out, const auto& data) {
            return out.begin_stage(site.value, owner.id_.value, time, data);
        }));
    return context_->call(
        [&](auto& out) { return RecordId(out.begin_stage(site.value, owner.id_.value, time)); });
}

void Writer::set_payload(RecordId record, std::uint64_t origin, SourceKind kind,
                         detail::EncodedPayload payload) {
    context_->check_origin(origin);
    context_->call_payload(payload, [&](auto& out, const auto& data) {
        if (kind == SourceKind::stage)
            out.stage_payload(record.value, data);
        else
            out.payload(record.value, data);
    });
}

void Writer::finish(RecordId record, std::uint64_t origin, SourceKind kind, Timestamp time,
                    Status result, std::optional<detail::EncodedPayload> payload) {
    context_->check_origin(origin);
    if (kind == SourceKind::stage) {
        if (payload)
            context_->call_payload(*payload, [&](auto& out, const auto& data) {
                out.stage_payload(record.value, data);
            });
        context_->call([&](auto& out) { out.end_stage(record.value, time); });
    } else if (payload) {
        context_->call_payload(*payload, [&](auto& out, const auto& data) {
            out.end_captured(record.value, time, data, static_cast<std::uint8_t>(result));
        });
    } else {
        context_->call(
            [&](auto& out) { out.end(record.value, time, static_cast<std::uint8_t>(result)); });
    }
}

RecordId Writer::instant(GeneratorId site, std::uint64_t origin, SourceKind kind, Timestamp time,
                         detail::EncodedPayload payload, std::optional<OwnerRef> owner,
                         std::optional<TypeId> datatype) {
    context_->check_origin(origin);
    if (owner)
        context_->check_origin(owner->origin_);
    return context_->call_payload(payload, [&](auto& out, const auto& data) {
        const auto parent = owner ? owner->id_.value : 0;
        return RecordId(kind == SourceKind::event
                            ? out.event(site.value, parent, time, data)
                            : out.log(site.value, datatype->value, time, data, parent));
    });
}

RecordId Writer::event_at(Source<Unit, SourceKind::event> site, OwnerRef owner, Timestamp time) {
    return event_at(site, owner, time, Unit{});
}

void Writer::attribute_u64(OwnerRef owner, StringId key, std::uint64_t data) {
    context_->check_origin(owner.origin_);
    context_->call([&](auto& out) { out.attribute(owner.id_.value, key.value, data); });
}

void Writer::attribute(OwnerRef owner, StringId key, const Value& data) {
    context_->check_origin(owner.origin_);
    if (const auto* number = std::get_if<std::uint64_t>(&data.data)) {
        context_->call([&](auto& out) { out.attribute(owner.id_.value, key.value, *number); });
        return;
    }
    context_->call(
        [&](auto& out) { out.attribute(owner.id_.value, key.value, detail::encode_value(data)); });
}

void Writer::classify(OwnerRef owner, SpanKind kind) {
    context_->check_origin(owner.origin_);
    context_->call(
        [&](auto& out) { out.span_kind(owner.id_.value, static_cast<std::uint8_t>(kind)); });
}

void Writer::relate(StringId kind, std::optional<RecordId> from, std::optional<RecordId> to) {
    context_->call(
        [&](auto& out) { out.relate(kind.value, from ? from->value : 0, to ? to->value : 0); });
}

RecordId Writer::run(ClockSource clock, Timestamp first_edge, Timestamp period) {
    context_->check_origin(clock.origin_);
    return context_->call(
        [&](auto& out) { return RecordId(out.clock_run(clock.id_.value, first_edge, period)); });
}

void Writer::stop(ClockSource clock, Timestamp time) {
    context_->check_origin(clock.origin_);
    context_->call([&](auto& out) { out.clock_stop(clock.id_.value, time); });
}

void Writer::advance_time(Timestamp time) {
    if (context_->stream) {
        detail::translate([&] { context_->stream->advance_time(time); });
    } else {
        (void)this->time();
        detail::translate([&] { context_->direct->set_time(time); });
        context_->current_time = time;
    }
}

void Writer::emit(SignalId signal, std::uint64_t value) {
    context_->call([&](auto& out) { out.emit(signal.value, value); });
}

void Writer::emit(std::span<const SignalUpdate> updates) {
    std::array<detail::wire::SignalUpdate, 64> local;
    std::vector<detail::wire::SignalUpdate> converted;
    if (updates.size() > local.size())
        converted.resize(updates.size());
    auto output = updates.size() <= local.size() ? std::span(local).first(updates.size())
                                                 : std::span(converted);
    for (std::size_t i = 0; i < updates.size(); ++i) {
        output[i] = {updates[i].signal.value, updates[i].value};
    }
    context_->call([&](auto& out) { out.emit(output); });
}

void Writer::emit_real(SignalId signal, double value) {
    context_->call([&](auto& out) { out.emit_real(signal.value, value); });
}

void Writer::emit_packed(SignalId signal, ByteView packed) {
    if (context_->stream)
        detail::translate(
            [&] { context_->stream->emit_packed(signal.value, 0, detail::view(packed)); });
    else {
        (void)time();
        detail::translate([&] {
            auto& out = *context_->direct;
            out.emit_packed(signal.value, out.signal_kind(signal.value).states,
                            detail::view(packed));
        });
    }
}

void Writer::emit_words(SignalId signal, std::span<const std::uint32_t> words,
                        std::uint32_t width) {
    if (!width || words.size() < (std::uint64_t(width) + 31) / 32)
        throw Error(ErrorCode::invalid_argument, "Insufficient signal words or zero input width");

    struct Input {
        std::span<const std::uint32_t> words;
        std::uint32_t width;
    } input{words, width};

    const detail::EncodedPayload payload{
        (std::uint64_t(width) + 7) / 8, &input,
        [](std::span<std::byte> destination, const void* source) {
            const auto& input = *static_cast<const Input*>(source);
            if constexpr (std::endian::native == std::endian::little) {
                std::memcpy(destination.data(), input.words.data(), destination.size());
            } else {
                for (std::size_t i = 0; i < destination.size(); ++i)
                    destination[i] = static_cast<std::byte>(input.words[i / 4] >> (8 * (i % 4)));
            }
            if (input.width % 8)
                destination.back() &= static_cast<std::byte>((1U << (input.width % 8)) - 1);
        }};
    context_->call([&](auto& out) {
        if constexpr (std::is_same_v<std::remove_reference_t<decltype(out)>,
                                     detail::engine::Stream>) {
            out.emit_packed(signal.value, 2, payload, width);
        } else {
            const auto kind = out.signal_kind(signal.value);
            if (kind.type != detail::wire::SignalKind::bits || kind.width != width)
                throw Error(ErrorCode::invalid_argument,
                            "Input word width differs from signal declaration");
            std::array<std::byte, 256> small;
            Bytes large;
            if (payload.size > small.size())
                large.resize(payload.size);
            auto bytes = payload.size <= small.size() ? std::span(small).first(payload.size)
                                                      : std::span(large);
            payload.copy_to(bytes);
            out.emit_packed(signal.value, 2, detail::view(bytes));
        }
    });
}

void Writer::emit_bytes(SignalId signal, ByteView value) {
    context_->call([&](auto& out) { out.emit_varlen(signal.value, detail::view(value)); });
}

void Writer::dump(bool enabled) {
    context_->call([&](auto& out) { out.dump(enabled); });
}

RecordId Writer::embedded_event(OwnerRef owner, Timestamp time, StringId name,
                                AttributeView attributes) {
    context_->check_origin(owner.origin_);
    return context_->call([&](auto& out) {
        return RecordId(out.embedded_event(owner.id_.value, time, name.value,
                                           detail::encode_attributes(attributes)));
    });
}

RecordId Writer::embedded_stage(OwnerRef owner, Timestamp begin, StringId name, StringId lane,
                                std::optional<Timestamp> end, AttributeView attributes) {
    context_->check_origin(owner.origin_);
    return context_->call([&](auto& out) {
        return RecordId(out.embedded_stage(owner.id_.value, name.value, lane.value, begin, end,
                                           detail::encode_attributes(attributes)));
    });
}

void Writer::end_embedded_stage(OwnerRef owner, RecordId stage, Timestamp time) {
    context_->check_origin(owner.origin_);
    context_->call([&](auto& out) { out.embedded_stage_end(owner.id_.value, stage.value, time); });
}

void Writer::relate(StringId kind, std::optional<RecordId> from, std::optional<RecordId> to,
                    AttributeView attributes) {
    context_->call([&](auto& out) {
        out.relate(kind.value, from ? from->value : 0, to ? to->value : 0,
                   detail::encode_attributes(attributes));
    });
}

void Writer::flush() {
    if (context_->stream)
        detail::translate([&] { context_->stream->flush(); });
    else
        context_->control([](auto& out) { out.flush(); });
}

void Writer::close() {
    if (context_->stream)
        detail::translate([&] { context_->stream->close(); });
    else {
        context_->control([](auto& out) { out.close(); });
        context_->closed = true;
    }
}

void Writer::seal() {
    if (context_->stream)
        detail::translate([&] { context_->stream->seal(); });
    else {
        context_->control([](auto& out) { out.seal(); });
        context_->closed = true;
    }
}

WriterStats Writer::stats() {
    return detail::translate([&] {
        auto backend = context_->stream ? context_->stream->stats() : context_->direct->stats();
        auto transport =
            context_->stream ? context_->stream->stream_stats() : detail::engine::StreamStats{};
        auto encoding = context_->stream ? context_->stream->handoff_stats()
                                         : context_->direct->handoff_stats();
        std::optional<std::uint64_t> backend_cpu;
        if (transport.worker_cpu_nanoseconds && encoding.worker_cpu_nanoseconds)
            backend_cpu = *transport.worker_cpu_nanoseconds + *encoding.worker_cpu_nanoseconds;
        return WriterStats{{transport.submitted, transport.completed, transport.waits,
                            transport.wait_nanoseconds, transport.peak_pending_bytes},
                           {encoding.accepted, encoding.written, encoding.wait_count,
                            encoding.wait_nanoseconds, encoding.peak_bytes},
                           backend.active_owners,
                           backend.active_stages,
                           backend.active_payload_bytes,
                           backend.peak_transaction_buffer_bytes,
                           backend.peak_record_buffer_bytes,
                           backend.peak_wave_buffer_bytes,
                           backend.payload_pool_bytes,
                           backend.payload_reuses,
                           backend.transaction_blocks,
                           backend.record_blocks,
                           transport.spilled_bytes,
                           backend_cpu};
    });
}

std::optional<std::uint64_t> Writer::backend_cpu_nanoseconds() const {
    if (!context_->stream)
        return 0;
    return detail::translate([&] { return context_->stream->backend_cpu_nanoseconds(); });
}

} // namespace vtr
