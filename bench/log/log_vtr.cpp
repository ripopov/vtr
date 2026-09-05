// VTR: the C++ header-only front end (vtr_log.hpp) over the C API.
//   log_vtr <n> <out.vtr> [--inline]     write (default writer options; --inline = no background thread)
//   log_vtr --decode <file.vtr>          render every record to text (counts bytes, nothing printed)
//   log_vtr --warn <file.vtr>            count records with severity >= WARN (block-pruned query)
#include "bench_common.hpp"
#include "workload.hpp"

#include "vtr.h"
#include "vtr_log.hpp"

#include <cstdio>
#include <cstring>
#include <string>

using namespace simlog;

static vtr::Severity sev_of(uint8_t s) {
    switch (s) {
    case DEBUG: return vtr::Severity::Debug;
    case INFO: return vtr::Severity::Info;
    case WARN: return vtr::Severity::Warn;
    default: return vtr::Severity::Error;
    }
}

static int decode(const char *path, bool print) {
    vtr_reader *r = vtr_reader_open(path);
    if (!r) {
        std::fprintf(stderr, "%s\n", vtr_last_error());
        return 1;
    }
    double t0 = benchutil::now();
    uint64_t lines = 0, bytes = 0;
    char buf[512];
    struct Ctx { uint64_t *lines, *bytes; char *buf; const vtr_reader *r; bool print; } ctx{&lines, &bytes, buf, r, print};
    int rc = vtr_reader_visit_log(
        r, VTR_NONE, VTR_NONE, 0, 0, 0,
        [](void *u, const vtr_log_rec *rec) -> int {
            Ctx *c = static_cast<Ctx *>(u);
            size_t n = vtr_log_rec_format(c->r, rec, c->buf, 512);
            *c->lines += 1;
            *c->bytes += n + 1 + 24; // message + newline + a time/level prefix like the text log
            if (c->print) std::printf("%llu %-5s %s\n", (unsigned long long)rec->time, simlog::sev_name(rec->severity), c->buf);
            return 0;
        },
        &ctx);
    double t1 = benchutil::now();
    vtr_reader_close(r);
    if (rc != VTR_OK) {
        std::fprintf(stderr, "%s\n", vtr_last_error());
        return 1;
    }
    benchutil::Result res{"vtr"};
    res.n = lines;
    res.decode_s = t1 - t0;
    res.lines = lines;
    res.bytes = bytes;
    res.cpu_s = benchutil::cpu_seconds();
    benchutil::print_json(res, print ? stderr : stdout);
    return 0;
}

static int count_warn(const char *path) {
    vtr_reader *r = vtr_reader_open(path);
    if (!r) return 1;
    double t0 = benchutil::now();
    uint64_t lines = 0;
    vtr_reader_visit_log(
        r, VTR_NONE, VTR_NONE, static_cast<uint8_t>(vtr::Severity::Warn), 0, 0,
        [](void *u, const vtr_log_rec *) -> int {
            *static_cast<uint64_t *>(u) += 1;
            return 0;
        },
        &lines);
    double t1 = benchutil::now();
    vtr_reader_close(r);
    benchutil::Result res{"vtr-warn"};
    res.n = lines;
    res.decode_s = t1 - t0;
    res.lines = lines;
    benchutil::print_json(res);
    return 0;
}

int main(int argc, char **argv) {
    if (argc >= 3 && std::strcmp(argv[1], "--decode") == 0) return decode(argv[2], argc > 3 && std::strcmp(argv[3], "--print") == 0);
    if (argc >= 3 && std::strcmp(argv[1], "--warn") == 0) return count_warn(argv[2]);
    if (argc < 3) {
        std::fprintf(stderr, "usage: %s <n> <out.vtr> [--inline] | --decode <file> [--print] | --warn <file>\n", argv[0]);
        return 2;
    }
    uint64_t n = benchutil::parse_count(argv[1]);
    bool inl = argc > 3 && std::strcmp(argv[3], "--inline") == 0;
    vtr_writer_options o;
    vtr_writer_options_default(&o);
    o.background = inl ? 0 : 1;
    vtr_writer *w = vtr_writer_create(argv[2], &o);
    if (!w) {
        std::fprintf(stderr, "%s\n", vtr_last_error());
        return 1;
    }
    vtr_writer_set_timescale(w, -9);
    uint32_t top = vtr_writer_begin_scope(w, "top", 64 /* generic */, "sim");
    vtr::LogStream ls(w, top, "log");
    vtr_writer_end_scope(w);
    Generator gen;
    Msg m{};
    double t0 = benchutil::now();
    for (uint64_t i = 0; i < n; ++i) {
        gen.next(m);
        switch (m.kind) {
#define X(id, SEV, pfmt, ffmt, bfmt, ...)                       \
    case id:                                                    \
        VTR_LOG(ls, sev_of(SEV), m.t, ffmt, __VA_ARGS__);       \
        break;
            SIMLOG_KINDS(X)
#undef X
        default: break;
        }
    }
    double t1 = benchutil::now();
    if (vtr_writer_close(w) != VTR_OK) {
        std::fprintf(stderr, "%s\n", vtr_last_error());
        return 1;
    }
    double t2 = benchutil::now();
    benchutil::Result r{inl ? "vtr-inline" : "vtr"};
    r.n = n;
    r.loop_s = t1 - t0;
    r.total_s = t2 - t0;
    r.cpu_s = benchutil::cpu_seconds();
    r.bytes = benchutil::file_size(argv[2]);
    benchutil::print_json(r);
    return 0;
}
