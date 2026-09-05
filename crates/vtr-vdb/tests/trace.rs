use std::process::Command;
use vtr::*;
use vtr_vdb::{Database, Debugger, TraceNode};

mod common;
use common::{db, trace};

fn flatten<'a>(n: &'a TraceNode, v: &mut Vec<&'a TraceNode>) {
    v.push(n);
    for c in &n.children {
        flatten(c, v);
    }
}
#[test]
fn pipeline_crosses_previous_events_and_enable_holds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.vtr");
    trace(&path, false, false);
    let db = db();
    let reader = Reader::open(&path).unwrap();
    let mut d = Debugger::attach(&db, &reader, "").unwrap();
    let tree = d.trace("top.q", 26, 14).unwrap();
    let out = tree.render();
    println!("{out}");
    assert!(out.contains("top.q = 00000011 @ 26"));
    assert!(out.contains("hold at 15"));
    assert!(out.contains("PosEdge top.u0.clk @ 5, event #1"));
    assert!(out.contains("pipeline.sv:5:"));
    assert!(!out.contains("mismatch"), "{out}");
    assert!(!out.contains("ambiguous"), "{out}");
    let mut nodes = vec![];
    flatten(&tree, &mut nodes);
    assert!(nodes
        .iter()
        .any(|n| n.symbol == "top.a" && n.at.time == 5 && n.at.before && n.value == "00000011"));
    assert!(!nodes.iter().any(|n| n.symbol == "top.b"));
    assert!(nodes
        .iter()
        .any(|n| n.symbol == "top.en" && n.at.time == 15 && n.at.before && n.value == "0"));
}
#[test]
fn mux_reset_depth_and_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.vtr");
    trace(&path, false, false);
    let db = db();
    let reader = Reader::open(&path).unwrap();
    let mut d = Debugger::attach(&db, &reader, "").unwrap();
    let mux = d.trace("top.mux", 24, 4).unwrap().render();
    assert!(mux.contains("top.b = 00001001"));
    assert!(!mux.contains("top.a ="));
    assert!(mux.contains("mux control: false branch"));
    let reset = d.trace("top.u0.q", 3, 4).unwrap().render();
    assert!(reset.contains("NegEdge top.u0.rst_n @ 2"), "{reset}");
    assert!(!reset.contains("top.u0.d ="));
    let depth = d.trace("top.q", 26, 0).unwrap();
    assert!(depth.children.is_empty());
    assert_eq!(depth.notes, ["depth limit"]);
    let out = Command::new(env!("CARGO_BIN_EXE_vtr-vdb"))
        .args([
            "trace",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/pipeline.vdb.json"
            ),
        ])
        .arg(&path)
        .args(["top.mux", "--time", "24", "--depth", "1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.starts_with("time unit: 1e"));
    assert!(
        stdout.contains("  top.sel = 0 @ 24 [mux control: false branch]"),
        "{stdout}"
    );
    assert!(stdout.contains("  top.b = 00001001 @ 24 [data]"));
    assert!(!stdout.contains("top.a ="));
}
#[test]
fn mapping_and_missing_data_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.vtr");
    let db = db();
    trace(&path, true, false);
    let r = Reader::open(&path).unwrap();
    assert!(Debugger::attach(&db, &r, "")
        .err()
        .unwrap()
        .contains("design mismatch at top.a"));
    trace(&path, false, true);
    let r = Reader::open(&path).unwrap();
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    assert!(d
        .diagnostics
        .iter()
        .any(|s| s.contains("missing trace signal: top.sel")));
    assert!(d
        .trace("top.mux", 24, 4)
        .unwrap()
        .render()
        .contains("missing trace signal: top.sel"));
    assert!(d
        .trace("top.q", 100, 4)
        .unwrap()
        .render()
        .contains("missing trace at 100"));
    assert!(d
        .trace("top.q", 0, 4)
        .unwrap()
        .render()
        .contains("missing prior assignment event"));
    assert!(d.trace("typo", 0, 1).is_err());
    assert!(d.trace("top.q", 0, 129).is_err());
    assert!(Debugger::attach(&db, &r, "wrong")
        .err()
        .unwrap()
        .contains("no VDB signals match"));
}

#[test]
fn procedural_order_arithmetic_sync_reset_and_nba_last_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.vtr");
    let mut w = Writer::create(&path).unwrap();
    w.begin_scope("top", ScopeType::Module, "top");
    let names = [
        ("clk", 1),
        ("rst", 1),
        ("en", 1),
        ("sel", 1),
        ("a", 8),
        ("b", 8),
        ("y", 8),
        ("q", 8),
        ("z", 8),
        ("ordered", 8),
        ("tmp", 8),
    ];
    let ids: Vec<_> = names.iter().map(|(n, b)| w.add_bits(n, *b, 4).1).collect();
    w.end_scope().unwrap();
    for (t, values) in [
        (0, [0, 1, 1, 0, 7, 4, 8, 0, 64, 0, 8]),
        (5, [1, 1, 1, 0, 7, 4, 8, 0, 64, 4, 8]),
        (10, [0, 0, 1, 0, 7, 4, 8, 0, 64, 4, 8]),
        (15, [1, 0, 1, 0, 7, 4, 8, 8, 64, 4, 8]),
        (20, [0, 0, 1, 1, 7, 4, 4, 8, 64, 4, 8]),
        (25, [1, 0, 1, 1, 7, 4, 4, 4, 64, 4, 8]),
        (30, [0, 0, 1, 1, 7, 4, 4, 4, 64, 4, 8]),
    ] {
        w.set_time(t).unwrap();
        for (id, v) in ids.iter().zip(values) {
            w.emit_u64(*id, v).unwrap();
        }
    }
    w.close().unwrap();
    let db = Database::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/semantics.vdb.json"
    ))
    .unwrap();
    let r = Reader::open(&path).unwrap();
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    let y = d.trace("top.y", 14, 3).unwrap().render();
    assert!(y.contains("top.a = 00000111"), "{y}");
    assert!(
        y.contains("top.sel = 0"),
        "later false overwrite must retain control: {y}"
    );
    assert!(
        !y.contains("top.tmp ="),
        "blocking temporary should substitute its true dependencies"
    );
    assert!(!y.contains("mismatch"), "{y}");
    let selected = d.trace("top.y", 24, 3).unwrap().render();
    assert!(selected.contains("top.b ="));
    assert!(!selected.contains("top.a ="));
    let ordered = d.trace("top.ordered", 26, 3).unwrap().render();
    assert!(ordered.contains("top.b = 00000100 @ 25-"), "{ordered}");
    assert!(!ordered.contains("top.a ="));
    assert!(!ordered.contains("mismatch"));
    let reset = d.trace("top.q", 6, 3).unwrap().render();
    assert!(reset.contains("top.rst = 1 @ 5-"), "{reset}");
    assert!(!reset.contains("top.y ="));
    let z = d.trace("top.z", 24, 3).unwrap().render();
    assert!(z.contains("top.a ="));
    assert!(z.contains("top.b ="));
    assert!(!z.contains("mismatch"), "{z}");
}

#[test]
fn unknown_controls_multiple_drivers_and_blackouts_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.vtr");
    trace(&path, false, false);
    let mut db = db();
    let r = Reader::open(&path).unwrap();
    let p = db
        .processes
        .iter()
        .find(|p| p.targets == ["top.mux"])
        .unwrap()
        .clone();
    db.processes.push(p);
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    assert!(d
        .trace("top.mux", 24, 4)
        .unwrap()
        .render()
        .contains("ambiguous: 2"));
    db.processes.pop();
    let mut w = Writer::create_with(
        dir.path().join("x.vtr"),
        WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    w.begin_scope("top", ScopeType::Module, "top");
    let (_, sel) = w.add_bits("sel", 1, 4);
    let (_, mux) = w.add_bits("mux", 8, 4);
    w.end_scope().unwrap();
    w.set_time(0).unwrap();
    w.emit_logic_str(sel, b"x").unwrap();
    w.emit_u64(mux, 0).unwrap();
    w.set_time(10).unwrap();
    w.emit_u64(mux, 1).unwrap();
    w.close().unwrap();
    let r = Reader::open(dir.path().join("x.vtr")).unwrap();
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    let out = d.trace("top.mux", 5, 4).unwrap().render();
    assert!(out.contains("ambiguous mux control (X/Z)"), "{out}");
}

#[test]
fn dump_gaps_and_design_identity_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gaps.vtr");
    let db = db();
    let mut w = Writer::create_with(
        &path,
        WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    w.begin_scope("top", ScopeType::Module, "top");
    let ids: Vec<_> = [("sel", 1), ("a", 8), ("b", 8), ("mux", 8)]
        .iter()
        .map(|(n, b)| w.add_bits(n, *b, 4).1)
        .collect();
    w.end_scope().unwrap();
    for t in [0, 10] {
        w.set_time(t).unwrap();
        for (id, v) in ids.iter().zip([0, 3, 9, 9]) {
            w.emit_u64(*id, v).unwrap();
        }
    }
    w.blackout_at(3, false);
    w.blackout_at(7, true);
    w.close().unwrap();
    let r = Reader::open(&path).unwrap();
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    assert!(d
        .trace("top.mux", 5, 3)
        .unwrap()
        .render()
        .contains("dumping disabled"));
    assert!(d
        .trace("top.mux", 8, 3)
        .unwrap()
        .render()
        .contains("missing fresh sample after dump gap"));
    assert!(d
        .trace("top.mux", 10, 3)
        .unwrap()
        .render()
        .contains("top.b = 00001001"));
    let path = dir.path().join("identity.vtr");
    let mut w = Writer::create(&path).unwrap();
    let id = w.intern("different-design");
    w.set_file_attr("design.vdb_id", Value::Str(id)).unwrap();
    w.begin_scope("top", ScopeType::Module, "top");
    w.add_bits("a", 8, 4);
    w.end_scope().unwrap();
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    assert!(Debugger::attach(&db, &r, "")
        .err()
        .unwrap()
        .contains("design.vdb_id differs"));
}

#[test]
fn asynchronous_release_at_clock_is_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.vtr");
    let db = db();
    let mut w = Writer::create(&path).unwrap();
    w.begin_scope("top", ScopeType::Module, "top");
    w.begin_scope("u0", ScopeType::Module, "stage");
    let ids: Vec<_> = [("clk", 1), ("rst_n", 1), ("en", 1), ("d", 8), ("q", 8)]
        .iter()
        .map(|(n, b)| w.add_bits(n, *b, 4).1)
        .collect();
    w.end_scope().unwrap();
    w.end_scope().unwrap();
    for (t, values) in [
        (0, [0, 0, 1, 3, 0]),
        (5, [1, 1, 1, 3, 3]),
        (10, [0, 1, 1, 3, 3]),
    ] {
        w.set_time(t).unwrap();
        for (id, v) in ids.iter().zip(values) {
            w.emit_u64(*id, v).unwrap();
        }
    }
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    let out = d.trace("top.u0.q", 6, 3).unwrap().render();
    assert!(
        out.contains(
            "ambiguous scheduling: non-triggering clock/reset transition on top.u0.rst_n at 5"
        ),
        "{out}"
    );
}
