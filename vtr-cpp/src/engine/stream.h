#pragma once

#include "engine/vtr.h"
#include <functional>
#include <optional>
#include <type_traits>
#include <utility>

namespace vtr::detail {
struct EncodedPayload;
}

namespace vtr::detail::engine {

struct StreamOptions {
    std::size_t queue_bytes = 256 << 10;
    std::size_t pending_bytes = 16 << 20;
};

struct StreamStats {
    std::uint64_t submitted = 0, completed = 0, waits = 0, wait_nanoseconds = 0;
    std::uint64_t spilled_bytes = 0;
    std::size_t peak_pending_bytes = 0;
    std::optional<std::uint64_t> worker_cpu_nanoseconds = 0;
};

class Stream {
  public:
    using ProducerId = std::uint32_t;

    explicit Stream(const std::string& path, WriterOptions options = {}, StreamOptions stream = {});
    ~Stream();
    Stream(const Stream&) = delete;
    Stream& operator=(const Stream&) = delete;

    const TraceId& trace_id() const noexcept;
    ProducerId add_producer(Timestamp initial_time = 0);
    void use_producer(ProducerId producer);
    void release_producer();
    void advance_time(Timestamp time);
    Timestamp time();
    void publish();
    void finish_producer();

    void emit(Id signal, std::uint64_t value);
    void emit(std::span<const SignalUpdate> updates);
    void emit_packed(Id signal, std::uint8_t states, View value);
    void emit_packed(Id signal, std::uint8_t states, const EncodedPayload& value,
                     std::uint32_t width);
    void emit_real(Id signal, double value);
    void emit_varlen(Id signal, View value);
    void dump(bool enabled);
    RecordId begin(GeneratorId generator, Timestamp time, RecordId parent = 0);
    RecordId begin(GeneratorId generator, Timestamp time, View payload, RecordId parent = 0);
    RecordId begin(GeneratorId generator, Timestamp time, const EncodedPayload& payload,
                   RecordId parent = 0);
    RecordId begin_stage(GeneratorId generator, RecordId owner, Timestamp time);
    RecordId begin_stage(GeneratorId generator, RecordId owner, Timestamp time, View payload);
    RecordId begin_stage(GeneratorId generator, RecordId owner, Timestamp time,
                         const EncodedPayload& payload);
    RecordId event(GeneratorId generator, RecordId owner, Timestamp time, View payload = {});
    RecordId event(GeneratorId generator, RecordId owner, Timestamp time,
                   const EncodedPayload& payload);
    RecordId log(GeneratorId generator, Timestamp time, View payload = {}, RecordId owner = 0);
    RecordId log(GeneratorId generator, Timestamp time, const EncodedPayload& payload,
                 RecordId owner = 0);
    void payload(RecordId record, View payload);
    void payload(RecordId record, const EncodedPayload& payload);
    void stage_payload(RecordId record, View payload);
    void stage_payload(RecordId record, const EncodedPayload& payload);
    void end(RecordId record, Timestamp time, std::uint8_t status = 0);
    void end_captured(RecordId record, Timestamp time, View payload, std::uint8_t status = 0);
    void end_captured(RecordId record, Timestamp time, const EncodedPayload& payload,
                      std::uint8_t status = 0);

    template <class T>
    void end_value(RecordId record, Timestamp time, const T& value, std::uint8_t status = 0) {
        auto payload = capture(value);
        end_captured(record, time, payload, status);
    }

    void end_stage(RecordId record, Timestamp time);
    void attribute(RecordId owner, Id key, const Value& value);
    void attribute(RecordId owner, Id key, std::uint64_t value);
    void span_kind(RecordId owner, std::uint8_t kind);
    void relate(Id kind, RecordId from, RecordId to,
                std::span<const vtr::detail::wire::Attribute> attributes = {});
    RecordId embedded_event(RecordId owner, Timestamp time, Id name,
                            std::span<const vtr::detail::wire::Attribute> attributes = {});
    RecordId embedded_stage(RecordId owner, Id name, Id lane, Timestamp begin,
                            std::optional<Timestamp> end = {},
                            std::span<const vtr::detail::wire::Attribute> attributes = {});
    void embedded_stage_end(RecordId owner, RecordId stage, Timestamp time);
    RecordId clock_run(GeneratorId clock, Timestamp first, Timestamp period);
    void clock_stop(GeneratorId clock, Timestamp time);

    template <class F> decltype(auto) control(F&& action) {
        check();
        using Result = std::invoke_result_t<F, Writer&>;
        using Storage = std::conditional_t<
            std::is_void_v<Result>, bool,
            std::conditional_t<std::is_reference_v<Result>,
                               std::add_pointer_t<std::remove_reference_t<Result>>, Result>>;
        std::optional<Storage> result;
        invoke([&](Writer& writer) {
            check();
            if constexpr (std::is_void_v<Result>) {
                action(writer);
                result = true;
            } else if constexpr (std::is_reference_v<Result>) {
                result = &action(writer);
            } else {
                result = action(writer);
            }
        });
        if constexpr (std::is_reference_v<Result>) {
            return **result;
        } else if constexpr (!std::is_void_v<Result>) {
            return Result(std::move(*result));
        }
    }

    void flush();
    void close();
    void seal();
    WriterStats stats();
    StreamStats stream_stats();
    std::optional<std::uint64_t> backend_cpu_nanoseconds() const;
    HandoffStats handoff_stats();

  private:
    void check() const;
    void invoke(const std::function<void(Writer&)>& action);
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace vtr::detail::engine
