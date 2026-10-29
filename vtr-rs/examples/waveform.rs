//! Waveforms with explicit declarations and producer time.

use vtr::{
    Direction, Reader, Recorder, SignalKind, SourceOptions, TimeWindow, VarType, Writer,
    WriterOptions,
};

fn main() -> vtr::Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("waveform.vtr"),
        std::path::PathBuf::from,
    );
    let mut options = WriterOptions::default();
    options.metadata.timescale = -9;
    let mut writer = Writer::create_with(&path, options)?;
    let top = writer.scope("top", None)?;
    let count = writer.signal(
        SourceOptions::new("count", Some(top)),
        SignalKind::Bits {
            width: 8,
            states: 2,
        },
        VarType::Reg,
        Direction::Output,
    )?;
    let ready = writer.signal(
        SourceOptions::new("ready", Some(top)),
        SignalKind::Bits {
            width: 1,
            states: 4,
        },
        VarType::Wire,
        Direction::Output,
    )?;
    writer.emit_u64(count, 0)?;
    writer.emit_packed(ready, 4, &[2])?;
    writer.advance_time(10)?;
    writer.emit_u64(count, 42)?;
    writer.emit_u64(ready, 1)?;
    writer.advance_time(15)?;
    writer.dump(false)?;
    writer.emit_u64(count, 255)?;
    writer.advance_time(20)?;
    writer.dump(true)?;
    writer.emit_u64(count, 43)?;
    writer.close()?;

    let reader = Reader::open(path)?;
    let count = reader.find_signal("top.count").expect("declared signal");
    assert_eq!(reader.value_at(count, 10)?.borrow().as_u64(), Some(42));
    assert_eq!(reader.value_at(count, 19)?.borrow().as_u64(), Some(42));
    assert_eq!(reader.value_at(count, 20)?.borrow().as_u64(), Some(43));
    reader.changes(Some(count), TimeWindow::default(), |time, _, value| {
        println!("top.count at {time} ns: {}", value.to_ascii());
        true
    })?;
    reader.validate()?;
    Ok(())
}
