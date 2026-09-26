//! The keyed tracker (vtr_track.hpp) and the tracker part of the vtr_trace
//! package (vtr_trace_dpi.hpp): builds tests/track_test.cpp against the static
//! library, runs it, and reads its recordings back with the Rust reader.

use std::path::{Path, PathBuf};
use std::process::Command;

use vtr::{Reader, TxQuery, Value};

fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Every transaction of a file, ordered by id, as `id stream/generator [b..e] status`
/// lines followed by attributes, events, stages (`name@lane b..e`) and relations.
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
        let stream = r.generator_stream(t.generator).map(|s| r.full_path(s, ".")).unwrap_or_default();
        out += &format!("{} {stream}/{} [{}..{}] {}", t.id, r.name(t.generator), t.begin, t.end, t.status.name());
        if let Some(p) = t.parent {
            out += &format!(" parent={p}");
        }
        out += "\n";
        for a in &t.attrs {
            out += &format!("  {}={}\n", r.str(a.key), val(&a.value));
        }
        for e in &t.events {
            out += &format!("  event {}@{}\n", r.str(e.name), e.time);
        }
        for s in &t.stages {
            let end = s.end.map(|e| e.to_string()).unwrap_or_else(|| "open".into());
            out += &format!("  {}@{} {}..{end}\n", r.str(s.name), r.str(s.lane), s.begin);
        }
        for rel in r.relations_from(t.id).unwrap() {
            out += &format!("  -> {} {}\n", r.str(rel.kind), rel.to);
        }
    }
    out
}

#[test]
fn tracker_and_package() {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    if Command::new(&cxx).arg("--version").output().is_err() {
        eprintln!("no C++ compiler ({cxx}); skipping");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tdir = target_dir();
    let mut build = Command::new(env!("CARGO"));
    build.args(["build", "-p", "vtr-capi", "--lib"]);
    if tdir.file_name().map(|n| n == "release").unwrap_or(false) {
        build.arg("--release");
    }
    assert!(build.status().unwrap().success(), "cargo build of libvtr failed");
    let out_dir = tdir.join("track_test");
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join("track_test");
    let st = Command::new(&cxx)
        .args(["-std=c++17", "-Wall", "-Wextra", "-Werror", "-O1"])
        .arg("-I")
        .arg(root.join("include"))
        .arg("-I")
        .arg(root.join("tests/dpi_stub"))
        .arg(root.join("tests/track_test.cpp"))
        .arg(tdir.join("libvtr.a"))
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(st.success(), "C++ compilation failed");
    let out = Command::new(&exe).arg(&out_dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!("{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "track_test failed:\n{stdout}");

    // Call results, open counts before and after detach(), diagnostics and warnings.
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

    // Part A. fe 1 and 2 fold into iid 5 (bind_oldest takes the oldest first) and end
    // together, with the pc replaced at 14; abort_younger(iid 6) ends fe 4 and 5 and
    // keeps iid 6; the stale fe 7 and iid 9 names abort their old items at 17;
    // abort_keyspace(fe) ends the items still named in fe at 18; iid 6 is open at
    // close with its buffered pc written, its main-lane EX stage open and its mem lane
    // stage ended at 14.
    let a = "\
1 cpu.pipeline/instruction [10..15] ok
  pc=0x1000
  vtr.label=\"one\"
  event replay@14
  ID@ 10..11
  IR@ 11..12
  IQ@ 12..15
2 cpu.pipeline/instruction [10..15] ok
  pc=0x1000
  event replay@14
  ID@ 10..11
  IR@ 11..12
  IQ@ 12..15
3 cpu.pipeline/instruction [10..20] open
  pc=0x2000
  ID@ 10..11
  IR@ 11..12
  IQ@ 12..13
  EX@ 13..20
  DC@mem 13..14
  -> wakeup 6
4 cpu.pipeline/instruction [13..15] aborted
  ID@ 13..15
5 cpu.pipeline/instruction [13..15] aborted
  ID@ 13..15
6 cpu.pipeline/instruction [16..17] aborted
  ID@ 16..17
7 cpu.lsu/request [16..19] ok parent=3
  Q@ 16..19
8 cpu.pipeline/instruction [16..17] aborted
  ID@ 16..17
9 cpu.pipeline/instruction [16..18] aborted
  ID@ 16..18
10 cpu.pipeline/instruction [17..18] aborted
  ID@ 17..18
11 cpu.lsu/request [19..19] aborted
  Q@ 19..19
12 cpu.lsu/request [19..19] aborted
  Q@ 19..19
";
    assert_eq!(dump(&out_dir.join("track_a.vtr")), a);

    // Part C: the six oldest items fold under one key and end together; the other 994
    // end at 4, each exactly once.
    let r = Reader::open(out_dir.join("track_c.vtr")).unwrap();
    let txs = r.transactions(&TxQuery::default()).unwrap();
    assert_eq!(txs.len(), 1000);
    let folded: Vec<_> = txs.iter().filter(|t| t.end == 3).collect();
    assert_eq!(folded.len(), 6);
    assert!(folded.iter().all(|t| t.id <= 6 && t.stages.len() == 2 && t.attrs.len() == 1));
    assert!(txs.iter().all(|t| t.status == vtr::TxStatus::Ok && (t.end == 3 || t.end == 4)));

    // Part B, first file: the tracker and its clock declared before the open are
    // replayed; the call made before the open left nothing; open_at back-dates to the
    // saved vtr_now(); the item open at close keeps the label bound through "rob".
    let b1 = "\
1 TOP.tb.core.clk/edges [50..150] open
  vtr.period=10t
2 TOP.tb.core.pipeline/instruction [100..130] ok
  pc=0x80
  F@ 100..120
  D@ 120..130
3 TOP.tb.core.pipeline/instruction [110..150] open
  vtr.label=\"two\"
  F@ 110..150
";
    assert_eq!(dump(&out_dir.join("track_b1.vtr")), b1);
    let r = Reader::open(out_dir.join("track_b1.vtr")).unwrap();
    let stream = r.streams().find(|&s| r.full_path(s, ".") == "TOP.tb.core.pipeline").unwrap();
    let clock = r.stream_clock(stream).expect("pipeline counted in TOP.tb.core.clk");
    assert_eq!(Some(clock), r.find_clock("TOP.tb.core.clk"));

    // Second file: the first file's keys were dropped at its close notice.
    let b2 = "\
1 TOP.tb.core.clk/edges [50..210] open
  vtr.period=10t
2 TOP.tb.core.pipeline/instruction [200..210] open
  F@ 200..210
3 TX.core0.events/event [200..210] open
  E@ 200..210
";
    assert_eq!(dump(&out_dir.join("track_b2.vtr")), b2);
}
