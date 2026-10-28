#include "packet_spool.h"
#include <algorithm>
#include <iostream>

using namespace vtr::detail::engine;
namespace d = vtr::detail::engine::detail;

void check(bool valid, const char* message) {
    if (!valid)
        throw std::runtime_error(message);
}

template <class Header> Bytes packet(Header header, View payload = {}) {
    Bytes bytes(sizeof(header) + payload.size());
    std::memcpy(bytes.data(), &header, sizeof(header));
    std::ranges::copy(payload, bytes.begin() + sizeof(header));
    return bytes;
}

int main() {
    try {
        d::PacketSpool spool;
        check(spool.front().empty(), "unopened spool has pending packets");
        for (unsigned round = 0; round < 8; ++round) {
            Bytes payload(8193 + round, static_cast<std::uint8_t>(round));
            d::RecordPacket record{d::Kind::log, 0, round, round + 1, round * 2};
            record.bytes = payload.size();
            const auto large = packet(record, payload);
            const auto scalar = packet(d::Packet{d::Kind::scalar, 0, 0, round, round * 3});
            const auto attribute = packet(
                d::AttributePacket{{d::Kind::scalar_attribute, 0, 0, round, round * 4}, round + 1});
            spool.append(large);
            spool.append(scalar);
            auto first = spool.front();
            check(std::ranges::equal(first, large), "spool changed an oversized packet");
            spool.append(attribute);
            check(std::ranges::equal(first, spool.front()),
                  "append invalidated an unconsumed replay packet");
            spool.consume();
            check(std::ranges::equal(spool.front(), scalar),
                  "read/write switch lost a buffered packet");
            spool.consume();
            spool.append(large);
            check(std::ranges::equal(spool.front(), attribute),
                  "read/write switch changed a short attribute packet");
            spool.consume();
            check(std::ranges::equal(spool.front(), large), "spool reordered appended packets");
            spool.consume();
            check(spool.front().empty() && !spool.opened(), "drained spool did not close");
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
