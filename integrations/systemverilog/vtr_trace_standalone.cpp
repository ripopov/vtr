// vtr_trace_standalone.cpp - the vtr_trace package for simulators other than
// the VTR Verilator fork (docs/c910-verilator-tx-stream.html).
//
// Compile this file, core/vtr-capi/include/vtr_trace.sv and the design with any
// simulator that supports DPI-C and VPI, and link libvtr. The package then
// writes its own VTR file, named by the plusarg +vtr_trace=<file>: declared
// clocks, trackers' transactions and the package's warnings, in the
// simulator's time precision. Without the plusarg every call does nothing.
//
// The file opens at the first package call (a constructor in an initial block)
// and closes at the end-of-simulation callback, or at process exit when the
// simulator never reports one. Time comes from vpi_get_time; the calling
// instance from svGetScope. Everything else is vtr_trace_dpi.hpp, shared with
// the fork's sink, so clocks and trackers behave identically.

#include "vtr_trace_dpi.hpp"

#include "vpi_user.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <string>

namespace {

// The simulation, as the package sees it.
class VpiHost final : public vtr_trace::Host {
public:
    uint64_t now() override {
        s_vpi_time t{};
        t.type = vpiSimTime;
        vpi_get_time(nullptr, &t);
        return (static_cast<uint64_t>(t.high) << 32) | t.low;
    }
    int timePrecision() override { return vpi_get(vpiTimePrecision, nullptr); }
    // Verilator names every instance below a TOP wrapper; other simulators start
    // instance paths at the top module.
    std::string rootPath() override { return m_verilator ? "TOP" : ""; }
    void identify() {
        s_vpi_vlog_info info{};
        if (vpi_get_vlog_info(&info) && info.product) m_verilator = std::strstr(info.product, "Verilator") != nullptr;
    }

private:
    bool m_verilator = false;
};

// A VTR file of its own: the package's declarations, transactions and warnings.
class FileSink final : public vtr_trace::Sink {
public:
    FileSink(vtr_writer* w, int timescale)
        : m_writer{w}
        , m_timescale{timescale} {
        const uint32_t log = vtr_writer_add_stream(w, VTR_NONE, "simulation_log", VTR_LOG_STREAM_KIND);
        const uint8_t text = VTR_VAL_TEXT;
        const char* const name = "message";
        m_warnSite = vtr_writer_add_log_site(w, log, VTR_SEVERITY_WARN, "{}", nullptr, 0, nullptr, 1, &text, &name);
    }
    vtr_writer* writer() override { return m_writer; }
    int timescale() override { return m_timescale; }
    uint32_t scopeNode(const std::string& path) override {
        if (path.empty()) return VTR_NONE;
        const auto it = m_scopes.find(path);
        if (it != m_scopes.end()) return it->second;
        const size_t dot = path.rfind('.');
        const uint32_t parent = dot == std::string::npos ? VTR_NONE : scopeNode(path.substr(0, dot));
        const std::string name = dot == std::string::npos ? path : path.substr(dot + 1);
        return m_scopes[path] = vtr_writer_add_scope(m_writer, parent, name.c_str(), VTR_SCOPE_MODULE, nullptr);
    }
    void warn(const std::string& text) override {
        vpi_printf(const_cast<char*>("%%Warning-VTRTRACE: %s\n"), text.c_str());
        vtr_value value{};
        value.tag = VTR_VAL_TEXT;
        value.data = reinterpret_cast<const uint8_t*>(text.data());
        value.len = text.size();
        vtr_writer_log(m_writer, m_warnSite, m_now, 0, 1, &value, nullptr);
    }
    uint64_t m_now = 0;  // the file-unit time of warnings written at close

private:
    vtr_writer* const m_writer;
    const int m_timescale;
    uint32_t m_warnSite = VTR_NONE;
    std::map<std::string, uint32_t> m_scopes;
};

struct Standalone {
    VpiHost host;
    vtr_trace::Runtime runtime{host};
    std::unique_ptr<FileSink> sink;
    bool started = false;

    // First package call: open the file named by +vtr_trace=, if any.
    void start() {
        started = true;
        host.identify();
        s_vpi_vlog_info info{};
        std::string path;
        if (vpi_get_vlog_info(&info)) {
            for (int i = 0; i < info.argc; ++i) {
                if (std::strncmp(info.argv[i], "+vtr_trace=", 11) == 0) path = info.argv[i] + 11;
            }
        }
        if (path.empty()) return;
        vtr_writer* const w = vtr_writer_create(path.c_str(), nullptr);
        if (!w) {
            vpi_printf(const_cast<char*>("%%Error: vtr_trace: cannot create %s: %s\n"), path.c_str(), vtr_last_error());
            return;
        }
        const int precision = host.timePrecision();
        vtr_writer_set_timescale(w, static_cast<int8_t>(precision));
        vtr_writer_set_writer_name(w, "vtr_trace standalone sink");
        sink.reset(new FileSink{w, precision});
        s_cb_data cb{};
        cb.reason = cbEndOfSimulation;
        cb.cb_rtn = [](p_cb_data) -> PLI_INT32 {
            instance().finish();
            return 0;
        };
        vpi_register_cb(&cb);
        std::atexit([] { instance().finish(); });
        runtime.opened(*sink);
    }

    // End of simulation: close running clocks and open items, then the file.
    void finish() {
        if (!sink) return;
        sink->m_now = host.now();
        runtime.closing();
        vtr_writer* const w = sink->writer();
        sink.reset();
        if (vtr_writer_close(w) != VTR_OK)
            vpi_printf(const_cast<char*>("%%Error: vtr_trace: closing the file failed: %s\n"), vtr_last_error());
    }

    static Standalone& instance() {
        static Standalone s;
        return s;
    }
};

}  // namespace

vtr_trace::Runtime& vtr_trace::runtime() {
    Standalone& s = Standalone::instance();
    if (!s.started) s.start();
    return s.runtime;
}
