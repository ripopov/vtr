#include "engine/vtr.h"
#include "stream.h"
#include <algorithm>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>

using namespace vtr::detail::engine;
namespace fs = std::filesystem;
constexpr std::array<Timestamp, 12> probes{0, 1, 7, 33, 50, 74, 75, 500, 700, 701, 1999, 5000};

void check(bool good, const char* why) {
    if (!good) {
        throw std::runtime_error(why);
    }
}

void hash_byte(std::uint64_t& h, std::uint8_t b) {
    h = (h ^ b) * 1099511628211ull;
}

void hash_number(std::uint64_t& h, std::uint64_t n) {
    for (unsigned i = 0; i < 8; ++i) {
        hash_byte(h, n >> (8 * i));
    }
}

void hash_value(std::uint64_t& h, View bytes) {
    hash_number(h, bytes.size());
    for (auto b : bytes) {
        hash_byte(h, b);
    }
}

Bytes number(std::uint64_t value, std::size_t bytes) {
    Bytes data(bytes);
    for (std::size_t i = 0; i < bytes; ++i) {
        data[i] = value >> (8 * i);
    }
    return data;
}

Bytes initial(SignalKind kind) {
    if (kind.type == SignalKind::real) {
        return Bytes(8);
    }
    if (kind.type == SignalKind::varlen) {
        return {};
    }
    auto stride = kind.states == 2 ? 1u : kind.states == 4 ? 2u : 4u;
    Bytes data((std::size_t(kind.width) * stride + 7) / 8);
    if (kind.states != 2) {
        for (unsigned bit = 0; bit < kind.width; ++bit) {
            data[bit * stride / 8] |= 2 << ((bit * stride) % 8);
        }
    }
    return data;
}

Bytes packed(SignalKind kind, std::uint64_t seed, bool binary) {
    auto stride = kind.states == 2 ? 1u : kind.states == 4 ? 2u : 4u;
    Bytes data((std::size_t(kind.width) * stride + 7) / 8);
    for (unsigned bit = 0; bit < kind.width; ++bit) {
        auto code = binary ? ((seed >> (bit % 64)) & 1) : (seed + bit * 7) % kind.states;
        data[bit * stride / 8] |= code << ((bit * stride) % 8);
    }
    return data;
}

struct Signal {
    SignalKind kind;
    bool event, emitted = false;
    Bytes first, last;
    std::vector<Timestamp> times;
    std::vector<Bytes> values;

    Bytes at(Timestamp time) const {
        auto i = std::upper_bound(times.begin(), times.end(), time);
        return i == times.begin() ? first : values[std::size_t(i - times.begin() - 1)];
    }
};

template <class Recording, class F> decltype(auto) control(Recording& writer, F&& action) {
    if constexpr (std::is_same_v<Recording, Stream>) {
        return writer.control(std::forward<F>(action));
    } else {
        return action(writer);
    }
}

template <class Recording>
std::uint64_t run(const fs::path& path, vtr::detail::wire::Codec codec, unsigned workers) {
    std::vector<Signal> signals;
    std::uint64_t expected = 1469598103934665603ull;
    {
        Recording writer(path, {.codec = codec,
                                .compression_level = 1,
                                .wave_records = 37,
                                .wave_bytes = 512,
                                .group_size = 4,
                                .workers = workers,
                                .queue_jobs = 3,
                                .queue_bytes = 256 << 10});
        auto scope =
            control(writer, [](Writer& backend) { return backend.add_scope(none, "top"); });
        auto declare = [&](std::string name, SignalKind kind, bool event = false) {
            auto id = control(writer, [&](Writer& backend) {
                return backend
                    .add_var(scope, name, kind,
                             event                             ? 0
                             : kind.type == SignalKind::real   ? 3
                             : kind.type == SignalKind::varlen ? 21
                                                               : 16)
                    .second;
            });
            check(id == signals.size(), "signal allocation order changed");
            auto zero = initial(kind);
            signals.push_back({kind, event, false, zero, zero, {}, {}});
            return id;
        };
        for (std::uint8_t states : {2, 4, 9}) {
            for (unsigned width : {1, 13, 96}) {
                declare("s" + std::to_string(signals.size()), {SignalKind::bits, width, states});
            }
        }
        auto real = declare("real", {SignalKind::real, 0, 0}),
             bytes = declare("bytes", {SignalKind::varlen, 0, 0});
        auto event = declare("event", {SignalKind::bits, 1, 2}, true);
        auto copy_a = declare("copy_a", {SignalKind::bits, 17, 2});
        for (unsigned i = 0; i < 4; ++i) {
            declare("quiet" + std::to_string(i), {SignalKind::bits, 1, 2});
        }
        auto copy_b = declare("copy_b", {SignalKind::bits, 17, 2});
        Id late = none;
        control(writer, [&](Writer& backend) { backend.add_alias(scope, "alias", 0); });
        bool active = true;
        auto emit = [&](Id id, Timestamp time, Bytes data) {
            auto& signal = signals[id];
            if (signal.kind.type == SignalKind::bits) {
                writer.emit_packed(id, signal.kind.states, data);
            } else if (signal.kind.type == SignalKind::varlen) {
                writer.emit_varlen(id, data);
            } else {
                std::uint64_t bits = 0;
                for (unsigned i = 0; i < 8; ++i) {
                    bits |= std::uint64_t(data[i]) << (8 * i);
                }
                writer.emit_real(id, std::bit_cast<double>(bits));
            }
            if (active && (signal.event || !signal.emitted || signal.last != data)) {
                signal.times.push_back(time);
                signal.values.push_back(data);
                signal.last = data;
                signal.emitted = true;
            }
            std::fill(data.begin(), data.end(), 255);
        };
        for (Timestamp time = 0; time < 2000; ++time) {
            if constexpr (std::is_same_v<Recording, Stream>) {
                writer.advance_time(time);
            } else {
                writer.set_time(time);
            }
            if (time == 50 || time == 1000) {
                writer.dump(false);
                active = false;
            }
            if (time == 75 || time == 1050) {
                writer.dump(true);
                active = true;
            }
            if (time == 700) {
                late = declare("late", {SignalKind::bits, 8, 2});
            }
            for (Id id = 0; id < 9; ++id) {
                emit(id, time, packed(signals[id].kind, time * 17 + id * 11, time % 17 != 0));
            }
            auto real_bits = time % 23 == 0 ? 0x7ff8000000000100ull + time % 17
                             : time % 97 == 0
                                 ? 0x8000000000000000ull
                                 : std::bit_cast<std::uint64_t>(double(time % 7) * 0.25);
            emit(real, time, number(real_bits, 8));
            Bytes text(3 + time % 19);
            for (std::size_t i = 0; i < text.size(); ++i) {
                text[i] = (time + i * 71) & 255;
            }
            emit(bytes, time, text);
            emit(copy_a, time, packed(signals[copy_a].kind, time ? time + 1 : 0, true));
            emit(copy_b, time, packed(signals[copy_b].kind, time ? time + 1 : 1, true));
            if (time == 0) {
                for (Id id = copy_a + 1; id < copy_b; ++id) {
                    emit(id, time, Bytes{0});
                }
            }
            if (late != none) {
                emit(late, time, Bytes{std::uint8_t(time)});
            }
            emit(event, time, Bytes{1});
            if (time % 31 == 0) {
                writer.flush();
            }
            emit(event, time, Bytes{1});
            if (time == 0) {
                writer.flush();
            }
        }
        writer.close();
        auto capture = writer.stats();
        check(capture.wave_buffer_bytes == 0 && capture.peak_wave_buffer_bytes >= 512 &&
                  capture.peak_wave_buffer_bytes <= 568,
              "wave capture buffer exceeded one-observation allowance");
        auto stats = writer.handoff_stats();
        check(stats.accepted == stats.written && !stats.jobs && !stats.bytes &&
                  stats.peak_jobs <= 3,
              "wave worker did not drain within job bound");
        check(!workers || (stats.accepted > 100 && stats.peak_bytes <= (256 << 10)),
              "wave worker charge exceeded byte budget");
    }
    Reader reader(path, {.verify_checksums = true});
    check(!reader.validate().incomplete_file, "wave trace was not finalized");
    check(reader.signals().size() == signals.size() && reader.find_signal("top.alias") == 0,
          "late declaration or static alias changed");
    ContainerReader container(path);
    auto blackouts = container.waveform().blackouts();
    check(std::vector<std::pair<Timestamp, bool>>(blackouts.begin(), blackouts.end()) ==
              std::vector<std::pair<Timestamp, bool>>(
                  {{50, false}, {75, true}, {1000, false}, {1050, true}}),
          "blackout transitions changed");
    for (Id id = 0; id < signals.size(); ++id) {
        const auto& source = signals[id];
        auto history = reader.load_signal(id);
        check(history->initial == source.first && history->times == source.times,
              "wave initial value or occurrence times differ from producer");
        hash_number(expected, id);
        hash_value(expected, source.first);
        for (std::size_t i = 0; i < source.times.size(); ++i) {
            auto value = history->at(i);
            check(Bytes(value.data.begin(), value.data.end()) == source.values[i],
                  "wave state or float bits differ from producer");
            hash_number(expected, source.times[i]);
            hash_value(expected, source.values[i]);
        }
        for (auto time : probes) {
            reader.clear_cache();
            auto value = reader.value_at(id, time);
            check(value == source.at(time), "cold point query changed frame or predecessor");
            hash_value(expected, source.at(time));
        }
    }
    std::vector<std::size_t> positions(signals.size());
    reader.changes(0, UINT64_MAX, [&](Timestamp time, Id id, const SignalValue& value) {
        check(id < signals.size(), "unknown signal in bulk scan");
        auto i = positions[id]++;
        check(i < signals[id].times.size() && time == signals[id].times[i] &&
                  Bytes(value.data.begin(), value.data.end()) == signals[id].values[i],
              "bulk scan changed signal occurrence");
        return true;
    });
    for (Id id = 0; id < signals.size(); ++id) {
        check(positions[id] == signals[id].times.size(), "bulk scan lost signal occurrences");
    }
    return expected;
}

Bytes read(const fs::path& path) {
    std::ifstream f(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(f), {}};
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: wave_worker_test DIRECTORY");
        }
        fs::path dir = argv[1];
        fs::create_directories(dir);
        for (bool stream : {false, true}) {
            for (unsigned codec = 0; codec < 3; ++codec) {
                Bytes first;
                for (unsigned workers : {0, 2}) {
                    auto name = (stream ? "stream-wave-c" : "wave-c") + std::to_string(codec) +
                                "-w" + std::to_string(workers) + ".vtr";
                    auto path = dir / name;
                    auto digest =
                        stream ? run<Stream>(path, static_cast<vtr::detail::wire::Codec>(codec),
                                             workers)
                               : run<Writer>(path, static_cast<vtr::detail::wire::Codec>(codec),
                                             workers);
                    auto bytes = read(path);
                    std::fill(bytes.begin() + 16, bytes.begin() + 32, 0);
                    if (!workers) {
                        first = std::move(bytes);
                    } else if (!stream) {
                        // SPSC declarations may interleave with collection; direct assembly fixes
                        // boundaries.
                        check(bytes == first, "wave worker changed wire bytes");
                    }
                    std::cout << name << ' ' << std::hex << std::setfill('0') << std::setw(16)
                              << digest << std::dec << '\n';
                }
            }
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
