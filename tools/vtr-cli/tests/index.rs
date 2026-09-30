//! `vtr index`: builds the activity index beside a trace, reports it in one
//! line, and `--check` tells a valid index from a stale or missing one.

use std::path::Path;
use std::process::{Command, Output};
use vtr::{Direction, ScopeType, SignalKind, VarType, Writer, WriterOptions};

fn trace(path: &Path, period: u64) {
    let opts = WriterOptions { block_records: 500, ..Default::default() };
    let mut w = Writer::create_with(path, opts).unwrap();
    w.set_timescale(-12).unwrap();
    let top = Some(w.add_scope(None, "top", ScopeType::Module, "").unwrap());
    let (_, clk) = w.add_var(top, "clk", VarType::Wire, Direction::Input, SignalKind::Bits { width: 1, states: 2 }).unwrap();
    let (_, cnt) = w.add_var(top, "cnt", VarType::Reg, Direction::Output, SignalKind::Bits { width: 8, states: 2 }).unwrap();
    for i in 0..4000u64 {
        w.set_time(i * period).unwrap();
        w.emit_bit(clk, (i & 1) as u8).unwrap();
        if i % 16 == 0 {
            w.emit_u64(cnt, i / 16).unwrap();
        }
    }
    w.close().unwrap();
}

fn vtr(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vtr")).args(args).env("XDG_CACHE_HOME", dir.join("cache")).output().unwrap()
}

#[test]
fn index_then_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    trace(&path, 500);
    let p = path.to_str().unwrap();

    let missing = vtr(dir.path(), &["index", p, "--check"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("no valid activity index"));

    let built = vtr(dir.path(), &["index", p, "--threads", "2"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let line = String::from_utf8_lossy(&built.stdout);
    assert!(line.starts_with(&format!("{p}.index: ")) && line.contains(" blocks, Δ ") && line.contains(" ns") && line.contains("% of "), "{line}");
    assert!(dir.path().join("run.vtr.index").exists());

    let valid = vtr(dir.path(), &["index", "--check", p]);
    assert!(valid.status.success());
    assert!(String::from_utf8_lossy(&valid.stdout).contains("valid for this trace"));

    // A re-simulated trace makes the index stale.
    trace(&path, 700);
    let stale = vtr(dir.path(), &["index", p, "--check"]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stdout).contains("built for another trace"));
}

/// `vtr active` answers exactly, refuses to guess without an index, and
/// builds one with `--build`.
#[test]
fn active_scopes_and_signals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    trace(&path, 500);
    let p = path.to_str().unwrap();

    let refused = vtr(dir.path(), &["active", p, "0", "1000000"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("vtr index"));

    // clk toggles every 500 ps and cnt every 16 steps (8 ns): a 5 ns window
    // between two cnt changes holds only clk.
    let built = vtr(dir.path(), &["active", p, "9ns", "14ns", "--build"]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let out = String::from_utf8_lossy(&built.stdout);
    let mut lines = out.lines();
    assert!(lines.next().unwrap().starts_with("9,000–14,000 ps · 1 of 2 signals change · "), "{out}");
    assert_eq!(lines.map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>(), ["top 1 / 2"]);
    assert!(dir.path().join("run.vtr.index").exists());

    let signals = vtr(dir.path(), &["active", p, "7000", "9000", "--signals"]);
    let out = String::from_utf8_lossy(&signals.stdout);
    assert_eq!(out.lines().skip(1).collect::<Vec<_>>(), ["top.clk", "top.cnt"], "{out}");
    let quiet = vtr(dir.path(), &["active", p, "3000000", "4000000", "--scope", "top"]);
    assert!(String::from_utf8_lossy(&quiet.stdout).contains("· 0 of 2 signals change · exact"));
}
