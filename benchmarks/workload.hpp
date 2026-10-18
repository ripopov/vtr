#pragma once

#include <algorithm>
#include <array>
#include <cstdint>
#include <limits>
#include <queue>
#include <stdexcept>
#include <string>
#include <vector>

namespace bench {
using u64 = std::uint64_t;
constexpr u64 unlimited = std::numeric_limits<u64>::max();
constexpr unsigned query_count = 12;
constexpr std::array<const char*, 5> attr_names{"serial", "address", "bytes", "opcode", "status"};

inline u64 mix(u64 x) {
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    return x ^ (x >> 31);
}

struct Profile {
    std::string name;
    unsigned signals, generators, ticks;
    bool quiet;
};

inline Profile profile(const std::string& name, unsigned scale) {
    Profile p;
    if (name == "core") p = {name, 64, 0, 480000, false};
    else if (name == "soc") p = {name, 4096, 0, 24000, false};
    else if (name == "idle") p = {name, 4096, 0, 48000, true};
    else if (name == "requests") p = {name, 0, 8, 120000, false};
    else if (name == "many-generators") p = {name, 0, 64, 24000, false};
    else if (name == "mixed") p = {name, 256, 16, 24000, false};
    else throw std::runtime_error("unknown profile: " + name);
    p.ticks = std::max(64u, p.ticks / scale);
    return p;
}

inline unsigned width(unsigned signal) {
    constexpr unsigned widths[]{1, 1, 1, 8, 32, 32, 64, 32};
    return widths[signal % 8];
}

inline u64 signal_value(unsigned signal, u64 tick) {
    const auto lane = signal / 8;
    switch (signal % 8) {
    case 0: return (tick / (1 + lane % 4)) & 1;
    case 1: return tick < 8;
    case 2: return (mix(tick / 4 + lane) & 7) < 5;
    case 3: return (tick / 8 + lane) % 32;
    case 4: return 0x80000000u + ((tick / 2 + lane * 13) % 4096) * 4;
    case 5: return ((tick / 8 + lane * 17) % 256) * 64;
    case 6: return mix(tick / 4 + lane * 1021);
    default: return tick / 1024 + lane;
    }
}

struct Transaction {
    u64 serial, begin, end;
    unsigned generator;
    std::array<u64, 5> attrs;
};

struct Digest {
    u64 count = 0, sum = 0, xors = 0, logical_bytes = 0;
    void add(u64 hash, u64 bytes) {
        ++count;
        sum += hash;
        xors ^= hash;
        logical_bytes += bytes;
    }
    void signal(u64 time, unsigned id, u64 value) {
        add(mix(time ^ mix(id + 1) ^ mix(value)), 8 + 4 + (width(id) + 7) / 8);
    }
    void tx(const Transaction& t) {
        u64 h = mix(t.begin) ^ mix(t.end) ^ mix(t.generator);
        for (unsigned i = 0; i < t.attrs.size(); ++i) h = mix(h ^ mix(t.attrs[i] + i));
        add(h, 8 + 4 + 8 + 8 + 5 * 8);
    }
};

struct Query {
    unsigned source;
    u64 begin, end, value;
};

inline Query query(const Profile& p, unsigned i, bool transaction, bool whole_trace = false) {
    const unsigned sources = transaction ? p.generators : p.signals / 8;
    const auto source = unsigned(mix(i + 19) % sources);
    const auto begin = mix(i + 47) % (p.ticks - 32);
    const auto end = std::min<u64>(p.ticks - 1, begin + 128);
    const unsigned signal = source * 8 + i % 8;
    u64 value = transaction ? ((begin / 4 + source * 17) % 256) * 64 : signal_value(signal, begin);
    if (i % 4 == 3) value = unlimited;
    return {transaction ? source : signal, whole_trace ? 0 : begin, whole_trace ? unlimited : end, value};
}

// Emission order is global time order; completion order can differ from issue order.
template<class Sink> void generate(const Profile& p, Sink& sink) {
    struct Later {
        bool operator()(const Transaction& a, const Transaction& b) const {
            return a.end != b.end ? a.end > b.end : a.serial > b.serial;
        }
    };
    std::priority_queue<Transaction, std::vector<Transaction>, Later> pending;
    std::vector<u64> state(p.signals, unlimited);
    u64 serial = 0;
    for (unsigned tick = 0; tick < p.ticks; ++tick) {
        sink.time(tick);
        while (!pending.empty() && pending.top().end <= tick) {
            sink.end(pending.top());
            pending.pop();
        }
        const bool gated = p.quiet && tick % 1024 >= 64;
        for (unsigned s = 0; s < p.signals; ++s) {
            if (gated && tick != 0) continue;
            const auto value = signal_value(s, tick);
            if (tick == 0 || state[s] != value) {
                sink.signal(tick, s, value);
                state[s] = value;
            }
        }
        for (unsigned g = 0; g < p.generators; ++g) {
            if ((tick + g) % 4 || (tick / 128 + g) % 7 == 6) continue;
            const auto latency = 4 + mix(serial + 71) % 60;
            Transaction t{serial, tick, tick + latency, g,
                {serial, ((tick / 4 + g * 17) % 256) * 64,
                 u64(8) << (mix(serial) % 4), mix(serial + 3) % 4,
                 mix(serial + 5) % 97 == 0 ? 1u : 0u}};
            sink.begin(t);
            pending.push(t);
            ++serial;
        }
    }
    while (!pending.empty()) {
        sink.time(pending.top().end);
        sink.end(pending.top());
        pending.pop();
    }
}

struct Oracle {
    const Profile& p;
    Digest all;
    std::array<Digest, query_count> waves{}, transactions{}, points{}, all_waves{}, all_transactions{};
    explicit Oracle(const Profile& profile) : p(profile) {}
    void time(u64) {}
    void begin(const Transaction&) {}
    void signal(u64 time, unsigned id, u64 value) {
        all.signal(time, id, value);
        for (unsigned i = 0; i < query_count; ++i) {
            const auto q = query(p, i, false);
            if (id != q.source) continue;
            if (value == q.value) all_waves[i].signal(time, id, value);
            if (time <= q.begin) {
                points[i] = {};
                points[i].signal(q.begin, id, value);
            }
            if (time >= q.begin && time <= q.end && value == q.value)
                waves[i].signal(time, id, value);
        }
    }
    void end(const Transaction& t) {
        all.tx(t);
        for (unsigned i = 0; i < query_count; ++i) {
            const auto q = query(p, i, true);
            if (t.generator == q.source && t.attrs[1] == q.value) all_transactions[i].tx(t);
            if (t.generator == q.source && t.begin <= q.end && t.end >= q.begin && t.attrs[1] == q.value)
                transactions[i].tx(t);
        }
    }
};
} // namespace bench
