//! Reproducible, small VTR feature gallery for Volna. No VDB presentation data.
//! cargo run -p vtr --example feature_showcase -- volna/volna/examples/feature_showcase.vtr

use std::path::Path;
use vtr::{
    Direction, LogArg, LogArgType, LogQuery, LogSiteSpec, NodeData, NodeId, Reader, ScopeType,
    Severity, SignalId, SignalKind, TxKind, TxQuery, TxStatus, Value, VarType, Writer,
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

    let fetch = w.intern("fetch");
    let decode = w.intern("decode");
    let execute = w.intern("execute");
    let retire = w.intern("retire");
    let lane = w.intern("main");
    let memory = w.intern("memory");
    let dependency = w.intern("dependency");
    let request = w.intern("request");
    let label_key = w.intern("vtr.label");
    let pc_key = w.intern("pc");
    let fault = w.intern("fault_injected");
    let mut previous = None;
    for i in 0..48u64 {
        let begin = 64 + i * 32;
        let tx = w.begin_tx(
            if i % 9 == 8 {
                speculative
            } else {
                instructions
            },
            begin,
        )?;
        w.tx_attr(
            tx,
            pc_key,
            &Value::U64(0x8000_0000 + i * 4),
        )?;
        let tx_label = w.intern(&format!("pc 0x{:08x}", 0x8000_0000 + i * 4));
        w.tx_attr(tx, label_key, &Value::Str(tx_label))?;
        let fetch_label = w.intern(&format!("fetch pc 0x{:08x}", 0x8000_0000 + i * 4));
        w.tx_stage(tx, fetch, lane, begin, begin + 8, &[(label_key, Value::Str(fetch_label))])?;
        let decode_label = w.intern(&format!("decode pc 0x{:08x}", 0x8000_0000 + i * 4));
        w.tx_stage(tx, decode, lane, begin + 8, begin + 16, &[(label_key, Value::Str(decode_label))])?;
        w.tx_stage_begin(tx, execute, lane, begin + 16)?;
        let execute_label = w.intern(&format!("execute pc 0x{:08x}", 0x8000_0000 + i * 4));
        w.tx_stage_attr(tx, label_key, &Value::Str(execute_label))?;
        w.tx_stage_attr(tx, fault, &Value::Bool(i == 22))?;
        w.tx_stage_end(tx, execute, lane, begin + 32)?;
        let retire_label = w.intern(&format!("retire pc 0x{:08x}", 0x8000_0000 + i * 4));
        w.tx_event(tx, begin + 32, retire, &[(label_key, Value::Str(retire_label))])?;
        if let Some(prev) = previous {
            let dependency_label = w.intern(&format!("dependency {prev} to {tx}"));
            w.relate(dependency, prev, tx, &[(label_key, Value::Str(dependency_label))])?;
        }
        previous = Some(tx);
        if i % 4 == 0 {
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
            let relation_label = w.intern(&format!("instruction {i} starts DMA transfer"));
            let mut relation_attrs = attrs[4..6].to_vec();
            relation_attrs.push((label_key, Value::Str(relation_label)));
            w.relate(request, tx, transfer, &relation_attrs)?;
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
        w.end_tx(
            tx,
            begin + 40,
            if i % 9 == 8 {
                TxStatus::Aborted
            } else {
                TxStatus::Ok
            },
        )?;
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
