// Timing, CPU accounting and JSON output shared by the log benchmark harnesses.
#pragma once

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <sys/resource.h>
#include <sys/stat.h>

namespace benchutil {

inline double now() {
    using namespace std::chrono;
    return duration_cast<duration<double>>(steady_clock::now().time_since_epoch()).count();
}

// User + system CPU seconds of the whole process (all threads).
inline double cpu_seconds() {
    struct rusage ru;
    getrusage(RUSAGE_SELF, &ru);
    return ru.ru_utime.tv_sec + ru.ru_utime.tv_usec * 1e-6 + ru.ru_stime.tv_sec + ru.ru_stime.tv_usec * 1e-6;
}

inline uint64_t file_size(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) return 0;
    return static_cast<uint64_t>(st.st_size);
}

// Sums the sizes of all regular files below `dir` (for loggers that write directories).
inline uint64_t tree_size(const std::string &path);

struct Result {
    const char *name;
    uint64_t n = 0;
    double loop_s = 0;   // time of the message loop (hot path incl. queueing)
    double total_s = 0;  // loop + flush/close: everything on disk
    double cpu_s = 0;    // process CPU time over total_s (all threads)
    uint64_t bytes = 0;  // output size
    // Optional extra fields (printed when non-negative).
    double format_s = -1;
    double decode_s = -1;
    uint64_t lines = 0;
};

inline void print_json(const Result &r, FILE *out = stdout) {
    std::fprintf(out, "{\"logger\":\"%s\",\"n\":%llu,\"loop_s\":%.6f,\"total_s\":%.6f,\"cpu_s\":%.6f,\"bytes\":%llu", r.name, (unsigned long long)r.n, r.loop_s, r.total_s, r.cpu_s,
                (unsigned long long)r.bytes);
    if (r.format_s >= 0) std::fprintf(out, ",\"format_s\":%.6f", r.format_s);
    if (r.decode_s >= 0) std::fprintf(out, ",\"decode_s\":%.6f,\"lines\":%llu", r.decode_s, (unsigned long long)r.lines);
    std::fprintf(out, "}\n");
}

inline uint64_t parse_count(const char *s) {
    char *end = nullptr;
    unsigned long long v = std::strtoull(s, &end, 10);
    if (end && (*end == 'm' || *end == 'M')) v *= 1000000ull;
    if (end && (*end == 'k' || *end == 'K')) v *= 1000ull;
    return v;
}

} // namespace benchutil
