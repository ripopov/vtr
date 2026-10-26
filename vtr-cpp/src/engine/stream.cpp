#include "stream.h"
#include "containers.h"
#include "format.h"
#include "record_ids.h"
#include "spsc.h"
#include "thread_cpu.h"
#include "vtr_common.h"

#include <atomic>
#include <bit>
#include <chrono>
#include <cstdio>
#include <limits>
#include <thread>

namespace vtr::detail::engine {
namespace {
namespace wire_detail = vtr::detail::wire::detail;
constexpr std::size_t drain_bytes = 256 << 10;

std::atomic<std::uint64_t> next_identity{1};
thread_local const auto thread_identity = next_identity.fetch_add(1, std::memory_order_relaxed);

[[gnu::cold, noreturn]] void reject(const char* message) {
    throw vtr::detail::wire::Error(message);
}

inline void require(bool condition, const char* message) {
    if (!condition)
        reject(message);
}

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

struct Operation {
    Kind kind;
    std::uint8_t status = 0;
    Id generator = 0;
    RecordId id = 0;
    Timestamp time = 0;
    std::uint64_t argument = 0;
    std::size_t offset = 0, size = 0;
};

struct Packet {
    Kind kind;
    std::uint8_t status = 0;
    std::uint16_t reserved = 0;
    Id generator = 0;
    std::uint64_t argument = 0;
};

struct RecordPacket {
    Packet packet;
    RecordId id = 0;
    Timestamp time = 0;
    std::size_t bytes = 0;
};

struct AttributePacket {
    Packet packet;
    RecordId id = 0;
};

static_assert(sizeof(Packet) == 16 && sizeof(RecordPacket) == 40 && sizeof(AttributePacket) == 24);

Packet packet_header(const std::uint8_t* data) {
    Packet packet;
    std::memcpy(&packet, data, sizeof(packet));
    return packet;
}

bool small_packet(Kind kind) {
    return kind == Kind::scalar || kind == Kind::advance || kind == Kind::finish ||
           kind == Kind::external || kind == Kind::dump;
}

std::size_t packet_header_bytes(Kind kind) {
    if (kind == Kind::scalar_attribute)
        return sizeof(AttributePacket);
    return small_packet(kind) ? sizeof(Packet) : sizeof(RecordPacket);
}

std::size_t packet_size(const std::uint8_t* data, Kind kind) {
    if (kind == Kind::scalar_attribute)
        return sizeof(AttributePacket);
    if (small_packet(kind))
        return sizeof(Packet);
    std::size_t bytes;
    std::memcpy(&bytes, data + offsetof(RecordPacket, bytes), sizeof(bytes));
    return sizeof(RecordPacket) + bytes;
}

struct Request {
    const std::function<void(Writer&)>& action;
    Request* next = nullptr;
    std::atomic<bool> done{false};
    std::exception_ptr error = {};
};

constexpr auto unowned_producer = std::numeric_limits<Stream::ProducerId>::max();

struct Touch {
    Timestamp time = 0;
    Stream::ProducerId producer = unowned_producer;
};

struct RecordOwner {
    RecordId owner;
    Touch touch;
    std::vector<RecordId> stages;
};

class TracingOwners {
    struct Slot {
        RecordId id = 0;
        RecordOwner value{};
    };

    std::array<Slot, 1024> slots_{};
    wire_detail::HashMap<RecordId, RecordOwner> overflow_;

  public:
    RecordOwner* find(RecordId id) {
        auto& slot = slots_[id & (slots_.size() - 1)];
        if (id && slot.id == id)
            return &slot.value;
        auto found = overflow_.find(id);
        return found == overflow_.end() ? nullptr : &found->second;
    }

    RecordOwner& at(RecordId id) {
        auto* value = find(id);
        require(value, "record is not active");
        return *value;
    }

    void emplace(RecordId id, RecordOwner value) {
        auto& slot = slots_[id & (slots_.size() - 1)];
        require(id && slot.id != id, "record identity reused");
        // Stream IDs are assigned once; displaced live records must survive slot reuse.
        if (slot.id) {
            require(overflow_.emplace(slot.id, std::move(slot.value)).second,
                    "displaced record identity reused");
        }
        slot.id = id;
        slot.value = std::move(value);
    }

    void erase(RecordId id) {
        auto& slot = slots_[id & (slots_.size() - 1)];
        if (id && slot.id == id) {
            slot = Slot{};
        } else {
            overflow_.erase(id);
        }
    }
};

struct Channel {
    const Stream::ProducerId id;
    Channel* const next;
    detail::SpscBytes<> incoming;
    alignas(64) std::atomic<std::uint64_t> writer{0};
    Timestamp producer_time;
    bool producer_finished = false;
    std::uint64_t producer_submitted = 0;
    std::atomic<std::uint64_t> submitted{0}, waits{0}, wait_nanoseconds{0};
    std::atomic<bool> external_pending{false};
    alignas(64) Timestamp watermark, last_time, play_time;
    bool finished = false;
    std::uint64_t received = 0;
    Bytes pending;
    std::size_t pending_read = 0;
    std::FILE* spool = nullptr;
    off_t read_offset = 0, write_offset = 0;
    Bytes replay;
    bool replay_ready = false;

    Channel(Stream::ProducerId number, Channel* previous, Timestamp start, std::size_t capacity,
            std::size_t pending_budget)
        : id(number), next(previous), incoming(capacity), producer_time(start), watermark(start),
          last_time(start), play_time(start) {
        pending.reserve(std::min<std::size_t>(64 << 10, pending_budget));
        replay.reserve(sizeof(RecordPacket));
    }

    ~Channel() {
        if (spool)
            std::fclose(spool);
        while (auto* data = incoming.peek()) {
            const auto packet = packet_header(data);
            const auto bytes = packet_size(data, packet.kind);
            if (packet.kind == Kind::external) {
                delete[] reinterpret_cast<std::uint8_t*>(packet.argument);
            }
            incoming.consume(bytes);
        }
    }
};

struct Selection {
    const void* recording = nullptr;
    std::uint64_t generation = 0;
    Channel* channel = nullptr;
};

thread_local Selection selected;
thread_local const void* backend_recording = nullptr;
} // namespace

struct Stream::Impl {
    enum class Startup : std::uint8_t { setup, declaring, starting, running };
    Writer backend;
    const StreamOptions options;
    const std::uint64_t generation = next_identity.fetch_add(1, std::memory_order_relaxed);
    const std::uint64_t coordinator = thread_identity;
    alignas(64) std::atomic<Startup> startup{Startup::setup};
    std::atomic<Channel*> channels{nullptr};
    std::vector<std::unique_ptr<Channel>> registered;
    alignas(64) std::atomic<Request*> requests{nullptr};
    std::atomic<unsigned> active_requests{0};
    // Backend control polling must not invalidate the producers' ID counter.
    detail::AtomicRecordIds<> record_ids;
    alignas(64) std::atomic<bool> failed{false}, stopped{false};
    std::atomic_flag failure_claim;
    std::exception_ptr failure;
    std::thread worker;
    detail::ThreadCpu worker_cpu;
    alignas(64) Timestamp frontier = 0, serialized_time = 0;
    bool all_finished = false;
    std::size_t pending_bytes = 0;
    StreamStats counters;
    std::vector<Touch> signals;
    TracingOwners records;
    wire_detail::HashMap<GeneratorId, Touch> clocks;
    std::optional<Touch> dump_owner;

    Impl(const std::string& path, WriterOptions encoding, StreamOptions buffer)
        : backend(path, encoding), options(buffer) {
        require(options.queue_bytes >= 64 && options.pending_bytes &&
                    std::has_single_bit(options.queue_bytes),
                "invalid stream buffer options");
    }

    void check() const {
        if (failed.load(std::memory_order_acquire)) {
            std::rethrow_exception(failure);
        }
        require(!stopped.load(std::memory_order_acquire), "recording is closed");
    }

    void fail(std::exception_ptr error) noexcept {
        if (!failure_claim.test_and_set(std::memory_order_relaxed)) {
            failure = std::move(error);
            failed.store(true, std::memory_order_release);
        }
    }

    void invoke(const std::function<void(Writer&)>& action) {
        if (backend_recording == this) {
            action(backend);
            return;
        }
        auto expected = Startup::setup;
        if (thread_identity == coordinator &&
            startup.compare_exchange_strong(expected, Startup::declaring,
                                            std::memory_order_acquire)) {
            struct SetupDeclaration {
                Impl& stream;
                const void* previous = backend_recording;

                ~SetupDeclaration() {
                    backend_recording = previous;
                    stream.startup.store(Startup::setup, std::memory_order_release);
                }
            } declaration{*this};

            backend_recording = this;
            require(!stopped.load(), "recording is closed");
            action(backend);
            return;
        }
        start();
        active_requests.fetch_add(1);

        struct PendingRequest {
            std::atomic<unsigned>& count;

            ~PendingRequest() {
                count.fetch_sub(1);
            }
        } pending_request{active_requests};

        require(!stopped.load(), "recording is closed");
        Request request{action};
        auto* head = requests.load(std::memory_order_relaxed);
        do {
            request.next = head;
        } while (!requests.compare_exchange_weak(head, &request, std::memory_order_release,
                                                 std::memory_order_relaxed));
        while (!request.done.load(std::memory_order_acquire)) {
            std::this_thread::yield();
        }
        if (request.error) {
            std::rethrow_exception(request.error);
        }
    }

    void start() {
        auto state = startup.load(std::memory_order_acquire);
        while (state != Startup::running) {
            require(!stopped.load(std::memory_order_acquire), "recording is closed");
            if (state == Startup::setup &&
                startup.compare_exchange_weak(state, Startup::starting,
                                              std::memory_order_acq_rel)) {
                try {
                    worker = std::thread([this] { run(); });
                } catch (...) {
                    startup.store(Startup::setup, std::memory_order_release);
                    throw;
                }
                startup.store(Startup::running, std::memory_order_release);
                return;
            }
            std::this_thread::yield();
            state = startup.load(std::memory_order_acquire);
        }
    }

    bool controls() {
        auto* list = requests.exchange(nullptr, std::memory_order_acquire);
        if (!list) {
            return false;
        }
        Request* ordered = nullptr;
        while (list) {
            auto* next = list->next;
            list->next = ordered;
            ordered = list;
            list = next;
        }
        while (ordered) {
            auto* next = ordered->next;
            try {
                ordered->action(backend);
            } catch (...) {
                ordered->error = std::current_exception();
            }
            ordered->done.store(true, std::memory_order_release);
            ordered = next;
        }
        return true;
    }

    Channel& add(Timestamp time) {
        require(time >= frontier, "producer starts before completed recording time");
        require(registered.size() < unowned_producer, "producer identifier exhaustion");
        auto channel = std::make_unique<Channel>(static_cast<ProducerId>(registered.size()),
                                                 channels.load(std::memory_order_relaxed), time,
                                                 options.queue_bytes, options.pending_bytes);
        auto* result = channel.get();
        registered.push_back(std::move(channel));
        channels.store(result, std::memory_order_release);
        return *result;
    }

    bool is_selected() const {
        return selected.recording == this && selected.generation == generation;
    }

    Channel* attached() {
        if (is_selected()) {
            return selected.channel;
        }
        for (auto* channel = channels.load(std::memory_order_acquire); channel;
             channel = channel->next) {
            if (channel->writer.load(std::memory_order_relaxed) == thread_identity) {
                selected = {this, generation, channel};
                return channel;
            }
        }
        return nullptr;
    }

    Channel& local() {
        check();
        if (is_selected())
            return *selected.channel;
        start();
        if (auto* channel = attached())
            return *channel;
        Channel* channel = nullptr;
        invoke([&](Writer&) { channel = &add(frontier); });
        channel->writer.store(thread_identity, std::memory_order_release);
        selected = {this, generation, channel};
        return *channel;
    }

    std::uint8_t* reserve(Channel& channel, std::size_t bytes) {
        auto* destination = channel.incoming.reserve(bytes);
        if (destination)
            return destination;
        ++channel.waits;

        struct Wait {
            Channel& channel;
            std::chrono::steady_clock::time_point begin = std::chrono::steady_clock::now();

            ~Wait() {
                channel.wait_nanoseconds.fetch_add(
                    std::chrono::duration_cast<std::chrono::nanoseconds>(
                        std::chrono::steady_clock::now() - begin)
                        .count(),
                    std::memory_order_relaxed);
            }
        } wait{channel};

        unsigned spins = 0;
        do {
            check();
            if (!spins++)
                std::this_thread::yield();
            else if ((spins & 255) == 0)
                std::this_thread::sleep_for(std::chrono::microseconds(1));
            else if ((spins & 63) == 0)
                std::this_thread::yield();
            else
                detail::relax();
            destination = channel.incoming.reserve(bytes);
        } while (!destination);
        return destination;
    }

    void append_small(Channel& channel, const Packet& packet) {
        require(!channel.producer_finished, "producer has finished");
        auto* destination = channel.incoming.reserve(sizeof(Packet));
        if (!destination)
            destination = reserve(channel, sizeof(Packet));
        std::memcpy(destination, &packet, sizeof(packet));
        channel.incoming.commit(sizeof(packet));
        channel.submitted.store(++channel.producer_submitted, std::memory_order_relaxed);
    }

    void append_record(Channel& channel, const RecordPacket& packet) {
        require(!channel.producer_finished, "producer has finished");
        auto* destination = channel.incoming.reserve(sizeof(RecordPacket));
        if (!destination)
            destination = reserve(channel, sizeof(RecordPacket));
        std::memcpy(destination, &packet, sizeof(packet));
        channel.incoming.commit(sizeof(packet));
        channel.submitted.store(++channel.producer_submitted, std::memory_order_relaxed);
    }

    RecordId create_record(Kind kind, Id generator, Timestamp time, RecordId parent) {
        try {
            auto& channel = local();
            const auto id = record_ids.acquire();
            require(id, "record identifier exhaustion");
            append_record(channel, {{kind, 0, 0, generator, parent}, id, time, 0});
            return id;
        } catch (...) {
            fail(std::current_exception());
            throw;
        }
    }

    void append_attribute(Channel& channel, RecordId id, Id key, std::uint8_t tag,
                          std::uint64_t value) {
        require(!channel.producer_finished, "producer has finished");
        auto* destination = channel.incoming.reserve(sizeof(AttributePacket));
        if (!destination)
            destination = reserve(channel, sizeof(AttributePacket));
        const AttributePacket packet{{Kind::scalar_attribute, tag, 0, key, value}, id};
        std::memcpy(destination, &packet, sizeof(packet));
        channel.incoming.commit(sizeof(packet));
        channel.submitted.store(++channel.producer_submitted, std::memory_order_relaxed);
    }

    template <class Payload = View>
    void append(Channel& channel, const Operation& operation, const Payload& data = {}) {
        require(!channel.producer_finished, "producer has finished");
        try {
            const bool small = small_packet(operation.kind);
            const auto data_bytes = [&] {
                if constexpr (std::is_same_v<Payload, EncodedPayload>)
                    return data.size;
                else
                    return data.size();
            }();
            const auto header_bytes = small ? sizeof(Packet) : sizeof(RecordPacket);
            require(data_bytes <= std::numeric_limits<std::size_t>::max() - header_bytes,
                    "recording operation is too large");
            const auto bytes = header_bytes + data_bytes;
            const bool oversized = bytes > channel.incoming.capacity();
            if (oversized) {
                while (channel.external_pending.load(std::memory_order_acquire)) {
                    check();
                    std::this_thread::yield();
                }
            }
            auto* destination = reserve(channel, oversized ? sizeof(Packet) : bytes);
            std::unique_ptr<std::uint8_t[]> overflow;
            if (oversized) {
                overflow = std::make_unique_for_overwrite<std::uint8_t[]>(bytes);
            }
            auto* output = oversized ? overflow.get() : destination;
            const RecordPacket packet{
                {operation.kind, operation.status, 0, operation.generator, operation.argument},
                operation.id,
                operation.time,
                data_bytes};
            std::memcpy(output, &packet, header_bytes);
            if constexpr (std::is_same_v<Payload, EncodedPayload>) {
                data.copy_to({reinterpret_cast<std::byte*>(output + header_bytes), data_bytes});
            } else if (data_bytes) {
                std::memcpy(output + header_bytes, data.data(), data_bytes);
            }
            if (oversized) {
                const Packet external{Kind::external, 0, 0, 0,
                                      reinterpret_cast<std::uintptr_t>(overflow.get())};
                std::memcpy(destination, &external, sizeof(external));
                overflow.release();
                channel.external_pending.store(true, std::memory_order_relaxed);
            }
            channel.incoming.commit(oversized ? sizeof(Packet) : bytes);
            channel.submitted.store(++channel.producer_submitted, std::memory_order_relaxed);
        } catch (...) {
            fail(std::current_exception());
            throw;
        }
    }

    template <class Payload = View> void append(Operation operation, const Payload& data = {}) {
        append(local(), operation, data);
    }

    template <class Payload = View> RecordId create(Operation operation, const Payload& data = {}) {
        try {
            // Attachment bounds concurrent ID reservations by the logical producer limit.
            auto& channel = local();
            const auto id = record_ids.acquire();
            require(id, "record identifier exhaustion");
            operation.id = id;
            append(channel, operation, data);
            return id;
        } catch (...) {
            fail(std::current_exception());
            throw;
        }
    }

    void spool_write(Channel& channel, View data) {
        if (!channel.spool) {
            channel.spool = std::tmpfile();
            require(channel.spool, "cannot create pending recording spool");
        }
        require(::fseeko(channel.spool, channel.write_offset, SEEK_SET) == 0,
                "cannot seek pending recording spool");
        require(std::fwrite(data.data(), 1, data.size(), channel.spool) == data.size(),
                "cannot write pending recording spool");
        channel.write_offset += static_cast<off_t>(data.size());
        counters.spilled_bytes += data.size();
    }

    void stage(Channel& channel, View packet) {
        if (channel.spool || packet.size() > options.pending_bytes - pending_bytes) {
            if (channel.pending_read < channel.pending.size()) {
                auto data = View(channel.pending).subspan(channel.pending_read);
                spool_write(channel, data);
                pending_bytes -= data.size();
                channel.pending.clear();
                channel.pending_read = 0;
            }
            spool_write(channel, packet);
            return;
        }
        if (channel.pending_read &&
            channel.pending.size() + packet.size() > channel.pending.capacity()) {
            const auto bytes = channel.pending.size() - channel.pending_read;
            std::memmove(channel.pending.data(), channel.pending.data() + channel.pending_read,
                         bytes);
            channel.pending.resize(bytes);
            channel.pending_read = 0;
        }
        channel.pending.insert(channel.pending.end(), packet.begin(), packet.end());
        pending_bytes += packet.size();
        counters.peak_pending_bytes = std::max(counters.peak_pending_bytes, pending_bytes);
    }

    void execute_packet(View data, Channel& channel) {
        const auto header = packet_header(data.data());
        if (header.kind == Kind::scalar) {
            touch_signal(header.generator, channel.id, channel.play_time);
            backend.emit(header.generator, header.argument);
        } else if (header.kind == Kind::scalar_attribute) {
            AttributePacket packet;
            std::memcpy(&packet, data.data(), sizeof(packet));
            touch_record(packet.id, channel.id, channel.play_time);
            if (header.status == static_cast<std::uint8_t>(vtr::detail::wire::Tag::u64)) {
                backend.attribute(packet.id, header.generator, header.argument);
                return;
            }
            Value value;
            value.tag = static_cast<vtr::detail::wire::Tag>(header.status);
            if (value.tag == vtr::detail::wire::Tag::str)
                value.literal = static_cast<Id>(header.argument);
            else
                value.number = header.argument;
            backend.attribute(packet.id, header.generator, value);
        } else {
            Operation operation{header.kind, header.status, header.generator};
            operation.argument = header.argument;
            if (!small_packet(header.kind)) {
                RecordPacket packet;
                std::memcpy(&packet, data.data(), sizeof(packet));
                operation.id = packet.id;
                operation.time = packet.time;
                data = data.subspan(sizeof(packet));
            } else {
                data = {};
            }
            execute(operation, data, channel.id, channel.play_time);
        }
    }

    bool collect(const std::vector<std::uint64_t>* limits = nullptr) {
        bool progress = false;
        for (auto& pointer : registered) {
            auto& channel = *pointer;
            const bool direct = registered.size() == 1 && channel.pending.empty() && !channel.spool;
            std::size_t collected = 0;
            while (collected < drain_bytes &&
                   (!limits || channel.incoming.consumed() != (*limits)[channel.id])) {
                const auto* raw = channel.incoming.peek();
                if (!raw)
                    break;
                auto header = packet_header(raw);
                const auto queue_bytes = packet_size(raw, header.kind);
                std::unique_ptr<std::uint8_t[]> external;
                if (header.kind == Kind::external) {
                    external.reset(reinterpret_cast<std::uint8_t*>(header.argument));
                    raw = external.get();
                    header = packet_header(raw);
                }
                const View data(raw, packet_size(raw, header.kind));
                try {
                    if (!failed.load(std::memory_order_acquire)) {
                        if (header.kind == Kind::advance) {
                            require(header.argument >= channel.watermark,
                                    "published recording time moved backward");
                            channel.last_time = channel.watermark = header.argument;
                        }
                        if (header.kind == Kind::finish) {
                            channel.finished = true;
                        } else if (direct) {
                            if (header.kind == Kind::advance) {
                                channel.play_time = channel.watermark;
                                backend.set_time(channel.play_time);
                                serialized_time = channel.play_time;
                            } else {
                                if (serialized_time != channel.play_time) {
                                    backend.set_time(channel.play_time);
                                    serialized_time = channel.play_time;
                                }
                                execute_packet(data, channel);
                            }
                        } else {
                            stage(channel, data);
                        }
                    }
                } catch (...) {
                    channel.incoming.consume(queue_bytes);
                    channel.incoming.release();
                    if (external) {
                        external.reset();
                        channel.external_pending.store(false, std::memory_order_release);
                    }
                    throw;
                }
                channel.incoming.consume(queue_bytes);
                if (external) {
                    external.reset();
                    channel.external_pending.store(false, std::memory_order_release);
                }
                ++channel.received;
                ++counters.completed;
                collected += queue_bytes;
                progress = true;
            }
            channel.incoming.release();
        }
        return progress;
    }

    View pending_head(Channel& channel) {
        if (channel.pending_read < channel.pending.size()) {
            const auto* data = channel.pending.data() + channel.pending_read;
            return {data, packet_size(data, packet_header(data).kind)};
        }
        if (channel.replay_ready)
            return channel.replay;
        if (channel.read_offset == channel.write_offset) {
            if (channel.spool) {
                require(std::fclose(channel.spool) == 0, "cannot close pending recording spool");
                channel.spool = nullptr;
                channel.read_offset = channel.write_offset = 0;
            }
            return {};
        }
        require(::fseeko(channel.spool, channel.read_offset, SEEK_SET) == 0,
                "cannot seek pending recording spool");
        channel.replay.resize(sizeof(RecordPacket));
        require(std::fread(channel.replay.data(), 1, sizeof(Packet), channel.spool) ==
                    sizeof(Packet),
                "cannot read pending recording spool");
        const auto kind = packet_header(channel.replay.data()).kind;
        const auto header = packet_header_bytes(kind);
        if (header > sizeof(Packet)) {
            const auto tail = header - sizeof(Packet);
            require(std::fread(channel.replay.data() + sizeof(Packet), 1, tail, channel.spool) ==
                        tail,
                    "cannot read pending recording spool");
        }
        const auto bytes = packet_size(channel.replay.data(), kind);
        channel.replay.resize(bytes);
        require(std::fread(channel.replay.data() + header, 1, bytes - header, channel.spool) ==
                    bytes - header,
                "cannot read pending recording spool");
        channel.read_offset += static_cast<off_t>(bytes);
        channel.replay_ready = true;
        return channel.replay;
    }

    void consume_pending(Channel& channel, std::size_t bytes) {
        if (channel.pending_read < channel.pending.size()) {
            channel.pending_read += bytes;
            pending_bytes -= bytes;
            if (channel.pending_read == channel.pending.size()) {
                channel.pending.clear();
                channel.pending_read = 0;
            }
        } else {
            channel.replay_ready = false;
        }
    }

    std::optional<Timestamp> head_time(Channel& channel) {
        for (;;) {
            auto data = pending_head(channel);
            if (data.empty())
                return {};
            const auto header = packet_header(data.data());
            if (header.kind != Kind::advance)
                return channel.play_time;
            channel.play_time = header.argument;
            consume_pending(channel, data.size());
        }
    }

    void touch(Touch& previous, ProducerId producer, Timestamp time) {
        require(previous.producer == unowned_producer || previous.time != time ||
                    previous.producer == producer,
                "multiple tracing owners at one recording timestamp");
        previous = {time, producer};
    }

    void touch_record(RecordId id, ProducerId producer, Timestamp time) {
        auto* found = records.find(id);
        require(found, "record is not active");
        if (found->owner) {
            found = records.find(found->owner);
            require(found, "record owner is not active");
        }
        touch(found->touch, producer, time);
    }

    void touch_signal(Id signal, ProducerId producer, Timestamp time) {
        if (signals.size() <= signal) {
            backend.signal_kind(signal);
            signals.resize(static_cast<std::size_t>(signal) + 1);
        }
        touch(signals[signal], producer, time);
    }

    void execute(const Operation& op, View data, ProducerId producer, Timestamp time) {
        using wire_detail::Cursor;
        switch (op.kind) {
        case Kind::scalar:
            touch_signal(op.generator, producer, time);
            backend.emit(op.generator, op.argument);
            break;
        case Kind::span: {
            for (std::size_t offset = 0; offset < data.size(); offset += sizeof(SignalUpdate)) {
                SignalUpdate update;
                std::memcpy(&update, data.data() + offset, sizeof(update));
                touch_signal(update.signal, producer, time);
                backend.emit(update.signal, update.value);
            }
            break;
        }
        case Kind::packed:
        case Kind::real:
        case Kind::bytes:
            touch_signal(op.generator, producer, time);
            if (op.kind == Kind::packed) {
                if (op.argument) {
                    const auto kind = backend.signal_kind(op.generator);
                    require(kind.type == SignalKind::bits && kind.width == op.argument,
                            "input word width differs from signal declaration");
                }
                const auto states =
                    op.status ? op.status : backend.signal_kind(op.generator).states;
                backend.emit_packed(op.generator, states, data);
            } else if (op.kind == Kind::real) {
                backend.emit_real(op.generator, std::bit_cast<double>(op.argument));
            } else {
                backend.emit_varlen(op.generator, data);
            }
            break;
        case Kind::dump:
            if (dump_owner) {
                touch(*dump_owner, producer, time);
            } else {
                dump_owner = Touch{time, producer};
            }
            backend.dump(op.status != 0);
            break;
        case Kind::begin:
        case Kind::begin_value:
            if (op.argument) {
                touch_record(op.argument, producer, time);
            }
            backend.assign_record_id(op.id);
            if (op.kind == Kind::begin) {
                backend.begin(op.generator, op.time, op.argument);
            } else {
                backend.begin(op.generator, op.time, data, op.argument);
            }
            records.emplace(op.id, RecordOwner{0, {time, producer}, {}});
            break;
        case Kind::stage:
        case Kind::stage_value:
            touch_record(op.argument, producer, time);
            backend.assign_record_id(op.id);
            if (op.kind == Kind::stage) {
                backend.begin_stage(op.generator, op.argument, op.time);
            } else {
                backend.begin_stage(op.generator, op.argument, op.time, data);
            }
            records.emplace(op.id, RecordOwner{op.argument, {time, producer}, {}});
            records.at(op.argument).stages.push_back(op.id);
            break;
        case Kind::event:
        case Kind::log:
            if (op.argument) {
                touch_record(op.argument, producer, time);
            }
            backend.assign_record_id(op.id);
            if (op.kind == Kind::event) {
                backend.event(op.generator, op.argument, op.time, data);
            } else {
                require(data.size() >= 4, "missing captured callsite");
                auto callsite = wire_detail::load<std::uint32_t>(data);
                backend.log(op.generator, callsite, op.time, data.subspan(4), op.argument);
            }
            break;
        case Kind::payload:
        case Kind::stage_payload:
            touch_record(op.id, producer, time);
            if (op.kind == Kind::payload) {
                backend.payload(op.id, data);
            } else {
                backend.stage_payload(op.id, data);
            }
            break;
        case Kind::end:
        case Kind::end_value: {
            touch_record(op.id, producer, time);
            if (op.kind == Kind::end) {
                backend.end(op.id, op.time, op.status);
            } else {
                backend.end_captured(op.id, op.time, data, op.status);
            }
            auto stages = std::move(records.at(op.id).stages);
            for (auto stage : stages) {
                records.erase(stage);
            }
            records.erase(op.id);
            break;
        }
        case Kind::stage_end: {
            touch_record(op.id, producer, time);
            backend.end_stage(op.id, op.time);
            auto& stages = records.at(records.at(op.id).owner).stages;
            std::erase(stages, op.id);
            records.erase(op.id);
            break;
        }
        case Kind::attribute: {
            touch_record(op.id, producer, time);
            Cursor cursor(data);
            auto value = wire_detail::decode_value(cursor);
            cursor.done();
            backend.attribute(op.id, op.generator, value);
            break;
        }
        case Kind::classify:
            touch_record(op.id, producer, time);
            backend.span_kind(op.id, op.status);
            break;
        case Kind::relate: {
            Cursor cursor(data);
            auto attributes = wire_detail::decode_attrs(cursor);
            backend.relate(op.generator, op.id, op.argument, attributes);
            break;
        }
        case Kind::embedded_event:
        case Kind::embedded_stage: {
            touch_record(op.argument, producer, time);
            Cursor cursor(data);
            backend.assign_record_id(op.id);
            if (op.kind == Kind::embedded_event) {
                backend.embedded_event(op.argument, op.time, op.generator,
                                       wire_detail::decode_attrs(cursor));
            } else {
                const auto lane = cursor.fixed<Id>();
                std::optional<Timestamp> end;
                if (op.status) {
                    end = cursor.fixed<Timestamp>();
                }
                backend.embedded_stage(op.argument, op.generator, lane, op.time, end,
                                       wire_detail::decode_attrs(cursor));
            }
            break;
        }
        case Kind::embedded_end:
            touch_record(op.argument, producer, time);
            backend.embedded_stage_end(op.argument, op.id, op.time);
            break;
        case Kind::clock_run:
        case Kind::clock_stop: {
            auto [found, inserted] = clocks.try_emplace(op.generator, Touch{time, producer});
            if (!inserted) {
                touch(found->second, producer, time);
            }
            if (op.kind == Kind::clock_run) {
                backend.assign_record_id(op.id);
                backend.clock_run(op.generator, op.time, op.argument);
            } else {
                backend.clock_stop(op.generator, op.time);
            }
            break;
        }
        case Kind::advance:
        case Kind::finish:
        case Kind::scalar_attribute:
        case Kind::external:
            throw vtr::detail::wire::Error("invalid recording marker execution");
        }
    }

    bool serialize(std::size_t budget = drain_bytes) {
        try {
            all_finished = !registered.empty();
            Timestamp last_time = 0;
            auto next_frontier = std::numeric_limits<Timestamp>::max();
            for (const auto& channel : registered) {
                last_time = std::max(last_time, channel->last_time);
                if (!channel->finished) {
                    all_finished = false;
                    next_frontier = std::min(next_frontier, channel->watermark);
                }
            }
            if (registered.empty()) {
                return false;
            }
            frontier = next_frontier;
            bool progress = false;
            for (;;) {
                Channel* selected_channel = nullptr;
                Timestamp next_time = std::numeric_limits<Timestamp>::max();
                for (const auto& channel : registered) {
                    auto time = head_time(*channel);
                    if (time && (all_finished || *time < frontier) &&
                        (!selected_channel || *time < next_time)) {
                        selected_channel = channel.get();
                        next_time = *time;
                    }
                }
                if (!selected_channel) {
                    const auto time = all_finished ? last_time : frontier;
                    if (time > serialized_time) {
                        backend.set_time(time);
                        serialized_time = time;
                        progress = true;
                    }
                    return progress;
                }
                if (serialized_time != next_time) {
                    backend.set_time(next_time);
                    serialized_time = next_time;
                }
                do {
                    const auto data = pending_head(*selected_channel);
                    execute_packet(data, *selected_channel);
                    const auto bytes = data.size();
                    consume_pending(*selected_channel, bytes);
                    progress = true;
                    if (bytes >= budget)
                        return true;
                    budget -= bytes;
                } while (head_time(*selected_channel) == next_time);
            }
        } catch (...) {
            fail(std::current_exception());
            throw;
        }
    }

    void run() noexcept {
        const detail::ThreadCpu::Measurement measurement{worker_cpu};
        backend_recording = this;
        unsigned idle_rounds = 0;
        for (;;) {
            bool progress = controls();
            if (stopped.load()) {
                if (!active_requests.load()) {
                    break;
                }
                std::this_thread::yield();
                continue;
            }
            try {
                progress = collect() || progress;
                if (!failed.load(std::memory_order_acquire)) {
                    // Match ingestion so balanced producers do not force an artificial spill.
                    progress = serialize(drain_bytes * registered.size()) || progress;
                }
            } catch (...) {
                fail(std::current_exception());
            }
            if (!progress) {
                if (++idle_rounds < 64) {
                    std::this_thread::yield();
                } else {
                    std::this_thread::sleep_for(std::chrono::microseconds(10));
                }
            } else {
                idle_rounds = 0;
            }
        }
        backend_recording = nullptr;
    }
};

Stream::Stream(const std::string& path, WriterOptions options, StreamOptions stream)
    : impl_(std::make_unique<Impl>(path, options, stream)) {}

Stream::~Stream() {
    try {
        seal();
    } catch (...) {
        impl_->stopped.store(true);
        if (impl_->worker.joinable()) {
            impl_->worker.join();
        }
    }
    if (impl_->is_selected()) {
        selected = {};
    }
}

const TraceId& Stream::trace_id() const noexcept {
    return impl_->backend.trace_id();
}

void Stream::check() const {
    impl_->check();
}

void Stream::invoke(const std::function<void(Writer&)>& action) {
    impl_->invoke(action);
}

Stream::ProducerId Stream::add_producer(Timestamp start) {
    ProducerId id = 0;
    invoke([&](Writer&) {
        impl_->check();
        id = impl_->add(start).id;
    });
    return id;
}

void Stream::use_producer(ProducerId id) {
    impl_->check();
    impl_->start();
    auto* previous = impl_->attached();
    Channel* target = impl_->channels.load(std::memory_order_acquire);
    while (target && target->id != id) {
        target = target->next;
    }
    require(target, "unknown recording producer");
    if (target->writer.load(std::memory_order_acquire) == thread_identity) {
        selected = {impl_.get(), impl_->generation, target};
        return;
    }
    std::uint64_t expected = 0;
    require(target->writer.compare_exchange_strong(expected, thread_identity,
                                                   std::memory_order_acquire),
            "producer is attached to another thread");
    if (target->producer_finished) {
        target->writer.store(0, std::memory_order_release);
        throw vtr::detail::wire::Error("producer has finished");
    }
    try {
        if (previous) {
            release_producer();
        }
    } catch (...) {
        target->writer.store(0, std::memory_order_release);
        throw;
    }
    selected = {impl_.get(), impl_->generation, target};
}

void Stream::release_producer() {
    if (impl_->attached()) {
        selected.channel->writer.store(0, std::memory_order_release);
        selected = {};
    }
}

void Stream::advance_time(Timestamp time) {
    auto& channel = impl_->local();
    require(!channel.producer_finished, "producer has finished");
    require(time >= channel.producer_time, "recording time moved backward");
    if (time != channel.producer_time) {
        impl_->append_small(channel, {Kind::advance, 0, 0, 0, time});
        channel.producer_time = time;
    }
}

Timestamp Stream::time() {
    return impl_->local().producer_time;
}

void Stream::publish() {
    (void)impl_->local();
}

void Stream::finish_producer() {
    auto& channel = impl_->local();
    impl_->append_small(channel, {Kind::finish});
    channel.producer_finished = true;
    channel.writer.store(0, std::memory_order_release);
    selected = {};
}

void Stream::flush() {
    invoke([&](Writer& writer) {
        impl_->check();
        std::vector<std::uint64_t> limits;
        for (const auto& channel : impl_->registered) {
            limits.push_back(channel->incoming.published());
        }
        while (impl_->collect(&limits)) {
        }
        while (impl_->serialize()) {
        }
        writer.flush();
    });
}

void Stream::close() {
    if (impl_->stopped.load(std::memory_order_acquire)) {
        return;
    }
    if (impl_->attached()) {
        finish_producer();
    }
    invoke([&](Writer& writer) {
        impl_->check();
        for (auto& channel : impl_->registered) {
            require(channel->writer.load(std::memory_order_acquire) == 0,
                    "release recording producers before close");
        }
        while (impl_->collect()) {
        }
        for (auto& channel : impl_->registered) {
            channel->finished = true;
        }
        while (impl_->serialize()) {
        }
        writer.close();
        impl_->stopped.store(true);
    });
    if (impl_->worker.joinable()) {
        impl_->worker.join();
    }
}

void Stream::seal() {
    if (impl_->stopped.load(std::memory_order_acquire)) {
        return;
    }
    if (impl_->attached()) {
        if (!impl_->failed.load(std::memory_order_acquire)) {
            finish_producer();
        } else {
            selected.channel->writer.store(0, std::memory_order_release);
            selected = {};
        }
    }
    invoke([&](Writer& writer) {
        for (const auto& channel : impl_->registered) {
            require(channel->writer.load(std::memory_order_acquire) == 0,
                    "release recording producers before seal");
        }
        if (!impl_->failed.load(std::memory_order_acquire)) {
            while (impl_->collect()) {
            }
            for (auto& channel : impl_->registered) {
                channel->finished = true;
            }
            try {
                while (impl_->serialize()) {
                }
            } catch (...) {
                impl_->fail(std::current_exception());
            }
        }
        writer.seal();
        impl_->stopped.store(true);
    });
    if (impl_->worker.joinable()) {
        impl_->worker.join();
    }
}

WriterStats Stream::stats() {
    if (impl_->stopped.load(std::memory_order_acquire)) {
        return impl_->backend.stats();
    }
    return control([](Writer& writer) { return writer.stats(); });
}

std::optional<std::uint64_t> Stream::backend_cpu_nanoseconds() const {
    const auto collector = impl_->worker_cpu.nanoseconds();
    const auto encoders = impl_->backend.handoff_stats().worker_cpu_nanoseconds;
    return collector && encoders ? std::optional(*collector + *encoders) : std::nullopt;
}

StreamStats Stream::stream_stats() {
    StreamStats result;
    auto read = [&] {
        result = impl_->counters;
        result.worker_cpu_nanoseconds = impl_->worker_cpu.nanoseconds();
        for (const auto& channel : impl_->registered) {
            result.submitted +=
                std::max(channel->submitted.load(std::memory_order_relaxed), channel->received);
            result.waits += channel->waits.load(std::memory_order_relaxed);
            result.wait_nanoseconds += channel->wait_nanoseconds.load(std::memory_order_relaxed);
        }
    };
    if (impl_->stopped.load(std::memory_order_acquire)) {
        read();
    } else {
        invoke([&](Writer&) { read(); });
    }
    return result;
}

HandoffStats Stream::handoff_stats() {
    if (impl_->stopped.load(std::memory_order_acquire)) {
        return impl_->backend.handoff_stats();
    }
    return control([](Writer& writer) { return writer.handoff_stats(); });
}

void Stream::emit(Id signal, std::uint64_t value) {
    impl_->append_small(impl_->local(), {Kind::scalar, 0, 0, signal, value});
}

void Stream::emit(std::span<const SignalUpdate> updates) {
    impl_->append({Kind::span},
                  {reinterpret_cast<const std::uint8_t*>(updates.data()), updates.size_bytes()});
}

void Stream::emit_packed(Id signal, std::uint8_t states, View value) {
    impl_->append({Kind::packed, states, signal}, value);
}

void Stream::emit_packed(Id signal, std::uint8_t states, const EncodedPayload& value,
                         std::uint32_t width) {
    impl_->append({Kind::packed, states, signal, 0, 0, width}, value);
}

void Stream::emit_real(Id signal, double value) {
    impl_->append_record(impl_->local(),
                         {{Kind::real, 0, 0, signal, std::bit_cast<std::uint64_t>(value)}});
}

void Stream::emit_varlen(Id signal, View value) {
    impl_->append({Kind::bytes, 0, signal}, value);
}

void Stream::dump(bool enabled) {
    impl_->append_small(impl_->local(), {Kind::dump, std::uint8_t(enabled)});
}

RecordId Stream::begin(GeneratorId generator, Timestamp time, RecordId parent) {
    return impl_->create_record(Kind::begin, generator, time, parent);
}

RecordId Stream::begin(GeneratorId generator, Timestamp time, View payload, RecordId parent) {
    return impl_->create({Kind::begin_value, 0, generator, 0, time, parent}, payload);
}

RecordId Stream::begin_stage(GeneratorId generator, RecordId owner, Timestamp time) {
    return impl_->create_record(Kind::stage, generator, time, owner);
}

RecordId Stream::begin_stage(GeneratorId generator, RecordId owner, Timestamp time, View payload) {
    return impl_->create({Kind::stage_value, 0, generator, 0, time, owner}, payload);
}

RecordId Stream::event(GeneratorId generator, RecordId owner, Timestamp time, View payload) {
    return impl_->create({Kind::event, 0, generator, 0, time, owner}, payload);
}

RecordId Stream::log(GeneratorId generator, std::uint32_t callsite, Timestamp time, View payload,
                     RecordId owner) {
    struct Capture {
        std::uint32_t callsite;
        EncodedPayload payload;
    };

    const Capture capture{callsite, {payload.size(), payload.data()}};
    require(capture.payload.size <= SIZE_MAX - 4, "log capture exceeds addressable storage");
    const EncodedPayload encoded{capture.payload.size + 4, &capture,
                                 [](std::span<std::byte> output, const void* source) {
                                     const auto& capture = *static_cast<const Capture*>(source);
                                     for (unsigned byte = 0; byte != 4; ++byte)
                                         output[byte] = std::byte(capture.callsite >> (byte * 8));
                                     capture.payload.copy_to(output.subspan(4));
                                 }};
    return impl_->create({Kind::log, 0, generator, 0, time, owner}, encoded);
}

void Stream::payload(RecordId record, View data) {
    impl_->append({Kind::payload, 0, 0, record}, data);
}

void Stream::stage_payload(RecordId record, View data) {
    impl_->append({Kind::stage_payload, 0, 0, record}, data);
}

void Stream::end(RecordId record, Timestamp time, std::uint8_t status) {
    impl_->append_record(impl_->local(), {{Kind::end, status}, record, time});
}

void Stream::end_captured(RecordId record, Timestamp time, View data, std::uint8_t status) {
    impl_->append({Kind::end_value, status, 0, record, time}, data);
}

RecordId Stream::begin(GeneratorId generator, Timestamp time, const EncodedPayload& payload,
                       RecordId parent) {
    return impl_->create({Kind::begin_value, 0, generator, 0, time, parent}, payload);
}

RecordId Stream::begin_stage(GeneratorId generator, RecordId owner, Timestamp time,
                             const EncodedPayload& payload) {
    return impl_->create({Kind::stage_value, 0, generator, 0, time, owner}, payload);
}

RecordId Stream::event(GeneratorId generator, RecordId owner, Timestamp time,
                       const EncodedPayload& payload) {
    return impl_->create({Kind::event, 0, generator, 0, time, owner}, payload);
}

RecordId Stream::log(GeneratorId generator, std::uint32_t callsite, Timestamp time,
                     const EncodedPayload& payload, RecordId owner) {
    struct Capture {
        std::uint32_t callsite;
        EncodedPayload payload;
    };

    const Capture capture{callsite, payload};
    require(capture.payload.size <= SIZE_MAX - 4, "log capture exceeds addressable storage");
    const EncodedPayload encoded{capture.payload.size + 4, &capture,
                                 [](std::span<std::byte> output, const void* source) {
                                     const auto& capture = *static_cast<const Capture*>(source);
                                     for (unsigned byte = 0; byte != 4; ++byte)
                                         output[byte] = std::byte(capture.callsite >> (byte * 8));
                                     capture.payload.copy_to(output.subspan(4));
                                 }};
    return impl_->create({Kind::log, 0, generator, 0, time, owner}, encoded);
}

void Stream::payload(RecordId record, const EncodedPayload& payload) {
    impl_->append({Kind::payload, 0, 0, record}, payload);
}

void Stream::stage_payload(RecordId record, const EncodedPayload& payload) {
    impl_->append({Kind::stage_payload, 0, 0, record}, payload);
}

void Stream::end_captured(RecordId record, Timestamp time, const EncodedPayload& payload,
                          std::uint8_t status) {
    impl_->append({Kind::end_value, status, 0, record, time}, payload);
}

void Stream::end_stage(RecordId record, Timestamp time) {
    impl_->append_record(impl_->local(), {{Kind::stage_end}, record, time});
}

void Stream::attribute(RecordId owner, Id key, std::uint64_t value) {
    impl_->append_attribute(impl_->local(), owner, key,
                            static_cast<std::uint8_t>(vtr::detail::wire::Tag::u64), value);
}

void Stream::attribute(RecordId owner, Id key, const Value& value) {
    if (value.tag <= vtr::detail::wire::Tag::str || value.tag == vtr::detail::wire::Tag::time ||
        value.tag == vtr::detail::wire::Tag::pointer) {
        impl_->append_attribute(impl_->local(), owner, key, static_cast<std::uint8_t>(value.tag),
                                value.tag == vtr::detail::wire::Tag::str ? value.literal
                                                                         : value.number);
        return;
    }
    Bytes data;
    wire_detail::encode_value(data, value);
    impl_->append({Kind::attribute, 0, key, owner}, data);
}

void Stream::span_kind(RecordId owner, std::uint8_t kind) {
    impl_->append_record(impl_->local(), {{Kind::classify, kind}, owner});
}

void Stream::relate(Id kind, RecordId from, RecordId to,
                    std::span<const vtr::detail::wire::Attribute> attributes) {
    if (attributes.empty()) {
        const std::array<std::uint8_t, 1> data{};
        impl_->append({Kind::relate, 0, kind, from, 0, to}, data);
        return;
    }
    Bytes data;
    wire_detail::encode_attrs(data, attributes);
    impl_->append({Kind::relate, 0, kind, from, 0, to}, data);
}

RecordId Stream::embedded_event(RecordId owner, Timestamp time, Id name,
                                std::span<const vtr::detail::wire::Attribute> attributes) {
    if (attributes.empty()) {
        const std::array<std::uint8_t, 1> data{};
        return impl_->create({Kind::embedded_event, 0, name, 0, time, owner}, data);
    }
    Bytes data;
    wire_detail::encode_attrs(data, attributes);
    return impl_->create({Kind::embedded_event, 0, name, 0, time, owner}, data);
}

RecordId Stream::embedded_stage(RecordId owner, Id name, Id lane, Timestamp begin,
                                std::optional<Timestamp> end,
                                std::span<const vtr::detail::wire::Attribute> attributes) {
    if (attributes.empty()) {
        std::array<std::uint8_t, sizeof(Id) + sizeof(Timestamp) + 1> data{};
        for (unsigned i = 0; i < sizeof(Id); ++i) {
            data[i] = static_cast<std::uint8_t>(lane >> (i * 8));
        }
        if (end) {
            for (unsigned i = 0; i < sizeof(Timestamp); ++i) {
                data[sizeof(Id) + i] = static_cast<std::uint8_t>(*end >> (i * 8));
            }
        }
        const auto size = sizeof(Id) + (end ? sizeof(Timestamp) : 0) + 1;
        return impl_->create(
            {Kind::embedded_stage, std::uint8_t(end.has_value()), name, 0, begin, owner},
            std::span<const std::uint8_t>(data.data(), size));
    }
    Bytes data;
    wire_detail::fixed(data, lane);
    if (end) {
        wire_detail::fixed(data, *end);
    }
    wire_detail::encode_attrs(data, attributes);
    return impl_->create(
        {Kind::embedded_stage, std::uint8_t(end.has_value()), name, 0, begin, owner}, data);
}

void Stream::embedded_stage_end(RecordId owner, RecordId stage, Timestamp time) {
    impl_->append({Kind::embedded_end, 0, 0, stage, time, owner});
}

RecordId Stream::clock_run(GeneratorId clock, Timestamp first, Timestamp period) {
    return impl_->create({Kind::clock_run, 0, clock, 0, first, period});
}

void Stream::clock_stop(GeneratorId clock, Timestamp time) {
    impl_->append({Kind::clock_stop, 0, clock, 0, time});
}

} // namespace vtr::detail::engine
