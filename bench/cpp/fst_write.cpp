// Replays a workload into the original GTKWave fstapi.c writer or the libfstwriter C++ writer.
// Usage: fst_write <in.rpl> <out.fst> <fstapi|fstcpp> [lz4|zlib]
#include "replay.h"
#include "fstapi.h"
#include "fstcpp/fstcpp_writer.h"
#include <string>
#include <vector>

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: %s <in.rpl> <out.fst> <fstapi|fstcpp> [lz4|zlib]\n", argv[0]); return 2; }
    rp_replay rp;
    if (rp_load(argv[1], &rp)) return 1;
    std::string mode = argv[3];
    bool zlib = argc > 4 && std::string(argv[4]) == "zlib";
    std::vector<uint32_t> handle(rp.n_sig, 0);
    std::vector<char> ascii(1 << 20);
    double w0 = rp_wall(), c0 = rp_cpu();
    uint64_t n = 0, skipped = 0;
    if (mode == "fstapi") {
        fstWriterContext *ctx = fstWriterCreate(argv[2], 1);
        fstWriterSetPackType(ctx, zlib ? FST_WR_PT_ZLIB : FST_WR_PT_LZ4);
        fstWriterSetTimescale(ctx, rp.timescale);
        fstWriterSetVersion(ctx, "vtr-bench");
        for (uint32_t i = 0; i < rp.n_hier; i++) {
            rp_hier &h = rp.hier[i];
            if (h.op == 1) fstWriterSetScope(ctx, FST_ST_VCD_MODULE, h.name, NULL);
            else if (h.op == 2) fstWriterSetUpscope(ctx);
            else {
                rp_sig &s = rp.sigs[h.sig];
                enum fstVarType vt = s.kind == 1 ? FST_VT_VCD_REAL : s.kind == 2 ? FST_VT_GEN_STRING : FST_VT_VCD_WIRE;
                uint32_t len = s.kind == 1 ? 8 : s.kind == 2 ? 0 : s.width;
                fstHandle hd = fstWriterCreateVar(ctx, vt, FST_VD_IMPLICIT, len, h.name, h.op == 4 ? handle[h.sig] : 0);
                if (h.op == 3) handle[h.sig] = hd;
            }
        }
        for (uint32_t s = 0; s < rp.n_sig; s++) if (!handle[s]) {
            rp_sig &sg = rp.sigs[s];
            char nm[32]; snprintf(nm, sizeof nm, "s%u", s);
            handle[s] = fstWriterCreateVar(ctx, sg.kind == 1 ? FST_VT_VCD_REAL : sg.kind == 2 ? FST_VT_GEN_STRING : FST_VT_VCD_WIRE, FST_VD_IMPLICIT, sg.kind == 1 ? 8 : sg.kind == 2 ? 0 : sg.width, nm, 0);
        }
        uint64_t last = UINT64_MAX;
        for (uint64_t i = 0; i < rp.n_rec; i++) {
            rp_rec &r = rp.recs[i];
            if (r.time != last) { fstWriterEmitTimeChange(ctx, r.time); last = r.time; }
            const uint8_t *p = rp.heap + r.off;
            uint32_t w = rp.sigs[r.sig].width;
            switch (r.form) {
            case 0:
                if (w <= 64) { uint64_t v = 0; memcpy(&v, p, r.len); fstWriterEmitValueChange64(ctx, handle[r.sig], w, v); }
                else { uint32_t words[1 << 12]; memset(words, 0, ((w + 31) / 32) * 4); memcpy(words, p, r.len); fstWriterEmitValueChangeVec32(ctx, handle[r.sig], w, words); }
                break;
            case 1: memcpy(ascii.data(), p, r.len); ascii[r.len] = 0; fstWriterEmitValueChange(ctx, handle[r.sig], ascii.data()); break;
            case 2: fstWriterEmitValueChange(ctx, handle[r.sig], p); break;
            default: fstWriterEmitVariableLengthValueChange(ctx, handle[r.sig], p, r.len); break;
            }
            n++;
        }
        if (rp.end_time != last) fstWriterEmitTimeChange(ctx, rp.end_time);
        fstWriterClose(ctx);
    } else {
        fst::Writer w(argv[2]);
        w.setWriterPackType(fst::WriterPackType::LZ4);
        w.setTimecale(rp.timescale);
        for (uint32_t i = 0; i < rp.n_hier; i++) {
            rp_hier &h = rp.hier[i];
            if (h.op == 1) w.setScope(fst::Hierarchy::ScopeType::VCD_MODULE, h.name, "");
            else if (h.op == 2) w.upscope();
            else {
                rp_sig &s = rp.sigs[h.sig];
                auto vt = s.kind == 1 ? fst::Hierarchy::VarType::VCD_REAL : s.kind == 2 ? fst::Hierarchy::VarType::GEN_STRING : fst::Hierarchy::VarType::VCD_WIRE;
                uint32_t len = s.kind == 1 ? 64 : s.kind == 2 ? 0 : s.width;
                uint32_t hd = w.createVar(vt, fst::Hierarchy::VarDirection::IMPLICIT, len, h.name, h.op == 4 ? handle[h.sig] : 0);
                if (h.op == 3) handle[h.sig] = hd;
            }
        }
        for (uint32_t s = 0; s < rp.n_sig; s++) if (!handle[s]) {
            rp_sig &sg = rp.sigs[s];
            char nm[32]; snprintf(nm, sizeof nm, "s%u", s);
            handle[s] = w.createVar(sg.kind == 1 ? fst::Hierarchy::VarType::VCD_REAL : fst::Hierarchy::VarType::VCD_WIRE, fst::Hierarchy::VarDirection::IMPLICIT, sg.kind == 1 ? 64 : sg.width, nm, 0);
        }
        uint64_t last = UINT64_MAX;
        for (uint64_t i = 0; i < rp.n_rec; i++) {
            rp_rec &r = rp.recs[i];
            if (r.time != last) { w.emitTimeChange(r.time); last = r.time; }
            const uint8_t *p = rp.heap + r.off;
            uint32_t wd = rp.sigs[r.sig].width;
            switch (r.form) {
            case 0:
                if (wd <= 64) { uint64_t v = 0; memcpy(&v, p, r.len); w.emitValueChange(handle[r.sig], v); }
                else { uint32_t words[1 << 12]; memset(words, 0, ((wd + 31) / 32) * 4); memcpy(words, p, r.len); w.emitValueChange(handle[r.sig], words, fst::EncodingType::BINARY); }
                break;
            case 1: memcpy(ascii.data(), p, r.len); ascii[r.len] = 0; w.emitValueChange(handle[r.sig], ascii.data()); break;
            case 2: w.emitValueChange(handle[r.sig], (const char *)p); break;
            default: skipped++; continue; // libfstwriter has no variable-length values
            }
            n++;
        }
        if (rp.end_time != last) w.emitTimeChange(rp.end_time);
        w.close();
    }
    double wall = rp_wall() - w0, cpu = rp_cpu() - c0;
    printf("{\"writer\": \"%s%s\", \"records\": %llu, \"skipped\": %llu, \"wall_s\": %.6f, \"cpu_s\": %.6f, \"bytes\": %ld, \"changes_per_s\": %.1f}\n",
           mode.c_str(), zlib ? "-zlib" : "", (unsigned long long)n, (unsigned long long)skipped, wall, cpu, rp_file_size(argv[2]), n / wall);
    return 0;
}
