#include "engine/activity.h"
#include "engine/vtr.h"
#include "format.h"
#include <filesystem>
#include <fstream>
#include <iostream>
#include <source_location>

using namespace vtr::detail::engine;
namespace fs = std::filesystem;
namespace d = vtr::detail::wire::detail;

void check(bool good, const char* why) {
    if (!good) {
        throw std::runtime_error(why);
    }
}

template <class F>
void rejects(F&& action, std::source_location at = std::source_location::current()) {
    try {
        action();
    } catch (const vtr::detail::wire::Error&) {
        return;
    }
    throw std::runtime_error("invalid activity input accepted at line " +
                             std::to_string(at.line()));
}

Bytes read(const fs::path& path) {
    std::ifstream f(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(f), {}};
}

void write(const fs::path& path, View bytes) {
    std::ofstream f(path, std::ios::binary);
    f.write(reinterpret_cast<const char*>(bytes.data()), bytes.size());
    check(bool(f), "fixture write failed");
}

void set_u32(Bytes& bytes, std::size_t at, std::uint32_t value) {
    for (unsigned i = 0; i < 4; ++i) {
        bytes[at + i] = value >> (i * 8);
    }
}

void identity(const fs::path& dir) {
    TraceId recordings[2];
    TraceFingerprint fingerprints[2];
    Bytes sidecar;
    for (unsigned i = 0; i < 2; ++i) {
        auto path = dir / ("identity-" + std::to_string(i) + ".vtr");
        {
            Writer w(path, {.codec = vtr::detail::wire::Codec::stored});
            recordings[i] = w.trace_id();
            auto a = w.add_var(none, "a", {SignalKind::bits, 1, 2}).second;
            auto b = w.add_var(none, "b", {SignalKind::bits, 1, 2}).second;
            w.set_time(0);
            w.emit(a, 0);
            w.emit(b, 0);
            w.set_time(1);
            w.emit(i ? b : a, 1);
            w.close();
        }
        ContainerReader trace(path);
        fingerprints[i] = *trace.waveform().fingerprint();
        check(trace.waveform().trace_id() == recordings[i], "writer/reader identity mismatch");
        auto index = ActivityIndex::build(trace);
        check(index.active(0, 0).empty(), "initial values counted as changes");
        check(index.active(1, 1) == std::vector<vtr::detail::wire::Id>{i},
              "activity changes swapped");
        if (!i) {
            sidecar = index.encode(trace);
            write(dir / "identity-0.index", sidecar);
        } else {
            rejects([&] { ActivityIndex::decode(trace, sidecar); });
        }
    }
    check(recordings[0] != recordings[1], "recordings share identity");
    check(fingerprints[0].bytes == fingerprints[1].bytes &&
              fingerprints[0].directory_crc == fingerprints[1].directory_crc,
          "identity collision fixture changed layout");
    auto path = dir / "identity-0.vtr";
    auto copy = dir / "same-recording.vtr";
    fs::copy_file(path, copy, fs::copy_options::overwrite_existing);
    ContainerReader copied(copy);
    check(ActivityIndex::decode(copied, sidecar).active(1, 1) ==
              std::vector<vtr::detail::wire::Id>{0},
          "same recording copy rejected");
    auto recovered = dir / "recovered.vtr";
    recover_container(path, recovered);
    ContainerReader r(recovered);
    check(r.waveform().trace_id() == recordings[0] && r.waveform().stats().recovered,
          "recovery lost execution identity");
    check(r.waveform().fingerprint() != std::optional(fingerprints[0]),
          "recovery retained derived fingerprint");
    rejects([&] { ActivityIndex::decode(r, sidecar); });
    auto data = read(path);
    auto no_trailer = dir / "no-trailer.vtr";
    write(no_trailer, View(data).first(data.size() - 24));
    ContainerReader prefix(no_trailer);
    check(!prefix.waveform().fingerprint(), "unfinished file exposes finalized fingerprint");
    rejects([&] { ActivityIndex::build(prefix); });
    for (auto field : {12, 16}) {
        auto malformed = data;
        if (field == 12) {
            malformed[12] = 1;
        } else {
            std::fill(malformed.begin() + 16, malformed.begin() + 32, 0);
        }
        auto bad = dir / "bad-header.vtr";
        write(bad, malformed);
        rejects([&] { Reader reader(bad); });
        rejects([&] { recover_container(bad, dir / "bad-recovered.vtr"); });
    }
}

void windows(const fs::path& dir) {
    auto path = dir / "windows.vtr";
    std::vector<std::vector<Timestamp>> expected(5);
    TraceId recording;
    {
        Writer w(path, {.codec = vtr::detail::wire::Codec::zstd, .wave_records = 9});
        recording = w.trace_id();
        std::array<vtr::detail::wire::Id, 5> signals;
        for (unsigned s = 0; s < 5; ++s) {
            signals[s] =
                w.add_var(none, "s" + std::to_string(s), {SignalKind::bits, 16, 2}, s == 4 ? 0 : 16)
                    .second;
        }
        w.add_alias(none, "alias", signals[0]);
        w.set_time(5);
        for (auto s : signals) {
            w.emit(s, 0);
        }
        for (Timestamp t = 6; t < 260; ++t) {
            w.set_time(t);
            for (unsigned s = 0; s < signals.size(); ++s) {
                if ((t * (s + 3) + s * s) % (s * 7 + 3) < s + 1) {
                    w.emit(signals[s], t);
                    expected[s].push_back(t);
                    if (s == 4) {
                        w.emit(signals[s], t);
                        w.flush();
                        w.emit(signals[s], t);
                    }
                }
            }
        }
        w.close();
    }
    ContainerReader trace(path, true);
    auto index = ActivityIndex::build(trace);
    auto bytes = index.encode(trace);
    write(dir / "windows.index", bytes);
    auto loaded = ActivityIndex::decode(trace, bytes);
    check(loaded.fingerprint().recording == recording, "index lost recording identity");
    for (Timestamp begin = 0; begin < 263; ++begin) {
        for (auto end :
             {begin, std::min<Timestamp>(262, begin + 1), std::min<Timestamp>(262, begin + 17),
              std::min<Timestamp>(262, begin + 99), UINT64_MAX}) {
            std::vector<vtr::detail::wire::Id> wanted;
            for (vtr::detail::wire::Id signal = 0; signal < expected.size(); ++signal) {
                auto i = std::lower_bound(expected[signal].begin(), expected[signal].end(), begin);
                if (i != expected[signal].end() && *i <= end) {
                    wanted.push_back(signal);
                }
            }
            check(index.active(begin, end) == wanted && loaded.active(begin, end) == wanted,
                  "activity window differs from producer oracle");
        }
    }
    rejects([&] { index.active(2, 1); });
    rejects([&] { ActivityIndex::build(trace, 1); });
    rejects([&] { ActivityIndex::decode(trace, bytes, bytes.size()); });
    for (auto cut : {std::size_t(0), std::size_t(32), bytes.size() - 1}) {
        rejects([&] { ActivityIndex::decode(trace, View(bytes).first(cut)); });
    }
    auto corrupt = bytes;
    corrupt[80] ^= 1;
    rejects([&] { ActivityIndex::decode(trace, corrupt); });
    auto wrong_id = bytes;
    wrong_id[32 + 24 + 40] ^= 1;
    set_u32(wrong_id, 32 + 16, d::checksum(View(wrong_id).subspan(56, 56)));
    rejects([&] { ActivityIndex::decode(trace, wrong_id); });
    auto old = bytes;
    old[8] = 2;
    rejects([&] { ActivityIndex::decode(trace, old); });
    auto directory_offset = d::load<std::uint64_t>(View(bytes).subspan(bytes.size() - 24));
    auto wrong_dir = bytes;
    wrong_dir[directory_offset + 24 + 8] ^= 1;
    set_u32(wrong_dir, directory_offset + 16,
            d::checksum(View(wrong_dir).subspan(directory_offset + 24,
                                                bytes.size() - directory_offset - 48)));
    rejects([&] { ActivityIndex::decode(trace, wrong_dir); });
}

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            throw std::runtime_error("usage: activity_test DIRECTORY");
        }
        fs::create_directories(argv[1]);
        identity(argv[1]);
        windows(argv[1]);
        std::cout << "activity identity/recovery/windows validated\n";
    } catch (const std::exception& e) {
        std::cerr << e.what() << '\n';
        return 1;
    }
}
