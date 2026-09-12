use vtr::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn shared_signal_histories_preserve_order_and_lifetimes() {
    let path = tmp("shared_histories.vtr");
    let opts = WriterOptions { block_records: 12, group_size: 2, background: false, dedup: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, bits) = w.add_bits("bits", 8, 4);
    let (_, quiet) = w.add_bits("quiet", 8, 4);
    let (_, text) = w.add_var("text", VarType::String, Direction::Implicit, SignalKind::VarLen);
    let (_, real) = w.add_var("real", VarType::Real, Direction::Implicit, SignalKind::Real);
    w.add_alias("alias", VarType::Wire, Direction::Implicit, bits).unwrap();
    for i in 0..40u64 {
        // Two updates at each time must retain their emission order.
        w.set_time(10 + i / 2).unwrap();
        w.emit_u64(bits, i).unwrap();
        w.emit_varlen(text, format!("value-{i}").as_bytes()).unwrap();
        w.emit_real(real, i as f64 + 0.5).unwrap();
    }
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    assert!(rd.block_count() > 1);
    assert!(rd.load_signals(&[]).unwrap().is_empty());
    assert!(matches!(rd.load_signals(&[bits, SignalId(u32::MAX), bits]), Err(Error::Invalid(_))));
    let alias = rd.find_signal("alias", '.').unwrap();
    let ids = [text, alias, quiet, real, bits, text, quiet, real];
    let loaded = rd.load_signals(&ids).unwrap();
    assert_eq!(loaded.len(), ids.len());
    for (d, id) in loaded.iter().zip(ids) {
        let expected = rd.load_signal(id).unwrap();
        assert_eq!(d.kind(), expected.kind());
        assert_eq!(d.initial(), expected.initial());
        assert_eq!(d.times(), expected.times());
        for i in 0..d.len() {
            assert_eq!(d.get(i), expected.get(i));
        }
    }
    for (a, b) in [(0, 5), (1, 4), (3, 7)] {
        assert!(std::ptr::eq(loaded[a].times(), loaded[b].times()), "duplicate histories must share time storage");
    }
    match (loaded[0].get(17), loaded[5].get(17)) {
        (SignalValue::VarLen(a), SignalValue::VarLen(b)) => assert!(std::ptr::eq(a, b)),
        _ => panic!("expected variable-length values"),
    }
    match (loaded[1].get(17), loaded[4].get(17)) {
        (SignalValue::Bits { data: a, .. }, SignalValue::Bits { data: b, .. }) => assert!(std::ptr::eq(a, b)),
        _ => panic!("expected packed bit values"),
    }
    assert!(loaded[2].is_empty());
    assert_eq!(loaded[2].value_at(100).to_ascii(), "xxxxxxxx");
    assert_eq!(loaded[1].value_at(9).to_ascii(), "xxxxxxxx");
    assert_eq!(loaded[1].index_at(10), Some(1));
    assert_eq!(loaded[1].value_at(10).as_u64(), Some(1));
    let survivor = loaded[0].clone();
    assert!(std::ptr::eq(survivor.times(), loaded[0].times()));
    drop(rd);
    drop(loaded);
    std::thread::spawn(move || {
        assert_eq!(survivor.len(), 40);
        assert_eq!(survivor.value_at(29), SignalValue::VarLen(b"value-39"));
    }).join().unwrap();
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
            assert_eq!(d.times()[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v, "cnt change {i}");
        }
        assert_eq!(d.initial().to_ascii(), "xxxxxxxx");
        let d = rd.load_signal(wide).unwrap();
        assert_eq!(d.len(), exp_wide.len());
        for (i, (t, v)) in exp_wide.iter().enumerate() {
            assert_eq!(d.times()[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v, "wide change {i}");
        }
        let d = rd.load_signal(r).unwrap();
        for (i, (t, v)) in exp_r.iter().enumerate() {
            assert_eq!(d.times()[i], *t);
            assert_eq!(d.get(i), SignalValue::Real(*v));
        }
        assert_eq!(d.len(), exp_r.len());
        let d = rd.load_signal(s).unwrap();
        for (i, (t, v)) in exp_s.iter().enumerate() {
            assert_eq!(d.times()[i], *t);
            assert_eq!(d.get(i).to_ascii(), *v);
        }
        let d = rd.load_signal(clk).unwrap();
        assert_eq!(d.len(), exp_clk.len());
        for (i, (t, v)) in exp_clk.iter().enumerate() {
            assert_eq!(d.times()[i], *t);
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
        let v = rd.load_signals(&[a, b, c, d, e, b, c, e]).unwrap();
        for (x, y) in [(1, 5), (2, 6), (4, 7)] {
            assert!(std::ptr::eq(v[x].times(), v[y].times()));
        }
        (v[0].clone(), v[1].clone(), v[2].clone(), v[3].clone(), v[4].clone())
    };
    assert_eq!(da.len(), 5000);
    assert_eq!(da.times(), db.times());
    assert!((0..da.len()).all(|i| da.get(i) == db.get(i)));
    assert_eq!(dc.len(), 5000);
    assert_ne!(da.get(4998).to_ascii(), dc.get(4998).to_ascii());
    assert!((0..dd.len()).all(|i| dd.get(i) == de.get(i)));
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

#[test]
fn dictionary_coded_columns_roundtrip() {
    // Signals whose values come from small sets (FSM states, opcodes, handshake
    // codes) are dictionary-coded (transform 4); a bus that never repeats is not.
    // Both must read back exactly through every reader path.
    let path = tmp("dict.vtr");
    let opts = WriterOptions { background: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, st) = w.add_bits("state", 16, 2);
    let (_, op) = w.add_bits("opcode", 32, 2);
    let (_, wide) = w.add_bits("tag", 184, 2); // 23-byte entries, 9 distinct values
    let (_, bus) = w.add_bits("bus", 32, 2);
    let states = [0x0001u64, 0x0010, 0x0100, 0x1000, 0x8000];
    let mut lcg = 0x1234_5678_9abc_def0u64;
    let mut expect: Vec<(u64, u64, u64, u64)> = Vec::new();
    for i in 0..40_000u64 {
        w.set_time(i * 5).unwrap();
        lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let s = states[((lcg >> 50) % 5) as usize];
        let o = 0xdead_0000 + (lcg >> 60); // 16 distinct values
        let t = ((lcg >> 40) % 9) * 0x0101_0101;
        let b = lcg >> 32;
        w.emit_u64(st, s).unwrap();
        w.emit_u64(op, o).unwrap();
        let mut words = [0u32; 6];
        words[0] = t as u32;
        words[5] = (t as u32) ^ 0xff;
        w.emit_words(wide, &words).unwrap();
        w.emit_u64(bus, b).unwrap();
        expect.push((s, o, t, b));
    }
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    let v = rd.load_signals(&[st, op, wide, bus]).unwrap();
    // Duplicate suppression drops repeated values, so compare against the deduplicated stream.
    let mut prev = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
    let mut n = [0usize; 4];
    for (i, &(s, o, t, b)) in expect.iter().enumerate() {
        let time = i as u64 * 5;
        for (k, (val, was)) in [(s, prev.0), (o, prev.1), (t, prev.2), (b, prev.3)].into_iter().enumerate() {
            if val != was {
                assert_eq!(v[k].times()[n[k]], time, "signal {k} change {}", n[k]);
                let a = v[k].get(n[k]).to_ascii();
                let low = u64::from_str_radix(&a[a.len().saturating_sub(64)..], 2).unwrap();
                assert_eq!(low, val, "signal {k} at {time}"); // the 184-bit tag's low word is `t`
                n[k] += 1;
            }
        }
        prev = (s, o, t, b);
    }
    for k in 0..4 {
        assert_eq!(v[k].len(), n[k]);
    }
    assert_eq!(rd.value_at(st, 5 * 12).unwrap().as_ascii_u64(), expect[12].0);
    assert_eq!(rd.changes(op, 1000, 1200).unwrap().len(), v[1].times().iter().filter(|&&t| (1000..=1200).contains(&t)).count());
    let mut streamed = [0usize; 4];
    rd.for_each_change(0, u64::MAX, |_, s, _| streamed[s.0 as usize] += 1).unwrap();
    assert_eq!(streamed, n);
    // The low-cardinality columns must actually have been dictionary-coded.
    let st = rd.run_stats().unwrap();
    assert!(st[4].0 > 0, "no dictionary-coded run: {st:?}");
}

#[test]
fn logs_roundtrip() {
    let path = tmp("log.vtr");
    // Small blocks so that several log blocks and transaction blocks interleave.
    let opts = WriterOptions { tx_block_bytes: 3000, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let soc = w.begin_scope("soc", ScopeType::Generic, "");
    let cpu = w.begin_scope("cpu0", ScopeType::Core, "");
    let log = w.add_log_stream(Some(cpu), "log");
    w.end_scope().unwrap();
    let bus = w.add_stream(Some(soc), "bus", "TRANSACTOR");
    let gen_rd = w.add_generator(bus, "read");
    let buslog = w.add_log_stream(Some(soc), "buslog");
    w.end_scope().unwrap();
    let s_fetch = w.add_log_site(&LogSiteSpec::new(log, Severity::Debug, "fetch pc={:#x} inst={:#010x}", &[LogArgType::U64, LogArgType::U64]).names(&["pc", "inst"]).location("cpu.cpp", 42).func("fetch"));
    let s_warn = w.add_log_site(&LogSiteSpec::new(log, Severity::Warn, "{}: stall {} cycles ({:.1}%)", &[LogArgType::Text, LogArgType::I64, LogArgType::F64]));
    let s_plain = w.add_log_site(&LogSiteSpec::new(buslog, Severity::Info, "bus idle", &[]));
    let s_err = w.add_log_site(&LogSiteSpec::new(buslog, Severity::Error, "{} bad {} at {} {}", &[LogArgType::Bool, LogArgType::Bytes, LogArgType::Time, LogArgType::Pointer]));
    let interned = w.intern("slave0");
    let s_str = w.add_log_site(&LogSiteSpec::new(buslog, Severity::Info, "target {}", &[LogArgType::Str]));
    let fetch_node = w.log_site_node(s_fetch).unwrap();
    let mut expect: Vec<(u64, u64, String, u8)> = Vec::new(); // (id, time, text, severity)
    let mut tx_ids = Vec::new();
    for i in 0..1000u64 {
        let t = i * 10;
        let id = w.log(s_fetch, t, &[LogArg::U64(0x8000_0000 + i * 4), LogArg::U64(0x0040_0093 ^ i)]).unwrap();
        expect.push((id, t, format!("fetch pc={:#x} inst={:#010x}", 0x8000_0000u64 + i * 4, 0x0040_0093u64 ^ i), 1));
        if i % 7 == 0 {
            let unit = if i % 2 == 0 { "lsu" } else { "alu" };
            let id = w.log(s_warn, t + 1, &[unit.into(), (i as i64 % 5 - 2).into(), (i as f64 / 7.0).into()]).unwrap();
            expect.push((id, t + 1, format!("{}: stall {} cycles ({:.1}%)", unit, i as i64 % 5 - 2, i as f64 / 7.0), 3));
        }
        if i % 100 == 0 {
            let tx = w.begin_tx(gen_rd, t).unwrap();
            w.tx_attr(tx, interned, AttrPhase::Begin, &Value::Text(format!("unique text {i}"))).unwrap();
            let id = w.log_with_parent(s_plain, t + 2, Some(tx), &[]).unwrap();
            expect.push((id, t + 2, "bus idle".into(), 2));
            let id = w.log(s_err, t + 3, &[LogArg::Bool(true), LogArg::Bytes(&[0xde, 0xad]), LogArg::Time(t), LogArg::Pointer(0x1000)]).unwrap();
            expect.push((id, t + 3, format!("true bad dead at {t} 0x1000"), 4));
            let id = w.log(s_str, t + 4, &[LogArg::Str(interned)]).unwrap();
            expect.push((id, t + 4, "target slave0".into(), 2));
            w.end_tx(tx, t + 5, TxStatus::Ok).unwrap();
            tx_ids.push(tx);
        }
    }
    // Type errors are reported, not silently accepted.
    assert!(w.log(s_fetch, 0, &[LogArg::U64(1)]).is_err());
    assert!(w.log(s_fetch, 0, &[LogArg::I64(1), LogArg::U64(1)]).is_err());
    assert!(w.log(LogSiteId(99), 0, &[]).is_err());
    let st = w.stats();
    assert_eq!(st.log_records, expect.len() as u64);
    assert_eq!(st.transactions, tx_ids.len() as u64);
    w.close().unwrap();

    let r = Reader::open(&path).unwrap();
    assert_eq!(r.version(), (1, 1));
    assert!(r.log_block_count() > 1, "expected several log blocks, got {}", r.log_block_count());
    assert_eq!(r.log_count(), expect.len() as u64);
    assert_eq!(r.tx_counts().0, expect.len() as u64 + tx_ids.len() as u64);
    // Sites.
    assert_eq!(r.log_sites().len(), 5);
    let site = r.log_site(fetch_node).unwrap();
    assert_eq!(r.str(site.fmt), "fetch pc={:#x} inst={:#010x}");
    assert_eq!(site.severity, Severity::Debug);
    assert_eq!(site.args, vec![LogArgType::U64, LogArgType::U64]);
    assert_eq!(site.names.iter().map(|n| r.str(*n)).collect::<Vec<_>>(), vec!["pc", "inst"]);
    assert_eq!((r.str(site.file.unwrap()), site.line, r.str(site.func.unwrap())), ("cpu.cpp", Some(42), "fetch"));
    assert_eq!(r.full_path(site.stream, "."), "soc.cpu0.log");
    let warn_site = r.log_sites().iter().find(|s| s.severity == Severity::Warn).unwrap();
    assert_eq!(warn_site.names.iter().map(|n| r.str(*n)).collect::<Vec<_>>(), vec!["0", "1", "2"]);
    // All records, in order, formatted.
    let mut got = Vec::new();
    r.visit_log(&LogQuery::default(), |rec| {
        got.push((rec.id, rec.time, rec.format(r.strings()), rec.severity().code()));
        true
    })
    .unwrap();
    assert_eq!(got.len(), expect.len());
    assert_eq!(got, expect);
    // Severity filter prunes to warnings and errors only.
    let mut n_warn = 0;
    let mut n_err = 0;
    r.visit_log(&LogQuery { min_severity: Severity::Warn, ..Default::default() }, |rec| {
        match rec.severity() {
            Severity::Warn => n_warn += 1,
            Severity::Error => n_err += 1,
            s => panic!("unexpected severity {s:?}"),
        }
        true
    })
    .unwrap();
    assert_eq!((n_warn, n_err), (expect.iter().filter(|e| e.3 == 3).count(), 10));
    // Stream and window filters.
    let mut n = 0;
    r.visit_log(&LogQuery { stream: Some(buslog), window: Some((0, 1005)), ..Default::default() }, |rec| {
        assert!(rec.time <= 1005);
        n += 1;
        true
    })
    .unwrap();
    assert_eq!(n, 6); // t=2,3,4 and t=1002,1003,1004
    // Parent link and the transaction view.
    let mut parents = 0;
    r.visit_log(&LogQuery { generator: Some(r.log_sites()[2].node), ..Default::default() }, |rec| {
        assert!(tx_ids.contains(&rec.parent.unwrap()));
        parents += 1;
        true
    })
    .unwrap();
    assert_eq!(parents, 10);
    let as_tx = r.transaction(expect[1].0).unwrap().unwrap();
    assert_eq!((as_tx.begin, as_tx.end, as_tx.generator), (expect[1].1, expect[1].1, warn_site.node));
    assert_eq!(as_tx.attrs.len(), 3);
    assert_eq!(r.str(as_tx.attrs[0].key), "0");
    assert!(matches!(&as_tx.attrs[0].value, Value::Text(s) if s == "lsu"));
    assert_eq!(as_tx.attrs[1].value, Value::I64(-2));
    // visit_transactions merges both kinds in file order and sees the Text attribute.
    let mut n_tx = 0;
    let mut n_log_as_tx = 0;
    let mut texts = 0;
    r.visit_transactions(&TxQuery::default(), |tx| {
        if r.log_site(tx.generator).is_some() {
            n_log_as_tx += 1;
        } else {
            n_tx += 1;
            if let Value::Text(s) = &tx.attrs[0].value {
                assert!(s.starts_with("unique text "));
                texts += 1;
            }
        }
        true
    })
    .unwrap();
    assert_eq!((n_tx, n_log_as_tx, texts), (10, expect.len(), 10));
    let mut only_bus = 0;
    r.visit_transactions(&TxQuery { stream: Some(buslog), ..Default::default() }, |_| {
        only_bus += 1;
        true
    })
    .unwrap();
    assert_eq!(only_bus, 30);
}


#[test]
fn log_raw_matches_log() {
    // The pre-encoded path (used by the C++ header) must produce the same block bytes as `log`.
    let a = tmp("log_a.vtr");
    let b = tmp("log_b.vtr");
    let opts = WriterOptions { background: false, ..Default::default() };
    let types = [LogArgType::Text, LogArgType::I64, LogArgType::U64, LogArgType::F64, LogArgType::Bool, LogArgType::Bytes];
    for (path, raw) in [(&a, false), (&b, true)] {
        let mut w = Writer::create_with(path, opts.clone()).unwrap();
        let st = w.add_log_stream(None, "log");
        let site = w.add_log_site(&LogSiteSpec::new(st, Severity::Info, "{} {} {} {} {} {}", &types));
        for i in 0..300u64 {
            let text = if i % 3 == 0 { "alpha" } else { "beta" };
            let args = [LogArg::Text(text), LogArg::I64(-(i as i64) * 1000), LogArg::U64(i << 40), LogArg::F64(i as f64 * 0.25), LogArg::Bool(i % 2 == 0), LogArg::Bytes(&[i as u8, 7])];
            if raw {
                let mut row = Vec::new();
                row.extend_from_slice(text.as_bytes().len().to_le_bytes().first().map(|_| ()).map(|_| Vec::<u8>::new()).unwrap_or_default().as_slice());
                // Row encoding: text = varint len + bytes; i64 zig-zag varint; u64 varint; f64 8 bytes; bool 1 byte; bytes = varint len + bytes.
                let mut put = |out: &mut Vec<u8>, mut v: u64| {
                    while v >= 0x80 {
                        out.push((v as u8) | 0x80);
                        v >>= 7;
                    }
                    out.push(v as u8);
                };
                put(&mut row, text.len() as u64);
                row.extend_from_slice(text.as_bytes());
                let z = -(i as i64) * 1000;
                put(&mut row, ((z << 1) ^ (z >> 63)) as u64);
                put(&mut row, i << 40);
                row.extend_from_slice(&(i as f64 * 0.25).to_le_bytes());
                row.push((i % 2 == 0) as u8);
                put(&mut row, 2);
                row.extend_from_slice(&[i as u8, 7]);
                w.log_raw(site, i * 3, if i == 5 { Some(2) } else { None }, &row).unwrap();
            } else {
                w.log_with_parent(site, i * 3, if i == 5 { Some(2) } else { None }, &args).unwrap();
            }
        }
        w.close().unwrap();
    }
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let r = Reader::open(&b).unwrap();
    let mut n = 0;
    r.visit_log(&LogQuery::default(), |rec| {
        if rec.id == 6 {
            assert_eq!(rec.parent, Some(2));
        }
        n += 1;
        true
    })
    .unwrap();
    assert_eq!(n, 300);
}
