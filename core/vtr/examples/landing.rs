//! Demo trace for Volna's landing page (docs/volna-landing.html): a small SoC
//! runs a loop on a five-stage core, misses in L1 and L2, refills from DRAM,
//! takes an interrupt and records it all on one time base: waveforms (bits,
//! buses with X/Z, text, an event), analog performance signals, two declared
//! clocks (one boosted, then gated), a PIPELINE stream with stalls, flushes and
//! dependencies, bus transactions with stages, parents and relations, and
//! logs. No VDB presentation data; Volna's layout for it lives in
//! volna/volna/examples/landing.vtr.volna.json.
//!
//! cargo run --locked -p vtr --example landing -- volna/volna/examples/landing.vtr

use std::collections::HashMap;
use std::path::Path;
use vtr::{
    Direction, LogArg, LogArgType, LogQuery, LogSiteSpec, NodeId, Reader, ScopeType, Severity,
    SignalId, SignalKind, StrId, TxId, TxQuery, TxStatus, Value, VarType, Writer, WriterOptions,
};

/// The page downloads this file: keep it small.
const MAX_BYTES: u64 = 150_000;
/// `soc.clk`: 2 ns, first rising edge at 1 ns. Times are nanoseconds.
const PERIOD: u64 = 2;
const FIRST_EDGE: u64 = 1;
/// Instructions fetched from the loop and the interrupt handler.
const INSTRUCTIONS: usize = 420;
/// The device interrupt arrives at this cycle.
const IRQ_CYCLE: u64 = 360;
/// Cycles of idle recording after the last instruction retires.
const IDLE_CYCLES: u64 = 40;
/// Cycles a read spends at L2 on a hit, and at DRAM on an L2 miss.
const L2_LATENCY: u64 = 4;
const DRAM_LATENCY: u64 = 14;

/// Rising edge that begins core cycle `cycle`.
fn edge(cycle: u64) -> u64 {
    FIRST_EDGE + PERIOD * cycle
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Alu,
    Mul,
    Load,
    Store,
    Branch,
    Return,
}

struct Op {
    text: &'static str,
    dst: Option<u8>,
    src: [Option<u8>; 2],
    kind: Kind,
}

const fn op(text: &'static str, dst: Option<u8>, src: [Option<u8>; 2], kind: Kind) -> Op {
    Op { text, dst, src, kind }
}

/// A dot product over a buffer: loads that miss, a multiply, dependent adds,
/// a store and the loop branch.
const LOOP: &[Op] = &[
    op("lw   a1, 0(a2)", Some(11), [Some(12), None], Kind::Load),
    op("lw   a3, 0(a4)", Some(13), [Some(14), None], Kind::Load),
    op("mul  a5, a1, a3", Some(15), [Some(11), Some(13)], Kind::Mul),
    op("add  a0, a0, a5", Some(10), [Some(10), Some(15)], Kind::Alu),
    op("addi a2, a2, 4", Some(12), [Some(12), None], Kind::Alu),
    op("addi a4, a4, 4", Some(14), [Some(14), None], Kind::Alu),
    op("sw   a0, 0(s1)", None, [Some(10), Some(9)], Kind::Store),
    op("addi t0, t0, -1", Some(5), [Some(5), None], Kind::Alu),
    op("bnez t0, loop", None, [Some(5), None], Kind::Branch),
];

/// The interrupt handler: save, read the device's cause register (an
/// uncached read that goes to DRAM), acknowledge, restore, return.
const ISR: &[Op] = &[
    op("csrrw sp, mscratch, sp", Some(2), [Some(2), None], Kind::Alu),
    op("sw   ra, 0(sp)", None, [Some(1), Some(2)], Kind::Store),
    op("lw   t1, 64(gp)", Some(6), [Some(3), None], Kind::Load),
    op("sw   t1, 68(gp)", None, [Some(6), Some(3)], Kind::Store),
    op("lw   ra, 0(sp)", Some(1), [Some(2), None], Kind::Load),
    op("mret", None, [None, None], Kind::Return),
];

const STAGES: [&str; 5] = ["F", "D", "X", "M", "W"];
const EXECUTE: usize = 2;
const MEMORY: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    L2,
    Dram,
}

/// A memory access at L2, in cycles.
struct Access {
    write: bool,
    begin: u64,
    end: u64,
    address: u64,
    level: Level,
}

struct Insn {
    index: usize,
    pc: u64,
    op: &'static Op,
    isr: bool,
    /// (begin, end) cycles per stage; flushed instructions have fewer.
    stages: Vec<(u64, u64)>,
    /// (begin, end, reason) of the waits between stages.
    stalls: Vec<(u64, u64, &'static str)>,
    end: u64,
    status: TxStatus,
    deps: Vec<usize>,
    access: Option<Access>,
    l1_miss: bool,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn chance(&mut self, one_in: u64) -> bool {
        self.next() % one_in == 0
    }
}

struct Run {
    insns: Vec<Insn>,
    /// Cycle the interrupt was taken.
    irq: u64,
    /// Cycles at which a mispredicted branch flushed younger instructions.
    flushes: Vec<u64>,
}

/// In-order single-issue pipeline: operand waits, loads that miss stall the
/// memory stage, every fifth loop branch mispredicts, and one interrupt runs
/// the handler between two loop iterations.
fn simulate() -> Run {
    let mut rng = Rng(0x7a11_d1c3);
    let mut occupancy = vec![0u64; STAGES.len()];
    let mut writers: HashMap<u8, (usize, u64)> = HashMap::new();
    let mut insns: Vec<Insn> = Vec::new();
    let mut fetch = 6u64; // reset is released at cycle 4
    let mut loop_index = 0usize;
    let mut isr_index: Option<usize> = None;
    let mut irq = None;
    let mut branches = 0usize;
    let mut flush_at: Option<(u64, usize)> = None;
    let mut flushes = Vec::new();
    let mut iteration = 0u64;
    while insns.len() < INSTRUCTIONS {
        let index = insns.len();
        if irq.is_none() && flush_at.is_none() && isr_index.is_none() && fetch >= IRQ_CYCLE {
            irq = Some(fetch);
            isr_index = Some(0);
            fetch += 1;
        }
        let (op, pc, isr) = match isr_index {
            Some(i) => (&ISR[i], 0x8000_0200 + 4 * i as u64, true),
            None => {
                let i = loop_index % LOOP.len();
                (&LOOP[i], 0x8000_0040 + 4 * i as u64, false)
            }
        };
        let mut stages: Vec<(u64, u64)> = Vec::with_capacity(STAGES.len());
        let mut stalls = Vec::new();
        let mut deps = Vec::new();
        let mut access = None;
        let mut l1_miss = false;
        let mut begin = fetch;
        for k in 0..STAGES.len() {
            let mut start = begin.max(occupancy[k]);
            let mut reason = "pipeline busy";
            let mut duration = 1;
            if k == EXECUTE {
                for reg in op.src.iter().flatten() {
                    if let Some(&(producer, ready)) = writers.get(reg) {
                        deps.push(producer);
                        if ready > start {
                            start = ready;
                            reason = "operand";
                        }
                    }
                }
                if op.kind == Kind::Mul {
                    duration = 3;
                }
            }
            if k == MEMORY && matches!(op.kind, Kind::Load | Kind::Store) {
                let address = if isr {
                    0x4000_0040 + if op.kind == Kind::Store { 4 } else { 0 }
                } else {
                    0x1000_0000 + 0x40 * iteration + 0x800 * (loop_index % LOOP.len() == 1) as u64
                };
                if op.kind == Kind::Load {
                    // The handler's device read is uncached; loop loads miss L1 one time in three.
                    l1_miss = (isr && op.text.contains("64(gp)")) || (!isr && rng.chance(3));
                    if l1_miss {
                        let level = if isr || rng.chance(2) { Level::Dram } else { Level::L2 };
                        duration += if level == Level::Dram { DRAM_LATENCY } else { L2_LATENCY };
                        access = Some(Access {
                            write: false,
                            begin: start,
                            end: start + duration,
                            address,
                            level,
                        });
                    }
                } else {
                    // Write-through stores post to L2 after the memory stage.
                    access = Some(Access {
                        write: true,
                        begin: start + 1,
                        end: start + 4,
                        address,
                        level: Level::L2,
                    });
                }
            }
            if k > 0 && start > begin {
                // Waiting to enter stage k keeps the instruction in stage k-1.
                stages[k - 1].1 = start;
                occupancy[k - 1] = start;
                stalls.push((begin, start, reason));
            }
            let end = start + duration;
            stages.push((start, end));
            occupancy[k] = end;
            begin = end;
        }
        deps.sort_unstable();
        deps.dedup();
        if let Some(dst) = op.dst {
            let ready = if op.kind == Kind::Load { stages[MEMORY].1 } else { stages[EXECUTE].1 };
            writers.insert(dst, (index, ready));
        }
        let end = stages.last().unwrap().1;
        insns.push(Insn {
            index,
            pc,
            op,
            isr,
            stages,
            stalls,
            end,
            status: TxStatus::Ok,
            deps,
            access,
            l1_miss,
        });
        fetch += 1;
        // A mispredicted branch resolves in execute and flushes the two
        // younger fall-through instructions fetched behind it.
        if let Some((resolve, first_victim)) = flush_at.filter(|(_, first)| index + 1 >= first + 2) {
            for victim in &mut insns[first_victim..] {
                victim.stages.retain(|(b, _)| *b < resolve);
                for stage in &mut victim.stages {
                    stage.1 = stage.1.min(resolve);
                }
                victim.stalls.retain(|(b, ..)| *b < resolve);
                for stall in &mut victim.stalls {
                    stall.1 = stall.1.min(resolve);
                }
                victim.end = resolve;
                victim.status = TxStatus::Aborted;
                victim.access = None;
                victim.deps.clear();
            }
            flushes.push(resolve);
            fetch = resolve + 1;
            for slot in &mut occupancy {
                *slot = (*slot).min(fetch);
            }
            writers.retain(|_, (producer, _)| *producer < first_victim);
            flush_at = None;
            loop_index -= loop_index % LOOP.len(); // the branch was taken: loop head
            continue;
        }
        match isr_index {
            Some(i) if i + 1 == ISR.len() => isr_index = None,
            Some(i) => isr_index = Some(i + 1),
            None => {
                if op.kind == Kind::Branch {
                    branches += 1;
                    iteration += 1;
                    if branches % 5 == 0 {
                        let resolve = insns[index].stages[EXECUTE].1;
                        flush_at = Some((resolve, index + 1));
                    }
                }
                loop_index += 1;
            }
        }
    }
    // The last fetches may still await a flush that never resolved: keep them.
    Run {
        insns,
        irq: irq.expect("the interrupt arrives before the program ends"),
        flushes,
    }
}

fn bits(w: &mut Writer, parent: NodeId, name: &str, width: u32, states: u8, dir: Direction) -> vtr::Result<SignalId> {
    Ok(w.add_var(Some(parent), name, VarType::Logic, dir, SignalKind::Bits { width, states })?.1)
}

fn real(w: &mut Writer, parent: NodeId, name: &str) -> vtr::Result<SignalId> {
    Ok(w.add_var(Some(parent), name, VarType::Real, Direction::Implicit, SignalKind::Real)?.1)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("usage: landing OUTPUT.vtr");
    let path = Path::new(&path);
    let run = simulate();
    let insns = &run.insns;
    let last_busy = insns.iter().map(|i| i.end).max().unwrap();
    let last_cycle = last_busy + IDLE_CYCLES;
    let dram_boost = edge(run.irq) + 1;
    let dram_gate = edge(last_busy + 8);

    let mut w = Writer::create_with(path, WriterOptions { background: false, ..WriterOptions::default() })?;
    w.set_timescale(-9)?;
    w.set_file_type(vtr::FileType::VerilogVhdl)?;
    w.set_writer_name("VTR landing demo")?;
    w.set_date("2026-09-27")?;
    w.set_comment("Volna landing demo: a dot-product loop with cache misses, DRAM refills and one interrupt. See examples/README.md.")?;

    // -- hierarchy ------------------------------------------------------------------
    let soc = w.add_scope(None, "soc", ScopeType::Module, "soc_top")?;
    let clk = bits(&mut w, soc, "clk", 1, 2, Direction::Input)?;
    let reset_n = bits(&mut w, soc, "reset_n", 1, 4, Direction::Input)?;
    let cpu = w.add_scope(Some(soc), "cpu0", ScopeType::Core, "rv32_core")?;
    let pc = bits(&mut w, cpu, "pc", 32, 2, Direction::Output)?;
    let (_, phase) = w.add_var(Some(cpu), "phase", VarType::String, Direction::Implicit, SignalKind::VarLen)?;
    let stall = bits(&mut w, cpu, "stall", 1, 2, Direction::Output)?;
    let flush = bits(&mut w, cpu, "flush", 1, 2, Direction::Output)?;
    let (_, irq) = w.add_var(Some(cpu), "irq", VarType::Event, Direction::Input, SignalKind::Bits { width: 1, states: 2 })?;
    let retired = bits(&mut w, cpu, "retired", 16, 2, Direction::Output)?;
    let pipeline = w.add_stream(Some(cpu), "pipeline", "PIPELINE")?;
    let core_clock = w.intern("soc.clk");
    w.node_attr(pipeline, "vtr.clock", Value::Str(core_clock))?;
    let instruction = w.add_generator(pipeline, "instruction")?;

    let l2 = w.add_scope(Some(soc), "l2", ScopeType::Module, "l2_cache")?;
    let req_valid = bits(&mut w, l2, "req_valid", 1, 2, Direction::Input)?;
    let req_write = bits(&mut w, l2, "req_write", 1, 2, Direction::Input)?;
    let req_addr = bits(&mut w, l2, "req_addr", 32, 4, Direction::Input)?;
    let resp_valid = bits(&mut w, l2, "resp_valid", 1, 2, Direction::Output)?;
    let resp_data = bits(&mut w, l2, "resp_data", 32, 4, Direction::Output)?;
    let mshr = bits(&mut w, l2, "mshr_used", 3, 2, Direction::Output)?;
    let bus = w.add_stream(Some(l2), "bus", "MEMORY_BUS")?;
    w.node_attr(bus, "vtr.clock", Value::Str(core_clock))?;
    let reads = w.add_generator(bus, "read")?;
    let writes = w.add_generator(bus, "write")?;

    let dram = w.add_scope(Some(soc), "dram", ScopeType::Module, "lpddr_ctrl")?;
    let dram_bus = w.add_stream(Some(dram), "bus", "MEMORY_BUS")?;
    let dram_clock = w.intern("soc.dram.dram_clk");
    w.node_attr(dram_bus, "vtr.clock", Value::Str(dram_clock))?;
    let bursts = w.add_generator(dram_bus, "burst")?;

    let perf = w.add_scope(Some(soc), "perf", ScopeType::Module, "perf_monitor")?;
    let ipc = real(&mut w, perf, "ipc")?;
    let bandwidth = real(&mut w, perf, "l2_bandwidth_gbps")?;
    let temperature = real(&mut w, perf, "temperature_c")?;

    let logs = w.add_stream(Some(soc), "log", vtr::LOG_STREAM_KIND)?;
    let text_label = |w: &mut Writer, severity, format: &str, args: &[LogArgType], names: &[&str], line| {
        w.add_log_site(
            &LogSiteSpec::new(logs, severity, format, args)
                .names(names)
                .location("tb/soc_tb.sv", line)
                .func("monitor"),
        )
    };
    let boot_site = text_label(&mut w, Severity::Info, "reset released at cycle {}", &[LogArgType::U64, LogArgType::Text], &["cycle", "vtr.label"], 41)?;
    let irq_site = text_label(&mut w, Severity::Info, "irq {} taken at pc {}", &[LogArgType::U64, LogArgType::Pointer, LogArgType::Text], &["line", "pc", "vtr.label"], 58)?;
    let miss_site = text_label(&mut w, Severity::Warn, "l2 miss at {} refilled in {} cycles", &[LogArgType::Pointer, LogArgType::U64, LogArgType::Text], &["address", "cycles", "vtr.label"], 73)?;
    let ecc_site = text_label(&mut w, Severity::Error, "dram ecc error at {} corrected", &[LogArgType::Pointer, LogArgType::Text], &["address", "vtr.label"], 88)?;
    let idle_site = text_label(&mut w, Severity::Info, "core idle, dram enters self-refresh at {}", &[LogArgType::Time, LogArgType::Text], &["time", "vtr.label"], 95)?;

    // Declared clocks: the dumped core clock, and a DRAM clock with no net
    // that boosts from 3 ns to 2 ns when the interrupt arrives and is gated
    // for self-refresh for 40 ns once the core goes idle.
    let core_clk = w.add_clock(Some(soc), "clk")?;
    w.clock_run(core_clk, FIRST_EDGE, PERIOD)?;
    let dram_clk = w.add_clock(Some(dram), "dram_clk")?;
    w.clock_run(dram_clk, 0, 3)?;
    w.clock_stop(dram_clk, dram_boost - 1)?;
    w.clock_run(dram_clk, dram_boost, 2)?;
    w.clock_stop(dram_clk, dram_gate)?;
    w.clock_run(dram_clk, dram_gate + 40, 2)?;

    // -- waveforms, in time order --------------------------------------------------------
    let accesses: Vec<&Access> = insns.iter().filter_map(|i| i.access.as_ref()).collect();
    let isr_end = insns.iter().filter(|i| i.isr).map(|i| i.end).max().unwrap();
    let mut temp = 41.0f64;
    for t in 0..=edge(last_cycle) {
        w.set_time(t)?;
        w.emit_bit(clk, u8::from(t >= FIRST_EDGE && (t - FIRST_EDGE) % PERIOD == 0))?;
        if t == 0 {
            w.emit_logic_str(reset_n, b"x")?;
            w.emit_varlen(phase, b"reset")?;
            w.emit_u64(pc, 0x8000_0000)?;
            w.emit_logic_str(req_addr, &[b'x'; 32])?;
            w.emit_logic_str(resp_data, &[b'z'; 32])?;
        }
        if t < FIRST_EDGE || (t - FIRST_EDGE) % PERIOD != 0 {
            continue;
        }
        let c = (t - FIRST_EDGE) / PERIOD;
        w.emit_logic_str(reset_n, if c < 4 { b"0" } else { b"1" })?;
        let fetched = insns.iter().find(|i| i.stages.first().is_some_and(|(b, _)| *b == c));
        if let Some(i) = fetched {
            w.emit_u64(pc, i.pc)?;
        }
        let phase_text: &[u8] = if c < 6 {
            b"reset"
        } else if c >= run.irq && c < isr_end {
            b"irq handler"
        } else if c >= last_busy {
            b"idle (wfi)"
        } else {
            b"dot product loop"
        };
        w.emit_varlen(phase, phase_text)?;
        w.emit_bit(stall, u8::from(insns.iter().any(|i| i.stalls.iter().any(|(b, e, _)| *b <= c && c < *e))))?;
        w.emit_bit(flush, u8::from(run.flushes.contains(&c)))?;
        if c == run.irq {
            w.emit_bit(irq, 1)?;
        }
        let done = insns.iter().filter(|i| i.status == TxStatus::Ok && i.end <= c).count() as u64;
        w.emit_u64(retired, done)?;
        let request = accesses.iter().find(|a| a.begin == c);
        w.emit_bit(req_valid, u8::from(request.is_some()))?;
        w.emit_bit(req_write, u8::from(request.is_some_and(|a| a.write)))?;
        match request {
            Some(a) => w.emit_u64(req_addr, a.address)?,
            None => w.emit_logic_str(req_addr, &[b'x'; 32])?,
        }
        let response = accesses.iter().find(|a| !a.write && a.end == c + 1);
        w.emit_bit(resp_valid, u8::from(response.is_some()))?;
        match response {
            Some(a) => w.emit_u64(resp_data, (a.address.wrapping_mul(0x9e37_79b9) >> 7) & 0xffff_ffff)?,
            None => w.emit_logic_str(resp_data, &[b'z'; 32])?,
        }
        let outstanding = accesses.iter().filter(|a| !a.write && a.begin <= c && c < a.end).count() as u64;
        w.emit_u64(mshr, outstanding.min(7))?;
        if c % 4 == 0 {
            // Over the last 32 cycles: retired instructions per cycle, and
            // 64-byte lines moved per nanosecond.
            let window = c.saturating_sub(32)..c;
            let recent = insns.iter().filter(|i| i.status == TxStatus::Ok && window.contains(&i.end)).count();
            let lines = accesses.iter().filter(|a| window.contains(&a.begin)).count();
            let ipc_now = recent as f64 / 32.0;
            w.emit_real(ipc, ipc_now)?;
            w.emit_real(bandwidth, lines as f64 * 64.0 / (32.0 * PERIOD as f64))?;
            let target = 40.0 + 30.0 * ipc_now + 4.0 * lines as f64 / 8.0;
            temp += (target - temp) * 0.08;
            w.emit_real(temperature, (temp * 100.0).round() / 100.0)?;
        }
    }

    // -- transactions ------------------------------------------------------------------------
    let k_label = w.intern("vtr.label");
    let k_pc = w.intern("pc");
    let k_index = w.intern("insn_id");
    let k_l1 = w.intern("l1");
    let k_detail = w.intern("detail");
    let k_reason = w.intern("reason");
    let k_addr = w.intern("address");
    let k_bytes = w.intern("bytes");
    let k_level = w.intern("served_by");
    let k_bank = w.intern("bank");
    let k_row = w.intern("row");
    let k_ecc = w.intern("ecc");
    let k_wakeup = w.intern("wakeup");
    let k_causes = w.intern("causes");
    let k_refill = w.intern("refill");
    let lane0 = w.intern("0");
    let lane_stall = w.intern("stall");
    let stall_name = w.intern("stl");
    let stage_names: Vec<StrId> = STAGES.iter().map(|s| w.intern(s)).collect();
    let s_addr = w.intern("addr");
    let s_data = w.intern("data");
    let s_act = w.intern("act");
    let s_cas = w.intern("cas");
    let s_burst = w.intern("burst");
    let hit = w.intern("hit");
    let miss = w.intern("miss");
    let served_l2 = w.intern("L2");
    let served_dram = w.intern("DRAM");
    let corrected = w.intern("corrected");
    let reasons: HashMap<&str, StrId> = ["operand", "pipeline busy"].into_iter().map(|r| (r, w.intern(r))).collect();

    w.log(boot_site, edge(4), &[LogArg::U64(4), LogArg::Text("reset released")])?;
    let mut ids: Vec<TxId> = Vec::with_capacity(insns.len());
    let mut refills = 0u64;
    let mut dram_misses = Vec::new();
    for insn in insns {
        let begin = edge(insn.stages[0].0);
        let tx = w.begin_tx(instruction, begin)?;
        ids.push(tx);
        let label = w.intern(&format!("{:08x}: {}", insn.pc, insn.op.text));
        w.tx_attr(tx, k_label, &Value::Str(label))?;
        w.tx_attr(tx, k_index, &Value::U64(insn.index as u64))?;
        w.tx_attr(tx, k_pc, &Value::U64(insn.pc))?;
        if insn.op.kind == Kind::Load && insn.status == TxStatus::Ok {
            w.tx_attr(tx, k_l1, &Value::Str(if insn.l1_miss { miss } else { hit }))?;
        }
        for (k, (b, e)) in insn.stages.iter().enumerate() {
            w.tx_stage(tx, stage_names[k], lane0, edge(*b), edge(*e), &[])?;
        }
        for (b, e, reason) in &insn.stalls {
            w.tx_stage(tx, stall_name, lane_stall, edge(*b), edge(*e), &[(k_reason, Value::Str(reasons[reason]))])?;
        }
        for &producer in &insn.deps {
            w.relate(k_wakeup, ids[producer], tx, &[])?;
        }
        if insn.isr && insn.index == insns.iter().find(|i| i.isr).unwrap().index {
            w.log_with_parent(irq_site, edge(run.irq), Some(tx), &[LogArg::U64(3), LogArg::Pointer(insn.pc), LogArg::Text("device interrupt")])?;
        }
        if let Some(a) = &insn.access {
            let generator = if a.write { writes } else { reads };
            let request = w.begin_tx(generator, edge(a.begin))?;
            w.set_tx_parent(request, tx)?;
            let text = format!("{} {:#010x}", if a.write { "write" } else { "read" }, a.address);
            let label = w.intern(&text);
            w.tx_attr(request, k_label, &Value::Str(label))?;
            w.tx_attr(request, k_addr, &Value::U64(a.address))?;
            w.tx_attr(request, k_bytes, &Value::U64(if a.write { 4 } else { 64 }))?;
            w.tx_attr(request, k_level, &Value::Str(if a.level == Level::Dram { served_dram } else { served_l2 }))?;
            w.tx_stage(request, s_addr, lane0, edge(a.begin), edge(a.begin + 1), &[])?;
            w.tx_stage(request, s_data, lane0, edge(a.end - 1), edge(a.end), &[])?;
            w.relate(k_causes, tx, request, &[])?;
            if a.level == Level::Dram {
                // The refill: activate, column access, then the data burst.
                let (b, e) = (a.begin + 2, a.end - 2);
                let burst = w.begin_tx(bursts, edge(b))?;
                w.set_tx_parent(burst, request)?;
                let label = w.intern(&format!("refill {:#010x}", a.address));
                w.tx_attr(burst, k_label, &Value::Str(label))?;
                w.tx_attr(burst, k_addr, &Value::U64(a.address))?;
                w.tx_attr(burst, k_bank, &Value::U64((a.address >> 6) & 7))?;
                w.tx_attr(burst, k_row, &Value::U64(a.address >> 13))?;
                w.tx_stage(burst, s_act, lane0, edge(b), edge(b + 4), &[])?;
                w.tx_stage(burst, s_cas, lane0, edge(b + 4), edge(e - 4), &[])?;
                w.tx_stage(burst, s_burst, lane0, edge(e - 4), edge(e), &[])?;
                w.relate(k_refill, burst, request, &[])?;
                refills += 1;
                let status = if refills == 9 {
                    w.tx_attr(burst, k_ecc, &Value::Str(corrected))?;
                    w.log_with_parent(ecc_site, edge(e - 1), Some(burst), &[LogArg::Pointer(a.address), LogArg::Text("ecc error")])?;
                    TxStatus::Error
                } else {
                    TxStatus::Ok
                };
                w.end_tx(burst, edge(e), status)?;
                if dram_misses.len() < 6 {
                    w.log_with_parent(miss_site, edge(a.end), Some(request), &[LogArg::Pointer(a.address), LogArg::U64(a.end - a.begin), LogArg::Text("l2 miss")])?;
                    dram_misses.push(a.address);
                }
            }
            w.end_tx(request, edge(a.end), TxStatus::Ok)?;
        }
        if insn.status == TxStatus::Aborted {
            let detail = w.intern("squashed by a mispredicted branch");
            w.tx_attr(tx, k_detail, &Value::Str(detail))?;
        }
        w.end_tx(tx, edge(insn.end), insn.status)?;
    }
    w.log(idle_site, dram_gate, &[LogArg::Time(dram_gate), LogArg::Text("self-refresh")])?;
    w.set_time(edge(last_cycle))?;
    w.close()?;

    // -- verify --------------------------------------------------------------------------
    let size = std::fs::metadata(path)?.len();
    if size >= MAX_BYTES {
        return Err(format!("{} is {size} bytes, limit {MAX_BYTES}", path.display()).into());
    }
    let r = Reader::open(path)?;
    let (n_tx, n_rel) = r.tx_counts();
    let mut per_generator: HashMap<String, usize> = HashMap::new();
    let mut statuses: HashMap<TxStatus, usize> = HashMap::new();
    let mut with_stalls = 0;
    r.visit_transactions(&TxQuery::default(), |tx| {
        *per_generator.entry(r.name(tx.generator).to_owned()).or_default() += 1;
        *statuses.entry(tx.status).or_default() += 1;
        with_stalls += usize::from(tx.stages.iter().any(|s| r.str(s.lane) == "stall"));
        true
    })?;
    let mut log_count = 0;
    r.visit_log(&LogQuery::default(), |_| {
        log_count += 1;
        true
    })?;
    let count = |name: &str| per_generator.get(name).copied().unwrap_or(0);
    assert_eq!(count("instruction"), INSTRUCTIONS);
    assert_eq!(count("burst") as u64, refills);
    assert!(count("read") > 20 && count("write") > 20 && refills >= 9);
    assert!(statuses.get(&TxStatus::Aborted).is_some_and(|&n| n > 0), "no flushed instructions");
    assert!(statuses.get(&TxStatus::Error).is_some_and(|&n| n == 1), "one ECC error");
    assert!(with_stalls > 0 && n_rel > 0);
    assert_eq!(log_count, 4 + dram_misses.len());
    assert_eq!(r.clocks().len(), 2);
    assert_eq!(r.time_range().unwrap().1, edge(last_cycle));
    let mut names: Vec<_> = per_generator.into_iter().collect();
    names.sort();
    println!(
        "{}: {size} bytes, 0..{} ns, irq at cycle {} ({} ns), {} flushes, {n_tx} transactions, {n_rel} relations, {log_count} logs",
        path.display(),
        edge(last_cycle),
        run.irq,
        edge(run.irq),
        run.flushes.len(),
    );
    for (name, count) in names {
        println!("  {name}: {count}");
    }
    Ok(())
}
