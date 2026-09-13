use vtr::*;

#[test]
fn paged_scan_matches_reference_at_every_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.vtr");
    let opts = WriterOptions { block_records: 12, group_size: 2, background: false, dedup: false, ..Default::default() };
    let mut writer = Writer::create_with(&path, opts).unwrap();
    let (_, bit) = writer.add_bits("bit", 1, 9);
    let (_, wide) = writer.add_bits("wide", 2048, 4);
    let (_, real) = writer.add_var("real", VarType::Real, Direction::Implicit, SignalKind::Real);
    let (_, text) = writer.add_var("text", VarType::String, Direction::Implicit, SignalKind::VarLen);
    let (_, quiet) = writer.add_bits("quiet", 1, 4);
    writer.add_alias("alias", VarType::Wire, Direction::Implicit, wide).unwrap();
    for i in 0..60u64 {
        // A timestamp spans multiple independently decoded blocks.
        writer.set_time(if i < 50 { i / 10 } else { u64::MAX }).unwrap();
        writer.emit_u64(bit, i % 2).unwrap();
        writer.emit_u64(wide, i).unwrap();
        writer.emit_real(real, f64::from_bits(0x7ff8000000000000 + i)).unwrap();
        writer.emit_varlen(text, format!("variable-{i}").as_bytes()).unwrap();
    }
    writer.close().unwrap();
    let reader = Reader::open(&path).unwrap();
    assert!(reader.block_count() > 2);
    for signal in [bit, wide, real, text, quiet, reader.find_signal("alias", '.').unwrap()] {
        let reference = reader.load_signal(signal).unwrap();
        for (start, end) in [(0, u64::MAX), (0, 0), (2, 3), (5, 6), (u64::MAX, u64::MAX)] {
            let expected: Vec<_> = (0..reference.len()).filter(|&i| start <= reference.times()[i] && reference.times()[i] <= end).collect();
            for work in [1, 2, 7, 256] {
                for page in [1, 3, usize::MAX] {
                    let mut scan = reader.change_scan(signal, start, end).unwrap();
                    let mut seen = 0;
                    let mut complete = false;
                    for _ in 0..1000 {
                        let mut count = 0;
                        complete = scan.scan(work, |time, value| {
                            let i = expected[seen];
                            assert_eq!(time, reference.times()[i]);
                            match (value, reference.get(i)) {
                                (SignalValue::Real(a), SignalValue::Real(b)) => assert_eq!(a.to_bits(), b.to_bits()),
                                (a, b) => assert_eq!(a.to_ascii(), b.to_ascii()),
                            }
                            seen += 1;
                            count += 1;
                            if count < page { ScanAction::Continue } else { ScanAction::StopAfter }
                        }).unwrap();
                        assert!(count <= work && count <= page);
                        if complete { break; }
                    }
                    assert!(complete, "scan made no bounded progress");
                    assert_eq!(seen, expected.len());
                    assert!(scan.scan(1, |_, _| panic!("completed scan delivered again")).unwrap());
                }
            }
        }
    }
    assert!(reader.change_scan(SignalId(u32::MAX), 0, 0).is_err());
    assert!(reader.change_scan(bit, 2, 1).is_err());
    assert!(reader.change_scan(bit, 0, 1).unwrap().scan(0, |_, _| ScanAction::Continue).is_err());
}
