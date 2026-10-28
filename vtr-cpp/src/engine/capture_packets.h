#pragma once

#include "engine/records.h"
#include <cstring>

namespace vtr::detail::engine::detail {

enum class Kind : std::uint8_t {
    scalar,
    span,
    packed,
    real,
    bytes,
    dump,
    begin,
    begin_value,
    stage,
    stage_value,
    event,
    log,
    payload,
    stage_payload,
    end,
    end_value,
    stage_end,
    attribute,
    scalar_attribute,
    classify,
    relate,
    embedded_event,
    embedded_stage,
    embedded_end,
    clock_run,
    clock_stop,
    advance,
    finish,
    external
};

struct Packet {
    Kind kind;
    std::uint8_t status = 0;
    std::uint16_t reserved = 0;
    wire::Id generator = 0;
    std::uint64_t argument = 0;
};

struct RecordPacket {
    Packet packet;
    RecordId id = 0;
    Timestamp time = 0;
    std::size_t bytes = 0;

    RecordPacket() = default;

    RecordPacket(Kind kind, std::uint8_t status = 0, wire::Id generator = 0, RecordId record = 0,
                 Timestamp timestamp = 0, std::uint64_t argument = 0)
        : packet{kind, status, 0, generator, argument}, id(record), time(timestamp) {}
};

struct AttributePacket {
    Packet packet;
    RecordId id = 0;
};

static_assert(sizeof(Packet) == 16 && sizeof(RecordPacket) == 40 && sizeof(AttributePacket) == 24);

inline Packet packet_header(const std::uint8_t* data) {
    Packet packet;
    std::memcpy(&packet, data, sizeof(packet));
    return packet;
}

inline bool small_packet(Kind kind) {
    return kind == Kind::scalar || kind == Kind::advance || kind == Kind::finish ||
           kind == Kind::external || kind == Kind::dump;
}

inline std::size_t packet_header_bytes(Kind kind) {
    if (kind == Kind::scalar_attribute)
        return sizeof(AttributePacket);
    return small_packet(kind) ? sizeof(Packet) : sizeof(RecordPacket);
}

inline std::size_t packet_size(const std::uint8_t* data, Kind kind) {
    if (kind == Kind::scalar_attribute)
        return sizeof(AttributePacket);
    if (small_packet(kind))
        return sizeof(Packet);
    std::size_t bytes;
    std::memcpy(&bytes, data + offsetof(RecordPacket, bytes), sizeof(bytes));
    return sizeof(RecordPacket) + bytes;
}

} // namespace vtr::detail::engine::detail
