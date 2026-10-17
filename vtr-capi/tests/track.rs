//! C++ and `SystemVerilog` keyed tracker integration tests.

#![cfg(target_os = "linux")]

use std::fmt::Write as _;

use std::path::{Path, PathBuf};
use std::process::Command;

use vtr::{Reader, TxQuery, Value};

mod support;

fn dump(path: &Path) -> String {
    let r = Reader::open(path).unwrap();
    let mut txs = r.transactions(&TxQuery::default()).unwrap();
    txs.sort_by_key(|t| t.id);
    let val = |v: &Value| match v {
        Value::U64(u) => format!("{u:#x}"),
        Value::Str(s) => format!("{:?}", r.str(*s)),
        Value::Time(t) => format!("{t}t"),
        other => format!("{other:?}"),
    };
    let mut out = String::new();
    for t in &txs {
        let path = r.full_path(t.generator, ".");
        write!(
            out,
            "{} {path} [{}..{}] {}",
            t.id,
            t.begin,
            t.end,
            t.status.name()
        )
        .unwrap();
        if let Some(p) = t.parent {
            write!(out, " parent={p}").unwrap();
        }
        out += "\n";
        for a in &t.attrs {
            writeln!(out, "  {}={}", r.str(a.key), val(&a.value)).unwrap();
        }
        for e in &t.events {
            writeln!(out, "  event {}@{}", r.str(e.name), e.time).unwrap();
        }
        for s in &t.stages {
            let end = s.end.map_or_else(|| "open".into(), |e| e.to_string());
            writeln!(
                out,
                "  {}@{} {}..{end}",
                r.str(s.name),
                r.str(s.lane),
                s.begin
            )
            .unwrap();
        }
        for rel in r.relations_from(t.id).unwrap() {
            writeln!(out, "  -> {} {}", r.str(rel.kind), rel.to).unwrap();
        }
    }
    out
}

#[test]
fn tracker_and_package() {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    assert!(
        Command::new(&cxx)
            .arg("--version")
            .output()
            .expect("C++ compiler is required")
            .status
            .success()
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lib = support::static_library();
    let temporary = tempfile::tempdir().unwrap();
    let out_dir = temporary.path();
    let exe = out_dir.join("track_test");
    let st = Command::new(&cxx)
        .args(["-std=c++17", "-Wall", "-Wextra", "-Werror", "-O1"])
        .arg("-I")
        .arg(root.join("include"))
        .arg("-I")
        .arg(root.join("tests/dpi_stub"))
        .arg(root.join("tests/track_test.cpp"))
        .arg(lib)
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(st.success(), "C++ compilation failed");
    let out = Command::new(&exe).arg(out_dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "track_test failed:\n{stdout}");

    let want = "\
RET 2 1 2 2 2
OPEN 1 0
OPEN 0 0
DIAG cpu.pipeline: bind to a key that still names another item; that item is aborted (1 time, first iid 9)
DIAG cpu.pipeline: open on a key that still names an item; that item is aborted (1 time, first fe 7)
DIAG cpu.pipeline: stage on a key that names no item (1 time, first fe 4)
C 6 0 0
WARN1 vtr_trace: TOP.tb.core.u_vtr.bus: clock TOP.tb.core.u_vtr.nope is not declared with vtr_clock (1 time)
WARN1 vtr_trace: vtr_item_bind: key spaces of two different trackers (TOP.tb.core.pipeline, TOP.tb.core.u_vtr.bus) (1 time)
WARN1 vtr_trace: vtr_item_close: status 7 is not VTR_TX_OK, VTR_TX_ERROR or VTR_TX_ABORTED (1 time)
WARN1 vtr_trace: vtr_item_stage: handle 33554433 is not a key space (1 time)
ABORT 0
WARN2 vtr_trace: TOP.tb.core.pipeline: stage on a key that names no item (1 time, first rob 3)
WARN2 vtr_trace: TOP.tb.core.u_vtr.bus: clock TOP.tb.core.u_vtr.nope is not declared with vtr_clock (1 time)
";
    assert_eq!(stdout, want);

    let a = "\
1 cpu.pipeline [10..15] ok
  pc=0x1000
  vtr.label=\"one\"
  event replay@14
  ID@ 10..11
  IR@ 11..12
  IQ@ 12..15
2 cpu.pipeline [10..15] ok
  pc=0x1000
  event replay@14
  ID@ 10..11
  IR@ 11..12
  IQ@ 12..15
3 cpu.pipeline [10..20] open
  pc=0x2000
  ID@ 10..11
  IR@ 11..12
  IQ@ 12..13
  EX@ 13..20
  DC@mem 13..14
  -> wakeup 6
4 cpu.pipeline [13..15] aborted
  ID@ 13..15
5 cpu.pipeline [13..15] aborted
  ID@ 13..15
6 cpu.pipeline [16..17] aborted
  ID@ 16..17
7 cpu.lsu [16..19] ok parent=3
  Q@ 16..19
8 cpu.pipeline [16..17] aborted
  ID@ 16..17
9 cpu.pipeline [16..18] aborted
  ID@ 16..18
10 cpu.pipeline [17..18] aborted
  ID@ 17..18
11 cpu.lsu [19..19] aborted
  Q@ 19..19
12 cpu.lsu [19..19] aborted
  Q@ 19..19
";
    assert_eq!(dump(&out_dir.join("track_a.vtr")), a);

    let r = Reader::open(out_dir.join("track_c.vtr")).unwrap();
    let txs = r.transactions(&TxQuery::default()).unwrap();
    assert_eq!(txs.len(), 1000);
    let folded: Vec<_> = txs.iter().filter(|t| t.end == 3).collect();
    assert_eq!(folded.len(), 6);
    assert!(
        folded
            .iter()
            .all(|t| t.id <= 6 && t.stages.len() == 2 && t.attrs.len() == 1)
    );
    assert!(
        txs.iter()
            .all(|t| t.status == vtr::TxStatus::Ok && (t.end == 3 || t.end == 4))
    );

    let b1 = "\
1 TOP.tb.core.clk [50..150] open
  vtr.period=10t
2 TOP.tb.core.pipeline [100..130] ok
  pc=0x80
  F@ 100..120
  D@ 120..130
3 TOP.tb.core.pipeline [110..150] open
  vtr.label=\"two\"
  F@ 110..150
";
    assert_eq!(dump(&out_dir.join("track_b1.vtr")), b1);
    let r = Reader::open(out_dir.join("track_b1.vtr")).unwrap();
    let stream = r
        .generators()
        .find(|&s| r.full_path(s, ".") == "TOP.tb.core.pipeline")
        .unwrap();
    let clock = r
        .generator_clock(stream)
        .expect("pipeline counted in TOP.tb.core.clk");
    assert_eq!(Some(clock), r.find_clock("TOP.tb.core.clk"));

    let b2 = "\
1 TOP.tb.core.clk [50..210] open
  vtr.period=10t
2 TOP.tb.core.pipeline [200..210] open
  F@ 200..210
3 TX.core0.events [200..210] open
  E@ 200..210
";
    assert_eq!(dump(&out_dir.join("track_b2.vtr")), b2);
}
