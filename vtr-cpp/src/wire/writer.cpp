#include "containers.h"
#include "format.h"
#include "handoff.h"
#include <cerrno>
#include <cstdio>
#include <limits>
#include <stdio_ext.h>
#include <sys/random.h>

namespace vtr::detail::wire {
using namespace detail;

namespace {
struct WaveColumn {
    SignalKind kind;
    bool event = false, emitted = false;
    std::uint8_t last_code = 255;
    std::uint32_t last_index = 0;
    std::uint64_t last_scalar = 0;
    std::size_t count = 0, compact_count = 0;
    Bytes headers, values;
    Bytes last, frame;
};

Bytes dictionary(View values, std::size_t width) {
    Bytes dict, codes;
    if (width == 1) {
        std::array<std::uint16_t, 256> ids{};
        std::uint16_t count = 0;
        codes.reserve(values.size());
        for (auto value : values) {
            auto& id = ids[value];
            if (!id) {
                id = ++count;
                dict.push_back(value);
            }
            codes.push_back(static_cast<std::uint8_t>(id - 1));
        }
        Bytes out;
        put(out, count);
        append(out, dict);
        append(out, codes);
        return out;
    }
    HashMap<std::string_view, std::uint8_t> ids;
    auto capacity = std::min<std::size_t>(values.size() / width, 256);
    ids.reserve(capacity);
    dict.reserve(capacity * width);
    codes.reserve(capacity);
    for (std::size_t i = 0; i < values.size(); i += width) {
        std::string_view key(reinterpret_cast<const char*>(values.data() + i), width);
        auto [it, inserted] = ids.try_emplace(key, static_cast<std::uint8_t>(ids.size()));
        if (inserted) {
            if (ids.size() > 256) {
                Bytes out{0};
                append(out, values);
                return out;
            }
            append(dict, values.subspan(i, width));
        }
        codes.push_back(it->second);
    }
    Bytes out;
    put(out, ids.size());
    append(out, dict);
    append(out, codes);
    return out;
}

std::size_t value_width(const WaveColumn& c) {
    if (c.kind.type == SignalKind::real) {
        return 8;
    }
    if (c.kind.type == SignalKind::bits && c.kind.width > 1) {
        if (c.compact_count == c.count) {
            return (c.kind.width + 7) / 8;
        }
        if (c.compact_count == 0) {
            return c.kind.packed_size();
        }
    }
    return 0;
}

Bytes make_column(const WaveColumn& c, unsigned xform) {
    Bytes out;
    if (!c.count) {
        return out;
    }
    if (c.kind.type == SignalKind::bits && c.kind.width == 1) {
        return c.headers;
    }
    auto width = value_width(c);
    put(out, (c.headers.size() << 2) | (width ? 2 : 0) |
                 (width && !c.compact_count && c.kind.type == SignalKind::bits ? 1 : 0));
    append(out, c.headers);
    if (!width || !xform) {
        append(out, c.values);
    } else if (xform == 4) {
        append(out, dictionary(c.values, width));
    } else {
        append(out, transform(c.values, width, xform, false));
    }
    return out;
}

struct EncodedWave {
    Bytes payload;
    std::vector<Bytes> containers;
    std::vector<Id> dirty;

    Bytes joined() && {
        auto size = payload.size();
        for (const auto& container : containers) {
            require(container.size() <= SIZE_MAX - size, "wave section length overflow");
            size += container.size();
        }
        payload.reserve(size);
        for (auto& container : containers) {
            append(payload, container);
            Bytes{}.swap(container);
        }
        return std::move(payload);
    }
};

struct WaveEncoder {
    const WriterOptions& options;
    std::span<const WaveColumn> waves;
    std::span<const Time> times;
    std::span<const Id> previous_dirty;

    Bytes run(std::size_t first, std::size_t end, const std::vector<bool>& alias,
              unsigned& selected) {
        Bytes sample;
        for (auto s = first; s < end; ++s) {
            if (!alias[s]) {
                auto w = value_width(waves[s]);
                if (!w) {
                    continue;
                }
                auto& v = waves[s].values;
                auto n = std::min<std::size_t>(v.size() / w, 128);
                append(sample, View(v).first(n * w));
            }
        }
        auto best = compress(sample, options.codec, options.compression_level).size();
        selected = 0;
        for (unsigned x = 1; x <= 4 && !sample.empty(); ++x) {
            Bytes candidate;
            for (auto s = first; s < end; ++s) {
                if (!alias[s]) {
                    auto w = value_width(waves[s]);
                    if (!w) {
                        continue;
                    }
                    auto& v = waves[s].values;
                    auto n = std::min<std::size_t>(v.size() / w, 128);
                    auto part = View(v).first(n * w);
                    append(candidate, x == 4 ? dictionary(part, w) : transform(part, w, x, false));
                }
            }
            auto size = compress(candidate, options.codec, options.compression_level).size();
            if (size * 100 < best * (x == 4 ? 80 : 96)) {
                best = size;
                selected = x;
            }
        }
        std::vector<Bytes> columns;
        Bytes raw;
        for (auto s = first; s < end; ++s) {
            columns.push_back(alias[s] ? Bytes{} : make_column(waves[s], selected));
            put(raw, columns.back().size());
        }
        for (auto& c : columns) {
            append(raw, c);
        }
        return compress(raw, options.codec, options.compression_level);
    }

    EncodedWave encode() {
        const auto groups = static_cast<std::uint32_t>((waves.size() + options.group_size - 1) /
                                                       options.group_size);
        std::vector<Id> dirty;
        std::vector<Bytes> containers;
        std::vector<Id> alias_of(waves.size(), none);
        std::vector<bool> alias(waves.size(), false);
        HashMap<std::uint64_t, Id> duplicates;
        duplicates.reserve(waves.size());
        std::vector<Id> next_candidate(waves.size(), none);
        for (Id s = 0; s < waves.size(); ++s) {
            if (waves[s].count) {
                auto& c = waves[s];
                auto hash_bytes = [](const Bytes& b) {
                    return StringHash{}({reinterpret_cast<const char*>(b.data()), b.size()});
                };
                auto hash = hash_bytes(c.headers) ^ std::rotl(hash_bytes(c.values), 17);
                auto [entry, inserted] = duplicates.try_emplace(hash, none);
                (void)inserted;
                for (auto other = entry->second; other != none; other = next_candidate[other]) {
                    if (waves[other].kind == c.kind && waves[other].headers == c.headers &&
                        waves[other].values == c.values) {
                        alias_of[s] = other;
                        alias[s] = true;
                        break;
                    }
                }
                if (!alias[s]) {
                    next_candidate[s] = entry->second;
                    entry->second = s;
                }
            }
        }
        for (Id g = 0; g < groups; ++g) {
            auto first = std::size_t(g) * options.group_size,
                 end = std::min(first + options.group_size, waves.size());
            if (std::none_of(waves.begin() + first, waves.begin() + end,
                             [](auto& c) { return c.count != 0; })) {
                continue;
            }
            dirty.push_back(g);
            Bytes container, frame;
            put(container, end - first);
            std::size_t count = 0;
            for (auto s = first; s < end; ++s) {
                count += alias[s];
            }
            put(container, count);
            for (auto s = first; s < end; ++s) {
                if (alias[s]) {
                    put(container, s - first);
                    put(container, alias_of[s]);
                }
            }
            for (auto s = first; s < end; ++s) {
                if (waves[s].kind.type == SignalKind::varlen) {
                    blob(frame, waves[s].frame);
                } else {
                    append(frame, waves[s].frame);
                }
            }
            blob(container, compress(frame, options.codec, options.compression_level));

            struct Run {
                std::size_t n;
                unsigned x;
                Bytes bytes;
            };

            std::vector<Run> runs;
            for (auto begin = first; begin < end;) {
                auto stop = begin;
                std::size_t size = 0;
                do {
                    size += waves[stop].headers.size() + waves[stop].values.size() + 10;
                    ++stop;
                } while (stop < end && stop - begin < 64 && size < 65536);
                unsigned x;
                auto data = run(begin, stop, alias, x);
                runs.push_back({stop - begin, x, std::move(data)});
                begin = stop;
            }
            put(container, runs.size());
            for (auto& r : runs) {
                put(container, r.n);
                put(container, r.x);
                put(container, r.bytes.size());
            }
            for (auto& r : runs) {
                append(container, r.bytes);
            }
            containers.push_back(std::move(container));
        }
        Bytes tt;
        Time prev = 0;
        for (auto t : times) {
            put(tt, t - prev);
            prev = t;
        }
        tt = compress(tt, options.codec, options.compression_level);
        Bytes payload;
        fixed(payload, times.front());
        fixed(payload, times.back());
        fixed<std::uint32_t>(payload, times.size());
        fixed<std::uint32_t>(payload, waves.size());
        fixed(payload, options.group_size);
        fixed<std::uint32_t>(payload, dirty.size());
        fixed(payload, groups);
        fixed<std::uint32_t>(payload, tt.size());
        append(payload, tt);
        std::uint64_t at = 0;
        for (std::size_t i = 0; i < dirty.size(); ++i) {
            fixed(payload, dirty[i]);
            fixed<std::uint32_t>(payload, containers[i].size());
            fixed(payload, at);
            at += containers[i].size();
        }
        for (auto previous : previous_dirty) {
            fixed(payload, previous);
        }
        return {std::move(payload), std::move(containers), std::move(dirty)};
    }
};

void reset_column(WaveColumn& c) {
    c.headers.clear();
    c.values.clear();
    if (c.kind.type == SignalKind::bits && c.kind.states == 2 && c.kind.width <= 64 && c.emitted) {
        c.last.clear();
        append_scalar(c.last, c.last_scalar, (c.kind.width + 7) / 8);
    }
    c.frame = c.last;
    c.count = c.compact_count = 0;
    c.last_index = 0;
    c.last_code = 255;
}

std::size_t wave_add(std::size_t a, std::size_t b) {
    return b > SIZE_MAX - a ? SIZE_MAX : a + b;
}

std::size_t wave_mul(std::size_t a, std::size_t b) {
    return a && b > SIZE_MAX / a ? SIZE_MAX : a * b;
}

struct WaveBatch {
    WriterOptions options;
    std::vector<WaveColumn> waves;
    std::vector<Time> times;
    std::vector<Id> previous_dirty;

    std::size_t charge() const {
        auto owned = wave_add(
            wave_mul(waves.capacity(), sizeof(WaveColumn)),
            wave_add(wave_mul(times.capacity(), 8), wave_mul(previous_dirty.capacity(), 4)));
        auto encoded =
            wave_add(4096, wave_add(wave_mul(times.size(), 10), wave_mul(waves.size(), 128)));
        for (const auto& column : waves) {
            for (auto bytes :
                 {column.frame.capacity(), column.headers.capacity(), column.values.capacity()}) {
                owned = wave_add(owned, bytes);
            }
            for (auto bytes : {column.frame.size(), column.headers.size(), column.values.size()}) {
                encoded = wave_add(encoded, wave_mul(bytes, 2));
            }
        }
        return wave_add(owned, wave_mul(encoded, 12));
    }

    Bytes encode() const {
        WaveEncoder encoder{options, waves, times, previous_dirty};
        return std::move(encoder.encode()).joined();
    }
};
} // namespace

struct Writer::Impl {
    WriterOptions options;
    Meta meta;
    FILE* file = nullptr;
    bool closed = false, meta_written = false, active = true, incomplete = false;
    std::uint64_t offset = 32, wave_records = 0;
    std::size_t wave_capture_bytes = 0, peak_wave_capture_bytes = 0;
    Time now = 0;
    std::vector<Section> directory;
    std::unique_ptr<vtr::detail::engine::detail::Handoff> handoff;
    // Interned views must survive ID-index growth and dictionary rehash.
    std::vector<std::unique_ptr<char[]>> string_chunks;
    std::size_t string_remaining = 0, string_offset = 0;
    std::vector<std::string_view> strings{std::string_view("")};
    HashMap<std::string_view, Id> interned{{std::string_view(""), 0}};
    std::vector<Node> nodes;
    std::vector<WaveColumn> waves;
    std::size_t written_strings = 0, written_nodes = 0, written_signals = 0;
    std::vector<Time> times;
    std::vector<std::pair<Time, bool>> blackout;
    std::vector<std::uint32_t> previous_dirty;
    std::uint32_t block_index = 0;
    TraceId recording{};

    Impl(const std::string& path, WriterOptions opts) : options(opts) {
        do {
            std::size_t filled = 0;
            while (filled < recording.size()) {
                auto n = ::getrandom(recording.data() + filled, recording.size() - filled, 0);
                if (n < 0 && errno == EINTR) {
                    continue;
                }
                require(n > 0, "cannot generate recording identity", 3);
                filled += static_cast<std::size_t>(n);
            }
        } while (std::all_of(recording.begin(), recording.end(), [](auto b) { return b == 0; }));
        require(opts.group_size && std::has_single_bit(opts.group_size) &&
                    opts.group_size <= (1u << 20),
                "invalid group size", 1);
        require(opts.block_records > 0 && opts.codec <= Codec::zstd, "invalid writer options", 1);
        meta.group_size = opts.group_size;
        file = std::fopen(path.c_str(), "wb");
        require(file, "cannot create VTR file", 3);
        // The assembly/output thread owns the file exclusively, including close.
        __fsetlocking(file, FSETLOCKING_BYCALLER);
        try {
            Bytes header(magic.begin(), magic.end());
            fixed<std::uint16_t>(header, format_major);
            fixed<std::uint16_t>(header, format_minor);
            header.resize(16);
            append(header, recording);
            write(header);
            if (opts.workers)
                handoff = std::make_unique<vtr::detail::engine::detail::Handoff>(
                    vtr::detail::engine::detail::HandoffOptions{opts.workers, opts.queue_jobs,
                                                                opts.queue_bytes},
                    [this](const vtr::detail::engine::detail::SectionRequest& section,
                           View payload) {
                        std::array<View, 1> parts{payload};
                        write_section(section.kind, parts, section.begin, section.end,
                                      section.flags, section.block_number);
                    });
        } catch (...) {
            std::fclose(file);
            file = nullptr;
            throw;
        }
    }

    ~Impl() {
        if (handoff) {
            handoff->stop();
        }
        if (file) {
            std::fclose(file);
        }
    }

    void alive() const {
        require(!closed, "writer is closed", 2);
        if (handoff) {
            handoff->check_failure();
        }
    }

    void write(View data) {
        require(std::fwrite(data.data(), 1, data.size(), file) == data.size(), "VTR write failed",
                3);
    }

    void section(std::uint32_t kind, View payload, std::uint64_t aux0 = 0, std::uint64_t aux1 = 0,
                 std::uint32_t flags = 0, std::uint64_t block_number = 0) {
        std::array<View, 1> parts{payload};
        section_parts(kind, parts, aux0, aux1, flags, block_number);
    }

    void section_parts(std::uint32_t kind, std::span<const View> parts, std::uint64_t aux0,
                       std::uint64_t aux1, std::uint32_t flags = 0,
                       std::uint64_t block_number = 0) {
        alive();
        if (!options.workers) {
            write_section(kind, parts, aux0, aux1, flags, block_number);
            return;
        }
        std::size_t length = 0;
        for (auto part : parts) {
            require(part.size() <= SIZE_MAX - length, "section length overflow");
            length += part.size();
        }
        auto payload = std::make_shared<Bytes>();
        payload->reserve(length);
        for (auto part : parts) {
            append(*payload, part);
        }
        require(payload->capacity() <= SIZE_MAX - 1024, "section charge overflow");
        auto charge = payload->capacity() + 1024;
        handoff->submit({kind, flags, aux0, aux1, block_number}, charge,
                        [payload] { return std::move(*payload); });
    }

    void write_section(std::uint32_t kind, std::span<const View> parts, std::uint64_t aux0,
                       std::uint64_t aux1, std::uint32_t flags = 0,
                       std::uint64_t block_number = 0) {
        std::uint64_t length = 0;
        std::uint32_t crc = 0;
        for (auto part : parts) {
            length += part.size();
            if (options.checksums && !part.empty()) {
                crc = checksum(part, crc);
            }
        }
        Bytes header;
        fixed(header, kind);
        fixed(header, flags);
        fixed(header, length);
        fixed(header, crc);
        fixed<std::uint32_t>(header, 0);
        write(header);
        for (auto part : parts) {
            if (!part.empty()) {
                write(part);
            }
        }
        require(std::fflush(file) == 0, "VTR flush failed", 3);
        directory.push_back({kind, flags, offset, length, aux0, aux1, block_number});
        offset += 24 + length;
    }

    Id intern(std::string_view value) {
        alive();
        validate_utf8(view(value));
        if (auto found = interned.find(value); found != interned.end())
            return found->second;
        require(strings.size() < none, "string limit reached", 1);
        if (value.size() > string_remaining) {
            const auto size = std::max<std::size_t>(4096, value.size());
            string_chunks.push_back(std::make_unique_for_overwrite<char[]>(size));
            string_remaining = size;
            string_offset = 0;
        }
        auto* data = string_chunks.back().get() + string_offset;
        std::memcpy(data, value.data(), value.size());
        const std::string_view stable(data, value.size());
        string_offset += value.size();
        string_remaining -= value.size();
        const auto id = static_cast<Id>(strings.size());
        strings.push_back(stable);
        try {
            auto [found, inserted] = interned.try_emplace(stable, id);
            if (!inserted)
                strings.pop_back();
            return found->second;
        } catch (...) {
            strings.pop_back();
            throw;
        }
    }

    void parent(Id p) const {
        require(p == none || (p < nodes.size() && nodes[p].kind == Node::scope),
                "parent must be a scope", 1);
    }

    void key(Id id) const {
        require(id < strings.size(), "unknown string identifier", 1);
    }

    void declarations(bool hierarchy = true) {
        for (auto& a : meta.attrs) {
            key(a.key);
            validate_value(a.value, strings.size());
        }
        validate_attrs(meta.attrs);
        if (written_strings < strings.size()) {
            Bytes raw;
            put(raw, written_strings);
            put(raw, strings.size() - written_strings);
            for (auto i = written_strings; i < strings.size(); ++i) {
                blob(raw, strings[i]);
            }
            section(2, compress(raw, options.codec, options.compression_level), written_strings,
                    strings.size() - written_strings);
            written_strings = strings.size();
        }
        if (!meta_written) {
            validate_utf8(view(meta.writer));
            validate_utf8(view(meta.date));
            validate_utf8(view(meta.comment));
            Bytes raw;
            put(raw, 1);
            sint(raw, static_cast<std::uint64_t>(meta.timescale));
            sint(raw, static_cast<std::uint64_t>(meta.time_zero));
            raw.push_back(meta.file_type);
            blob(raw, meta.writer);
            blob(raw, meta.date);
            blob(raw, meta.comment);
            put(raw, options.group_size);
            encode_attrs(raw, meta.attrs);
            section(1, raw);
            meta_written = true;
        }
        if (hierarchy && written_nodes < nodes.size()) {
            Bytes raw;
            put(raw, written_nodes);
            put(raw, nodes.size() - written_nodes);
            for (auto i = written_nodes; i < nodes.size(); ++i) {
                auto& n = nodes[i];
                raw.push_back(n.kind);
                put(raw, n.parent == none ? 0 : i - n.parent);
                put(raw, n.name);
                switch (n.kind) {
                case Node::scope:
                    put(raw, n.type);
                    put(raw, n.aux);
                    break;
                case Node::generator:
                    put(raw, n.aux);
                    put(raw, n.payload_type);
                    break;
                case Node::enum_table:
                    put(raw, n.entries.size());
                    for (auto [a, b] : n.entries) {
                        put(raw, a);
                        put(raw, b);
                    }
                    break;
                case Node::variable: {
                    put(raw, n.type);
                    raw.push_back(n.direction);
                    put(raw, written_signals - n.signal);
                    raw.push_back(!n.alias);
                    if (!n.alias) {
                        auto k = waves[n.signal].kind;
                        raw.push_back(k.type == SignalKind::bits   ? (k.states == 2   ? 0
                                                                      : k.states == 4 ? 1
                                                                                      : 2)
                                      : k.type == SignalKind::real ? 3
                                                                   : 4);
                        if (k.type == SignalKind::bits) {
                            put(raw, k.width);
                        }
                        ++written_signals;
                    }
                    break;
                }
                }
                encode_attrs(raw, n.attrs);
            }
            section(3, compress(raw, options.codec, options.compression_level), written_nodes,
                    nodes.size() - written_nodes);
            written_nodes = nodes.size();
        }
    }

    void capture_bytes(std::size_t bytes) {
        require(bytes <= SIZE_MAX - wave_capture_bytes, "wave capture byte overflow");
        wave_capture_bytes += bytes;
        peak_wave_capture_bytes = std::max(peak_wave_capture_bytes, wave_capture_bytes);
    }

    bool wave_full() const {
        return wave_records >= options.block_records ||
               (options.wave_bytes && wave_capture_bytes >= options.wave_bytes) ||
               times.size() >= UINT32_MAX;
    }

    void prepare_wave_change() {
        if (wave_full()) {
            flush_waves();
        }
    }

    void emit_scalar(Id signal, std::uint64_t value) {
        auto& p = *this;
        require(signal < p.waves.size(), "unknown signal", 1);
        auto& c = p.waves[signal];
        auto k = c.kind;
        require(k.type == SignalKind::bits && k.width <= 64,
                "emit requires a vector of at most 64 bits", 1);
        if (k.width < 64) {
            value &= (std::uint64_t{1} << k.width) - 1;
        }
        if (k.states == 2) {
            if (!p.active || (p.options.dedup && !c.event && c.emitted && value == c.last_scalar)) {
                return;
            }
            p.prepare_wave_change();
            if (p.times.empty()) {
                p.times.push_back(p.now);
                p.capture_bytes(8);
            }
            auto index = static_cast<std::uint32_t>(p.times.size() - 1);
            auto delta = std::uint64_t(index - c.last_index);
            if (k.width == 1) {
                auto code = static_cast<std::uint8_t>(value);
                put(c.headers, c.last_code < 2 && code == (c.last_code ^ 1)
                                   ? delta << 1
                                   : (delta << 5) | (code << 1) | 1);
                c.last_code = code;
            } else {
                put(c.headers, (delta << 1) | 1);
                append_scalar(c.values, value, c.last.size());
            }
            c.last_scalar = value;
            c.emitted = true;
            c.last_index = index;
            ++c.count;
            ++c.compact_count;
            ++p.wave_records;
            p.capture_bytes(8 + (k.width + 7) / 8);
            return;
        }
        Bytes compact;
        append_scalar(compact, value, (k.width + 7) / 8);
        auto full = repack(compact, k.width, 2, k.states);
        p.add_value(signal, full, compact, true);
    }

    void add_value(Id signal, View full, View compact, bool is_compact) {
        alive();
        require(signal < waves.size(), "unknown signal", 1);
        if (!active) {
            return;
        }
        auto& c = waves[signal];
        if (options.dedup && !c.event && c.emitted &&
            std::equal(full.begin(), full.end(), c.last.begin(), c.last.end())) {
            return;
        }
        prepare_wave_change();
        if (times.empty()) {
            times.push_back(now);
            capture_bytes(8);
        }
        auto index = static_cast<std::uint32_t>(times.size() - 1), dt = index - c.last_index;
        if (c.kind.type == SignalKind::bits && c.kind.width == 1) {
            auto code = full[0];
            auto entry = c.last_code < 2 && code == (c.last_code ^ 1)
                             ? std::uint64_t(dt) << 1
                             : (std::uint64_t(dt) << 5) | (code << 1) | 1;
            put(c.headers, entry);
            c.last_code = code;
        } else {
            put(c.headers,
                c.kind.type == SignalKind::bits ? (std::uint64_t(dt) << 1) | is_compact : dt);
            if (c.kind.type == SignalKind::varlen) {
                blob(c.values, full);
            } else {
                append(c.values, is_compact ? compact : full);
            }
        }
        c.last.assign(full.begin(), full.end());
        if (c.kind.type == SignalKind::bits && c.kind.states == 2 && c.kind.width <= 64) {
            c.last_scalar = 0;
            for (std::size_t i = 0; i < full.size(); ++i) {
                c.last_scalar |= std::uint64_t(full[i]) << (8 * i);
            }
        }
        c.emitted = true;
        c.last_index = index;
        ++c.count;
        c.compact_count += is_compact;
        ++wave_records;
        capture_bytes(8);
        capture_bytes(is_compact ? compact.size() : full.size());
    }

    void flush_waves() {
        if (times.empty()) {
            return;
        }
        try {
            declarations();
            require(block_index != none, "waveform block index exhaustion", 1);
            const auto groups = static_cast<std::uint32_t>((waves.size() + options.group_size - 1) /
                                                           options.group_size);
            previous_dirty.resize(groups, none);
            if (options.workers) {
                auto batch = std::make_shared<WaveBatch>();
                batch->options = options;
                batch->waves.resize(waves.size());
                batch->times = std::move(times);
                batch->previous_dirty = previous_dirty;
                for (std::size_t signal = 0; signal < waves.size(); ++signal) {
                    auto& source = waves[signal];
                    auto& captured = batch->waves[signal];
                    captured.kind = source.kind;
                    captured.count = source.count;
                    captured.compact_count = source.compact_count;
                    captured.frame = std::move(source.frame);
                    captured.headers = std::move(source.headers);
                    captured.values = std::move(source.values);
                    if (source.count) {
                        previous_dirty[signal / options.group_size] = block_index;
                    }
                    reset_column(source);
                }
                ++block_index;
                wave_records = 0;
                wave_capture_bytes = 0;
                times.clear();
                handoff->submit({4, 0, batch->times.front(), batch->times.back(), 0},
                                batch->charge(), [batch] { return batch->encode(); });
            } else {
                WaveEncoder encoder{options, waves, times, previous_dirty};
                auto encoded = encoder.encode();
                std::vector<View> parts{encoded.payload};
                for (const auto& container : encoded.containers) {
                    parts.emplace_back(container);
                }
                section_parts(4, parts, times.front(), times.back());
                for (auto group : encoded.dirty) {
                    previous_dirty[group] = block_index;
                }
                ++block_index;
                times.clear();
                wave_records = 0;
                wave_capture_bytes = 0;
                for (auto& column : waves) {
                    reset_column(column);
                }
            }
        } catch (const Error& error) {
            if (error.code == 3) {
                throw;
            }
            throw Error(error.what(), 6);
        }
    }
};

const TraceId& Writer::trace_id() const noexcept {
    return impl_->recording;
}

Writer::Writer(const std::string& path, WriterOptions options)
    : impl_(std::make_unique<Impl>(path, options)) {}

std::size_t Writer::string_count() const noexcept {
    return impl_->strings.size();
}

std::string_view Writer::string(Id id) const {
    impl_->key(id);
    return impl_->strings[id];
}

Writer::~Writer() {
    try {
        if (!impl_->closed) {
            close();
        }
    } catch (...) {
    }
}

Meta& Writer::metadata() {
    impl_->alive();
    require(!impl_->meta_written, "metadata has already been written", 2);
    return impl_->meta;
}

Id Writer::intern(std::string_view value) {
    return impl_->intern(value);
}

std::optional<Id> Writer::intern_with_budget(std::string_view value, std::size_t& remaining) {
    auto& p = *impl_;
    p.alive();
    if (auto i = p.interned.find(value); i != p.interned.end())
        return i->second;
    if (value.size() > remaining)
        return {};
    const auto id = p.intern(value);
    remaining -= value.size();
    return id;
}

Id Writer::add_scope(Id parent, std::string_view name, std::uint16_t type,
                     std::string_view component) {
    auto& p = *impl_;
    p.alive();
    p.parent(parent);
    Node n;
    n.parent = parent;
    n.name = intern(name);
    n.type = type;
    n.aux = intern(component);
    p.nodes.push_back(std::move(n));
    return static_cast<Id>(p.nodes.size() - 1);
}

std::pair<Id, Id> Writer::add_var(Id parent, std::string_view name, SignalKind kind,
                                  std::uint16_t type, std::uint8_t direction) {
    auto& p = *impl_;
    p.alive();
    p.parent(parent);
    validate_kind(kind);
    p.flush_waves();
    Node n;
    n.kind = Node::variable;
    n.parent = parent;
    n.name = intern(name);
    n.type = type;
    n.direction = direction;
    n.signal = static_cast<Id>(p.waves.size());
    WaveColumn c;
    c.kind = kind;
    c.event = type == 0;
    c.last = default_value(kind);
    c.frame = c.last;
    p.waves.push_back(std::move(c));
    p.nodes.push_back(std::move(n));
    return {static_cast<Id>(p.nodes.size() - 1), static_cast<Id>(p.waves.size() - 1)};
}

SignalKind Writer::signal_kind(Id signal) const {
    require(signal < impl_->waves.size(), "unknown signal", 1);
    return impl_->waves[signal].kind;
}

Id Writer::add_alias(Id parent, std::string_view name, Id signal, std::uint16_t type,
                     std::uint8_t direction) {
    auto& p = *impl_;
    p.alive();
    p.parent(parent);
    require(signal < p.waves.size() && p.waves[signal].event == (type == 0), "invalid signal alias",
            1);
    Node n;
    n.kind = Node::variable;
    n.parent = parent;
    n.name = intern(name);
    n.type = type;
    n.direction = direction;
    n.signal = signal;
    n.alias = true;
    p.nodes.push_back(std::move(n));
    return static_cast<Id>(p.nodes.size() - 1);
}

Id Writer::add_generator(Id parent, std::string_view name, std::string_view kind, Id payload_type) {
    auto& p = *impl_;
    p.alive();
    p.parent(parent);
    Node n;
    n.kind = Node::generator;
    n.parent = parent;
    n.name = intern(name);
    n.aux = intern(kind);
    n.payload_type = payload_type;
    p.nodes.push_back(std::move(n));
    return static_cast<Id>(p.nodes.size() - 1);
}

Id Writer::add_enum_table(Id parent, std::string_view name,
                          std::span<const std::pair<Id, Id>> entries) {
    auto& p = *impl_;
    p.alive();
    p.parent(parent);
    for (auto [a, b] : entries) {
        p.key(a);
        p.key(b);
    }
    Node n;
    n.kind = Node::enum_table;
    n.parent = parent;
    n.name = intern(name);
    n.entries.assign(entries.begin(), entries.end());
    p.nodes.push_back(std::move(n));
    return static_cast<Id>(p.nodes.size() - 1);
}

void Writer::node_attr(Id node, Id key, const Value& value) {
    auto& p = *impl_;
    p.alive();
    require(node >= p.written_nodes && node < p.nodes.size(), "node is absent or already written",
            2);
    p.key(key);
    auto& attrs = p.nodes[node].attrs;
    unique_attr(attrs, key);
    validate_value(value, p.strings.size());
    attrs.push_back({key, value});
}

void Writer::set_time(Time time) {
    auto& p = *impl_;
    p.alive();
    require(time >= p.now, "waveform time moved backwards", 1);
    if (p.waves.empty()) {
        // Endpoints preserve idle time before the first signal declaration.
        if (p.wave_full())
            p.flush_waves();
        p.now = time;
        if (p.times.empty() || (p.times.size() == 1 && p.times.back() != time)) {
            p.times.push_back(time);
            p.capture_bytes(8);
        } else {
            p.times.back() = time;
        }
        return;
    }
    if (p.wave_full()) {
        p.flush_waves();
    }
    p.now = time;
    if (p.times.empty() || p.times.back() != time) {
        p.times.push_back(time);
        p.capture_bytes(8);
    }
}

Time Writer::time() const noexcept {
    return impl_->now;
}

void Writer::dump(bool active) {
    auto& p = *impl_;
    p.alive();
    if (p.active != active) {
        p.active = active;
        p.blackout.emplace_back(p.now, active);
    }
}

void Writer::emit(Id signal, std::uint64_t value) {
    impl_->alive();
    impl_->emit_scalar(signal, value);
}

void Writer::emit(std::span<const SignalUpdate> updates) {
    auto& p = *impl_;
    p.alive();
    for (const auto& update : updates) {
        require(update.signal < p.waves.size(), "unknown signal", 1);
        const auto kind = p.waves[update.signal].kind;
        require(kind.type == SignalKind::bits && kind.width <= 64,
                "emit requires a vector of at most 64 bits", 1);
    }
    for (const auto& update : updates) {
        p.emit_scalar(update.signal, update.value);
    }
}

void Writer::emit_bit(Id signal, std::uint8_t code) {
    if (signal < impl_->waves.size() && impl_->waves[signal].kind.states == 2) {
        require(impl_->waves[signal].kind.width == 1 && code < 2, "invalid bit value", 1);
        emit(signal, code);
        return;
    }
    auto& p = *impl_;
    require(signal < p.waves.size(), "unknown signal", 1);
    auto k = p.waves[signal].kind;
    require(k.type == SignalKind::bits && k.width == 1 && code < k.states, "invalid bit value", 1);
    p.add_value(signal, View(&code, 1), View(&code, 1), code < 2);
}

void Writer::emit_real(Id signal, double value) {
    auto& p = *impl_;
    require(signal < p.waves.size() && p.waves[signal].kind.type == SignalKind::real,
            "signal is not real", 1);
    Bytes b;
    fixed(b, std::bit_cast<std::uint64_t>(value));
    p.add_value(signal, b, {}, false);
}

void Writer::emit_varlen(Id signal, std::span<const std::uint8_t> value) {
    auto& p = *impl_;
    require(signal < p.waves.size() && p.waves[signal].kind.type == SignalKind::varlen,
            "signal is not variable length", 1);
    p.add_value(signal, value, {}, false);
}

void Writer::emit_packed(Id signal, std::uint8_t states, std::span<const std::uint8_t> value) {
    auto& p = *impl_;
    require(signal < p.waves.size() && p.waves[signal].kind.type == SignalKind::bits,
            "signal is not a vector", 1);
    auto k = p.waves[signal].kind;
    if (states == 2 && k.states == 2) {
        require(value.size() == (std::size_t(k.width) + 7) / 8, "packed signal size mismatch", 1);
        if (k.width <= 64) {
            std::uint64_t scalar = 0;
            for (std::size_t i = 0; i < value.size(); ++i) {
                scalar |= std::uint64_t(value[i]) << (8 * i);
            }
            p.alive();
            p.emit_scalar(signal, scalar);
        } else if (k.width % 8 && (value.back() >> (k.width % 8))) {
            Bytes masked(value.begin(), value.end());
            masked.back() &= (1U << (k.width % 8)) - 1;
            p.add_value(signal, masked, masked, true);
        } else {
            p.add_value(signal, value, value, true);
        }
        return;
    }
    auto full = repack(value, k.width, states, k.states);
    if (k.states == 2 && k.width <= 64) {
        std::uint64_t scalar = 0;
        for (std::size_t i = 0; i < full.size(); ++i) {
            scalar |= std::uint64_t(full[i]) << (8 * i);
        }
        emit(signal, scalar);
        return;
    }

    bool compact = true;
    auto stride = k.states == 2 ? 1u : k.states == 4 ? 2u : 4u;
    for (std::size_t i = 0; i < k.width; ++i) {
        if (((full[i * stride / 8] >> ((i * stride) % 8)) & ((1u << stride) - 1)) > 1) {
            compact = false;
            break;
        }
    }
    if (compact) {
        auto packed = repack(full, k.width, k.states, 2);
        p.add_value(signal, full, packed, true);
    } else {
        p.add_value(signal, full, {}, false);
    }
}

void Writer::emit_words(Id signal, std::span<const std::uint32_t> words) {
    auto& p = *impl_;
    require(signal < p.waves.size(), "unknown signal", 1);
    auto k = p.waves[signal].kind;
    require(k.type == SignalKind::bits && words.size() >= (std::size_t(k.width) + 31) / 32,
            "insufficient vector words", 1);
    Bytes bytes((std::size_t(k.width) + 7) / 8);
    for (std::size_t i = 0; i < bytes.size(); ++i) {
        bytes[i] = static_cast<std::uint8_t>(words[i / 4] >> ((i % 4) * 8));
    }
    if (k.width % 8) {
        bytes.back() &= (1u << (k.width % 8)) - 1;
    }
    emit_packed(signal, 2, bytes);
}

void Writer::emit_logic(Id signal, std::string_view value) {
    auto& p = *impl_;
    require(signal < p.waves.size(), "unknown signal", 1);
    auto k = p.waves[signal].kind;
    require(k.type == SignalKind::bits && value.size() == k.width, "logic string width mismatch",
            1);
    Bytes bytes((std::size_t(k.width) + 1) / 2);
    for (std::size_t i = 0; i < value.size(); ++i) {
        auto c = value[value.size() - 1 - i];
        if (c >= 'A' && c <= 'Z') {
            c += 32;
        }
        auto code = std::string_view("01xzuwlh-").find(c);
        require(code != std::string_view::npos, "invalid logic character", 1);
        bytes[i / 2] |= code << ((i % 2) * 4);
    }
    emit_packed(signal, 9, bytes);
}

void Writer::append_section(std::uint32_t kind, std::span<const std::uint8_t> payload, Time begin,
                            Time end, std::uint64_t block_number) {
    auto& p = *impl_;
    require(kind == 5 || kind == 9 || kind == 10, "invalid extension section kind", 1);
    require((kind == 10) == (block_number == 0), "invalid extension block number", 1);
    // Schemas contain inline names, so publishing them need not freeze pending nodes.
    p.declarations(kind != 10);
    if (kind == 10) {
        Cursor declarations(payload);
        begin = declarations.var();
        end = declarations.var();
    }
    p.section(kind, payload, begin, end, 0, block_number);
}

void Writer::enqueue_section(std::uint32_t kind, Time begin, Time end, std::uint64_t block_number,
                             std::size_t memory_charge, std::function<Bytes()> encode) {
    auto& p = *impl_;
    p.alive();
    require((kind == 5 || kind == 9) && block_number, "invalid encoded section request", 1);
    p.declarations();
    if (p.handoff)
        p.handoff->submit({kind, 0, begin, end, block_number}, memory_charge, std::move(encode));
    else
        p.section(kind, encode(), begin, end, 0, block_number);
}

HandoffStats Writer::handoff_stats() const {
    return impl_->handoff ? impl_->handoff->stats() : HandoffStats{};
}

std::pair<std::size_t, std::size_t> Writer::wave_buffer_bytes() const noexcept {
    return {impl_->wave_capture_bytes, impl_->peak_wave_capture_bytes};
}

bool Writer::failed() const noexcept {
    return impl_->handoff && impl_->handoff->failed();
}

void Writer::check_failure() const {
    if (impl_->handoff)
        impl_->handoff->check_failure();
}

void Writer::flush() {
    auto& p = *impl_;
    p.alive();
    p.declarations();
    p.flush_waves();
    if (!p.blackout.empty()) {
        Bytes b;
        put(b, p.blackout.size());
        Time prev = 0;
        for (auto [t, active] : p.blackout) {
            b.push_back(active);
            put(b, t - prev);
            prev = t;
        }
        p.section(6, b);
        p.blackout.clear();
    }
    if (p.handoff)
        p.handoff->drain();
}

void Writer::abandon() noexcept {
    auto& p = *impl_;
    p.incomplete = true;
    p.closed = true;
    if (p.handoff) {
        p.handoff->stop();
    }
    if (p.file) {
        std::fclose(p.file);
        p.file = nullptr;
    }
}

void Writer::mark_incomplete() {
    impl_->incomplete = true;
}

void Writer::close_complete() {
    impl_->incomplete = false;
    try {
        close();
    } catch (...) {
        impl_->incomplete = true;
        throw;
    }
}

void Writer::close_incomplete() {
    impl_->incomplete = true;
    close();
}

void Writer::close() {
    auto& p = *impl_;
    if (p.closed) {
        return;
    }
    flush();
    if (p.handoff)
        p.handoff->stop();
    Bytes dir;
    fixed<std::uint64_t>(dir, p.directory.size());
    fixed<std::uint64_t>(dir, 0);
    for (auto& s : p.directory) {
        fixed(dir, s.kind);
        fixed(dir, s.flags);
        fixed(dir, s.offset);
        fixed(dir, s.length);
        fixed(dir, s.aux0);
        fixed(dir, s.aux1);
        fixed(dir, s.block_number);
    }
    auto directory_offset = p.offset;
    std::array<View, 1> parts{dir};
    p.write_section(7, parts, 0, 0, p.incomplete ? 2 : 0);
    Bytes end;
    fixed(end, directory_offset);
    fixed(end, p.offset + 24);
    append(end, trailer);
    p.write(end);
    auto result = std::fclose(p.file);
    p.file = nullptr;
    p.closed = true;
    require(result == 0, "VTR close failed", 3);
}
} // namespace vtr::detail::wire
