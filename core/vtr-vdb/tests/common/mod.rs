use std::process::Command;
use vtr::*;
use vtr_vdb::Database;

pub fn db() -> Database {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if let Ok(python) = std::env::var("VTR_VDB_PYTHON") {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.json");
        let out = Command::new(python)
            .current_dir(&root)
            .args(["integrations/slang/export.py", "--top", "top", "-o"])
            .arg(&path)
            .arg("core/vtr-vdb/tests/rtl/pipeline.sv")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Database::open(path).unwrap()
    } else {
        Database::open(root.join("core/vtr-vdb/tests/fixtures/pipeline.vdb.json")).unwrap()
    }
}
pub fn trace(path: &std::path::Path, bad_width: bool, missing: bool) {
    let mut w = Writer::create(path).unwrap();
    let top = w.add_scope(None, "top", ScopeType::Module, "top").unwrap();
    let mut ids = vec![];
    for (name, width) in [
        ("clk", 1),
        ("rst_n", 1),
        ("en", 1),
        ("sel", 1),
        ("a", 8),
        ("b", 8),
        ("mux", 8),
        ("first", 8),
        ("q", 8),
    ] {
        let name = if missing && name == "sel" {
            "not_sel"
        } else {
            name
        };
        ids.push(
            w.add_var(Some(top), name, VarType::Wire, Direction::Implicit, SignalKind::Bits { width: if bad_width && name == "a" { 7 } else { width }, states: 4 }).unwrap()
                .1,
        );
    }
    for (name, input, output) in [("u0", 6, 7), ("u1", 7, 8)] {
        let scope = w.add_scope(Some(top), name, ScopeType::Module, "stage").unwrap();
        for (name, id) in [
            ("clk", 0),
            ("rst_n", 1),
            ("en", 2),
            ("d", input),
            ("q", output),
        ] {
            w.add_alias(Some(scope), name, VarType::Logic, Direction::Implicit, ids[id])
                .unwrap();
        }
    }
    // Independent explicit simulation snapshots. Inputs change away from edges.
    for (t, values) in [
        (0, [0, 1, 1, 1, 3, 9, 3, 0, 0]),
        (2, [0, 0, 1, 1, 3, 9, 3, 0, 0]), // asynchronous reset
        (4, [0, 1, 1, 1, 3, 9, 3, 0, 0]),
        (5, [1, 1, 1, 1, 3, 9, 3, 3, 0]), // first=3, q=old first=0
        (10, [0, 1, 1, 1, 3, 9, 3, 3, 0]),
        (12, [0, 1, 0, 0, 3, 9, 9, 3, 0]),
        (15, [1, 1, 0, 0, 3, 9, 9, 3, 0]), // disabled: both hold
        (20, [0, 1, 0, 0, 3, 9, 9, 3, 0]),
        (22, [0, 1, 1, 0, 3, 9, 9, 3, 0]),
        (25, [1, 1, 1, 0, 3, 9, 9, 9, 3]), // q gets first from time 5
        (30, [0, 1, 1, 0, 3, 9, 9, 9, 3]),
    ] {
        w.set_time(t).unwrap();
        for (id, value) in ids.iter().zip(values) {
            w.emit_u64(*id, value).unwrap();
        }
    }
    w.close().unwrap();
}
