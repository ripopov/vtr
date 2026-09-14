use vtr::*;

#[test]
fn bounded_histories_match_bulk_and_survive_the_loader() {
    let path = std::env::temp_dir().join(format!("vtr-bounded-history-{}.vtr", std::process::id()));
    let mut writer = Writer::create_with(&path, WriterOptions {
        block_records: 8, background: false, dedup: false, ..Default::default()
    }).unwrap();
    let (_, bits) = writer.add_bits("bits", 129, 9);
    let (_, quiet) = writer.add_bits("quiet", 8, 4);
    let (_, real) = writer.add_var("real", VarType::Real, Direction::Implicit, SignalKind::Real);
    let (_, text) = writer.add_var("text", VarType::String, Direction::Implicit, SignalKind::VarLen);
    for i in 0..48 {
        writer.set_time(if i == 47 { u64::MAX } else { i / 2 }).unwrap();
        writer.emit_u64(bits, i).unwrap();
        writer.emit_real(real, i as f64 + 0.25).unwrap();
        writer.emit_varlen(text, &vec![i as u8; i as usize * 7]).unwrap();
    }
    writer.close().unwrap();
    let reader = Reader::open(&path).unwrap();
    assert!(reader.block_count() > 1);
    assert!(reader.history_load(SignalId(u32::MAX), 0).is_err());
    for signal in [bits, quiet, real, text] {
        let expected = reader.load_signal(signal).unwrap();
        let mut load = reader.history_load(signal, 1 << 20).unwrap();
        assert!(load.advance(0).is_err());
        let mut pending = 0;
        let actual = loop {
            match load.advance(1).unwrap() {
                HistoryProgress::Pending => pending += 1,
                HistoryProgress::Complete(history) => break history,
                HistoryProgress::BudgetExceeded => panic!("unexpected refusal"),
            }
        };
        assert!(pending > 0);
        assert!(actual.retained_bytes() <= 1 << 20);
        let HistoryProgress::Complete(repeated) = load.advance(1).unwrap() else { panic!("completion lost") };
        assert!(std::ptr::eq(actual.times(), repeated.times()));
        drop(load);
        assert_eq!(actual.initial(), expected.initial());
        assert_eq!(actual.times(), expected.times());
        for i in 0..expected.len() { assert_eq!(actual.get(i), expected.get(i)); }
        let mut refused = reader.history_load(signal, 0).unwrap();
        assert!(matches!(refused.advance(1).unwrap(), HistoryProgress::BudgetExceeded));
        assert!(matches!(refused.advance(1).unwrap(), HistoryProgress::BudgetExceeded));
    }
    // A budget that admits initial storage must still refuse a growing history,
    // without ever presenting its already-built prefix as a complete history.
    let mut small = reader.history_load(text, 512).unwrap();
    loop {
        match small.advance(1).unwrap() {
            HistoryProgress::Pending => {},
            HistoryProgress::BudgetExceeded => break,
            HistoryProgress::Complete(_) => panic!("oversize history accepted"),
        }
    }
    drop(small);
    drop(reader);
    std::fs::remove_file(path).unwrap();
}
