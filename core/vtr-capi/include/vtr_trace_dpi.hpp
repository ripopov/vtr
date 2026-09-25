// vtr_trace_dpi.hpp - DPI bodies of the vtr_trace SystemVerilog package.
//
// Include this header in exactly one C++ file of a simulator integration (the
// sink). It defines the package's DPI-C functions over a small interface the
// sink implements, so a new package function changes this file and
// vtr_trace.sv and nothing in any sink. See docs/vtr_clocks.html.
//
// The integration provides:
//   * vtr_trace::runtime(): the Runtime of the calling simulation context,
//     built over a Host (the context's time source and what $root names);
//   * a vtr_trace::Sink, passed to Runtime::opened() when its file opens:
//     the writer, the file's time unit, a map from instance path to
//     hierarchy node and a warning log;
//   * Runtime::closing() just before the file closes.
// Declarations and each clock's latest run made while no file is open are
// kept and replayed at open.

#ifndef VTR_TRACE_DPI_HPP
#define VTR_TRACE_DPI_HPP

#include "svdpi.h"
#include "vtr.h"

#include <cstdint>
#include <map>
#include <mutex>
#include <string>
#include <vector>

namespace vtr_trace {

/// A simulation context, whether or not a file is open.
class Host {
public:
    virtual ~Host() = default;
    /// Current simulation time in units of 10^timePrecision() s.
    virtual uint64_t now() = 0;
    virtual int timePrecision() = 0;
    /// Absolute instance path that $root stands for ("TOP" in Verilator, "" elsewhere).
    virtual std::string rootPath() = 0;
};

/// What a simulator integration provides for one open file.
class Sink {
public:
    virtual ~Sink() = default;
    /// The open writer.
    virtual vtr_writer* writer() = 0;
    /// The file's time unit as a power of ten seconds.
    virtual int timescale() = 0;
    /// Hierarchy node of a '.'-separated absolute instance path, created when missing.
    virtual uint32_t scopeNode(const std::string& path) = 0;
    /// A warning for the file's simulation log.
    virtual void warn(const std::string& text) = 0;
};

/// Resolves a package scope path against the calling instance `caller`
/// (absolute, '.'-separated). Returns false for a path above the root.
inline bool resolveScope(const std::string& caller, const std::string& root,
                         const std::string& path, std::string& out) {
    if (path.compare(0, 5, "$root") == 0 && (path.size() == 5 || path[5] == '.')) {
        const std::string rest = path.size() > 6 ? path.substr(6) : "";
        out = root.empty() ? rest : rest.empty() ? root : root + "." + rest;
        return true;
    }
    std::string base = caller;
    size_t i = 0;
    while (i < path.size() && path[i] == '^') {
        const size_t dot = base.rfind('.');
        if (base.empty()) return false;
        base = dot == std::string::npos ? "" : base.substr(0, dot);
        ++i;
        if (i < path.size()) {
            if (path[i] != '.') return false;
            ++i;
        }
    }
    const std::string rest = path.substr(i);
    out = base.empty() ? rest : rest.empty() ? base : base + "." + rest;
    return true;
}

/// Package state of one simulation context: handles, declarations kept for
/// replay, and misuse counts.
class Runtime {
public:
    static constexpr uint32_t KIND_SHIFT = 24;
    static constexpr uint32_t KIND_CLOCK = 1;

    explicit Runtime(Host& host)
        : m_host{host} {}

    // ---- Sink side ----------------------------------------------------------------------

    /// A file opened: declare everything and replay each clock's latest run.
    void opened(Sink& sink) {
        const std::lock_guard<std::mutex> lock{m_mutex};
        m_sink = &sink;
        for (Clock& c : m_clocks) {
            declare(c);
            if (c.running) run(c);
        }
    }

    /// The file is about to close: running clocks end at the current time, and the
    /// misuse counts go into the simulation log. Declarations are kept.
    void closing() {
        const std::lock_guard<std::mutex> lock{m_mutex};
        if (!m_sink) return;
        bool running = false;
        for (const Clock& c : m_clocks) running |= c.running && c.id != VTR_NONE;
        // A stretch still running at close ends at its last edge at or before the writer's time.
        if (running) vtr_writer_set_time(m_sink->writer(), fileNow());
        for (const auto& m : m_misuse) {
            m_sink->warn("vtr_trace: " + m.first + " (" + std::to_string(m.second)
                         + (m.second == 1 ? " time)" : " times)"));
        }
        m_misuse.clear();
        for (Clock& c : m_clocks) c.id = VTR_NONE;
        m_sink = nullptr;
    }

    // ---- Package side -------------------------------------------------------------------

    /// Current time in the file's unit (in the context's precision while no file is open).
    uint64_t now() {
        const std::lock_guard<std::mutex> lock{m_mutex};
        return m_sink ? fileNow() : m_host.now();
    }

    uint32_t clock(const std::string& caller, const char* scope, const char* name) {
        const std::lock_guard<std::mutex> lock{m_mutex};
        Clock c;
        c.name = name ? name : "";
        const std::string root = m_host.rootPath();
        if (!resolveScope(caller, root, scope ? scope : "", c.scope)) {
            misuse("vtr_clock(\"" + std::string{scope ? scope : ""} + "\", \"" + c.name
                   + "\") in " + caller + ": scope above the root, using the root");
            c.scope = root;
        }
        m_clocks.push_back(c);
        if (m_sink) declare(m_clocks.back());
        return (KIND_CLOCK << KIND_SHIFT) | static_cast<uint32_t>(m_clocks.size());
    }

    void clockRun(uint32_t handle, uint64_t period, int unit) {
        const std::lock_guard<std::mutex> lock{m_mutex};
        Clock* const cp = clockOf(handle, "vtr_clock_run");
        if (!cp) return;
        if (cp->running) {
            misuse(cp->path() + ": vtr_clock_run on a running clock; call vtr_clock_stop first");
            return;
        }
        cp->period = period;
        cp->unit = unit;
        cp->first = m_host.now();
        cp->running = true;
        if (m_sink) run(*cp);
    }

    void clockStop(uint32_t handle) {
        const std::lock_guard<std::mutex> lock{m_mutex};
        Clock* const cp = clockOf(handle, "vtr_clock_stop");
        if (!cp || !cp->running) return;
        cp->running = false;
        if (m_sink && cp->id != VTR_NONE && cp->started) {
            check(vtr_writer_clock_stop(m_sink->writer(), cp->id, fileNow()), cp->path());
        }
        cp->started = false;
    }

private:
    struct Clock {
        std::string scope;  // absolute instance path
        std::string name;
        uint32_t id = VTR_NONE;  // clock id in the open file
        bool running = false;  // latest call was a run
        bool started = false;  // that run reached the open file
        uint64_t first = 0;  // first edge of the latest run, in the context's precision
        uint64_t period = 0;
        int unit = 0;
        std::string path() const { return scope.empty() ? name : scope + "." + name; }
    };

    Clock* clockOf(uint32_t handle, const char* fn) {
        const uint32_t index = handle & ((1u << KIND_SHIFT) - 1);
        if ((handle >> KIND_SHIFT) != KIND_CLOCK || index == 0 || index > m_clocks.size()) {
            misuse(std::string{fn} + ": handle " + std::to_string(handle) + " is not a clock");
            return nullptr;
        }
        return &m_clocks[index - 1];
    }

    void declare(Clock& c) {
        vtr_writer* const w = m_sink->writer();
        c.id = vtr_writer_add_clock(w, m_sink->scopeNode(c.scope), c.name.c_str());
        if (c.id == VTR_NONE) misuse(c.path() + ": " + vtr_last_error());
    }

    /// The clock's latest run reaches the open file.
    void run(Clock& c) {
        if (c.id == VTR_NONE) return;
        uint64_t period;
        if (c.period == 0 || !scale(c.period, c.unit, m_sink->timescale(), period)) {
            misuse(c.path() + ": period " + std::to_string(c.period) + " x 10^" + std::to_string(c.unit)
                   + " s is not a whole number of file units (10^"
                   + std::to_string(m_sink->timescale()) + " s)");
            return;
        }
        c.started = check(vtr_writer_clock_run(m_sink->writer(), c.id, toFile(c.first), period), c.path());
    }

    /// A context time in the file's unit.
    uint64_t toFile(uint64_t t) {
        uint64_t v;
        return scale(t, m_host.timePrecision(), m_sink->timescale(), v) ? v : t;
    }
    uint64_t fileNow() { return toFile(m_host.now()); }

    /// Converts `value` x 10^unit s to units of 10^fileExp s with integers only; false
    /// when the result is not whole or does not fit.
    static bool scale(uint64_t value, int unit, int fileExp, uint64_t& out) {
        uint64_t v = value;
        for (int e = unit; e > fileExp; --e) {
            if (v > UINT64_MAX / 10) return false;
            v *= 10;
        }
        for (int e = unit; e < fileExp; ++e) {
            if (v % 10) return false;
            v /= 10;
        }
        out = v;
        return true;
    }

    bool check(int status, const std::string& what) {
        if (status == VTR_OK) return true;
        misuse(what + ": " + vtr_last_error());
        return false;
    }

    void misuse(const std::string& message) { ++m_misuse[message]; }

    std::mutex m_mutex;
    Host& m_host;
    Sink* m_sink = nullptr;
    std::vector<Clock> m_clocks;
    std::map<std::string, uint64_t> m_misuse;
};

/// The Runtime of the calling simulation context; defined by the sink.
Runtime& runtime();

/// Absolute path of the instance making the current context import call.
inline std::string callerScope() {
    const char* const name = svGetNameFromScope(svGetScope());
    return name ? name : "";
}

}  // namespace vtr_trace

// ---- DPI-C functions of package vtr_trace ------------------------------------------------

extern "C" {

unsigned long long vtr_now() { return vtr_trace::runtime().now(); }

unsigned int vtr_clock(const char* scope, const char* name) {
    return vtr_trace::runtime().clock(vtr_trace::callerScope(), scope, name);
}

void vtr_clock_run(unsigned int c, unsigned long long period, int unit) {
    vtr_trace::runtime().clockRun(c, period, unit);
}

void vtr_clock_stop(unsigned int c) { vtr_trace::runtime().clockStop(c); }

}  // extern "C"

#endif  // VTR_TRACE_DPI_HPP
