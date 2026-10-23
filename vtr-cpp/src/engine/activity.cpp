#include "engine/activity.h"
#include "format.h"

namespace vtr::detail::engine {
namespace {
namespace d = vtr::detail::wire::detail;
using d::require;
using Section = vtr::detail::wire::ContainerSection;
constexpr std::array<std::uint8_t, 8> magic{0x89, 'V', 'T', 'I', '\r', '\n', 0x1a, '\n'};

vtr::detail::wire::TraceFingerprint identity(const ContainerReader& trace) {
    auto value = trace.waveform().fingerprint();
    require(value.has_value(), "activity index requires a finalized directory");
    return *value;
}

std::vector<Section> waves(const ContainerReader& trace) {
    std::vector<Section> result;
    for (auto section : trace.sections()) {
        if (section.kind == 4) {
            d::Cursor h(trace.section_data(section));
            section.aux0 = h.fixed<Timestamp>();
            section.aux1 = h.fixed<Timestamp>();
            result.push_back(section);
        }
    }
    return result;
}

void budget(std::size_t used, std::size_t limit) {
    require(used <= limit, "activity index exceeds memory limit");
}

std::size_t base_bytes(std::size_t signals, std::size_t limit) {
    require(signals <= limit / sizeof(std::vector<ActivityStretch>),
            "activity signal table exceeds memory limit");
    return signals * sizeof(std::vector<ActivityStretch>);
}

void add(std::vector<ActivityStretch>& list, ActivityStretch value, std::size_t& used,
         std::size_t limit) {
    if (list.size() == list.capacity()) {
        auto remaining = (limit - used) / sizeof(ActivityStretch);
        require(remaining, "activity index exceeds memory limit");
        auto growth = std::min(std::max<std::size_t>(list.capacity(), 1), remaining);
        list.reserve(list.capacity() + growth);
        used += growth * sizeof(ActivityStretch);
    }
    list.push_back(value);
}

template <std::size_t N> Bytes pack(const std::array<Bytes, N>& columns) {
    Bytes raw;
    for (const auto& col : columns) {
        d::put(raw, col.size());
    }
    for (const auto& col : columns) {
        d::append(raw, col);
    }
    return d::compress(raw, vtr::detail::wire::Codec::zstd, 3);
}

template <std::size_t N>
std::array<d::Cursor, N> unpack(View bytes, Bytes& raw, std::size_t available) {
    d::Cursor h(bytes);
    h.byte();
    auto size = h.var();
    require(size <= available, "activity page exceeds memory limit");
    raw = d::decompress(bytes);
    d::Cursor r(raw);
    std::array<std::uint64_t, N> lengths;
    for (auto& n : lengths) {
        n = r.var();
    }
    std::array<d::Cursor, N> columns;
    for (std::size_t i = 0; i < N; ++i) {
        columns[i] = d::Cursor(r.take(lengths[i]));
    }
    r.done();
    return columns;
}

void section(Bytes& file, std::vector<Section>& dir, std::uint32_t kind, View body,
             Timestamp begin = 0, Timestamp end = 0) {
    dir.push_back({kind, 0, file.size(), body.size(), begin, end});
    d::fixed(file, kind);
    d::fixed<std::uint32_t>(file, 0);
    d::fixed<std::uint64_t>(file, body.size());
    d::fixed(file, d::checksum(body));
    d::fixed<std::uint32_t>(file, 0);
    d::append(file, body);
}
} // namespace

ActivityIndex ActivityIndex::build(const ContainerReader& trace, std::size_t memory_limit) {
    ActivityIndex index;
    index.fingerprint_ = identity(trace);
    auto blocks = waves(trace);
    index.minimum_ = blocks.empty() ? 0 : blocks.front().aux0;
    auto used = base_bytes(trace.waveform().signals().size(), memory_limit);
    index.signals_.resize(trace.waveform().signals().size());
    trace.waveform().changes(0, UINT64_MAX,
                             [&](Timestamp t, vtr::detail::wire::Id signal, const auto&) {
                                 if (t <= index.minimum_) {
                                     return true;
                                 }
                                 auto& list = index.signals_[signal];
                                 if (!list.empty() && t == list.back().end) {
                                     return true;
                                 }
                                 if (!list.empty() && t - list.back().end == 1) {
                                     list.back().end = t;
                                     list.back().gap = 1;
                                 } else {
                                     add(list, {t, t, 0}, used, memory_limit);
                                 }
                                 return true;
                             });
    return index;
}

Bytes ActivityIndex::encode(const ContainerReader& trace) const {
    require(identity(trace) == fingerprint_, "activity source identity mismatch");
    auto blocks = waves(trace);
    Bytes file(magic.begin(), magic.end());
    d::fixed<std::uint16_t>(file, 3);
    d::fixed<std::uint16_t>(file, 0);
    file.resize(32);
    std::vector<Section> dir;
    Bytes header;
    d::fixed(header, fingerprint_.bytes);
    d::fixed(header, fingerprint_.directory_crc);
    header.push_back(1);
    header.resize(16);
    d::fixed<std::uint32_t>(header, signals_.size());
    header.resize(32);
    d::fixed(header, minimum_);
    d::append(header, fingerprint_.recording);
    section(file, dir, 1, header);
    std::vector<std::size_t> closed(signals_.size());
    std::vector<bool> first(signals_.size());
    for (const auto& block : blocks) {
        std::array<Bytes, 6> cols;
        vtr::detail::wire::Id previous = 0, rows = 0, nclosed = 0;
        for (vtr::detail::wire::Id signal = 0; signal < signals_.size(); ++signal) {
            const auto& list = signals_[signal];
            if (list.empty()) {
                continue;
            }
            bool initial = !first[signal] && list[0].begin <= block.aux1;
            auto count = closed[signal];
            while (count + 1 < list.size() && list[count + 1].begin <= block.aux1) {
                ++count;
            }
            auto number = count - closed[signal];
            if (!initial && !number) {
                continue;
            }
            d::put(cols[0], signal - previous);
            previous = signal;
            d::put(cols[1], (number << 1) | initial);
            if (initial) {
                require(list[0].begin >= block.aux0, "activity first outside source block");
                d::put(cols[2], list[0].begin - block.aux0);
                first[signal] = true;
            }
            for (auto i = closed[signal]; i < count; ++i) {
                d::put(cols[3], list[i].end - list[i].begin);
                d::put(cols[4], list[i + 1].begin - list[i].end);
                d::put(cols[5], list[i].gap);
            }
            closed[signal] = count;
            ++rows;
            require(number <= UINT32_MAX - nclosed, "activity stretch count overflow");
            nclosed += number;
        }
        Bytes body;
        d::fixed(body, block.aux0);
        d::fixed(body, block.aux1);
        d::fixed(body, block.length + 24);
        d::fixed(body, rows);
        d::fixed(body, nclosed);
        body.resize(40);
        d::append(body, pack(cols));
        section(file, dir, 2, body, block.aux0, block.aux1);
    }
    std::array<Bytes, 3> tails;
    vtr::detail::wire::Id previous = 0, rows = 0;
    for (vtr::detail::wire::Id signal = 0; signal < signals_.size(); ++signal) {
        if (!signals_[signal].empty()) {
            const auto& s = signals_[signal].back();
            d::put(tails[0], signal - previous);
            previous = signal;
            d::put(tails[1], s.end - s.begin);
            d::put(tails[2], s.gap);
            ++rows;
        }
    }
    Bytes tail;
    d::fixed(tail, rows);
    d::fixed<std::uint32_t>(tail, 0);
    d::append(tail, pack(tails));
    section(file, dir, 3, tail);
    Bytes directory;
    d::fixed<std::uint64_t>(directory, dir.size());
    for (const auto& s : dir) {
        d::fixed(directory, s.kind);
        d::fixed(directory, s.flags);
        d::fixed(directory, s.offset);
        d::fixed(directory, s.length);
        d::fixed(directory, s.aux0);
        d::fixed(directory, s.aux1);
    }
    auto off = file.size();
    section(file, dir, 7, directory);
    d::fixed<std::uint64_t>(file, off);
    d::fixed<std::uint64_t>(file, file.size() + 16);
    d::append(file, d::trailer);
    return file;
}

ActivityIndex ActivityIndex::decode(const ContainerReader& trace, View bytes,
                                    std::size_t memory_limit) {
    auto expected = identity(trace);
    budget(bytes.size(), memory_limit);
    require(bytes.size() >= 56 && std::equal(magic.begin(), magic.end(), bytes.begin()),
            "invalid activity magic");
    d::Cursor file_header(bytes.subspan(8, 24));
    require(file_header.fixed<std::uint16_t>() == 3 && file_header.fixed<std::uint16_t>() == 0,
            "unsupported activity version");
    for (auto b : file_header.data) {
        require(b == 0, "invalid activity header flags");
    }
    auto end = bytes.size() - 24;
    d::Cursor trailer(bytes.subspan(end));
    auto dir_offset = trailer.fixed<std::uint64_t>();
    require(trailer.fixed<std::uint64_t>() == bytes.size() &&
                std::equal(d::trailer.begin(), d::trailer.end(), trailer.data.begin()) &&
                dir_offset >= 32 && dir_offset < end,
            "invalid activity trailer");
    ActivityIndex index;
    auto blocks = waves(trace);
    std::size_t block_index = 0;
    std::vector<std::optional<Timestamp>> starts;
    std::vector<Section> sections;
    bool header = false, tail = false, directory = false;
    std::size_t used = bytes.size();
    for (std::size_t offset = 32; offset < end;) {
        require(end - offset >= 24 && !directory, "invalid activity section order");
        d::Cursor h(bytes.subspan(offset, 24));
        auto kind = h.fixed<std::uint32_t>();
        auto flags = h.fixed<std::uint32_t>();
        auto length = h.fixed<std::uint64_t>();
        auto crc = h.fixed<std::uint32_t>();
        require(!flags && h.fixed<std::uint32_t>() == 0 && length <= end - offset - 24,
                "invalid activity section bounds");
        auto body = bytes.subspan(offset + 24, length);
        require(d::checksum(body) == crc, "activity checksum mismatch");
        d::Cursor b(body);
        Timestamp begin = 0, finish = 0;
        if (kind == 1) {
            require(!header && sections.empty() && length == 56, "invalid activity HEADER");
            index.fingerprint_.bytes = b.fixed<std::uint64_t>();
            index.fingerprint_.directory_crc = b.fixed<std::uint32_t>();
            require(b.byte() == 1, "invalid activity source format");
            for (auto v : b.take(3)) {
                require(v == 0, "invalid activity reserved field");
            }
            auto signals = b.fixed<std::uint32_t>();
            require(signals == trace.waveform().signals().size(), "activity signal count mismatch");
            require(b.fixed<std::uint32_t>() == 0 && b.fixed<std::uint32_t>() == 0 &&
                        b.fixed<std::uint32_t>() == 0,
                    "invalid activity budgets");
            index.minimum_ = b.fixed<Timestamp>();
            auto id = b.take(16);
            std::copy(id.begin(), id.end(), index.fingerprint_.recording.begin());
            b.done();
            require(index.fingerprint_ == expected, "activity source identity mismatch");
            require(index.minimum_ == (blocks.empty() ? 0 : blocks.front().aux0),
                    "activity minimum mismatch");
            auto base = base_bytes(signals, memory_limit - used);
            used += base;
            require(signals <= (memory_limit - used) / sizeof(std::optional<Timestamp>),
                    "activity working set exceeds memory limit");
            used += signals * sizeof(std::optional<Timestamp>);
            starts.resize(signals);
            index.signals_.resize(signals);
            header = true;
        } else if (kind == 2 || kind == 3) {
            require(header && !tail, "invalid activity data order");
            std::uint32_t rows, closed = 0;
            if (kind == 2) {
                require(block_index < blocks.size(), "extra activity BLOCK");
                const auto& source = blocks[block_index++];
                begin = b.fixed<Timestamp>();
                finish = b.fixed<Timestamp>();
                require(begin == source.aux0 && finish == source.aux1 &&
                            b.fixed<std::uint64_t>() == source.length + 24,
                        "activity source block mismatch");
                rows = b.fixed<std::uint32_t>();
                closed = b.fixed<std::uint32_t>();
                require(b.byte() == 0, "unsupported activity threshold");
                for (auto v : b.take(7)) {
                    require(v == 0, "invalid activity block flags");
                }
            } else {
                require(block_index == blocks.size(), "missing activity BLOCK");
                rows = b.fixed<std::uint32_t>();
                require(b.fixed<std::uint32_t>() == 0, "invalid activity TAIL");
                tail = true;
            }
            require(rows <= starts.size(), "invalid activity rows");
            Bytes raw;
            std::array<d::Cursor, 6> columns;
            if (kind == 2) {
                columns = unpack<6>(b.data, raw, memory_limit - used);
            } else {
                auto three = unpack<3>(b.data, raw, memory_limit - used);
                std::copy(three.begin(), three.end(), columns.begin());
            }
            auto working = used + raw.size();
            std::uint64_t signal = 0, observed_closed = 0;
            auto stretch = [&](Timestamp length, Timestamp gap) {
                require(starts[signal] && length <= UINT64_MAX - *starts[signal] && gap <= 1 &&
                            gap <= length && ((length == 0) == (gap == 0)),
                        "invalid activity stretch");
                auto last = *starts[signal] + length;
                require(!blocks.empty() && last <= blocks.back().aux1,
                        "activity stretch exceeds source time");
                add(index.signals_[signal], {*starts[signal], last, gap}, working, memory_limit);
                return last;
            };
            for (std::uint32_t i = 0; i < rows; ++i) {
                auto delta = columns[0].var();
                require(delta <= UINT32_MAX - signal && (i == 0 || delta),
                        "invalid activity signal delta");
                signal += delta;
                require(signal < starts.size(), "invalid activity signal");
                if (kind == 2) {
                    auto head = columns[1].var();
                    require(head != 0, "empty activity row");
                    auto number = head >> 1;
                    require(number <= columns[3].data.size(), "invalid activity closure count");
                    if (head & 1) {
                        require(!starts[signal], "duplicate activity first");
                        auto dt = columns[2].var();
                        require(dt <= finish - begin && begin + dt > index.minimum_,
                                "invalid activity first time");
                        starts[signal] = begin + dt;
                    }
                    for (std::uint64_t j = 0; j < number; ++j) {
                        auto length = columns[3].var(), silence = columns[4].var(),
                             gap = columns[5].var();
                        auto last = stretch(length, gap);
                        require(silence > 1 && silence <= UINT64_MAX - last &&
                                    last + silence >= begin && last + silence <= finish,
                                "invalid activity silence");
                        starts[signal] = last + silence;
                    }
                    observed_closed += number;
                } else {
                    stretch(columns[1].var(), columns[2].var());
                    starts[signal].reset();
                }
            }
            for (auto& col : columns) {
                col.done();
            }
            require(kind == 3 || observed_closed == closed, "activity closure count mismatch");
            used = working - raw.size();
        } else if (kind == 7) {
            require(header && tail && offset == dir_offset && length == 8 + sections.size() * 40,
                    "invalid activity DIRECTORY");
            require(b.fixed<std::uint64_t>() == sections.size(),
                    "invalid activity directory count");
            for (const auto& s : sections) {
                require(b.fixed<std::uint32_t>() == s.kind && b.fixed<std::uint32_t>() == 0 &&
                            b.fixed<std::uint64_t>() == s.offset &&
                            b.fixed<std::uint64_t>() == s.length &&
                            b.fixed<Timestamp>() == s.aux0 && b.fixed<Timestamp>() == s.aux1,
                        "activity directory mismatch");
            }
            b.done();
            directory = true;
        } else {
            require(false, "unknown activity section");
        }
        if (kind != 7) {
            sections.push_back({kind, 0, offset, length, begin, finish});
        }
        offset += 24 + length;
    }
    require(header && tail && directory, "incomplete activity index");
    for (auto start : starts) {
        require(!start, "missing activity tail row");
    }
    return index;
}

std::vector<vtr::detail::wire::Id> ActivityIndex::active(Timestamp begin, Timestamp end) const {
    require(begin <= end, "invalid activity window");
    std::vector<vtr::detail::wire::Id> result;
    for (vtr::detail::wire::Id signal = 0; signal < signals_.size(); ++signal) {
        const auto& list = signals_[signal];
        auto i = std::lower_bound(list.begin(), list.end(), begin,
                                  [](const auto& s, auto t) { return s.end < t; });
        if (i != list.end() && i->begin <= end) {
            result.push_back(signal);
        }
    }
    return result;
}
} // namespace vtr::detail::engine
