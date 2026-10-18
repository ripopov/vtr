#include "readers.hpp"
#include <chrono>
#include <iomanip>
#include <iostream>
#include <sys/resource.h>

namespace {
using Clock = std::chrono::steady_clock;
double seconds(Clock::time_point start) { return std::chrono::duration<double>(Clock::now() - start).count(); }
bench::u64 peak_rss_kib() {
    // VmHWM belongs to this executable's address space; ru_maxrss can retain the parent's fork-time RSS.
    std::ifstream status("/proc/self/status");
    std::string line;
    while (std::getline(status, line)) {
        if (line.rfind("VmHWM:", 0) == 0) return std::stoull(line.substr(6));
    }
    throw std::runtime_error("cannot read process peak RSS from /proc/self/status");
}
void digest(const bench::Digest& d) {
    std::cout << "{\"count\":" << d.count << ",\"sum\":\"" << d.sum << "\",\"xor\":\"" << d.xors
              << "\",\"logical_bytes\":" << d.logical_bytes << '}';
}
}

int main(int argc, char** argv) {
    try {
        if (argc != 6) throw std::runtime_error("usage: benchmark OP FORMAT PROFILE PATH SCALE_DIVISOR");
        const std::string op = argv[1], format = argv[2], path = argv[4];
        const auto scale = std::stoul(argv[5]);
        if (scale == 0 || scale > 10000) throw std::runtime_error("scale divisor must be 1..10000");
        const auto p = bench::profile(argv[3], unsigned(scale));
        std::cout << std::setprecision(12);
        if (op == "oracle") {
            bench::Oracle oracle(p);
            bench::generate(p, oracle);
            std::cout << "{\"all\":";
            digest(oracle.all);
            for (const std::string kind : {"search-signal", "point", "search-transaction", "find-signal", "find-transaction"}) {
                std::cout << ",\"" << kind << "\":[";
                for (unsigned i = 0; i < bench::query_count; ++i) {
                    if (i) std::cout << ',';
                    if (kind == "point") digest(oracle.points[i]);
                    else if (kind == "search-signal") digest(oracle.waves[i]);
                    else if (kind == "search-transaction") digest(oracle.transactions[i]);
                    else if (kind == "find-signal") digest(oracle.all_waves[i]);
                    else digest(oracle.all_transactions[i]);
                }
                std::cout << ']';
            }
            std::cout << "}\n";
            return 0;
        }
        const auto start = Clock::now();
        double capture = 0, drain = 0;
        bench::u64 baseline_count = 0;
        std::vector<bench::Digest> digests;
        std::vector<double> latencies;
        if (op == "write") {
            auto sink = bench::writer(p, format, path);
            const auto capture_start = Clock::now();
            bench::generate(p, *sink);
            capture = seconds(capture_start);
            if (auto null = dynamic_cast<bench::NullWriter*>(sink.get())) baseline_count = null->count;
            const auto drain_start = Clock::now();
            sink->close();
            sink.reset();
            drain = seconds(drain_start);
        } else if (op == "read") digests.push_back(bench::read(p, format, path, op, 0));
        else if (op == "search-signal" || op == "point" || op == "search-transaction" || op == "find-signal" || op == "find-transaction") {
            const bool tx = op == "search-transaction" || op == "find-transaction";
            if ((tx && !p.generators) || (!tx && !p.signals))
                throw std::runtime_error("query is not applicable to profile");
            for (unsigned i = 0; i < bench::query_count; ++i) {
                const auto query_start = Clock::now();
                digests.push_back(bench::read(p, format, path, op, i));
                latencies.push_back(seconds(query_start));
            }
        } else throw std::runtime_error("unknown operation: " + op);
        const auto elapsed = seconds(start);
        struct rusage usage{};
        if (getrusage(RUSAGE_SELF, &usage)) throw std::runtime_error("getrusage failed");
        const double cpu = usage.ru_utime.tv_sec + usage.ru_utime.tv_usec * 1e-6
            + usage.ru_stime.tv_sec + usage.ru_stime.tv_usec * 1e-6;
        std::cout << "{\"seconds\":" << elapsed << ",\"capture_seconds\":" << capture << ",\"close_seconds\":" << drain
                  << ",\"cpu_seconds\":" << cpu << ",\"peak_rss_kib\":" << peak_rss_kib()
                  << ",\"ticks\":" << p.ticks << ",\"signals\":" << p.signals << ",\"generators\":" << p.generators
                  << ",\"baseline_count\":" << baseline_count << ",\"digests\":[";
        for (unsigned i = 0; i < digests.size(); ++i) { if (i) std::cout << ','; digest(digests[i]); }
        std::cout << "],\"latencies_seconds\":[";
        for (unsigned i = 0; i < latencies.size(); ++i) { if (i) std::cout << ','; std::cout << latencies[i]; }
        std::cout << "]}\n";
    } catch (const std::exception& e) {
        std::cerr << "benchmark: " << e.what() << '\n';
        return 1;
    }
}
