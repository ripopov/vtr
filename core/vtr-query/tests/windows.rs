#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use vtr_query::{
    native::Window,
    wave::{Bytes, Change, Limits, Predecessor, Sample, Value, WindowPage},
    Budget, Cancellation, Error, Interval, TimeBound,
};

#[test]
fn exact_pages_keep_boundaries_payloads_and_shared_predecessor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("window.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            block_records: 4,
            dedup: false,
            background: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_bits("s", 64, 2);
    for i in 0..40 {
        writer.set_time(i / 5).unwrap();
        writer.emit_u64(signal, i).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(4096);
    let limits = Limits {
        records: 2,
        bytes: 1024,
        work: 7,
    };
    let mut window = Window::new(
        &reader,
        signal.0,
        Interval::new(2, TimeBound::Tick(5)).unwrap(),
        limits,
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    let mut values = Vec::new();
    let mut predecessor = None;
    for _ in 0..100 {
        let page = window.next_page().unwrap();
        assert!(page.changes().len() <= 2);
        if let Some(previous) = &predecessor {
            assert!(std::sync::Arc::ptr_eq(previous, &page.predecessor));
        }
        predecessor = Some(page.predecessor.clone());
        match &page.predecessor.sample {
            Sample::Known(Value::Bits { data, .. }) => {
                assert_eq!(u64::from_le_bytes(data.as_slice().try_into().unwrap()), 9)
            }
            other => panic!("unexpected predecessor: {other:?}"),
        }
        for change in page.changes() {
            assert!((2..5).contains(&change.time));
            match &change.value {
                Value::Bits { data, .. } => {
                    values.push(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
                }
                _ => panic!("expected bits"),
            }
        }
        if page.complete {
            break;
        }
    }
    assert_eq!(values, (10..25).collect::<Vec<_>>());
    drop(window);
    drop(predecessor);
    assert_eq!(budget.used(), 0);
}

#[test]
fn byte_full_pages_retry_the_unconsumed_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bytes.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_var(
        "text",
        vtr::VarType::String,
        vtr::Direction::Implicit,
        vtr::SignalKind::VarLen,
    );
    for _ in 0..4 {
        writer.emit_varlen(signal, &[7; 100]).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let mut narrow = Window::new(
        &reader,
        signal.0,
        Interval::new(0, TimeBound::AfterMax).unwrap(),
        Limits {
            records: 128,
            work: 128,
            bytes: std::mem::size_of::<WindowPage>()
                + std::mem::size_of::<Predecessor>()
                + std::mem::size_of::<Change>()
                + Bytes::retained_size(100).unwrap(),
        },
        Budget::new(10000),
        Cancellation::default(),
    )
    .unwrap();
    let mut narrow_count = 0;
    loop {
        let page = narrow.next_page().unwrap();
        assert!(page.changes().len() <= 1);
        narrow_count += page.changes().len();
        assert!(narrow_count <= 4);
        if page.complete {
            break;
        }
    }
    assert_eq!(narrow_count, 4);
    // Capacity for four fixed records but only one payload. Byte admission,
    // rather than record count, must cause the page boundary.
    let bytes = std::mem::size_of::<WindowPage>()
        + std::mem::size_of::<Predecessor>()
        + 4 * std::mem::size_of::<Change>()
        + Bytes::retained_size(100).unwrap();
    let limits = Limits {
        bytes,
        records: 4,
        work: 100,
    };
    let budget = Budget::new(10000);
    let token = Cancellation::default();
    let mut window = Window::new(
        &reader,
        signal.0,
        Interval::new(0, TimeBound::AfterMax).unwrap(),
        limits,
        budget.clone(),
        token.clone(),
    )
    .unwrap();
    let mut count = 0;
    for _ in 0..10 {
        let page = window.next_page().unwrap();
        assert!(page.changes().len() <= 1);
        count += page.changes().len();
        if page.complete {
            break;
        }
    }
    assert_eq!(count, 4);
    token.cancel();
    assert!(matches!(window.next_page(), Err(Error::Cancelled)));
    drop(window);
    assert_eq!(budget.used(), 0);
}

#[test]
fn pinned_pages_exhaust_budget_without_losing_the_next_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_var(
        "text",
        vtr::VarType::String,
        vtr::Direction::Implicit,
        vtr::SignalKind::VarLen,
    );
    for i in 0..4 {
        writer.emit_varlen(signal, &[i; 100]).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(
        std::mem::size_of::<WindowPage>()
            + std::mem::size_of::<Predecessor>()
            + 4 * std::mem::size_of::<Change>()
            + Bytes::retained_size(100).unwrap(),
    );
    let mut window = Window::new(
        &reader,
        signal.0,
        Interval::new(0, TimeBound::AfterMax).unwrap(),
        Limits {
            records: 4,
            bytes: 4096,
            work: 256,
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    for i in 0..4 {
        let page = window.next_page().unwrap();
        assert_eq!(page.changes().len(), 1);
        match &page.changes()[0].value {
            Value::Bytes(data) => assert_eq!(data.as_slice(), &[i; 100]),
            _ => panic!("expected bytes"),
        }
        assert!(matches!(window.next_page(), Err(Error::ResourceLimit)));
        assert!(budget.used() <= budget.limit());
    }
    let page = window.next_page().unwrap();
    assert!(page.complete && page.changes().is_empty());
    drop(window);
    assert!(budget.used() > 0);
    drop(page);
    assert_eq!(budget.used(), 0);
}

#[test]
fn dense_summary_counts_events_without_materializing_exact_pages() {
    use vtr_query::{native::Summary, Grid};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("summary.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            block_records: 17,
            background: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_var(
        "real",
        vtr::VarType::Real,
        vtr::Direction::Implicit,
        vtr::SignalKind::Real,
    );
    for i in 0..1000u64 {
        writer.set_time(i / 10).unwrap();
        writer.emit_real(signal, (i % 13) as f64).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let grid = Grid::new(0, 5, 4).unwrap();
    let budget = Budget::new(10000);
    let mut summary = Summary::new(
        &reader,
        signal.0,
        grid,
        Limits {
            records: 1,
            bytes: 2048,
            work: 7,
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    let mut bins = Vec::new();
    let mut empty = 0;
    let mut done = false;
    for _ in 0..1000 {
        let page = summary.next_page().unwrap();
        assert_eq!(page.offset as usize, bins.len());
        if page.bins().is_empty() {
            empty += 1;
        }
        bins.extend(page.bins().iter().cloned());
        assert!(budget.used() <= budget.limit());
        if page.complete {
            done = true;
            break;
        }
    }
    assert!(done && empty > 0);
    assert_eq!(
        bins.iter().map(|b| b.changes).collect::<Vec<_>>(),
        [320, 320, 320, 40]
    );
    for bin in &bins {
        let end = match bin.interval.end() {
            TimeBound::Tick(t) => t,
            _ => unreachable!(),
        };
        let held: Vec<f64> = (bin.interval.start()..end)
            .map(
                |time| match reader.value_at(signal, time).unwrap().borrow() {
                    vtr::SignalValue::Real(value) => value,
                    _ => unreachable!(),
                },
            )
            .collect();
        let min = held.iter().copied().min_by(f64::total_cmp).unwrap();
        let max = held.iter().copied().max_by(f64::total_cmp).unwrap();
        assert_eq!(bin.real.as_ref().unwrap().finite_min, Some(min.to_bits()));
        assert_eq!(bin.real.as_ref().unwrap().finite_max, Some(max.to_bits()));
    }
    drop(summary);
    drop(bins);
    assert_eq!(budget.used(), 0);
}

#[test]
fn summary_delivers_completed_bins_before_exhausting_shared_budget() {
    use vtr_query::{
        native::{Summary, SummaryPage},
        summary::WaveBin,
        Grid,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("summary-quota.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_var(
        "real",
        vtr::VarType::Real,
        vtr::Direction::Implicit,
        vtr::SignalKind::Real,
    );
    writer.emit_real(signal, 7.0).unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(
        std::mem::size_of::<SummaryPage>()
            + 4 * std::mem::size_of::<std::sync::Arc<WaveBin>>()
            + std::mem::size_of::<WaveBin>(),
    );
    let mut summary = Summary::new(
        &reader,
        signal.0,
        Grid::new(0, 4, 4).unwrap(),
        Limits {
            bytes: 8192,
            records: 4,
            work: 4096,
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    for offset in 0..4 {
        let page = summary.next_page().unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.bins().len(), 1);
        assert_eq!(page.bins()[0].changes, u64::from(offset == 0));
        assert_eq!(page.complete, offset == 3);
    }
    drop(summary);
    assert_eq!(budget.used(), 0);
}
