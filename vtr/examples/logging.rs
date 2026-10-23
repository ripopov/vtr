//! Typed log fields, severity metadata, and retained record decoding.

use vtr::{Attribute, FormatLocale, Reader, Recorder, Result, SourceOptions, Value, Writer};

struct Step {
    phase: String,
    cycles: u64,
}
vtr::impl_payload!(Step { phase, cycles });

fn main() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("logging.vtr"),
        std::path::PathBuf::from,
    );
    let mut writer = Writer::create(&path)?;
    let cpu = writer.scope("cpu", None)?;
    let severity = writer.intern("log.severity")?;
    let mut source = SourceOptions::new("{} completed in {:L} cycles", Some(cpu));
    source.attributes.push(Attribute {
        key: severity,
        value: Value::U64(2),
    });
    let source = writer.logs::<Step>(source)?;
    for time in 0..10 {
        writer.advance_time(time)?;
        let mut step = Step {
            phase: format!("step {time}"),
            cycles: time * 3000,
        };
        writer.log(source, &step, None)?;
        step.phase.clear();
    }
    writer.close()?;

    let reader = Reader::open(path)?;
    let locale = FormatLocale {
        thousands_sep: b'_',
        grouping: &[3],
        ..FormatLocale::default()
    };
    let mut retained = Vec::new();
    reader.records(
        vtr::RecordQuery {
            kind: Some(vtr::RecordKind::Log),
            ..vtr::RecordQuery::default()
        },
        |record| {
            println!(
                "{}: {}",
                record.begin(),
                reader
                    .format_log_with_locale(&record, &locale)
                    .expect("log format")
            );
            retained.push(record);
            true
        },
    )?;
    assert_eq!(
        reader.format_log(&retained[3])?,
        "step 3 completed in 9000 cycles"
    );
    assert_eq!(
        reader.format_log_with_locale(&retained[3], &locale)?,
        "step 3 completed in 9_000 cycles"
    );
    drop(reader);
    assert_eq!(retained[3].decode::<Step>()?.phase, "step 3");
    assert_eq!(
        retained[3].payload().field("cycles")?.decode::<u64>()?,
        9000
    );
    Ok(())
}
