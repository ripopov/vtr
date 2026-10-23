#pragma once

#include "engine/vtr.h"
#include "format.h"

#include "vtr_common.h"
#include <filesystem>

namespace vtr::detail {

namespace binary = wire::detail;

static_assert(format_major == binary::format_major && format_minor == binary::format_minor);

template <class F> decltype(auto) translate(F&& action) {
    try {
        return std::forward<F>(action)();
    } catch (const wire::Error& error) {
        auto code = ErrorCode::invalid_argument;
        switch (error.code) {
        case 2:
            code = ErrorCode::invalid_state;
            break;
        case 3:
            code = ErrorCode::io;
            break;
        case 4:
        case 6:
        case 7:
            code = ErrorCode::corrupt_data;
            break;
        case 5:
            code = ErrorCode::unsupported_version;
            break;
        default:
            break;
        }
        throw Error(code, error.what());
    } catch (const std::filesystem::filesystem_error& error) {
        throw Error(ErrorCode::io, error.what());
    }
}

inline wire::detail::View view(ByteView bytes) {
    return {reinterpret_cast<const std::uint8_t*>(bytes.data()), bytes.size()};
}

inline Bytes bytes(wire::detail::View value) {
    auto data = std::as_bytes(value);
    return {data.begin(), data.end()};
}

inline RecordingId identity(const wire::TraceId& id) {
    RecordingId result;
    std::copy(id.begin(), id.end(), reinterpret_cast<std::uint8_t*>(result.bytes.data()));
    return result;
}

inline Fingerprint fingerprint(const wire::TraceFingerprint& value) {
    return {identity(value.recording), value.bytes, value.directory_crc};
}

inline wire::SignalKind signal_kind(SignalKind kind) {
    return {static_cast<wire::SignalKind::Type>(kind.encoding), kind.width,
            static_cast<std::uint8_t>(kind.alphabet)};
}

inline SignalKind signal_kind(wire::SignalKind kind) {
    return {static_cast<SignalEncoding>(kind.type), kind.width, static_cast<States>(kind.states)};
}

inline RecordKind record_kind(engine::RecordKind kind) {
    switch (kind) {
    case engine::RecordKind::ordinary:
        return RecordKind::transaction;
    case engine::RecordKind::clock:
        return RecordKind::clock;
    case engine::RecordKind::event:
        return RecordKind::event;
    case engine::RecordKind::stage:
        return RecordKind::stage;
    case engine::RecordKind::log:
        return RecordKind::log;
    case engine::RecordKind::embedded_event:
        return RecordKind::embedded_event;
    case engine::RecordKind::embedded_stage:
        return RecordKind::embedded_stage;
    }
    throw Error(ErrorCode::corrupt_data, "Unknown record kind");
}

wire::Value encode_value(const Value& value, unsigned depth = 0);
Value decode_value(const wire::Value& value, unsigned depth = 0);
wire::Attributes encode_attributes(AttributeView attributes);
Attributes decode_attributes(std::span<const wire::Attribute> attributes);
wire::Meta encode_metadata(const Metadata& metadata);
Metadata decode_metadata(const wire::Meta& metadata);
DataType decode_type(std::uint32_t id, const engine::Type& type);

} // namespace vtr::detail
