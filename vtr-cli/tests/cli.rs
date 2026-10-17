use std::path::{Path, PathBuf};
use std::process::{Command, Output};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../vtr-convert/tests/fixtures")
        .join(name)
}
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vtr"))
        .args(args)
        .current_dir(dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .unwrap()
}
fn ok(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
fn help_errors_and_atomic_conversion() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for command in [
        "info",
        "hierarchy",
        "value",
        "changes",
        "transactions",
        "logs",
        "clocks",
        "convert",
        "to-vcd",
        "recover",
        "index",
        "active",
    ] {
        assert!(ok(d, &[command, "--help"]).contains("Usage:"));
    }
    for args in [
        vec!["nonsense"],
        vec!["info", "missing.vtr"],
        vec!["info", "file", "--unknown"],
        vec!["changes", "file", "sig", "--from", "bad"],
        vec!["active", "file", "--from", "9", "--to", "8"],
        vec!["convert", "bad.xyz", "out.vtr"],
        vec!["convert", "missing.vcd", "out.vtr"],
    ] {
        let out = run(d, &args);
        assert_eq!(out.status.code(), Some(2));
        assert!(!out.stderr.is_empty());
    }
    std::fs::write(d.join("bad.kanata"), "Kanata\t0004\nI\tbad\n").unwrap();
    let out = run(d, &["convert", "bad.kanata", "out.vtr"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("line 2"));
    assert!(!d.join("out.vtr").exists());
    std::fs::write(d.join("out.vtr"), "keep me").unwrap();
    assert_eq!(
        run(
            d,
            &["convert", fixture("waves.vcd").to_str().unwrap(), "out.vtr"]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(std::fs::read(d.join("out.vtr")).unwrap(), b"keep me");
    assert_eq!(
        run(d, &["convert", "bad.kanata", "bad.kanata"])
            .status
            .code(),
        Some(2)
    );
    std::fs::write(d.join("bad.vcd"), "not a waveform").unwrap();
    assert_eq!(
        run(d, &["convert", "bad.vcd", "new.vtr"]).status.code(),
        Some(2)
    );
    assert!(!d.join("new.vtr").exists());
}
#[test]
fn waveform_queries_export_and_activity() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    ok(
        d,
        &["convert", fixture("waves.vcd").to_str().unwrap(), "run.vtr"],
    );
    assert!(ok(d, &["info", "run.vtr"]).contains("1e-8 s"));
    assert!(ok(d, &["hierarchy", "run.vtr", "--vars"]).contains("alias"));
    assert!(ok(d, &["hierarchy", "run.vtr", "--sizes"]).contains("signals"));
    assert_eq!(
        ok(d, &["value", "run.vtr", "top.bus [3:0]", "5"]).trim(),
        "z001"
    );
    assert_eq!(
        ok(
            d,
            &["changes", "run.vtr", "top.clk", "--from", "5", "--to", "7"]
        )
        .trim(),
        "5\t1\n7\t0"
    );
    assert_eq!(
        run(d, &["value", "run.vtr", "nope", "0"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run(d, &["index", "run.vtr", "--check"]).status.code(),
        Some(1)
    );
    assert_eq!(run(d, &["active", "run.vtr"]).status.code(), Some(2));
    assert_eq!(
        run(d, &["index", "run.vtr", "--memory", "0"]).status.code(),
        Some(2)
    );
    ok(d, &["index", "run.vtr", "--threads", "2"]);
    assert!(ok(d, &["index", "run.vtr", "--check"]).contains("valid"));
    let active = ok(d, &["active", "run.vtr", "--from", "5", "--to", "5"]);
    assert!(active.contains("top.clk"));
    assert!(!active.contains("top.alias"));
    assert!(ok(d, &["active", "run.vtr", "--from", "100", "--to", "200"]).is_empty());
    ok(d, &["to-vcd", "run.vtr", "out.vcd"]);
    ok(d, &["convert", "out.vcd", "roundtrip.vtr"]);
    std::fs::remove_file(d.join("run.vtr")).unwrap();
    ok(
        d,
        &[
            "convert",
            fixture("pipeline.kanata").to_str().unwrap(),
            "run.vtr",
        ],
    );
    assert_eq!(
        run(d, &["index", "run.vtr", "--check"]).status.code(),
        Some(1)
    );
}
#[test]
fn transactions_clocks_logs_and_window_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    ok(
        d,
        &[
            "convert",
            fixture("pipeline.kanata").to_str().unwrap(),
            "run.vtr",
        ],
    );
    assert!(ok(d, &["clocks", "run.vtr"]).contains("cpu.cycle"));
    let output = ok(
        d,
        &["transactions", "run.vtr", "--scope", "cpu", "--from", "104"],
    );
    assert!(output.contains("insn_id_in_sim = 12"));
    assert!(!output.contains("insn_id_in_sim = 10"));
    assert!(ok(d, &["transactions", "run.vtr", "--id", "2"]).contains("wakeup"));
    assert_eq!(
        run(d, &["transactions", "run.vtr", "--id", "999"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(d, &["transactions", "run.vtr", "--scope", "absent"])
            .status
            .code(),
        Some(2)
    );
    let mut w = vtr::Writer::create(d.join("logs.vtr")).unwrap();
    let stream = w
        .add_scope(None, "messages", vtr::ScopeType::Generic, "")
        .unwrap();
    let site = w
        .add_log_site(&vtr::LogSiteSpec::new(
            Some(stream),
            vtr::Severity::Warn,
            "hello {}",
            &[vtr::LogArgType::U64],
        ))
        .unwrap();
    w.log(site, 5, &[1u64.into()]).unwrap();
    w.log(site, 10, &[2u64.into()]).unwrap();
    w.close().unwrap();
    let logs = ok(d, &["logs", "logs.vtr", "--to", "5"]);
    assert!(logs.contains("hello 1"));
    assert!(!logs.contains("hello 2"));
    assert!(ok(d, &["logs", "logs.vtr", "--sites"]).contains("hello {}"));
    assert!(ok(d, &["logs", "logs.vtr", "--severity", "error"]).is_empty());
    assert_eq!(
        run(d, &["logs", "logs.vtr", "--severity", "oops"])
            .status
            .code(),
        Some(2)
    );
}
#[test]
fn fst_must_be_converted_before_activity_indexing() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../vtr-convert/tests/fixtures/values.fst");
    std::fs::copy(fixture, d.join("values.fst")).unwrap();
    for command in ["index", "active"] {
        assert!(!run(d, &[command, "values.fst"]).status.success());
    }
    ok(d, &["convert", "values.fst", "values.vtr"]);
    ok(d, &["index", "values.vtr"]);
    ok(d, &["index", "values.vtr", "--check"]);
    let output = ok(d, &["active", "values.vtr", "--from", "11", "--to", "1000"]);
    let reader = vtr::Reader::open(d.join("values.vtr")).unwrap();
    let refs: Vec<_> = (0..reader.signal_count()).map(vtr::SignalId).collect();
    let mut active: std::collections::BTreeSet<_> =
        vtr::activity::resolve(&reader, &refs, 11, 1000)
            .unwrap()
            .into_iter()
            .collect();
    let expected: Vec<_> = reader
        .hierarchy()
        .ids()
        .filter(|&id| {
            reader
                .hierarchy()
                .signal_of(id)
                .is_some_and(|s| active.remove(&s))
        })
        .map(|id| reader.full_path(id, "."))
        .collect();
    assert_eq!(output.lines().collect::<Vec<_>>(), expected);
}
#[test]
fn recovery_reports_dropped_bytes_and_preserves_complete_files() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let mut w = vtr::Writer::create_with(
        d.join("full.vtr"),
        vtr::WriterOptions {
            block_records: 100,
            background: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, s) = w
        .add_var(
            None,
            "a",
            vtr::VarType::Wire,
            vtr::Direction::Implicit,
            vtr::SignalKind::bits(8, vtr::LogicStates::Two),
        )
        .unwrap();
    for t in 0..1000 {
        w.set_time(t).unwrap();
        w.emit_u64(s, t).unwrap();
    }
    w.close().unwrap();
    let bytes = std::fs::read(d.join("full.vtr")).unwrap();
    std::fs::write(d.join("cut.vtr"), &bytes[..bytes.len() - 700]).unwrap();
    assert_eq!(
        run(d, &["index", "cut.vtr", "--check"]).status.code(),
        Some(1)
    );
    assert_eq!(run(d, &["index", "cut.vtr"]).status.code(), Some(2));
    let out = run(d, &["recover", "cut.vtr", "recovered.vtr"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = vtr::Reader::open(d.join("recovered.vtr")).unwrap();
    assert!(r.recovered().is_none());
    assert!(matches!(r.ending().unwrap().0,vtr::Ending::Recovered{dropped} if dropped>0));
    ok(d, &["recover", "full.vtr", "copy.vtr"]);
    assert_eq!(bytes, std::fs::read(d.join("copy.vtr")).unwrap());
}

#[cfg(unix)]
#[test]
fn valid_activity_index_can_be_reused_without_write_access() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    ok(
        d,
        &["convert", fixture("waves.vcd").to_str().unwrap(), "run.vtr"],
    );
    ok(d, &["index", "run.vtr"]);
    std::fs::remove_file(d.join("run.vtr.index.lock")).unwrap();
    std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = run(d, &["index", "run.vtr"]);
    std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!d.join("run.vtr.index.lock").exists());
    assert!(!d.join("cache").exists());
}
