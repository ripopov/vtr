#pragma once

#include "capture_packets.h"
#include <cstdio>

namespace vtr::detail::engine::detail {

// The assembly thread replays packets that exceed its in-memory pending budget.
class PacketSpool {
  public:
    PacketSpool() {
        replay_.reserve(sizeof(RecordPacket));
    }

    PacketSpool(const PacketSpool&) = delete;
    PacketSpool& operator=(const PacketSpool&) = delete;

    bool opened() const noexcept {
        return bool(file_);
    }

    void append(View packet);
    View front();

    void consume() noexcept {
        replay_ready_ = false;
    }

  private:
    struct CloseFile {
        void operator()(std::FILE* file) const noexcept {
            std::fclose(file);
        }
    };
    enum class Direction { none, read, write };
    void position(Direction direction, off_t offset);
    std::unique_ptr<std::FILE, CloseFile> file_;
    off_t read_offset_ = 0, write_offset_ = 0;
    Bytes replay_;
    bool replay_ready_ = false;
    Direction direction_ = Direction::none;
};

} // namespace vtr::detail::engine::detail
