//! Log workload: the Rust port of `bench/log/workload.hpp` (same generator,
//! same call sites, same message sequence) driving the Rust `Writer::log` API,
//! plus an encoder micro-benchmark for the log block codec settings.

use crate::util::{cpu_seconds, file_size};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use vtr::logblock::{self, LogBlockInput, LogSiteEnc};
use vtr::{Compression, LogArg, LogArgType, LogSiteId, LogSiteSpec, ScopeType, Severity, Writer, WriterOptions};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const COMPONENTS: [&str; 24] = [
    "top.soc.cpu0.lsu", "top.soc.cpu0.ifu", "top.soc.cpu0.alu", "top.soc.cpu0.mmu", "top.soc.cpu1.lsu", "top.soc.cpu1.ifu", "top.soc.cpu1.alu", "top.soc.cpu1.mmu",
    "top.soc.cpu2.lsu", "top.soc.cpu2.ifu", "top.soc.cpu3.lsu", "top.soc.cpu3.ifu", "top.soc.noc.router0", "top.soc.noc.router1", "top.soc.noc.router2",
    "top.soc.noc.router3", "top.soc.dma0", "top.soc.dma1", "top.soc.uart0", "top.soc.ddr_ctrl", "top.soc.l2", "top.soc.gpio", "top.soc.eth0", "top.soc.spi0",
];
const RESP: [&str; 4] = ["OKAY", "EXOKAY", "SLVERR", "DECERR"];
const HITMISS: [&str; 2] = ["hit", "miss"];
const CHANNELS: [&str; 5] = ["AW", "W", "B", "AR", "R"];
const IRQ_SRC: [&str; 6] = ["uart0", "timer0", "dma0", "dma1", "gpio", "eth0"];
const STATES: [&str; 8] = ["IDLE", "REQ", "WAIT", "BURST", "RESP", "RETRY", "DRAIN", "DONE"];
const UART_LINES: [&str; 32] = [
    "boot: stage0", "boot: stage1", "boot: ddr init ok", "boot: loading kernel", "hello world", "OK", "ERROR", "AT+RST", "AT+GMR", "ready", "login:", "root",
    "Password:", "# ", "$ ls", "$ cat /proc/cpuinfo", "$ reboot", "sync", "irq storm", "watchdog kick", "tick", "tock", "tx done", "rx overflow", "cts", "rts",
    "break", "frame err", "parity err", "timeout", "retry", "bye",
];
const WEIGHTS: [u32; 13] = [120, 120, 200, 200, 100, 30, 40, 8, 40, 60, 30, 12, 40];
const SEVS: [Severity; 13] = [
    Severity::Info, Severity::Info, Severity::Debug, Severity::Debug, Severity::Info, Severity::Warn, Severity::Info, Severity::Error, Severity::Info,
    Severity::Debug, Severity::Info, Severity::Info, Severity::Debug,
];
const FMTS: [&str; 13] = [
    "[{}] AXI write addr={:#010x} data={:#018x} len={}",
    "[{}] AXI read  addr={:#010x} resp={}",
    "core{}: fetch pc={:#010x} inst={:#010x}",
    "core{}: retire pc={:#010x} rd=x{} val={:#x}",
    "{}: cache {} line {:#x} way {}",
    "{}: backpressure {} cycles on channel {}",
    "DMA channel {} moved {} bytes {:#x} -> {:#x} in {:.2f} us",
    "{}: parity error at {:#x} expected {:#x} got {:#x}",
    "uart0 tx '{}'",
    "scheduler: {} ready, {} blocked, load {:.3f}",
    "irq {} raised by {}",
    "checkpoint {} reached after {} cycles",
    "{}: fsm {} -> {}",
];
use LogArgType::{Text as T, F64 as F, U64 as U};
const TYPES: [&[LogArgType]; 13] = [
    &[T, U, U, U], &[T, U, T], &[U, U, U], &[U, U, U, U], &[T, T, U, U], &[T, U, T], &[U, U, U, U, F], &[T, U, U, U], &[T], &[U, U, F], &[U, T], &[U, U], &[T, T, T],
];

#[derive(Default, Clone, Copy)]
pub struct Msg {
    pub kind: usize,
    pub t: u64,
    pub s0: &'static str,
    pub s1: &'static str,
    pub u0: u64,
    pub u1: u64,
    pub u2: u64,
    pub u3: u64,
    pub f0: f64,
}

pub struct Generator {
    rng: Rng,
    cum: [u32; 13],
    t: u64,
    seq: u64,
    pc: u64,
    checkpoint: u64,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        let mut cum = [0u32; 13];
        let mut acc = 0;
        for i in 0..13 {
            acc += WEIGHTS[i];
            cum[i] = acc;
        }
        Generator { rng: Rng(if seed == 0 { 0x9E3779B97F4A7C15 } else { seed }), cum, t: 0, seq: 0, pc: 0, checkpoint: 0 }
    }

    pub fn next(&mut self, m: &mut Msg) {
        let r = self.rng.below(1000) as u32;
        let mut k = 0;
        while r >= self.cum[k] {
            k += 1;
        }
        m.kind = k;
        self.t += 1 + self.rng.below(50);
        self.seq += 1;
        if self.seq & 1023 == 0 {
            self.t += 10000;
        }
        m.t = self.t;
        m.s0 = COMPONENTS[self.rng.below(24) as usize];
        match k {
            0 => {
                m.u0 = 0x8000_0000 + self.rng.below(0x100000) * 4;
                m.u1 = self.rng.next();
                m.u2 = 1 + self.rng.below(16);
            }
            1 => {
                m.u0 = 0x8000_0000 + self.rng.below(0x100000) * 4;
                m.s1 = RESP[if self.rng.below(100) < 97 { self.rng.below(2) } else { 2 + self.rng.below(2) } as usize];
            }
            2 => {
                m.u0 = self.rng.below(4);
                self.pc += 4;
                m.u1 = 0x8000_0000 + (self.pc & 0x3ffff);
                m.u2 = self.rng.next() & 0xffff_ffff;
            }
            3 => {
                m.u0 = self.rng.below(4);
                m.u1 = 0x8000_0000 + (self.pc & 0x3ffff);
                m.u2 = self.rng.below(32);
                m.u3 = self.rng.next();
            }
            4 => {
                m.s1 = HITMISS[if self.rng.below(100) < 80 { 0 } else { 1 }];
                m.u0 = self.rng.below(0x8000) << 6;
                m.u1 = self.rng.below(8);
            }
            5 => {
                m.u0 = 1 + self.rng.below(100);
                m.s1 = CHANNELS[self.rng.below(5) as usize];
            }
            6 => {
                m.u0 = self.rng.below(8);
                m.u1 = 64u64 << self.rng.below(11);
                m.u2 = 0x1000_0000 + self.rng.below(0x10000) * 64;
                m.u3 = 0x2000_0000 + self.rng.below(0x10000) * 64;
                m.f0 = m.u1 as f64 / (100 + self.rng.below(900)) as f64;
            }
            7 => {
                m.u0 = 0x8000_0000 + self.rng.below(0x100000) * 4;
                m.u1 = self.rng.next();
                m.u2 = m.u1 ^ (1u64 << self.rng.below(64));
            }
            8 => m.s0 = UART_LINES[self.rng.below(32) as usize],
            9 => {
                m.u0 = self.rng.below(64);
                m.u1 = self.rng.below(16);
                m.f0 = self.rng.below(1000) as f64 / 1000.0;
            }
            10 => {
                m.u0 = self.rng.below(32);
                m.s0 = IRQ_SRC[self.rng.below(6) as usize];
            }
            11 => {
                self.checkpoint += 1;
                m.u0 = self.checkpoint;
                m.u1 = self.t / 10;
            }
            _ => {
                m.s1 = STATES[self.rng.below(8) as usize];
                m.u0 = self.rng.below(8);
            }
        }
    }
}

/// Arguments of a message in site order.
#[inline]
fn args(m: &Msg, out: &mut [LogArg<'static>; 5]) -> usize {
    match m.kind {
        0 => {
            *out = [LogArg::Text(m.s0), LogArg::U64(m.u0), LogArg::U64(m.u1), LogArg::U64(m.u2), LogArg::Bool(false)];
            4
        }
        1 => {
            out[0] = LogArg::Text(m.s0);
            out[1] = LogArg::U64(m.u0);
            out[2] = LogArg::Text(m.s1);
            3
        }
        2 => {
            out[0] = LogArg::U64(m.u0);
            out[1] = LogArg::U64(m.u1);
            out[2] = LogArg::U64(m.u2);
            3
        }
        3 => {
            *out = [LogArg::U64(m.u0), LogArg::U64(m.u1), LogArg::U64(m.u2), LogArg::U64(m.u3), LogArg::Bool(false)];
            4
        }
        4 => {
            *out = [LogArg::Text(m.s0), LogArg::Text(m.s1), LogArg::U64(m.u0), LogArg::U64(m.u1), LogArg::Bool(false)];
            4
        }
        5 => {
            out[0] = LogArg::Text(m.s0);
            out[1] = LogArg::U64(m.u0);
            out[2] = LogArg::Text(m.s1);
            3
        }
        6 => {
            *out = [LogArg::U64(m.u0), LogArg::U64(m.u1), LogArg::U64(m.u2), LogArg::U64(m.u3), LogArg::F64(m.f0)];
            5
        }
        7 => {
            *out = [LogArg::Text(m.s0), LogArg::U64(m.u0), LogArg::U64(m.u1), LogArg::U64(m.u2), LogArg::Bool(false)];
            4
        }
        8 => {
            out[0] = LogArg::Text(m.s0);
            1
        }
        9 => {
            out[0] = LogArg::U64(m.u0);
            out[1] = LogArg::U64(m.u1);
            out[2] = LogArg::F64(m.f0);
            3
        }
        10 => {
            out[0] = LogArg::U64(m.u0);
            out[1] = LogArg::Text(m.s0);
            2
        }
        11 => {
            out[0] = LogArg::U64(m.u0);
            out[1] = LogArg::U64(m.u1);
            2
        }
        _ => {
            out[0] = LogArg::Text(m.s0);
            out[1] = LogArg::Text(m.s1);
            out[2] = LogArg::Text(STATES[m.u0 as usize]);
            3
        }
    }
}

/// `vtr-bench log-write <n> <out.vtr> --as-tx`: the same messages written as
/// ordinary zero-duration transactions with one attribute per argument (the
/// encoding a producer would use without log support), for the rationale.
pub fn run_write_as_tx(n: u64, path: &str, opts: WriterOptions) -> serde_json::Value {
    use vtr::{AttrPhase, TxStatus, Value};
    let mut w = Writer::create_with(path, opts).unwrap();
    w.set_timescale(-9).unwrap();
    let top = w.begin_scope("top", ScopeType::Generic, "sim");
    let stream = w.add_stream(Some(top), "log", "LOG");
    w.end_scope().unwrap();
    let gens: Vec<vtr::NodeId> = (0..13).map(|k| w.add_generator(stream, FMTS[k])).collect();
    let keys: Vec<vtr::StrId> = (0..5).map(|i| w.intern(&i.to_string())).collect();
    let mut gen = Generator::new(42);
    let mut m = Msg::default();
    let mut a = [LogArg::Bool(false); 5];
    let cpu0 = cpu_seconds();
    let t0 = Instant::now();
    for _ in 0..n {
        gen.next(&mut m);
        let k = args(&m, &mut a);
        let tx = w.begin_tx(gens[m.kind], m.t).unwrap();
        for (i, arg) in a[..k].iter().enumerate() {
            let v = match arg {
                LogArg::Text(s) => Value::Text(s.to_string()),
                other => other.to_value(),
            };
            w.tx_attr(tx, keys[i], AttrPhase::Record, &v).unwrap();
        }
        w.end_tx(tx, m.t, TxStatus::Unset).unwrap();
    }
    let loop_s = t0.elapsed().as_secs_f64();
    w.close().unwrap();
    let total_s = t0.elapsed().as_secs_f64();
    json!({"logger": "vtr-as-tx", "n": n, "loop_s": loop_s, "total_s": total_s, "cpu_s": cpu_seconds() - cpu0, "bytes": file_size(path)})
}

/// `vtr-bench log-write <n> <out.vtr> [--no-background] [--codec ..] [--level L]`
pub fn run_write(n: u64, path: &str, opts: WriterOptions, label: &str) -> serde_json::Value {
    let mut w = Writer::create_with(path, opts).unwrap();
    w.set_timescale(-9).unwrap();
    let top = w.begin_scope("top", ScopeType::Generic, "sim");
    let stream = w.add_log_stream(Some(top), "log");
    w.end_scope().unwrap();
    let sites: Vec<LogSiteId> = (0..13).map(|k| w.add_log_site(&LogSiteSpec::new(stream, SEVS[k], FMTS[k], TYPES[k]).location("workload.hpp", 60 + k as u32))).collect();
    let mut gen = Generator::new(42);
    let mut m = Msg::default();
    let mut a = [LogArg::Bool(false); 5];
    let cpu0 = cpu_seconds();
    let t0 = Instant::now();
    for _ in 0..n {
        gen.next(&mut m);
        let k = args(&m, &mut a);
        w.log(sites[m.kind], m.t, &a[..k]).unwrap();
    }
    let loop_s = t0.elapsed().as_secs_f64();
    w.close().unwrap();
    let total_s = t0.elapsed().as_secs_f64();
    json!({"logger": label, "n": n, "loop_s": loop_s, "total_s": total_s, "cpu_s": cpu_seconds() - cpu0, "bytes": file_size(path)})
}

/// `vtr-bench log-encode <n>`: encodes the rows of the workload into one block
/// with several codecs and reports raw size, compressed size and encode/decode time.
pub fn run_encode(n: u64) -> serde_json::Value {
    let sites: Vec<LogSiteEnc> = (0..13).map(|k| LogSiteEnc { node: 3 + k as u32, args: TYPES[k].to_vec() }).collect();
    let sites = Arc::new(sites);
    let mut rows = Vec::new();
    let mut gen = Generator::new(42);
    let mut m = Msg::default();
    let mut a = [LogArg::Bool(false); 5];
    let t0 = Instant::now();
    for i in 0..n {
        gen.next(&mut m);
        let k = args(&m, &mut a);
        logblock::encode_row(&mut rows, m.kind as u32, m.t, i + 1, 0, &a[..k]);
    }
    let row_s = t0.elapsed().as_secs_f64();
    let input = LogBlockInput { rows, n, sites: sites.clone() };
    let mut variants = Vec::new();
    for (name, comp) in [
        ("none", Compression::NONE),
        ("lz4", Compression::LZ4),
        ("zstd-1", Compression::ZSTD_FAST),
        ("zstd-3", Compression::ZSTD_DEFAULT),
        ("zstd-6", Compression { codec: vtr::Codec::Zstd, level: 6 }),
    ] {
        let mut comp_obj = vtr::codec::Compressor::new();
        let mut best = f64::MAX;
        let mut out = Vec::new();
        for _ in 0..3 {
            out.clear();
            let t = Instant::now();
            logblock::encode_log_block(&input, comp, &mut comp_obj, &mut out).unwrap();
            best = best.min(t.elapsed().as_secs_f64());
        }
        let mut dbest = f64::MAX;
        for _ in 0..3 {
            let t = Instant::now();
            let d = logblock::decode_log_block(&out, &mut vtr::codec::Decompressor::new(), |g| sites.iter().find(|s| s.node == g).map(|s| s.args.as_slice())).unwrap();
            dbest = dbest.min(t.elapsed().as_secs_f64());
            assert_eq!(d.recs.len() as u64, n);
        }
        variants.push(json!({"codec": name, "bytes": out.len(), "encode_s": best, "decode_s": dbest}));
    }
    json!({"n": n, "row_bytes": input.rows.len(), "row_encode_s": row_s, "variants": variants})
}
