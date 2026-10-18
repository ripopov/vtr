//! Minimal waveform round trip.

use vtr::{Direction, Logic, LogicStates, Reader, SignalKind, Timescale, VarType, Writer};

fn main() -> vtr::Result<()> {
    let temporary = tempfile::tempdir()?;
    let path = std::env::args_os().nth(1).map_or_else(
        || temporary.path().join("waveform.vtr"),
        std::path::PathBuf::from,
    );
    // Configure metadata before creating the file. Timescale defaults to picoseconds;
    // named units and Exponent(i8) support other decimal powers of seconds.
    let mut writer = Writer::builder()
        .timescale(Timescale::Nanoseconds)
        .create(&path)?;
    // add_module, add_wire, and add_reg simplify common declarations.
    // Wires and registers declared with these helpers use four-state logic.
    let top = writer.add_module(None, "top", "counter")?;
    let count = writer.add_reg(Some(top), "count", 8, Direction::Output)?;
    let ready = writer.add_wire(Some(top), "ready", 1, Direction::Output)?;
    // add_var also returns the hierarchy node ID. Choose LogicStates::Two or
    // LogicStates::Nine when a signal needs a different logic alphabet.
    let (_valid_node, valid) = writer.add_var(
        Some(top),
        "valid",
        VarType::Wire,
        Direction::Output,
        SignalKind::bits(1, LogicStates::Four),
    )?;

    // Waveform time must be non-decreasing. Transaction and log timestamps are
    // independent and may arrive out of order, as in temporally decoupled TLM
    // simulations. Transactions are stored in end_tx call order;
    // Reader::visit_transactions returns file order without timestamp sorting.
    // Time-window queries still find matching intervals. Sort explicitly when
    // chronological iteration is needed (see docs/SPEC.md, section 7.2).
    writer.set_time(0)?;
    writer.emit_bit(ready, Logic::One)?;
    writer.emit_bit(valid, Logic::X)?;
    writer.emit_u64(count, 0)?;
    writer.set_time(10)?;
    writer.emit_u64(count, 42)?;
    // Explicit close reports write errors that Drop cannot return.
    writer.close()?;

    // Reader is Send + Sync. Do not modify or truncate the file while a reader
    // memory-maps it; see the safety contract in Reader::open_with.
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
