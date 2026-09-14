#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use vtr_query::{
    native_samples::ValuesAt,
    wave::{Bytes, Limits, Sample, SampleAt, SignalTime, Value, ValuesPage},
    Budget, Cancellation, Error,
};

#[test]
fn exact_samples_preserve_request_order_defaults_and_last_same_time_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("samples.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            block_records: 2,
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
    writer.set_time(5).unwrap();
    writer.emit_u64(bits, 1).unwrap();
    writer.emit_u64(bits, 2).unwrap();
    writer.emit_u64(event, 1).unwrap();
    writer.set_time(u64::MAX).unwrap();
    writer.emit_u64(bits, 3).unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let pairs = [
        SignalTime {
            signal: bits.0,
            time: u64::MAX,
        },
        SignalTime {
            signal: event.0,
            time: 5,
        },
        SignalTime {
            signal: bits.0,
            time: 4,
        },
        SignalTime {
            signal: bits.0,
            time: 5,
        },
        SignalTime {
            signal: bits.0,
            time: 5,
        },
        SignalTime {
            signal: event.0,
            time: 0,
        },
    ];
    let budget = Budget::new(4096);
    let mut query = ValuesAt::new(
        &reader,
        &pairs,
        Limits {
            work: 2,
            records: 1,
            bytes: 1024,
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    for (offset, expected) in [Some(3), None, Some(0), Some(2), Some(2), None]
        .into_iter()
        .enumerate()
    {
        let page = query.next_page().unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.complete, offset == pairs.len() - 1);
        assert_eq!(page.samples().len(), 1);
        let sample = &page.samples()[0];
        assert_eq!(sample.pair, pairs[offset]);
        match (&sample.sample, expected) {
            (
                Sample::Known(Value::Bits {
                    width: 8,
                    states: 2,
                    data,
                }),
                Some(value),
            ) => assert_eq!(data.as_slice(), &[value]),
            (Sample::Event, None) => {}
            other => panic!("unexpected sample {other:?}"),
        }
    }
    let final_page = query.next_page().unwrap();
    assert!(final_page.complete);
    assert!(final_page.samples().is_empty());
    drop(final_page);
    drop(query);
    assert_eq!(budget.used(), 0);
}

#[test]
fn byte_pages_retry_wide_samples_and_retain_their_storage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_bits("wide", 2048, 2);
    writer.emit_packed(signal, 2, &[0xa5; 256]).unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(8192);
    let pair = SignalTime {
        signal: signal.0,
        time: 0,
    };
    let pairs = [pair; 4];
    let limits = Limits {
        records: 4,
        work: 4,
        bytes: std::mem::size_of::<ValuesPage>()
            + 4 * std::mem::size_of::<SampleAt>()
            + Bytes::retained_size(256).unwrap(),
    };
    let mut query = ValuesAt::new(
        &reader,
        &pairs,
        limits,
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    let mut retained = None;
    for offset in 0..4 {
        let page = query.next_page().unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.samples().len(), 1);
        assert_eq!(page.complete, offset == 3);
        let Sample::Known(Value::Bits { data, .. }) = &page.samples()[0].sample else {
            panic!("bits");
        };
        assert_eq!(data.as_slice(), &[0xa5; 256]);
        retained = Some(data.clone());
    }
    drop(query);
    assert_eq!(budget.used(), Bytes::retained_size(256).unwrap());
    drop(retained);
    assert_eq!(budget.used(), 0);

    // A large record limit must not reserve empty row slots at the expense of
    // a value that would fit with a single populated row.
    let mut narrow = ValuesAt::new(
        &reader,
        &pairs,
        Limits {
            records: 128,
            work: 128,
            bytes: std::mem::size_of::<ValuesPage>()
                + std::mem::size_of::<SampleAt>()
                + Bytes::retained_size(256).unwrap(),
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    for offset in 0..pairs.len() {
        let page = narrow.next_page().unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.samples().len(), 1);
        assert_eq!(page.complete, offset + 1 == pairs.len());
    }
}

#[test]
fn cancellation_and_budget_failure_do_not_consume_pairs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admission.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, signal) = writer.add_bits("s", 1, 2);
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(4096);
    let cancellation = Cancellation::default();
    let pairs = [SignalTime {
        signal: signal.0,
        time: 0,
    }; 2];
    let mut query = ValuesAt::new(
        &reader,
        &pairs,
        Limits {
            work: 1,
            ..Limits::default()
        },
        budget.clone(),
        cancellation.clone(),
    )
    .unwrap();
    let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert_eq!(query.next_page().unwrap_err(), Error::ResourceLimit);
    drop(pressure);
    let page = query.next_page().unwrap();
    assert_eq!(page.offset, 0);
    assert_eq!(page.samples().len(), 1);
    assert!(!page.complete);
    cancellation.cancel();
    assert_eq!(query.next_page().unwrap_err(), Error::Cancelled);
    drop(query);
    drop(page);
    assert_eq!(budget.used(), 0);
    assert!(ValuesAt::new(
        &reader,
        &[
            pairs[0],
            SignalTime {
                signal: u32::MAX,
                time: 0
            }
        ],
        Limits::default(),
        budget.clone(),
        Cancellation::default()
    )
    .is_err());
    assert!(ValuesAt::new(
        &reader,
        &vec![pairs[0]; 4097],
        Limits::default(),
        budget.clone(),
        Cancellation::default()
    )
    .is_err());
    let mut empty = ValuesAt::new(
        &reader,
        &[],
        Limits::default(),
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    assert!(empty.next_page().unwrap().complete);
    drop(empty);
    assert_eq!(budget.used(), 0);
}
