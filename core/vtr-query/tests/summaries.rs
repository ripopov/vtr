use vtr_query::{
    summary::{BinBuilder, WaveBin},
    wave::{Sample, Value},
    Budget, Interval, TimeBound,
};
fn interval(start: u64, end: u64) -> Interval {
    Interval::new(start, TimeBound::Tick(end)).unwrap()
}
fn real(v: f64) -> Value {
    Value::Real(v.to_bits())
}
fn sample(v: f64) -> Sample {
    Sample::Known(real(v))
}
fn value_bits(sample: &Sample) -> u64 {
    match sample {
        Sample::Known(Value::Real(bits)) => *bits,
        _ => panic!("expected real"),
    }
}
fn reduce(
    start: u64,
    end: u64,
    entry: f64,
    changes: &[(u64, f64)],
    budget: &Budget,
) -> std::sync::Arc<WaveBin> {
    let mut bin = BinBuilder::new(interval(start, end), sample(entry), budget).unwrap();
    for &(time, value) in changes {
        if start <= time && time < end {
            bin.push(time, real(value)).unwrap();
        }
    }
    bin.finish()
}
#[test]
fn real_extrema_use_held_spans_and_count_nonfinite_source_changes() {
    let budget = Budget::new(10000);
    let bin = reduce(
        0,
        8,
        -1000.0,
        &[
            (0, 900.0),
            (0, 2.0),
            (2, f64::NAN),
            (3, f64::INFINITY),
            (4, -2.0),
            (7, f64::NEG_INFINITY),
        ],
        &budget,
    );
    let real = bin.real.as_ref().unwrap();
    assert_eq!(real.finite_min, Some((-2.0f64).to_bits()));
    assert_eq!(real.finite_max, Some(2.0f64.to_bits()));
    assert_eq!(
        (
            real.nan_changes,
            real.positive_infinite_changes,
            real.negative_infinite_changes
        ),
        (1, 1, 1)
    );
    assert_eq!(bin.changes, 6);
    assert_eq!(value_bits(&bin.entry), (-1000.0f64).to_bits());
    assert_eq!(value_bits(&bin.exit), f64::NEG_INFINITY.to_bits());
    let empty = reduce(0, 8, 42.0, &[], &budget);
    assert_eq!(
        empty.real.as_ref().unwrap().finite_min,
        Some(42.0f64.to_bits())
    );
    assert_eq!(empty.changes, 0);
}
#[test]
fn merging_aligned_children_matches_direct_parent_reduction() {
    let budget = Budget::new(100000);
    let changes = [
        (0, 7.0),
        (0, -4.0),
        (1, f64::NAN),
        (3, f64::INFINITY),
        (4, -0.0),
        (4, 0.0),
        (5, 3.0),
        (6, f64::NEG_INFINITY),
        (7, 8.0),
    ];
    let parent = reduce(0, 8, 100.0, &changes, &budget);
    for width in [1, 2, 4] {
        let mut bins = Vec::new();
        let mut entry = 100.0;
        for start in (0..8).step_by(width) {
            let bin = reduce(start, start + width as u64, entry, &changes, &budget);
            entry = f64::from_bits(value_bits(&bin.exit));
            bins.push(bin);
        }
        while bins.len() > 1 {
            bins = bins
                .chunks_exact(2)
                .map(|pair| WaveBin::merge(&pair[0], &pair[1], &budget).unwrap())
                .collect();
        }
        let merged = &bins[0];
        assert_eq!(merged.changes, parent.changes);
        assert_eq!(merged.real, parent.real);
        assert_eq!(value_bits(&merged.entry), value_bits(&parent.entry));
        assert_eq!(value_bits(&merged.exit), value_bits(&parent.exit));
        assert_eq!(
            merged.first.as_ref().unwrap().time,
            parent.first.as_ref().unwrap().time
        );
        assert_eq!(
            merged.last.as_ref().unwrap().time,
            parent.last.as_ref().unwrap().time
        );
    }
    let left = reduce(1, 2, 1.0, &[], &budget);
    let right = reduce(2, 3, 1.0, &[], &budget);
    assert!(WaveBin::merge(&left, &right, &budget).is_err());
}
#[test]
fn event_counts_preserve_repeated_occurrences_without_held_values() {
    let budget = Budget::new(10000);
    let mut bin = BinBuilder::new(
        Interval::new(u64::MAX, TimeBound::AfterMax).unwrap(),
        Sample::Event,
        &budget,
    )
    .unwrap();
    for _ in 0..4 {
        bin.push(u64::MAX, real(1.0)).unwrap();
    }
    let bin = bin.finish();
    assert_eq!(bin.changes, 4);
    assert!(matches!(bin.exit, Sample::Event));
    assert!(bin.real.is_none());
    drop(bin);
    assert_eq!(budget.used(), 0);
}

#[test]
fn summary_samples_are_returned_only_when_the_bin_proves_the_value() {
    let budget = Budget::new(65536);
    for changes in [
        vec![],
        vec![(3, 1.0)],
        vec![(3, 1.0), (7, 2.0)],
        vec![(3, 1.0), (5, 9.0), (7, 2.0)],
        vec![(5, 1.0), (5, 9.0), (5, 2.0)],
    ] {
        let bin = reduce(0, 10, -1.0, &changes, &budget);
        for time in 0..10 {
            if let Some(actual) = bin.sample_at(time) {
                let expected: f64 = changes
                    .iter()
                    .rev()
                    .find(|(t, _)| *t <= time)
                    .map_or(-1.0, |(_, value)| *value);
                assert_eq!(value_bits(&actual), expected.to_bits());
            }
        }
        assert!(bin.sample_at(10).is_none());
        if changes.len() <= 2 {
            assert!((0..10).all(|time| bin.sample_at(time).is_some()));
        }
    }
    let dense = reduce(0, 10, -1.0, &[(3, 1.0), (5, 9.0), (7, 2.0)], &budget);
    assert!(dense.sample_at(2).is_some());
    assert!(dense.sample_at(3).is_none());
    assert!(dense.sample_at(6).is_none());
    assert!(dense.sample_at(7).is_some());
    let mut event = BinBuilder::new(interval(0, 10), Sample::Event, &budget).unwrap();
    event.push(5, real(1.0)).unwrap();
    let event = event.finish();
    assert!((0..10).all(|time| event.sample_at(time).is_none()));
}
