// The synthetic simulator log shared by every logger in the log benchmark.
//
// A deterministic generator produces N timestamped messages drawn from 13
// call sites that model what a SoC simulation prints: bus transactions with
// hex addresses and data, per-core fetch/retire lines, cache events,
// back-pressure warnings, DMA completions with a floating-point duration,
// parity errors, UART output, scheduler statistics, interrupts, checkpoints
// and FSM transitions. Every logger receives exactly the same sequence of
// (site, simulation time, arguments); string arguments come from small pools
// (component paths, responses, states) the way names do in a real design.
//
// SIMLOG_KINDS(X) lists the sites with a printf format (text baseline,
// NanoLog), a std::format / VTR format (Quill, VTR) and a plain `{}` format
// (binlog, whose placeholders take no specification). Severities: 50% DEBUG,
// 46% INFO, 3% WARN, 1% ERROR.
#pragma once

#include <cinttypes>
#include <cstdint>
#include <cstring>

namespace simlog {

struct Rng {
    uint64_t s;
    explicit Rng(uint64_t seed) : s(seed ? seed : 0x9E3779B97F4A7C15ull) {}
    inline uint64_t next() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        return s;
    }
    inline uint64_t below(uint64_t n) { return next() % n; }
};

inline const char *const COMPONENTS[24] = {
    "top.soc.cpu0.lsu", "top.soc.cpu0.ifu", "top.soc.cpu0.alu", "top.soc.cpu0.mmu",
    "top.soc.cpu1.lsu", "top.soc.cpu1.ifu", "top.soc.cpu1.alu", "top.soc.cpu1.mmu",
    "top.soc.cpu2.lsu", "top.soc.cpu2.ifu", "top.soc.cpu3.lsu", "top.soc.cpu3.ifu",
    "top.soc.noc.router0", "top.soc.noc.router1", "top.soc.noc.router2", "top.soc.noc.router3",
    "top.soc.dma0", "top.soc.dma1", "top.soc.uart0", "top.soc.ddr_ctrl",
    "top.soc.l2", "top.soc.gpio", "top.soc.eth0", "top.soc.spi0",
};
inline const char *const RESP[4] = {"OKAY", "EXOKAY", "SLVERR", "DECERR"};
inline const char *const HITMISS[2] = {"hit", "miss"};
inline const char *const CHANNELS[5] = {"AW", "W", "B", "AR", "R"};
inline const char *const IRQ_SRC[6] = {"uart0", "timer0", "dma0", "dma1", "gpio", "eth0"};
inline const char *const STATES[8] = {"IDLE", "REQ", "WAIT", "BURST", "RESP", "RETRY", "DRAIN", "DONE"};
inline const char *const UART_LINES[32] = {
    "boot: stage0", "boot: stage1", "boot: ddr init ok", "boot: loading kernel", "hello world", "OK", "ERROR", "AT+RST",
    "AT+GMR", "ready", "login:", "root", "Password:", "# ", "$ ls", "$ cat /proc/cpuinfo", "$ reboot", "sync",
    "irq storm", "watchdog kick", "tick", "tock", "tx done", "rx overflow", "cts", "rts", "break", "frame err",
    "parity err", "timeout", "retry", "bye",
};

enum Sev : uint8_t { DEBUG = 1, INFO = 2, WARN = 3, ERROR = 4 };

struct Msg {
    uint32_t kind;
    uint64_t t;
    const char *s0;
    const char *s1;
    uint64_t u0, u1, u2, u3;
    double f0;
};

// X(id, SEV, printf_fmt, fmt_fmt, plain_fmt, args...)
#define SIMLOG_KINDS(X)                                                                                                                                                                       \
    X(0, INFO, "[%s] AXI write addr=0x%08" PRIx64 " data=0x%016" PRIx64 " len=%" PRIu64, "[{}] AXI write addr={:#010x} data={:#018x} len={}", "[{}] AXI write addr={} data={} len={}", m.s0, m.u0, m.u1, m.u2)   \
    X(1, INFO, "[%s] AXI read  addr=0x%08" PRIx64 " resp=%s", "[{}] AXI read  addr={:#010x} resp={}", "[{}] AXI read  addr={} resp={}", m.s0, m.u0, m.s1)                                   \
    X(2, DEBUG, "core%" PRIu64 ": fetch pc=0x%08" PRIx64 " inst=0x%08" PRIx64, "core{}: fetch pc={:#010x} inst={:#010x}", "core{}: fetch pc={} inst={}", m.u0, m.u1, m.u2)                  \
    X(3, DEBUG, "core%" PRIu64 ": retire pc=0x%08" PRIx64 " rd=x%" PRIu64 " val=0x%" PRIx64, "core{}: retire pc={:#010x} rd=x{} val={:#x}", "core{}: retire pc={} rd=x{} val={}", m.u0, m.u1, m.u2, m.u3) \
    X(4, INFO, "%s: cache %s line 0x%" PRIx64 " way %" PRIu64, "{}: cache {} line {:#x} way {}", "{}: cache {} line {} way {}", m.s0, m.s1, m.u0, m.u1)                                      \
    X(5, WARN, "%s: backpressure %" PRIu64 " cycles on channel %s", "{}: backpressure {} cycles on channel {}", "{}: backpressure {} cycles on channel {}", m.s0, m.u0, m.s1)                 \
    X(6, INFO, "DMA channel %" PRIu64 " moved %" PRIu64 " bytes 0x%" PRIx64 " -> 0x%" PRIx64 " in %.2f us", "DMA channel {} moved {} bytes {:#x} -> {:#x} in {:.2f} us", "DMA channel {} moved {} bytes {} -> {} in {} us", m.u0, m.u1, m.u2, m.u3, m.f0) \
    X(7, ERROR, "%s: parity error at 0x%" PRIx64 " expected 0x%" PRIx64 " got 0x%" PRIx64, "{}: parity error at {:#x} expected {:#x} got {:#x}", "{}: parity error at {} expected {} got {}", m.s0, m.u0, m.u1, m.u2) \
    X(8, INFO, "uart0 tx '%s'", "uart0 tx '{}'", "uart0 tx '{}'", m.s0)                                                                                                                       \
    X(9, DEBUG, "scheduler: %" PRIu64 " ready, %" PRIu64 " blocked, load %.3f", "scheduler: {} ready, {} blocked, load {:.3f}", "scheduler: {} ready, {} blocked, load {}", m.u0, m.u1, m.f0) \
    X(10, INFO, "irq %" PRIu64 " raised by %s", "irq {} raised by {}", "irq {} raised by {}", m.u0, m.s0)                                                                                     \
    X(11, INFO, "checkpoint %" PRIu64 " reached after %" PRIu64 " cycles", "checkpoint {} reached after {} cycles", "checkpoint {} reached after {} cycles", m.u0, m.u1)                      \
    X(12, DEBUG, "%s: fsm %s -> %s", "{}: fsm {} -> {}", "{}: fsm {} -> {}", m.s0, m.s1, STATES[m.u0])

inline constexpr int N_KINDS = 13;
// Weights per kind (sum 1000).
inline constexpr uint16_t WEIGHTS[N_KINDS] = {120, 120, 200, 200, 100, 30, 40, 8, 40, 60, 30, 12, 40};

inline const char *sev_name(uint8_t s) {
    switch (s) {
    case DEBUG: return "DEBUG";
    case INFO: return "INFO";
    case WARN: return "WARN";
    default: return "ERROR";
    }
}

class Generator {
public:
    explicit Generator(uint64_t seed = 42) : rng_(seed) {
        uint32_t acc = 0;
        for (int i = 0; i < N_KINDS; ++i) {
            acc += WEIGHTS[i];
            cum_[i] = acc;
        }
    }

    inline void next(Msg &m) {
        uint32_t r = static_cast<uint32_t>(rng_.below(1000));
        uint32_t k = 0;
        while (r >= cum_[k]) ++k;
        m.kind = k;
        // Simulation time: mostly small steps, an occasional idle gap.
        t_ += 1 + rng_.below(50);
        if ((++seq_ & 1023) == 0) t_ += 10000;
        m.t = t_;
        m.s0 = COMPONENTS[rng_.below(24)];
        switch (k) {
        case 0:
            m.u0 = 0x80000000ull + rng_.below(0x100000) * 4;
            m.u1 = rng_.next();
            m.u2 = 1 + rng_.below(16);
            break;
        case 1:
            m.u0 = 0x80000000ull + rng_.below(0x100000) * 4;
            m.s1 = RESP[rng_.below(100) < 97 ? rng_.below(2) : 2 + rng_.below(2)];
            break;
        case 2:
            m.u0 = rng_.below(4);
            m.u1 = 0x80000000ull + ((pc_ += 4) & 0x3ffff);
            m.u2 = rng_.next() & 0xffffffffull;
            break;
        case 3:
            m.u0 = rng_.below(4);
            m.u1 = 0x80000000ull + (pc_ & 0x3ffff);
            m.u2 = rng_.below(32);
            m.u3 = rng_.next();
            break;
        case 4:
            m.s1 = HITMISS[rng_.below(100) < 80 ? 0 : 1];
            m.u0 = rng_.below(0x8000) << 6;
            m.u1 = rng_.below(8);
            break;
        case 5:
            m.u0 = 1 + rng_.below(100);
            m.s1 = CHANNELS[rng_.below(5)];
            break;
        case 6:
            m.u0 = rng_.below(8);
            m.u1 = 64ull << rng_.below(11);
            m.u2 = 0x10000000ull + rng_.below(0x10000) * 64;
            m.u3 = 0x20000000ull + rng_.below(0x10000) * 64;
            m.f0 = static_cast<double>(m.u1) / static_cast<double>(100 + rng_.below(900));
            break;
        case 7:
            m.u0 = 0x80000000ull + rng_.below(0x100000) * 4;
            m.u1 = rng_.next();
            m.u2 = m.u1 ^ (1ull << rng_.below(64));
            break;
        case 8:
            m.s0 = UART_LINES[rng_.below(32)];
            break;
        case 9:
            m.u0 = rng_.below(64);
            m.u1 = rng_.below(16);
            m.f0 = static_cast<double>(rng_.below(1000)) / 1000.0;
            break;
        case 10:
            m.u0 = rng_.below(32);
            m.s0 = IRQ_SRC[rng_.below(6)];
            break;
        case 11:
            m.u0 = ++checkpoint_;
            m.u1 = t_ / 10;
            break;
        default:
            m.s1 = STATES[rng_.below(8)];
            m.u0 = rng_.below(8);
            break;
        }
    }

private:
    Rng rng_;
    uint32_t cum_[N_KINDS] = {};
    uint64_t t_ = 0;
    uint64_t seq_ = 0;
    uint64_t pc_ = 0;
    uint64_t checkpoint_ = 0;
};

// Severity of each kind, indexable by kind id.
inline constexpr uint8_t KIND_SEV[N_KINDS] = {INFO, INFO, DEBUG, DEBUG, INFO, WARN, INFO, ERROR, INFO, DEBUG, INFO, INFO, DEBUG};

} // namespace simlog
