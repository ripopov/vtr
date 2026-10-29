//! Keyed instructions, grouped retirement, and an unfinished detached instruction.

use vtr::{Reader, Recorder, SourceOptions, Status, Tracker, TransactionQuery, Writer};

fn main() -> vtr::Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("tracking.vtr"),
        std::path::PathBuf::from,
    );
    let mut writer = Writer::create(&path)?;
    let instructions =
        writer.transactions::<()>(SourceOptions::new("instructions", None), "PIPELINE")?;
    let mut tracker = Tracker::new(&mut writer, instructions, "instructions")?;
    let decoded = tracker.keyspace("decoded")?;
    let retirement = tracker.keyspace("retirement")?;
    for key in 0..3 {
        tracker.open(&mut writer, decoded, key, "decode", 0, None)?;
    }
    writer.advance_time(1)?;
    assert_eq!(
        tracker.bind_oldest(&mut writer, decoded, 2, retirement, 7, 1)?,
        2
    );
    tracker.stage(&mut writer, retirement, 7, "execute", 1)?;
    tracker.label(&mut writer, retirement, 7, "fused pair")?;
    writer.advance_time(2)?;
    assert_eq!(tracker.close(&mut writer, retirement, 7, Status::Ok, 2)?, 2);
    tracker.label(&mut writer, decoded, 2, "pending")?;
    writer.advance_time(3)?;
    tracker.detach(&mut writer)?;
    assert!(tracker.diagnostics().is_empty());
    writer.close()?;
    let reader = Reader::open(path)?;
    reader.validate()?;
    let mut complete = 0;
    let mut open = 0;
    reader.transactions(TransactionQuery::default(), |record| {
        println!(
            "instruction {}: {}..{} {}",
            record.id(),
            record.begin(),
            record.end(),
            record.status()
        );
        complete += usize::from(record.status() == Status::Ok);
        open += usize::from(record.status() == Status::Open);
        true
    })?;
    assert_eq!((complete, open), (2, 1));
    Ok(())
}
