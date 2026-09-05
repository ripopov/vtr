#!/usr/bin/env python3
"""Copies NanoLog's C++17 runtime into a build directory and patches it so it
compiles on macOS / arm64 (upstream supports Linux/x86-64 only).

    nanolog_port.py <ext/NanoLog> <out-dir>

Patches (all guarded so the sources still build unchanged on Linux/x86):
  * Cycles.h, Util.h: rdtsc/rdpmc/cpuid inline assembly -> cntvct_el0 on
    aarch64 (a constant-rate counter like the TSC), no-ops for the perf helpers
  * Util.h: gettid() via pthread_threadid_np, thread pinning stubs (no
    sched_setaffinity on macOS)
  * Config.h: O_NOATIME does not exist on macOS; O_DSYNC dropped so NanoLog
    writes through the page cache like every other logger in the comparison
  * RuntimeLogger.cc: sched_getcpu() -> 0, O_DIRECT -> 0
  * Log.cc: drop the glibc-private <bits/algorithmfwd.h>
  * Packer.h: on macOS int64_t is `long long`, so the extra `long long`
    overload becomes the `long` overload
Also runs NanoLog's preprocessor once to produce GeneratedCode.cc (needed by
the library and the decompressor for the legacy dictionary path).
"""
import os
import shutil
import subprocess
import sys


def patch(path, pairs):
    s = open(path).read()
    for old, new in pairs:
        if s.count(old) != 1:
            raise SystemExit(f"{path}: anchor not found exactly once: {old[:60]!r}")
        s = s.replace(old, new)
    open(path, "w").write(s)


def main():
    src, out = sys.argv[1], sys.argv[2]
    rt = os.path.join(out, "runtime")
    if os.path.exists(rt):
        shutil.rmtree(rt)
    shutil.copytree(os.path.join(src, "runtime"), rt)
    shutil.copytree(os.path.join(src, "preprocessor"), os.path.join(out, "preprocessor"), dirs_exist_ok=True)

    patch(os.path.join(rt, "Cycles.h"), [
        ('        size_t lo, hi;\n        __asm__ __volatile__("rdtsc" : "=a" (lo), "=d" (hi));\n//        __asm__ __volatile__("rdtscp" : "=a" (lo), "=d" (hi) : : "%rcx");\n        return (((uint64_t)hi << 32) | lo);',
         '#if defined(__aarch64__)\n        uint64_t v;\n        __asm__ __volatile__("mrs %0, cntvct_el0" : "=r"(v));\n        return v;\n#else\n        size_t lo, hi;\n        __asm__ __volatile__("rdtsc" : "=a" (lo), "=d" (hi));\n        return (((uint64_t)hi << 32) | lo);\n#endif'),
    ])
    patch(os.path.join(rt, "Util.h"), [
        ('    unsigned int a, d;\n    __asm __volatile("rdpmc" : "=a"(a), "=d"(d) : "c"(ecx));\n    return ((uint64_t)a) | (((uint64_t)d) << 32);',
         '#if defined(__x86_64__)\n    unsigned int a, d;\n    __asm __volatile("rdpmc" : "=a"(a), "=d"(d) : "c"(ecx));\n    return ((uint64_t)a) | (((uint64_t)d) << 32);\n#else\n    (void)ecx;\n    return 0;\n#endif'),
        ('    return static_cast<pid_t>(syscall( __NR_gettid ));',
         '#if defined(__APPLE__)\n    uint64_t tid = 0;\n    pthread_threadid_np(NULL, &tid);\n    return static_cast<pid_t>(tid);\n#else\n    return static_cast<pid_t>(syscall( __NR_gettid ));\n#endif'),
        ('static FORCE_INLINE\nvoid pinThreadToCore(int id) {\n    cpu_set_t cpuset;\n\n    CPU_ZERO(&cpuset);\n        CPU_SET(id, &cpuset);\n        assert(sched_setaffinity(0, sizeof(cpuset), &cpuset) == 0);\n}',
         '#if defined(__APPLE__)\ntypedef struct { int dummy; } cpu_set_t;\nstatic FORCE_INLINE void pinThreadToCore(int) {}\nstatic FORCE_INLINE cpu_set_t getCpuAffinity() { return cpu_set_t{0}; }\nstatic FORCE_INLINE void setCpuAffinity(cpu_set_t) {}\n#else\nstatic FORCE_INLINE\nvoid pinThreadToCore(int id) {\n    cpu_set_t cpuset;\n\n    CPU_ZERO(&cpuset);\n        CPU_SET(id, &cpuset);\n        assert(sched_setaffinity(0, sizeof(cpuset), &cpuset) == 0);\n}'),
        ('static FORCE_INLINE\nvoid setCpuAffinity(cpu_set_t cpuset) {\n    assert(sched_setaffinity(0, sizeof(cpuset), &cpuset) == 0);\n}',
         'static FORCE_INLINE\nvoid setCpuAffinity(cpu_set_t cpuset) {\n    assert(sched_setaffinity(0, sizeof(cpuset), &cpuset) == 0);\n}\n#endif /* __APPLE__ */'),
        ('    uint32_t eax, ebx, ecx, edx;\n    __asm volatile("cpuid"\n        : "=a" (eax), "=b" (ebx), "=c" (ecx), "=d" (edx)\n        : "a" (1U));',
         '#if defined(__x86_64__)\n    uint32_t eax, ebx, ecx, edx;\n    __asm volatile("cpuid"\n        : "=a" (eax), "=b" (ebx), "=c" (ecx), "=d" (edx)\n        : "a" (1U));\n#else\n    __asm__ __volatile__("isb" ::: "memory");\n#endif'),
    ])
    patch(os.path.join(rt, "Config.h"), [
        # NanoLog opens its default file at start-up, before setLogFile: keep it out of the working directory.
        ('    static const char DEFAULT_LOG_FILE[] = "./compressedLog";', '    static const char DEFAULT_LOG_FILE[] = "/tmp/nanolog_default_compressedLog";'),
        ('    static const int FILE_PARAMS = O_APPEND|O_RDWR|O_CREAT|O_NOATIME|O_DSYNC;',
         '#if defined(__APPLE__)\n    // No O_NOATIME on macOS; O_DSYNC dropped so the file goes through the page cache\n    // like the other loggers of the comparison (none of them fsyncs).\n    static const int FILE_PARAMS = O_APPEND|O_RDWR|O_CREAT;\n#else\n    static const int FILE_PARAMS = O_APPEND|O_RDWR|O_CREAT|O_NOATIME|O_DSYNC;\n#endif'),
    ])
    patch(os.path.join(rt, "RuntimeLogger.cc"), [
        ('        coreId = sched_getcpu();', '#if defined(__APPLE__)\n        coreId = 0;\n#else\n        coreId = sched_getcpu();\n#endif'),
        ('#include <fcntl.h>', '#include <fcntl.h>\n#ifndef O_DIRECT\n#define O_DIRECT 0\n#endif\n#if defined(__APPLE__)\n#define fdatasync fsync\n#endif'),
    ])
    patch(os.path.join(rt, "Fence.h"), [
        ('        __asm__ __volatile__("lfence" ::: "memory");',
         '#if defined(__aarch64__)\n        __asm__ __volatile__("dmb ishld" ::: "memory");\n#else\n        __asm__ __volatile__("lfence" ::: "memory");\n#endif'),
        ('        __asm__ __volatile__("sfence" ::: "memory");',
         '#if defined(__aarch64__)\n        __asm__ __volatile__("dmb ishst" ::: "memory");\n#else\n        __asm__ __volatile__("sfence" ::: "memory");\n#endif'),
    ])
    patch(os.path.join(rt, "Log.cc"), [
        ('#include <bits/algorithmfwd.h>\n', ''),
    ])
    patch(os.path.join(rt, "Packer.h"), [
        ('inline int\npack(char **buffer, long long int val)', '#if defined(__APPLE__)\ninline int\npack(char **buffer, long int val)\n#else\ninline int\npack(char **buffer, long long int val)\n#endif'),
    ])
    # GeneratedCode.cc from the sample client (needed by the library and decompressor).
    cxx = os.environ.get("CXX", "clang++")
    client = os.path.join(rt, "testHelper", "client.cc")
    pre = os.path.join(rt, "testHelper", "client.cc.i")
    subprocess.run([cxx, "-std=c++17", "-E", "-I", rt, client, "-o", pre], check=True)
    parser = os.path.join(out, "preprocessor", "parser.py")
    mapf = os.path.join(rt, "testHelper", "client.map")
    subprocess.run([sys.executable, parser, f"--mapOutput={mapf}", pre], check=True, cwd=rt)
    gen = os.path.join(rt, "GeneratedCode.cc")
    subprocess.run([sys.executable, parser, f"--combinedOutput={gen}", mapf], check=True, cwd=rt)
    # `double_t` also names a math.h type on macOS: qualify the enumerator.
    g = open(gen).read().replace("pf->argType = double_t;", "pf->argType = NanoLogInternal::Log::FormatType::double_t;")
    open(gen, "w").write(g)
    print(f"NanoLog runtime ported to {rt}")


if __name__ == "__main__":
    main()
