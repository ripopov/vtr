#include "containers.h"
#include "format.h"
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

namespace vtr::detail::wire {
using namespace detail;

namespace {
struct Mapping {
    const std::uint8_t* address = nullptr;
    std::size_t size = 0;

    explicit Mapping(const std::string& path) {
        int fd = ::open(path.c_str(), O_RDONLY);
        require(fd >= 0, "cannot open VTR file", ErrorCode::io);

        struct stat st {};

        if (fstat(fd, &st) != 0 || st.st_size < 32) {
            ::close(fd);
            throw Error(ErrorCode::corrupt_data, "invalid VTR file size");
        }
        size = static_cast<std::size_t>(st.st_size);
        auto p = mmap(nullptr, size, PROT_READ, MAP_PRIVATE, fd, 0);
        ::close(fd);
        require(p != MAP_FAILED, "cannot map VTR file", ErrorCode::io);
        address = static_cast<const std::uint8_t*>(p);
    }

    ~Mapping() {
        if (address) {
            munmap(const_cast<std::uint8_t*>(address), size);
        }
    }

    View slice(std::uint64_t offset, std::uint64_t len) const {
        require(offset <= size && len <= size - offset, "section exceeds file");
        return {address + offset, static_cast<std::size_t>(len)};
    }
};

struct WaveGroupEntry {
    Id id;
    std::uint32_t length;
    std::uint64_t offset;
};

struct SignalBlock {
    Section section;
    Time begin, end;
    std::uint32_t n_times, n_signals, group_size, n_dirty, n_groups, tt_len;
    std::vector<WaveGroupEntry> dirty;
    View predecessors, containers;
};

struct Group {
    std::size_t first = 0, count = 0;
    std::vector<Id> aliases;

    Id alias(Id local) const {
        return aliases.empty() ? none : aliases[local];
    }

    Bytes frame;

    struct Run {
        std::size_t first, count;
        unsigned transform;
        View bytes;
    };

    std::vector<Run> runs;
};

} // namespace

struct Reader::Impl {
    Mapping file;
    TraceId recording{};
    std::optional<TraceFingerprint> source_fingerprint;
    bool verify_crc;
    Meta meta;
    ReaderStats stats;
    std::vector<Section> sections;
    std::vector<std::string> strings;
    std::vector<Node> nodes;
    std::vector<SignalKind> signals;
    std::vector<Id> signal_nodes;
    std::vector<SignalBlock> waves;
    std::vector<std::pair<Time, bool>> blackout;
    mutable HashMap<Id, std::shared_ptr<const SignalData>> history_cache;
    mutable std::vector<Time> all_times;
    mutable bool times_loaded = false;

    explicit Impl(const std::string& path, ReaderOptions options)
        : file(path), verify_crc(options.verify_crc) {
        require(std::equal(magic.begin(), magic.end(), file.address), "invalid VTR magic");
        Cursor header(file.slice(8, 24));
        require(header.fixed<std::uint16_t>() == format_major &&
                    header.fixed<std::uint16_t>() == format_minor,
                "unsupported VTR version", ErrorCode::unsupported_version);
        require(header.fixed<std::uint32_t>() == 0, "unsupported VTR header flags");
        auto identity = header.take(16);
        require(std::any_of(identity.begin(), identity.end(), [](auto b) { return b != 0; }),
                "missing recording identity");
        std::copy(identity.begin(), identity.end(), recording.begin());
        bool directory = false;
        if (file.size >= 56 &&
            std::equal(trailer.begin(), trailer.end(), file.address + file.size - 8)) {
            auto off = load<std::uint64_t>(file.slice(file.size - 24, 8));
            auto len = load<std::uint64_t>(file.slice(file.size - 16, 8));
            if (len == file.size && off >= 32 && off <= file.size - 48) {
                auto d = section_at(off);
                require(d.kind == 7 && d.length == file.size - off - 48,
                        "invalid directory bounds");
                Cursor r(payload(d, true));
                auto n = r.fixed<std::uint64_t>();
                stats.recovered_dropped = r.fixed<std::uint64_t>();
                require((d.flags & ~2u) == 0 && ((d.flags & 2) || stats.recovered_dropped == 0),
                        "invalid directory recovery state");
                require(n <= r.data.size() / 48, "invalid directory entry count");
                std::uint64_t previous = 32;
                for (std::uint64_t i = 0; i < n; ++i) {
                    Section s;
                    s.kind = r.fixed<std::uint32_t>();
                    s.flags = r.fixed<std::uint32_t>();
                    s.offset = r.fixed<std::uint64_t>();
                    s.length = r.fixed<std::uint64_t>();
                    s.aux0 = r.fixed<std::uint64_t>();
                    s.aux1 = r.fixed<std::uint64_t>();
                    s.block_number = r.fixed<std::uint64_t>();
                    require(s.offset == previous && s.offset < off && off - s.offset >= 24 &&
                                s.length <= off - s.offset - 24,
                            "invalid directory entry");
                    auto actual = section_at(s.offset);
                    require(s.kind == actual.kind && s.flags == actual.flags &&
                                s.length == actual.length,
                            "directory/header mismatch");
                    previous = s.offset + 24 + s.length;
                    sections.push_back(s);
                }
                r.done();
                require(previous == off, "directory omits sections");
                source_fingerprint =
                    TraceFingerprint{recording, file.size, checksum(payload(d, true))};
                directory = true;
                stats.recovered = (d.flags & 2) != 0;
            }
        }
        if (!directory) {
            stats.recovered = true;
            std::uint64_t off = 32;
            while (off <= file.size && file.size - off >= 24) {
                try {
                    auto s = section_at(off);
                    if (!s.kind || s.kind == 7) {
                        break;
                    }
                    payload(s, true);
                    sections.push_back(s);
                    off += 24 + s.length;
                } catch (const Error&) {
                    break;
                }
            }
            stats.recovered_dropped = file.size - off;
        }
        bool have_meta = false;
        std::vector<Id> previous_dirty;
        for (auto& s : sections) {
            require(!directory || ((s.kind == 5 || s.kind == 9) == (s.block_number != 0)),
                    "invalid directory block-number field");
            if (!directory && (s.kind == 5 || s.kind == 9)) {
                Cursor bounds(unchecked_payload(s));
                s.block_number = bounds.fixed<std::uint64_t>();
                bounds.take(s.kind == 9 ? 24 : 32);
                s.aux0 = bounds.fixed<std::uint64_t>();
                s.aux1 = bounds.fixed<std::uint64_t>();
            }
            switch (s.kind) {
            case 1: {
                require(!have_meta, "duplicate META section");
                Cursor r(payload(s));
                require(r.var() == 1, "unknown META layout");
                auto exp = r.signed_var();
                require(exp >= -128 && exp <= 127, "invalid timescale");
                meta.timescale = static_cast<std::int8_t>(exp);
                meta.time_zero = r.signed_var();
                meta.file_type = r.byte();
                meta.writer = r.string();
                meta.date = r.string();
                meta.comment = r.string();
                meta.group_size = r.id();
                require(meta.group_size && std::has_single_bit(meta.group_size) &&
                            meta.group_size <= (1u << 20),
                        "invalid signal group size");
                meta.attrs = decode_attrs(r);
                attrs_valid(meta.attrs);
                r.done();
                have_meta = true;
                break;
            }
            case 2: {
                auto raw = decompress(payload(s));
                Cursor r(raw);
                require(r.id() == strings.size(), "noncontiguous string identifiers");
                auto n = r.var();
                require(n <= r.data.size(), "invalid string count");
                if (strings.size() + n > strings.capacity()) {
                    strings.reserve(std::max(strings.size() + static_cast<std::size_t>(n),
                                             strings.capacity() * 2));
                }
                for (std::uint64_t i = 0; i < n; ++i) {
                    auto text = r.blob();
                    validate_utf8(text);
                    strings.emplace_back(reinterpret_cast<const char*>(text.data()), text.size());
                }
                r.done();
                require(!strings.empty() && strings[0].empty(), "string zero is not empty");
                break;
            }
            case 3:
                require(have_meta, "hierarchy precedes metadata");
                read_nodes(s);
                break;
            case 4: {
                require(have_meta, "signal block precedes metadata");
                Cursor r(unchecked_payload(s));
                SignalBlock b{s,
                              r.fixed<std::uint64_t>(),
                              r.fixed<std::uint64_t>(),
                              r.fixed<std::uint32_t>(),
                              r.fixed<std::uint32_t>(),
                              r.fixed<std::uint32_t>(),
                              r.fixed<std::uint32_t>(),
                              r.fixed<std::uint32_t>(),
                              r.fixed<std::uint32_t>(),
                              {},
                              {},
                              {}};
                require(b.begin <= b.end && b.n_times > 0 && b.n_signals <= signals.size() &&
                            b.group_size == meta.group_size &&
                            b.n_groups ==
                                (std::uint64_t(b.n_signals) + b.group_size - 1) / b.group_size &&
                            b.n_dirty <= b.n_groups,
                        "invalid signal block header");
                require(waves.empty() ||
                            (waves.back().end <= b.begin && waves.back().n_signals <= b.n_signals),
                        "overlapping signal blocks");
                require(b.tt_len <= r.data.size(), "invalid time table length");
                if (!directory) {
                    s.aux0 = b.begin;
                    s.aux1 = b.end;
                } else {
                    require(s.aux0 == b.begin && s.aux1 == b.end, "wave directory bounds mismatch");
                }
                r.take(b.tt_len);
                std::uint64_t container_end = 0;
                for (Id i = 0; i < b.n_dirty; ++i) {
                    WaveGroupEntry entry{r.fixed<Id>(), r.fixed<std::uint32_t>(),
                                         r.fixed<std::uint64_t>()};
                    require(entry.id < b.n_groups && (i == 0 || b.dirty.back().id < entry.id) &&
                                entry.length && entry.offset == container_end &&
                                entry.length <= UINT64_MAX - container_end,
                            "invalid wave group directory");
                    container_end += entry.length;
                    b.dirty.push_back(entry);
                }
                b.predecessors = r.take(std::size_t(b.n_groups) * 4);
                previous_dirty.resize(b.n_groups, none);
                Cursor predecessors(b.predecessors);
                for (Id g = 0; g < b.n_groups; ++g) {
                    require(predecessors.fixed<Id>() == previous_dirty[g],
                            "invalid wave predecessor");
                }
                require(container_end == r.data.size(), "wave containers do not partition section");
                b.containers = r.data;
                require(waves.size() < none, "wave block index exhausted");
                for (const auto& entry : b.dirty) {
                    previous_dirty[entry.id] = static_cast<Id>(waves.size());
                }
                waves.push_back(std::move(b));
                range(b.begin, b.end);
                break;
            }
            case 5:
            case 9: {
                require(have_meta, "record block precedes metadata");
                Cursor r(unchecked_payload(s));
                auto number = r.fixed<std::uint64_t>();
                require(number && (!directory || number == s.block_number),
                        "invalid directory block number");
                s.block_number = number;
                break;
            }
            case 10:
                require(have_meta && s.block_number == 0, "invalid datatype section");
                break;
            case 6: {
                Cursor r(payload(s));
                auto n = r.var();
                require(n <= r.data.size() / 2, "invalid blackout count");
                Time t = 0;
                for (std::uint64_t i = 0; i < n; ++i) {
                    auto active = r.byte();
                    require(active <= 1, "invalid blackout state");
                    auto dt = r.var();
                    require(dt <= forever - t, "blackout time overflow");
                    t += dt;
                    blackout.emplace_back(t, active != 0);
                }
                r.done();
                break;
            }
            default:
                require(s.flags & 1, "unknown required section");
                break;
            }
        }
        require(have_meta && !strings.empty(), "missing metadata or strings");
        stats.signal_blocks = static_cast<std::uint32_t>(waves.size());
    }

    Section section_at(std::uint64_t offset) const {
        Cursor r(file.slice(offset, 24));
        Section s;
        s.offset = offset;
        s.kind = r.fixed<std::uint32_t>();
        s.flags = r.fixed<std::uint32_t>();
        s.length = r.fixed<std::uint64_t>();
        r.fixed<std::uint32_t>();
        require(r.fixed<std::uint32_t>() == 0, "nonzero section reserved field");
        file.slice(offset + 24, s.length);
        return s;
    }

    View unchecked_payload(const Section& s) const {
        return file.slice(s.offset + 24, s.length);
    }

    View payload(const Section& s, bool force_crc = false) const {
        auto p = unchecked_payload(s);
        auto crc = load<std::uint32_t>(file.slice(s.offset + 16, 4));
        require(!(verify_crc || force_crc) || !crc || checksum(p) == crc,
                "section checksum mismatch", ErrorCode::corrupt_data);
        return p;
    }

    void range(Time begin, Time end) {
        if (!stats.time_range) {
            stats.time_range = {{begin, end}};
        } else {
            stats.time_range->first = std::min(stats.time_range->first, begin);
            stats.time_range->second = std::max(stats.time_range->second, end);
        }
    }

    void string_id(Id id) const {
        require(id < strings.size(), "unknown string identifier");
    }

    void attrs_valid(std::span<const Attribute> attrs) const {
        for (auto& a : attrs) {
            string_id(a.key);
            validate_value(a.value, strings.size());
        }
    }

    void read_nodes(const Section& section) {
        auto raw = decompress(payload(section));
        Cursor r(raw);
        require(r.id() == nodes.size(), "noncontiguous node identifiers");
        auto count = r.var();
        require(count <= r.data.size() / 4, "invalid node count");
        if (nodes.size() + count > nodes.capacity()) {
            nodes.reserve(
                std::max(nodes.size() + static_cast<std::size_t>(count), nodes.capacity() * 2));
        }
        for (std::uint64_t i = 0; i < count; ++i) {
            Node n;
            n.kind = static_cast<Node::Kind>(r.byte());
            auto parent = r.id();
            require(parent <= nodes.size(), "invalid parent distance");
            n.parent = parent ? static_cast<Id>(nodes.size() - parent) : none;
            require(n.parent == none || nodes[n.parent].kind == Node::scope,
                    "parent is not a scope");
            n.name = r.id();
            string_id(n.name);
            switch (n.kind) {
            case Node::scope:
                n.type = static_cast<std::uint16_t>(r.id());
                n.aux = r.id();
                string_id(n.aux);
                break;
            case Node::generator:
                n.aux = r.id();
                n.payload_type = r.id();
                string_id(n.aux);
                break;
            case Node::variable: {
                n.type = static_cast<std::uint16_t>(r.id());
                n.direction = r.byte();
                auto back = r.id();
                auto declares = r.byte();
                require(declares <= 1 && back <= signals.size(), "invalid signal declaration");
                n.alias = !declares;
                n.signal = static_cast<Id>(signals.size() - back);
                if (declares) {
                    require(!back, "new signal has nonzero back reference");
                    auto code = r.byte();
                    require(code <= 4, "unknown signal kind");
                    SignalKind k;
                    if (code <= 2) {
                        k.type = SignalKind::bits;
                        k.states = code == 0 ? 2 : code == 1 ? 4 : 9;
                        k.width = r.id();
                    } else {
                        k.type = code == 3 ? SignalKind::real : SignalKind::varlen;
                        k.width = 0;
                        k.states = 0;
                    }
                    validate_kind(k);
                    signals.push_back(k);
                    signal_nodes.push_back(static_cast<Id>(nodes.size()));
                } else {
                    require(back > 0, "alias points to undeclared signal");
                    require((nodes[signal_nodes[n.signal]].type == 0) == (n.type == 0),
                            "conflicting event alias");
                }
                break;
            }
            case Node::enum_table: {
                auto n_entries = r.var();
                require(n_entries <= r.data.size() / 2, "invalid enumeration count");
                for (std::uint64_t j = 0; j < n_entries; ++j) {
                    auto a = r.id(), b = r.id();
                    string_id(a);
                    string_id(b);
                    n.entries.emplace_back(a, b);
                }
                break;
            }
            default:
                throw Error(ErrorCode::corrupt_data, "unknown node kind");
            }
            n.attrs = decode_attrs(r);
            attrs_valid(n.attrs);
            nodes.push_back(std::move(n));
        }
        r.done();
    }

    bool in_scope(Id node, Id scope) const {
        if (scope == none) {
            return true;
        }
        while (node != none) {
            require(node < nodes.size(), "unknown node", ErrorCode::invalid_argument);
            if (node == scope) {
                return true;
            }
            node = nodes[node].parent;
        }
        return false;
    }

    std::vector<Time> block_times(const SignalBlock& b) const {
        auto raw = decompress(unchecked_payload(b.section).subspan(40, b.tt_len));
        Cursor r(raw);
        require(b.n_times <= raw.size(), "invalid time table count");
        std::vector<Time> result;
        result.reserve(b.n_times);
        Time t = 0;
        for (Id i = 0; i < b.n_times; ++i) {
            auto delta = r.var();
            require(delta <= forever - t && (i == 0 || delta > 0), "invalid time table ordering");
            t += delta;
            result.push_back(t);
        }
        r.done();
        require(result.front() == b.begin && result.back() == b.end, "time table bounds mismatch");
        return result;
    }

    std::optional<Group> group(const SignalBlock& b, Id group_id) const {
        auto entry = std::lower_bound(b.dirty.begin(), b.dirty.end(), group_id,
                                      [](const auto& entry, Id id) { return entry.id < id; });
        if (entry == b.dirty.end() || entry->id != group_id) {
            return {};
        }
        Cursor c(b.containers.subspan(entry->offset, entry->length));
        Group result;
        result.first = std::size_t(group_id) * b.group_size;
        result.count = c.var();
        require(result.count == std::min<std::size_t>(b.group_size, b.n_signals - result.first),
                "invalid group signal count");
        auto aliases = c.var();
        require(aliases <= result.count, "invalid alias count");
        if (aliases) {
            result.aliases.resize(result.count, none);
        }
        for (std::uint64_t i = 0; i < aliases; ++i) {
            auto local = c.id(), target = c.id();
            require(local < result.count && target < b.n_signals && target < result.first + local &&
                        signals[target] == signals[result.first + local],
                    "invalid dynamic alias");
            require(result.alias(local) == none, "duplicate alias");
            result.aliases[local] = target;
        }
        result.frame = decompress(c.blob());
        auto n = c.var();
        require(n <= result.count, "invalid run count");

        struct Descriptor {
            std::size_t count;
            unsigned transform;
            std::size_t size;
        };

        std::vector<Descriptor> descriptors;
        std::size_t total = 0;
        for (std::uint64_t i = 0; i < n; ++i) {
            auto count = c.var(), x = c.var(), size = c.var();
            require(count > 0 && count <= result.count - total && x <= 4, "invalid run descriptor");
            descriptors.push_back({static_cast<std::size_t>(count), static_cast<unsigned>(x),
                                   static_cast<std::size_t>(size)});
            total += count;
        }
        require(total == result.count, "runs do not cover group");
        total = result.first;
        for (auto d : descriptors) {
            result.runs.push_back({total, d.count, d.transform, c.take(d.size)});
            total += d.count;
        }
        c.done();
        return result;
    }

    Bytes frame_value(const Group& g, Id signal) const {
        Cursor r(g.frame);
        Bytes result;
        for (std::size_t s = g.first; s < g.first + g.count; ++s) {
            auto k = signals[s];
            auto value = k.type == SignalKind::varlen ? r.blob() : r.take(k.packed_size());
            if (s == signal) {
                result.assign(value.begin(), value.end());
            }
        }
        r.done();
        return result;
    }

    bool column(Id signal, View column, unsigned x, const std::vector<Time>& tt, Time begin,
                Time end, const std::function<bool(Time, Id, const SignalValue&)>& visit) const {
        if (column.empty()) {
            return true;
        }
        auto kind = signals[signal];
        Cursor r(column);
        std::uint64_t index = 0;
        if (kind.type == SignalKind::bits && kind.width == 1) {
            std::uint8_t code = 255;
            const SignalValue sample{kind, View(&code, 1)};
            while (!r.empty()) {
                auto entry = r.var();
                auto delta = entry >> (entry & 1 ? 5 : 1);
                require(delta <= forever - index, "signal time overflow");
                index += delta;
                if (entry & 1) {
                    code = (entry >> 1) & 15;
                } else {
                    require(code < 2, "invalid bit toggle");
                    code ^= 1;
                }
                require(index < tt.size() && code < kind.states, "invalid bit column");
                auto time = tt[index];
                if (time > end) {
                    break;
                }
                if (time >= begin && !visit(time, signal, sample)) {
                    return false;
                }
            }
            return true;
        }
        auto word = r.var();
        auto header_len = word >> 2;
        Cursor headers(r.take(header_len));
        auto values = r.data;
        bool eligible = word & 2, full = word & 1;
        Bytes transformed;
        if (eligible) {
            require(kind.type != SignalKind::varlen,
                    "variable length column cannot be transformed");
            auto width = kind.type == SignalKind::real ? 8
                         : full                        ? kind.packed_size()
                                                       : (std::size_t(kind.width) + 7) / 8;
            std::size_t count = 0;
            Cursor h = headers;
            while (!h.empty()) {
                h.var();
                ++count;
            }
            require(width && count < (1u << 30) / width, "column exceeds format limit");
            if (x == 4) {
                Cursor dict(values);
                auto n = dict.var();
                require(n <= 256, "invalid dictionary size");
                if (!n) {
                    transformed = Bytes(dict.take(count * width).begin(), dict.data.end());
                } else {
                    auto entries = dict.take(n * width);
                    transformed.reserve(count * width);
                    for (std::size_t i = 0; i < count; ++i) {
                        auto code = dict.byte();
                        require(code < n, "invalid dictionary code");
                        append(transformed, entries.subspan(code * width, width));
                    }
                }
                dict.done();
                values = transformed;
            } else if (x) {
                transformed = transform(values, width, x, true);
                values = transformed;
            }
            require(values.size() == count * width, "fixed column value length mismatch");
        }
        Cursor v(values);
        Bytes expanded;
        SignalValue sample{kind, {}};
        while (!headers.empty()) {
            auto h = headers.var();
            auto delta = kind.type == SignalKind::bits ? h >> 1 : h;
            require(delta <= forever - index, "signal time overflow");
            index += delta;
            require(index < tt.size(), "signal time index exceeds table");
            View value;
            if (kind.type == SignalKind::varlen) {
                value = v.blob();
            } else if (kind.type == SignalKind::real) {
                value = v.take(8);
            } else {
                bool compact = h & 1;
                require(kind.states != 2 || compact, "noncompact two-state value");
                value = v.take(compact ? (std::size_t(kind.width) + 7) / 8 : kind.packed_size());
                auto bits = std::size_t(kind.width) * (compact || kind.states == 2 ? 1u
                                                       : kind.states == 4          ? 2u
                                                                                   : 4u);
                require(bits % 8 == 0 || (value.back() >> (bits % 8)) == 0,
                        "nonzero signal padding");
                if (compact && kind.states != 2) {
                    expanded = repack(value, kind.width, 2, kind.states);
                    value = expanded;
                }
            }
            auto time = tt[index];
            if (time > end) {
                break;
            }
            if (time >= begin) {
                sample.data = value;
                if (!visit(time, signal, sample)) {
                    return false;
                }
            }
        }
        if (headers.empty()) {
            v.done();
        }
        return true;
    }

    bool one_signal(const SignalBlock& b, const Group& g, Id signal, const std::vector<Time>& times,
                    Time begin, Time end,
                    const std::function<bool(Time, Id, const SignalValue&)>& visit) const {
        auto target = g.alias(static_cast<Id>(signal - g.first));
        if (target != none) {
            auto target_group = group(b, target / b.group_size);
            require(target_group.has_value() &&
                        target_group->alias(static_cast<Id>(target - target_group->first)) == none,
                    "alias target is absent or aliased");
            return one_signal(b, *target_group, target, times, begin, end,
                              [&](Time t, Id, SignalValue v) { return visit(t, signal, v); });
        }
        for (auto& run : g.runs) {
            if (signal >= run.first && signal < run.first + run.count) {
                auto raw = decompress(run.bytes);
                Cursor r(raw);
                std::vector<std::size_t> sizes;
                for (std::size_t i = 0; i < run.count; ++i) {
                    sizes.push_back(r.var());
                }
                for (std::size_t i = 0; i < run.count; ++i) {
                    auto col = r.take(sizes[i]);
                    if (run.first + i == signal) {
                        return column(signal, col, run.transform, times, begin, end, visit);
                    }
                }
            }
        }
        throw Error(ErrorCode::corrupt_data, "missing signal run");
    }

    bool signal_changes(Id signal, Time begin, Time end,
                        const std::function<bool(Time, Id, const SignalValue&)>& visit) const {
        require(signal < signals.size(), "unknown signal", ErrorCode::invalid_argument);
        for (auto& b : waves) {
            if (b.begin > end) {
                break;
            }
            if (b.end < begin || signal >= b.n_signals) {
                continue;
            }
            payload(b.section);
            auto g = group(b, signal / b.group_size);
            if (!g) {
                continue;
            }
            auto tt = block_times(b);
            if (!one_signal(b, *g, signal, tt, begin, end, visit)) {
                return false;
            }
        }
        return true;
    }

    void scan_wave(Time begin, Time end,
                   const std::function<bool(Time, Id, const SignalValue&)>& visit,
                   const std::function<void(const Group&)>& frame = {}) const {
        for (auto& b : waves) {
            if (b.begin > end) {
                break;
            }
            if (b.end < begin) {
                continue;
            }
            payload(b.section);
            auto times = block_times(b);
            std::vector<std::optional<Group>> groups;
            groups.reserve(b.n_groups);
            std::vector<Id> alias_heads, next_alias;
            for (Id g = 0; g < b.n_groups; ++g) {
                groups.push_back(this->group(b, g));
                if (groups.back() && !groups.back()->aliases.empty()) {
                    if (alias_heads.empty()) {
                        alias_heads.resize(b.n_signals, none);
                        next_alias.resize(b.n_signals, none);
                    }
                    auto& group = *groups.back();
                    for (Id local = 0; local < group.count; ++local) {
                        auto target = group.alias(local);
                        if (target != none) {
                            auto id = static_cast<Id>(group.first + local);
                            next_alias[id] = alias_heads[target];
                            alias_heads[target] = id;
                        }
                    }
                }
            }
            if (frame) {
                for (const auto& group : groups) {
                    if (group) {
                        frame(*group);
                    }
                }
            }
            for (Id target = 0; target < alias_heads.size(); ++target) {
                if (alias_heads[target] == none) {
                    continue;
                }
                auto& owner = groups[target / b.group_size];
                require(owner && owner->alias(static_cast<Id>(target - owner->first)) == none,
                        "alias target is absent or aliased");
            }
            for (auto& group : groups) {
                if (!group) {
                    continue;
                }
                for (auto& run : group->runs) {
                    auto raw = decompress(run.bytes);
                    Cursor r(raw);
                    std::vector<std::size_t> sizes;
                    for (std::size_t i = 0; i < run.count; ++i) {
                        sizes.push_back(r.var());
                    }
                    for (std::size_t i = 0; i < run.count; ++i) {
                        auto col = r.take(sizes[i]);
                        auto id = static_cast<Id>(run.first + i);
                        if (group->alias(static_cast<Id>(id - group->first)) != none) {
                            require(col.empty(), "aliased signal has its own column");
                            continue;
                        }
                        auto copies = alias_heads.empty() ? none : alias_heads[id];
                        if (copies == none) {
                            if (!column(id, col, run.transform, times, begin, end, visit)) {
                                return;
                            }
                        } else if (!column(id, col, run.transform, times, begin, end,
                                           [&](Time t, Id signal, const SignalValue& value) {
                                               if (!visit(t, signal, value)) {
                                                   return false;
                                               }
                                               for (auto alias = copies; alias != none;
                                                    alias = next_alias[alias]) {
                                                   if (!visit(t, alias, value)) {
                                                       return false;
                                                   }
                                               }
                                               return true;
                                           })) {
                            return;
                        }
                    }
                    r.done();
                }
            }
        }
    }
};

Reader::Reader(const std::string& path, ReaderOptions options)
    : impl_(std::make_unique<Impl>(path, options)) {}

Reader::~Reader() = default;

const TraceId& Reader::trace_id() const noexcept {
    return impl_->recording;
}

std::optional<TraceFingerprint> Reader::fingerprint() const noexcept {
    return impl_->source_fingerprint;
}

const Meta& Reader::metadata() const {
    return impl_->meta;
}

const ReaderStats& Reader::stats() const {
    return impl_->stats;
}

std::span<const std::string> Reader::strings() const {
    return impl_->strings;
}

std::string_view Reader::string(Id id) const {
    impl_->string_id(id);
    return impl_->strings[id];
}

std::span<const ContainerSection> Reader::sections() const {
    return impl_->sections;
}

std::span<const std::uint8_t> Reader::section_data(const ContainerSection& section,
                                                   bool verify) const {
    return impl_->payload(section, verify);
}

std::span<const Node> Reader::nodes() const {
    return impl_->nodes;
}

std::span<const SignalKind> Reader::signals() const {
    return impl_->signals;
}

Id Reader::signal_var(Id id) const {
    require(id < impl_->signal_nodes.size(), "unknown signal", ErrorCode::invalid_argument);
    return impl_->signal_nodes[id];
}

Id Reader::find_node(std::string_view path, char separator, std::optional<Node::Kind> kind) const {
    auto& p = *impl_;
    Id parent = none;
    std::size_t start = 0;
    while (start <= path.size()) {
        auto end = path.find(separator, start);
        auto part =
            path.substr(start, end == std::string_view::npos ? path.size() - start : end - start);
        Id found = none;
        for (Id i = 0; i < p.nodes.size(); ++i) {
            if (p.nodes[i].parent == parent && p.strings[p.nodes[i].name] == part &&
                (end == std::string_view::npos ? (!kind || p.nodes[i].kind == *kind)
                                               : p.nodes[i].kind == Node::scope)) {
                found = i;
                break;
            }
        }
        if (found == none) {
            return none;
        }
        parent = found;
        if (end == std::string_view::npos) {
            return parent;
        }
        start = end + 1;
    }
    return none;
}

Id Reader::find_signal(std::string_view path, char separator) const {
    auto node = find_node(path, separator, Node::variable);
    return node == none || impl_->nodes[node].kind != Node::variable ? none
                                                                     : impl_->nodes[node].signal;
}

bool Reader::in_scope(Id node, Id scope) const {
    return impl_->in_scope(node, scope);
}

void Reader::changes(Id signal, Time begin, Time end,
                     const std::function<bool(Time, Id, const SignalValue&)>& visit) const {
    require(bool(visit), "missing change callback", ErrorCode::invalid_argument);
    impl_->signal_changes(signal, begin, end, visit);
}

void Reader::changes(Time begin, Time end,
                     const std::function<bool(Time, Id, const SignalValue&)>& visit) const {
    require(bool(visit), "missing change callback", ErrorCode::invalid_argument);
    impl_->scan_wave(begin, end, visit);
}

void Reader::validate_waveforms() const {
    auto& p = *impl_;
    std::vector<Bytes> last;
    last.reserve(p.signals.size());
    for (auto kind : p.signals) {
        last.push_back(default_value(kind));
    }
    auto valid = [&](Id signal, View bytes) {
        auto kind = p.signals[signal];
        if (kind.type == SignalKind::varlen) {
            return;
        }
        require(bytes.size() == kind.packed_size(), "invalid wave value length");
        if (kind.type != SignalKind::bits) {
            return;
        }
        auto stride = kind.states == 2 ? 1u : kind.states == 4 ? 2u : 4u;
        auto bits = std::size_t(kind.width) * stride;
        require(bits % 8 == 0 || (bytes.back() >> (bits % 8)) == 0, "nonzero wave padding");
        if (kind.states == 9) {
            for (std::size_t bit = 0; bit < kind.width; ++bit) {
                require(((bytes[bit / 2] >> ((bit % 2) * 4)) & 15) < 9,
                        "invalid nine-state wave code");
            }
        }
    };
    p.scan_wave(
        0, forever,
        [&](Time, Id signal, const SignalValue& value) {
            valid(signal, value.data);
            last[signal].assign(value.data.begin(), value.data.end());
            return true;
        },
        [&](const Group& group) {
            Cursor r(group.frame);
            for (std::size_t signal = group.first; signal < group.first + group.count; ++signal) {
                auto kind = p.signals[signal];
                auto value =
                    kind.type == SignalKind::varlen ? r.blob() : r.take(kind.packed_size());
                valid(signal, value);
                require(std::equal(value.begin(), value.end(), last[signal].begin(),
                                   last[signal].end()),
                        "wave frame disagrees with preceding history");
            }
            r.done();
        });
}

Bytes Reader::value_at(Id signal, Time time) const {
    auto& p = *impl_;
    require(signal < p.signals.size(), "unknown signal", ErrorCode::invalid_argument);
    Bytes result = default_value(p.signals[signal]);
    auto it = std::upper_bound(p.waves.begin(), p.waves.end(), time,
                               [](Time t, const auto& b) { return t < b.begin; });
    while (it != p.waves.begin()) {
        --it;
        auto& b = *it;
        if (signal >= b.n_signals) {
            return result;
        }
        p.payload(b.section);
        auto g = p.group(b, signal / b.group_size);
        if (!g) {
            Cursor previous(b.predecessors.subspan(std::size_t(signal / b.group_size) * 4, 4));
            auto index = previous.fixed<Id>();
            if (index == none) {
                return result;
            }
            it = p.waves.begin() + index + 1;
            continue;
        }
        result = p.frame_value(*g, signal);
        auto times = p.block_times(b);
        p.one_signal(b, *g, signal, times, 0, time, [&](Time, Id, SignalValue v) {
            result.assign(v.data.begin(), v.data.end());
            return true;
        });
        break;
    }
    return result;
}

std::shared_ptr<const SignalData> Reader::load_signal(Id signal) const {
    auto& p = *impl_;
    {
        auto found = p.history_cache.find(signal);
        if (found != p.history_cache.end()) {
            return found->second;
        }
    }
    require(signal < p.signals.size(), "unknown signal", ErrorCode::invalid_argument);
    auto data = std::make_shared<SignalData>();
    data->kind = p.signals[signal];
    data->initial = default_value(data->kind);
    data->offsets.push_back(0);
    p.signal_changes(signal, 0, forever, [&](Time time, Id, SignalValue value) {
        data->times.push_back(time);
        append(data->values, value.data);
        data->offsets.push_back(data->values.size());
        return true;
    });
    return p.history_cache.emplace(signal, data).first->second;
}

const std::vector<Time>& Reader::time_table() const {
    auto& p = *impl_;
    if (!p.times_loaded) {
        for (auto& b : p.waves) {
            p.payload(b.section);
            for (auto time : p.block_times(b)) {
                if (p.all_times.empty() || p.all_times.back() != time) {
                    p.all_times.push_back(time);
                }
            }
        }
        p.times_loaded = true;
    }
    return p.all_times;
}

std::span<const std::pair<Time, bool>> Reader::blackouts() const {
    return impl_->blackout;
}

void Reader::clear_cache() const {
    impl_->history_cache.clear();
    impl_->all_times.clear();
    impl_->times_loaded = false;
}

} // namespace vtr::detail::wire
