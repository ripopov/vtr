//! Read actual Verilator output, including traces closed by fatal exit callbacks.

use std::path::PathBuf;
use vtr::logging::Severity;
use vtr::*;

fn fixture(scenario: &str, execution: &str) -> Reader {
    let directory = std::env::var_os("VTR_VERILATOR_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verilator"),
        PathBuf::from,
    );
    Reader::open(directory.join(format!("logs_{scenario}_{execution}.vtr"))).unwrap()
}

#[test]
fn verilator_declares_typed_text_sources_and_preserves_partial_output() {
    for execution in ["inline", "threaded"] {
        let reader = fixture("normal", execution);
        let sources = vtr::logging::callsites(&reader).unwrap();
        assert_eq!(sources.len(), 6);
        let logger = reader.find_generator("simulation_log").unwrap();
        assert_eq!(
            reader
                .declarations()
                .nodes
                .iter()
                .filter(|node| matches!(node.data, NodeData::Generator { .. }))
                .count(),
            1
        );
        for (source, severity) in sources.iter().zip([
            Severity::Trace,
            Severity::Debug,
            Severity::Info,
            Severity::Warn,
            Severity::Error,
            Severity::Fatal,
        ]) {
            assert_eq!(source.severity, severity);
            assert_eq!(source.format, "{}");
            assert!(
                reader
                    .declarations()
                    .types
                    .get(source.datatype)
                    .unwrap()
                    .attributes
                    .len()
                    >= 3
            );
            assert_eq!(
                reader
                    .declarations()
                    .types
                    .get(source.datatype)
                    .unwrap()
                    .fixed_size,
                None
            );
        }
        let mut records = Vec::new();
        reader
            .records(
                RecordQuery {
                    kind: Some(RecordKind::Log),
                    ..RecordQuery::default()
                },
                |record| {
                    records.push(record);
                    true
                },
            )
            .unwrap();
        records.sort_unstable_by_key(Record::id);
        assert_eq!(records.len(), 9);
        assert_eq!(
            vtr::logging::format_log(&reader, &records[0]).unwrap(),
            "boot complete\n"
        );
        assert_eq!(
            vtr::logging::format_log(&reader, &records[3]).unwrap(),
            "fragment "
        );
        assert_eq!(
            vtr::logging::format_log(&reader, &records[4]).unwrap(),
            "complete\n"
        );
        assert_eq!(records.last().unwrap().begin(), 7);
        let warning = sources
            .iter()
            .find(|source| source.severity == Severity::Warn)
            .unwrap();
        let mut count = 0;
        reader
            .records(
                RecordQuery {
                    datatype: Some(warning.datatype),
                    begin: 0,
                    end: 0,
                    ..RecordQuery::default()
                },
                |record| {
                    assert!(
                        vtr::logging::format_log(&reader, &record)
                            .unwrap()
                            .contains("retry pending")
                    );
                    count += 1;
                    true
                },
            )
            .unwrap();
        assert_eq!(count, 1);
        for record in &records {
            assert_eq!(record.generator(), Some(logger));
            assert_eq!(
                reader.lookup(record.id()).unwrap().unwrap().generator(),
                record.generator()
            );
        }
        reader.validate().unwrap();
        drop(reader);
        assert_eq!(
            records[0]
                .payload()
                .field("value")
                .unwrap()
                .decode::<String>()
                .unwrap(),
            "boot complete\n"
        );
    }
}

#[test]
fn terminating_tasks_binary_output_and_runtime_warnings_remain_readable() {
    for execution in ["inline", "threaded"] {
        for (scenario, time, severity, text) in [
            ("error", 5, Severity::Error, "response rejected"),
            ("fatal", 5, Severity::Fatal, "watchdog expired"),
            ("finish", 5, Severity::Info, "Verilog $finish"),
            ("bytes", 5, Severity::Info, "[non-UTF-8 bytes] 72617720ff0a"),
            ("runtime", 5, Severity::Info, "Time scale"),
            ("repeat", 3, Severity::Warn, "previous dump"),
        ] {
            let reader = fixture(scenario, execution);
            let sources = vtr::logging::callsites(&reader).unwrap();
            let mut found = false;
            reader
                .records(
                    RecordQuery {
                        kind: Some(RecordKind::Log),
                        ..RecordQuery::default()
                    },
                    |record| {
                        let source = sources
                            .iter()
                            .find(|source| source.datatype == record.datatype())
                            .unwrap();
                        found |= record.begin() == time
                            && source.severity == severity
                            && vtr::logging::format_log(&reader, &record)
                                .unwrap()
                                .contains(text);
                        true
                    },
                )
                .unwrap();
            assert!(found, "{scenario}, {execution}");
            assert_eq!(reader.completion(), Completion::Complete);
            reader.validate().unwrap();
        }
    }
}
