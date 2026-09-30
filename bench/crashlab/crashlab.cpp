// Crash lab: a prototype of the crash guard designed in docs/crash-safe-vtr.html.
//
// A "simulator thread" (main) writes value changes and log records into a VTR
// writer, then dies in one of several ways. The guard installed here is the
// design reduced to its core:
//
//  * The fatal-signal handler runs on an alternate stack and does only
//    async-signal-safe work: atomics, write(2) to an eventfd, clock_gettime,
//    poll(2) as a sleep, sigaction and tgkill.
//  * A rescue thread, created at install time with every signal blocked, does
//    the real work: it records the crash as a FATAL log record and closes the
//    writer through the ordinary close path.
//  * The handler waits for the rescue with a hard deadline, restores the
//    previous disposition and re-raises, so the exit status and the core dump
//    are those of the original crash.
//  * Stop requests (SIGTERM, SIGINT, SIGHUP, SIGXCPU) only set a flag; the
//    simulation loop ends and closes normally. A second request is not graceful.
//  * exit() runs an atexit hook that closes the writer on the calling thread.
//
// With --vtr-guard the library's guard (vtr_guard_install, vtr.h) replaces
// the prototype: the measurement of what landed.
//
// usage: crashlab <mode> <out.vtr> [--no-guard] [--vtr-guard] [--records N] [--signals S]
//                 [--timeout-ms T] [--stall-ms S] [--no-altstack] [--busy-step|--busy-call]
//   mode: none segv abort stack throw heap heaplock term exit kill
#include "vtr.h"

#include <atomic>
#include <cerrno>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <poll.h>
#include <pthread.h>
#include <stdexcept>
#include <string>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

namespace {

// ---------------------------------------------------------------- the guard
constexpr int kFatal[] = {SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT, SIGTRAP, SIGSYS};
constexpr int kStop[] = {SIGTERM, SIGINT, SIGHUP, SIGXCPU};
struct sigaction g_old[65];
std::atomic<int> g_state{0};      // 0 idle, 1 rescuing, 2 rescue finished
std::atomic<int> g_signo{0};
std::atomic<pid_t> g_tid{0};
std::atomic<bool> g_stop{false};  // a stop was requested (SIGTERM & co.)
std::atomic<int> g_stop_signo{0};
int g_efd = -1;
long g_timeout_ms = 10000;
long g_stall_ms = 0;              // test hook: a slow or stuck rescue
std::atomic<vtr_writer *> g_writer{nullptr};
uint32_t g_crash_site = 0;
std::atomic<uint64_t> g_time{0};  // simulation time, published once per step
std::atomic<uint32_t> g_busy{0};  // 1 while the owner is inside the writer
int g_busy_mode = 0;              // 0 off, 1 per time step (a Verilator dump), 2 per writer call

uint64_t now_ns() {
    timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);  // async-signal-safe
    return uint64_t(ts.tv_sec) * 1000000000u + uint64_t(ts.tv_nsec);
}

void install_altstack() {
    // Per thread: 64 KiB with a PROT_NONE guard page below it.
    const size_t page = size_t(sysconf(_SC_PAGESIZE)), size = 64 * 1024;
    char *p = static_cast<char *>(mmap(nullptr, size + page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    mprotect(p, page, PROT_NONE);
    stack_t ss{};
    ss.ss_sp = p + page;
    ss.ss_size = size;
    sigaltstack(&ss, nullptr);
}

void say(const char *s) { (void)!write(2, s, strlen(s)); }

void on_fatal(int sig, siginfo_t *info, void *) {
    const int saved_errno = errno;
    int expected = 0;
    if (!g_state.compare_exchange_strong(expected, 1)) {
        // Another thread is rescuing: wait for it to end the process.
        for (;;) pause();
    }
    g_signo.store(sig);
    g_tid.store(pid_t(syscall(SYS_gettid)));
    const uint64_t one = 1;
    (void)!write(g_efd, &one, sizeof one);  // wake the rescue thread
    const uint64_t deadline = now_ns() + uint64_t(g_timeout_ms) * 1000000u;
    while (g_state.load() != 2 && now_ns() < deadline) poll(nullptr, 0, 1);  // poll(2) is async-signal-safe
    if (g_state.load() != 2) say("crashlab: rescue timed out\n");
    // Restore the previous disposition and let the same signal end the process.
    sigaction(sig, &g_old[sig], nullptr);
    errno = saved_errno;
    if (info->si_code > 0 && sig != SIGABRT) return;  // a hardware fault re-executes and faults again
    syscall(SYS_tgkill, getpid(), syscall(SYS_gettid), sig);  // pending until the handler returns
}

void on_stop(int sig, siginfo_t *, void *) {
    if (g_stop.exchange(true)) {
        sigaction(sig, &g_old[sig], nullptr);  // second request: not graceful any more
        syscall(SYS_tgkill, getpid(), syscall(SYS_gettid), sig);
        return;
    }
    g_stop_signo.store(sig);
}

void log_fatal(vtr_writer *w, const char *text) {
    vtr_value arg{};
    arg.tag = VTR_VAL_TEXT;
    arg.data = reinterpret_cast<const uint8_t *>(text);
    arg.len = strlen(text);
    vtr_writer_log(w, g_crash_site, g_time.load(), 0, 1, &arg, nullptr);
}

void *rescue_main(void *) {
    uint64_t v;
    while (read(g_efd, &v, sizeof v) != sizeof v) {}
    const uint64_t t0 = now_ns();
    if (g_stall_ms) poll(nullptr, 0, int(g_stall_ms));
    vtr_writer *w = g_writer.exchange(nullptr);
    if (w && g_busy.load() == 0) {
        char text[128];
        snprintf(text, sizeof text, "simulation crashed: signal %d (%s) in thread %d", g_signo.load(), strsignal(g_signo.load()), int(g_tid.load()));
        log_fatal(w, text);
        vtr_writer_close(w);
    } else if (w) {
        say("crashlab: the owner was inside the writer; not closing it\n");
    }
    char line[64];
    snprintf(line, sizeof line, "crashlab: rescue_ms=%.1f\n", double(now_ns() - t0) / 1e6);
    say(line);
    g_state.store(2);
    return nullptr;
}

void at_exit() {
    // exit() from anywhere in the simulation: close on the calling thread, in normal context.
    if (vtr_writer *w = g_writer.exchange(nullptr)) {
        log_fatal(w, "simulation called exit()");
        vtr_writer_close(w);
    }
}

void guard_install(bool altstack) {
    g_efd = eventfd(0, EFD_CLOEXEC);
    // The rescue thread inherits a full mask, so it never runs a handler.
    sigset_t all, prev;
    sigfillset(&all);
    pthread_sigmask(SIG_SETMASK, &all, &prev);
    pthread_t t;
    pthread_create(&t, nullptr, rescue_main, nullptr);
    pthread_detach(t);
    pthread_sigmask(SIG_SETMASK, &prev, nullptr);
    if (altstack) install_altstack();
    struct sigaction sa{};
    sigfillset(&sa.sa_mask);  // nothing else interrupts the handler
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sa.sa_sigaction = on_fatal;
    for (int s : kFatal) sigaction(s, &sa, &g_old[s]);
    sa.sa_sigaction = on_stop;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK | SA_RESTART;
    for (int s : kStop) sigaction(s, &sa, &g_old[s]);
    atexit(at_exit);
}

// ---------------------------------------------------------------- the workload
uint64_t rng = 0x9E3779B97F4A7C15ull;
inline uint64_t next() {
    rng ^= rng << 13;
    rng ^= rng >> 7;
    rng ^= rng << 17;
    return rng;
}

int recurse(int n) {
    volatile char pad[4096];
    pad[0] = char(n);
    return recurse(n + 1) + pad[0];
}

[[noreturn]] void die(const std::string &mode) {
    // Function pointers through volatile keep the compiler from eliding the heap misuse.
    void *(*volatile m)(size_t) = malloc;
    void (*volatile f)(void *) = free;
    if (mode == "segv") *static_cast<volatile int *>(nullptr) = 1;
    if (mode == "abort") abort();
    if (mode == "stack") recurse(0);
    if (mode == "throw") throw std::runtime_error("uncaught");  // std::terminate -> abort
    if (mode == "heap") {
        // Double free of a small block: glibc's tcache check aborts without holding a lock.
        void *p = m(64);
        f(p);
        f(p);
    }
    if (mode == "heaplock") {
        // Double free of a 64 KiB block: glibc detects it in _int_free while holding the
        // main arena's lock and aborts with the lock still held.
        void *p = m(64 * 1024 - 64);
        void *guard = m(64);
        (void)guard;
        f(p);
        f(p);
    }
    if (mode == "exit") exit(3);
    if (mode == "kill") kill(getpid(), SIGKILL);
    for (;;) pause();
}

}  // namespace

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: crashlab <mode> <out.vtr> [options]\n");
        return 2;
    }
    const std::string mode = argv[1];
    const char *path = argv[2];
    bool guard = true, altstack = true, lib_guard = false;
    uint64_t target = 30000000, signals = 67144;
    for (int i = 3; i < argc; ++i) {
        const std::string a = argv[i];
        if (a == "--no-guard") guard = false;
        else if (a == "--vtr-guard") lib_guard = true;
        else if (a == "--no-altstack") altstack = false;
        else if (a == "--records") target = strtoull(argv[++i], nullptr, 10);
        else if (a == "--signals") signals = strtoull(argv[++i], nullptr, 10);
        else if (a == "--timeout-ms") g_timeout_ms = strtol(argv[++i], nullptr, 10);
        else if (a == "--stall-ms") g_stall_ms = strtol(argv[++i], nullptr, 10);
        else if (a == "--busy-step") g_busy_mode = 1;
        else if (a == "--busy-call") g_busy_mode = 2;
    }
    vtr_writer_options o;
    vtr_writer_options_default(&o);
    o.dedup = 0;  // as in the Verilator fork: every emit is a change
    vtr_writer *w = vtr_writer_create(path, &o);
    const uint32_t top = vtr_writer_add_scope(w, VTR_NONE, "top", VTR_SCOPE_MODULE, nullptr);
    uint32_t *sig = new uint32_t[signals];
    for (uint64_t s = 0; s < signals; ++s) {
        char name[32];
        snprintf(name, sizeof name, "s%llu", static_cast<unsigned long long>(s));
        vtr_writer_add_var(w, top, name, VTR_VAR_WIRE, VTR_DIR_IMPLICIT, VTR_SIGNAL_BITS, 32, 2, nullptr, &sig[s]);
    }
    const uint32_t stream = vtr_writer_add_stream(w, VTR_NONE, "simulation_log", VTR_LOG_STREAM_KIND);
    const uint8_t text = VTR_VAL_TEXT, u64 = VTR_VAL_U64;
    const char *msg = "message", *step = "step";
    g_crash_site = vtr_writer_add_log_site(w, stream, VTR_SEVERITY_FATAL, "{}", nullptr, 0, nullptr, 1, &text, &msg);
    const uint32_t tick = vtr_writer_add_log_site(w, stream, VTR_SEVERITY_INFO, "step {}", nullptr, 0, nullptr, 1, &u64, &step);
    g_writer.store(w);
    if (lib_guard) {
        guard = false;
        vtr_guard_install(nullptr, nullptr);
        vtr_guard_watch(w);
    } else if (guard) {
        guard_install(altstack);
    }

    // About 1,183 changes per time step, as in openC910 CoreMark; a log record every 64 steps.
    const uint64_t per_step = 1183;
    uint64_t records = 0, logs = 0, t = 0;
    bool sent = false;
    const uint64_t t_start = now_ns();
    while (!g_stop.load(std::memory_order_relaxed)) {
        if (records >= target) {
            if (mode != "term") break;
            if (!sent) {
                fprintf(stderr, "crashlab: emitted records=%llu logs=%llu last_time=%llu write_s=%.3f\n", static_cast<unsigned long long>(records),
                        static_cast<unsigned long long>(logs), static_cast<unsigned long long>(t), double(now_ns() - t_start) / 1e9);
                kill(getpid(), SIGTERM);  // arrives while the simulation runs
            }
            sent = true;
        }
        t += 1;
        if (g_busy_mode == 1) {
            g_busy.store(1, std::memory_order_relaxed);
            std::atomic_signal_fence(std::memory_order_seq_cst);
        }
        vtr_writer_set_time(w, t);
        for (uint64_t k = 0; k < per_step; ++k) {
            if (g_busy_mode == 2) {
                g_busy.store(1, std::memory_order_relaxed);
                std::atomic_signal_fence(std::memory_order_seq_cst);
            }
            vtr_writer_emit_u64(w, sig[next() % signals], next() & 0xffff);
            if (g_busy_mode == 2) {
                std::atomic_signal_fence(std::memory_order_seq_cst);
                g_busy.store(0, std::memory_order_relaxed);
            }
        }
        records += per_step;
        if (t % 64 == 0) {
            vtr_value v{};
            v.tag = VTR_VAL_U64;
            v.u = t;
            vtr_writer_log(w, tick, t, 0, 1, &v, nullptr);
            logs += 1;
        }
        if (g_busy_mode == 1) {
            std::atomic_signal_fence(std::memory_order_seq_cst);
            g_busy.store(0, std::memory_order_relaxed);
        }
        g_time.store(t, std::memory_order_relaxed);
    }
    fprintf(stderr, "crashlab: emitted records=%llu logs=%llu last_time=%llu write_s=%.3f\n", static_cast<unsigned long long>(records),
            static_cast<unsigned long long>(logs), static_cast<unsigned long long>(t), double(now_ns() - t_start) / 1e9);
    if (g_stop.load()) {
        // A stop request: the ordinary close path on the owning thread.
        const uint64_t t0 = now_ns();
        if (g_writer.exchange(nullptr)) {
            char line[64];
            snprintf(line, sizeof line, "simulation stopped: signal %d (%s)", g_stop_signo.load(), strsignal(g_stop_signo.load()));
            log_fatal(w, line);
            vtr_writer_close(w);
        }
        fprintf(stderr, "crashlab: stop_close_ms=%.1f\n", double(now_ns() - t0) / 1e6);
        signal(g_stop_signo.load(), SIG_DFL);
        raise(g_stop_signo.load());  // the exit status still says "terminated by SIGTERM"
    }
    if (mode == "none") {
        const uint64_t t0 = now_ns();
        if (lib_guard) vtr_guard_unwatch(w);
        if (g_writer.exchange(nullptr)) vtr_writer_close(w);
        fprintf(stderr, "crashlab: close_ms=%.1f\n", double(now_ns() - t0) / 1e6);
        return 0;
    }
    die(mode);
}
