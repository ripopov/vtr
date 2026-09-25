//! Reproducible pipeline showcase for the Volna pipeline panel: two cores
//! running the same loop, each recorded as a PIPELINE stream of instruction
//! transactions with stage cells, stall overlays, flushes, dependencies and
//! memory requests, plus a few waveforms on the same cycle time base.
//! No VDB presentation data.
//!
//! cargo run -p vtr --example pipeline_showcase -- volna/volna/examples/pipeline_showcase.vtr

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use vtr::{
    Direction, Reader, ScopeType, SignalId, SignalKind, StrId, TxId, TxQuery, TxStatus,
    Value, VarType, Writer, WriterOptions,
};

/// The file must stay a small, checked-in example.
const MAX_BYTES: u64 = 250_000;

/// A tiny RISC-V-like loop body. `dst`/`src` are architectural registers, so
/// the simulator can derive read-after-write dependencies.
struct Op {
    text: &'static str,
    dst: Option<u8>,
    src: [Option<u8>; 2],
    kind: OpKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Alu,
    Load,
    Store,
    Branch,
}

const PROGRAM: &[Op] = &[
    Op { text: "lw   a1, 0(a2)", dst: Some(11), src: [Some(12), None], kind: OpKind::Load },
    Op { text: "addi a0, a0, 1", dst: Some(10), src: [Some(10), None], kind: OpKind::Alu },
    Op { text: "add  a3, a1, a0", dst: Some(13), src: [Some(11), Some(10)], kind: OpKind::Alu },
    Op { text: "xor  a5, a3, a4", dst: Some(15), src: [Some(13), Some(14)], kind: OpKind::Alu },
    Op { text: "sw   a3, 4(a2)", dst: None, src: [Some(13), Some(12)], kind: OpKind::Store },
    Op { text: "lw   a6, 8(a2)", dst: Some(16), src: [Some(12), None], kind: OpKind::Load },
    Op { text: "slli a7, a6, 2", dst: Some(17), src: [Some(16), None], kind: OpKind::Alu },
    Op { text: "or   a5, a5, a7", dst: Some(15), src: [Some(15), Some(17)], kind: OpKind::Alu },
    Op { text: "addi a2, a2, 16", dst: Some(12), src: [Some(12), None], kind: OpKind::Alu },
    Op { text: "sub  t0, a4, a0", dst: Some(5), src: [Some(14), Some(10)], kind: OpKind::Alu },
    Op { text: "sw   a5, -4(a2)", dst: None, src: [Some(15), Some(12)], kind: OpKind::Store },
    Op { text: "bne  a0, a4, loop", dst: None, src: [Some(10), Some(14)], kind: OpKind::Branch },
];

/// How one core is recorded.
struct CoreConfig {
    name: &'static str,
    /// Stage names on the primary lane, in order. `execute` and `memory`
    /// index into it.
    stages: &'static [&'static str],
    execute: usize,
    memory: usize,
    /// Instructions that may share one cycle in every stage.
    width: usize,
    /// Extra cycles a load spends in its memory stage on a cache miss.
    miss_penalty: u64,
    /// Every n-th branch is mispredicted and flushes the younger instructions.
    mispredict_every: usize,
    /// Cycles between a mispredict and the corrected fetch.
    redirect_penalty: u64,
    seed: u64,
    instructions: usize,
    /// The recording stops this many instructions before the last simulated
    /// one is fetched, so instructions still in flight stay open.
    in_flight: usize,
}

const CORES: [CoreConfig; 2] = [
    CoreConfig {
        name: "cpu0",
        stages: &["F", "D", "X", "M", "W"],
        execute: 2,
        memory: 3,
        width: 1,
        miss_penalty: 9,
        mispredict_every: 5,
        redirect_penalty: 1,
        seed: 0x5eed_0001,
        instructions: 720,
        in_flight: 6,
    },
    CoreConfig {
        name: "cpu1",
        stages: &["F", "Dc", "Rn", "Ds", "Is", "Rr", "X", "Cm"],
        execute: 6,
        memory: 6,
        width: 2,
        miss_penalty: 14,
        mispredict_every: 7,
        redirect_penalty: 2,
        seed: 0x5eed_0002,
        instructions: 960,
        in_flight: 10,
    },
];

/// One simulated instruction: stage intervals on the primary lane, stall
/// overlays, its outcome and its dependencies.
struct Insn {
    index: usize,
    pc: u64,
    op: usize,
    /// (begin, end) per stage, in `stages` order; flushed instructions have
    /// fewer entries.
    stages: Vec<(u64, u64)>,
    /// (begin, end) of stall overlays.
    stalls: Vec<(u64, u64)>,
    end: u64,
    status: TxStatus,
    /// Producers this instruction waited for (register RAW).
    deps: Vec<usize>,
    /// A memory request: (begin, end, is_write).
    request: Option<(u64, u64, bool)>,
}

/// A small deterministic generator (xorshift).
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

/// In-order pipeline with `width` instructions per stage and cycle, load
/// misses that stall younger instructions, and periodic branch flushes.
fn simulate(cfg: &CoreConfig) -> Vec<Insn> {
    let mut rng = Rng(cfg.seed);
    let n_stages = cfg.stages.len();
    // Cycles at which the last `width` occupants left each stage.
    let mut occupancy: Vec<VecDeque<u64>> = (0..n_stages)
        .map(|_| VecDeque::from(vec![0; cfg.width]))
        .collect();
    // The last instruction that wrote each register and its result cycle.
    let mut writers: HashMap<u8, (usize, u64)> = HashMap::new();
    let mut insns: Vec<Insn> = Vec::new();
    let mut fetch = 1u64;
    let mut fetched_this_cycle = 0usize;
    let mut pc_index = 0usize;
    let mut branches = 0usize;
    // Younger instructions that a resolved mispredict discards: they were
    // fetched from the fall-through path and are flushed at the resolve cycle.
    let mut flush_at: Option<(u64, usize)> = None;
    while insns.len() < cfg.instructions {
        let index = insns.len();
        let op_ix = pc_index % PROGRAM.len();
        let op = &PROGRAM[op_ix];
        let pc = 0x8000_0000 + 4 * pc_index as u64;
        if fetched_this_cycle == cfg.width {
            fetch += 1;
            fetched_this_cycle = 0;
        }
        fetched_this_cycle += 1;
        let mut stages: Vec<(u64, u64)> = Vec::with_capacity(n_stages);
        let mut stalls = Vec::new();
        let mut deps = Vec::new();
        let mut begin = fetch;
        for k in 0..n_stages {
            // The stage frees when its oldest occupant left it.
            let structural = occupancy[k].front().copied().unwrap_or(0);
            let mut start = begin.max(structural);
            let mut duration = 1;
            if k == cfg.execute {
                // Operands: wait for producers' results (a load's memory stage).
                for reg in op.src.iter().flatten() {
                    if let Some(&(producer, ready)) = writers.get(reg) {
                        deps.push(producer);
                        start = start.max(ready);
                    }
                }
            }
            if k == cfg.memory && op.kind == OpKind::Load && rng.chance(4) {
                duration += cfg.miss_penalty;
            }
            if k > 0 && start > begin {
                // Waiting to enter stage k, the instruction stays in stage
                // k-1 (Konata draws the cell extended under a stall overlay).
                stages[k - 1].1 = start;
                *occupancy[k - 1].back_mut().unwrap() = start;
                stalls.push((begin, start));
            }
            let end = start + duration;
            stages.push((start, end));
            occupancy[k].pop_front();
            occupancy[k].push_back(end);
            begin = end;
        }
        deps.sort_unstable();
        deps.dedup();
        let result_ready = stages[cfg.memory].1;
        if let Some(dst) = op.dst {
            writers.insert(dst, (index, result_ready));
        }
        let request = match op.kind {
            OpKind::Load => Some((stages[cfg.memory].0, stages[cfg.memory].1, false)),
            OpKind::Store => Some((stages[cfg.memory].1, stages[cfg.memory].1 + 3, true)),
            _ => None,
        };
        let end = stages.last().unwrap().1;
        insns.push(Insn {
            index,
            pc,
            op: op_ix,
            stages,
            stalls,
            end,
            status: TxStatus::Unset,
            deps,
            request,
        });
        // A mispredicted branch resolves in execute; the younger fall-through
        // instructions already fetched are flushed then, and fetch restarts.
        if let Some((resolve, first_victim)) =
            flush_at.filter(|(_, first_victim)| index + 1 >= first_victim + cfg.width * 2)
        {
            for victim in &mut insns[first_victim..] {
                victim.stages.retain(|(b, _)| *b < resolve);
                for stage in &mut victim.stages {
                    stage.1 = stage.1.min(resolve);
                }
                victim.stalls.retain(|(b, _)| *b < resolve);
                for stall in &mut victim.stalls {
                    stall.1 = stall.1.min(resolve);
                }
                victim.end = resolve;
                victim.status = TxStatus::Aborted;
                victim.request = None;
                victim.deps.clear();
            }
            fetch = resolve + cfg.redirect_penalty;
            fetched_this_cycle = 0;
            for stage in &mut occupancy {
                for slot in stage.iter_mut() {
                    *slot = (*slot).min(fetch);
                }
            }
            flush_at = None;
            // Victims never wrote their registers.
            writers.retain(|_, (producer, _)| *producer < first_victim);
            pc_index += 1; // the loop branch was taken: continue at the loop head
            pc_index -= pc_index % PROGRAM.len();
            continue;
        }
        if op.kind == OpKind::Branch {
            branches += 1;
            if branches % cfg.mispredict_every == 0 && flush_at.is_none() {
                let resolve = insns[index].stages[cfg.execute].1;
                flush_at = Some((resolve, index + 1));
            }
        }
        pc_index += 1;
    }
    insns
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: pipeline_showcase OUTPUT.vtr");
    let path = Path::new(&path);
    let mut w = Writer::create_with(
        path,
        WriterOptions {
            background: false,
            ..WriterOptions::default()
        },
    )?;
    w.set_timescale(0)?;
    w.set_file_type(vtr::FileType::Architectural)?;
    let unit = w.intern("cycle");
    w.set_file_attr("time.unit", Value::Str(unit))?;
    w.set_file_attr("seed", Value::U64(CORES[0].seed))?;
    let comment = w.intern("Volna pipeline showcase: two synthetic cores, one loop");
    w.set_file_attr("comment", Value::Str(comment))?;

    let k_label = w.intern("vtr.label");
    let k_detail = w.intern("detail");
    let k_index = w.intern("insn_id");
    let k_pc = w.intern("pc");
    let k_iteration = w.intern("iteration");
    let k_wakeup = w.intern("wakeup");
    let k_causes = w.intern("causes");
    let k_addr = w.intern("address");
    let k_miss = w.intern("miss");
    let k_reason = w.intern("reason");
    let lane0 = w.intern("0");
    let lane_stall = w.intern("stall");
    let stall_name = w.intern("stl");
    let reason_miss = w.intern("cache miss");
    let reason_dep = w.intern("operand");

    let soc = w.add_scope(None, "soc", ScopeType::Module, "soc_top")?;
    struct CoreNodes {
        generator: vtr::NodeId,
        pc: SignalId,
        fetch_valid: SignalId,
        stall: SignalId,
        flush: SignalId,
        retired: SignalId,
    }
    let mut nodes = Vec::new();
    for cfg in &CORES {
        let core = w.add_scope(Some(soc), cfg.name, ScopeType::Core, "rv32_core")?;
        let pc = w
            .add_var(
                Some(core),
                "pc",
                VarType::Logic,
                Direction::Implicit,
                SignalKind::Bits {
                    width: 32,
                    states: 2,
                },
            )?
            .1;
        let bit = |w: &mut Writer, name: &str| {
            let kind = SignalKind::Bits { width: 1, states: 2 };
            w.add_var(Some(core), name, VarType::Logic, Direction::Implicit, kind).map(|(_, s)| s)
        };
        let fetch_valid = bit(&mut w, "fetch_valid")?;
        let stall = bit(&mut w, "stall")?;
        let flush = bit(&mut w, "flush")?;
        let retired = w
            .add_var(
                Some(core),
                "retired",
                VarType::Logic,
                Direction::Implicit,
                SignalKind::Bits {
                    width: 16,
                    states: 2,
                },
            )?
            .1;
        let stream = w.add_stream(Some(core), "pipeline", "PIPELINE")?;
        let generator = w.add_generator(stream, "instruction")?;
        nodes.push(CoreNodes {
            generator,
            pc,
            fetch_valid,
            stall,
            flush,
            retired,
        });
    }
    let l2 = w.add_scope(Some(soc), "l2", ScopeType::Module, "l2_cache")?;
    let bus = w.add_stream(Some(l2), "bus", "MEMORY_BUS")?;
    let reads = w.add_generator(bus, "read")?;
    let writes = w.add_generator(bus, "write")?;
    let _ = soc;

    // -- simulate, then waveforms in time order, then transactions -------------
    let cores: Vec<Vec<Insn>> = CORES.iter().map(simulate).collect();
    // The recording stops when the in-flight tail is fetched; every core
    // shares one time range.
    let cutoffs: Vec<u64> = CORES
        .iter()
        .zip(&cores)
        .map(|(cfg, insns)| insns[insns.len() - cfg.in_flight].stages[0].0)
        .collect();
    let last_cycle = *cutoffs.iter().max().unwrap();
    for cycle in 0..=last_cycle {
        w.set_time(cycle)?;
        for (cutoff, (insns, n)) in cutoffs.iter().zip(cores.iter().zip(&nodes)) {
            let fetched = insns
                .iter()
                .find(|i| i.stages.first().is_some_and(|(b, _)| *b == cycle));
            w.emit_bit(n.fetch_valid, u8::from(fetched.is_some()))?;
            if let Some(i) = fetched {
                w.emit_u64(n.pc, i.pc)?;
            } else if cycle == 0 {
                w.emit_u64(n.pc, 0x8000_0000)?;
            }
            let stalled = insns
                .iter()
                .any(|i| i.stalls.iter().any(|(b, e)| *b <= cycle && cycle < *e));
            w.emit_bit(n.stall, u8::from(stalled))?;
            let flushed = insns
                .iter()
                .any(|i| i.status == TxStatus::Aborted && i.end == cycle);
            w.emit_bit(n.flush, u8::from(flushed))?;
            let retired = insns
                .iter()
                .filter(|i| i.status != TxStatus::Aborted && i.end <= cycle && i.end <= *cutoff)
                .count() as u64;
            w.emit_u64(n.retired, retired)?;
        }
    }

    let mut stage_names: HashMap<&str, StrId> = HashMap::new();
    for cfg in &CORES {
        for name in cfg.stages {
            let id = w.intern(name);
            stage_names.insert(name, id);
        }
    }
    let mut totals = (0usize, 0usize, 0usize, 0usize); // instructions, flushed, open, requests
    for ((cfg, cutoff), (insns, n)) in CORES
        .iter()
        .zip(cutoffs.iter().copied())
        .zip(cores.iter().zip(&nodes))
    {
        let mut ids: Vec<TxId> = Vec::with_capacity(insns.len());
        for insn in insns {
            let begin = insn.stages.first().map_or(insn.end, |(b, _)| *b);
            if begin > cutoff {
                // Fetched after the recording stopped: not part of the trace.
                break;
            }
            let in_flight = insn.status != TxStatus::Aborted && insn.end > cutoff;
            let tx = w.begin_tx(n.generator, begin)?;
            ids.push(tx);
            let op = &PROGRAM[insn.op];
            let label = w.intern(&format!("{:08x}: {}", insn.pc, op.text));
            w.tx_attr(tx, k_label, &Value::Str(label))?;
            w.tx_attr(tx, k_index, &Value::U64(insn.index as u64))?;
            w.tx_attr(tx, k_pc, &Value::U64(insn.pc))?;
            let iteration = ((insn.pc - 0x8000_0000) / 4) / PROGRAM.len() as u64;
            w.tx_attr(tx, k_iteration, &Value::U64(iteration))?;
            for (k, (b, e)) in insn.stages.iter().enumerate() {
                if *b > cutoff {
                    break;
                }
                let name = stage_names[cfg.stages[k]];
                let attrs: Vec<(StrId, Value)> = if k == cfg.memory && e - b > 1 {
                    vec![(k_miss, Value::Bool(true))]
                } else {
                    vec![]
                };
                if in_flight && *e > cutoff {
                    // Still in this stage when the recording stopped.
                    w.tx_stage_begin(tx, name, lane0, *b)?;
                    let stage_label = w.intern(&format!("{} for instruction {}", cfg.stages[k], insn.index));
                    w.tx_stage_attr(tx, k_label, &Value::Str(stage_label))?;
                } else {
                    let stage_label = w.intern(&format!("{} for instruction {}", cfg.stages[k], insn.index));
                    let mut attrs = attrs;
                    attrs.push((k_label, Value::Str(stage_label)));
                    w.tx_stage(tx, name, lane0, *b, *e, &attrs)?;
                }
            }
            for (b, e) in &insn.stalls {
                if *b > cutoff {
                    break;
                }
                let reason = if insn.deps.is_empty() {
                    reason_miss
                } else {
                    reason_dep
                };
                let stall_label = w.intern(&format!("stall for instruction {}", insn.index));
                w.tx_stage(
                    tx,
                    stall_name,
                    lane_stall,
                    *b,
                    (*e).min(cutoff),
                    &[(k_reason, Value::Str(reason)), (k_label, Value::Str(stall_label))],
                )?;
            }
            for &producer in &insn.deps {
                if let Some(&producer) = ids.get(producer) {
                    let edge_label = w.intern(&format!("wakeup {producer} to {tx}"));
                    w.relate(k_wakeup, producer, tx, &[(k_label, Value::Str(edge_label))])?;
                }
            }
            if let Some((b, e, is_write)) = insn.request.filter(|(b, ..)| *b <= cutoff) {
                let request = w.begin_tx(if is_write { writes } else { reads }, b)?;
                w.set_tx_parent(request, tx)?;
                let address = 0x1000_0000 + 16 * iteration + if is_write { 4 } else { 0 };
                w.tx_attr(request, k_addr, &Value::U64(address))?;
                let request_label = w.intern(&format!("{} 0x{address:08x}", if is_write { "write" } else { "read" }));
                w.tx_attr(request, k_label, &Value::Str(request_label))?;
                let cause_label = w.intern(&format!("instruction {} causes request", insn.index));
                w.relate(k_causes, tx, request, &[(k_label, Value::Str(cause_label))])?;
                if e <= cutoff {
                    w.end_tx(request, e, TxStatus::Ok)?;
                }
                totals.3 += 1;
            }
            if insn.status == TxStatus::Aborted {
                let detail = w.intern("squashed by a mispredicted branch");
                w.tx_attr(tx, k_detail, &Value::Str(detail))?;
                w.end_tx(tx, insn.end, TxStatus::Aborted)?;
                totals.1 += 1;
            } else if !in_flight {
                w.end_tx(tx, insn.end, TxStatus::Unset)?;
            } else {
                // Still in flight when the recording stops: left open, the
                // writer closes it (status Open) at the final time.
                totals.2 += 1;
            }
            totals.0 += 1;
        }
    }
    w.set_time(last_cycle)?;
    w.close()?;

    // -- verify ------------------------------------------------------------------
    let size = std::fs::metadata(path)?.len();
    if size >= MAX_BYTES {
        return Err(format!("{} is {size} bytes, limit {MAX_BYTES}", path.display()).into());
    }
    let rd = Reader::open(path)?;
    let (n_tx, n_rel) = rd.tx_counts();
    let mut per_generator: HashMap<String, usize> = HashMap::new();
    let mut flushed = 0;
    let mut open = 0;
    let mut with_stalls = 0;
    rd.visit_transactions(&TxQuery::default(), |tx| {
        assert!(tx.attrs.iter().any(|attr| rd.str(attr.key) == "vtr.label" && matches!(attr.value, Value::Str(_) | Value::Text(_))), "transaction {} lacks vtr.label", tx.id);
        assert!(tx.stages.iter().all(|stage| stage.attrs.iter().any(|(key, value)| rd.str(*key) == "vtr.label" && matches!(value, Value::Str(_) | Value::Text(_)))), "transaction {} has an unlabeled stage", tx.id);
        *per_generator
            .entry(rd.name(tx.generator).to_owned())
            .or_default() += 1;
        match tx.status {
            TxStatus::Aborted => flushed += 1,
            TxStatus::Open => open += 1,
            _ => {}
        }
        if tx.stages.iter().any(|s| rd.str(s.lane) == "stall") {
            with_stalls += 1;
        }
        true
    })?;
    rd.visit_relations(|relation| {
        assert!(relation.attrs.iter().any(|(key, value)| rd.str(*key) == "vtr.label" && matches!(value, Value::Str(_) | Value::Text(_))), "relation {} -> {} lacks vtr.label", relation.from, relation.to);
        true
    })?;
    assert_eq!(n_tx as usize, totals.0 + totals.3);
    assert_eq!(flushed, totals.1);
    assert!(open >= totals.2, "{open} open transactions, {} instructions in flight", totals.2);
    assert!(flushed > 0 && open > 0 && with_stalls > 0 && n_rel > 0);
    let (a, b) = rd.time_range().unwrap();
    assert_eq!(b, last_cycle);
    println!(
        "{}: {size} bytes, cycles {a}..{b}, {n_tx} transactions ({flushed} flushed, {open} open, {with_stalls} with stalls), {n_rel} relations",
        path.display()
    );
    let mut names: Vec<_> = per_generator.into_iter().collect();
    names.sort();
    for (name, count) in names {
        println!("  {name}: {count}");
    }
    Ok(())
}
