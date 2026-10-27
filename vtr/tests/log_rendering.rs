//! Rendering reads typed fields and checks their recording identity.

use vtr::{
    logging::Severity,
    recording::{ExecutionMode, Recorder, SourceOptions, Writer, WriterOptions},
    trace::Reader,
};

struct Arguments {
    number: u64,
    text: String,
    measure: f64,
    flag: bool,
}
vtr::impl_payload!(Arguments {
    number,
    text,
    measure,
    flag
});

#[test]
fn typed_fields_escaping_specs_and_wrong_recording_checks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("logs.vtr");
    let mut writer = Writer::create_with(
        &path,
        WriterOptions {
            mode: ExecutionMode::Inline,
            ..Default::default()
        },
    )
    .unwrap();
    let logger = writer
        .logger(SourceOptions::new("diagnostics", None))
        .unwrap();
    let cases = [
        (
            "{{message}} {0:#010x} {1} {3}",
            "{message} 0x000000ff héllo true",
        ),
        ("{2:.2f}|{1:*^7}|{1:.1}", "-2.50|*héllo*|h"),
        ("{1:>7}|{1:<7}|{2:+08.1f}", "  héllo|héllo  |-00002.5"),
        ("{0} {bad} {99} {", "255 {?} {?} {"),
        (
            "{0:bad} {0:99999999999999999999999999999999} {1:x}",
            "{?} {?} {?}",
        ),
    ];
    let mut ids = Vec::new();
    for (format, _) in cases {
        let source = vtr::logging::declare::<Arguments>(
            &mut writer,
            vtr::logging::CallsiteOptions {
                severity: 3,
                ..vtr::logging::CallsiteOptions::new(format)
            },
        )
        .unwrap();
        ids.push(
            writer
                .log(
                    logger,
                    source,
                    &Arguments {
                        number: 255,
                        text: "héllo".into(),
                        measure: -2.5,
                        flag: true,
                    },
                    None,
                )
                .unwrap(),
        );
    }
    let owner_source = writer
        .transactions::<u64>(SourceOptions::new("owners", None), "TX")
        .unwrap();
    let owner = writer.begin(owner_source, &5, None).unwrap();
    writer.end(owner, vtr::recording::Status::Ok).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(&path).unwrap();
    reader.validate().unwrap();
    assert_eq!(vtr::logging::callsites(&reader).unwrap().len(), cases.len());
    assert!(
        vtr::logging::callsites(&reader)
            .unwrap()
            .iter()
            .all(|source| source.severity == Severity::Warn)
    );
    for (id, (_, expected)) in ids.into_iter().zip(cases) {
        let record = reader.lookup(id).unwrap().unwrap();
        assert_eq!(
            vtr::logging::format_log(&reader, &record).unwrap(),
            expected
        );
        assert_eq!(
            record
                .payload()
                .field("value")
                .unwrap()
                .at(0)
                .unwrap()
                .decode::<u64>()
                .unwrap(),
            255
        );
        let foreign = directory.path().join("foreign.vtr");
        let mut foreign_writer = Writer::create(&foreign).unwrap();
        foreign_writer.close().unwrap();
        let foreign = Reader::open(foreign).unwrap();
        assert!(vtr::logging::format_log(&foreign, &record).is_err());
    }
    assert!(
        vtr::logging::format_log(&reader, &reader.lookup(owner.id()).unwrap().unwrap()).is_err()
    );
}

#[test]
fn empty_queries_reject_reversed_windows_and_clock_cycle_max_is_representable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clock.vtr");
    let mut writer = Writer::create_with(
        &path,
        WriterOptions {
            mode: ExecutionMode::Inline,
            ..Default::default()
        },
    )
    .unwrap();
    let clock = writer.clock(SourceOptions::new("clock", None)).unwrap();
    writer.clock_run(clock, 0, 1).unwrap();
    writer.advance_time(u64::MAX).unwrap();
    writer.clock_stop(clock, u64::MAX).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    reader.validate().unwrap();
    assert_eq!(
        reader.cycle_at(clock.id(), u64::MAX).unwrap(),
        Some(u64::MAX)
    );
    assert!(
        reader
            .records(
                vtr::records::RecordQuery {
                    begin: 1,
                    end: 0,
                    ..Default::default()
                },
                |_| true
            )
            .is_err()
    );
    assert!(
        reader
            .transactions(
                vtr::transactions::TransactionQuery {
                    begin: 1,
                    end: 0,
                    ..Default::default()
                },
                |_| true
            )
            .is_err()
    );
}

#[test]
fn scalar_presentations_match_the_independent_cpp_format_oracle() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/log-format.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("format.vtr");
    let mut writer = Writer::create_with(
        &path,
        WriterOptions {
            mode: ExecutionMode::Inline,
            ..Default::default()
        },
    )
    .unwrap();
    let logger = writer
        .logger(SourceOptions::new("diagnostics", None))
        .unwrap();
    let mut float64_sources = std::collections::HashMap::new();
    let mut float32_sources = std::collections::HashMap::new();
    let mut signed_sources = std::collections::HashMap::new();
    let mut unsigned_sources = std::collections::HashMap::new();
    let mut text_sources = std::collections::HashMap::new();
    let mut bool_sources = std::collections::HashMap::new();
    let mut ids = Vec::new();
    for case in &cases {
        let spec = case["spec"].as_str().unwrap();
        let bits: u64 = case["bits"].as_str().unwrap().parse().unwrap();
        let id = match case["type"].as_str().unwrap() {
            "f64" => oracle_log(
                &mut writer,
                logger,
                &mut float64_sources,
                spec,
                &f64::from_bits(bits),
            ),
            "f32" => oracle_log(
                &mut writer,
                logger,
                &mut float32_sources,
                spec,
                &f32::from_bits(bits as u32),
            ),
            "i64" => oracle_log(
                &mut writer,
                logger,
                &mut signed_sources,
                spec,
                &(bits as i64),
            ),
            "u64" => oracle_log(&mut writer, logger, &mut unsigned_sources, spec, &bits),
            "text" => {
                let bytes = unhex(case["text_hex"].as_str().unwrap());
                oracle_log(
                    &mut writer,
                    logger,
                    &mut text_sources,
                    spec,
                    &String::from_utf8(bytes).unwrap(),
                )
            }
            "bool" => oracle_log(&mut writer, logger, &mut bool_sources, spec, &(bits != 0)),
            _ => panic!("unknown oracle type"),
        };
        ids.push(id);
    }
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    let mut failures = Vec::new();
    let classic = serde_json::json!({"grouping_hex": "", "point_hex": "2e", "separator_hex": "2c", "true_hex": "74727565", "false_hex": "66616c7365"});
    for (id, case) in ids.into_iter().zip(cases) {
        let record = reader.lookup(id).unwrap().unwrap();
        let settings = case.get("locale").unwrap_or(&classic);
        let grouping = unhex(settings["grouping_hex"].as_str().unwrap());
        let point = unhex(settings["point_hex"].as_str().unwrap());
        let separator = unhex(settings["separator_hex"].as_str().unwrap());
        let true_name = unhex(settings["true_hex"].as_str().unwrap());
        let false_name = unhex(settings["false_hex"].as_str().unwrap());
        let locale = vtr::FormatLocale {
            decimal_point: point[0],
            thousands_sep: separator[0],
            grouping: &grouping,
            true_name: &true_name,
            false_name: &false_name,
        };
        let actual = vtr::logging::format_log_bytes_with_locale(&reader, &record, &locale).unwrap();
        if case.get("locale").is_none() {
            assert_eq!(
                vtr::logging::format_log_bytes(&reader, &record).unwrap(),
                actual
            );
        }
        let expected = unhex(case["expected_hex"].as_str().unwrap());
        if actual == expected {
            if let Ok(text) = std::str::from_utf8(&expected) {
                assert_eq!(
                    vtr::logging::format_log_with_locale(&reader, &record, &locale).unwrap(),
                    text
                );
            } else {
                assert!(vtr::logging::format_log_with_locale(&reader, &record, &locale).is_err());
            }
        }
        if actual != expected {
            failures.push(format!(
                "{} bits={} spec={} locale={} expected_len={} actual_len={} first_difference={:?}",
                case["type"],
                case["bits"],
                case["spec"],
                case["locale"],
                expected.len(),
                actual.len(),
                actual.iter().zip(&expected).position(|(a, b)| a != b)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches, first: {:#?}",
        failures.len(),
        &failures[..failures.len().min(12)]
    );
}

fn oracle_log<T: vtr::Payload + 'static>(
    writer: &mut Writer,
    logger: vtr::Logger,
    sources: &mut std::collections::HashMap<String, vtr::PayloadType<T>>,
    spec: &str,
    value: &T,
) -> u64 {
    let source = *sources.entry(spec.into()).or_insert_with(|| {
        vtr::logging::declare::<T>(
            writer,
            vtr::logging::CallsiteOptions::new(format!("{{:{spec}}}")),
        )
        .unwrap()
    });
    writer.log(logger, source, value, None).unwrap()
}

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn retained_log_from_a_later_publication_is_rejected_by_an_earlier_prefix() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prefix.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode,
                ..Default::default()
            },
        )
        .unwrap();
        let logger = writer
            .logger(SourceOptions::new("diagnostics", None))
            .unwrap();
        let early = vtr::logging::declare::<u64>(
            &mut writer,
            vtr::logging::CallsiteOptions::new("early {}"),
        )
        .unwrap();
        writer.log(logger, early, &1, None).unwrap();
        writer.flush().unwrap();
        let prefix = std::fs::read(&path).unwrap();
        let late = vtr::logging::declare::<u64>(
            &mut writer,
            vtr::logging::CallsiteOptions::new("late {}"),
        )
        .unwrap();
        let id = writer.log(logger, late, &2, None).unwrap();
        writer.close().unwrap();
        let reader = Reader::open(&path).unwrap();
        let retained = reader.lookup(id).unwrap().unwrap();
        assert_eq!(
            vtr::logging::format_log(&reader, &retained).unwrap(),
            "late 2"
        );
        let prefix = Reader::from_bytes(prefix).unwrap();
        assert_eq!(prefix.identity(), reader.identity());
        assert!(vtr::logging::format_log(&prefix, &retained).is_err());
        assert!(vtr::logging::format_log_bytes(&prefix, &retained).is_err());
    }
}
