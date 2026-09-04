// Replays a transaction workload into the LWTR4SC FTR writer (ftr_writer.h, LZ4-compressed).
// Usage: ftr_write <in.txr> <out.ftr> [compressed|raw]
#include "txreplay.hpp"
#include "replay.h"
#include <ftr/ftr_writer.h>

template <bool C> static void run(const TxReplay &rp, const char *out, uint64_t &ntx, uint64_t &nattr, uint64_t &nrel) {
    ftr::ftr_writer<C> w(out);
    w.writeInfo(-9);
    // FTR stream and generator ids share one namespace; use 1..n for streams, then generators.
    std::vector<uint64_t> stream_id(rp.streams.size()), gen_id(rp.gens.size()), gen_stream(rp.gens.size());
    uint64_t next = 1;
    for (size_t i = 0; i < rp.streams.size(); i++) { stream_id[i] = next++; w.writeStream(stream_id[i], rp.strings[rp.streams[i]], "TRANSACTOR"); }
    for (size_t i = 0; i < rp.gens.size(); i++) { gen_id[i] = next++; gen_stream[i] = stream_id[rp.gens[i].first]; w.writeGenerator(gen_id[i], rp.strings[rp.gens[i].second], gen_stream[i]); }
    uint64_t max_id = 0;
    for (auto &o : rp.ops) if (o.op == 1 && o.id > max_id) max_id = o.id;
    std::vector<uint64_t> tx_stream(max_id + 1, 0);
    for (auto &o : rp.ops) {
        switch (o.op) {
        case 1: tx_stream[o.id] = gen_stream[o.gen_or_key]; w.startTransaction(o.id, gen_id[o.gen_or_key], gen_stream[o.gen_or_key], o.time); ntx++; break;
        case 2: {
            auto ev = o.phase == 0 ? ftr::event_type::BEGIN : o.phase == 2 ? ftr::event_type::END : ftr::event_type::RECORD;
            const char *key = rp.strings[o.gen_or_key].c_str();
            switch (o.ty) {
            case 0: w.writeAttribute(o.id, ev, key, ftr::data_type::BOOLEAN, (bool)(o.v != 0)); break;
            case 1: w.writeAttribute(o.id, ev, key, ftr::data_type::INTEGER, (int64_t)o.v); break;
            case 2: w.writeAttribute(o.id, ev, key, ftr::data_type::UNSIGNED, (uint64_t)o.v); break;
            case 3: { double d; memcpy(&d, &o.v, 8); w.writeAttribute(o.id, ev, key, ftr::data_type::FLOATING_POINT_NUMBER, d); break; }
            default: w.writeAttribute(o.id, ev, key, ftr::data_type::STRING, rp.strings[o.v]); break;
            }
            nattr++; break;
        }
        case 3: w.endTransaction(o.id, o.time); break;
        default: w.writeRelation(rp.strings[o.kind], tx_stream[o.to], o.to, tx_stream[o.from], o.from); nrel++; break;
        }
    }
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: %s <in.txr> <out.ftr> [compressed|raw]\n", argv[0]); return 2; }
    TxReplay rp;
    if (!tx_load(argv[1], rp)) return 1;
    bool raw = argc > 3 && strcmp(argv[3], "raw") == 0;
    uint64_t ntx = 0, nattr = 0, nrel = 0;
    double w0 = rp_wall(), c0 = rp_cpu();
    if (raw) run<false>(rp, argv[2], ntx, nattr, nrel); else run<true>(rp, argv[2], ntx, nattr, nrel);
    double wall = rp_wall() - w0, cpu = rp_cpu() - c0;
    printf("{\"writer\": \"ftr%s\", \"transactions\": %llu, \"attributes\": %llu, \"relations\": %llu, \"wall_s\": %.6f, \"cpu_s\": %.6f, \"bytes\": %ld}\n",
           raw ? "-raw" : "", (unsigned long long)ntx, (unsigned long long)nattr, (unsigned long long)nrel, wall, cpu, rp_file_size(argv[2]));
    return 0;
}
