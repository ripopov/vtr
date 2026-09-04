// Converts a Kanata pipeline log into FTR (LZ4) using the natural FTR mapping:
// one stream per thread, an "instruction" generator for ops and a "stage" generator for
// pipeline stages (child transactions linked with a "parent_of" relation, attribute "lane"),
// labels as string attributes, retire id / flush as end attributes, W as "wakeup" relations.
// Usage: konata_ftr <in.log> <out.ftr>
#include "replay.h"
#include <ftr/ftr_writer.h>
#include <deque>
#include <fstream>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>

struct Stage { uint64_t tx; std::string name; uint32_t lane; bool open; };
struct Op { uint64_t tx; uint64_t stream, gen_stage; std::vector<Stage> stages; std::string label, detail; bool retired = false; uint64_t retire_cycle = 0; };
struct Thread { uint64_t stream, gen_insn, gen_stage; };

struct Counts { uint64_t insn = 0, stage = 0, rel = 0; };

static Counts run(const char *in_path, const char *out_path) {
    Counts cn;
    std::ifstream in(in_path);
    if (!in) { perror(in_path); exit(1); }
    std::string line;
    std::getline(in, line);
    if (line.rfind("Kanata", 0) != 0) { fprintf(stderr, "not a Kanata log\n"); exit(1); }
    ftr::ftr_writer<true> w(out_path);
    uint64_t &n_insn = cn.insn, &n_stage = cn.stage, &n_rel = cn.rel;
    w.writeInfo(0);
    std::unordered_map<uint64_t, Thread> threads;
    std::unordered_map<uint64_t, Op> ops;
    std::deque<uint64_t> retired;
    uint64_t next_id = 1, next_tx = 1;
    int64_t cycle = 0;
    auto flush = [&](uint64_t id) {
        auto it = ops.find(id);
        if (it == ops.end()) return;
        Op &o = it->second;
        if (!o.label.empty()) w.writeAttribute(o.tx, ftr::event_type::RECORD, "label", ftr::data_type::STRING, o.label);
        if (!o.detail.empty()) w.writeAttribute(o.tx, ftr::event_type::RECORD, "detail", ftr::data_type::STRING, o.detail);
        for (auto &s : o.stages) if (s.open) w.endTransaction(s.tx, o.retire_cycle);
        w.endTransaction(o.tx, o.retire_cycle);
        ops.erase(it);
    };
    while (std::getline(in, line)) {
        if (line.empty()) continue;
        std::vector<std::string> f;
        size_t p = 0;
        while (true) { size_t q = line.find('\t', p); f.push_back(line.substr(p, q == std::string::npos ? std::string::npos : q - p)); if (q == std::string::npos) break; p = q + 1; }
        const std::string &cmd = f[0];
        auto num = [&](size_t i) -> int64_t { return i < f.size() ? std::strtoll(f[i].c_str(), nullptr, 10) : 0; };
        if (cmd == "C=") cycle = num(1);
        else if (cmd == "C") cycle += num(1);
        else if (cmd == "I") {
            uint64_t id = num(1); int64_t gid = num(2); uint64_t tid = num(3);
            auto th = threads.find(tid);
            if (th == threads.end()) {
                Thread t; t.stream = next_id++; t.gen_insn = next_id++; t.gen_stage = next_id++;
                w.writeStream(t.stream, "thread" + std::to_string(tid), "PIPELINE");
                w.writeGenerator(t.gen_insn, "instruction", t.stream);
                w.writeGenerator(t.gen_stage, "stage", t.stream);
                th = threads.emplace(tid, t).first;
            }
            Op o; o.tx = next_tx++; o.stream = th->second.stream; o.gen_stage = th->second.gen_stage;
            w.startTransaction(o.tx, th->second.gen_insn, o.stream, (uint64_t)(cycle < 0 ? 0 : cycle));
            w.writeAttribute(o.tx, ftr::event_type::BEGIN, "insn_id_in_sim", ftr::data_type::INTEGER, gid);
            ops[id] = std::move(o); n_insn++;
        } else if (cmd == "L") {
            auto it = ops.find(num(1)); if (it == ops.end()) continue;
            int64_t ty = num(2); const std::string &text = f.size() > 3 ? f[3] : "";
            if (ty == 0) it->second.label += text;
            else if (ty == 1) it->second.detail += text;
            else if (!it->second.stages.empty()) w.writeAttribute(it->second.stages.back().tx, ftr::event_type::RECORD, "label", ftr::data_type::STRING, text);
        } else if (cmd == "S" || cmd == "E") {
            auto it = ops.find(num(1)); if (it == ops.end() || it->second.retired) continue;
            Op &o = it->second; uint32_t lane = (uint32_t)num(2); const std::string &st = f.size() > 3 ? f[3] : "";
            uint64_t t = cycle < 0 ? 0 : cycle;
            if (cmd == "S") {
                for (auto r = o.stages.rbegin(); r != o.stages.rend(); ++r) if (r->lane == lane && r->open) { w.endTransaction(r->tx, t); r->open = false; break; }
                Stage s; s.tx = next_tx++; s.name = st; s.lane = lane; s.open = true;
                w.startTransaction(s.tx, o.gen_stage, o.stream, t);
                w.writeAttribute(s.tx, ftr::event_type::BEGIN, "name", ftr::data_type::STRING, st);
                w.writeAttribute(s.tx, ftr::event_type::BEGIN, "lane", ftr::data_type::UNSIGNED, (uint64_t)lane);
                w.writeRelation("parent_of", o.stream, s.tx, o.stream, o.tx);
                o.stages.push_back(s); n_stage++; n_rel++;
            } else {
                for (auto r = o.stages.rbegin(); r != o.stages.rend(); ++r) if (r->lane == lane && r->name == st && r->open) { w.endTransaction(r->tx, t); r->open = false; break; }
            }
        } else if (cmd == "R") {
            auto it = ops.find(num(1)); if (it == ops.end()) continue;
            Op &o = it->second; o.retired = true; o.retire_cycle = cycle < 0 ? 0 : cycle;
            w.writeAttribute(o.tx, ftr::event_type::END, "retire_id", ftr::data_type::INTEGER, (int64_t)num(2));
            w.writeAttribute(o.tx, ftr::event_type::END, "flushed", ftr::data_type::BOOLEAN, (bool)(num(3) == 1));
            retired.push_back(num(1));
            if (retired.size() > 4096) { uint64_t old = retired.front(); retired.pop_front(); flush(old); }
        } else if (cmd == "W") {
            auto c = ops.find(num(1)); auto pr = ops.find(num(2));
            if (c != ops.end() && pr != ops.end()) { w.writeRelation("wakeup", c->second.stream, c->second.tx, pr->second.stream, pr->second.tx); n_rel++; }
        }
    }
    while (!retired.empty()) { uint64_t old = retired.front(); retired.pop_front(); flush(old); }
    std::vector<uint64_t> rest; for (auto &kv : ops) rest.push_back(kv.first);
    for (uint64_t id : rest) { ops[id].retire_cycle = cycle < 0 ? 0 : cycle; flush(id); }
    return cn; // the writer's destructor flushes the file
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s <in.log> <out.ftr>\n", argv[0]); return 2; }
    double w0 = rp_wall(), c0 = rp_cpu();
    Counts cn = run(argv[1], argv[2]);
    double wall = rp_wall() - w0, cpu = rp_cpu() - c0;
    printf("{\"writer\": \"ftr\", \"transactions\": %llu, \"stages\": %llu, \"relations\": %llu, \"wall_s\": %.6f, \"cpu_s\": %.6f, \"bytes\": %ld}\n",
           (unsigned long long)cn.insn, (unsigned long long)cn.stage, (unsigned long long)cn.rel, wall, cpu, rp_file_size(argv[2]));
    return 0;
}
