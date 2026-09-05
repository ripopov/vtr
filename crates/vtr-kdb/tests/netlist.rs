mod common;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use vtr::*;
use vtr_kdb::{netlist::NetlistIndex, Database, Debugger};
fn fixture(name: &str) -> Database {
    Database::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}.kdb.json")),
    )
    .unwrap()
}
fn golden(name: &str, svg: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let artifacts = root.join("../../target/netlist-svg");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(artifacts.join(format!("{name}.svg")), svg).unwrap();
    let path = root.join(format!("tests/goldens/{name}.svg"));
    if std::env::var_os("VTR_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, svg).unwrap();
    }
    assert_eq!(
        svg,
        std::fs::read_to_string(path).expect("missing reviewed SVG golden"),
        "SVG regression: {name}"
    );
    assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>\n"));
    assert!(!svg.contains("NaN") && !svg.contains("inf\""));
}
fn validate_geometry(view: &vtr_kdb::netlist::LaidOutNetlist) {
    let nodes = view.geometry["children"].as_array().unwrap();
    let num = |v: &serde_json::Value, k: &str| v[k].as_f64().unwrap();
    let mut anchors = BTreeMap::new();
    for a in nodes {
        let (x, y, w, h) = (num(a, "x"), num(a, "y"), num(a, "width"), num(a, "height"));
        assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0);
        for p in a["ports"].as_array().unwrap() {
            anchors.insert(
                p["id"].as_str().unwrap(),
                (x + num(p, "x"), y + num(p, "y")),
            );
        }
        for b in nodes {
            if a["id"] == b["id"] {
                continue;
            }
            assert!(
                x + w <= num(b, "x")
                    || num(b, "x") + num(b, "width") <= x
                    || y + h <= num(b, "y")
                    || num(b, "y") + num(b, "height") <= y,
                "overlapping blocks"
            );
        }
    }
    let edges = view.geometry["edges"].as_array().unwrap();
    assert_eq!(edges.len(), view.netlist.wires.len());
    for e in edges {
        let sections = e["sections"].as_array().expect("unrouted edge");
        assert_eq!(sections.len(), 1);
        let s = &sections[0];
        let mut points = vec![&s["startPoint"]];
        if let Some(b) = s["bendPoints"].as_array() {
            points.extend(b);
        }
        points.push(&s["endPoint"]);
        for pair in points.windows(2) {
            assert!(
                (num(pair[0], "x") - num(pair[1], "x")).abs() < 0.01
                    || (num(pair[0], "y") - num(pair[1], "y")).abs() < 0.01,
                "diagonal route: {e}"
            );
        }
        for (p, side) in [(&s["startPoint"], "sources"), (&s["endPoint"], "targets")] {
            let a = anchors[e[side][0].as_str().unwrap()];
            assert!(
                (num(p, "x") - a.0).abs() < 0.01 && (num(p, "y") - a.1).abs() < 0.01,
                "detached wire"
            );
        }
    }
}
fn pipeline(name: &str, module: &str, time: u64, missing: bool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.vtr");
    common::trace(&path, false, missing);
    let db = common::db();
    let r = Reader::open(path).unwrap();
    let mut debug = Debugger::attach(&db, &r, "").unwrap();
    let view = NetlistIndex::new(&db)
        .unwrap()
        .module(module)
        .unwrap()
        .layout()
        .unwrap();
    validate_geometry(&view);
    let svg = view.svg(&mut debug, time).unwrap();
    if module == "top" {
        assert_eq!(
            view.netlist
                .blocks
                .iter()
                .filter(|b| b.child.is_some())
                .count(),
            2
        );
        assert!(!view
            .netlist
            .blocks
            .iter()
            .any(|b| b.title == "clocked process"));
        assert!(svg.contains("data-instance=\"top.u0\""));
        if time == 26 {
            assert!(svg.contains("top.q = 00000011"));
        }
    } else {
        assert!(view.netlist.blocks.iter().all(|b| b.child.is_none()));
        assert!(view
            .netlist
            .blocks
            .iter()
            .any(|b| b.title == "clocked process"));
    }
    if missing {
        assert!(svg.contains("missing trace signal: top.sel"));
    }
    golden(name, &svg);
}
macro_rules! pipeline_test {
    ($name:ident,$module:expr,$time:expr,$missing:expr) => {
        #[test]
        fn $name() {
            pipeline(stringify!($name), $module, $time, $missing);
        }
    };
}
pipeline_test!(pipeline_initial, "top", 0, false);
pipeline_test!(pipeline_reset, "top", 3, false);
pipeline_test!(pipeline_first_edge, "top", 5, false);
pipeline_test!(pipeline_hold, "top", 16, false);
pipeline_test!(pipeline_late, "top", 26, false);
pipeline_test!(pipeline_missing, "top", 26, true);
pipeline_test!(stage_reset, "top.u0", 3, false);
pipeline_test!(stage_first, "top.u0", 5, false);
pipeline_test!(stage_hold, "top.u0", 16, false);
pipeline_test!(stage_late, "top.u0", 26, false);
pipeline_test!(stage_second, "top.u1", 26, false);
pipeline_test!(outside_range, "top", 31, false);

// Deterministic recorded samples exercise display independently of RTL evaluation.
fn recorded(db: &Database, path: &Path, gap: bool, prefix: &str, missing: bool) {
    let mut w = Writer::create_with(
        path,
        WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut ids = vec![];
    if !prefix.is_empty() {
        w.begin_scope(prefix, ScopeType::Module, "");
    }
    for (name, s) in &db.symbols {
        if s.value.is_some() || (missing && name.ends_with(".a")) {
            continue;
        }
        let parts: Vec<_> = name.split('.').collect();
        for part in &parts[..parts.len() - 1] {
            w.begin_scope(part, ScopeType::Module, "");
        }
        let id = w.add_bits(parts.last().unwrap(), s.ty.width, 4).1;
        ids.push((id, s.ty.width));
        for _ in &parts[..parts.len() - 1] {
            w.end_scope().unwrap();
        }
    }
    if !prefix.is_empty() {
        w.end_scope().unwrap();
    }
    for t in [0, 10] {
        w.set_time(t).unwrap();
        for (id, width) in &ids {
            let text: Vec<_> = (0..*width)
                .map(|i| {
                    if t == 0 {
                        b"10xz"[i as usize % 4]
                    } else {
                        b"01zx"[i as usize % 4]
                    }
                })
                .collect();
            w.emit_logic_str(*id, &text).unwrap();
        }
    }
    if gap {
        w.blackout_at(3, false);
        w.blackout_at(7, true);
    }
    w.close().unwrap();
}
fn render_fixture(name: &str, design: &str, module: &str, time: u64, gap: bool, prefix: &str) {
    let db = fixture(design);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.vtr");
    recorded(&db, &path, gap, prefix, false);
    let r = Reader::open(&path).unwrap();
    let mut debug = Debugger::attach(&db, &r, prefix).unwrap();
    let index = NetlistIndex::new(&db).unwrap();
    let view = index.module(module).unwrap().layout().unwrap();
    validate_geometry(&view);
    let svg = view.svg(&mut debug, time).unwrap();
    if gap && time == 5 {
        assert!(svg.contains("dumping disabled"));
    }
    if gap && time == 8 {
        assert!(svg.contains("missing fresh sample"));
    }
    if design == "netlist" && module == "top" {
        assert_eq!(
            view.netlist
                .blocks
                .iter()
                .filter(|b| b.child.is_some())
                .count(),
            3
        );
        assert!(!view
            .netlist
            .blocks
            .iter()
            .any(|b| b.child.as_deref() == Some("top.group0.u")));
        assert!(!view
            .netlist
            .blocks
            .iter()
            .any(|b| b.title == "clocked process"));
        assert!(svg.contains("top.wide = 10xz10xz"));
    }
    if design == "edge_cases" && module == "top" {
        assert!(svg.contains("slice Simple"));
        assert!(svg.contains("&lt;&amp;name"));
        assert!(svg.contains("↔ io"));
        assert!(!svg.contains("unsupported: expression Assignment"));
        let process = view
            .netlist
            .blocks
            .iter()
            .find(|b| b.detail.contains("statement Case"))
            .unwrap();
        assert!(process
            .pins
            .iter()
            .any(|p| p.symbol.as_deref() == Some("top.sel")));
        assert!(process
            .pins
            .iter()
            .any(|p| p.symbol.as_deref() == Some("top.a")));
    }
    golden(name, &svg);
}
macro_rules! fixture_test {
    ($name:ident,$design:expr,$module:expr,$time:expr,$gap:expr,$prefix:expr) => {
        #[test]
        fn $name() {
            render_fixture(stringify!($name), $design, $module, $time, $gap, $prefix);
        }
    };
}
fixture_test!(hierarchical, "netlist", "top", 0, false, "");
fixture_test!(nested_group, "netlist", "top.group0", 0, false, "");
fixture_test!(nested_leaf, "netlist", "top.group0.u", 0, false, "");
fixture_test!(generated_leaf, "netlist", "top.lanes[1].u", 0, false, "");
fixture_test!(semantics, "semantics", "top", 0, false, "");
fixture_test!(semantics_changed, "semantics", "top", 10, false, "");
fixture_test!(dump_off, "semantics", "top", 5, true, "");
fixture_test!(dump_resumed_stale, "semantics", "top", 8, true, "");
fixture_test!(dump_refreshed, "semantics", "top", 10, true, "");
fixture_test!(wrapper_prefix, "semantics", "top", 0, false, "TOP");

#[test]
fn layout_is_reused_and_deterministic() {
    let db = fixture("pipeline");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.vtr");
    common::trace(&path, false, false);
    let r = Reader::open(path).unwrap();
    let mut d = Debugger::attach(&db, &r, "").unwrap();
    let index = NetlistIndex::new(&db).unwrap();
    let a = index.module("top").unwrap().layout().unwrap();
    let b = index.module("top").unwrap().layout().unwrap();
    assert_eq!(a.geometry, b.geometry);
    let first = a.svg(&mut d, 0).unwrap();
    let late = a.svg(&mut d, 26).unwrap();
    assert_ne!(first, late);
    assert_eq!(a.geometry, b.geometry);
    assert_eq!(first, a.svg(&mut d, 0).unwrap());
}
#[test]
fn invalid_hierarchy_and_ownership_rejected() {
    let mut db = fixture("pipeline");
    assert!(NetlistIndex::new(&db)
        .unwrap()
        .module("top.missing")
        .is_err());
    db.instances[0].parent = Some("top.u0".into());
    assert!(NetlistIndex::new(&db).err().unwrap().contains("cyclic"));
    let mut db = fixture("pipeline");
    db.symbols.get_mut("top.a").unwrap().owner = "absent".into();
    assert!(NetlistIndex::new(&db).is_err());
    let mut db = fixture("pipeline");
    db.instances[0].ports[0].symbol = "top.u0.clk".into();
    assert!(NetlistIndex::new(&db).is_err());
}
#[test]
fn cli_writes_svg_and_requires_time_output() {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.vtr");
    common::trace(&trace, false, false);
    let output = dir.path().join("netlist.svg");
    let base = || {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_vtr-kdb"));
        cmd.args([
            "netlist",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/pipeline.kdb.json"
            ),
        ])
        .arg(&trace)
        .arg("top");
        cmd
    };
    assert!(!base().output().unwrap().status.success());
    assert!(!base()
        .args(["--time", "26"])
        .output()
        .unwrap()
        .status
        .success());
    let out = base()
        .args(["--time", "26", "--output"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    golden("pipeline_late", &std::fs::read_to_string(output).unwrap());
}
fixture_test!(feedback_and_escaping, "edge_cases", "top", 0, false, "");
fixture_test!(bidirectional_child, "edge_cases", "top.u", 0, false, "");
fixture_test!(empty_module, "edge_cases", "top.empty", 0, false, "");

#[test]
fn descendants_do_not_change_parent_view() {
    let mut db = fixture("pipeline");
    let a = NetlistIndex::new(&db)
        .unwrap()
        .module("top")
        .unwrap()
        .layout()
        .unwrap();
    let template = db.instances[1].clone();
    for n in 0..2000 {
        let mut i = template.clone();
        i.path = format!("top.u0.child{n}");
        i.parent = Some("top.u0".into());
        i.ports.clear();
        db.instances.push(i);
    }
    let b = NetlistIndex::new(&db)
        .unwrap()
        .module("top")
        .unwrap()
        .layout()
        .unwrap();
    assert_eq!(a.geometry, b.geometry);
    assert_eq!(a.netlist.blocks.len(), b.netlist.blocks.len());
}
#[test]
fn old_kdb_versions_require_reexport() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("old.json");
    std::fs::write(&file, r#"{"format":"vtr-rtl-kdb","version":1}"#).unwrap();
    assert!(Database::open(file)
        .err()
        .unwrap()
        .contains("re-export RTL"));
}
