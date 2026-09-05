// Logging demo (C++): a toy SoC simulation logs through vtr_log.hpp next to
// its transaction stream, then reads its own file back three ways.
//
//   cmake -S demos/logging -B build/logging && cmake --build build/logging
//   ./build/logging/sim_log_demo demo.vtr
//   vtr log demo.vtr --severity warn          # the same with the CLI
#include "vtr.h"
#include "vtr_log.hpp"

#include <cstdio>
#include <cstdlib>
#include <string>

namespace {

// A component with its own LOG stream: every VTR_LOG statement in its methods
// becomes one generator ("log site") of that stream on first use.
struct Dma {
    vtr::LogStream log;
    uint32_t xfers;   // transaction stream for the transfers themselves
    uint32_t gen;
    uint64_t open_tx = 0;
    uint64_t t0 = 0;

    Dma(vtr_writer *w, uint32_t scope) : log(w, scope, "log") {
        xfers = vtr_writer_add_stream(w, scope, "transfers", "TLM");
        gen = vtr_writer_add_generator(w, xfers, "transfer");
    }
    void start(vtr_writer *w, uint64_t t, unsigned ch, uint64_t src, uint64_t dst, uint32_t bytes) {
        vtr_writer_begin_tx(w, gen, t, &open_tx);
        log.set_parent(open_tx); // messages until finish() belong to this transaction
        t0 = t;
        VTR_LOG_INFO(log, t, "channel {} start {} bytes {:#x} -> {:#x}", ch, bytes, src, dst);
    }
    void finish(vtr_writer *w, uint64_t t, unsigned ch, bool error) {
        if (error) {
            VTR_LOG_ERROR(log, t, "channel {} bus error at {:#x} ({})", ch, 0x10000800u, "SLVERR");
            vtr_writer_end_tx(w, open_tx, t, 2 /* error */);
        } else {
            VTR_LOG_INFO(log, t, "channel {} done in {:.2f} us", ch, (t - t0) / 1000.0);
            vtr_writer_end_tx(w, open_tx, t, 1 /* ok */);
        }
        log.set_parent(0);
    }
};

struct Cpu {
    vtr::LogStream log;
    uint64_t pc = 0x80000000;
    explicit Cpu(vtr_writer *w, uint32_t scope) : log(w, scope, "log") {}
    void cycle(uint64_t t, uint64_t n) {
        VTR_LOG_DEBUG(log, t, "fetch pc={:#010x} inst={:#010x}", pc, 0x00400093u ^ n);
        if (n % 3 == 0) VTR_LOG_INFO(log, t + 2, "retire pc={:#010x} rd=x{} value={:#x}", pc, n % 32, n * 0x9e37);
        if (n % 97 == 0) {
            const char *unit = n % 2 ? "fpu" : "lsu";
            VTR_LOG_WARN(log, t + 3, "{} stalled {} cycles waiting for {}", unit, n % 7 + 1, "dcache");
        }
        pc += 4;
    }
};

} // namespace

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "demo_log.vtr";
    vtr_writer *w = vtr_writer_create(path, nullptr);
    if (!w) {
        std::fprintf(stderr, "%s\n", vtr_last_error());
        return 1;
    }
    vtr_writer_set_timescale(w, -9);
    uint32_t soc = vtr_writer_begin_scope(w, "soc", 64 /* generic */, nullptr);
    uint32_t cpu_scope = vtr_writer_begin_scope(w, "cpu0", 68 /* core */, nullptr);
    Cpu cpu(w, cpu_scope);
    vtr_writer_end_scope(w);
    uint32_t dma_scope = vtr_writer_begin_scope(w, "dma", 64, nullptr);
    Dma dma(w, dma_scope);
    vtr_writer_end_scope(w);
    vtr_writer_end_scope(w);
    (void)soc;

    // Simulate 20k cycles at 10 ns.
    uint64_t t = 0;
    bool dma_busy = false;
    unsigned ch = 0;
    for (uint64_t n = 0; n < 20000; ++n) {
        t += 10;
        cpu.cycle(t, n);
        if (n % 500 == 0 && !dma_busy) {
            ch = static_cast<unsigned>(n / 500 % 4);
            dma.start(w, t, ch, 0x10000000, 0x20000000, 4096);
            dma_busy = true;
        }
        if (dma_busy && t - dma.t0 >= 3000) {
            dma.finish(w, t, ch, ch == 3);
            dma_busy = false;
        }
    }
    if (vtr_writer_close(w) != VTR_OK) {
        std::fprintf(stderr, "%s\n", vtr_last_error());
        return 1;
    }

    // Read back.
    vtr_reader *r = vtr_reader_open(path);
    vtr_meta meta;
    vtr_reader_meta(r, &meta);
    std::printf("wrote %s: %llu log records from %u sites, %llu transactions\n", path, (unsigned long long)meta.log_count, meta.log_site_count,
                (unsigned long long)(meta.tx_count - meta.log_count));

    // 1. Everything at WARN or above, rendered to text (block-level pruning by severity).
    std::printf("\n-- severity >= warn --\n");
    int shown = 0, total = 0;
    vtr::for_each_log(r, VTR_NONE, static_cast<uint8_t>(vtr::Severity::Warn), 0, 0, [&](const vtr_log_rec &rec) {
        ++total;
        if (shown++ < 5) std::printf("%8llu ns %-5s %s\n", (unsigned long long)rec.time, vtr::severity_name(rec.severity), vtr::format_log(r, rec).c_str());
        return true;
    });
    std::printf("(%d records)\n", total);

    // 2. The messages of the DMA transfers that failed (parent links).
    std::printf("\n-- messages of failed transfers --\n");
    uint32_t dma_log = VTR_NONE;
    vtr_reader_find_node(r, "soc.dma.log", '.', &dma_log);
    vtr::for_each_log(r, dma_log, 0, 0, 0, [&](const vtr_log_rec &rec) {
        if (rec.severity == static_cast<uint8_t>(vtr::Severity::Error) && rec.parent)
            std::printf("%8llu ns tx %llu: %s\n", (unsigned long long)rec.time, (unsigned long long)rec.parent, vtr::format_log(r, rec).c_str());
        return true;
    });

    // 3. Structured access: the arguments of one site without formatting.
    std::printf("\n-- stall statistics from the site's arguments --\n");
    uint32_t n_sites = vtr_reader_log_site_count(r);
    for (uint32_t i = 0; i < n_sites; ++i) {
        vtr_log_site_info si;
        vtr_reader_log_site(r, i, &si);
        size_t len;
        const char *fmt = vtr_reader_str(r, si.fmt, &len);
        if (std::string(fmt, len).find("stalled") == std::string::npos) continue;
        uint64_t sum = 0, count = 0, worst = 0;
        vtr::for_each_log(r, si.stream, 0, 0, 0, [&](const vtr_log_rec &rec) {
            if (rec.generator != si.node) return true;
            vtr_value v;
            vtr_log_rec_arg(&rec, 1, &v); // the cycle count argument
            sum += v.u;
            ++count;
            if (v.u > worst) worst = v.u;
            return true;
        });
        std::printf("%llu stalls, %.2f cycles on average, worst %llu\n", (unsigned long long)count, count ? double(sum) / count : 0.0, (unsigned long long)worst);
    }
    vtr_reader_close(r);
    return 0;
}
