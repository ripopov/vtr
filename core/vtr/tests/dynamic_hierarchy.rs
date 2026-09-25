//! Hierarchy that grows while values are written (docs/dyn_hierarchy.html).

use vtr::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-dyn-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Declares an 8-bit 2-state wire under `parent`.
fn byte(w: &mut Writer, parent: Option<NodeId>, name: &str) -> SignalId {
    w.add_var(parent, name, VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap().1
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Bit,
    Byte2,
    Byte4,
    Real,
    Text,
}

const KINDS: [Kind; 5] = [Kind::Bit, Kind::Byte2, Kind::Byte4, Kind::Real, Kind::Text];

/// A declared signal and the history the test expects to read back.
struct Expected {
    id: SignalId,
    kind: Kind,
    declared_at: u64,
    emits: u64,
    history: Vec<(u64, String)>,
}

impl Expected {
    fn default_ascii(&self) -> &'static str {
        match self.kind {
            Kind::Bit => "x",
            Kind::Byte2 => "00000000",
            Kind::Byte4 => "xxxxxxxx",
            Kind::Real => "0",
            Kind::Text => "",
        }
    }

    fn value_at(&self, t: u64) -> String {
        let n = self.history.partition_point(|(ht, _)| *ht <= t);
        if n == 0 {
            self.default_ascii().to_string()
        } else {
            self.history[n - 1].1.clone()
        }
    }

    fn changes(&self, t0: u64, t1: u64) -> Vec<(u64, String)> {
        self.history.iter().filter(|(t, _)| (t0..=t1).contains(t)).cloned().collect()
    }

    /// Emits the next value, which always differs from the previous one.
    fn emit(&mut self, w: &mut Writer, t: u64) {
        let c = self.emits;
        self.emits += 1;
        let ascii = match self.kind {
            Kind::Bit => {
                w.emit_bit(self.id, (c % 2) as u8).unwrap();
                format!("{}", c % 2)
            }
            Kind::Byte2 => {
                let v = c % 255 + 1;
                w.emit_u64(self.id, v).unwrap();
                format!("{v:08b}")
            }
            Kind::Byte4 if c % 7 == 3 => {
                w.emit_logic_str(self.id, b"01xz01x0").unwrap();
                "01xz01x0".to_string()
            }
            Kind::Byte4 => {
                let v = c % 256;
                w.emit_u64(self.id, v).unwrap();
                format!("{v:08b}")
            }
            Kind::Real => {
                let v = c as f64 + 0.5;
                w.emit_real(self.id, v).unwrap();
                format!("{v}")
            }
            Kind::Text => {
                let v = format!("s{c}");
                w.emit_varlen(self.id, v.as_bytes()).unwrap();
                v
            }
        };
        self.history.push((t, ascii));
    }
}

fn declare(w: &mut Writer, sigs: &mut Vec<Expected>, t: u64) {
    for kind in KINDS {
        let name = format!("s{}", sigs.len());
        let (var_type, sk) = match kind {
            Kind::Bit => (VarType::Wire, SignalKind::Bits { width: 1, states: 4 }),
            Kind::Byte2 => (VarType::Bit, SignalKind::Bits { width: 8, states: 2 }),
            Kind::Byte4 => (VarType::Logic, SignalKind::Bits { width: 8, states: 4 }),
            Kind::Real => (VarType::Real, SignalKind::Real),
            Kind::Text => (VarType::String, SignalKind::VarLen),
        };
        let (_, id) = w.add_var(None, &name, var_type, Direction::Implicit, sk).unwrap();
        sigs.push(Expected { id, kind, declared_at: t, emits: 0, history: Vec::new() });
    }
}

const STEPS: u64 = 60;

/// Writes signals declared at five moments of the run and returns their expected histories.
fn write_run(path: &std::path::Path, group_size: u32, background: bool) -> Vec<Expected> {
    let opts = WriterOptions { group_size, background, block_records: 8, chunk_records: 4, ..Default::default() };
    let mut w = Writer::create_with(path, opts).unwrap();
    let mut sigs = Vec::new();
    // Before the first time step.
    declare(&mut w, &mut sigs, 0);
    for step in 0..STEPS {
        let t = step * 10;
        w.set_time(t).unwrap();
        if step == 25 {
            w.flush().unwrap();
            // Right after an explicit flush.
            declare(&mut w, &mut sigs, t);
        }
        for (i, s) in sigs.iter_mut().enumerate() {
            if (step + i as u64) % (1 + i as u64 % 3) == 0 {
                s.emit(&mut w, t);
            }
        }
        match step {
            // At time 0 after the first values: what an initial block does.
            0 => declare(&mut w, &mut sigs, t),
            // In the middle of a block.
            13 => declare(&mut w, &mut sigs, t),
            // Late in the run.
            50 => declare(&mut w, &mut sigs, t),
            _ => {}
        }
    }
    w.close().unwrap();
    sigs
}

fn ascii_history(d: &SignalData) -> Vec<(u64, String)> {
    (0..d.len()).map(|i| (d.times()[i], d.get(i).to_ascii())).collect()
}

#[test]
fn signals_declared_mid_run_read_back() {
    for group_size in [1, 3, 256] {
        for background in [false, true] {
            let ctx = format!("group_size {group_size}, background {background}");
            let path = tmp(&format!("mid_run_{group_size}_{background}.vtr"));
            let sigs = write_run(&path, group_size, background);
            let rd = Reader::open(&path).unwrap();
            assert_eq!(rd.signal_count() as usize, sigs.len(), "{ctx}");
            assert!(rd.block_count() > 4, "{ctx}: {} blocks", rd.block_count());
            let last = (STEPS - 1) * 10;

            for s in &sigs {
                let ctx = format!("{ctx}, signal {} ({:?}, declared at {})", s.id.0, s.kind, s.declared_at);
                let d = rd.load_signal(s.id).unwrap();
                assert_eq!(d.initial().to_ascii(), s.default_ascii(), "{ctx}: initial");
                assert_eq!(ascii_history(&d), s.history, "{ctx}: load_signal");
                for t in 0..=last + 5 {
                    let v = rd.value_at(s.id, t).unwrap_or_else(|e| panic!("{ctx}: value_at {t}: {e}"));
                    assert_eq!(v.to_ascii(), s.value_at(t), "{ctx}: value_at {t}");
                }
                for (t0, t1) in [(0, last), (s.declared_at.saturating_sub(40), s.declared_at + 70), (5, 305), (last, last)] {
                    let got: Vec<_> = rd.changes(s.id, t0, t1).unwrap().into_iter().map(|(t, v)| (t, v.to_ascii())).collect();
                    assert_eq!(got, s.changes(t0, t1), "{ctx}: changes [{t0}, {t1}]");
                }
            }

            // Late signals mixed with early ones, with repeated ids.
            let n = sigs.len();
            let picks = [n - 1, 0, 7, n - 1, 12, 3, 22, 7, 16, n - 3];
            let ids: Vec<_> = picks.iter().map(|&i| sigs[i].id).collect();
            let loaded = rd.load_signals(&ids).unwrap();
            for (d, &i) in loaded.iter().zip(&picks) {
                assert_eq!(d.initial().to_ascii(), sigs[i].default_ascii(), "{ctx}: load_signals initial of {i}");
                assert_eq!(ascii_history(d), sigs[i].history, "{ctx}: load_signals {i}");
            }

            // The merged stream delivers every change once, in time order.
            let mut got: Vec<(u64, u32, String)> = Vec::new();
            rd.for_each_change(0, u64::MAX, |t, s, v| got.push((t, s.0, v.to_ascii()))).unwrap();
            assert!(got.windows(2).all(|p| p[0].0 <= p[1].0), "{ctx}: for_each_change order");
            got.sort();
            let mut want: Vec<(u64, u32, String)> =
                sigs.iter().flat_map(|s| s.history.iter().map(move |(t, v)| (*t, s.id.0, v.clone()))).collect();
            want.sort();
            assert_eq!(got, want, "{ctx}: for_each_change");
        }
    }
}

#[test]
fn late_declarations_survive_recovery() {
    let path = tmp("late_recovery.vtr");
    let opts = WriterOptions { block_records: 10, background: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let a = byte(&mut w, None, "a");
    let mut b = None;
    let mut want_a = Vec::new();
    let mut want_b = Vec::new();
    for step in 0..100u64 {
        w.set_time(step).unwrap();
        w.emit_u64(a, step + 1).unwrap();
        want_a.push((step, step + 1));
        if step == 30 {
            let late = w.add_scope(None, "late", ScopeType::Module, "late_m").unwrap();
            b = Some(byte(&mut w, Some(late), "b"));
        }
        if let Some(b) = b {
            w.emit_u64(b, step + 2).unwrap();
            want_b.push((step, step + 2));
        }
        if step == 97 {
            byte(&mut w, None, "tail");
        }
    }
    w.close().unwrap();
    let full = std::fs::read(&path).unwrap();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    assert!(rd.find_node(&["tail"]).is_some());
    // Cut inside the last signal block written before the chunk declaring `tail`.
    let sections = rd.sections();
    let tail_chunk = sections.iter().filter(|e| e.kind == 3).map(|e| e.offset).max().unwrap();
    let cut_block = sections.iter().filter(|e| e.kind == 4 && e.offset < tail_chunk).map(|e| e.offset).max().unwrap();
    let rd = Reader::from_bytes(full[..cut_block as usize + 30].to_vec()).unwrap();
    assert!(rd.recovered());
    assert!(rd.find_node(&["tail"]).is_none(), "nodes after the cut are absent");
    let late = rd.find_node(&["late"]).expect("late scope before the cut");
    let b = b.unwrap();
    assert_eq!(rd.find_signal("late.b", '.'), Some(b));
    let (_, end) = rd.time_range().unwrap();
    assert!(end >= 80, "recovered up to {end}");
    for (sig, want) in [(a, &want_a), (b, &want_b)] {
        let d = rd.load_signal(sig).unwrap();
        let got: Vec<_> = (0..d.len()).map(|i| (d.times()[i], d.get(i).as_u64().unwrap())).collect();
        let want: Vec<_> = want.iter().copied().filter(|(t, _)| *t <= end).collect();
        assert_eq!(got, want, "signal {}", sig.0);
        assert_eq!(rd.value_at(sig, end).unwrap().borrow().as_u64(), want.last().map(|v| v.1));
    }
    assert_eq!(rd.value_at(b, 29).unwrap().to_ascii(), "00000000");
    assert_eq!(rd.hierarchy().parent(rd.find_node(&["late", "b"]).unwrap()), Some(late));
}

#[test]
fn nodes_added_mid_run() {
    for background in [false, true] {
        let path = tmp(&format!("nodes_mid_run_{background}.vtr"));
        let opts = WriterOptions { background, block_records: 8, chunk_records: 4, ..Default::default() };
        let mut w = Writer::create_with(&path, opts).unwrap();
        let top = w.add_scope(None, "top", ScopeType::Module, "top_m").unwrap();
        let clk = byte(&mut w, Some(top), "clk");
        for t in 0..20 {
            w.set_time(t).unwrap();
            w.emit_u64(clk, t % 2 + 1).unwrap();
        }
        // Two subtrees built in interleaved order: one under `top`, one at the root.
        let env = w.add_scope(Some(top), "env", ScopeType::Class, "uvm_env").unwrap();
        let uvm = w.add_scope(None, "uvm_test_top", ScopeType::Class, "my_test").unwrap();
        let my_env = w.intern("my_env");
        w.node_attr(env, "uvm.type", Value::Str(my_env)).unwrap();
        let (count_node, count) = w.add_var(Some(env), "count", VarType::Int, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap();
        let unit = w.intern("items");
        w.node_attr(count_node, "unit", Value::Str(unit)).unwrap();
        let phase = w.add_var(Some(uvm), "phase", VarType::String, Direction::Implicit, SignalKind::VarLen).unwrap().1;
        let alias = w.add_alias(Some(uvm), "clk_seen", VarType::Wire, Direction::Implicit, clk).unwrap();
        w.node_attr(alias, "origin", Value::U64(1)).unwrap();
        let table = w.add_enum_table(Some(env), "state_t", &[("IDLE", "00"), ("BUSY", "01")]).unwrap();
        let (state_node, state) = w.add_var(Some(env), "state", VarType::Enum, Direction::Implicit, SignalKind::Bits { width: 2, states: 2 }).unwrap();
        w.node_attr(state_node, "enum_table", Value::U64(table.0 as u64)).unwrap();
        let items = w.add_stream(Some(uvm), "items", "UVM").unwrap();
        let seqr = w.intern("seqr");
        w.node_attr(items, "uvm.sequencer", Value::Str(seqr)).unwrap();
        let item = w.add_generator(items, "bus_item").unwrap();
        let bus_item = w.intern("bus_item");
        w.node_attr(item, "uvm.class", Value::Str(bus_item)).unwrap();
        let log = w.add_stream(Some(env), "log", vtr::LOG_STREAM_KIND).unwrap();
        let site = w.add_log_site(&LogSiteSpec::new(log, Severity::Info, "item {} done", &[LogArgType::U64])).unwrap();
        for t in 20..40 {
            w.set_time(t).unwrap();
            w.emit_u64(clk, t % 2 + 1).unwrap();
            w.emit_u64(count, t - 19).unwrap();
            w.emit_u64(state, t % 2).unwrap();
            if t == 20 {
                w.emit_varlen(phase, b"run").unwrap();
            }
        }
        let tx = w.begin_tx(item, 25).unwrap();
        let addr = w.intern("addr");
        w.tx_attr(tx, addr, &Value::U64(0x40)).unwrap();
        w.end_tx(tx, 30, TxStatus::Ok).unwrap();
        let rec = w.log(site, 31, &[LogArg::U64(7)]).unwrap();
        w.close().unwrap();

        let rd = Reader::open(&path).unwrap();
        let h = rd.hierarchy();
        let attr = |n: NodeId, key: &str| h.attrs(n).iter().find(|(k, _)| rd.str(*k) == key).map(|(_, v)| v.clone());
        let as_str = |v: Option<Value>| match v {
            Some(Value::Str(s)) => rd.str(s).to_string(),
            v => panic!("expected a string attribute, got {v:?}"),
        };
        assert_eq!(rd.find_node(&["top", "env"]), Some(env));
        assert_eq!(rd.find_node(&["uvm_test_top"]), Some(uvm));
        assert_eq!(h.parent(env), Some(top));
        assert_eq!(h.parent(uvm), None);
        let names = |n: Option<NodeId>| -> Vec<String> {
            let ids: Vec<NodeId> = match n {
                Some(n) => h.children(n).collect(),
                None => h.roots().collect(),
            };
            ids.into_iter().map(|c| rd.name(c).to_string()).collect()
        };
        assert_eq!(names(None), ["top", "uvm_test_top"]);
        assert_eq!(names(Some(top)), ["clk", "env"]);
        assert_eq!(names(Some(env)), ["count", "state_t", "state", "log"]);
        assert_eq!(names(Some(uvm)), ["phase", "clk_seen", "items"]);
        assert_eq!(as_str(attr(env, "uvm.type")), "my_env");
        assert_eq!(as_str(attr(count_node, "unit")), "items");
        assert_eq!(attr(alias, "origin"), Some(Value::U64(1)));
        assert_eq!(attr(state_node, "enum_table"), Some(Value::U64(table.0 as u64)));
        assert_eq!(as_str(attr(items, "uvm.sequencer")), "seqr");
        assert_eq!(as_str(attr(item, "uvm.class")), "bus_item");
        assert_eq!(h.enum_entries(table).map(|e| e.len()), Some(2));
        assert_eq!(rd.find_signal("uvm_test_top.clk_seen", '.'), Some(clk));
        assert_eq!(rd.find_signal("top.env.count", '.'), Some(count));
        assert_eq!(rd.value_at(count, 10).unwrap().to_ascii(), "00000000");
        assert_eq!(rd.value_at(count, 39).unwrap().borrow().as_u64(), Some(20));
        assert_eq!(rd.value_at(phase, 5).unwrap().to_ascii(), "");
        assert_eq!(rd.value_at(phase, 39).unwrap().to_ascii(), "run");
        assert_eq!(rd.generator_stream(item), Some(items));
        let txs = rd.transactions(&TxQuery { generator: Some(item), ..Default::default() }).unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!((txs[0].id, txs[0].begin, txs[0].end), (tx, 25, 30));
        assert_eq!(txs[0].attrs[0].value, Value::U64(0x40));
        let mut logs = Vec::new();
        rd.visit_log(&LogQuery { stream: Some(log), ..Default::default() }, |r| {
            logs.push((r.id, r.time, r.format(rd.strings())));
            true
        })
        .unwrap();
        assert_eq!(logs, [(rec, 31, "item 7 done".to_string())]);
    }
}

#[test]
fn parents_are_validated() {
    let path = tmp("parents.vtr");
    let mut w = Writer::create_with(&path, WriterOptions { background: false, ..Default::default() }).unwrap();
    let top = w.add_scope(None, "top", ScopeType::Module, "").unwrap();
    let (var, sig) = w.add_var(Some(top), "v", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 1, states: 2 }).unwrap();
    let stream = w.add_stream(Some(top), "s", "T").unwrap();
    let gen = w.add_generator(stream, "g").unwrap();
    let table = w.add_enum_table(Some(top), "e", &[("A", "0")]).unwrap();
    let unknown = NodeId(1000);
    let bits = SignalKind::Bits { width: 1, states: 2 };
    let (nodes, signals) = (w.stats().nodes, w.signal_count());
    for bad in [var, stream, gen, table, unknown] {
        assert!(w.add_scope(Some(bad), "x", ScopeType::Module, "").is_err(), "scope under {bad:?}");
        assert!(w.add_var(Some(bad), "x", VarType::Wire, Direction::Implicit, bits).is_err(), "var under {bad:?}");
        assert!(w.add_alias(Some(bad), "x", VarType::Wire, Direction::Implicit, sig).is_err(), "alias under {bad:?}");
        assert!(w.add_enum_table(Some(bad), "x", &[]).is_err(), "enum table under {bad:?}");
        assert!(w.add_stream(Some(bad), "x", "T").is_err(), "stream under {bad:?}");
    }
    for bad in [top, var, gen, table, unknown] {
        assert!(w.add_generator(bad, "x").is_err(), "generator under {bad:?}");
        let spec = LogSiteSpec::new(bad, Severity::Info, "x", &[]);
        assert!(w.add_log_site(&spec).is_err(), "log site under {bad:?}");
    }
    assert!(w.add_alias(Some(top), "x", VarType::Wire, Direction::Implicit, SignalId(99)).is_err());
    assert!(w.begin_tx(stream, 0).is_err(), "transactions need a generator");
    assert_eq!((w.stats().nodes, w.signal_count()), (nodes, signals), "rejected calls add nothing");
    w.set_time(0).unwrap();
    w.emit_u64(sig, 1).unwrap();
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    assert_eq!(rd.hierarchy().len() as u32, nodes);
    assert_eq!(rd.signal_count(), signals);
    assert!((0..rd.hierarchy().len() as u32).all(|i| rd.name(NodeId(i)) != "x"));
}

/// HIERARCHY sections and file size when 10,000 signals are declared one per
/// time step, next to the same run with every declaration up front. Run with
/// `--nocapture` for the numbers recorded in docs/RATIONALE.md.
#[test]
fn late_declaration_overhead() {
    const N: u64 = 10_000;
    let write = |late: bool, flush_every: u64| {
        let path = tmp(&format!("overhead_{late}_{flush_every}.vtr"));
        let mut w = Writer::create_with(&path, WriterOptions { background: false, ..Default::default() }).unwrap();
        let top = w.add_scope(None, "top", ScopeType::Module, "").unwrap();
        let mut sigs = Vec::new();
        if !late {
            sigs.extend((0..N).map(|i| byte(&mut w, Some(top), &format!("s{i}"))));
        }
        for t in 0..N {
            w.set_time(t).unwrap();
            if late {
                sigs.push(byte(&mut w, Some(top), &format!("s{t}")));
            }
            w.emit_u64(sigs[t as usize], 1).unwrap();
            w.emit_u64(sigs[(t / 2) as usize], t & 0xff).unwrap();
            if flush_every > 0 && t % flush_every == flush_every - 1 {
                w.flush().unwrap();
            }
        }
        w.close().unwrap();
        let rd = Reader::open(&path).unwrap();
        assert_eq!(rd.signal_count() as u64, N);
        // HIERARCHY and STRINGS sections, header included.
        let (mut n, mut bytes) = (0, 0);
        for e in rd.sections().iter().filter(|e| e.kind == 3 || e.kind == 2) {
            n += (e.kind == 3) as usize;
            bytes += e.len + 24;
        }
        (n, bytes, std::fs::metadata(&path).unwrap().len())
    };
    for flush_every in [0, 100] {
        let (n0, h0, f0) = write(false, flush_every);
        let (n1, h1, f1) = write(true, flush_every);
        println!(
            "flush every {flush_every}: up front {n0} HIERARCHY chunks, names and nodes {h0} B, file {f0} B; \
             one per step {n1} chunks, {h1} B, file {f1} B ({:+.1}%)",
            100.0 * (f1 as f64 / f0 as f64 - 1.0)
        );
        // One chunk at most per flush of values.
        assert!(n1 as u64 <= 2 + if flush_every > 0 { N / flush_every } else { 1 });
        assert!(f1 as f64 <= f0 as f64 * 1.10, "late declarations cost {f1} B against {f0} B");
    }
}
