#include "packet_spool.h"
#include "format.h"
#include <limits>

namespace vtr::detail::engine::detail {
namespace {
void io(bool success, const char* message) {
    wire::detail::require(success, message, ErrorCode::io);
}
} // namespace

void PacketSpool::position(Direction direction, off_t offset) {
    // Update streams require positioning when switching between reads and writes.
    if (direction_ != direction) {
        io(::fseeko(file_.get(), offset, SEEK_SET) == 0, "Cannot seek pending recording spool");
        direction_ = direction;
    }
}

void PacketSpool::append(View packet) {
    if (!file_) {
        file_.reset(std::tmpfile());
        io(bool(file_), "Cannot create pending recording spool");
    }
    wire::detail::require(packet.size() <= static_cast<std::uint64_t>(
                                               std::numeric_limits<off_t>::max() - write_offset_),
                          "Pending recording spool exceeds file offset limit",
                          ErrorCode::resource_limit);
    position(Direction::write, write_offset_);
    io(std::fwrite(packet.data(), 1, packet.size(), file_.get()) == packet.size(),
       "Cannot write pending recording spool");
    write_offset_ += static_cast<off_t>(packet.size());
}

View PacketSpool::front() {
    if (!file_)
        return {};
    if (replay_ready_)
        return replay_;
    if (read_offset_ == write_offset_) {
        auto* file = file_.release();
        io(std::fclose(file) == 0, "Cannot close pending recording spool");
        read_offset_ = write_offset_ = 0;
        direction_ = Direction::none;
        return {};
    }
    position(Direction::read, read_offset_);
    replay_.resize(sizeof(RecordPacket));
    io(std::fread(replay_.data(), 1, sizeof(Packet), file_.get()) == sizeof(Packet),
       "Cannot read pending recording spool");
    const auto kind = packet_header(replay_.data()).kind;
    const auto header = packet_header_bytes(kind);
    if (header > sizeof(Packet)) {
        const auto tail = header - sizeof(Packet);
        io(std::fread(replay_.data() + sizeof(Packet), 1, tail, file_.get()) == tail,
           "Cannot read pending recording spool");
    }
    const auto bytes = packet_size(replay_.data(), kind);
    replay_.resize(bytes);
    io(std::fread(replay_.data() + header, 1, bytes - header, file_.get()) == bytes - header,
       "Cannot read pending recording spool");
    read_offset_ += static_cast<off_t>(bytes);
    replay_ready_ = true;
    return replay_;
}

} // namespace vtr::detail::engine::detail
