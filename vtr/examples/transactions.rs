//! Typed instructions, embedded pipeline stages, and dependency relations.

use vtr::{Reader, Recorder, SourceOptions, Status, TransactionQuery, Writer};

fn main() -> vtr::Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("transactions.vtr"),
        std::path::PathBuf::from,
    );
    let mut writer = Writer::create(&path)?;
    let core = writer.scope("cpu", None)?;
    let instructions = writer
        .transactions::<[u64; 2]>(SourceOptions::new("instructions", Some(core)), "PIPELINE")?;
    let depends_on = writer.intern("depends_on")?;
    let fetch = writer.intern("fetch")?;
    let execute = writer.intern("execute")?;
    let lane = writer.intern("0")?;
    let mut previous = None;
    for serial in 0..100 {
        writer.advance_time(serial)?;
        let instruction = writer.begin_deferred(instructions, None)?;
        writer.embedded_stage(
            instruction.owner(),
            serial,
            fetch,
            lane,
            Some(serial + 1),
            vec![],
        )?;
        writer.embedded_stage(
            instruction.owner(),
            serial + 1,
            execute,
            lane,
            Some(serial + 3),
            vec![],
        )?;
        if let Some(previous) = previous {
            writer.relate(depends_on, previous, instruction.id(), vec![])?;
        }
        writer.end_at_with_payload(
            instruction,
            serial + 3,
            &[serial, 0x1000 + serial * 4],
            Status::Ok,
        )?;
        previous = Some(instruction.id());
    }
    writer.close()?;

    let reader = Reader::open(path)?;
    reader.transactions(
        TransactionQuery {
            begin: 10,
            end: 12,
            ..TransactionQuery::default()
        },
        |record| {
            let [serial, pc] = record
                .payload()
                .decode::<[u64; 2]>()
                .expect("instruction payload");
            println!(
                "instruction {serial}, pc {pc:#x}: {}..{}",
                record.begin(),
                record.end()
            );
            for stage in record.stages() {
                println!(
                    "  {}: {}..{:?}",
                    reader.string(stage.name).expect("stage name"),
                    stage.begin,
                    stage.end
                );
            }
            true
        },
    )?;
    let report = reader.validate()?;
    assert_eq!(report.relations, 99);
    Ok(())
}
