//! Typed log fields, severity metadata, and retained record decoding.

use vtr::{FormatLocale, Reader, Recorder, Result, SourceOptions, Writer};

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
    let logger = writer.logger(SourceOptions::new("cpu", Some(cpu)))?;
    let source = vtr::logging::declare::<Step>(
        &mut writer,
        vtr::logging::CallsiteOptions::new("{} completed in {:L} cycles"),
    )?;
    let retry = vtr::logging::declare::<u32>(
        &mut writer,
        vtr::logging::CallsiteOptions {
            severity: 3,
            ..vtr::logging::CallsiteOptions::new("retry bank {}")
        },
    )?;
    for time in 0..10 {
        writer.advance_time(time)?;
        let mut step = Step {
            phase: format!("step {time}"),
            cycles: time * 3000,
        };
        writer.log(logger, source, &step, None)?;
        if time % 3 == 0 {
            writer.log(logger, retry, &3, None)?;
        }
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
                vtr::logging::format_log_with_locale(&reader, &record, &locale)
                    .expect("log format")
            );
            if record.datatype() == source.id() {
                retained.push(record);
            }
            true
        },
    )?;
    assert_eq!(
        vtr::logging::format_log(&reader, &retained[3])?,
        "step 3 completed in 9000 cycles"
    );
    assert_eq!(
        vtr::logging::format_log_with_locale(&reader, &retained[3], &locale)?,
        "step 3 completed in 9_000 cycles"
    );
    drop(reader);
    let payload = retained[3].payload().field("value")?;
    assert_eq!(payload.decode::<Step>()?.phase, "step 3");
    assert_eq!(payload.field("cycles")?.decode::<u64>()?, 9000);
    Ok(())
}
