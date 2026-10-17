//! Minimal waveform round trip.

use vtr::{Direction, Reader, Timescale, Writer};

fn main() -> vtr::Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("waveform.vtr"),
        std::path::PathBuf::from,
    );
    let mut writer = Writer::builder()
        .timescale(Timescale::Nanoseconds)
        .create(&path)?;
    let top = writer.add_module(None, "top", "counter")?;
    let count = writer.add_reg(Some(top), "count", 8, Direction::Output)?;
    writer.set_time(0)?;
    writer.emit_u64(count, 0)?;
    writer.set_time(10)?;
    writer.emit_u64(count, 42)?;
    writer.close()?;

    let reader = Reader::open(&path)?;
    let count = reader
        .find_signal("top.count", '.')
        .expect("declared signal");
    let value = reader.value_at(count, 10)?;
    assert_eq!(value.borrow().as_u64(), Some(42));
    assert_eq!(reader.changes(count, 0, 10)?.len(), 2);
    println!("top.count at 10 ns: {}", value.to_ascii());
    Ok(())
}
