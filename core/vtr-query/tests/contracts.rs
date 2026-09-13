use vtr_query::{Budget, Cancellation, Error, Grid, Interval, TimeBound};

#[test]
fn time_domain_and_record_boundaries() {
    let all = Interval::new(0, TimeBound::AfterMax).unwrap();
    assert!(all.contains(0));
    assert!(all.contains(u64::MAX));
    assert_eq!(
        TimeBound::from_wide(1u128 << 64).unwrap(),
        TimeBound::AfterMax
    );
    assert!(TimeBound::from_wide((1u128 << 64) + 1).is_err());
    assert!(Interval::new(2, TimeBound::Tick(1)).is_err());

    let query = Interval::new(10, TimeBound::Tick(20)).unwrap();
    for (begin, end, expected) in [
        (0, 100, true),
        (0, 10, false),
        (20, 30, false),
        (10, 10, true),
        (19, 19, true),
        (20, 20, false),
        (10, 20, true),
        (0, 0, false),
    ] {
        assert_eq!(query.overlaps_record(begin, end).unwrap(), expected);
    }
    assert!(query.overlaps_record(12, 11).is_err());
    let empty = Interval::new(15, TimeBound::Tick(15)).unwrap();
    assert!(!empty.overlaps_record(0, 100).unwrap());
    assert!(!empty.contains(15));
}

#[test]
fn grids_normalize_across_pans_and_include_maximum_time() {
    let a = Grid::covering(Interval::new(101, TimeBound::Tick(149)).unwrap(), 4)
        .unwrap()
        .unwrap();
    let b = Grid::covering(Interval::new(103, TimeBound::Tick(151)).unwrap(), 4)
        .unwrap()
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(a, Grid::new(96, 4, 4).unwrap());
    assert_eq!(a.first_bin(), 6);
    assert_eq!(
        a.bin(3).unwrap(),
        Interval::new(144, TimeBound::Tick(160)).unwrap()
    );
    assert!(a.bin(4).is_err());
    assert!(Grid::new(97, 4, 4).is_err());
    assert!(Grid::new(0, 65, 1).is_err());
    assert!(Grid::new(0, 64, 2).is_err());
    assert!(Grid::new(0, 0, 0).is_err());
    assert_eq!(
        Grid::new(0, 64, 1).unwrap().interval(),
        Interval::new(0, TimeBound::AfterMax).unwrap()
    );
    assert_eq!(
        Grid::new(u64::MAX, 0, 1).unwrap().interval(),
        Interval::new(u64::MAX, TimeBound::AfterMax).unwrap()
    );
    assert!(Grid::new(u64::MAX, 0, 2).is_err());
}

#[test]
fn normalized_grid_is_smallest_fitting_level() {
    let boundaries = [0, 1, 2, 15, 16, 17, 255, 256, 257, u64::MAX - 1, u64::MAX];
    for start in boundaries {
        for end in boundaries
            .into_iter()
            .map(TimeBound::Tick)
            .chain([TimeBound::AfterMax])
        {
            if end.wide() < u128::from(start) {
                continue;
            }
            let interval = Interval::new(start, end).unwrap();
            for cap in [1, 2, 3, 4, 255, u32::MAX] {
                let grid = Grid::covering(interval, cap).unwrap();
                if interval.is_empty() {
                    assert!(grid.is_none());
                    continue;
                }
                let grid = grid.unwrap();
                assert!(grid.count() <= cap);
                assert!(grid.start() <= start);
                assert!(grid.interval().end() >= end);
                if grid.level() > 0 {
                    let width = 1u128 << (grid.level() - 1);
                    let count = end.wide().div_ceil(width) - u128::from(start) / width;
                    assert!(count > u128::from(cap));
                }
            }
        }
    }
}

#[test]
fn reservations_follow_shared_lifetimes_and_reject_overflow() {
    let budget = Budget::new(100);
    let owner = budget.reserve(60).unwrap();
    let pin = owner.clone();
    drop(owner);
    assert_eq!(budget.used(), 60);
    assert_eq!(pin.bytes(), 60);
    assert!(budget.reserve(41).is_err());
    let second = budget.reserve(40).unwrap();
    assert_eq!(budget.used(), 100);
    drop(pin);
    assert_eq!(budget.used(), 40);
    drop(second);
    assert_eq!(budget.used(), 0);
    let large = Budget::new(usize::MAX);
    let _reservation = large.reserve(usize::MAX).unwrap();
    assert!(large.reserve(1).is_err());
}

#[test]
fn concurrent_reservations_cannot_exceed_ceiling() {
    let budget = Budget::new(8);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(32));
    std::thread::scope(|scope| {
        for _ in 0..32 {
            let budget = budget.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                let reservation = budget.reserve(1);
                barrier.wait();
                assert_eq!(budget.used(), 8);
                barrier.wait();
                drop(reservation);
            });
        }
    });
    assert_eq!(budget.used(), 0);
}

#[test]
fn cancellation_is_shared_and_idempotent() {
    let token = Cancellation::default();
    assert!(token.check().is_ok());
    let worker = token.clone();
    token.cancel();
    token.cancel();
    assert_eq!(worker.check(), Err(Error::Cancelled));
}
