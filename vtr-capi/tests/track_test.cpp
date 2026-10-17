// Writes the recordings that tests/track.rs reads back: part A drives vtr::Tracker
// (vtr_track.hpp) directly, part B the vtr_trace package's DPI functions
// (vtr_trace_dpi.hpp) over a test host and sink.

#include "vtr_trace_dpi.hpp"

#include <cstdio>
#include <map>
#include <string>
#include <vector>

const char* g_test_scope = "";

namespace {

class TestHost final : public vtr_trace::Host {
public:
    uint64_t t = 0;
    uint64_t now() override { return t; }
    int timePrecision() override { return -12; }
    std::string rootPath() override { return "TOP"; }
};

class TestSink final : public vtr_trace::Sink {
public:
    vtr_writer* w = nullptr;
    std::map<std::string, uint32_t> nodes;
    std::vector<std::string> warnings;
    vtr_writer* writer() override { return w; }
    int timescale() override { return -12; }
    uint32_t scopeNode(const std::string& path) override {
        if (path.empty()) return VTR_NONE;
        auto it = nodes.find(path);
        if (it != nodes.end()) return it->second;
        const size_t dot = path.rfind('.');
        const uint32_t parent = dot == std::string::npos ? VTR_NONE : scopeNode(path.substr(0, dot));
        const std::string name = dot == std::string::npos ? path : path.substr(dot + 1);
        return nodes[path] = vtr_writer_add_scope(w, parent, name.c_str(), VTR_SCOPE_MODULE, nullptr);
    }
    void warn(const std::string& text) override { warnings.push_back(text); }
};

TestHost s_host;
vtr_trace::Runtime s_runtime{s_host};

int fail(const char* what) {
    std::fprintf(stderr, "track_test: %s: %s\n", what, vtr_last_error());
    return 1;
}

int partA(const std::string& path) {
    vtr_writer* w = vtr_writer_create(path.c_str(), nullptr);
    if (!w) return fail("create");
    vtr_writer_set_timescale(w, -9);
    const uint32_t cpu = vtr_writer_add_scope(w, VTR_NONE, "cpu", VTR_SCOPE_MODULE, nullptr);
    const uint32_t pg = vtr_writer_add_generator(w, cpu, "pipeline", "PIPELINE");
    const uint32_t lg = vtr_writer_add_generator(w, cpu, "lsu", "LSU");
    vtr::Tracker p{w, pg, "cpu.pipeline"}, l{w, lg, "cpu.lsu"};
    const uint32_t fe = p.keyspace("fe"), iid = p.keyspace("iid"), preg = p.keyspace("preg"), sq = l.keyspace("sq");

    p.open(fe, 1, "ID", 10);
    p.attr(fe, 1, "pc", uint64_t{0x100});
    p.label(fe, 1, "one");
    p.open(fe, 2, "ID", 10);
    p.open(fe, 3, "ID", 10);
    for (uint64_t k = 1; k <= 3; ++k) p.stage(fe, k, "IR", 11);
    const int n1 = p.bind_oldest(fe, 2, iid, 5, 12);
    p.stage(iid, 5, "IQ", 12);
    const int n2 = p.bind_oldest(fe, 1, iid, 6, 12);
    p.stage(iid, 6, "IQ", 12);
    p.bind(iid, 6, preg, 40, 12);
    p.stage(preg, 40, "EX", 13);
    p.lane(preg, 40, "mem", "DC", 13);
    p.open(fe, 4, "ID", 13);
    p.open(fe, 5, "ID", 13);
    p.lane(preg, 40, "mem", "", 14);
    p.stage(iid, 6, "EX", 14);
    p.attr(iid, 5, "pc", uint64_t{0x1000});
    p.attr(iid, 6, "pc", uint64_t{0x2000});
    p.event(iid, 5, "replay", 14);
    p.close(iid, 5, VTR_TX_STATUS_OK, 15);
    const int y = p.abort_younger(iid, 6, 15);
    p.stage(fe, 4, "IR", 16);
    p.open(fe, 7, "ID", 16);
    l.open(sq, 0, "Q", 16);
    l.parent(sq, 0, p, iid, 6);
    p.relate("wakeup", iid, 6, p, fe, 7);
    p.open(fe, 8, "ID", 16);
    p.bind(fe, 8, iid, 9, 16);
    p.open(fe, 9, "ID", 16);
    p.open(fe, 7, "ID", 17);
    p.bind(fe, 9, iid, 9, 17);
    const int k = p.abort_keyspace(fe, 18);
    l.close(sq, 0, VTR_TX_STATUS_OK, 19);
    l.open(sq, 1, "Q", 19);
    l.open(sq, 2, "Q", 19);
    const int a = l.abort_all(19);
    std::printf("RET %d %d %d %d %d\n", n1, n2, y, k, a);
    std::printf("OPEN %zu %zu\n", p.open_count(), l.open_count());
    vtr_writer_set_time(w, 20);
    p.detach();
    l.detach();
    std::printf("OPEN %zu %zu\n", p.open_count(), l.open_count());
    for (const std::string& d : p.take_diagnostics()) std::printf("DIAG %s\n", d.c_str());
    for (const std::string& d : l.take_diagnostics()) std::printf("DIAG %s\n", d.c_str());
    return vtr_writer_close(w) == VTR_OK ? 0 : fail("close A");
}

int partC(const std::string& path) {
    vtr_writer* w = vtr_writer_create(path.c_str(), nullptr);
    if (!w) return fail("create");
    const uint32_t gen = vtr_writer_add_generator(w, VTR_NONE, "many", "PIPELINE");
    vtr::Tracker q{w, gen, "many"};
    const uint32_t s = q.keyspace("s"), g = q.keyspace("g");
    for (uint64_t k = 0; k < 1000; ++k) q.open(s, k, "A", 1);
    const int moved = q.bind_oldest(s, 6, g, 1, 2);
    q.stage(g, 1, "X", 2);
    q.attr(g, 1, "folded", uint64_t{6});
    q.close(g, 1, VTR_TX_STATUS_OK, 3);
    for (uint64_t i = 0; i < 994; ++i) q.close(s, 6 + (i * 7919) % 994, VTR_TX_STATUS_OK, 4);
    std::printf("C %d %zu %zu\n", moved, q.open_count(), q.diagnostics().size());
    return vtr_writer_close(w) == VTR_OK ? 0 : fail("close C");
}

int openFile(TestSink& sink, const std::string& path) {
    sink.w = vtr_writer_create(path.c_str(), nullptr);
    if (!sink.w) return fail("create");
    vtr_writer_set_timescale(sink.w, -12);
    sink.nodes.clear();
    sink.warnings.clear();
    s_runtime.opened(sink);
    return 0;
}

int closeFile(TestSink& sink, const char* tag) {
    s_runtime.closing();
    for (const std::string& t : sink.warnings) std::printf("%s %s\n", tag, t.c_str());
    return vtr_writer_close(sink.w) == VTR_OK ? 0 : fail("close");
}

int partB(const std::string& b1, const std::string& b2) {
    TestSink sink;
    g_test_scope = "TOP.tb.core.u_vtr";
    s_host.t = 50;
    const unsigned trk = vtr_pipeline("^", "pipeline", "clk");
    const unsigned c = vtr_clock("^", "clk");
    const unsigned sn = vtr_keyspace(trk, "sn");
    const unsigned other = vtr_tracker("", "bus", "BUS", "nope");
    const unsigned grouped = vtr_keyspace(vtr_tracker("/TX.core0", "events", "EVENTS", ""), "ev");
    vtr_clock_run(c, 10, -12);
    vtr_item_open(sn, 1, "F");
    if (openFile(sink, b1)) return 1;
    s_host.t = 100;
    vtr_item_open(sn, 1, "F");
    vtr_item_attr_u64(sn, 1, "pc", 0x80);
    s_host.t = 110;
    const unsigned long long saved = vtr_now();
    s_host.t = 120;
    vtr_item_open_at(sn, 2, "F", saved);
    vtr_item_stage(sn, 1, "D");
    s_host.t = 130;
    vtr_item_close(sn, 1, VTR_TX_STATUS_OK);
    vtr_item_close(sn, 2, 7);
    vtr_item_stage(trk, 1, "X");
    const unsigned rob = vtr_keyspace(trk, "rob");
    s_host.t = 140;
    vtr_item_bind(sn, 2, rob, 3);
    vtr_item_label(rob, 3, "two");
    vtr_item_bind(sn, 2, vtr_keyspace(other, "id"), 1);
    s_host.t = 150;
    if (closeFile(sink, "WARN1")) return 1;
    vtr_item_open(sn, 9, "F");
    if (openFile(sink, b2)) return 1;
    s_host.t = 200;
    vtr_item_stage(rob, 3, "X");
    vtr_item_open(sn, 5, "F");
    vtr_item_open(grouped, 1, "E");
    std::printf("ABORT %d\n", vtr_tracker_abort(other));
    s_host.t = 210;
    return closeFile(sink, "WARN2");
}

}

vtr_trace::Runtime& vtr_trace::runtime() { return s_runtime; }

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: track_test <out-dir>\n");
        return 2;
    }
    const std::string dir = argv[1];
    if (partA(dir + "/track_a.vtr")) return 1;
    if (partC(dir + "/track_c.vtr")) return 1;
    return partB(dir + "/track_b1.vtr", dir + "/track_b2.vtr");
}
