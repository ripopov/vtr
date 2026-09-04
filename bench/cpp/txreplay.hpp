// Loader for transaction replay files (crates/vtr-bench/src/tx.rs, "VTRTXR01").
#pragma once
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

struct TxOp { uint8_t op; uint64_t id; uint32_t gen_or_key; uint8_t phase, ty; uint64_t v; uint64_t time; uint32_t kind; uint64_t from, to; };
struct TxReplay {
    std::vector<std::string> strings;
    std::vector<uint32_t> streams;
    std::vector<std::pair<uint32_t, uint32_t>> gens;
    std::vector<TxOp> ops;
};
static uint64_t tx_varint(const uint8_t *b, size_t &p) { uint64_t r = 0; int s = 0; for (;;) { uint8_t x = b[p++]; r |= (uint64_t)(x & 0x7f) << s; if (x < 0x80) return r; s += 7; } }
static bool tx_load(const char *path, TxReplay &rp) {
    FILE *f = fopen(path, "rb"); if (!f) { perror(path); return false; }
    fseek(f, 0, SEEK_END); long len = ftell(f); fseek(f, 0, SEEK_SET);
    std::vector<uint8_t> d(len); if (fread(d.data(), 1, len, f) != (size_t)len) return false; fclose(f);
    if (memcmp(d.data(), "VTRTXR01", 8) != 0) { fprintf(stderr, "not a tx replay\n"); return false; }
    size_t p = 8;
    size_t ns = tx_varint(d.data(), p);
    for (size_t i = 0; i < ns; i++) { size_t l = tx_varint(d.data(), p); rp.strings.emplace_back((const char *)d.data() + p, l); p += l; }
    size_t nst = tx_varint(d.data(), p);
    for (size_t i = 0; i < nst; i++) rp.streams.push_back((uint32_t)tx_varint(d.data(), p));
    size_t ng = tx_varint(d.data(), p);
    for (size_t i = 0; i < ng; i++) { uint32_t st = (uint32_t)tx_varint(d.data(), p); uint32_t n = (uint32_t)tx_varint(d.data(), p); rp.gens.push_back({st, n}); }
    size_t nops = tx_varint(d.data(), p);
    rp.ops.reserve(nops);
    for (size_t i = 0; i < nops; i++) {
        TxOp o{}; o.op = d[p++];
        switch (o.op) {
        case 1: o.id = tx_varint(d.data(), p); o.gen_or_key = (uint32_t)tx_varint(d.data(), p); o.time = tx_varint(d.data(), p); break;
        case 2: o.id = tx_varint(d.data(), p); o.gen_or_key = (uint32_t)tx_varint(d.data(), p); o.phase = d[p++]; o.ty = d[p++];
                if (o.ty == 3) { memcpy(&o.v, d.data() + p, 8); p += 8; } else o.v = tx_varint(d.data(), p); break;
        case 3: o.id = tx_varint(d.data(), p); o.time = tx_varint(d.data(), p); break;
        default: o.kind = (uint32_t)tx_varint(d.data(), p); o.from = tx_varint(d.data(), p); o.to = tx_varint(d.data(), p); break;
        }
        rp.ops.push_back(o);
    }
    return true;
}
