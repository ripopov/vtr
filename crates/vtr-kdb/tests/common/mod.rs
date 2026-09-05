use std::process::Command;
use vtr::*;
use vtr_kdb::Database;

pub fn db() -> Database {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if let Ok(python) = std::env::var("VTR_KDB_PYTHON") {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("design.json");
        let out = Command::new(python)
            .current_dir(&root)
            .args(["tools/kdb/export.py", "--top", "top", "-o"])
            .arg(&path)
            .arg("crates/vtr-kdb/tests/rtl/pipeline.sv")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Database::open(path).unwrap()
    } else {
        Database::open(root.join("crates/vtr-kdb/tests/fixtures/pipeline.kdb.json")).unwrap()
    }
}
pub fn trace(path: &std::path::Path, bad_width: bool, missing: bool) {
    let mut w = Writer::create(path).unwrap();
    w.begin_scope("top", ScopeType::Module, "top");
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
            w.add_bits(name, if bad_width && name == "a" { 7 } else { width }, 4)
                .1,
        );
    }
    for (name, input, output) in [("u0", 6, 7), ("u1", 7, 8)] {
        w.begin_scope(name, ScopeType::Module, "stage");
        for (name, id) in [
            ("clk", 0),
            ("rst_n", 1),
            ("en", 2),
            ("d", input),
            ("q", output),
        ] {
            w.add_alias(name, VarType::Logic, Direction::Implicit, ids[id])
                .unwrap();
        }
        w.end_scope().unwrap();
    }
    w.end_scope().unwrap();
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
