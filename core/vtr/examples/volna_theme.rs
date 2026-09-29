//! Showcase trace for the proposed Volna theme (docs/volna-theme.html): one
//! small SoC whose recording holds every case the page specifies, so each
//! colour, shape and rendering rule can be seen on real data.
//!
//! - Values: 1-bit highs and lows, buses, X (all-X and partial), Z on bits
//!   and buses, don't-care, weak drive, events coalescing into ×n.
//! - Rendering: dense columns of values only, of X only, and of values mixed
//!   with X or Z; activity from a slow toggle to a burst; zero-idle counters;
//!   neighbouring addresses that differ in their last digits; one-digit
//!   segments; text states that recur; analog rows with a one-sample spike.
//! - Clocks: a dumped 1 ns core clock, a declared DDR clock with no net that
//!   boosts and is gated, a slow always-on clock and an undeclared gated SPI
//!   clock.
//! - Pipelines: a five-stage core and a thirteen-stage core named after
//!   C910's stages (the page's ladder), with stalls, flushed (`Aborted`)
//!   instructions, `wakeup` relations and instructions still `Open` at the end.
//! - Transactions: L2 reads and writes parented to their instructions, DMA
//!   descriptors with stages, one error, logs at five severities.
//! - Hierarchy: a 14-level path, one signal under five port paths, 64
//!   numbered ROB entries, a quiet vector FPU and static configuration.
//!
//! Times are picoseconds. No VDB presentation data.
//!
//! cargo run --locked -p vtr --example volna_theme -- volna/volna/examples/volna-theme.vtr

use std::collections::HashMap;
use std::path::Path;
use vtr::{
    Direction, LogArg, LogArgType, LogQuery, LogSiteSpec, NodeId, Reader, ScopeType, Severity,
    SignalId, SignalKind, StrId, TxId, TxQuery, TxStatus, Value, VarType, Writer, WriterOptions,
};

/// Keep the example small enough to check in and to download on a page.
const MAX_BYTES: u64 = 500_000;
/// `soc.clk`: 1 ns, first rising edge at 500 ps.
const PERIOD: u64 = 1000;
const FIRST_EDGE: u64 = 500;
/// Recorded core cycles.
const CYCLES: u64 = 4096;
/// Reset is released at this cycle.
const RESET_DONE: u64 = 6;
/// SRAM built-in self test: the read port returns only unknowns.
const BIST: (u64, u64) = (8, 240);
/// DMA descriptors run in this window.
const DMA: (u64, u64) = (1000, 2400);
/// The device interrupt, which also boosts the DDR clock.
const IRQ_CYCLE: u64 = 2048;
/// L2 responses carry an uncorrectable byte in this window.
const FAULT: (u64, u64) = (2600, 2700);
/// cpu0 waits for an interrupt and the DDR clock is gated for self-refresh.
const WFI: (u64, u64) = (3000, 3400);
/// Instructions fetched after this cycle stay in flight at close.
const LAST_FETCH: u64 = CYCLES - 2;

/// Rising edge that begins cycle `cycle`.
fn edge(cycle: u64) -> u64 {
    FIRST_EDGE + PERIOD * cycle
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
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------------------------------------
// Pipelines

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Alu,
    Mul,
    Load,
    Store,
    Branch,
}

struct Op {
    text: &'static str,
    dst: Option<u8>,
    src: [Option<u8>; 2],
    kind: Kind,
}

const fn op(text: &'static str, dst: Option<u8>, src: [Option<u8>; 2], kind: Kind) -> Op {
    Op {
        text,
        dst,
        src,
        kind,
    }
}

/// A dot product: loads that miss, a multiply, dependent adds, a store, the branch.
const DOT: &[Op] = &[
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

/// A checksum over a list: pointer chasing, shifts and an xor fold.
const CHASE: &[Op] = &[
    op("ld   t1, 8(a0)", Some(6), [Some(10), None], Kind::Load),
    op("ld   a0, 0(a0)", Some(10), [Some(10), None], Kind::Load),
    op("slli t2, t1, 7", Some(7), [Some(6), None], Kind::Alu),
    op("xor  a1, a1, t2", Some(11), [Some(11), Some(7)], Kind::Alu),
    op("mul  a2, a1, s2", Some(12), [Some(11), Some(18)], Kind::Mul),
    op("sd   a2, 0(s3)", None, [Some(12), Some(19)], Kind::Store),
    op("bnez a0, next", None, [Some(10), None], Kind::Branch),
];

struct Core {
    stages: &'static [&'static str],
    /// Stage that executes (a multiply takes three cycles) and resolves branches.
    execute: usize,
    /// Stage that accesses memory.
    memory: usize,
    ops: &'static [Op],
    base_pc: u64,
    base_addr: u64,
    seed: u64,
    mispredict_every: usize,
    idle: Option<(u64, u64)>,
}

const CPU0: Core = Core {
    stages: &["F", "D", "X", "M", "W"],
    execute: 2,
    memory: 3,
    ops: DOT,
    base_pc: 0x8000_0040,
    base_addr: 0x8010_0000,
    seed: 0x7a11_d1c3,
    mispredict_every: 5,
    idle: Some(WFI),
};

/// C910's stage names, as the page's ladder lists them, run in order.
const CPU1: Core = Core {
    stages: &[
        "ID", "IR", "IS", "IQ", "RF", "EX", "AG", "BJ", "DC", "CM", "DA", "RT", "WB",
    ],
    execute: 5,
    memory: 8,
    ops: CHASE,
    base_pc: 0x8002_1000,
    base_addr: 0x8040_0000,
    seed: 0x0c91_0c91,
    mispredict_every: 7,
    idle: None,
};

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
    /// (begin, end) cycles per stage; flushed instructions have fewer.
    stages: Vec<(u64, u64)>,
    /// (begin, end, reason) of the waits between stages.
    stalls: Vec<(u64, u64, &'static str)>,
    end: u64,
    status: TxStatus,
    deps: Vec<usize>,
    access: Option<Access>,
}

/// In-order single-issue pipeline: operand waits, loads that miss stall the
/// memory stage, every n-th branch mispredicts and flushes the instructions
/// fetched behind it. Fetch stops at `LAST_FETCH`, so the youngest
/// instructions are still in flight when the recording ends.
fn simulate(core: &Core) -> (Vec<Insn>, Vec<u64>) {
    let mut rng = Rng(core.seed);
    let n = core.stages.len();
    let mut occupancy = vec![0u64; n];
    let mut writers: HashMap<u8, (usize, u64)> = HashMap::new();
    let mut insns: Vec<Insn> = Vec::new();
    let mut fetch = RESET_DONE + 2;
    let mut slot = 0usize;
    let mut branches = 0usize;
    let mut iteration = 0u64;
    let mut flush_at: Option<(u64, usize)> = None;
    let mut flushes = Vec::new();
    while fetch < LAST_FETCH {
        if let Some((a, b)) = core.idle.filter(|(a, b)| (*a..*b).contains(&fetch)) {
            debug_assert!(a < b);
            fetch = b;
            occupancy.iter_mut().for_each(|o| *o = (*o).max(b));
        }
        let index = insns.len();
        let op = &core.ops[slot % core.ops.len()];
        let pc = core.base_pc + 4 * (slot % core.ops.len()) as u64;
        let mut stages: Vec<(u64, u64)> = Vec::with_capacity(n);
        let mut stalls = Vec::new();
        let mut deps = Vec::new();
        let mut access = None;
        let mut begin = fetch;
        for k in 0..n {
            let mut start = begin.max(occupancy[k]);
            let mut reason = "pipeline busy";
            let mut duration = 1;
            if k == core.execute {
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
            if k == core.memory && matches!(op.kind, Kind::Load | Kind::Store) {
                let address =
                    core.base_addr + 0x40 * iteration + 0x10 * (slot % core.ops.len()) as u64;
                if op.kind == Kind::Load {
                    if rng.chance(3) {
                        let level = if rng.chance(3) {
                            Level::Dram
                        } else {
                            Level::L2
                        };
                        duration += if level == Level::Dram { 14 } else { 4 };
                        access = Some(Access {
                            write: false,
                            begin: start,
                            end: start + duration,
                            address,
                            level,
                        });
                    }
                } else {
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
            let ready = if op.kind == Kind::Load {
                stages[core.memory].1
            } else {
                stages[core.execute].1
            };
            writers.insert(dst, (index, ready));
        }
        let end = stages.last().unwrap().1;
        insns.push(Insn {
            index,
            pc,
            op,
            stages,
            stalls,
            end,
            status: TxStatus::Ok,
            deps,
            access,
        });
        // Fetch waits while the instruction ahead still holds the first stage.
        fetch = insns[index].stages[0].0 + 1;
        // A mispredicted branch resolves in execute and flushes the younger
        // fall-through instructions fetched until then.
        if let Some((resolve, first)) = flush_at.filter(|(resolve, _)| fetch >= *resolve) {
            // Fetches that start after the branch resolved are refetched.
            while insns.len() > first && insns.last().unwrap().stages[0].0 >= resolve {
                insns.pop();
            }
            for victim in &mut insns[first..] {
                victim.stages.retain(|(b, _)| *b < resolve);
                victim
                    .stages
                    .iter_mut()
                    .for_each(|s| s.1 = s.1.min(resolve));
                victim.stalls.retain(|(b, ..)| *b < resolve);
                victim
                    .stalls
                    .iter_mut()
                    .for_each(|s| s.1 = s.1.min(resolve));
                victim.end = resolve;
                victim.status = TxStatus::Aborted;
                victim.access = None;
                victim.deps.clear();
            }
            flushes.push(resolve);
            fetch = resolve + 1;
            occupancy.iter_mut().for_each(|o| *o = (*o).min(fetch));
            writers.retain(|_, (producer, _)| *producer < first);
            flush_at = None;
            slot -= slot % core.ops.len(); // the branch was taken: loop head
            continue;
        }
        if op.kind == Kind::Branch {
            branches += 1;
            iteration += 1;
            if branches % core.mispredict_every == 0 {
                flush_at = Some((insns[index].stages[core.execute].1, index + 1));
            }
        }
        slot += 1;
    }
    insns.retain(|i| i.stages[0].0 < CYCLES);
    (insns, flushes)
}

// ---------------------------------------------------------------------------------------------
// Waveforms: every change is collected first, then written in time order.

#[derive(Clone, PartialEq)]
enum V {
    Bit(u8),
    Num(u64),
    /// Logic characters, MSB first (`0 1 x z u w l h -`).
    Logic(String),
    Text(String),
    Real(f64),
    Event,
}

#[derive(Default)]
struct Waves {
    changes: Vec<(u64, SignalId, V)>,
}

impl Waves {
    fn at(&mut self, t: u64, sig: SignalId, v: V) {
        self.changes.push((t, sig, v));
    }
    fn bit(&mut self, t: u64, sig: SignalId, b: u8) {
        self.at(t, sig, V::Bit(b));
    }
    fn num(&mut self, t: u64, sig: SignalId, n: u64) {
        self.at(t, sig, V::Num(n));
    }
    fn logic(&mut self, t: u64, sig: SignalId, s: &str) {
        self.at(t, sig, V::Logic(s.to_owned()));
    }
    fn text(&mut self, t: u64, sig: SignalId, s: &str) {
        self.at(t, sig, V::Text(s.to_owned()));
    }
    fn real(&mut self, t: u64, sig: SignalId, x: f64) {
        self.at(t, sig, V::Real(x));
    }

    /// Writes the changes in time order, dropping repeats of a signal's
    /// current value (events always count). Returns the number written.
    fn write(mut self, w: &mut Writer) -> vtr::Result<usize> {
        self.changes.sort_by_key(|c| c.0);
        let mut current: HashMap<SignalId, V> = HashMap::new();
        let mut written = 0;
        let mut now = None;
        for (t, sig, v) in self.changes {
            if v != V::Event && current.get(&sig) == Some(&v) {
                continue;
            }
            if now != Some(t) {
                w.set_time(t)?;
                now = Some(t);
            }
            match &v {
                V::Bit(b) => w.emit_bit(sig, *b)?,
                V::Num(n) => w.emit_u64(sig, *n)?,
                V::Logic(s) => w.emit_logic_str(sig, s.as_bytes())?,
                V::Text(s) => w.emit_varlen(sig, s.as_bytes())?,
                V::Real(x) => w.emit_real(sig, *x)?,
                V::Event => w.emit_bit(sig, 1)?,
            }
            current.insert(sig, v);
            written += 1;
        }
        Ok(written)
    }
}

fn bits(
    w: &mut Writer,
    parent: NodeId,
    name: &str,
    width: u32,
    states: u8,
    dir: Direction,
) -> vtr::Result<SignalId> {
    let var_type = if states == 2 && width == 1 {
        VarType::Wire
    } else {
        VarType::Logic
    };
    Ok(w.add_var(
        Some(parent),
        name,
        var_type,
        dir,
        SignalKind::Bits { width, states },
    )?
    .1)
}

fn real(w: &mut Writer, parent: NodeId, name: &str) -> vtr::Result<SignalId> {
    Ok(w.add_var(
        Some(parent),
        name,
        VarType::Real,
        Direction::Implicit,
        SignalKind::Real,
    )?
    .1)
}

fn text(w: &mut Writer, parent: NodeId, name: &str) -> vtr::Result<SignalId> {
    Ok(w.add_var(
        Some(parent),
        name,
        VarType::String,
        Direction::Implicit,
        SignalKind::VarLen,
    )?
    .1)
}

fn event(w: &mut Writer, parent: NodeId, name: &str) -> vtr::Result<SignalId> {
    Ok(w.add_var(
        Some(parent),
        name,
        VarType::Event,
        Direction::Implicit,
        SignalKind::Bits {
            width: 1,
            states: 2,
        },
    )?
    .1)
}

/// `width` bits of `value`, MSB first, with the byte `unknown` (0 = LSB) as X.
fn with_x_byte(value: u64, width: usize, unknown: usize) -> String {
    (0..width)
        .rev()
        .map(|i| {
            if i / 8 == unknown {
                'x'
            } else if value >> i & 1 == 1 {
                '1'
            } else {
                '0'
            }
        })
        .collect()
}

/// A DMA descriptor, in cycles.
struct Descriptor {
    begin: u64,
    read: u64,
    write: u64,
    done: u64,
    end: u64,
    src: u64,
    dst: u64,
    beats: u64,
    error: bool,
}

fn descriptors() -> Vec<Descriptor> {
    let mut rng = Rng(0xd0a_c4a1);
    let mut out = Vec::new();
    let mut c = DMA.0;
    let mut n = 0u64;
    while c < DMA.1 - 60 {
        let beats = 6 + rng.next() % 20;
        let read = c + 4;
        let write = read + beats;
        let done = write + beats;
        let error = n == 9;
        let end = done + if error { 6 } else { 2 };
        out.push(Descriptor {
            begin: c,
            read,
            write,
            done,
            end,
            src: 0x0000_0080_4000_0000 + 0x400 * n,
            dst: 0x0000_0080_6000_0000 + 0x400 * n,
            beats,
            error,
        });
        // Back to back at first, then idle gaps that grow.
        c = end
            + if n < 6 {
                1
            } else {
                4 + rng.next() % (8 * (n - 4))
            };
        n += 1;
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path] = args.as_slice() else {
        return Err("usage: volna_theme OUTPUT.vtr".into());
    };
    let path = Path::new(path);
    let (cpu0_insns, cpu0_flushes) = simulate(&CPU0);
    let (cpu1_insns, _) = simulate(&CPU1);
    let dma = descriptors();
    let last = edge(CYCLES);
    let mut rng = Rng(0x5eed_7e3e);

    let mut w = Writer::create_with(
        path,
        WriterOptions {
            background: false,
            ..WriterOptions::default()
        },
    )?;
    w.set_timescale(-12)?;
    w.set_file_type(vtr::FileType::VerilogVhdl)?;
    w.set_writer_name("VTR Volna theme showcase")?;
    w.set_date("2026-09-29")?;
    w.set_comment("Every value kind, rendering case, pipeline and hierarchy shape of docs/volna-theme.html. See examples/README.md.")?;

    // -- hierarchy ------------------------------------------------------------------------
    let soc = w.add_scope(None, "soc", ScopeType::Module, "soc_top")?;
    let clk = bits(&mut w, soc, "clk", 1, 2, Direction::Input)?;
    let reset_n = bits(&mut w, soc, "reset_n", 1, 4, Direction::Input)?;
    let core_clock = w.intern("soc.clk");

    // Static configuration: never changes, so its values are faint.
    let cfg = w.add_scope(Some(soc), "cfg", ScopeType::Module, "soc_config")?;
    let hart_count = bits(&mut w, cfg, "hart_count", 4, 2, Direction::Output)?;
    let isa = text(&mut w, cfg, "isa")?;
    let l2_size_kb = bits(&mut w, cfg, "l2_size_kb", 16, 2, Direction::Output)?;
    let boot_vector = bits(&mut w, cfg, "boot_vector", 64, 2, Direction::Output)?;
    let core_mhz = bits(&mut w, cfg, "core_mhz", 16, 2, Direction::Output)?;

    // cpu0: the five-stage core.
    let cpu0 = w.add_scope(Some(soc), "cpu0", ScopeType::Core, "rv32_core")?;
    let pc0 = bits(&mut w, cpu0, "pc", 32, 2, Direction::Output)?;
    let phase = text(&mut w, cpu0, "phase")?;
    let stall0 = bits(&mut w, cpu0, "stall", 1, 2, Direction::Output)?;
    let flush0 = bits(&mut w, cpu0, "flush", 1, 2, Direction::Output)?;
    let retired0 = bits(&mut w, cpu0, "retired", 16, 2, Direction::Output)?;
    let irq = event(&mut w, cpu0, "irq")?;
    let irq_pending = bits(&mut w, cpu0, "irq_pending", 8, 2, Direction::Input)?;
    let pipe0 = w.add_stream(Some(cpu0), "pipeline", "PIPELINE")?;
    w.node_attr(pipe0, "vtr.clock", Value::Str(core_clock))?;
    let insn0 = w.add_generator(pipe0, "instruction")?;

    // cpu1: the thirteen-stage core, with C910's module names and depth.
    let cpu1 = w.add_scope(Some(soc), "cpu1", ScopeType::Core, "openc910")?;
    let pipe1 = w.add_stream(Some(cpu1), "pipeline", "PIPELINE")?;
    w.node_attr(pipe1, "vtr.clock", Value::Str(core_clock))?;
    let insn1 = w.add_generator(pipe1, "instruction")?;
    let top = w.add_scope(Some(cpu1), "x_cpu_top", ScopeType::Module, "openC910")?;
    let ct_top = w.add_scope(Some(top), "x_ct_top_0", ScopeType::Module, "ct_top")?;
    let ct_core = w.add_scope(Some(ct_top), "x_ct_core", ScopeType::Module, "ct_core")?;
    let ifu = w.add_scope(
        Some(ct_core),
        "x_ct_ifu_top",
        ScopeType::Module,
        "ct_ifu_top",
    )?;
    let ibdp = w.add_scope(Some(ifu), "x_ct_ifu_ibdp", ScopeType::Module, "ct_ifu_ibdp")?;
    let ibuf = w.add_scope(
        Some(ibdp),
        "x_ct_ifu_ibuf",
        ScopeType::Module,
        "ct_ifu_ibuf",
    )?;
    let inst0 = bits(
        &mut w,
        ibuf,
        "ifu_idu_ib_inst0_data",
        32,
        2,
        Direction::Output,
    )?;
    let inst0_vld = bits(
        &mut w,
        ibuf,
        "ifu_idu_ib_inst0_vld",
        1,
        2,
        Direction::Output,
    )?;
    let ibuf_num = bits(&mut w, ibuf, "ibuf_entry_num", 5, 2, Direction::Output)?;
    // Down to level 14: soc cpu1 x_cpu_top x_ct_top_0 x_ct_core x_ct_ifu_top
    // x_ct_ifu_ibdp x_ct_ifu_ibuf, then six more.
    let mut deep = ibuf;
    for name in [
        "x_ibuf_bank0",
        "x_ibuf_entry_ctrl",
        "x_ibuf_entry_data",
        "x_ibuf_ecc",
        "x_ibuf_ecc_chk",
        "x_ibuf_ecc_bit",
    ] {
        deep = w.add_scope(Some(deep), name, ScopeType::Module, &name[2..])?;
    }
    let ecc_err = bits(&mut w, deep, "ecc_err", 1, 2, Direction::Output)?;
    // The decoded instruction is a port on four more paths.
    let idu = w.add_scope(
        Some(ct_core),
        "x_ct_idu_top",
        ScopeType::Module,
        "ct_idu_top",
    )?;
    let id_dp = w.add_scope(
        Some(idu),
        "x_ct_idu_id_dp",
        ScopeType::Module,
        "ct_idu_id_dp",
    )?;
    for parent in [ifu, ct_core, idu, id_dp] {
        w.add_alias(
            Some(parent),
            "ifu_idu_ib_inst0_data",
            VarType::Logic,
            Direction::Input,
            inst0,
        )?;
    }
    let id_pc = bits(&mut w, id_dp, "id_inst0_pc", 40, 2, Direction::Output)?;
    // 64 reorder buffer entries: a run of numbered siblings.
    let rtu = w.add_scope(
        Some(ct_core),
        "x_ct_rtu_top",
        ScopeType::Module,
        "ct_rtu_top",
    )?;
    let rob = w.add_scope(Some(rtu), "x_ct_rtu_rob", ScopeType::Module, "ct_rtu_rob")?;
    let mut rob_entries = Vec::with_capacity(64);
    for i in 0..64 {
        let e = w.add_scope(
            Some(rob),
            &format!("x_ct_rtu_rob_entry{i}"),
            ScopeType::Module,
            "ct_rtu_rob_entry",
        )?;
        let vld = bits(&mut w, e, "vld", 1, 2, Direction::Output)?;
        let iid = bits(&mut w, e, "iid", 7, 2, Direction::Output)?;
        let cmplt = bits(&mut w, e, "cmplt", 1, 2, Direction::Output)?;
        rob_entries.push((vld, iid, cmplt));
    }
    // The vector FPU: 32 signals, none of which moves (CoreMark does no FP).
    let vfpu = w.add_scope(
        Some(ct_core),
        "x_ct_vfpu_top",
        ScopeType::Module,
        "ct_vfpu_top",
    )?;
    let mut vfpu_sigs = Vec::new();
    for pipe in 0..2 {
        let p = w.add_scope(
            Some(vfpu),
            &format!("x_ct_vfpu_dp_pipe{}", pipe + 6),
            ScopeType::Module,
            "ct_vfpu_dp",
        )?;
        for (name, width) in [
            ("ex1_vld", 1),
            ("ex1_op", 8),
            ("ex1_src0", 64),
            ("ex1_src1", 64),
            ("ex2_vld", 1),
            ("ex3_result", 64),
            ("fflags", 5),
            ("frm", 3),
        ] {
            vfpu_sigs.push((bits(&mut w, p, name, width, 2, Direction::Output)?, width));
        }
        for i in 0..16 {
            vfpu_sigs.push((
                bits(&mut w, p, &format!("vreg_wen{i}"), 1, 2, Direction::Output)?,
                1,
            ));
        }
    }

    // L2: requests from cpu0, X address and Z data while idle.
    let l2 = w.add_scope(Some(soc), "l2", ScopeType::Module, "l2_cache")?;
    let req_valid = bits(&mut w, l2, "req_valid", 1, 2, Direction::Input)?;
    let req_write = bits(&mut w, l2, "req_write", 1, 2, Direction::Input)?;
    let req_addr = bits(&mut w, l2, "req_addr", 32, 4, Direction::Input)?;
    let resp_valid = bits(&mut w, l2, "resp_valid", 1, 2, Direction::Output)?;
    let resp_data = bits(&mut w, l2, "resp_data", 32, 4, Direction::Output)?;
    let hit_way = bits(&mut w, l2, "hit_way", 2, 2, Direction::Output)?;
    let mshr = bits(&mut w, l2, "mshr_used", 3, 2, Direction::Output)?;
    let bus = w.add_stream(Some(l2), "bus", "MEMORY_BUS")?;
    w.node_attr(bus, "vtr.clock", Value::Str(core_clock))?;
    let reads = w.add_generator(bus, "read")?;
    let writes = w.add_generator(bus, "write")?;

    // SRAM: all unknown during self test, then reads with an uninitialised line.
    let sram = w.add_scope(Some(soc), "sram", ScopeType::Module, "sram_2p")?;
    let sram_rdata = bits(&mut w, sram, "rdata", 32, 9, Direction::Output)?;
    let sram_bist = bits(&mut w, sram, "bist_busy", 1, 2, Direction::Output)?;

    // DMA: an FSM whose states recur, a beat strobe from quiet to busy, a
    // 64-bit address that counts in its last digits, a queue that drains to 0.
    let dmas = w.add_scope(Some(soc), "dma", ScopeType::Module, "dma_engine")?;
    let dma_state = text(&mut w, dmas, "state")?;
    let dma_beat = bits(&mut w, dmas, "beat", 1, 2, Direction::Output)?;
    let dma_addr = bits(&mut w, dmas, "src_addr", 64, 2, Direction::Output)?;
    let dma_queue = bits(&mut w, dmas, "queue_depth", 4, 2, Direction::Output)?;
    let dma_err = bits(&mut w, dmas, "err", 1, 2, Direction::Output)?;
    let chan = w.add_stream(Some(dmas), "chan0", "DMA")?;
    w.node_attr(chan, "vtr.clock", Value::Str(core_clock))?;
    let desc_gen = w.add_generator(chan, "descriptor")?;

    // I/O pads: nine-state logic with weak pull-ups, contention and don't-care.
    let io = w.add_scope(Some(soc), "io", ScopeType::Module, "pad_ring")?;
    let i2c = w.add_scope(Some(io), "i2c0", ScopeType::Module, "i2c_master")?;
    let scl = bits(&mut w, i2c, "scl", 1, 9, Direction::InOut)?;
    let sda = bits(&mut w, i2c, "sda", 1, 9, Direction::InOut)?;
    let i2c_state = text(&mut w, i2c, "state")?;
    let gpio = bits(&mut w, io, "gpio", 8, 9, Direction::InOut)?;
    let pullups = bits(&mut w, io, "strap", 4, 9, Direction::Input)?;
    let match_mask = bits(&mut w, io, "addr_match", 8, 9, Direction::Input)?;
    let scan_en = bits(&mut w, io, "scan_en", 1, 9, Direction::Input)?;
    let spi = w.add_scope(Some(io), "spi0", ScopeType::Module, "spi_master")?;
    let sck = bits(&mut w, spi, "sck", 1, 2, Direction::Output)?;
    let cs_n = bits(&mut w, spi, "cs_n", 1, 2, Direction::Output)?;
    let mosi = bits(&mut w, spi, "mosi", 1, 4, Direction::Output)?;
    let miso = bits(&mut w, spi, "miso", 1, 4, Direction::Input)?;

    // Always-on domain: a slow clock with a net, a nibble counter, a debug trigger.
    let aon = w.add_scope(Some(soc), "aon", ScopeType::Module, "always_on")?;
    let slow_clk = bits(&mut w, aon, "slow_clk", 1, 2, Direction::Input)?;
    let tick = bits(&mut w, aon, "tick", 4, 2, Direction::Output)?;
    let trigger = event(&mut w, aon, "dbg_trigger")?;

    // DDR controller: its clock is declared only.
    let ddr = w.add_scope(Some(soc), "ddr", ScopeType::Module, "lpddr_ctrl")?;
    let ddr_state = text(&mut w, ddr, "state")?;
    let ddr_cke = bits(&mut w, ddr, "cke", 1, 2, Direction::Output)?;

    let perf = w.add_scope(Some(soc), "perf", ScopeType::Module, "perf_monitor")?;
    let ipc = real(&mut w, perf, "ipc")?;
    let bandwidth = real(&mut w, perf, "l2_bandwidth_gbps")?;
    let temperature = real(&mut w, perf, "temperature_c")?;
    let vdd = real(&mut w, perf, "vdd_core_mv")?;

    let logs = w.add_stream(Some(soc), "log", vtr::LOG_STREAM_KIND)?;
    let site =
        |w: &mut Writer, severity, format: &str, args: &[LogArgType], names: &[&str], line| {
            w.add_log_site(
                &LogSiteSpec::new(logs, severity, format, args)
                    .names(names)
                    .location("tb/theme_tb.sv", line)
                    .func("monitor"),
            )
        };
    let bist_site = site(
        &mut w,
        Severity::Debug,
        "sram bist done after {} cycles",
        &[LogArgType::U64],
        &["cycles"],
        24,
    )?;
    let boot_site = site(
        &mut w,
        Severity::Info,
        "reset released at cycle {}",
        &[LogArgType::U64],
        &["cycle"],
        31,
    )?;
    let irq_site = site(
        &mut w,
        Severity::Info,
        "irq {} taken",
        &[LogArgType::U64],
        &["line"],
        47,
    )?;
    let ecc_site = site(
        &mut w,
        Severity::Warn,
        "l2 read at {} has an uncorrectable byte",
        &[LogArgType::Pointer],
        &["address"],
        62,
    )?;
    let dma_site = site(
        &mut w,
        Severity::Error,
        "dma descriptor {} aborted: bus error at {}",
        &[LogArgType::U64, LogArgType::Pointer],
        &["descriptor", "address"],
        78,
    )?;
    let wfi_site = site(
        &mut w,
        Severity::Info,
        "cpu0 waits for interrupt, ddr enters self-refresh",
        &[],
        &[],
        90,
    )?;
    let hang_site = site(
        &mut w,
        Severity::Fatal,
        "watchdog would fire in {} cycles",
        &[LogArgType::U64],
        &["cycles"],
        97,
    )?;

    // Declared clocks: the dumped core clock, the DDR clock with no net
    // (600 ps, 400 ps from the interrupt, gated while cpu0 waits) and the
    // slow always-on clock.
    let core_clk = w.add_clock(Some(soc), "clk")?;
    w.clock_run(core_clk, FIRST_EDGE, PERIOD)?;
    let ddr_clk = w.add_clock(Some(ddr), "ck")?;
    w.clock_run(ddr_clk, 0, 600)?;
    w.clock_stop(ddr_clk, edge(IRQ_CYCLE) - 1)?;
    w.clock_run(ddr_clk, edge(IRQ_CYCLE), 400)?;
    w.clock_stop(ddr_clk, edge(WFI.0))?;
    w.clock_run(ddr_clk, edge(WFI.1), 400)?;
    let aon_clk = w.add_clock(Some(aon), "slow_clk")?;
    w.clock_run(aon_clk, 0, 64 * PERIOD)?;

    // -- waveforms ------------------------------------------------------------------------
    let mut v = Waves::default();
    for t in (0..=last).step_by(PERIOD as usize / 2) {
        v.bit(
            t,
            clk,
            u8::from(t >= FIRST_EDGE && (t - FIRST_EDGE) % PERIOD == 0),
        );
    }
    for t in (0..=last).step_by(32 * PERIOD as usize) {
        v.bit(t, slow_clk, u8::from(t / (32 * PERIOD) % 2 == 0));
    }
    v.logic(0, reset_n, "x");
    v.logic(edge(1), reset_n, "0");
    v.logic(edge(RESET_DONE), reset_n, "1");

    v.num(0, hart_count, 2);
    v.text(0, isa, "rv64gc_zba_zbb");
    v.num(0, l2_size_kb, 1024);
    v.num(0, boot_vector, 0x0000_0000_8000_0000);
    v.num(0, core_mhz, 1000);
    for &(sig, width) in &vfpu_sigs {
        v.num(0, sig, if width == 3 { 0b001 } else { 0 });
    }

    // cpu0 and the L2 port.
    let cpu0_accesses: Vec<&Access> = cpu0_insns
        .iter()
        .filter_map(|i| i.access.as_ref())
        .collect();
    v.num(0, pc0, 0x8000_0000);
    v.text(0, phase, "reset");
    v.num(0, retired0, 0);
    v.num(0, irq_pending, 0);
    v.logic(0, req_addr, "x");
    v.logic(0, resp_data, "z");
    v.num(0, mshr, 0);
    v.num(0, hit_way, 0);
    let mut done = 0u64;
    let mut retire_at: Vec<u64> = cpu0_insns
        .iter()
        .filter(|i| i.status == TxStatus::Ok)
        .map(|i| i.end)
        .collect();
    retire_at.sort_unstable();
    let mut next_retire = 0;
    for c in 0..=CYCLES {
        let t = edge(c);
        let phase_text = if c < RESET_DONE + 2 {
            "reset"
        } else if (IRQ_CYCLE..IRQ_CYCLE + 40).contains(&c) {
            "irq handler"
        } else if (WFI.0..WFI.1).contains(&c) {
            "idle (wfi)"
        } else if (DMA.0..DMA.1).contains(&c) {
            "dma copy"
        } else {
            "dot product"
        };
        v.text(t, phase, phase_text);
        while next_retire < retire_at.len() && retire_at[next_retire] <= c {
            done += 1;
            next_retire += 1;
        }
        v.num(t, retired0, done);
        v.bit(
            t,
            stall0,
            u8::from(
                cpu0_insns
                    .iter()
                    .any(|i| i.stalls.iter().any(|(b, e, _)| *b <= c && c < *e)),
            ),
        );
        v.bit(t, flush0, u8::from(cpu0_flushes.contains(&c)));
        let request = cpu0_accesses.iter().find(|a| a.begin == c);
        v.bit(t, req_valid, u8::from(request.is_some()));
        v.bit(t, req_write, u8::from(request.is_some_and(|a| a.write)));
        match request {
            Some(a) => {
                v.num(t, req_addr, a.address & 0xffff_ffff);
                v.num(t, hit_way, (a.address >> 4) & 3);
            }
            None => v.logic(t, req_addr, "x"),
        }
        let response = cpu0_accesses.iter().find(|a| !a.write && a.end == c + 1);
        v.bit(t, resp_valid, u8::from(response.is_some()));
        match response {
            Some(a) => {
                let data = (a.address.wrapping_mul(0x9e37_79b9) >> 7) & 0xffff_ffff;
                if (FAULT.0..FAULT.1).contains(&c) {
                    v.logic(t, resp_data, &with_x_byte(data, 32, 2));
                } else {
                    v.num(t, resp_data, data);
                }
            }
            None => v.logic(t, resp_data, "z"),
        }
        let outstanding = cpu0_accesses
            .iter()
            .filter(|a| !a.write && a.begin <= c && c < a.end)
            .count() as u64;
        v.num(t, mshr, outstanding.min(7));
    }
    for i in &cpu0_insns {
        v.num(edge(i.stages[0].0), pc0, i.pc);
    }
    // The interrupt: three edges within 300 ps coalesce, and its line stays
    // pending until the handler clears it.
    for k in 0..3 {
        v.at(edge(IRQ_CYCLE) + 100 * k, irq, V::Event);
    }
    v.num(edge(IRQ_CYCLE), irq_pending, 0x08);
    v.num(edge(IRQ_CYCLE + 30), irq_pending, 0);
    v.at(edge(WFI.1 - 1), irq, V::Event);
    v.num(edge(WFI.1 - 1), irq_pending, 0x20);
    v.num(edge(WFI.1 + 12), irq_pending, 0);

    // cpu1: the instruction buffer, the decoder and the reorder buffer.
    v.num(0, inst0, 0);
    v.bit(0, inst0_vld, 0);
    v.num(0, ibuf_num, 0);
    v.bit(0, ecc_err, 0);
    v.num(0, id_pc, 0);
    for &(vld, iid, cmplt) in &rob_entries {
        v.bit(0, vld, 0);
        v.num(0, iid, 0);
        v.bit(0, cmplt, 0);
    }
    for c in 0..=CYCLES {
        let t = edge(c);
        let decoding = cpu1_insns.iter().find(|i| i.stages[0].0 == c);
        v.bit(t, inst0_vld, u8::from(decoding.is_some()));
        if let Some(i) = decoding {
            v.num(
                t,
                inst0,
                (i.pc.wrapping_mul(0x2545_f491) >> 3) & 0xffff_ffff | 0x3,
            );
            v.num(t, id_pc, i.pc);
        }
        let queued = cpu1_insns
            .iter()
            .filter(|i| i.stages[0].0 <= c && c < i.stages.get(3).map_or(i.end, |s| s.0))
            .count();
        v.num(t, ibuf_num, queued.min(31) as u64);
    }
    v.bit(edge(1777), ecc_err, 1);
    v.bit(edge(1778), ecc_err, 0);
    for i in &cpu1_insns {
        let (vld, iid, cmplt) = rob_entries[i.index % 64];
        v.bit(edge(i.stages[0].0), vld, 1);
        v.num(edge(i.stages[0].0), iid, i.index as u64 & 0x7f);
        if i.end <= CYCLES {
            if let Some(&(wb, _)) = i.stages.get(12) {
                v.bit(edge(wb), cmplt, 1);
            }
            v.bit(edge(i.end), vld, 0);
            v.bit(edge(i.end), cmplt, 0);
        }
    }

    // SRAM: self test reads only unknowns (uninitialised u, conflicting x),
    // then real data, with an uninitialised line read now and then.
    v.logic(0, sram_rdata, "u");
    v.bit(0, sram_bist, 0);
    v.bit(edge(BIST.0), sram_bist, 1);
    for c in BIST.0..BIST.1 {
        v.logic(edge(c), sram_rdata, if c % 2 == 0 { "x" } else { "u" });
    }
    v.bit(edge(BIST.1), sram_bist, 0);
    for c in BIST.1..CYCLES {
        if c % 3 == 0 {
            let value = rng.next() & 0xffff_ffff;
            if rng.chance(9) {
                v.logic(
                    edge(c),
                    sram_rdata,
                    &with_x_byte(value, 32, (c % 4) as usize),
                );
            } else {
                v.num(edge(c), sram_rdata, value);
            }
        }
    }

    // DMA descriptors.
    v.text(0, dma_state, "IDLE");
    v.bit(0, dma_beat, 0);
    v.num(0, dma_addr, 0);
    v.num(0, dma_queue, 0);
    v.bit(0, dma_err, 0);
    v.num(edge(DMA.0 - 2), dma_queue, dma.len().min(15) as u64);
    for (n, d) in dma.iter().enumerate() {
        v.text(edge(d.begin), dma_state, "DESC");
        v.text(edge(d.read), dma_state, "READ");
        v.text(edge(d.write), dma_state, "WRITE");
        v.text(
            edge(d.done),
            dma_state,
            if d.error { "ERROR" } else { "DONE" },
        );
        v.text(edge(d.end), dma_state, "IDLE");
        v.num(edge(d.end), dma_queue, (dma.len() - n - 1).min(15) as u64);
        if d.error {
            v.bit(edge(d.done), dma_err, 1);
            v.bit(edge(d.end), dma_err, 0);
        }
        for b in 0..d.beats {
            v.num(edge(d.read + b), dma_addr, d.src + 0x10 * b);
            v.num(edge(d.write + b), dma_addr, d.dst + 0x10 * b);
            v.bit(edge(d.read + b), dma_beat, (b % 2 == 0) as u8);
            v.bit(edge(d.write + b), dma_beat, (b % 2 == 0) as u8);
        }
        v.bit(edge(d.done), dma_beat, 0);
        v.num(edge(d.end), dma_addr, 0);
    }
    // A trickle after the copies: one beat every 24 cycles, then silence.
    for c in (2600..WFI.0).step_by(24) {
        v.bit(edge(c), dma_beat, 1);
        v.bit(edge(c + 1), dma_beat, 0);
    }

    // I/O. The I2C bus is open drain: released lines are weak highs.
    v.logic(0, scl, "u");
    v.logic(0, sda, "u");
    v.text(0, i2c_state, "IDLE");
    v.logic(edge(RESET_DONE), scl, "h");
    v.logic(edge(RESET_DONE), sda, "h");
    for (n, start) in [400u64, 1600, 3500].into_iter().enumerate() {
        // START, 9 bits of 4 cycles each (address byte + ACK), STOP.
        v.text(edge(start), i2c_state, "START");
        v.logic(edge(start), sda, "0");
        let byte = [0xa0u64, 0x3c, 0x51][n];
        for bit in 0..9 {
            let c = start + 2 + 4 * bit;
            v.text(edge(c), i2c_state, if bit < 8 { "ADDR" } else { "ACK" });
            v.logic(edge(c), scl, "0");
            let level = if bit == 8 {
                "0"
            } else if byte >> (7 - bit) & 1 == 1 {
                "h"
            } else {
                "0"
            };
            v.logic(edge(c + 1), sda, level);
            v.logic(edge(c + 2), scl, "h");
        }
        let stop = start + 2 + 4 * 9;
        v.text(edge(stop), i2c_state, "STOP");
        v.logic(edge(stop), scl, "0");
        v.logic(edge(stop), sda, "0");
        v.logic(edge(stop + 1), scl, "h");
        v.logic(edge(stop + 2), sda, "h");
        v.text(edge(stop + 3), i2c_state, "IDLE");
    }
    // A wired-or glitch: both ends drive, the line is contended (w).
    v.logic(edge(1618), sda, "w");
    v.logic(edge(1619), sda, "h");
    v.logic(0, gpio, "uuuuuuuu");
    v.logic(edge(RESET_DONE), gpio, "zzzzzzzz");
    v.logic(edge(120), gpio, "zzzzhhhh");
    v.logic(edge(700), gpio, "0000hhhh");
    v.logic(edge(900), gpio, "1010hlhh");
    v.logic(edge(1300), gpio, "10100000");
    v.logic(edge(1900), gpio, "1010zzzz");
    v.logic(edge(2500), gpio, "0x10hlhh");
    v.logic(edge(2560), gpio, "0110hlhh");
    v.logic(edge(WFI.0), gpio, "----hhhh");
    v.logic(edge(WFI.1), gpio, "0110hhhh");
    v.logic(0, pullups, "hhlh");
    v.logic(edge(RESET_DONE), pullups, "1101");
    v.logic(0, match_mask, "--------");
    v.logic(edge(300), match_mask, "1-0-11--");
    v.logic(edge(1500), match_mask, "1-0-1100");
    v.logic(edge(2800), match_mask, "0110----");
    v.logic(0, scan_en, "-");
    v.logic(edge(RESET_DONE), scan_en, "0");
    v.logic(edge(3800), scan_en, "l");
    // SPI: an undeclared clock that only runs during transfers.
    v.bit(0, sck, 0);
    v.bit(0, cs_n, 1);
    v.logic(0, mosi, "z");
    v.logic(0, miso, "z");
    for start in [250u64, 1100, 1180, 2200, 3600] {
        v.bit(edge(start), cs_n, 0);
        for b in 0..16u64 {
            let t = edge(start + 1) + b * PERIOD * 3 / 2;
            v.bit(t, sck, 1);
            v.bit(t + PERIOD * 3 / 4, sck, 0);
            v.logic(t, mosi, if rng.chance(2) { "1" } else { "0" });
            v.logic(
                t,
                miso,
                if b < 8 {
                    "z"
                } else if rng.chance(2) {
                    "1"
                } else {
                    "0"
                },
            );
        }
        let stop = edge(start + 1) + 16 * PERIOD * 3 / 2;
        v.bit(stop, cs_n, 1);
        v.logic(stop, mosi, "z");
        v.logic(stop, miso, "z");
    }

    // Always on: a 4-bit tick counter wraps every 16 slow cycles, one digit
    // per segment; the debug trigger fires in bursts of up to five.
    for c in (0..CYCLES).step_by(64) {
        v.num(edge(c), tick, (c / 64) % 16);
    }
    for (c, burst) in [(500u64, 1u64), (1450, 2), (2048, 5), (2650, 3), (3900, 4)] {
        for k in 0..burst {
            v.at(edge(c) + 150 * k, trigger, V::Event);
        }
    }

    // DDR controller.
    v.text(0, ddr_state, "INIT");
    v.bit(0, ddr_cke, 0);
    v.bit(edge(40), ddr_cke, 1);
    v.text(edge(40), ddr_state, "IDLE");
    for a in cpu0_accesses
        .iter()
        .filter(|a| a.level == Level::Dram && a.end <= CYCLES)
    {
        v.text(edge(a.begin + 2), ddr_state, "ACT");
        v.text(edge(a.begin + 6), ddr_state, "RD");
        v.text(edge(a.end - 6), ddr_state, "BURST");
        v.text(edge(a.end - 2), ddr_state, "IDLE");
    }
    v.text(edge(WFI.0), ddr_state, "SREF");
    v.bit(edge(WFI.0), ddr_cke, 0);
    v.text(edge(WFI.1), ddr_state, "IDLE");
    v.bit(edge(WFI.1), ddr_cke, 1);

    // Performance: sampled every 8 cycles, with a one-sample supply droop
    // at the interrupt that must survive every zoom.
    let mut temp = 41.0f64;
    for c in (0..=CYCLES).step_by(8) {
        let t = edge(c);
        let window = c.saturating_sub(64)..c;
        let recent = cpu0_insns
            .iter()
            .filter(|i| i.status == TxStatus::Ok && window.contains(&i.end))
            .count()
            + cpu1_insns
                .iter()
                .filter(|i| i.status == TxStatus::Ok && window.contains(&i.end))
                .count();
        let lines = cpu0_accesses
            .iter()
            .filter(|a| window.contains(&a.begin))
            .count()
            + dma.iter().filter(|d| window.contains(&d.read)).count() * 4;
        let ipc_now = recent as f64 / 64.0;
        v.real(t, ipc, (ipc_now * 1000.0).round() / 1000.0);
        v.real(
            t,
            bandwidth,
            ((lines as f64 * 64.0 / 64.0) * 100.0).round() / 100.0,
        );
        let target = 42.0 + 18.0 * ipc_now + 0.6 * lines as f64;
        temp += (target - temp) * 0.05;
        v.real(t, temperature, (temp * 100.0).round() / 100.0);
        let mv = if c == IRQ_CYCLE {
            688.0
        } else {
            750.0 - 6.0 * ipc_now + 3.0 * (rng.unit() - 0.5)
        };
        v.real(t, vdd, (mv * 10.0).round() / 10.0);
    }
    let changes = v.write(&mut w)?;

    // -- transactions ----------------------------------------------------------------------
    let k_label = w.intern("vtr.label");
    let k_pc = w.intern("pc");
    let k_index = w.intern("insn_id");
    let k_detail = w.intern("detail");
    let k_reason = w.intern("reason");
    let k_addr = w.intern("address");
    let k_bytes = w.intern("bytes");
    let k_level = w.intern("served_by");
    let k_core = w.intern("core");
    let k_src = w.intern("src");
    let k_dst = w.intern("dst");
    let k_beats = w.intern("beats");
    let k_wakeup = w.intern("wakeup");
    let k_causes = w.intern("causes");
    let k_next = w.intern("next");
    let lane0 = w.intern("0");
    let lane_stall = w.intern("stall");
    let stall_name = w.intern("stl");
    let s_addr = w.intern("addr");
    let s_data = w.intern("data");
    let s_desc = w.intern("desc");
    let s_read = w.intern("read");
    let s_write = w.intern("write");
    let served_l2 = w.intern("L2");
    let served_dram = w.intern("DRAM");
    let squashed = w.intern("squashed by a mispredicted branch");
    let reasons: HashMap<&str, StrId> = ["operand", "pipeline busy"]
        .into_iter()
        .map(|r| (r, w.intern(r)))
        .collect();

    w.log(bist_site, edge(BIST.1), &[LogArg::U64(BIST.1 - BIST.0)])?;
    w.log(boot_site, edge(RESET_DONE), &[LogArg::U64(RESET_DONE)])?;
    let mut open = 0;
    for (core, insns, generator, name) in [
        (&CPU0, &cpu0_insns, insn0, "cpu0"),
        (&CPU1, &cpu1_insns, insn1, "cpu1"),
    ] {
        let stage_names: Vec<StrId> = core.stages.iter().map(|s| w.intern(s)).collect();
        let core_name = w.intern(name);
        let mut ids: Vec<TxId> = Vec::with_capacity(insns.len());
        let mut logged_ecc = false;
        let first_after_irq = insns
            .iter()
            .find(|i| i.stages[0].0 >= IRQ_CYCLE)
            .map(|i| i.index);
        for insn in insns {
            let tx = w.begin_tx(generator, edge(insn.stages[0].0))?;
            ids.push(tx);
            let label = w.intern(&format!("{:08x}: {}", insn.pc, insn.op.text));
            w.tx_attr(tx, k_label, &Value::Str(label))?;
            w.tx_attr(tx, k_index, &Value::U64(insn.index as u64))?;
            w.tx_attr(tx, k_pc, &Value::U64(insn.pc))?;
            let in_flight = insn.end > CYCLES;
            for (k, &(b, e)) in insn.stages.iter().enumerate() {
                if b >= CYCLES {
                    break;
                }
                if e > CYCLES {
                    w.tx_stage_begin(tx, stage_names[k], lane0, edge(b))?;
                } else {
                    w.tx_stage(tx, stage_names[k], lane0, edge(b), edge(e), &[])?;
                }
            }
            for &(b, e, reason) in insn.stalls.iter().filter(|s| s.1 <= CYCLES) {
                w.tx_stage(
                    tx,
                    stall_name,
                    lane_stall,
                    edge(b),
                    edge(e),
                    &[(k_reason, Value::Str(reasons[reason]))],
                )?;
            }
            for &producer in &insn.deps {
                w.relate(k_wakeup, ids[producer], tx, &[])?;
            }
            if let Some(a) = insn.access.as_ref().filter(|a| a.end <= CYCLES) {
                let request = w.begin_tx(if a.write { writes } else { reads }, edge(a.begin))?;
                w.set_tx_parent(request, tx)?;
                let label = w.intern(&format!(
                    "{} {:#010x}",
                    if a.write { "write" } else { "read" },
                    a.address
                ));
                w.tx_attr(request, k_label, &Value::Str(label))?;
                w.tx_attr(request, k_core, &Value::Str(core_name))?;
                w.tx_attr(request, k_addr, &Value::U64(a.address))?;
                w.tx_attr(request, k_bytes, &Value::U64(if a.write { 8 } else { 64 }))?;
                w.tx_attr(
                    request,
                    k_level,
                    &Value::Str(if a.level == Level::Dram {
                        served_dram
                    } else {
                        served_l2
                    }),
                )?;
                w.tx_stage(
                    request,
                    s_addr,
                    lane0,
                    edge(a.begin),
                    edge(a.begin + 1),
                    &[],
                )?;
                w.tx_stage(request, s_data, lane0, edge(a.end - 1), edge(a.end), &[])?;
                w.relate(k_causes, tx, request, &[])?;
                let faulty =
                    name == "cpu0" && !a.write && (FAULT.0..FAULT.1).contains(&(a.end - 1));
                if faulty && !logged_ecc {
                    w.log_with_parent(
                        ecc_site,
                        edge(a.end - 1),
                        Some(request),
                        &[LogArg::Pointer(a.address)],
                    )?;
                    logged_ecc = true;
                }
                w.end_tx(
                    request,
                    edge(a.end),
                    if faulty {
                        TxStatus::Error
                    } else {
                        TxStatus::Ok
                    },
                )?;
            }
            if name == "cpu0" && Some(insn.index) == first_after_irq {
                w.log_with_parent(irq_site, edge(IRQ_CYCLE), Some(tx), &[LogArg::U64(3)])?;
            }
            if insn.status == TxStatus::Aborted {
                w.tx_attr(tx, k_detail, &Value::Str(squashed))?;
            }
            if in_flight {
                open += 1;
            } else {
                w.end_tx(tx, edge(insn.end), insn.status)?;
            }
        }
    }
    let mut previous: Option<TxId> = None;
    for (n, d) in dma.iter().enumerate() {
        let tx = w.begin_tx(desc_gen, edge(d.begin))?;
        let label = w.intern(&format!("copy {} × 16 B", d.beats));
        w.tx_attr(tx, k_label, &Value::Str(label))?;
        w.tx_attr(tx, k_src, &Value::U64(d.src))?;
        w.tx_attr(tx, k_dst, &Value::U64(d.dst))?;
        w.tx_attr(tx, k_beats, &Value::U64(d.beats))?;
        w.tx_stage(tx, s_desc, lane0, edge(d.begin), edge(d.read), &[])?;
        w.tx_stage(tx, s_read, lane0, edge(d.read), edge(d.write), &[])?;
        w.tx_stage(tx, s_write, lane0, edge(d.write), edge(d.done), &[])?;
        if let Some(p) = previous {
            w.relate(k_next, p, tx, &[])?;
        }
        if d.error {
            w.log_with_parent(
                dma_site,
                edge(d.done),
                Some(tx),
                &[LogArg::U64(n as u64), LogArg::Pointer(d.dst)],
            )?;
        }
        w.end_tx(
            tx,
            edge(d.end),
            if d.error {
                TxStatus::Error
            } else {
                TxStatus::Ok
            },
        )?;
        previous = Some(tx);
    }
    w.log(wfi_site, edge(WFI.0), &[])?;
    w.log(hang_site, edge(CYCLES - 16), &[LogArg::U64(4096)])?;
    w.set_time(last)?;
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
    r.visit_transactions(&TxQuery::default(), |tx| {
        *per_generator
            .entry(r.name(tx.generator).to_owned())
            .or_default() += 1;
        *statuses.entry(tx.status).or_default() += 1;
        true
    })?;
    let mut log_count = 0;
    r.visit_log(&LogQuery::default(), |_| {
        log_count += 1;
        true
    })?;
    let status = |s| statuses.get(&s).copied().unwrap_or(0);
    assert!(status(TxStatus::Aborted) > 20, "flushed instructions");
    assert!(status(TxStatus::Error) >= 2, "a DMA error and an ECC error");
    assert!(
        status(TxStatus::Open) > open,
        "in-flight instructions and running clocks"
    );
    assert!(n_rel > 1000);
    assert_eq!(log_count, 7);
    assert_eq!(r.clocks().len(), 3);
    assert_eq!(r.time_range().unwrap().1, last);
    let mut names: Vec<_> = per_generator.into_iter().collect();
    names.sort();
    println!(
        "{}: {size} bytes, 0..{} ps, {changes} value changes, {n_tx} transactions ({open} instructions in flight), {n_rel} relations, {log_count} logs",
        path.display(),
        last,
    );
    for (name, count) in names {
        println!("  {name}: {count}");
    }
    Ok(())
}
