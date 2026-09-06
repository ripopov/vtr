//! Read actual Verilator output, including files closed during fatal shutdown.
use std::path::PathBuf;
use vtr::{LogQuery, Reader, Severity, TxQuery};

fn fixture(mode: &str) -> Reader {
    Reader::open(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../ext/surfer/examples/verilator/logs_{mode}.vtr"
    )))
    .unwrap()
}

#[test]
fn verilator_uses_one_stream_and_one_generator_per_severity() {
    let reader = fixture("normal");
    assert_eq!(reader.log_sites().len(), 6);
    let stream = reader.log_sites()[0].stream;
    for (level, site) in reader.log_sites().iter().enumerate() {
        assert_eq!(site.severity.code(), level as u8);
        assert_eq!(site.stream, stream);
        assert_eq!(reader.str(site.fmt), "{}");
        assert_eq!(site.args, [vtr::LogArgType::Text]);
    }
    let mut rows = Vec::new();
    reader
        .visit_log(&LogQuery::default(), |r| {
            rows.push((r.id, r.time, r.severity(), r.format(reader.strings())));
            true
        })
        .unwrap();
    assert_eq!(rows.len(), 15);
    assert_eq!(
        (rows[0].1, rows[0].2, rows[0].3.as_str()),
        (0, Severity::Info, "boot complete\n")
    );
    assert_eq!(rows[2].2, Severity::Warn);
    assert!(rows[2].3.contains("retry pending"));
    assert_eq!(rows[4].3, "fragment ");
    assert_eq!(rows[5].3, "complete\n");
    assert_eq!(rows.last().unwrap().1, 7);
    let mut transactions = Vec::new();
    reader
        .visit_transactions(&TxQuery::default(), |tx| {
            assert_eq!(tx.begin, tx.end);
            transactions.push(tx.id);
            true
        })
        .unwrap();
    assert_eq!(transactions, rows.iter().map(|r| r.0).collect::<Vec<_>>());
    let warning = reader
        .log_sites()
        .iter()
        .find(|s| s.severity == Severity::Warn)
        .unwrap();
    let mut count = 0;
    reader
        .visit_log(
            &LogQuery {
                generator: Some(warning.node),
                window: Some((0, 0)),
                ..Default::default()
            },
            |_| {
                count += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn terminating_tasks_leave_readable_timestamped_logs() {
    for (mode, severity, text) in [
        ("error", Severity::Error, "response rejected"),
        ("fatal", Severity::Fatal, "watchdog expired"),
        ("finish", Severity::Info, "Verilog $finish"),
    ] {
        let reader = fixture(mode);
        let mut found = false;
        reader
            .visit_log(&LogQuery::default(), |r| {
                assert!(r.time <= 5);
                found |= r.time == 5
                    && r.severity() == severity
                    && r.format(reader.strings()).contains(text);
                true
            })
            .unwrap();
        assert!(found, "{mode}");
    }
}

#[test]
fn binary_hdl_output_is_recoverable_without_aborting() {
    let reader = fixture("bytes");
    let mut found = false;
    reader
        .visit_log(&LogQuery::default(), |r| {
            found |= r.time == 5 && r.format(reader.strings()) == "[non-UTF-8 bytes] 72617720ff0a";
            true
        })
        .unwrap();
    assert!(found);
    assert_eq!(reader.log_count(), 16);
}

#[test]
fn runtime_diagnostics_and_trace_reentry_are_recorded() {
    for (mode, time, severity, text) in [
        ("runtime", 5, Severity::Info, "Time scale"),
        ("repeat", 3, Severity::Warn, "previous dump"),
    ] {
        let reader = fixture(mode);
        let mut found = false;
        reader
            .visit_log(&LogQuery::default(), |r| {
                found |= r.time == time
                    && r.severity() == severity
                    && r.format(reader.strings()).contains(text);
                true
            })
            .unwrap();
        assert!(found, "{mode}");
    }
}
