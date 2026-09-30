// The crash matrix's simulated run (tests/crash.rs, docs/crash-safe-vtr.html).
//
// The main thread writes value changes and a log record every 64 time steps
// into a VTR writer, then ends as <mode> says:
//
//   none      closes the writer and returns 0
//   segv      null pointer write (SIGSEGV)
//   abort     abort() (SIGABRT)
//   throw     an uncaught C++ exception (std::terminate, SIGABRT)
//   stack     unbounded recursion (SIGSEGV on the guard page)
//   heap      double free of a small block (glibc's tcache check, SIGABRT)
//   heaplock  double free of a 64 KiB block (detected under the main arena's lock, SIGABRT)
//   term      SIGTERM while the simulation runs
//   exit      exit(3) from deep code
//   kill      SIGKILL
//   thread    a second thread faults while the owner keeps writing
//
// usage: crash_harness <mode> <out.vtr> [--records N] [--signals S] [--block-records B] [--inline]
//
// Before it dies the harness prints "emitted changes=N logs=M time=T" on
// stderr: what the writer accepted up to that point.
#include "vtr.h"

#include <atomic>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <pthread.h>
#include <stdexcept>
#include <string>
#include <sys/resource.h>
#include <unistd.h>

namespace {

uint64_t rng = 0x9E3779B97F4A7C15ull;
inline uint64_t next() {
    rng ^= rng << 13;
    rng ^= rng >> 7;
    rng ^= rng << 17;
    return rng;
}

#pragma GCC diagnostic ignored "-Winfinite-recursion"
int recurse(int n) {
    volatile char pad[4096];
    pad[0] = char(n);
    return recurse(n + 1) + pad[0];
}

std::atomic<bool> g_crash_thread{false};

void *crasher(void *) {
    while (!g_crash_thread.load()) usleep(100);
    *static_cast<volatile int *>(nullptr) = 1;
    return nullptr;
}

[[noreturn]] void die(const std::string &mode) {
    // Function pointers through volatile keep the compiler from eliding the heap misuse.
    void *(*volatile m)(size_t) = malloc;
    void (*volatile f)(void *) = free;
    if (mode == "segv") *static_cast<volatile int *>(nullptr) = 1;
    if (mode == "abort") abort();
    if (mode == "stack") recurse(0);
    if (mode == "throw") throw std::runtime_error("uncaught");
    if (mode == "heap") {
        void *p = m(64);
        f(p);
        f(p);
    }
    if (mode == "heaplock") {
        void *p = m(64 * 1024 - 64);
        void *guard = m(64);
        (void)guard;
        f(p);
        f(p);
    }
    if (mode == "exit") exit(3);
    if (mode == "kill") kill(getpid(), SIGKILL);
    if (mode == "term") kill(getpid(), SIGTERM);
    for (;;) pause();
}

}  // namespace

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: crash_harness <mode> <out.vtr> [options]\n");
        return 2;
    }
    const rlimit no_core{0, 0};
    setrlimit(RLIMIT_CORE, &no_core);
    const std::string mode = argv[1];
    const char *path = argv[2];
    uint64_t target = 3000000, signals = 2000, block_records = 1 << 20;
    bool inline_encoder = false;
    for (int i = 3; i < argc; ++i) {
        const std::string a = argv[i];
        if (a == "--records") target = strtoull(argv[++i], nullptr, 10);
        else if (a == "--signals") signals = strtoull(argv[++i], nullptr, 10);
        else if (a == "--block-records") block_records = strtoull(argv[++i], nullptr, 10);
        else if (a == "--inline") inline_encoder = true;
    }
    vtr_writer_options o;
    vtr_writer_options_default(&o);
    o.dedup = 0;  // as in the Verilator fork: every emit is a change
    o.block_records = block_records;
    o.background = inline_encoder ? 0 : 1;
    vtr_writer *w = vtr_writer_create(path, &o);
    if (!w) {
        fprintf(stderr, "create: %s\n", vtr_last_error());
        return 2;
    }
    const uint32_t top = vtr_writer_add_scope(w, VTR_NONE, "top", VTR_SCOPE_MODULE, nullptr);
    uint32_t *sig = new uint32_t[signals];
    for (uint64_t s = 0; s < signals; ++s) {
        char name[32];
        snprintf(name, sizeof name, "s%llu", static_cast<unsigned long long>(s));
        vtr_writer_add_var(w, top, name, VTR_VAR_WIRE, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 32, 2, nullptr, &sig[s]);
    }
    const uint32_t stream = vtr_writer_add_stream(w, VTR_NONE, "simulation_log", VTR_LOG_STREAM_KIND);
    const uint8_t u64 = VTR_VAL_U64;
    const char *step = "step";
    const uint32_t tick = vtr_writer_add_log_site(w, stream, VTR_SEVERITY_INFO, "step {}", nullptr, 0, nullptr, 1, &u64, &step);
    pthread_t crash_thread;
    if (mode == "thread") pthread_create(&crash_thread, nullptr, crasher, nullptr);

    const uint64_t per_step = 97;
    uint64_t records = 0, logs = 0, t = 0;
    for (;;) {
        if (records >= target) {
            if (mode != "thread") break;
            if (!g_crash_thread.load()) {
                fprintf(stderr, "emitted changes=%llu logs=%llu time=%llu\n", static_cast<unsigned long long>(records),
                        static_cast<unsigned long long>(logs), static_cast<unsigned long long>(t));
                g_crash_thread.store(true);  // the owner keeps writing while the other thread dies
            }
        }
        t += 1;
        vtr_writer_set_time(w, t);
        for (uint64_t k = 0; k < per_step; ++k) vtr_writer_emit_u64(w, sig[next() % signals], next() & 0xffff);
        records += per_step;
        if (t % 64 == 0) {
            vtr_value v{};
            v.tag = VTR_VAL_U64;
            v.u = t;
            vtr_writer_log(w, tick, t, 0, 1, &v, nullptr);
            logs += 1;
        }
    }
    fprintf(stderr, "emitted changes=%llu logs=%llu time=%llu\n", static_cast<unsigned long long>(records), static_cast<unsigned long long>(logs),
            static_cast<unsigned long long>(t));
    if (mode == "none") return vtr_writer_close(w) == VTR_OK ? 0 : 1;
    die(mode);
}
