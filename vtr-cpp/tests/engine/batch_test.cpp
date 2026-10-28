#include "engine/vtr.h"
#include "payload.h"
#include <filesystem>
#include <fstream>
#include <iostream>
using namespace vtr::detail::engine;
namespace fs = std::filesystem;

void check(bool ok, const char* why) {
    if (!ok) {
        throw std::runtime_error(why);
    }
}

template <class F> void rejects(F&& f) {
    try {
        f();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("invalid batch/final payload accepted");
}

Bytes bytes(const fs::path& path) {
    std::ifstream f(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(f), {}};
}

void run(const fs::path& path, unsigned workers, bool batch) {
    Writer writer(path, {.codec = vtr::detail::wire::Codec::stored,
                         .wave_records = 19,
                         .wave_bytes = 256,
                         .group_size = 4,
                         .workers = workers,
                         .queue_jobs = 2,
                         .queue_bytes = 1 << 20});
    std::vector<Id> signals;
    for (std::uint8_t states : {2, 4, 9}) {
        for (unsigned width : {1, 13, 64}) {
            signals.push_back(writer
                                  .add_var(none, "s" + std::to_string(signals.size()),
                                           {SignalKind::bits, width, states})
                                  .second);
        }
    }
    auto event = writer.add_var(none, "event", {SignalKind::bits, 1, 2}, 0).second;
    auto type = writer.bind<std::uint64_t>();
    auto owner_gen = writer.add_generator(none, "owner", "TRANSACTION", type),
         stage_gen = writer.add_generator(none, "visit", "STAGE", type);
    auto owner = writer.begin(owner_gen, 0, capture(std::uint64_t(7)));
    auto stage = writer.begin_stage(stage_gen, owner, 1);
    rejects([&] { writer.end(owner, 2, capture(std::uint64_t(99))); });
    writer.stage_payload(stage, capture(std::uint64_t(11)));
    writer.end_stage(stage, 2);
    rejects([&] { writer.end(owner, 3, Bytes{1}); });
    rejects([&] { writer.end(owner, 3, capture(std::uint64_t(99)), 9); });
    for (unsigned time = 0; time < 200; ++time) {
        writer.set_time(time);
        std::vector<SignalUpdate> updates;
        for (auto id : signals) {
            updates.push_back({id, std::uint64_t(time) * 17 + id});
        }
        updates.push_back({event, 1});
        updates.push_back({event, 1});
        if (batch) {
            auto invalid = updates;
            invalid.back().signal = 9999;
            rejects([&] { writer.emit(invalid); });
            writer.emit(updates);
        } else {
            for (auto update : updates) {
                writer.emit(update.signal, update.value);
            }
        }
        for (auto& update : updates) {
            update.value = UINT64_MAX;
        }
        if (time % 31 == 0) {
            writer.flush();
        }
    }
    writer.end(owner, 201, capture(std::uint64_t(42)));
    writer.close();
    Reader reader(path, {.verify_checksums = true});
    check(!reader.validate().incomplete_file, "batch trace incomplete");
    check(reader.transaction(owner)->payload == capture(std::uint64_t(42)),
          "moving final payload lost value");
    check(reader.load_signal(event)->times.size() == 400, "batch coalesced event occurrences");
    for (unsigned time : {0, 1, 17, 199}) {
        check(reader.value_at(2, time) == capture(std::uint64_t(std::uint64_t(time) * 17 + 2)),
              "batch scalar history changed");
    }
}

int main(int argc, char** argv) {
    try {
        check(argc == 2, "usage: batch_test DIRECTORY");
        fs::path dir = argv[1];
        fs::create_directories(dir);
        Bytes first;
        for (unsigned workers : {0, 2}) {
            for (bool batch : {false, true}) {
                auto path = dir / ("batch-" + std::to_string(workers) + "-" +
                                   std::to_string(batch) + ".vtr");
                run(path, workers, batch);
                auto data = bytes(path);
                std::fill(data.begin() + 16, data.begin() + 32, 0);
                if (first.empty()) {
                    first = std::move(data);
                } else {
                    check(data == first, "batch or workers changed observation bytes");
                }
            }
        }
        std::cout << "Scalar/batch wire equivalence, preflight rejection, repeated "
                     "events, bounded splits and moved final payload validated\n";
    } catch (const std::exception& e) {
        std::cerr << e.what() << '\n';
        return 1;
    }
}
