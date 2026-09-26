//! Reproducible, small VTR feature gallery for Volna. No VDB presentation data.
//! cargo run -p vtr --example feature_showcase -- volna/volna/examples/feature_showcase.vtr

use std::path::Path;
use vtr::{
    Direction, LogArg, LogArgType, LogQuery, LogSiteSpec, NodeData, NodeId, Reader, ScopeType,
    Severity, SignalId, SignalKind, TxId, TxKind, TxQuery, TxStatus, Value, VarType, Writer,
    WriterOptions,
};

fn bits(w: &mut Writer, parent: NodeId, name: &str, width: u32, states: u8, dir: Direction) -> vtr::Result<SignalId> {
    Ok(w.add_var(Some(parent), name, VarType::Logic, dir, SignalKind::Bits { width, states })?.1)
}

fn attributes(w: &mut Writer) -> Vec<(vtr::StrId, Value)> {
    let ready = w.intern("READY");
    let inner = w.intern("samples");
    [
        ("optional", Value::Null),
        ("enabled", Value::Bool(true)),
        ("signed", Value::I64(-42)),
        ("unsigned", Value::U64(1 << 60)),
        ("ratio", Value::F64(0.625)),
        ("state", Value::Str(ready)),
        ("packet", Value::Bytes(vec![0, 0x7f, 0x80, 0xff])),
        (
            "mask",
            Value::Bits {
                width: 8,
                data: vec![0xa5],
            },
        ),
        (
            "bus",
            Value::Logic {
                width: 4,
                data: vec![0xe4],
            },
        ),
        (
            "resolved",
            Value::Logic9 {
                width: 9,
                data: vec![0x10, 0x32, 0x54, 0x76, 8],
            },
        ),
        ("deadline", Value::Time(160)),
        (
            "opcode",
            Value::Enum {
                value: 2,
                name: ready,
            },
        ),
        ("handle", Value::Pointer(0x1000)),
        ("gain", Value::Fixed { raw: -13, scale: 3 }),
        (
            "voltage",
            Value::UFixed {
                raw: 3300,
                scale: 10,
            },
        ),
        (
            "vector",
            Value::List(vec![Value::I64(-1), Value::Bool(false)]),
        ),
        (
            "nested",
            Value::Map(vec![(
                inner,
                Value::List(vec![Value::F64(1.5), Value::Null]),
            )]),
        ),
        ("message", Value::Text("DMA → memory: café ✓".into())),
    ]
    .into_iter()
    .map(|(key, value)| (w.intern(key), value))
    .collect()
}

// -- soc.cpu: a five-stage in-order pipeline, simulated cycle by cycle ---------
//
// One instruction per stage and cycle. Execute forwards results, so only a
// load (value after memory) or a multiply (three execute cycles) makes a
// dependent instruction wait in decode. Branches are predicted statically
// (backward taken, forward not taken) and resolve when they leave execute;
// the wrong-path instructions fetched meanwhile are squashed. Loads miss the
// data cache now and then (four extra memory cycles), and the first memory
// access after the injected fault at 768 ns waits in memory until recovery.

const STAGES: [&str; 5] = ["fetch", "decode", "execute", "memory", "writeback"];
const RESET_CYCLE: u64 = 4; // first rising edge after reset_n rises at 32 ns
const LOOP_ITERATIONS: u64 = 12;

/// The rising edge that begins core clock cycle `cycle`: `soc.clk` rises at 4 ns, every 8 ns.
fn cycle_time(cycle: u64) -> u64 {
    4 + 8 * cycle
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Alu,
    Mul,
    Load,
    Store,
    Branch(u64),
    Halt,
}

struct Instr {
    asm: &'static str,
    op: Op,
    dst: Option<&'static str>,
    src: &'static [&'static str],
}

const fn ins(asm: &'static str, op: Op, dst: Option<&'static str>, src: &'static [&'static str]) -> Instr {
    Instr { asm, op, dst, src }
}

const TEXT: u64 = 0x8000_0000;
const LOOP: u64 = TEXT + 0x18;
/// Boot, then a loop that sums and multiplies pairs of words, stores a
/// result on some iterations and rings the DMA doorbell (`8(s1)`) on each.
const PROGRAM: &[Instr] = &[
    ins("lui s1,0x10000", Op::Alu, Some("s1"), &[]),
    ins("sw zero,8(s1)", Op::Store, None, &["s1"]),
    ins("lui a0,0x80010", Op::Alu, Some("a0"), &[]),
    ins("addi s2,zero,12", Op::Alu, Some("s2"), &[]),
    ins("addi a1,zero,0", Op::Alu, Some("a1"), &[]),
    ins("addi a2,zero,0", Op::Alu, Some("a2"), &[]),
    ins("lw t1,0(a0)", Op::Load, Some("t1"), &["a0"]),
    ins("add a1,a1,t1", Op::Alu, Some("a1"), &["a1", "t1"]),
    ins("lw t2,4(a0)", Op::Load, Some("t2"), &["a0"]),
    ins("mul t3,t2,t1", Op::Mul, Some("t3"), &["t2", "t1"]),
    ins("addi a0,a0,8", Op::Alu, Some("a0"), &["a0"]),
    ins("xor a2,a2,t3", Op::Alu, Some("a2"), &["a2", "t3"]),
    ins("andi t4,a1,3", Op::Alu, Some("t4"), &["a1"]),
    ins("beq t4,zero,0x8000003c", Op::Branch(TEXT + 0x3c), None, &["t4"]),
    ins("sw a2,-8(a0)", Op::Store, None, &["a2", "a0"]),
    ins("sw a1,8(s1)", Op::Store, None, &["a1", "s1"]),
    ins("addi s2,s2,-1", Op::Alu, Some("s2"), &["s2"]),
    ins("bne s2,zero,0x80000018", Op::Branch(LOOP), None, &["s2"]),
    ins("fence", Op::Alu, None, &[]),
    ins("wfi", Op::Halt, None, &[]),
];

/// A deterministic hash for "random" outcomes.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

struct Stall {
    name: &'static str,
    label: String,
    begin: u64,
    end: u64,
}

/// One fetched instruction; times are core clock cycles.
struct Insn {
    pc: u64,
    asm: &'static str,
    op: Op,
    wrong_path: bool,
    /// Position on the committed path (wrong-path instructions share their predecessor's).
    order: u64,
    enter: [Option<u64>; 5],
    end: u64,
    squashed: bool,
    busy: u64,
    /// Source register and producing instruction.
    deps: Vec<(&'static str, usize)>,
    miss: bool,
    /// The cycle the injected fault caught this access in memory.
    fault: Option<u64>,
    mispredict: bool,
    redirect: u64,
    waiting: Option<(u64, &'static str, usize)>,
    stalls: Vec<Stall>,
}

impl Insn {
    fn label(&self) -> String {
        let path = if self.wrong_path { " (wrong path)" } else { "" };
        format!("0x{:08x} {}{path}", self.pc, self.asm)
    }
}

fn simulate() -> Vec<Insn> {
    let mut run: Vec<Insn> = Vec::new();
    let mut slot: [Option<usize>; 5] = [None; 5];
    let mut last_writer: std::collections::HashMap<&str, usize> = Default::default();
    let (mut fetch_pc, mut wrong_path, mut halted) = (TEXT, false, false);
    let (mut order, mut iteration, mut fault_pending) = (0u64, 0u64, true);
    let mut cycle = RESET_CYCLE;
    let recovery = (0..).find(|&c| cycle_time(c) >= 896).unwrap();
    let is_access = |insn: &Insn| matches!(insn.op, Op::Load | Op::Store) && !insn.wrong_path;
    loop {
        // The fault catches the access in memory, or the next one to get there.
        if fault_pending && cycle_time(cycle) >= 768 {
            if let Some(k) = slot[3].filter(|&k| is_access(&run[k])) {
                fault_pending = false;
                run[k].fault = Some(cycle);
                run[k].busy = recovery;
            }
        }
        // Writeback retires.
        if let Some(k) = slot[4].filter(|&k| run[k].busy <= cycle) {
            run[k].end = cycle;
            slot[4] = None;
        }
        // Memory to writeback; a long memory access is a stall.
        if let Some(k) = slot[3].filter(|&k| run[k].busy <= cycle && slot[4].is_none()) {
            let entered = run[k].enter[3].unwrap();
            let fault = run[k].fault.map_or(cycle, |at| at.max(entered + 1));
            if fault > entered + 1 {
                let label = format!("data cache miss at 0x{:08x}", run[k].pc);
                run[k].stalls.push(Stall { name: "dcache miss", label, begin: entered + 1, end: fault });
            }
            if fault < cycle {
                let label = "bus fault: the access is retried until recovery".to_owned();
                run[k].stalls.push(Stall { name: "bus fault", label, begin: fault, end: cycle });
            }
            run[k].enter[4] = Some(cycle);
            run[k].busy = cycle + 1;
            slot[3] = None;
            slot[4] = Some(k);
        }
        // Execute to memory; a mispredicted branch squashes what follows it.
        if let Some(k) = slot[2].filter(|&k| run[k].busy <= cycle && slot[3].is_none()) {
            let mut busy = cycle + 1;
            if is_access(&run[k]) {
                if fault_pending && cycle_time(cycle) >= 768 {
                    fault_pending = false;
                    run[k].fault = Some(cycle);
                    busy = recovery;
                } else if run[k].miss {
                    busy += 4;
                }
            }
            run[k].enter[3] = Some(cycle);
            run[k].busy = busy;
            slot[2] = None;
            slot[3] = Some(k);
            if run[k].mispredict {
                for younger in [slot[1].take(), slot[0].take()].into_iter().flatten() {
                    run[younger].end = cycle;
                    run[younger].squashed = true;
                }
                fetch_pc = run[k].redirect;
                wrong_path = false;
            }
        }
        // Decode to execute, once every operand can be forwarded.
        if let Some(k) = slot[1].filter(|&k| run[k].busy <= cycle) {
            let blocked = run[k].deps.iter().copied().find(|&(_, p)| {
                let producer = &run[p];
                let available = match producer.op {
                    Op::Load => producer.enter[4],
                    _ => producer.enter[3],
                };
                !available.is_some_and(|at| at <= cycle)
            });
            match (blocked, run[k].waiting) {
                (Some((reg, p)), None) => run[k].waiting = Some((cycle, reg, p)),
                (None, Some((begin, reg, p))) => {
                    let label = format!("waits for {reg} from 0x{:08x} {}", run[p].pc, run[p].asm);
                    run[k].stalls.push(Stall { name: "operand", label, begin, end: cycle });
                    run[k].waiting = None;
                }
                _ => {}
            }
            if blocked.is_none() && slot[2].is_none() {
                run[k].enter[2] = Some(cycle);
                run[k].busy = cycle + if run[k].op == Op::Mul { 3 } else { 1 };
                slot[1] = None;
                slot[2] = Some(k);
            }
        }
        // Fetch to decode.
        if let Some(k) = slot[0].filter(|&k| run[k].busy <= cycle && slot[1].is_none()) {
            run[k].enter[1] = Some(cycle);
            run[k].busy = cycle + 1;
            slot[0] = None;
            slot[1] = Some(k);
        }
        // Fetch along the predicted path.
        if slot[0].is_none() && !halted {
            let instr = &PROGRAM[((fetch_pc - TEXT) / 4) as usize];
            let k = run.len();
            let deps = instr.src.iter().filter_map(|reg| last_writer.get(reg).map(|&p| (*reg, p))).collect();
            let mut insn = Insn {
                pc: fetch_pc,
                asm: instr.asm,
                op: instr.op,
                wrong_path,
                order,
                enter: [Some(cycle), None, None, None, None],
                end: 0,
                squashed: false,
                busy: cycle + 1,
                deps,
                miss: instr.op == Op::Load && mix(order + 4) % 4 == 0,
                fault: None,
                mispredict: false,
                redirect: 0,
                waiting: None,
                stalls: Vec::new(),
            };
            let mut next = fetch_pc + 4;
            if let Op::Branch(target) = instr.op {
                let predicted = target < fetch_pc;
                let taken = if wrong_path {
                    predicted
                } else if target == LOOP {
                    iteration += 1;
                    iteration < LOOP_ITERATIONS
                } else {
                    mix(1000 + iteration) % 3 == 0
                };
                if !wrong_path && taken != predicted {
                    insn.mispredict = true;
                    insn.redirect = if taken { target } else { fetch_pc + 4 };
                    wrong_path = true;
                }
                if predicted {
                    next = target;
                }
            }
            if !wrong_path || insn.mispredict {
                if let Some(dst) = instr.dst {
                    last_writer.insert(dst, k);
                }
                order += 1;
                halted = instr.op == Op::Halt;
            }
            fetch_pc = next;
            slot[0] = Some(k);
            run.push(insn);
        }
        if halted && slot.iter().all(Option::is_none) {
            return run;
        }
        cycle += 1;
        assert!(cycle_time(cycle) < 2040, "the program runs past the capture");
    }
}

fn write(path: &Path) -> vtr::Result<()> {
    let mut w = Writer::create_with(
        path,
        WriterOptions {
            background: false,
            ..Default::default()
        },
    )?;
    w.set_timescale(-9)?;
    w.set_file_type(vtr::FileType::VerilogVhdl)?;
    w.set_time_zero(-16)?;
    w.set_writer_name("VTR feature showcase")?;
    w.set_date("2026-09-18")?;
    w.set_comment("Synthetic 2 us debugging lab: boot, DMA traffic, injected bus fault, recovery. See examples/README.md.")?;
    w.set_file_attr("seed", Value::U64(2026))?;
    let soc = w.add_scope(None, "soc", ScopeType::Module, "debug_lab")?;
    let clk = bits(&mut w, soc, "clk", 1, 2, Direction::Input)?;
    let reset = bits(&mut w, soc, "reset_n", 1, 4, Direction::Input)?;
    let valid = bits(&mut w, soc, "valid", 1, 2, Direction::Output)?;
    let ready = bits(&mut w, soc, "ready", 1, 4, Direction::Input)?;
    let address = bits(&mut w, soc, "address", 32, 2, Direction::Output)?;
    let data = bits(&mut w, soc, "data", 32, 4, Direction::InOut)?;
    let wide = bits(&mut w, soc, "payload_128", 128, 2, Direction::Output)?;
    let resolved = bits(&mut w, soc, "resolved_bus", 9, 9, Direction::InOut)?;
    let (_, temperature) = w.add_var(
        Some(soc),
        "temperature_c",
        VarType::Real,
        Direction::Implicit,
        SignalKind::Real,
    )?;
    let [sine_fast, sine_slow] = ["sine_fast", "sine_slow"].map(|name| {
        w.add_var(
            Some(soc),
            name,
            VarType::Real,
            Direction::Implicit,
            SignalKind::Real,
        )
        .map(|(_, id)| id)
    });
    let (sine_fast, sine_slow) = (sine_fast?, sine_slow?);
    let (_, message) = w.add_var(
        Some(soc),
        "phase",
        VarType::String,
        Direction::Implicit,
        SignalKind::VarLen,
    )?;
    let (_, packet) = w.add_var(
        Some(soc),
        "packet_bytes",
        VarType::Bytes,
        Direction::Implicit,
        SignalKind::VarLen,
    )?;
    let (_, event) = w.add_var(
        Some(soc),
        "interrupt",
        VarType::Event,
        Direction::Output,
        SignalKind::Bits {
            width: 1,
            states: 2,
        },
    )?;
    let (_, parameter) = w.add_var(
        Some(soc),
        "BUS_WIDTH",
        VarType::Parameter,
        Direction::Implicit,
        SignalKind::Bits {
            width: 32,
            states: 2,
        },
    )?;
    let table = w.add_enum_table(
        Some(soc),
        "phase_t",
        &[
            ("BOOT", "00"),
            ("RUN", "01"),
            ("FAULT", "10"),
            ("RECOVER", "11"),
        ],
    )?;
    let (state_node, state) = w.add_var(
        Some(soc),
        "state",
        VarType::Enum,
        Direction::Output,
        SignalKind::Bits {
            width: 2,
            states: 2,
        },
    )?;
    w.node_attr(state_node, "enum_table", Value::U64(table.0 as u64))?;
    w.add_alias(Some(soc), "clock_alias", VarType::Wire, Direction::Input, clk)?;

    let cpu = w.add_scope(Some(soc), "cpu", ScopeType::Core, "tiny_cpu")?;
    let pc = bits(&mut w, cpu, "pc", 32, 2, Direction::Output)?;
    let pipeline = w.add_stream(Some(cpu), "thread0", "PIPELINE")?;
    let core_path = w.intern("soc.clk");
    w.node_attr(pipeline, "vtr.clock", Value::Str(core_path))?;
    let instructions = w.add_generator(pipeline, "instructions")?;
    let speculative = w.add_generator(pipeline, "speculative")?;
    let dma = w.add_scope(Some(soc), "dma", ScopeType::ScModule, "dma_engine")?;
    let bus = w.add_stream(Some(dma), "memory_bus", "MEMORY_BUS")?;
    let dma_path = w.intern("soc.dma.dma_clk");
    w.node_attr(bus, "vtr.clock", Value::Str(dma_path))?;
    let reads = w.add_generator(bus, "read")?;
    let writes = w.add_generator(bus, "write")?;
    w.add_generator(bus, "idle")?;
    w.add_stream(Some(dma), "standby_bus", "MEMORY_BUS")?;
    let logs = w.add_stream(Some(soc), "log", vtr::LOG_STREAM_KIND)?;

    // Declared clocks (docs/vtr_clocks.html). `soc.clk` is the dumped `clk`: 8 ns,
    // rising at 4 ns and running at capture end. The DMA clock has no dumped net:
    // 12 ns, stopped for the fault window, 6 ns while it recovers, then 16 ns.
    let core_clk = w.add_clock(Some(soc), "clk")?;
    w.clock_run(core_clk, 4, 8)?;
    let dma_clk = w.add_clock(Some(dma), "dma_clk")?;
    w.clock_run(dma_clk, 8, 12)?;
    w.clock_stop(dma_clk, 768)?;
    w.clock_run(dma_clk, 904, 6)?;
    w.clock_stop(dma_clk, 1400)?;
    w.clock_run(dma_clk, 1406, 16)?;
    let mut sites = Vec::new();
    for (i, severity) in [
        Severity::Trace,
        Severity::Debug,
        Severity::Info,
        Severity::Warn,
        Severity::Error,
        Severity::Fatal,
        Severity::Other(9),
    ]
    .into_iter()
    .enumerate()
    {
        sites.push(
            w.add_log_site(
                &LogSiteSpec::new(
                    logs,
                    severity,
                    &format!("{}: cycle={{}} phase={{}}", severity.name()),
                    &[LogArgType::U64, LogArgType::Text, LogArgType::Text],
                )
                .names(&["cycle", "phase", "vtr.label"])
                .location("debug_lab.cpp", 40 + i as u32)
                .func("tick"),
            )?,
        );
    }
    let typed_site = w.add_log_site(
        &LogSiteSpec::new(
            logs,
            Severity::Debug,
            "typed args: {} {} {} {} {} {} {} {} {}",
            &[
                LogArgType::Bool,
                LogArgType::I64,
                LogArgType::U64,
                LogArgType::F64,
                LogArgType::Str,
                LogArgType::Bytes,
                LogArgType::Time,
                LogArgType::Pointer,
                LogArgType::Text,
                LogArgType::Text,
            ],
        )
        .names(&["bool", "i64", "u64", "f64", "str", "bytes", "time", "pointer", "text", "vtr.label"])
        .location("debug_lab.cpp", 80)
        .func("inspect_packet"),
    )?;

    // A compact declaration gallery: every standard scope code under `scopes` and
    // every variable code under `declarations`, each plus an unknown producer code.
    // The useful design remains at the top of the tree.
    let type_gallery = w.add_scope(Some(soc), "type_gallery", ScopeType::Package, "")?;
    let scopes = w.add_scope(Some(type_gallery), "scopes", ScopeType::Generic, "")?;
    let mut gallery = Vec::new();
    for code in (0..=22).chain(64..=68).chain([200]) {
        let kind = ScopeType::from_code(code);
        let scope = w.add_scope(Some(scopes), kind.name(), kind, "")?;
        let signal = bits(&mut w, scope, "active", 1, 2, Direction::Implicit)?;
        gallery.push((
            signal,
            SignalKind::Bits {
                width: 1,
                states: 2,
            },
            VarType::Logic,
        ));
    }
    let declarations = w.add_scope(Some(type_gallery), "declarations", ScopeType::Generic, "")?;
    for code in (0..=29).chain([64, 65, 200]) {
        let typ = VarType::from_code(code);
        let kind = match typ {
            VarType::Real | VarType::RealParameter | VarType::RealTime | VarType::ShortReal => {
                SignalKind::Real
            }
            VarType::String | VarType::Bytes | VarType::Port => SignalKind::VarLen,
            VarType::Event => SignalKind::Bits {
                width: 1,
                states: 2,
            },
            VarType::Enum => SignalKind::Bits {
                width: 2,
                states: 2,
            },
            _ => SignalKind::Bits {
                width: 16,
                states: 4,
            },
        };
        let (node, signal) = w.add_var(Some(declarations), typ.name(), typ, Direction::from_u8((code % 6) as u8), kind)?;
        if typ == VarType::Enum {
            w.node_attr(node, "enum_table", Value::U64(table.0 as u64))?;
        }
        gallery.push((signal, kind, typ));
    }
    let literal_names = w.add_scope(Some(type_gallery), "literal.names", ScopeType::Struct, "")?;
    w.add_alias(Some(literal_names), "escaped.signal[3]", VarType::Wire, Direction::Linkage, data)?;
    let attrs = attributes(&mut w);
    for (key, value) in &attrs {
        let key = w.string(*key).to_owned();
        w.node_attr(soc, &key, value.clone())?;
    }

    let resource = w.add_scope(None, "firmware", ScopeType::Resource, "")?;
    let service = w.intern("dma-demo");
    w.node_attr(resource, "service.name", Value::Str(service))?;
    let otel = w.add_stream(Some(resource), "driver", "otel.scope")?;
    let spans = w.add_generator(otel, "submit_transfer")?;

    // 256 cycles, 8 ns per cycle. The blackout is deliberately not populated:
    // dump_off/on are markers; producers decide which values to omit.
    let mut reading = None;
    for t in (0..=2048u64).step_by(4) {
        w.set_time(t)?;
        if t == 960 {
            w.dump_off();
        }
        if t == 992 {
            w.dump_on();
        }
        if (960..992).contains(&t) {
            continue;
        }
        let cycle = t / 8;
        let phase = if t < 64 {
            0
        } else if t < 768 {
            1
        } else if t < 896 {
            2
        } else {
            3
        };
        w.emit_bit(clk, ((t / 4) & 1) as u8)?;
        w.emit_bit(reset, u8::from(t >= 32))?;
        w.emit_bit(valid, u8::from(t >= 64 && cycle % 4 != 0))?;
        w.emit_bit(
            ready,
            if phase == 2 {
                2
            } else {
                u8::from(cycle % 7 != 0)
            },
        )?;
        w.emit_u64(address, 0x1000 + 4 * (cycle % 64))?;
        if phase == 2 {
            w.emit_logic_str(data, b"xxxxxxxxzzzzzzzz0000000011111111")?;
        } else {
            w.emit_u64(data, cycle.wrapping_mul(0x9e3779b9) & 0xffff_ffff)?;
        }
        w.emit_words(
            wide,
            &[cycle as u32, 0xdead_beef, !(cycle as u32), 0x0123_4567],
        )?;
        w.emit_logic_str(
            resolved,
            if cycle % 2 == 0 {
                b"01XZUWLH-"
            } else {
                b"-HLWUZX10"
            },
        )?;
        w.emit_real(temperature, 35.0 + ((cycle % 64) as f64 - 32.0).abs() / 8.0)?;
        // Sampled every 4 ns: amplitude 0.5 with a 128 ns period, and
        // amplitude 20 with a 1,024 ns period.
        let angle = std::f64::consts::TAU * t as f64;
        w.emit_real(sine_fast, 0.5 * (angle / 128.0).sin())?;
        w.emit_real(sine_slow, 20.0 * (angle / 1024.0).sin())?;
        w.emit_varlen(
            message,
            [
                b"BOOT".as_slice(),
                b"RUN",
                b"FAULT: retry",
                b"RECOVER -> RUN",
            ][phase],
        )?;
        w.emit_varlen(packet, &[0, cycle as u8, 0x80, 0xff])?;
        w.emit_u64(parameter, 32)?;
        w.emit_u64(state, phase as u64)?;
        w.emit_u64(pc, 0x8000_0000 + 4 * cycle)?;
        if t % 128 == 0 {
            w.emit_bit(event, 1)?;
            w.emit_bit(event, 1)?; // two occurrences, same timestamp
        }
        for (id, kind, typ) in &gallery {
            let sample = match typ {
                VarType::Parameter | VarType::RealParameter | VarType::Supply1 => 1,
                VarType::Supply0 => 0,
                VarType::Enum => cycle % 4,
                _ => cycle,
            };
            match kind {
                SignalKind::Bits { width, .. } => {
                    w.emit_u64(*id, if *width == 1 { sample & 1 } else { sample })?
                }
                SignalKind::Real => w.emit_real(*id, 1.25 + sample as f64 / 16.0)?,
                SignalKind::VarLen => {
                    w.emit_varlen(*id, if cycle % 2 == 0 { b"hello" } else { b"world" })?
                }
            }
        }
        if t == 1024 {
            w.flush()?;
            // Hierarchy added halfway through the run: a new signal and an alias.
            let hotplug_sensor = w.add_scope(None, "hotplug_sensor", ScopeType::ScModule, "late_sensor")?;
            reading = Some(bits(&mut w, hotplug_sensor, "reading", 8, 4, Direction::Output)?);
            w.add_alias(Some(hotplug_sensor), "sample", VarType::Wire, Direction::Output, data)?;
        }
        if let Some(reading) = reading {
            w.emit_u64(reading, cycle & 0xff)?;
        }
    }

    // soc.cpu.thread0: the five-stage pipeline below, one record per fetched
    // instruction. Stages sit on lane `main`; the `stall` lane marks why an
    // instruction waits (an operand, a cache miss, the bus fault).
    let main_lane = w.intern("main");
    let stall_lane = w.intern("stall");
    let stage_names: Vec<_> = STAGES.iter().map(|name| w.intern(name)).collect();
    let execute = stage_names[2];
    let memory = w.intern("memory");
    let dependency = w.intern("dependency");
    let request = w.intern("request");
    let retire = w.intern("retire");
    let mispredict = w.intern("mispredict");
    let label_key = w.intern("vtr.label");
    let pc_key = w.intern("pc");
    let fault = w.intern("fault_injected");
    let cpu_run = simulate();
    let mut insn_tx = Vec::with_capacity(cpu_run.len());
    for insn in &cpu_run {
        let begin = cycle_time(insn.enter[0].unwrap());
        let tx = w.begin_tx(if insn.wrong_path { speculative } else { instructions }, begin)?;
        insn_tx.push(tx);
        w.tx_attr(tx, pc_key, &Value::U64(insn.pc))?;
        let label = w.intern(&insn.label());
        w.tx_attr(tx, label_key, &Value::Str(label))?;
        let end = cycle_time(insn.end);
        for (s, name) in stage_names.iter().enumerate() {
            let Some(enter) = insn.enter[s] else { break };
            let leave = insn.enter.get(s + 1).copied().flatten().map_or(end, cycle_time);
            let stage_label = w.intern(&format!("{} 0x{:08x}", STAGES[s], insn.pc));
            let mut attrs = vec![(label_key, Value::Str(stage_label))];
            if s == 3 && matches!(insn.op, Op::Load | Op::Store) {
                attrs.push((fault, Value::Bool(insn.fault.is_some())));
            }
            w.tx_stage(tx, *name, main_lane, cycle_time(enter), leave, &attrs)?;
        }
        for stall in &insn.stalls {
            let name = w.intern(stall.name);
            let stall_label = w.intern(&stall.label);
            w.tx_stage(tx, name, stall_lane, cycle_time(stall.begin), cycle_time(stall.end), &[(label_key, Value::Str(stall_label))])?;
        }
        if insn.mispredict {
            let resolve = cycle_time(insn.enter[3].unwrap());
            let event_label = w.intern(&format!("mispredicted 0x{:08x}; fetch redirected", insn.pc));
            w.tx_event(tx, resolve, mispredict, &[(label_key, Value::Str(event_label))])?;
        }
        if !insn.squashed {
            let event_label = w.intern(&format!("retire 0x{:08x}", insn.pc));
            w.tx_event(tx, end, retire, &[(label_key, Value::Str(event_label))])?;
        }
        w.end_tx(tx, end, if insn.squashed { TxStatus::Aborted } else { TxStatus::Ok })?;
    }
    // Operand dependencies between nearby instructions of the committed path.
    for (k, insn) in cpu_run.iter().enumerate() {
        for &(reg, producer) in &insn.deps {
            if insn.squashed || insn.order - cpu_run[producer].order > 4 {
                continue;
            }
            let dependency_label = w.intern(&format!("{reg}: 0x{:08x} to 0x{:08x}", cpu_run[producer].pc, insn.pc));
            w.relate(dependency, insn_tx[producer], insn_tx[k], &[(label_key, Value::Str(dependency_label))])?;
        }
    }
    // DMA doorbell stores, in the order their memory stage begins.
    let doorbells: Vec<(u64, TxId)> = cpu_run
        .iter()
        .zip(&insn_tx)
        .filter(|(insn, _)| !insn.squashed && insn.asm.ends_with(",8(s1)"))
        .map(|(insn, tx)| (cycle_time(insn.enter[3].unwrap()), *tx))
        .collect();
    // One DMA transfer every 128 ns, each requested by the latest doorbell store
    // whose memory stage began by then.
    for i in (0..48u64).step_by(4) {
        let begin = 64 + i * 32;
        let rung = doorbells.iter().take_while(|(t, _)| *t <= begin + 16).count();
        let doorbell = doorbells[rung.max(1) - 1].1;
        let span = w.begin_tx(spans, begin)?;
        w.set_tx_kind(span, TxKind::Client)?;
        let span_label = w.intern(&format!("submission {i}"));
        w.tx_attr(span, label_key, &Value::Str(span_label))?;
        let transfer = w.begin_tx(if i % 8 == 0 { reads } else { writes }, begin + 16)?;
        w.set_tx_parent(transfer, span)?;
        w.set_tx_kind(transfer, TxKind::Consumer)?;
        let transfer_label = w.intern(&format!("DMA transfer {i}"));
        w.tx_attr(transfer, label_key, &Value::Str(transfer_label))?;
        for (key, value) in &attrs {
            w.tx_attr(transfer, *key, value)?;
        }
        let memory_label = w.intern(&format!("DMA memory access {i}"));
        let mut stage_attrs = attrs[..2].to_vec();
        stage_attrs.push((label_key, Value::Str(memory_label)));
        w.tx_stage(
            transfer,
            execute,
            memory,
            begin + 16,
            begin + 56,
            &stage_attrs,
        )?;
        let request_label = w.intern(&format!("DMA request {i}"));
        let mut event_attrs = attrs[2..4].to_vec();
        event_attrs.push((label_key, Value::Str(request_label)));
        w.tx_event(transfer, begin + 40, request, &event_attrs)?;
        let relation_label = w.intern(&format!("doorbell store starts DMA transfer {i}"));
        let mut relation_attrs = attrs[4..6].to_vec();
        relation_attrs.push((label_key, Value::Str(relation_label)));
        w.relate(request, doorbell, transfer, &relation_attrs)?;
        let log_label = format!("DMA issue log {i}");
        w.log_with_parent(
            sites[2],
            begin + 20,
            Some(transfer),
            &[LogArg::U64(i), LogArg::Text("DMA issued"), LogArg::Text(&log_label)],
        )?;
        w.end_tx(
            transfer,
            begin + 64,
            if i == 24 {
                TxStatus::Error
            } else {
                TxStatus::Ok
            },
        )?;
        w.end_tx(span, begin + 72, TxStatus::Ok)?;
    }
    for (i, site) in sites.into_iter().enumerate() {
        let log_label = format!("injected fault log {i}");
        w.log(
            site,
            768 + i as u64 * 16,
            &[
                LogArg::U64(96 + i as u64 * 2),
                LogArg::Text("injected fault / recovery"),
                LogArg::Text(&log_label),
            ],
        )?;
    }
    let interned = w.intern("READY");
    w.log(
        typed_site,
        900,
        &[
            LogArg::Bool(true),
            LogArg::I64(-7),
            LogArg::U64(42),
            LogArg::F64(3.25),
            LogArg::Str(interned),
            LogArg::Bytes(&[0, 255]),
            LogArg::Time(900),
            LogArg::Pointer(0x2000),
            LogArg::Text("café ✓"),
            LogArg::Text("typed argument sample"),
        ],
    )?;
    // All span kinds and an unfinished operation/stage at capture end.
    for (i, kind) in [
        TxKind::Internal,
        TxKind::Server,
        TxKind::Client,
        TxKind::Producer,
        TxKind::Consumer,
    ]
    .into_iter()
    .enumerate()
    {
        let tx = w.begin_tx(spans, 1800 + i as u64 * 16)?;
        w.set_tx_kind(tx, kind)?;
        let kind_label = w.intern(&format!("span instance {i}"));
        w.tx_attr(tx, label_key, &Value::Str(kind_label))?;
        w.end_tx(
            tx,
            1808 + i as u64 * 16,
            if i == 0 {
                TxStatus::Unset
            } else {
                TxStatus::Ok
            },
        )?;
    }
    let open = w.begin_tx(writes, 2000)?;
    let open_label = w.intern("unfinished write 0");
    w.tx_attr(open, label_key, &Value::Str(open_label))?;
    w.tx_stage_begin(open, execute, memory, 2000)?;
    let open_stage_label = w.intern("unfinished memory access 0");
    w.tx_stage_attr(open, label_key, &Value::Str(open_stage_label))?;
    w.close()?;
    verify(path)
}

fn verify(path: &Path) -> vtr::Result<()> {
    let size = std::fs::metadata(path)?.len();
    assert!(size < 500_000, "demo exceeds 500 KB: {size}");
    let r = Reader::open(path)?;
    assert!(!r.recovered());
    assert_eq!(r.time_range(), Some((0, 2048)));
    assert_eq!(r.blackout().len(), 2);
    assert!(r.block_count() >= 2);
    let root = r.find_node(&["soc"]).unwrap();
    let tags: std::collections::BTreeSet<_> = r
        .hierarchy()
        .node(root)
        .attrs
        .iter()
        .map(|(_, v)| v.tag() as u8)
        .collect();
    assert_eq!(tags, (0..=17).collect());
    let mut changes = 0;
    for id in 0..r.signal_count() {
        changes += r.load_signal(SignalId(id))?.len();
    }
    let transactions = r.transactions(&TxQuery::default())?;
    let has_label = |attrs: &[(vtr::StrId, Value)]| {
        attrs.iter().any(|(key, value)| {
            r.str(*key) == "vtr.label" && matches!(value, Value::Str(_) | Value::Text(_))
        })
    };
    // Clock stretches are unnamed by design; every other record carries a label.
    let clock_generators: Vec<_> = r.clocks().iter().map(|c| c.generator).collect();
    for tx in transactions.iter().filter(|t| !clock_generators.contains(&t.generator)) {
        assert!(tx.attrs.iter().any(|attr| r.str(attr.key) == "vtr.label" && matches!(attr.value, Value::Str(_) | Value::Text(_))), "transaction {} lacks vtr.label", tx.id);
        assert!(tx.events.iter().all(|event| has_label(&event.attrs)), "transaction {} has an unlabeled event", tx.id);
        assert!(tx.stages.iter().all(|stage| has_label(&stage.attrs)), "transaction {} has an unlabeled stage", tx.id);
    }
    let statuses: std::collections::BTreeSet<_> =
        transactions.iter().map(|t| t.status as u8).collect();
    assert_eq!(statuses, (0..=4).collect());
    let kinds: std::collections::BTreeSet<_> = transactions.iter().map(|t| t.kind as u8).collect();
    assert_eq!(kinds, (0..=5).collect());
    let unfinished = transactions
        .iter()
        .find(|t| t.status == TxStatus::Open)
        .unwrap();
    assert_eq!(unfinished.stages[0].end, Some(2048));
    let event = r.find_signal("soc.interrupt", '.').unwrap();
    let occurrences = r.load_signal(event)?;
    assert!(occurrences
        .times()
        .windows(2)
        .any(|times| times[0] == times[1]));
    let mut relations = 0;
    r.visit_relations(|relation| {
        assert!(has_label(&relation.attrs), "relation {} -> {} lacks vtr.label", relation.from, relation.to);
        relations += 1;
        true
    })?;
    let mut logs = 0;
    r.visit_log(&LogQuery::default(), |record| {
        let _ = record.format(r.strings());
        logs += 1;
        true
    })?;
    assert_eq!(logs, 20);
    let paths: Vec<&str> = r.clocks().iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["soc.clk", "soc.dma.dma_clk"]);
    let core = r.clock(r.clocks()[0].id)?;
    let clk = r.load_signal(r.find_signal("soc.clk", '.').unwrap())?;
    let rising: Vec<u64> = (1..clk.len()).filter(|&i| clk.get(i).as_u64() == Some(1)).map(|i| clk.times()[i]).collect();
    // The clock keeps ticking through the 960-992 recording gap, where the waveform has no values.
    let edges: Vec<u64> = (0..core.edge_count())
        .map(|c| core.edge(c).unwrap())
        .filter(|t| !(960..992).contains(t))
        .collect();
    assert_eq!(edges, rising, "the declared core clock reproduces the dumped clk");
    let dma = r.clock(r.clocks()[1].id)?;
    let stretches: Vec<_> = dma.stretches().iter().map(|s| (s.begin, s.end, s.period)).collect();
    assert_eq!(stretches, [(8, 764, 12), (904, 1396, 6), (1406, 2046, 16)]);
    assert!(dma.cycle_at(800).unwrap().stopped);
    let stream = |path: &[&str]| r.find_node(path).unwrap();
    assert_eq!(r.stream_clock(stream(&["soc", "cpu", "thread0"])), Some(r.clocks()[0].id));
    assert_eq!(r.stream_clock(stream(&["soc", "dma", "memory_bus"])), Some(r.clocks()[1].id));
    // The pipeline: several instructions in flight at once, stages on the main
    // lane and waits on the stall lane, wrong-path instructions squashed.
    let instructions = stream(&["soc", "cpu", "thread0", "instructions"]);
    let speculative = stream(&["soc", "cpu", "thread0", "speculative"]);
    let insns: Vec<_> = transactions.iter().filter(|t| t.generator == instructions).collect();
    let lanes: std::collections::BTreeSet<_> = insns.iter().flat_map(|t| &t.stages).map(|s| r.str(s.lane)).collect();
    assert_eq!(lanes, ["main", "stall"].into());
    let in_flight = |t: u64| insns.iter().filter(|i| i.begin <= t && t < i.end).count();
    assert_eq!((0..2048).map(in_flight).max(), Some(5));
    assert!(transactions.iter().filter(|t| t.generator == speculative).all(|t| t.status == TxStatus::Aborted));
    let vars = r
        .hierarchy()
        .ids()
        .filter(|&id| matches!(r.hierarchy().node(id).data, NodeData::Var { .. }))
        .count();
    println!("{}: {size} bytes; {vars} variables / {} signals; {changes} changes; {} transactions (including {logs} logs and {} clock stretches); {relations} relations", path.display(), r.signal_count(), transactions.len(), core.stretches().len() + dma.stretches().len());
    Ok(())
}

fn main() -> vtr::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .expect("usage: feature_showcase OUTPUT.vtr");
    write(Path::new(&path))
}
