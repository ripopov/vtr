use vtr::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn signals_roundtrip_multi_block() {
    for background in [false, true] {
        let path = tmp(&format!("sig_{background}.vtr"));
        // Small blocks and groups exercise many block boundaries.
        let opts = WriterOptions { block_records: 50, group_size: 3, background, ..Default::default() };
        let mut w = Writer::create_with(&path, opts).unwrap();
        w.set_timescale(-12).unwrap();
        w.set_comment("hello").unwrap();
        let s_test = w.intern("test");
        w.set_file_attr("tool", Value::Str(s_test)).unwrap();
        let top = w.begin_scope("top", ScopeType::Module, "top_mod");
        let (_, clk) = w.add_var("clk", VarType::Wire, Direction::Input, SignalKind::Bits { width: 1, states: 4 });
        let (_, cnt) = w.add_var("cnt", VarType::Reg, Direction::Implicit, SignalKind::Bits { width: 8, states: 4 });
        let (_, wide) = w.add_var("wide", VarType::Logic, Direction::Output, SignalKind::Bits { width: 100, states: 4 });
        let (_, r) = w.add_var("r", VarType::Real, Direction::Implicit, SignalKind::Real);
        let (_, s) = w.add_var("s", VarType::String, Direction::Implicit, SignalKind::VarLen);
        let (_, two) = w.add_var("two", VarType::Bit, Direction::Implicit, SignalKind::Bits { width: 40, states: 2 });
        let (_, nine) = w.add_var("nine", VarType::Logic, Direction::Implicit, SignalKind::Bits { width: 6, states: 9 });
        let (_, ev) = w.add_var("ev", VarType::Event, Direction::Implicit, SignalKind::Bits { width: 1, states: 2 });
        let (_, quiet) = w.add_var("quiet", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 4, states: 4 });
        let alias = w.add_alias("clk_alias", VarType::Wire, Direction::Implicit, clk).unwrap();
        w.node_attr(alias, "note", Value::U64(7)).unwrap();
        w.end_scope().unwrap();
        let _ = top;
        // Expected values.
        let mut exp_cnt: Vec<(u64, String)> = Vec::new();
        let mut exp_wide: Vec<(u64, String)> = Vec::new();
        let mut exp_r: Vec<(u64, f64)> = Vec::new();
        let mut exp_s: Vec<(u64, String)> = Vec::new();
        let mut exp_clk: Vec<(u64, String)> = Vec::new();
        for i in 0..400u64 {
            let t = i * 5;
            w.set_time(t).unwrap();
            w.emit_bit(clk, (i & 1) as u8).unwrap();
            exp_clk.push((t, format!("{}", i & 1)));
            if i % 3 == 0 {
                let v = (i * 7) & 0xff;
                w.emit_u64(cnt, v).unwrap();
                exp_cnt.push((t, format!("{:08b}", v)));
            }
            if i % 7 == 0 {
                let mut sv = String::new();
                for b in 0..100 {
                    sv.push(match (b + i) % 5 {
                        0 => '0',
                        1 => '1',
                        2 => 'x',
                        3 => 'z',
                        _ => '1',
                    });
                }
                w.emit_logic_str(wide, sv.as_bytes()).unwrap();
                exp_wide.push((t, sv));
            }
            if i % 11 == 0 {
                w.emit_real(r, i as f64 * 0.5).unwrap();
                if i > 0 {
                    // 0.0 at time 0 equals the initial value and is deduplicated.
                    exp_r.push((t, i as f64 * 0.5));
                }
            }
            if i % 13 == 0 {
                let st = format!("str{i}");
                w.emit_varlen(s, st.as_bytes()).unwrap();
                exp_s.push((t, st));
            }
            if i % 17 == 0 {
                w.emit_words(two, &[i as u32, 0xff]).unwrap();
            }
            if i % 19 == 0 {
                w.emit_logic_str(nine, b"uwlh-1").unwrap();
            }
            if i % 23 == 0 {
                w.emit_bit(ev, 1).unwrap();
            }
            if i == 100 {
                w.emit_u64(quiet, 0xA).unwrap();
            }
            if i == 200 {
                w.dump_off();
            }
            if i == 210 {
                w.dump_on();
            }
        }
        // Duplicate suppression: same value again is dropped.
        w.set_time(2000).unwrap();
        w.emit_u64(cnt, (399 / 3 * 3 * 7) & 0xff).unwrap();
        let stats = w.stats();
        w.close().unwrap();
        assert!(stats.blocks >= 5, "expected several blocks, got {}", stats.blocks);

        let rd = Reader::open(&path).unwrap();
        assert!(!rd.recovered());
        assert_eq!(rd.meta().timescale, -12);
        assert_eq!(rd.meta().comment, "hello");
        assert_eq!(rd.str(rd.meta().attrs[0].0), "tool");
        assert_eq!(rd.time_range(), Some((0, 2000)));
        assert_eq!(rd.blackout().len(), 2);
        assert_eq!(rd.blackout()[0], Blackout { time: 1000, active: false });
        assert_eq!(rd.signal_count(), 9);
        let h = rd.hierarchy();
        assert_eq!(h.len(), 11);
        let roots: Vec<_> = h.roots().collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(rd.name(roots[0]), "top");
        assert_eq!(h.children(roots[0]).count(), 10);
        assert_eq!(rd.find_signal("top.clk_alias", '.'), Some(clk));
        assert_eq!(rd.full_path(alias, "."), "top.clk_alias");
        assert_eq!(h.node(alias).attrs[0].1, Value::U64(7));

        // load_signal parity.
        let d = rd.load_signal(cnt).unwrap();
        assert_eq!(d.len(), exp_cnt.len());
        for (i, (t, v)) in exp_cnt.iter().enumerate() {
            assert_eq!(d.times[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v, "cnt change {i}");
        }
        assert_eq!(d.initial().to_ascii(), "xxxxxxxx");
        let d = rd.load_signal(wide).unwrap();
        assert_eq!(d.len(), exp_wide.len());
        for (i, (t, v)) in exp_wide.iter().enumerate() {
            assert_eq!(d.times[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v, "wide change {i}");
        }
        let d = rd.load_signal(r).unwrap();
        for (i, (t, v)) in exp_r.iter().enumerate() {
            assert_eq!(d.times[i], *t);
            assert_eq!(d.get(i), SignalValue::Real(*v));
        }
        assert_eq!(d.len(), exp_r.len());
        let d = rd.load_signal(s).unwrap();
        for (i, (t, v)) in exp_s.iter().enumerate() {
            assert_eq!(d.times[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v);
        }
        let d = rd.load_signal(clk).unwrap();
        assert_eq!(d.len(), exp_clk.len());
        for (i, (t, v)) in exp_clk.iter().enumerate() {
            assert_eq!(d.times[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v);
        }
        let d = rd.load_signal(nine).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d.get(0).to_ascii(), "uwlh-1");
        let d = rd.load_signal(two).unwrap();
        assert_eq!(d.get(1).as_u64(), Some(17 | (0xff << 32)));
        // value_at
        assert_eq!(rd.value_at(cnt, 0).unwrap().to_ascii(), "00000000");
        assert_eq!(rd.value_at(cnt, 14).unwrap().to_ascii(), "00000000");
        assert_eq!(rd.value_at(cnt, 15).unwrap().to_ascii(), format!("{:08b}", (3 * 7) & 0xff));
        assert_eq!(rd.value_at(cnt, 30).unwrap().to_ascii(), format!("{:08b}", (6 * 7) & 0xff));
        assert_eq!(rd.value_at(cnt, 5000).unwrap().to_ascii(), format!("{:08b}", (399 / 3 * 3 * 7) & 0xff));
        assert_eq!(rd.value_at(quiet, 0).unwrap().to_ascii(), "xxxx");
        assert_eq!(rd.value_at(quiet, 499).unwrap().to_ascii(), "xxxx");
        assert_eq!(rd.value_at(quiet, 500).unwrap().to_ascii(), "1010");
        assert_eq!(rd.value_at(quiet, 1999).unwrap().to_ascii(), "1010");
        assert_eq!(rd.value_at(quiet, 2000).unwrap().to_ascii(), "1010");
        assert_eq!(rd.value_at(r, 54).unwrap(), OwnedSignalValue::Real(0.0));
        assert_eq!(rd.value_at(r, 55).unwrap(), OwnedSignalValue::Real(5.5));
        assert_eq!(rd.value_at(s, 0).unwrap().to_ascii(), "str0");
        assert_eq!(rd.value_at(s, 64).unwrap().to_ascii(), "str0");
        assert_eq!(rd.value_at(s, 65).unwrap().to_ascii(), "str13");
        // changes window
        let c = rd.changes(clk, 100, 120).unwrap();
        assert_eq!(c.len(), 5);
        assert_eq!(c[0].0, 100);
        assert_eq!(c[4].0, 120);
        // global time table
        let tt = rd.time_table().unwrap();
        assert_eq!(tt.len(), 401);
        assert_eq!(tt[400], 2000);
        assert!(tt.windows(2).all(|w| w[0] < w[1]));
        // for_each_change
        let mut n = 0;
        let mut last_t = 0;
        rd.for_each_change(0, u64::MAX, |t, _, _| {
            assert!(t >= last_t);
            last_t = t;
            n += 1;
        })
        .unwrap();
        assert_eq!(n as u64, stats.records);
    }
}

#[test]
fn transactions_roundtrip() {
    let path = tmp("tx.vtr");
    let opts = WriterOptions { tx_block_bytes: 2000, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let sc = w.begin_scope("cpu", ScopeType::Core, "");
    let st = w.add_stream(Some(sc), "pipe", "TRANSACTOR");
    let gen_i = w.add_generator(st, "instruction");
    let st2 = w.add_stream(None, "bus", "TRANSACTOR");
    let gen_b = w.add_generator(st2, "read");
    w.end_scope().unwrap();
    let k_pc = w.intern("pc");
    let k_lbl = w.intern("label");
    let k_dep = w.intern("wakeup");
    let lane0 = w.intern("0");
    let s_f = w.intern("F");
    let s_x = w.intern("X");
    let e_ret = w.intern("retire");
    let s_add = w.intern("add");
    let mut ids = Vec::new();
    for i in 0..500u64 {
        let t = w.begin_tx(gen_i, i).unwrap();
        w.tx_attr(t, k_pc, AttrPhase::Begin, &Value::U64(0x1000 + i * 4)).unwrap();
        w.tx_attr(t, k_lbl, AttrPhase::Record, &Value::Str(s_add)).unwrap();
        w.tx_stage_begin(t, s_f, lane0, i).unwrap();
        w.tx_stage_begin(t, s_x, lane0, i + 1).unwrap();
        w.tx_stage_attr(t, k_lbl, &Value::I64(-3)).unwrap();
        w.tx_event(t, i + 2, e_ret, &[(k_pc, Value::Bool(true))]).unwrap();
        if i > 0 {
            w.relate(k_dep, ids[i as usize - 1], t, &[]).unwrap();
            w.set_tx_parent(t, ids[0]).unwrap();
        }
        if i % 50 == 0 {
            let b = w.begin_tx(gen_b, i).unwrap();
            w.set_tx_kind(b, TxKind::Client).unwrap();
            w.set_tx_parent(b, t).unwrap();
            w.end_tx(b, i + 10, TxStatus::Ok).unwrap();
        }
        w.end_tx(t, i + 3, if i % 5 == 0 { TxStatus::Aborted } else { TxStatus::Unset }).unwrap();
        ids.push(t);
    }
    let open = w.begin_tx(gen_i, 600).unwrap();
    w.close().unwrap();

    let rd = Reader::open(&path).unwrap();
    assert!(rd.tx_block_count() > 3, "blocks: {}", rd.tx_block_count());
    assert_eq!(rd.tx_counts(), (511, 499));
    let streams: Vec<_> = rd.streams().collect();
    assert_eq!(streams.len(), 2);
    let all = rd.transactions(&TxQuery::default()).unwrap();
    assert_eq!(all.len(), 511);
    let t7 = rd.transaction(ids[7]).unwrap().unwrap();
    assert_eq!(t7.begin, 7);
    assert_eq!(t7.end, 10);
    assert_eq!(t7.generator, gen_i);
    assert_eq!(t7.parent, Some(ids[0]));
    assert_eq!(t7.attrs.len(), 2);
    assert_eq!(t7.attrs[0].phase, AttrPhase::Begin);
    assert_eq!(t7.attrs[0].value, Value::U64(0x1000 + 28));
    assert_eq!(rd.str(t7.attrs[1].key), "label");
    assert_eq!(t7.stages.len(), 2);
    assert_eq!(t7.stages[0].end, Some(8));
    assert_eq!(t7.stages[1].end, Some(10)); // closed at tx end
    assert_eq!(t7.stages[1].attrs[0].1, Value::I64(-3));
    assert_eq!(t7.events[0].time, 9);
    assert_eq!(rd.str(t7.events[0].name), "retire");
    assert_eq!(t7.status, TxStatus::Unset);
    assert_eq!(rd.transaction(ids[10]).unwrap().unwrap().status, TxStatus::Aborted);
    let o = rd.transaction(open).unwrap().unwrap();
    assert_eq!(o.status, TxStatus::Open);
    let rf = rd.relations_from(ids[7]).unwrap();
    assert_eq!(rf.len(), 1);
    assert_eq!(rf[0].to, ids[8]);
    let rt = rd.relations_to(ids[7]).unwrap();
    assert_eq!(rt.len(), 1);
    assert_eq!(rt[0].from, ids[6]);
    assert_eq!(rd.str(rt[0].kind), "wakeup");
    let bus = rd.transactions(&TxQuery { stream: Some(st2), ..Default::default() }).unwrap();
    assert_eq!(bus.len(), 10);
    assert_eq!(bus[0].kind, TxKind::Client);
    let win = rd.transactions(&TxQuery { window: Some((100, 105)), generator: Some(gen_i), ..Default::default() }).unwrap();
    // begin in 97..=105 overlap [100,105]
    assert_eq!(win.len(), 9);
    assert_eq!(rd.time_range(), Some((0, 600)));
}

#[test]
fn recovery_without_directory() {
    let path = tmp("crash.vtr");
    let opts = WriterOptions { block_records: 10, background: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, a) = w.add_bits("a", 8, 2);
    for i in 0..100u64 {
        w.set_time(i).unwrap();
        w.emit_u64(a, i).unwrap();
    }
    w.close().unwrap();
    let full = std::fs::read(&path).unwrap();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    assert_eq!(rd.load_signal(a).unwrap().len(), 100);
    // Chop off the directory + trailer + part of the last block.
    // Find the last signal block and cut inside it (directory and trailer are gone too).
    let last_block_off = rd.sections().iter().filter(|e| e.kind == 4).map(|e| e.offset).max().unwrap() as usize;
    let truncated = full[..last_block_off + 30].to_vec();
    let rd = Reader::from_bytes(truncated).unwrap();
    assert!(rd.recovered());
    let d = rd.load_signal(a).unwrap();
    assert!(d.len() >= 80 && d.len() < 100, "{}", d.len());
    assert_eq!(d.get(50).as_u64(), Some(50));
    // Garbage is rejected.
    assert!(Reader::from_bytes(b"not a vtr file at all".to_vec()).is_err());
}

#[test]
fn errors() {
    let path = tmp("err.vtr");
    let mut w = Writer::create(&path).unwrap();
    let (_, a) = w.add_bits("a", 4, 2);
    w.set_time(10).unwrap();
    assert!(w.set_time(5).is_err());
    assert!(w.emit_logic_str(a, b"1x").is_err());
    assert!(w.emit_real(a, 1.0).is_err());
    assert!(w.emit_u64(SignalId(99), 1).is_err());
    assert!(w.end_tx(5, 1, TxStatus::Ok).is_err());
    assert!(w.end_scope().is_err());
    w.emit_u64(a, 3).unwrap();
    w.flush().unwrap();
    assert!(w.set_timescale(-6).is_err());
    w.close().unwrap();
}

#[test]
fn long_columns_with_checkpoints() {
    let path = tmp("ckpt.vtr");
    let opts = WriterOptions { background: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, a) = w.add_bits("a", 16, 4);
    let (_, b) = w.add_bits("b", 1, 4);
    let (_, c) = w.add_var("c", VarType::Real, Direction::Implicit, SignalKind::Real);
    for i in 0..20_000u64 {
        w.set_time(i * 3).unwrap();
        w.emit_u64(a, i & 0xffff).unwrap();
        if i % 2 == 0 {
            w.emit_bit(b, ((i / 2) & 1) as u8).unwrap();
        }
        if i % 5 == 0 {
            w.emit_real(c, i as f64).unwrap();
        }
    }
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    assert_eq!(rd.block_count(), 1);
    for t in [0u64, 1, 2, 3, 767, 768, 769, 3000, 30_000, 59_997, 59_998, 100_000] {
        let i = (t / 3).min(19_999);
        assert_eq!(rd.value_at(a, t).unwrap().as_ascii_u64(), i & 0xffff, "a at {t}");
        let ib = (i / 2) * 2; // last even index <= i
        assert_eq!(rd.value_at(b, t).unwrap().to_ascii(), format!("{}", (ib / 2) & 1), "b at {t}");
        let ic = (i / 5) * 5;
        assert_eq!(rd.value_at(c, t).unwrap(), OwnedSignalValue::Real(ic as f64), "c at {t}");
    }
    let ch = rd.changes(a, 30_000, 30_030).unwrap();
    assert_eq!(ch.len(), 11);
    assert_eq!(ch[0].0, 30_000);
    let mut n = 0u64;
    rd.for_each_change(0, u64::MAX, |_, _, _| n += 1).unwrap();
    assert_eq!(n, 20_000 + 10_000 + 4_000 - 1); // c at time 0 equals the initial 0.0 and is deduplicated
}

trait AsciiU64 {
    fn as_ascii_u64(&self) -> u64;
}
impl AsciiU64 for OwnedSignalValue {
    fn as_ascii_u64(&self) -> u64 {
        u64::from_str_radix(&self.to_ascii(), 2).unwrap()
    }
}

#[test]
fn dynamic_aliasing_of_identical_columns() {
    let path = tmp("alias.vtr");
    let opts = WriterOptions { background: false, group_size: 2, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, a) = w.add_bits("a", 1, 4);
    let (_, b) = w.add_bits("b", 1, 4); // identical to a
    let (_, c) = w.add_bits("c", 1, 4); // identical to a except the last change
    let (_, d) = w.add_bits("d", 16, 4); // identical to e
    let (_, e) = w.add_bits("e", 16, 4);
    for i in 0..5000u64 {
        w.set_time(i * 2).unwrap();
        let bit = (i & 1) as u8;
        w.emit_bit(a, bit).unwrap();
        w.emit_bit(b, bit).unwrap();
        w.emit_bit(c, if i == 4998 { 2 } else { bit }).unwrap(); // an X breaks the identity
        w.emit_u64(d, i * 7).unwrap();
        w.emit_u64(e, i * 7).unwrap();
    }
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    let (da, db, dc, dd, de) = {
        let v = rd.load_signals(&[a, b, c, d, e]).unwrap();
        (v[0].clone(), v[1].clone(), v[2].clone(), v[3].clone(), v[4].clone())
    };
    assert_eq!(da.len(), 5000);
    assert_eq!(da.times, db.times);
    assert_eq!(da.data, db.data);
    assert_eq!(dc.len(), 5000);
    assert_ne!(da.get(4998).to_ascii(), dc.get(4998).to_ascii());
    assert_eq!(dd.data, de.data);
    assert_eq!(rd.value_at(b, 1234).unwrap().to_ascii(), "1");
    assert_eq!(rd.value_at(e, 1000).unwrap().as_ascii_u64(), 500 * 7);
    assert_eq!(rd.changes(b, 100, 110).unwrap().len(), 6);
    let mut n = [0u64; 5];
    rd.for_each_change(0, u64::MAX, |_, s, _| n[s.0 as usize] += 1).unwrap();
    assert_eq!(n, [5000, 5000, 5000, 5000, 5000]);
    // The file must actually contain aliases: it should be much smaller than 5 independent columns.
    let size = std::fs::metadata(&path).unwrap().len();
    let opts = WriterOptions { background: false, ..Default::default() };
    let path1 = tmp("alias_one.vtr");
    let mut w = Writer::create_with(&path1, opts).unwrap();
    let (_, x) = w.add_bits("x", 16, 4);
    for i in 0..5000u64 {
        w.set_time(i * 2).unwrap();
        w.emit_u64(x, i * 7).unwrap();
    }
    w.close().unwrap();
    let _ = size;
    let _ = x;
}

#[test]
fn stream_delivers_same_step_repeats_in_order() {
    // With dedup off, a signal may change several times in one time step; every
    // streaming path must deliver all of them, in emission order.
    for n_sigs in [4u32, 40_000] {
        let path = tmp(&format!("repeat_{n_sigs}.vtr"));
        let opts = WriterOptions { dedup: false, background: false, ..Default::default() };
        let mut w = Writer::create_with(&path, opts).unwrap();
        let sigs: Vec<SignalId> = (0..n_sigs).map(|i| w.add_var(&format!("s{i}"), VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).1).collect();
        let mut expected: Vec<(u64, u32, u64)> = Vec::new();
        for t in 0..20u64 {
            w.set_time(t * 10).unwrap();
            for (i, &s) in sigs.iter().enumerate().take(64) {
                let reps = if (i as u64 + t) % 3 == 0 { 3 } else { 1 };
                for k in 0..reps {
                    w.emit_u64(s, (t * 4 + k) & 0xff).unwrap();
                    expected.push((t * 10, s.0, (t * 4 + k) & 0xff));
                }
            }
        }
        w.close().unwrap();
        let rd = Reader::open(&path).unwrap();
        let mut got: Vec<(u64, u32, u64)> = Vec::new();
        rd.for_each_change(0, u64::MAX, |t, s, v| got.push((t, s.0, v.as_u64().unwrap()))).unwrap();
        assert_eq!(got.len(), expected.len(), "n_sigs {n_sigs}");
        // Same (time, signal) pairs in the same order; signal order within a step is free.
        let key = |v: &[(u64, u32, u64)]| {
            let mut k = v.to_vec();
            k.sort_by_key(|e| (e.0, e.1));
            k
        };
        assert_eq!(key(&got), key(&expected), "n_sigs {n_sigs}");
    }
}
