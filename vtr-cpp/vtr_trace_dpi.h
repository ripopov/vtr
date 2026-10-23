// Serialized DPI-C calls into one logical recording producer.

#ifndef VTR_TRACE_DPI_H
#define VTR_TRACE_DPI_H

#include "svdpi.h"
#include "vtr_track.h"

#include <cstdint>
#include <map>
#include <memory>
#include <string>
#include <vector>

namespace vtr_trace {

class Host {
public:
    virtual ~Host() = default;
    virtual uint64_t now() = 0;
    virtual int timePrecision() = 0;
    virtual std::string rootPath() = 0;
};

class Sink {
public:
    virtual ~Sink() = default;
    virtual vtr::Writer& writer() = 0;
    virtual void enter(uint64_t time) = 0;
    virtual void leave() noexcept = 0;
    virtual int timescale() = 0;
    virtual std::optional<vtr::ScopeId> scopeNode(const std::string& path) = 0;
    virtual void warn(const std::string& text) = 0;
};

inline bool resolveScope(const std::string& caller,
                         const std::string& root,
                         const std::string& path,
                         std::string& out) {
    if (!path.empty() && path[0] == '/') {
        out = path.substr(1);
        return !out.empty();
    }
    if (path.compare(0, 5, "$root") == 0 && (path.size() == 5 || path[5] == '.')) {
        const std::string rest = path.size() > 6 ? path.substr(6) : "";
        out = root.empty() ? rest : rest.empty() ? root : root + "." + rest;
        return true;
    }
    std::string base = caller;
    size_t i = 0;
    while (i < path.size() && path[i] == '^') {
        const size_t dot = base.rfind('.');
        if (base.empty())
            return false;
        base = dot == std::string::npos ? "" : base.substr(0, dot);
        ++i;
        if (i < path.size()) {
            if (path[i] != '.')
                return false;
            ++i;
        }
    }
    const std::string rest = path.substr(i);
    out = base.empty() ? rest : rest.empty() ? base : base + "." + rest;
    return true;
}

class Recording {
    Sink* sink_;
public:
    Recording(Sink* sink, uint64_t time) : sink_(sink) {
        if (sink_) sink_->enter(time);
    }
    ~Recording() { if (sink_) sink_->leave(); }
    Recording(const Recording&) = delete;
    Recording& operator=(const Recording&) = delete;
};

using Keyspace = vtr::Tracker::Keyspace;

class Runtime {
public:
    static constexpr uint32_t KIND_SHIFT = 24;
    static constexpr uint32_t KIND_CLOCK = 1;
    static constexpr uint32_t KIND_TRACKER = 2;
    static constexpr uint32_t KIND_KEYSPACE = 3;

    explicit Runtime(Host& host) : m_host{host} {}

    void abandon() noexcept {
        m_sink = nullptr;
        for (auto& tracker : m_trackers) {
            tracker.live.reset();
            tracker.source.reset();
            tracker.liveSpaces.clear();
        }
        for (auto& clock : m_clocks) clock.id.reset();
    }

    void opened(Sink& sink) {
        if (m_sink)
            throw vtr::Error(vtr::ErrorCode::invalid_state, "A recording is already attached to this context");
        m_sink = &sink;
        m_fileExp = sink.timescale();
        m_hostExp = m_host.timePrecision();
        const Recording recording{m_sink, fileNow()};
        for (Clock& c : m_clocks) {
            declare(c);
            if (c.running)
                run(c);
        }
        for (Tracker& t : m_trackers)
            declare(t);
    }

    void closing() {
        Sink* const closingSink = m_trackers.empty() && m_clocks.empty() ? nullptr : m_sink;
        const Recording recording{closingSink, closingSink ? fileNow() : 0};
        if (!m_sink)
            return;
        std::vector<std::string> notes;
        for (Tracker& t : m_trackers) {
            if (!t.clock.empty() && !findClock(t.clock))
                misuse(t.path() + ": clock " + t.clock + " is not declared with vtr_clock");
            else if (!t.clock.empty() && !t.associated)
                misuse(t.path() + ": clock " + t.clock + " must be declared before recording items");
            if (!t.live)
                continue;
            t.live->detach();
            for (std::string& n : t.live->take_diagnostics())
                notes.push_back(std::move(n));
            t.live.reset();
            t.source.reset();
            t.liveSpaces.clear();
            t.recorded = t.associated = false;
        }
        for (const std::string& n : notes)
            m_sink->warn("vtr_trace: " + n);
        for (const auto& m : m_misuse) {
            m_sink->warn("vtr_trace: " + m.first + " (" + std::to_string(m.second) +
                         (m.second == 1 ? " time)" : " times)"));
        }
        m_misuse.clear();
        for (Clock& c : m_clocks)
            c.id.reset();
        m_sink = nullptr;
    }

    uint64_t now() {
        return m_sink ? fileNow() : m_host.now();
    }

    uint32_t clock(const std::string& caller, const char* scope, const char* name) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        Clock c;
        c.name = name ? name : "";
        const std::string root = m_host.rootPath();
        if (!resolveScope(caller, root, scope ? scope : "", c.scope)) {
            misuse("vtr_clock(\"" + std::string{scope ? scope : ""} + "\", \"" + c.name +
                   "\") in " + caller + ": scope above the root, using the root");
            c.scope = root;
        }
        m_clocks.push_back(c);
        if (m_sink)
            declare(m_clocks.back());
        return (KIND_CLOCK << KIND_SHIFT) | static_cast<uint32_t>(m_clocks.size());
    }

    void clockRun(uint32_t handle, uint64_t period, int unit) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        Clock* const cp = clockOf(handle, "vtr_clock_run");
        if (!cp)
            return;
        if (cp->running) {
            misuse(cp->path() + ": vtr_clock_run on a running clock; call vtr_clock_stop first");
            return;
        }
        cp->period = period;
        cp->unit = unit;
        cp->first = m_host.now();
        cp->running = true;
        if (m_sink)
            run(*cp);
    }

    void clockStop(uint32_t handle) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        Clock* const cp = clockOf(handle, "vtr_clock_stop");
        if (!cp || !cp->running)
            return;
        cp->running = false;
        if (m_sink && cp->id && cp->started)
            m_sink->writer().stop(*cp->id, fileNow());
        cp->started = false;
    }

    uint32_t tracker(const std::string& caller,
                     const char* scope,
                     const char* generator,
                     const char* kind,
                     const char* clock) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        Tracker t;
        t.kind = kind ? kind : "";
        t.generator = generator ? generator : "";
        const std::string root = m_host.rootPath();
        if (!resolveScope(caller, root, scope ? scope : "", t.scope)) {
            misuse("vtr_tracker(\"" + std::string{scope ? scope : ""} + "\", \"" + t.generator +
                   "\") in " + caller + ": scope above the root, using the root");
            t.scope = root;
        }
        if (clock && *clock && !resolveScope(t.scope, root, clock, t.clock))
            misuse(t.path() + ": clock path " + clock + " is above the root; no clock recorded");
        m_trackers.push_back(std::move(t));
        if (m_sink)
            declare(m_trackers.back());
        return (KIND_TRACKER << KIND_SHIFT) | static_cast<uint32_t>(m_trackers.size());
    }

    uint32_t keyspace(uint32_t trk, const char* name) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        Tracker* const t = trackerOf(trk, "vtr_keyspace");
        if (!t)
            return 0;
        t->spaces.push_back(name ? name : "");
        if (t->live)
            t->liveSpaces.push_back(t->live->keyspace(name ? name : ""));
        m_spaces.push_back(SpaceRef{static_cast<uint32_t>(t - m_trackers.data()),
                                    static_cast<uint32_t>(t->spaces.size() - 1)});
        return (KIND_KEYSPACE << KIND_SHIFT) | static_cast<uint32_t>(m_spaces.size());
    }

    template <typename F> int item(uint32_t sp, const char* fn, F f) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        if (!m_sink)
            return 0;
        const SpaceRef* const r = spaceOf(sp, fn);
        if (!r || !m_trackers[r->tracker].live)
            return 0;
        auto& t = m_trackers[r->tracker];
        t.recorded = true;
        return f(*t.live, t.liveSpaces[r->space], fileNow());
    }

    template <typename F> int item2(uint32_t sp, uint32_t to, const char* fn, F f) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        if (!m_sink)
            return 0;
        const SpaceRef* const a = spaceOf(sp, fn);
        const SpaceRef* const b = spaceOf(to, fn);
        if (!a || !b || !m_trackers[a->tracker].live)
            return 0;
        if (a->tracker != b->tracker) {
            misuse(std::string{fn} + ": key spaces of two different trackers (" +
                   m_trackers[a->tracker].path() + ", " + m_trackers[b->tracker].path() + ")");
            return 0;
        }
        auto& t = m_trackers[a->tracker];
        return f(*t.live, t.liveSpaces[a->space], t.liveSpaces[b->space], fileNow());
    }

    template <typename F> void link(uint32_t sa, uint32_t sb, const char* fn, F f) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        if (!m_sink)
            return;
        const SpaceRef* const a = spaceOf(sa, fn);
        const SpaceRef* const b = spaceOf(sb, fn);
        if (!a || !b || !m_trackers[a->tracker].live || !m_trackers[b->tracker].live)
            return;
        auto& from = m_trackers[a->tracker];
        auto& to = m_trackers[b->tracker];
        f(*from.live, from.liveSpaces[a->space], *to.live, to.liveSpaces[b->space], fileNow());
    }

    int trackerAbort(uint32_t trk) {
        const Recording recording{m_sink, m_sink ? fileNow() : 0};
        if (!m_sink)
            return 0;
        Tracker* const t = trackerOf(trk, "vtr_tracker_abort");
        return t && t->live ? t->live->abort_all(fileNow()) : 0;
    }

    std::optional<vtr::Status> status(char s, const char* fn) {
        const uint8_t value = static_cast<uint8_t>(s);
        if (value >= 1 && value <= 3)
            return static_cast<vtr::Status>(value);
        misuse(std::string{fn} + ": invalid transaction status " + std::to_string(value));
        return {};
    }

private:
    struct Tracker {
        std::string scope;
        std::string kind, generator;
        std::string clock;
        std::vector<std::string> spaces;
        std::unique_ptr<vtr::Tracker> live;
        std::optional<vtr::Source<vtr::Unit, vtr::SourceKind::transaction>> source;
        std::vector<Keyspace> liveSpaces;
        bool recorded = false, associated = false;

        std::string path() const {
            return scope.empty() ? generator : scope + "." + generator;
        }
    };

    struct SpaceRef {
        uint32_t tracker;
        uint32_t space;
    };

    Tracker* trackerOf(uint32_t handle, const char* fn) {
        const uint32_t index = handle & ((1u << KIND_SHIFT) - 1);
        if ((handle >> KIND_SHIFT) != KIND_TRACKER || index == 0 || index > m_trackers.size()) {
            misuse(std::string{fn} + ": handle " + std::to_string(handle) + " is not a tracker");
            return nullptr;
        }
        return &m_trackers[index - 1];
    }

    const SpaceRef* spaceOf(uint32_t handle, const char* fn) {
        const uint32_t index = handle & ((1u << KIND_SHIFT) - 1);
        if ((handle >> KIND_SHIFT) != KIND_KEYSPACE || index == 0 || index > m_spaces.size()) {
            misuse(std::string{fn} + ": handle " + std::to_string(handle) + " is not a key space");
            return nullptr;
        }
        return &m_spaces[index - 1];
    }

    void declare(Tracker& t) {
        auto& writer = m_sink->writer();
        t.source = writer.transactions({.name = t.generator, .parent = m_sink->scopeNode(t.scope)}, t.kind);
        t.recorded = t.associated = false;
        if (const Clock* clock = findClock(t.clock); !t.clock.empty() && clock && clock->id) {
            writer.associate_clock(*t.source, *clock->id);
            t.associated = true;
        }
        t.live = std::make_unique<vtr::Tracker>(writer, *t.source, t.path());
        t.liveSpaces.clear();
        for (const auto& space : t.spaces)
            t.liveSpaces.push_back(t.live->keyspace(space));
    }

    struct Clock {
        std::string scope;
        std::string name;
        std::optional<vtr::ClockSource> id;
        bool running = false;
        bool started = false;
        uint64_t first = 0;
        uint64_t period = 0;
        int unit = 0;

        std::string path() const {
            return scope.empty() ? name : scope + "." + name;
        }
    };

    const Clock* findClock(const std::string& path) const {
        for (const Clock& c : m_clocks)
            if (c.path() == path)
                return &c;
        return nullptr;
    }

    Clock* clockOf(uint32_t handle, const char* fn) {
        const uint32_t index = handle & ((1u << KIND_SHIFT) - 1);
        if ((handle >> KIND_SHIFT) != KIND_CLOCK || index == 0 || index > m_clocks.size()) {
            misuse(std::string{fn} + ": handle " + std::to_string(handle) + " is not a clock");
            return nullptr;
        }
        return &m_clocks[index - 1];
    }

    void declare(Clock& c) {
        auto& writer = m_sink->writer();
        c.id = writer.clock({.name = c.name, .parent = m_sink->scopeNode(c.scope)});
        for (auto& tracker : m_trackers) {
            if (tracker.source && !tracker.recorded && !tracker.associated && tracker.clock == c.path()) {
                writer.associate_clock(*tracker.source, *c.id);
                tracker.associated = true;
            }
        }
    }

    void run(Clock& c) {
        if (!c.id)
            return;
        uint64_t period;
        if (!c.period || !scale(c.period, c.unit, m_fileExp, period)) {
            misuse(c.path() + ": clock period is not a positive whole number of file units");
            return;
        }
        (void)m_sink->writer().run(*c.id, toFile(c.first), period);
        c.started = true;
    }

    uint64_t toFile(uint64_t t) {
        uint64_t v;
        return m_hostExp == m_fileExp ? t : scale(t, m_hostExp, m_fileExp, v) ? v : t;
    }

    uint64_t fileNow() {
        return toFile(m_host.now());
    }

    static bool scale(uint64_t value, int unit, int fileExp, uint64_t& out) {
        if (unit < -128 || unit > 127 || fileExp < -128 || fileExp > 127)
            return false;
        uint64_t v = value;
        for (int e = unit; e > fileExp; --e) {
            if (v > UINT64_MAX / 10)
                return false;
            v *= 10;
        }
        for (int e = unit; e < fileExp; ++e) {
            if (v % 10)
                return false;
            v /= 10;
        }
        out = v;
        return true;
    }

    void misuse(const std::string& message) {
        ++m_misuse[message];
    }

    Host& m_host;
    Sink* m_sink = nullptr;
    int m_fileExp = 0, m_hostExp = 0;
    std::vector<Clock> m_clocks;
    std::vector<Tracker> m_trackers;
    std::vector<SpaceRef> m_spaces;
    std::map<std::string, uint64_t> m_misuse;
};

Runtime& runtime();
[[noreturn]] void failure(const char* message);

template<class F> decltype(auto) checked(F&& call) {
    try { return std::forward<F>(call)(); }
    catch (const std::exception& error) { failure(error.what()); }
}

inline std::string callerScope() {
    const char* const name = svGetNameFromScope(svGetScope());
    return name ? name : "";
}

inline void openChild(unsigned int child_space, unsigned long long child_key, const char* stage,
                       unsigned int parent_space, unsigned long long parent_key,
                       std::optional<uint64_t> at, const char* function) {
    runtime().link(child_space, parent_space, function,
        [&](vtr::Tracker& child, Keyspace cs, vtr::Tracker& parent, Keyspace ps, uint64_t now) {
            if (auto owner = parent.owner(ps, parent_key)) {
                (void)child.open(cs, child_key, stage ? stage : "", at.value_or(now), owner);
            }
        });
}

} // namespace vtr_trace

extern "C" {

unsigned long long vtr_now() {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().now();
    });
}

unsigned int vtr_clock(const char* scope, const char* name) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().clock(vtr_trace::callerScope(), scope, name);
    });
}

void vtr_clock_run(unsigned int c, unsigned long long period, int unit) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().clockRun(c, period, unit);
    });
}

void vtr_clock_stop(unsigned int c) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().clockStop(c);
    });
}

unsigned int
vtr_tracker(const char* scope, const char* generator, const char* kind, const char* clock) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().tracker(vtr_trace::callerScope(), scope, generator, kind, clock);
    });
}

unsigned int vtr_pipeline(const char* scope, const char* generator, const char* clock) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().tracker(
        vtr_trace::callerScope(), scope, generator, "PIPELINE", clock);
    });
}

unsigned int vtr_keyspace(unsigned int trk, const char* name) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().keyspace(trk, name);
    });
}

void vtr_item_open(unsigned int sp, unsigned long long k, const char* stage) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_open", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
        t.open(s, k, stage ? stage : "", now);
        return 0;
    });
    });
}

void vtr_item_open_at(unsigned int sp,
                      unsigned long long k,
                      const char* stage,
                      unsigned long long at) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_open_at", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t) {
        t.open(s, k, stage ? stage : "", at);
        return 0;
    });
    });
}

void vtr_item_bind(unsigned int sp,
                   unsigned long long k,
                   unsigned int to_sp,
                   unsigned long long to_k) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item2(
        sp, to_sp, "vtr_item_bind", [&](vtr::Tracker& t, vtr_trace::Keyspace a, vtr_trace::Keyspace b, uint64_t now) {
            t.bind(a, k, b, to_k, now);
            return 0;
        });
    });
}

int vtr_item_bind_oldest(unsigned int sp, int n, unsigned int to_sp, unsigned long long to_k) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().item2(sp,
                                      to_sp,
                                      "vtr_item_bind_oldest",
                                      [&](vtr::Tracker& t, vtr_trace::Keyspace a, vtr_trace::Keyspace b, uint64_t now) {
                                          return t.bind_oldest(a, static_cast<std::size_t>(std::max(n, 0)), b, to_k, now);
                                      });
    });
}

void vtr_item_stage(unsigned int sp, unsigned long long k, const char* stage) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_stage", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
        t.stage(s, k, stage ? stage : "", now);
        return 0;
    });
    });
}

void vtr_item_stage_at(unsigned int sp,
                       unsigned long long k,
                       const char* stage,
                       unsigned long long at) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_stage_at", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t) {
        t.stage(s, k, stage ? stage : "", at);
        return 0;
    });
    });
}

void vtr_item_lane(unsigned int sp, unsigned long long k, const char* lane, const char* stage) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_lane", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
        t.lane(s, k, lane ? lane : "", stage ? stage : "", now);
        return 0;
    });
    });
}

void vtr_item_event(unsigned int sp, unsigned long long k, const char* name) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_event", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
        t.event(s, k, name ? name : "", now);
        return 0;
    });
    });
}

void vtr_item_attr_u64(unsigned int sp,
                       unsigned long long k,
                       const char* key,
                       unsigned long long v) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_attr_u64", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t) {
        t.attribute(s, k, key ? key : "", static_cast<uint64_t>(v));
        return 0;
    });
    });
}

void vtr_item_attr_str(unsigned int sp, unsigned long long k, const char* key, const char* v) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_attr_str", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t) {
        t.attribute(s, k, key ? key : "", v ? v : "");
        return 0;
    });
    });
}

void vtr_item_label(unsigned int sp, unsigned long long k, const char* text) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().item(sp, "vtr_item_label", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t) {
        t.label(s, k, text ? text : "");
        return 0;
    });
    });
}

void vtr_item_close(unsigned int sp, unsigned long long k, char status) {
    return vtr_trace::checked([&]() {
    vtr_trace::Runtime& rt = vtr_trace::runtime();
    const auto st = rt.status(status, "vtr_item_close");
    if (!st)
        return;
    rt.item(sp, "vtr_item_close", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
        t.close(s, k, *st, now);
        return 0;
    });
    });
}

int vtr_item_abort_younger(unsigned int sp, unsigned long long k) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().item(
        sp, "vtr_item_abort_younger", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
            return t.abort_younger(s, k, now);
        });
    });
}

int vtr_keyspace_abort(unsigned int sp) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().item(
        sp, "vtr_keyspace_abort", [&](vtr::Tracker& t, vtr_trace::Keyspace s, uint64_t now) {
            return t.abort_keyspace(s, now);
        });
    });
}

int vtr_tracker_abort(unsigned int trk) {
    return vtr_trace::checked([&]() {
    return vtr_trace::runtime().trackerAbort(trk);
    });
}

void vtr_item_relate(const char* kind,
                     unsigned int sa,
                     unsigned long long ka,
                     unsigned int sb,
                     unsigned long long kb) {
    return vtr_trace::checked([&]() {
    vtr_trace::runtime().link(
        sa, sb, "vtr_item_relate", [&](vtr::Tracker& a, vtr_trace::Keyspace as, vtr::Tracker& b, vtr_trace::Keyspace bs, uint64_t) {
            a.relate(kind ? kind : "", as, ka, b, bs, kb);
        });
    });
}

void vtr_item_open_child(unsigned int child_space, unsigned long long child_key, const char* stage,
                         unsigned int parent_space, unsigned long long parent_key) {
    return vtr_trace::checked([&]() {
        vtr_trace::openChild(child_space, child_key, stage, parent_space, parent_key,
                             {}, "vtr_item_open_child");
    });
}

void vtr_item_open_child_at(unsigned int child_space, unsigned long long child_key, const char* stage,
                            unsigned int parent_space, unsigned long long parent_key,
                            unsigned long long at) {
    return vtr_trace::checked([&]() {
        vtr_trace::openChild(child_space, child_key, stage, parent_space, parent_key,
                             at, "vtr_item_open_child_at");
    });
}

}

#endif
