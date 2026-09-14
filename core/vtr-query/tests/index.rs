#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use vtr_query::{
    native::Summary,
    native_index::SignalIndex,
    wave::{Direction, Limits, Sample, Value},
    Budget, Cancellation, Error, Grid, Interval, TimeBound,
};

fn sample(sample: &Sample) -> String {
    match sample {
        Sample::BackendDefault(vtr_query::wave::Kind::Bits { width, states: 2 }) => {
            format!("{width}:2:{:?}", vec![0u8; (*width as usize).div_ceil(8)])
        }
        Sample::BackendDefault(vtr_query::wave::Kind::Bytes) => "bytes:[]".into(),
        Sample::BackendDefault(kind) => format!("default:{kind:?}"),
        Sample::Event => "event".into(),
        Sample::Known(value) => value_text(value),
    }
}
fn value_text(value: &Value) -> String {
    match value {
        Value::Bits {
            width,
            states,
            data,
        } => format!("{width}:{states}:{:?}", data.as_slice()),
        Value::Bytes(bytes) => format!("bytes:{:?}", bytes.as_slice()),
        Value::Real(bits) => format!("real:{bits}"),
    }
}

#[test]
fn indexed_discrete_bins_match_cold_reduction_with_same_time_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            block_records: 4,
            background: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, bits) = writer.add_bits("bits", 8, 2);
    let (_, event) = writer.add_var(
        "event",
        vtr::VarType::Event,
        vtr::Direction::Implicit,
        vtr::SignalKind::Bits {
            width: 1,
            states: 2,
        },
    );
    let (_, bytes) = writer.add_var(
        "bytes",
        vtr::VarType::String,
        vtr::Direction::Implicit,
        vtr::SignalKind::VarLen,
    );
    for i in 0..96 {
        writer.set_time(i / 3 + 3).unwrap();
        writer.emit_u64(bits, i).unwrap();
        writer.emit_u64(event, i % 2).unwrap();
        writer.emit_varlen(bytes, &[i as u8, 0xff]).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(1 << 20);
    for (signal, ty) in [
        (bits, vtr::VarType::Wire),
        (event, vtr::VarType::Event),
        (bytes, vtr::VarType::String),
    ] {
        let history = reader.load_signal(signal).unwrap();
        let index = SignalIndex::new(&history, ty);
        for level in 0..=6 {
            let grid = Grid::new(0, level, 64 >> level).unwrap();
            let mut cold = Summary::new(
                &reader,
                signal.0,
                grid,
                Limits {
                    work: 7,
                    records: 4,
                    bytes: 8192,
                },
                budget.clone(),
                Cancellation::default(),
            )
            .unwrap();
            loop {
                let page = cold.next_page().unwrap();
                for expected in page.bins() {
                    let actual = index
                        .discrete_summary(expected.interval, 8192, &budget)
                        .unwrap();
                    assert_eq!(actual.changes, expected.changes);
                    assert_eq!(sample(&actual.entry), sample(&expected.entry));
                    assert_eq!(sample(&actual.exit), sample(&expected.exit));
                    for (a, b) in [
                        (&actual.first, &expected.first),
                        (&actual.last, &expected.last),
                    ] {
                        assert_eq!(
                            a.as_ref().map(|c| (c.time, value_text(&c.value))),
                            b.as_ref().map(|c| (c.time, value_text(&c.value)))
                        );
                    }
                    assert!(actual.delivery_bytes() <= 8192);
                }
                if page.complete {
                    break;
                }
            }
        }
    }
    assert_eq!(budget.used(), 0);
}

#[test]
fn indexed_navigation_and_samples_cover_full_timestamp_domain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edges.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_bits("s", 8, 2);
    for (time, value) in [(0, 1), (3, 2), (3, 3), (u64::MAX, 4)] {
        writer.set_time(time).unwrap();
        writer.emit_u64(signal, value).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let history = reader.load_signal(signal).unwrap();
    drop(reader);
    let index = SignalIndex::new(&history, vtr::VarType::Wire);
    let budget = Budget::new(4096);
    for time in [0, 1, 3, 4, u64::MAX - 1, u64::MAX] {
        assert_eq!(
            index.find_change(time, Direction::Previous),
            history.times().iter().rev().copied().find(|&t| t < time)
        );
        assert_eq!(
            index.find_change(time, Direction::Next),
            history.times().iter().copied().find(|&t| t > time)
        );
    }
    assert_eq!(sample(&index.sample(3, 1024, &budget).unwrap()), "8:2:[3]");
    let final_bin = index
        .discrete_summary(
            Interval::new(u64::MAX, TimeBound::AfterMax).unwrap(),
            1024,
            &budget,
        )
        .unwrap();
    assert_eq!(final_bin.changes, 1);
    assert_eq!(sample(&final_bin.exit), "8:2:[4]");
    drop(final_bin);
    assert_eq!(budget.used(), 0);
    assert!(matches!(
        index.sample(3, 0, &budget),
        Err(Error::ResourceLimit)
    ));
    assert!(matches!(
        index.discrete_summary(Interval::new(0, TimeBound::AfterMax).unwrap(), 1, &budget),
        Err(Error::ResourceLimit)
    ));
    assert_eq!(budget.used(), 0);
}

#[test]
fn real_samples_preserve_bits_without_inventing_extrema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("real.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_var(
        "r",
        vtr::VarType::Real,
        vtr::Direction::Implicit,
        vtr::SignalKind::Real,
    );
    let bits = 0x7ff800000000beef;
    writer.emit_real(signal, f64::from_bits(bits)).unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let history = reader.load_signal(signal).unwrap();
    let index = SignalIndex::new(&history, vtr::VarType::Real);
    let budget = Budget::new(4096);
    assert!(
        matches!(index.sample(0, 1024, &budget).unwrap(), Sample::Known(Value::Real(v)) if v == bits)
    );
    assert!(matches!(
        index.discrete_summary(
            Interval::new(0, TimeBound::AfterMax).unwrap(),
            1024,
            &budget
        ),
        Err(Error::Invalid(_))
    ));
}
