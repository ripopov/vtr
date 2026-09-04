//! Converter tests: every input format the CLI accepts round-trips into a
//! readable VTR file with the expected content.

use std::path::{Path, PathBuf};
use vtr::{Reader, TxQuery, TxStatus, Value, Writer, WriterOptions};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn writer(path: &Path) -> Writer {
    let mut o = WriterOptions::default();
    o.background = false;
    Writer::create_with(path, o).unwrap()
}

#[test]
fn ftr_fixture() {
    let out = tmp("tiny.vtr");
    let mut w = writer(&out);
    vtr_cli::ftr::convert_ftr(fixture("tiny_tlm.ftr").to_str().unwrap(), &mut w).unwrap();
    w.close().unwrap();
    let r = Reader::open(&out).unwrap();
    assert_eq!(r.tx_counts(), (120, 116));
    assert_eq!(r.streams().count(), 9);
    assert_eq!(r.generators().count(), 13);
    assert_eq!(r.meta().timescale, -9);
    let all = r.transactions(&TxQuery::default()).unwrap();
    let t = &all[0];
    // FTR ids are preserved as attributes and the three attribute phases survive.
    let names: Vec<(&str, vtr::AttrPhase)> = t.attrs.iter().map(|a| (r.str(a.key), a.phase)).collect();
    assert!(names.contains(&("ftr.id", vtr::AttrPhase::Begin)));
    assert!(names.iter().any(|(n, p)| *n == "addr" && *p == vtr::AttrPhase::Begin));
    assert!(names.iter().any(|(n, p)| *n == "response" && *p == vtr::AttrPhase::End));
    assert!(t.attrs.iter().any(|a| matches!(a.value, Value::Str(_))));
    assert!(t.attrs.iter().any(|a| matches!(a.value, Value::U64(_))));
    // Relations reference converted ids and carry the FTR stream ids.
    let rels = r.relations_from(t.id).unwrap();
    assert!(!rels.is_empty() || !r.relations_to(t.id).unwrap().is_empty());
    let mut kinds: Vec<String> = Vec::new();
    r.visit_relations(|rel| {
        let k = r.str(rel.kind).to_string();
        if !kinds.contains(&k) {
            kinds.push(k);
        }
        assert!(rel.attrs.iter().any(|(k, _)| r.str(*k) == "ftr.from_stream"));
        true
    })
    .unwrap();
    kinds.sort();
    assert_eq!(kinds, vec!["parent_of", "pred"]);
}

#[test]
fn otlp_json_fixture() {
    let out = tmp("otlp.vtr");
    let mut w = writer(&out);
    vtr_cli::otlp::convert_otlp_json(fixture("otlp_sample.json").to_str().unwrap(), &mut w).unwrap();
    w.close().unwrap();
    let r = Reader::open(&out).unwrap();
    assert_eq!(r.tx_counts(), (2, 2));
    let h = r.hierarchy();
    let res = h.roots().next().unwrap();
    assert_eq!(h.node(res).attrs.iter().filter(|(k, _)| r.str(*k) == "service.name").count(), 1);
    assert!(h.node(res).attrs.iter().any(|(_, v)| matches!(v, Value::List(l) if l.len() == 2)));
    let server = r.transaction(1).unwrap().unwrap();
    assert_eq!(server.kind, vtr::TxKind::Server);
    assert_eq!(server.status, TxStatus::Error);
    assert_eq!(server.begin, 1544712660000000000);
    assert_eq!(server.end, 1544712661500000000);
    assert_eq!(server.events.len(), 1);
    assert_eq!(r.str(server.events[0].name), "cache.miss");
    assert!(server.attrs.iter().any(|a| r.str(a.key) == "otel.status_message"));
    assert!(server.attrs.iter().any(|a| r.str(a.key) == "otel.trace_id" && matches!(&a.value, Value::Bytes(b) if b.len() == 16)));
    assert!(server.attrs.iter().any(|a| r.str(a.key) == "nested" && matches!(a.value, Value::Map(_))));
    assert!(server.attrs.iter().any(|a| r.str(a.key) == "net.peer.port" && a.value == Value::F64(8080.5)));
    let client = r.transaction(2).unwrap().unwrap();
    assert_eq!(client.kind, vtr::TxKind::Client);
    assert_eq!(client.parent, Some(1));
    assert_eq!(client.status, TxStatus::Ok);
    let links = r.relations_from(2).unwrap();
    assert_eq!(links.len(), 2);
    assert!(links.iter().any(|l| l.to == 1));
    assert!(links.iter().any(|l| l.to == 0)); // link to a span outside the file
}

#[test]
fn kanata_sample() {
    let src = root().join("ext/Konata/docs/kanata-sample-2.log.gz");
    if !src.exists() {
        eprintln!("skipping: submodule ext/Konata not checked out");
        return;
    }
    let out = tmp("kanata.vtr");
    let mut w = writer(&out);
    vtr_cli::kanata::convert_kanata(src.to_str().unwrap(), &mut w).unwrap();
    w.close().unwrap();
    let r = Reader::open(&out).unwrap();
    let (ntx, _) = r.tx_counts();
    assert_eq!(ntx, 4041);
    assert_eq!(r.streams().count(), 1);
    let t = r.transaction(1).unwrap().unwrap();
    assert!(!t.stages.is_empty());
    let stage_names: Vec<&str> = t.stages.iter().map(|s| r.str(s.name)).collect();
    assert!(stage_names.contains(&"F"));
    assert!(t.attrs.iter().any(|a| r.str(a.key) == "insn_id_in_sim"));
    assert!(t.attrs.iter().any(|a| r.str(a.key) == "retire_id"));
    // Labels get their escaped newlines back.
    let detail = t.attrs.iter().find(|a| r.str(a.key) == "detail").map(|a| match a.value {
        Value::Str(s) => r.str(s).to_string(),
        _ => String::new(),
    });
    assert!(detail.map(|d| d.contains('\n')).unwrap_or(false));
    assert!(r.meta().attrs.iter().any(|(k, _)| r.str(*k) == "kanata.start_cycle"));
}

#[test]
fn fst_fixture_parity() {
    let src = root().join("ext/wavepeek/tests/fixtures/hand/verilator_pack_array.fst");
    if !src.exists() {
        eprintln!("skipping: submodule ext/wavepeek not checked out");
        return;
    }
    let out = tmp("pack_array.vtr");
    let mut w = writer(&out);
    vtr_cli::fst::convert_fst(src.to_str().unwrap(), &mut w, &vtr_cli::fst::FstConvertOptions { states: None, progress: false }).unwrap();
    w.close().unwrap();
    let r = Reader::open(&out).unwrap();
    // Compare every change against fst-reader.
    let f = std::fs::File::open(&src).unwrap();
    let mut fst = fst_reader::FstReader::open(std::io::BufReader::new(f)).unwrap();
    let hdr = fst.get_header();
    assert_eq!(r.meta().timescale, hdr.timescale_exponent);
    let mut expected: Vec<(u64, u32, String)> = Vec::new();
    fst.read_signals(&fst_reader::FstFilter::all(), |t, h, v| -> Result<(), ()> {
        let s = match v {
            fst_reader::FstSignalValue::String(s) => String::from_utf8_lossy(s).into_owned(),
            fst_reader::FstSignalValue::Real(f) => f.to_string(),
        };
        expected.push((t, (h.get_index()) as u32, s));
        Ok(())
    })
    .unwrap();
    let mut got: Vec<(u64, u32, String)> = Vec::new();
    r.for_each_change(0, u64::MAX, |t, s, v| got.push((t, s.0, v.to_ascii()))).unwrap();
    // The FST dump may repeat unchanged values; VTR drops those, so compare after dedup per signal.
    let mut last: std::collections::HashMap<u32, String> = Default::default();
    let mut exp_dedup = Vec::new();
    for (t, s, v) in expected {
        if last.get(&s) != Some(&v) {
            last.insert(s, v.clone());
            exp_dedup.push((t, s, v));
        }
    }
    let mut exp_sorted = exp_dedup.clone();
    exp_sorted.sort();
    let mut got_sorted = got.clone();
    got_sorted.sort();
    // Values that equal the initial default (all-x / 0 for two-state) at their first change are
    // also dropped by VTR; filter those from the expectation.
    let exp_final: Vec<_> = exp_sorted
        .into_iter()
        .filter(|(_, s, v)| {
            let k = r.hierarchy().signal_kind(vtr::SignalId(*s)).unwrap();
            let default = {
                let mut d = Vec::new();
                vtr::signal::default_value(k, &mut d);
                vtr::block::frame_value(k, &d).to_ascii()
            };
            !(got_sorted.iter().all(|(_, gs, _)| gs != s) && *v == default)
        })
        .collect();
    assert_eq!(got_sorted.len(), exp_final.len(), "change counts differ");
    for (a, b) in got_sorted.iter().zip(exp_final.iter()) {
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
        assert_eq!(a.2, b.2, "value mismatch for signal {} at {}", a.1, a.0);
    }
    // Hierarchy attributes carried over from FST (pack/array attributes are what this fixture is about).
    let n_attr = r.hierarchy().attr_count();
    assert!(n_attr > 0, "expected FST attributes to be preserved");
}

#[test]
fn cli_end_to_end() {
    // Exercises the binary: convert, info, hier, value, changes, dump, tx.
    let exe = env!("CARGO_BIN_EXE_vtr");
    let src = fixture("otlp_sample.json");
    let out = tmp("cli.vtr");
    let st = std::process::Command::new(exe).args(["convert", src.to_str().unwrap(), out.to_str().unwrap()]).status().unwrap();
    assert!(st.success());
    for args in [vec!["info"], vec!["hier", "--vars"], vec!["tx", "--max", "5"], vec!["tx", "--id", "2"], vec!["dump"]] {
        let mut a: Vec<&str> = vec![args[0], out.to_str().unwrap()];
        a.extend(&args[1..]);
        let o = std::process::Command::new(exe).args(&a).output().unwrap();
        assert!(o.status.success(), "vtr {:?} failed: {}", a, String::from_utf8_lossy(&o.stderr));
    }
    let o = std::process::Command::new(exe).args(["tx", out.to_str().unwrap(), "--id", "999"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let o = std::process::Command::new(exe).args(["info", "/nonexistent.vtr"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn vcd_export_counts_every_change() {
    use vtr::{Direction, ScopeType, SignalKind, VarType};
    let out = tmp("vcdout.vtr");
    let mut w = writer(&out);
    w.begin_scope("top", ScopeType::Module, "");
    // 4-state: a two-state signal starts at 0 in VTR, so an initial 0 would not be a change.
    let (_, clk) = w.add_var("clk", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 1, states: 4 });
    let (_, bus) = w.add_var("bus [7:0]", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 4 });
    let (_, r) = w.add_var("r", VarType::Real, Direction::Implicit, SignalKind::Real);
    w.add_alias("clk_alias", VarType::Wire, Direction::Implicit, clk).unwrap();
    w.end_scope().unwrap();
    let mut n = 0u64;
    for t in 0..50u64 {
        w.set_time(t).unwrap();
        w.emit_bit(clk, (t & 1) as u8).unwrap();
        n += 1;
        if t % 3 == 0 {
            w.emit_u64(bus, t).unwrap();
            n += 1;
        }
        if t % 7 == 0 {
            w.emit_real(r, (t as f64 + 1.0) / 2.0).unwrap(); // reals start at 0.0 implicitly
            n += 1;
        }
    }
    w.close().unwrap();
    let vcd = tmp("vcdout.vcd");
    let written = vtr_cli::vcdout::vtr_to_vcd(out.to_str().unwrap(), vcd.to_str().unwrap()).unwrap();
    assert_eq!(written, n);
    let st = vtr_cli::vcdout::vcd_stats(vcd.to_str().unwrap()).unwrap();
    assert_eq!(st.changes, n);
    // Aliases share the signal's id code: three signals, the alias is not a fourth.
    assert_eq!(st.per_signal.len(), 3);
    assert_eq!(st.per_signal["top.clk"], 50);
    assert_eq!(st.per_signal["top.bus[7:0]"], 17);
    assert_eq!(st.per_signal["top.r"], 8);
    assert!(vtr_cli::vcdout::compare(&st, &st).identical());
}

#[test]
fn vcd_export_matches_fst_source() {
    // FST -> VTR -> VCD holds exactly the changes of FST -> VCD.
    let src = root().join("ext/wavepeek/tests/fixtures/hand/verilator_pack_array.fst");
    if !src.exists() {
        eprintln!("skipping: submodule ext/wavepeek not checked out");
        return;
    }
    let out = tmp("pack_array_x.vtr");
    let mut w = writer(&out);
    vtr_cli::fst::convert_fst(src.to_str().unwrap(), &mut w, &vtr_cli::fst::FstConvertOptions { states: None, progress: false }).unwrap();
    w.close().unwrap();
    let (a, b) = (tmp("pack_array_fst.vcd"), tmp("pack_array_vtr.vcd"));
    let na = vtr_cli::vcdout::fst_to_vcd(src.to_str().unwrap(), a.to_str().unwrap()).unwrap();
    let nb = vtr_cli::vcdout::vtr_to_vcd(out.to_str().unwrap(), b.to_str().unwrap()).unwrap();
    assert_eq!(na, nb);
    let sa = vtr_cli::vcdout::vcd_stats(a.to_str().unwrap()).unwrap();
    let sb = vtr_cli::vcdout::vcd_stats(b.to_str().unwrap()).unwrap();
    let c = vtr_cli::vcdout::compare(&sa, &sb);
    assert!(c.identical(), "mismatches: {:?}", c.mismatches);
    assert!(sa.changes > 0);
}
